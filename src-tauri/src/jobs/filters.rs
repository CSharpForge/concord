//! Scan filters: which of the user's own messages a scan should collect.
//! All conditions must hold (AND). Pure and fully unit-tested.

use serde::{Deserialize, Serialize};

use crate::discord::model::RawMessage;

const MAX_CONTAINS_CHARS: usize = 200;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filters {
    /// Only messages sent at or after this time (ms since the Unix epoch).
    pub after_ms: Option<i64>,
    /// Only messages sent strictly before this time.
    pub before_ms: Option<i64>,
    /// Case-insensitive text the message must contain.
    pub contains: Option<String>,
    pub only_with_attachments: bool,
    pub only_with_links: bool,
}

impl Filters {
    /// Trim/limit the keyword and reject impossible combinations.
    pub fn normalized(mut self) -> Result<Self, String> {
        self.contains = self
            .contains
            .map(|s| s.trim().chars().take(MAX_CONTAINS_CHARS).collect::<String>())
            .filter(|s| !s.is_empty());
        if let (Some(a), Some(b)) = (self.after_ms, self.before_ms) {
            if a >= b {
                return Err("The start date must be before the end date.".into());
            }
        }
        Ok(self)
    }

    /// Does `m` (sent at `ts_ms`) pass every filter?
    pub fn matches(&self, m: &RawMessage, ts_ms: i64) -> bool {
        if self.after_ms.is_some_and(|a| ts_ms < a) || self.before_ms.is_some_and(|b| ts_ms >= b) {
            return false;
        }
        if self.only_with_attachments && m.attachments.is_empty() {
            return false;
        }
        if self.only_with_links {
            let c = m.content.to_lowercase();
            if !(c.contains("http://") || c.contains("https://")) {
                return false;
            }
        }
        if let Some(needle) = &self.contains {
            if !m.content.to_lowercase().contains(&needle.to_lowercase()) {
                return false;
            }
        }
        true
    }

    /// A scan walks newest -> oldest, so once a message is older than the start
    /// date, nothing further back can match and the scan may stop.
    pub fn older_cannot_match(&self, ts_ms: i64) -> bool {
        self.after_ms.is_some_and(|a| ts_ms < a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discord::model::{RawAttachment, RawAuthor};

    fn msg(content: &str, attachments: usize) -> RawMessage {
        RawMessage {
            id: "1".into(),
            author: RawAuthor { id: "1".into() },
            kind: 0,
            pinned: false,
            content: content.into(),
            attachments: (0..attachments).map(|_| RawAttachment {}).collect(),
        }
    }

    #[test]
    fn empty_filters_match_everything() {
        assert!(Filters::default().matches(&msg("", 0), 0));
        assert!(Filters::default().matches(&msg("anything", 2), i64::MAX));
    }

    #[test]
    fn date_range_is_inclusive_start_exclusive_end() {
        let f = Filters { after_ms: Some(100), before_ms: Some(200), ..Default::default() };
        let m = msg("x", 0);
        assert!(!f.matches(&m, 99));
        assert!(f.matches(&m, 100));
        assert!(f.matches(&m, 199));
        assert!(!f.matches(&m, 200));
    }

    #[test]
    fn keyword_is_case_insensitive_substring() {
        let f = Filters { contains: Some("Pizza".into()), ..Default::default() };
        assert!(f.matches(&msg("I love PIZZA night", 0), 0));
        assert!(f.matches(&msg("pizzas", 0), 0));
        assert!(!f.matches(&msg("pasta", 0), 0));
        // Non-ASCII case folding works too.
        let g = Filters { contains: Some("ÄPFEL".into()), ..Default::default() };
        assert!(g.matches(&msg("ich mag äpfel", 0), 0));
    }

    #[test]
    fn attachments_and_links() {
        let a = Filters { only_with_attachments: true, ..Default::default() };
        assert!(a.matches(&msg("", 1), 0));
        assert!(!a.matches(&msg("text", 0), 0));
        let l = Filters { only_with_links: true, ..Default::default() };
        assert!(l.matches(&msg("see HTTPS://example.com now", 0), 0));
        assert!(l.matches(&msg("http://a.b", 0), 0));
        assert!(!l.matches(&msg("no link, just example.com", 0), 0));
    }

    #[test]
    fn all_conditions_must_hold() {
        let f = Filters {
            after_ms: Some(10),
            contains: Some("cat".into()),
            only_with_attachments: true,
            ..Default::default()
        };
        assert!(f.matches(&msg("a cat", 1), 10));
        assert!(!f.matches(&msg("a cat", 0), 10)); // no attachment
        assert!(!f.matches(&msg("a dog", 1), 10)); // no keyword
        assert!(!f.matches(&msg("a cat", 1), 9)); // too old
    }

    #[test]
    fn normalization_trims_drops_empty_and_checks_range() {
        let f = Filters { contains: Some("  hi  ".into()), ..Default::default() }.normalized().unwrap();
        assert_eq!(f.contains.as_deref(), Some("hi"));
        let g = Filters { contains: Some("   ".into()), ..Default::default() }.normalized().unwrap();
        assert_eq!(g.contains, None);
        let long = Filters { contains: Some("x".repeat(500)), ..Default::default() }.normalized().unwrap();
        assert_eq!(long.contains.unwrap().chars().count(), MAX_CONTAINS_CHARS);
        assert!(Filters { after_ms: Some(5), before_ms: Some(5), ..Default::default() }.normalized().is_err());
        assert!(Filters { after_ms: Some(9), before_ms: Some(5), ..Default::default() }.normalized().is_err());
        assert!(Filters { after_ms: Some(1), before_ms: Some(5), ..Default::default() }.normalized().is_ok());
    }

    #[test]
    fn early_stop_only_when_older_than_start_date() {
        let f = Filters { after_ms: Some(100), ..Default::default() };
        assert!(f.older_cannot_match(99));
        assert!(!f.older_cannot_match(100));
        assert!(!Filters::default().older_cannot_match(0)); // no start date: never stop early
        // A before-only filter must NOT stop early (older messages still match).
        assert!(!Filters { before_ms: Some(100), ..Default::default() }.older_cannot_match(1));
    }
}
