//! The cleanup engine.
//!
//! * A **scan** job walks a channel's history newest -> oldest and saves the
//!   user's own messages that pass the job's filters (id, time, a short text
//!   preview) so the user can review them. It never sends a DELETE.
//! * A **delete** job removes exactly the reviewed list saved by a scan, minus
//!   anything the user excluded. It never re-scans, so what was reviewed is
//!   what gets deleted.
//!
//! Safety rules, enforced here and covered by tests:
//!  * only messages whose author id equals the job's `author_id` (the signed-in
//!    user) are ever saved as candidates, so nothing else can ever be deleted;
//!  * only message types Discord documents as deletable;
//!  * pinned messages are skipped unless the job opts in;
//!  * jobs on a channel/server on the protect-list refuse to run;
//!  * a scan job never sends a DELETE request.
//!
//! Resume model: a scan's `cursor` is the id of the last message fully handled
//! (the next page is requested `before=<cursor>`); a delete job's progress is
//! the per-message state in the database, so both continue exactly where they
//! stopped, and deleting never shifts pagination.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Notify;

use crate::auth::token_store::TokenStore;
use crate::discord::model::{snowflake_time_ms, RawMessage, DELETABLE_MESSAGE_TYPES};
use crate::discord::{ApiError, DiscordClient, RateInfo};

use super::filters::Filters;
use super::pacing::{Pacer, PacingConfig, Route, Speed};
use super::store::{Counters, Job, JobKind, JobStatus, NewCandidate, Store};

const PAGE_SIZE: u8 = 100;
const DELETE_BATCH: i64 = 50;
const PREVIEW_CHARS: usize = 140;
const MAX_RATE_LIMIT_STREAK: u32 = 8;
const MAX_NETWORK_RETRIES: u32 = 5;
const MAX_DELETE_FAILURE_STREAK: u32 = 10;
const PERSIST_EVERY: u32 = 10;
const EMIT_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct JobOptions {
    pub speed: Speed,
    pub skip_pinned: bool,
    pub filters: Filters,
}

impl Default for JobOptions {
    fn default() -> Self {
        Self { speed: Speed::Gentle, skip_pinned: true, filters: Filters::default() }
    }
}

/// How the engine tells the outside world what happened.
pub trait Reporter: Send + Sync {
    fn job_updated(&self, job: &Job);
    /// Discord rejected the token (401).
    fn signed_out(&self);
}

pub struct Engine {
    pub store: Store,
    pub client: DiscordClient,
    pub tokens: Arc<dyn TokenStore>,
    pub reporter: Arc<dyn Reporter>,
    /// Pinged when a job is paused/cancelled so waits end early.
    pub control: Arc<Notify>,
    /// Tests use zero gaps; production leaves this `None` (uses the job's speed).
    pub pacing_override: Option<PacingConfig>,
}

enum Exit {
    Completed,
    /// The user paused/cancelled; their status change already happened.
    Interrupted,
    Paused(String),
    Failed(String),
    SignedOut,
}

struct Run<'a> {
    e: &'a Engine,
    token: String,
    kind: JobKind,
    opts: JobOptions,
    pacer: Pacer,
    live: Job,
    c: Counters,
    cursor: Option<String>,
    unsaved: u32,
    last_emit: Instant,
    rate_limit_streak: u32,
    network_attempts: u32,
    delete_failure_streak: u32,
}

fn db_failed(e: sqlx::Error) -> Exit {
    Exit::Failed(format!("Database error: {e}"))
}

/// One line of the message's own text, limited in length.
fn preview(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(PREVIEW_CHARS).collect()
}

impl Engine {
    /// Run one claimed (`running`) job until it finishes, is interrupted, or must stop.
    pub async fn run(&self, job: Job) {
        let id = job.id;
        let Some(kind) = job.kind() else {
            self.finish(id, JobStatus::Failed, Some("Unknown job type")).await;
            return;
        };
        let opts = serde_json::from_str::<JobOptions>(&job.options).unwrap_or_default();
        let token = match self.tokens.load() {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.reporter.signed_out();
                self.finish(id, JobStatus::Paused, Some("Not signed in. Sign in, then resume.")).await;
                return;
            }
            Err(e) => {
                self.finish(id, JobStatus::Failed, Some(&e.to_string())).await;
                return;
            }
        };
        let cfg = self.pacing_override.unwrap_or_else(|| opts.speed.config());
        let source = job.source_scan_id;
        let mut run = Run {
            e: self,
            token,
            kind,
            opts,
            pacer: Pacer::new(cfg, Instant::now()),
            c: Counters {
                scanned: job.scanned,
                found: job.found,
                deleted: job.deleted,
                failed: job.failed,
                skipped: job.skipped,
            },
            cursor: job.cursor.clone(),
            live: job,
            unsaved: 0,
            last_emit: Instant::now() - EMIT_EVERY,
            rate_limit_streak: 0,
            network_attempts: 0,
            delete_failure_streak: 0,
        };

        let exit = run.drive().await;
        run.persist().await;
        let completed = matches!(exit, Exit::Completed);
        match exit {
            Exit::Completed => self.finish(id, JobStatus::Completed, None).await,
            Exit::Interrupted => self.report_fresh(id).await,
            Exit::Paused(msg) => self.finish(id, JobStatus::Paused, Some(&msg)).await,
            Exit::Failed(msg) => self.finish(id, JobStatus::Failed, Some(&msg)).await,
            Exit::SignedOut => {
                self.reporter.signed_out();
                self.finish(id, JobStatus::Paused, Some("Signed out. Sign in again, then resume.")).await;
            }
        }
        // Deleting is done: the reviewed text is no longer needed on disk.
        if completed && kind == JobKind::Delete {
            if let Some(src) = source {
                let _ = self.store.purge_previews(src).await;
            }
        }
    }

    /// running -> `to`, unless the user already changed the status meanwhile.
    async fn finish(&self, id: i64, to: JobStatus, error: Option<&str>) {
        let _ = self.store.transition(id, &[JobStatus::Running], to, error).await;
        self.report_fresh(id).await;
    }

    async fn report_fresh(&self, id: i64) {
        if let Ok(Some(job)) = self.store.get(id).await {
            self.reporter.job_updated(&job);
        }
    }

    async fn is_running(&self, id: i64) -> bool {
        matches!(self.store.status(id).await, Ok(Some(JobStatus::Running)))
    }
}

impl Run<'_> {
    async fn drive(&mut self) -> Exit {
        // Defence in depth: the protect-list is also checked when jobs are created.
        match self.e.store.is_protected(self.live.guild_id.as_deref(), &self.live.channel_id).await {
            Ok(false) => {}
            Ok(true) => return Exit::Failed("This channel (or its server) is on your protect-list.".into()),
            Err(e) => return db_failed(e),
        }
        match self.kind {
            JobKind::Scan => self.drive_scan().await,
            JobKind::Delete => self.drive_delete().await,
        }
    }

    // ------------------------------------------------------------------ scan

    async fn drive_scan(&mut self) -> Exit {
        loop {
            if !self.wait_ready(Route::ListMessages).await {
                return Exit::Interrupted;
            }
            let result = self
                .e
                .client
                .channel_messages(&self.token, &self.live.channel_id, self.cursor.as_deref(), PAGE_SIZE)
                .await;
            let page = match result {
                Ok((page, info)) => {
                    self.rate_limit_streak = 0;
                    self.network_attempts = 0;
                    self.pacer.on_success(Route::ListMessages, &info, Instant::now());
                    page
                }
                Err(err) => match self.on_list_error(err) {
                    Some(exit) => return exit,
                    None => continue,
                },
            };
            if page.is_empty() {
                return Exit::Completed;
            }

            let mut batch = Vec::new();
            let mut reached_start_date = false;
            for m in &page {
                let Some(ts) = snowflake_time_ms(&m.id) else {
                    self.c.scanned += 1;
                    self.cursor = Some(m.id.clone());
                    continue;
                };
                // Newest -> oldest: nothing further back can match a start date.
                if self.opts.filters.older_cannot_match(ts) {
                    reached_start_date = true;
                    break;
                }
                if self.is_candidate(m, ts) {
                    batch.push(NewCandidate {
                        message_id: m.id.clone(),
                        ts,
                        preview: preview(&m.content),
                        attachments: m.attachments.len() as i64,
                    });
                }
                self.c.scanned += 1;
                self.cursor = Some(m.id.clone());
            }
            if let Err(e) = self.e.store.add_candidates(self.live.id, &batch).await {
                return db_failed(e);
            }
            match self.e.store.count_candidates(self.live.id, None).await {
                Ok(n) => self.c.found = n,
                Err(e) => return db_failed(e),
            }
            self.persist().await;
            self.emit(true);
            if reached_start_date {
                return Exit::Completed;
            }
        }
    }

    /// The only gate for what a scan may collect.
    fn is_candidate(&self, m: &RawMessage, ts_ms: i64) -> bool {
        m.author.id == self.live.author_id
            && DELETABLE_MESSAGE_TYPES.contains(&m.kind)
            && !(self.opts.skip_pinned && m.pinned)
            && self.opts.filters.matches(m, ts_ms)
    }

    /// `None` = retry the request; `Some` = stop the job.
    fn on_list_error(&mut self, err: ApiError) -> Option<Exit> {
        let now = Instant::now();
        match err {
            ApiError::Unauthorized => Some(Exit::SignedOut),
            ApiError::RateLimited { retry_after_ms } => {
                self.pacer.on_rate_limited(Route::ListMessages, retry_after_ms, now);
                self.rate_limit_streak += 1;
                (self.rate_limit_streak > MAX_RATE_LIMIT_STREAK)
                    .then(|| Exit::Paused("Discord keeps rate limiting us. Try again later or use Gentle speed.".into()))
            }
            ApiError::Http(403) => Some(Exit::Failed(
                "No access: you can't read this channel's history (missing permission).".into(),
            )),
            ApiError::Http(404) => {
                Some(Exit::Failed("Channel not found. It may have been deleted, or you lost access.".into()))
            }
            ApiError::Http(code) if code < 500 => Some(Exit::Failed(format!("Discord returned HTTP {code}."))),
            // 5xx and network problems are treated as transient.
            ApiError::Http(_) | ApiError::Network(_) => {
                self.network_attempts += 1;
                if self.network_attempts > MAX_NETWORK_RETRIES {
                    return Some(Exit::Paused(format!("Network problem ({err}). Resume to retry.")));
                }
                self.pacer.on_network_error(Route::ListMessages, self.network_attempts, now);
                None
            }
        }
    }

    // ---------------------------------------------------------------- delete

    async fn drive_delete(&mut self) -> Exit {
        let Some(source) = self.live.source_scan_id else {
            return Exit::Failed("A delete job needs a reviewed scan to work from.".into());
        };
        // First run: remember how many messages this job will delete (for progress).
        if self.c.found == 0 && self.c.deleted + self.c.skipped + self.c.failed == 0 {
            match self.e.store.count_pending(source).await {
                Ok(n) => self.c.found = n,
                Err(e) => return db_failed(e),
            }
            self.persist().await;
            self.emit(true);
        }
        loop {
            let batch = match self.e.store.next_pending(source, DELETE_BATCH).await {
                Ok(b) => b,
                Err(e) => return db_failed(e),
            };
            if batch.is_empty() {
                return Exit::Completed;
            }
            for c in &batch {
                if let Some(exit) = self.delete(source, &c.message_id).await {
                    return exit;
                }
            }
            self.persist().await;
            self.emit(true);
        }
    }

    /// `None` = handled (deleted/skipped/failed, and recorded); `Some` = stop the job.
    async fn delete(&mut self, source: i64, message_id: &str) -> Option<Exit> {
        let outcome = loop {
            if !self.wait_ready(Route::DeleteMessage).await {
                return Some(Exit::Interrupted);
            }
            let now = Instant::now();
            match self.e.client.delete_message(&self.token, &self.live.channel_id, message_id).await {
                Ok(info) => {
                    self.pacer.on_success(Route::DeleteMessage, &info, now);
                    self.c.deleted += 1;
                    self.rate_limit_streak = 0;
                    self.network_attempts = 0;
                    self.delete_failure_streak = 0;
                    break "deleted";
                }
                Err(ApiError::Unauthorized) => return Some(Exit::SignedOut),
                Err(ApiError::RateLimited { retry_after_ms }) => {
                    self.pacer.on_rate_limited(Route::DeleteMessage, retry_after_ms, now);
                    self.rate_limit_streak += 1;
                    if self.rate_limit_streak > MAX_RATE_LIMIT_STREAK {
                        return Some(Exit::Paused(
                            "Discord keeps rate limiting us. Try again later or use Gentle speed.".into(),
                        ));
                    }
                }
                // Already gone (deleted elsewhere in the meantime).
                Err(ApiError::Http(404)) => {
                    self.pacer.on_success(Route::DeleteMessage, &RateInfo::default(), now);
                    self.c.skipped += 1;
                    break "skipped";
                }
                Err(ApiError::Http(code)) if code < 500 => {
                    self.pacer.on_success(Route::DeleteMessage, &RateInfo::default(), now);
                    self.c.failed += 1;
                    self.delete_failure_streak += 1;
                    if self.delete_failure_streak >= MAX_DELETE_FAILURE_STREAK {
                        // Record this one, then stop.
                        let _ = self.e.store.mark_candidate(source, message_id, "failed").await;
                        self.c.scanned += 1;
                        return Some(Exit::Paused(format!(
                            "Stopped: {MAX_DELETE_FAILURE_STREAK} deletions in a row failed (HTTP {code})."
                        )));
                    }
                    break "failed";
                }
                Err(err @ (ApiError::Http(_) | ApiError::Network(_))) => {
                    self.network_attempts += 1;
                    if self.network_attempts > MAX_NETWORK_RETRIES {
                        return Some(Exit::Paused(format!("Network problem ({err}). Resume to retry.")));
                    }
                    self.pacer.on_network_error(Route::DeleteMessage, self.network_attempts, now);
                }
            }
        };
        // Record the result immediately, so a crash can't repeat or lose it.
        if let Err(e) = self.e.store.mark_candidate(source, message_id, outcome).await {
            return Some(db_failed(e));
        }
        self.c.scanned += 1; // for a delete job: messages handled so far
        self.unsaved += 1;
        if self.unsaved >= PERSIST_EVERY {
            self.persist().await;
        }
        self.emit(false);
        None
    }

    // --------------------------------------------------------------- helpers

    /// Wait until pacing allows `route`. `false` if the job is no longer running.
    async fn wait_ready(&mut self, route: Route) -> bool {
        loop {
            let wait = self.pacer.wait_for(route, Instant::now());
            if wait.is_zero() {
                return self.e.is_running(self.live.id).await;
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = self.e.control.notified() => {
                    if !self.e.is_running(self.live.id).await {
                        return false;
                    }
                }
            }
        }
    }

    async fn persist(&mut self) {
        let _ = self.e.store.save_progress(self.live.id, self.cursor.as_deref(), &self.c).await;
        self.unsaved = 0;
    }

    fn emit(&mut self, force: bool) {
        if !force && self.last_emit.elapsed() < EMIT_EVERY {
            return;
        }
        self.last_emit = Instant::now();
        self.live.cursor = self.cursor.clone();
        self.live.scanned = self.c.scanned;
        self.live.found = self.c.found;
        self.live.deleted = self.c.deleted;
        self.live.failed = self.c.failed;
        self.live.skipped = self.c.skipped;
        self.e.reporter.job_updated(&self.live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::token_store::MemoryStore;
    use crate::discord::model::DISCORD_EPOCH_MS;
    use crate::jobs::store::NewJob;
    use std::sync::Mutex;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ME: &str = "42";
    const OTHER: &str = "99";
    const CH: &str = "500";
    /// 2023-11-14, comfortably after the Discord epoch.
    const T0: i64 = 1_700_000_000_000;

    #[derive(Default)]
    struct TestReporter {
        updates: Mutex<Vec<Job>>,
        signed_out: Mutex<bool>,
    }
    impl Reporter for TestReporter {
        fn job_updated(&self, job: &Job) {
            self.updates.lock().unwrap().push(job.clone());
        }
        fn signed_out(&self) {
            *self.signed_out.lock().unwrap() = true;
        }
    }

    fn fast() -> PacingConfig {
        PacingConfig {
            list_gap: Duration::ZERO,
            delete_gap: Duration::ZERO,
            margin: Duration::from_millis(5),
            backoff_base: Duration::from_millis(1),
            jitter: false,
        }
    }

    /// A real-looking snowflake for a message sent at `ms`.
    fn sf(ms: i64) -> String {
        (((ms - DISCORD_EPOCH_MS) as u64) << 22).to_string()
    }

    fn msg(id: &str, author: &str, kind: u8, pinned: bool) -> serde_json::Value {
        msg_full(id, author, kind, pinned, "secret text", 0)
    }

    fn msg_full(id: &str, author: &str, kind: u8, pinned: bool, content: &str, attachments: usize) -> serde_json::Value {
        serde_json::json!({
            "id": id, "author": {"id": author, "username": "x"}, "type": kind, "pinned": pinned,
            "content": content,
            "attachments": (0..attachments).map(|i| serde_json::json!({"id": i.to_string(), "filename": "f.png"})).collect::<Vec<_>>()
        })
    }

    struct Fixture {
        server: MockServer,
        engine: Engine,
        reporter: Arc<TestReporter>,
    }

    async fn fixture(token: Option<&str>) -> Fixture {
        let server = MockServer::start().await;
        let tokens = MemoryStore::default();
        if let Some(t) = token {
            tokens.save(t).unwrap();
        }
        let reporter = Arc::new(TestReporter::default());
        let engine = Engine {
            store: Store::in_memory().await.unwrap(),
            client: DiscordClient::new(server.uri()),
            tokens: Arc::new(tokens),
            reporter: reporter.clone(),
            control: Arc::new(Notify::new()),
            pacing_override: Some(fast()),
        };
        Fixture { server, engine, reporter }
    }

    fn new_job(kind: JobKind, options: &str, source: Option<i64>, guild: Option<&str>) -> NewJob {
        NewJob {
            kind,
            channel_id: CH.into(),
            channel_name: "general".into(),
            location: "Test".into(),
            author_id: ME.into(),
            options: options.into(),
            guild_id: guild.map(Into::into),
            source_scan_id: source,
        }
    }

    /// Create a scan job and claim it (-> running).
    async fn scan_job(f: &Fixture, options: &str, cursor: Option<&str>) -> Job {
        let j = f.engine.store.create(new_job(JobKind::Scan, options, None, None)).await.unwrap();
        if let Some(c) = cursor {
            f.engine.store.save_progress(j.id, Some(c), &Counters::default()).await.unwrap();
        }
        f.engine.store.claim_next().await.unwrap().unwrap()
    }

    /// A finished scan with these candidates, ready to be deleted from.
    async fn finished_scan(f: &Fixture, ids: &[&str]) -> i64 {
        let j = f.engine.store.create(new_job(JobKind::Scan, "{}", None, None)).await.unwrap();
        let items: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| NewCandidate {
                message_id: (*id).into(),
                ts: 1000 + i as i64,
                preview: format!("text {id}"),
                attachments: 0,
            })
            .collect();
        f.engine.store.add_candidates(j.id, &items).await.unwrap();
        f.engine.store.transition(j.id, &[JobStatus::Queued], JobStatus::Completed, None).await.unwrap();
        j.id
    }

    /// Create a delete job from a scan and claim it.
    async fn delete_job(f: &Fixture, scan: i64, guild: Option<&str>) -> Job {
        let j = f.engine.store.create(new_job(JobKind::Delete, "{}", Some(scan), guild)).await.unwrap();
        let claimed = f.engine.store.claim_next().await.unwrap().unwrap();
        assert_eq!(claimed.id, j.id);
        claimed
    }

    async fn page(f: &Fixture, before: Option<&str>, msgs: Vec<serde_json::Value>) {
        let m = Mock::given(method("GET")).and(path(format!("/channels/{CH}/messages")));
        let m = match before {
            Some(b) => m.and(query_param("before", b)),
            None => m.and(query_param_is_missing("before")),
        };
        m.respond_with(ResponseTemplate::new(200).set_body_json(msgs)).mount(&f.server).await;
    }

    /// A catch-all `DELETE` mock that must never be hit.
    async fn forbid_deletes(f: &Fixture) {
        Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(500)).expect(0).mount(&f.server).await;
    }

    /// `DELETE` mocks that must be hit exactly once each, plus a catch-all that
    /// must never be hit (so deleting anything else fails the test).
    async fn expect_deletes(f: &Fixture, ids: &[&str]) {
        for id in ids {
            Mock::given(method("DELETE"))
                .and(path(format!("/channels/{CH}/messages/{id}")))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&f.server)
                .await;
        }
        forbid_deletes(f).await;
    }

    async fn final_job(f: &Fixture, j: &Job) -> Job {
        f.engine.store.get(j.id).await.unwrap().unwrap()
    }

    async fn candidate_ids(f: &Fixture, job_id: i64) -> Vec<String> {
        let mut v: Vec<String> =
            f.engine.store.candidates(job_id, 0, 500, None).await.unwrap().into_iter().map(|c| c.message_id).collect();
        v.sort();
        v
    }

    fn mixed() -> Vec<serde_json::Value> {
        vec![
            msg("105", ME, 0, false),    // mine, deletable
            msg("104", OTHER, 0, false), // someone else's
            msg("103", ME, 3, false),    // mine, CALL: not deletable
            msg("102", ME, 0, true),     // mine, pinned
            msg("101", ME, 19, false),   // mine, reply
        ]
    }

    // ------------------------------------------------------------------ scan

    #[tokio::test]
    async fn scan_collects_only_my_deletable_unpinned_messages_and_never_deletes() {
        let f = fixture(Some("tok")).await;
        page(&f, None, mixed()).await;
        page(&f, Some("101"), vec![]).await;
        forbid_deletes(&f).await;

        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;

        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Completed));
        assert_eq!((g.scanned, g.found, g.deleted), (5, 2, 0));
        assert_eq!(candidate_ids(&f, j.id).await, ["101", "105"]); // not 104 (other), 103 (CALL), 102 (pinned)
        assert_eq!(g.cursor.as_deref(), Some("101"));
    }

    #[tokio::test]
    async fn pinned_messages_are_collected_only_when_opted_in() {
        let f = fixture(Some("tok")).await;
        page(&f, None, mixed()).await;
        page(&f, Some("101"), vec![]).await;
        let j = scan_job(&f, r#"{"skip_pinned":false}"#, None).await;
        f.engine.run(j.clone()).await;
        assert_eq!(candidate_ids(&f, j.id).await, ["101", "102", "105"]);
    }

    #[tokio::test]
    async fn scan_saves_a_short_one_line_preview_and_attachment_count() {
        let f = fixture(Some("tok")).await;
        let long = format!("line one\n\n  line   two {}", "x".repeat(300));
        page(&f, None, vec![msg_full("9", ME, 0, false, &long, 2), msg_full("8", ME, 0, false, "", 1)]).await;
        page(&f, Some("8"), vec![]).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        let c = f.engine.store.candidates(j.id, 0, 10, None).await.unwrap();
        let nine = c.iter().find(|c| c.message_id == "9").unwrap();
        assert!(nine.preview.as_deref().unwrap().starts_with("line one line two xxx"));
        assert_eq!(nine.preview.as_deref().unwrap().chars().count(), PREVIEW_CHARS);
        assert_eq!(nine.attachments, 2);
        assert_eq!(c.iter().find(|c| c.message_id == "8").unwrap().preview.as_deref(), Some(""));
    }

    #[tokio::test]
    async fn scan_applies_keyword_filter_and_never_another_authors_message() {
        let f = fixture(Some("tok")).await;
        page(
            &f,
            None,
            vec![
                msg_full("30", ME, 0, false, "Pizza night", 0),
                msg_full("29", ME, 0, false, "pasta", 0),
                msg_full("27", OTHER, 0, false, "pizza by someone else", 0),
            ],
        )
        .await;
        page(&f, Some("27"), vec![]).await;
        let j = scan_job(&f, r#"{"filters":{"contains":"PIZZA"}}"#, None).await;
        f.engine.run(j.clone()).await;
        assert_eq!(candidate_ids(&f, j.id).await, ["30"]);
    }

    #[tokio::test]
    async fn scan_applies_attachment_only_filter() {
        let f = fixture(Some("tok")).await;
        page(&f, None, vec![msg_full("30", ME, 0, false, "a", 0), msg_full("29", ME, 0, false, "b", 1)]).await;
        page(&f, Some("29"), vec![]).await;
        let j = scan_job(&f, r#"{"filters":{"only_with_attachments":true}}"#, None).await;
        f.engine.run(j.clone()).await;
        assert_eq!(candidate_ids(&f, j.id).await, ["29"]);
    }

    #[tokio::test]
    async fn scan_applies_link_only_filter() {
        let f = fixture(Some("tok")).await;
        page(&f, None, vec![msg_full("30", ME, 0, false, "no link", 0), msg_full("29", ME, 0, false, "go HTTPS://a.b", 0)]).await;
        page(&f, Some("29"), vec![]).await;
        let j = scan_job(&f, r#"{"filters":{"only_with_links":true}}"#, None).await;
        f.engine.run(j.clone()).await;
        assert_eq!(candidate_ids(&f, j.id).await, ["29"]);
    }

    #[tokio::test]
    async fn date_range_filter_selects_the_window_and_stops_early() {
        let f = fixture(Some("tok")).await;
        // Newest -> oldest: +5s, +4s, +3s, +2s, +1s.
        let ids: Vec<String> = (1..=5).rev().map(|s| sf(T0 + s * 1000)).collect();
        let msgs: Vec<_> = ids.iter().map(|id| msg(id, ME, 0, false)).collect();
        page(&f, None, msgs).await;
        // The scan must NOT ask for another page: it already passed the start date.
        Mock::given(method("GET"))
            .and(query_param("before", ids[3].as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(Vec::<serde_json::Value>::new()))
            .expect(0)
            .mount(&f.server)
            .await;

        // after = +2s (inclusive), before = +5s (exclusive) -> +4s, +3s, +2s
        let opts = format!(r#"{{"filters":{{"after_ms":{},"before_ms":{}}}}}"#, T0 + 2000, T0 + 5000);
        let j = scan_job(&f, &opts, None).await;
        f.engine.run(j.clone()).await;

        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Completed));
        let mut want = vec![ids[1].clone(), ids[2].clone(), ids[3].clone()];
        want.sort();
        assert_eq!(candidate_ids(&f, j.id).await, want);
        assert_eq!(g.scanned, 4, "scanned +5s..+2s, stopped at +1s");
    }

    #[tokio::test]
    async fn scan_walks_pages_with_before_cursor() {
        let f = fixture(Some("tok")).await;
        page(&f, None, vec![msg("30", ME, 0, false), msg("29", ME, 0, false)]).await;
        page(&f, Some("29"), vec![msg("28", ME, 0, false)]).await;
        page(&f, Some("28"), vec![]).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!((g.scanned, g.found), (3, 3));
        assert_eq!(g.status(), Some(JobStatus::Completed));
    }

    #[tokio::test]
    async fn scan_resumes_from_saved_cursor_without_duplicating_candidates() {
        let f = fixture(Some("tok")).await;
        // Only `before=103` is mocked: a request without the cursor would 404 and fail the job.
        page(&f, Some("103"), vec![msg("102", ME, 0, false)]).await;
        page(&f, Some("102"), vec![]).await;
        let mut j = scan_job(&f, "{}", Some("103")).await;
        // Earlier progress, including a candidate saved before the interruption.
        f.engine
            .store
            .add_candidates(j.id, &[NewCandidate { message_id: "104".into(), ts: 1, preview: "old".into(), attachments: 0 }])
            .await
            .unwrap();
        j.scanned = 10;
        j.found = 1;
        f.engine.run(j.clone()).await;

        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Completed));
        assert_eq!((g.scanned, g.found), (11, 2));
        assert_eq!(candidate_ids(&f, j.id).await, ["102", "104"]);
    }

    #[tokio::test]
    async fn scan_of_a_protected_channel_or_server_refuses_without_any_request() {
        for (protect_kind, protect_id) in [("channel", CH), ("guild", "777")] {
            let f = fixture(Some("tok")).await;
            Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&f.server).await;
            f.engine.store.protect_add(protect_kind, protect_id, "x").await.unwrap();
            f.engine.store.create(new_job(JobKind::Scan, "{}", None, Some("777"))).await.unwrap();
            let j = f.engine.store.claim_next().await.unwrap().unwrap();
            f.engine.run(j.clone()).await;
            let g = final_job(&f, &j).await;
            assert_eq!(g.status(), Some(JobStatus::Failed), "{protect_kind}");
            assert!(g.error.unwrap().contains("protect-list"));
        }
    }

    // ---------------------------------------------------------------- delete

    #[tokio::test]
    async fn delete_job_deletes_exactly_the_reviewed_list_minus_exclusions() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["10", "11", "12"]).await;
        f.engine.store.set_excluded(scan, &["11".into()], true).await.unwrap(); // user unticked 11
        expect_deletes(&f, &["10", "12"]).await; // 11 (excluded) and anything else: never

        let j = delete_job(&f, scan, None).await;
        f.engine.run(j.clone()).await;

        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Completed));
        assert_eq!((g.found, g.deleted, g.skipped, g.failed, g.scanned), (2, 2, 0, 0, 2));
        let states: Vec<(String, String)> = f
            .engine
            .store
            .candidates(scan, 0, 10, None)
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.message_id, c.state))
            .collect();
        assert!(states.contains(&("10".into(), "deleted".into())));
        assert!(states.contains(&("11".into(), "pending".into())), "excluded message untouched");
        assert!(states.contains(&("12".into(), "deleted".into())));
    }

    #[tokio::test]
    async fn finished_delete_wipes_the_stored_message_text() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["10", "11"]).await;
        expect_deletes(&f, &["10", "11"]).await;
        let j = delete_job(&f, scan, None).await;
        f.engine.run(j).await;
        let c = f.engine.store.candidates(scan, 0, 10, None).await.unwrap();
        assert!(c.iter().all(|c| c.preview.is_none()), "previews purged after deleting");
        assert!(c.iter().all(|c| c.state == "deleted"));
    }

    #[tokio::test]
    async fn delete_job_without_a_source_scan_fails_and_sends_nothing() {
        let f = fixture(Some("tok")).await;
        forbid_deletes(&f).await;
        f.engine.store.create(new_job(JobKind::Delete, "{}", None, None)).await.unwrap();
        let j = f.engine.store.claim_next().await.unwrap().unwrap();
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Failed));
        assert!(g.error.unwrap().contains("reviewed scan"));
    }

    #[tokio::test]
    async fn delete_job_never_runs_on_a_protected_channel_or_server() {
        for (kind, id) in [("channel", CH), ("guild", "777")] {
            let f = fixture(Some("tok")).await;
            let scan = finished_scan(&f, &["10"]).await;
            forbid_deletes(&f).await;
            f.engine.store.protect_add(kind, id, "x").await.unwrap(); // protected AFTER the scan
            let j = delete_job(&f, scan, Some("777")).await;
            f.engine.run(j.clone()).await;
            assert_eq!(final_job(&f, &j).await.status(), Some(JobStatus::Failed), "{kind}");
            assert_eq!(f.engine.store.count_pending(scan).await.unwrap(), 1, "nothing deleted");
        }
    }

    #[tokio::test]
    async fn delete_resumes_with_only_the_messages_still_pending() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["10", "11", "12"]).await;
        f.engine.store.mark_candidate(scan, "10", "deleted").await.unwrap(); // done before the interruption
        expect_deletes(&f, &["11", "12"]).await; // NOT 10 again

        let mut j = delete_job(&f, scan, None).await;
        j.found = 3;
        j.deleted = 1;
        j.scanned = 1;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!((g.status(), g.found, g.deleted, g.scanned), (Some(JobStatus::Completed), 3, 3, 3));
    }

    #[tokio::test]
    async fn delete_retries_after_429_then_succeeds() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["10"]).await;
        Mock::given(method("DELETE"))
            .and(path(format!("/channels/{CH}/messages/10")))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0.02"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&f.server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/channels/{CH}/messages/10")))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&f.server)
            .await;

        let j = delete_job(&f, scan, None).await;
        let t = Instant::now();
        f.engine.run(j.clone()).await;
        assert!(t.elapsed() >= Duration::from_millis(20), "must wait out retry-after");
        let g = final_job(&f, &j).await;
        assert_eq!((g.status(), g.deleted, g.failed), (Some(JobStatus::Completed), 1, 0));
    }

    #[tokio::test]
    async fn delete_404_counts_as_skipped_and_403_as_failed() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["12", "11"]).await;
        Mock::given(method("DELETE"))
            .and(path(format!("/channels/{CH}/messages/12")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&f.server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/channels/{CH}/messages/11")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&f.server)
            .await;

        let j = delete_job(&f, scan, None).await;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!((g.status(), g.deleted, g.skipped, g.failed), (Some(JobStatus::Completed), 0, 1, 1));
        let st: Vec<String> =
            f.engine.store.candidates(scan, 0, 10, None).await.unwrap().into_iter().map(|c| c.state).collect();
        assert!(st.contains(&"skipped".to_string()) && st.contains(&"failed".to_string()));
    }

    #[tokio::test]
    async fn repeated_delete_failures_pause_the_job() {
        let f = fixture(Some("tok")).await;
        let ids: Vec<String> = (1..=20).map(|i| (1000 + i).to_string()).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let scan = finished_scan(&f, &refs).await;
        Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(403)).mount(&f.server).await;
        let j = delete_job(&f, scan, None).await;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Paused));
        assert_eq!(g.failed, MAX_DELETE_FAILURE_STREAK as i64);
        assert!(g.error.unwrap().contains("in a row failed"));
        assert_eq!(f.engine.store.count_pending(scan).await.unwrap(), 10, "the rest stay pending");
    }

    // ---------------------------------------------------------- shared paths

    #[tokio::test]
    async fn unauthorized_pauses_and_signals_sign_out() {
        let f = fixture(Some("tok")).await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(401)).mount(&f.server).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Paused));
        assert!(g.error.unwrap().contains("Signed out"));
        assert!(*f.reporter.signed_out.lock().unwrap());
    }

    #[tokio::test]
    async fn unauthorized_during_delete_keeps_the_message_pending() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["10"]).await;
        Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(401)).mount(&f.server).await;
        let j = delete_job(&f, scan, None).await;
        f.engine.run(j.clone()).await;
        assert_eq!(final_job(&f, &j).await.status(), Some(JobStatus::Paused));
        assert!(*f.reporter.signed_out.lock().unwrap());
        assert_eq!(f.engine.store.count_pending(scan).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn missing_token_pauses_without_any_request() {
        let f = fixture(None).await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&f.server).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        assert_eq!(final_job(&f, &j).await.status(), Some(JobStatus::Paused));
        assert!(*f.reporter.signed_out.lock().unwrap());
    }

    #[tokio::test]
    async fn no_access_to_history_fails_with_clear_message() {
        let f = fixture(Some("tok")).await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(403)).mount(&f.server).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Failed));
        assert!(g.error.unwrap().contains("No access"));
    }

    #[tokio::test]
    async fn job_paused_before_start_sends_no_requests() {
        let f = fixture(Some("tok")).await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).expect(0).mount(&f.server).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.store.transition(j.id, &[JobStatus::Running], JobStatus::Paused, None).await.unwrap();
        f.engine.run(j.clone()).await;
        assert_eq!(final_job(&f, &j).await.status(), Some(JobStatus::Paused)); // not overwritten
    }

    #[tokio::test]
    async fn delete_paused_before_start_deletes_nothing() {
        let f = fixture(Some("tok")).await;
        let scan = finished_scan(&f, &["10"]).await;
        forbid_deletes(&f).await;
        let j = delete_job(&f, scan, None).await;
        f.engine.store.transition(j.id, &[JobStatus::Running], JobStatus::Paused, None).await.unwrap();
        f.engine.run(j.clone()).await;
        assert_eq!(f.engine.store.count_pending(scan).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn network_errors_retry_then_pause() {
        let mut f = fixture(Some("tok")).await;
        f.engine.client = DiscordClient::new("http://127.0.0.1:9"); // nothing listens
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        let g = final_job(&f, &j).await;
        assert_eq!(g.status(), Some(JobStatus::Paused));
        assert!(g.error.unwrap().contains("Network problem"));
    }

    #[tokio::test]
    async fn progress_is_reported_and_final_state_matches_store() {
        let f = fixture(Some("tok")).await;
        page(&f, None, vec![msg("5", ME, 0, false)]).await;
        page(&f, Some("5"), vec![]).await;
        let j = scan_job(&f, "{}", None).await;
        f.engine.run(j.clone()).await;
        let updates = f.reporter.updates.lock().unwrap();
        assert!(updates.len() >= 2);
        let last = updates.last().unwrap();
        assert_eq!((last.status(), last.found), (Some(JobStatus::Completed), 1));
    }

    /// LIVE test against real Discord. Never runs by default. Use a THROWAWAY
    /// account, never your main account.
    ///
    ///   CONCORD_TEST_TOKEN=<throwaway account token> \
    ///   CONCORD_TEST_CHANNEL=<channel id in a private test server/DM> \
    ///   cargo test live_job -- --ignored --nocapture
    ///
    /// Scans only (collects your messages, deletes nothing) unless you also set
    /// CONCORD_TEST_DELETE=yes, which then deletes everything the scan found.
    /// The token is read from the environment and never printed.
    #[tokio::test]
    #[ignore = "talks to real Discord; needs CONCORD_TEST_TOKEN and CONCORD_TEST_CHANNEL"]
    async fn live_job() {
        let token = std::env::var("CONCORD_TEST_TOKEN").expect("set CONCORD_TEST_TOKEN");
        let channel = std::env::var("CONCORD_TEST_CHANNEL").expect("set CONCORD_TEST_CHANNEL");
        let delete = std::env::var("CONCORD_TEST_DELETE").as_deref() == Ok("yes");

        let client = DiscordClient::with_default_base();
        let me = client.current_user(&token).await.expect("token rejected by Discord");
        println!("signed in as @{} (id {})", me.username, me.id);

        let tokens = MemoryStore::default();
        tokens.save(&token).unwrap();
        let engine = Engine {
            store: Store::in_memory().await.unwrap(),
            client,
            tokens: Arc::new(tokens),
            reporter: Arc::new(TestReporter::default()),
            control: Arc::new(Notify::new()),
            pacing_override: None, // real Gentle pacing
        };
        let mk = |kind, source| NewJob {
            kind,
            channel_id: channel.clone(),
            channel_name: "live-test".into(),
            location: "live".into(),
            author_id: me.id.clone(),
            options: "{}".into(),
            guild_id: None,
            source_scan_id: source,
        };
        let scan = engine.store.create(mk(JobKind::Scan, None)).await.unwrap();
        let claimed = engine.store.claim_next().await.unwrap().unwrap();
        engine.run(claimed).await;
        let s = engine.store.get(scan.id).await.unwrap().unwrap();
        println!("scan: status={} scanned={} found={} error={:?}", s.status, s.scanned, s.found, s.error);
        assert_eq!(s.status(), Some(JobStatus::Completed), "scan did not complete: {:?}", s.error);

        if delete {
            let d = engine.store.create(mk(JobKind::Delete, Some(scan.id))).await.unwrap();
            let claimed = engine.store.claim_next().await.unwrap().unwrap();
            assert_eq!(claimed.id, d.id);
            engine.run(claimed).await;
            let g = engine.store.get(d.id).await.unwrap().unwrap();
            println!("delete: status={} deleted={} skipped={} failed={} error={:?}", g.status, g.deleted, g.skipped, g.failed, g.error);
            assert_eq!(g.status(), Some(JobStatus::Completed), "delete did not complete: {:?}", g.error);
        }
    }
}
