//! Request pacing: decides how long to wait before the next Discord request.
//!
//! Discord says not to hard-code limits and to follow response headers
//! (`X-RateLimit-Remaining`, `X-RateLimit-Reset-After`) and `Retry-After` on
//! 429 (docs: topics/rate-limits). We therefore:
//!   * keep a polite minimum gap between requests of each kind (the "speed"),
//!   * if a response says the bucket is empty, wait for its reset,
//!   * on a 429, wait `retry_after` plus a margin and slow down further
//!     (a penalty multiplier that decays again after successes).
//! Jitter only avoids perfectly regular request timing; it is not an attempt to
//! disguise anything.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::discord::RateInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Speed {
    #[default]
    Gentle,
    Normal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PacingConfig {
    pub list_gap: Duration,
    pub delete_gap: Duration,
    /// Extra time added after a 429's `retry_after`.
    pub margin: Duration,
    /// First network-error retry delay; doubles per attempt (capped at 32x).
    pub backoff_base: Duration,
    pub jitter: bool,
}

impl Speed {
    pub fn config(self) -> PacingConfig {
        match self {
            Speed::Gentle => PacingConfig {
                list_gap: Duration::from_millis(800),
                delete_gap: Duration::from_millis(1500),
                margin: Duration::from_millis(500),
                backoff_base: Duration::from_secs(1),
                jitter: true,
            },
            Speed::Normal => PacingConfig {
                list_gap: Duration::from_millis(400),
                delete_gap: Duration::from_millis(700),
                margin: Duration::from_millis(250),
                backoff_base: Duration::from_secs(1),
                jitter: true,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    ListMessages,
    DeleteMessage,
}

const MAX_PENALTY: f64 = 4.0;
const PENALTY_STEP: f64 = 1.5;
const PENALTY_DECAY: f64 = 0.9;

#[derive(Debug)]
pub struct Pacer {
    cfg: PacingConfig,
    list_ready: Instant,
    delete_ready: Instant,
    penalty: f64,
    rng: u64,
}

impl Pacer {
    pub fn new(cfg: PacingConfig, now: Instant) -> Self {
        // Not cryptographic: just spreads timing a little.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
        Self { cfg, list_ready: now, delete_ready: now, penalty: 1.0, rng: seed }
    }

    #[cfg(test)]
    pub fn penalty(&self) -> f64 {
        self.penalty
    }

    fn ready_mut(&mut self, route: Route) -> &mut Instant {
        match route {
            Route::ListMessages => &mut self.list_ready,
            Route::DeleteMessage => &mut self.delete_ready,
        }
    }

    /// How long to wait (zero if we may go now) before sending on `route`.
    pub fn wait_for(&self, route: Route, now: Instant) -> Duration {
        let ready = match route {
            Route::ListMessages => self.list_ready,
            Route::DeleteMessage => self.delete_ready,
        };
        ready.saturating_duration_since(now)
    }

    fn jitter_factor(&mut self) -> f64 {
        if !self.cfg.jitter {
            return 1.0;
        }
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        let unit = (self.rng >> 11) as f64 / (1u64 << 53) as f64; // [0,1)
        0.85 + unit * 0.30 // +/- 15 %
    }

    /// Record a successful response on `route`.
    pub fn on_success(&mut self, route: Route, info: &RateInfo, now: Instant) {
        self.penalty = (self.penalty * PENALTY_DECAY).max(1.0);
        let gap = match route {
            Route::ListMessages => self.cfg.list_gap,
            Route::DeleteMessage => self.cfg.delete_gap,
        };
        let mut wait = gap.mul_f64(self.penalty * self.jitter_factor());
        if info.remaining == Some(0) {
            if let Some(ms) = info.reset_after_ms {
                wait = wait.max(Duration::from_millis(ms) + Duration::from_millis(50));
            }
        }
        *self.ready_mut(route) = now + wait;
    }

    /// Record a 429: wait `retry_after_ms` + margin, and slow down.
    pub fn on_rate_limited(&mut self, route: Route, retry_after_ms: u64, now: Instant) {
        self.penalty = (self.penalty * PENALTY_STEP).min(MAX_PENALTY);
        let wait = Duration::from_millis(retry_after_ms) + self.cfg.margin;
        let r = self.ready_mut(route);
        *r = (*r).max(now + wait);
        // Discord limits can span routes (global / shared); be conservative.
        let other = match route {
            Route::ListMessages => Route::DeleteMessage,
            Route::DeleteMessage => Route::ListMessages,
        };
        let o = self.ready_mut(other);
        *o = (*o).max(now + wait.min(Duration::from_secs(5)));
    }

    /// Back off after a network error (`attempt` starts at 1).
    pub fn on_network_error(&mut self, route: Route, attempt: u32, now: Instant) {
        let factor = 1u32 << attempt.saturating_sub(1).min(5); // 1,2,4,8,16,32
        let wait = self.cfg.backoff_base * factor;
        let r = self.ready_mut(route);
        *r = (*r).max(now + wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> PacingConfig {
        PacingConfig {
            list_gap: Duration::from_millis(100),
            delete_gap: Duration::from_millis(200),
            margin: Duration::from_millis(50),
            backoff_base: Duration::from_secs(1),
            jitter: false,
        }
    }

    fn info(remaining: Option<u32>, reset_ms: Option<u64>) -> RateInfo {
        RateInfo { remaining, reset_after_ms: reset_ms, bucket: None }
    }

    #[test]
    fn can_go_immediately_at_start() {
        let t = Instant::now();
        let p = Pacer::new(cfg(), t);
        assert_eq!(p.wait_for(Route::DeleteMessage, t), Duration::ZERO);
    }

    #[test]
    fn enforces_min_gap_per_route() {
        let t = Instant::now();
        let mut p = Pacer::new(cfg(), t);
        p.on_success(Route::DeleteMessage, &info(Some(5), Some(1000)), t);
        assert_eq!(p.wait_for(Route::DeleteMessage, t), Duration::from_millis(200));
        assert_eq!(p.wait_for(Route::ListMessages, t), Duration::ZERO); // other route unaffected
        assert_eq!(p.wait_for(Route::DeleteMessage, t + Duration::from_millis(250)), Duration::ZERO);
    }

    #[test]
    fn empty_bucket_waits_for_reset() {
        let t = Instant::now();
        let mut p = Pacer::new(cfg(), t);
        p.on_success(Route::DeleteMessage, &info(Some(0), Some(1500)), t);
        assert_eq!(p.wait_for(Route::DeleteMessage, t), Duration::from_millis(1550));
    }

    #[test]
    fn rate_limit_waits_retry_after_plus_margin_and_slows_down() {
        let t = Instant::now();
        let mut p = Pacer::new(cfg(), t);
        p.on_rate_limited(Route::DeleteMessage, 2000, t);
        assert_eq!(p.wait_for(Route::DeleteMessage, t), Duration::from_millis(2050));
        assert!((p.penalty() - 1.5).abs() < 1e-9);
        // Next success uses the (decayed) penalty: 1.5*0.9 = 1.35 -> 200ms*1.35
        p.on_success(Route::DeleteMessage, &info(Some(3), None), t + Duration::from_secs(3));
        assert_eq!(
            p.wait_for(Route::DeleteMessage, t + Duration::from_secs(3)),
            Duration::from_millis(270)
        );
    }

    #[test]
    fn penalty_is_capped_and_decays_back_to_one() {
        let t = Instant::now();
        let mut p = Pacer::new(cfg(), t);
        for _ in 0..20 {
            p.on_rate_limited(Route::ListMessages, 10, t);
        }
        assert!((p.penalty() - MAX_PENALTY).abs() < 1e-9);
        for _ in 0..60 {
            p.on_success(Route::ListMessages, &info(None, None), t);
        }
        assert!((p.penalty() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn rate_limit_on_one_route_briefly_holds_the_other() {
        let t = Instant::now();
        let mut p = Pacer::new(cfg(), t);
        p.on_rate_limited(Route::DeleteMessage, 1000, t);
        assert_eq!(p.wait_for(Route::ListMessages, t), Duration::from_millis(1050));
        // ...but never for more than 5s.
        p.on_rate_limited(Route::DeleteMessage, 600_000, t);
        assert_eq!(p.wait_for(Route::ListMessages, t), Duration::from_secs(5));
    }

    #[test]
    fn network_backoff_doubles_up_to_32s() {
        let t = Instant::now();
        let mut p = Pacer::new(cfg(), t);
        p.on_network_error(Route::ListMessages, 1, t);
        assert_eq!(p.wait_for(Route::ListMessages, t), Duration::from_secs(1));
        p.on_network_error(Route::ListMessages, 3, t);
        assert_eq!(p.wait_for(Route::ListMessages, t), Duration::from_secs(4));
        p.on_network_error(Route::ListMessages, 99, t);
        assert_eq!(p.wait_for(Route::ListMessages, t), Duration::from_secs(32));
    }

    #[test]
    fn jitter_stays_within_15_percent() {
        let t = Instant::now();
        let mut c = cfg();
        c.jitter = true;
        let mut p = Pacer::new(c, t);
        for _ in 0..200 {
            p.on_success(Route::DeleteMessage, &info(None, None), t);
            let w = p.wait_for(Route::DeleteMessage, t).as_millis();
            assert!((169..=231).contains(&w), "{w}");
        }
    }

    #[test]
    fn speed_profiles_are_sane() {
        assert!(Speed::Gentle.config().delete_gap > Speed::Normal.config().delete_gap);
        assert!(Speed::Normal.config().delete_gap >= Duration::from_millis(500));
    }
}
