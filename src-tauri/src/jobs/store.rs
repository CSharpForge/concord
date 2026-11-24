//! SQLite persistence for cleanup jobs (sqlx; the backend owns this database).
//! Stores only counters and a resume cursor: no message content, never a token.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

pub type Result<T> = std::result::Result<T, sqlx::Error>;

/// Schema migrations, applied in order; `PRAGMA user_version` tracks progress.
const MIGRATIONS: &[&str] = &["
CREATE TABLE jobs (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  kind         TEXT    NOT NULL,
  channel_id   TEXT    NOT NULL,
  channel_name TEXT    NOT NULL,
  location     TEXT    NOT NULL,
  status       TEXT    NOT NULL,
  author_id    TEXT    NOT NULL,
  options      TEXT    NOT NULL,
  cursor       TEXT,
  scanned      INTEGER NOT NULL DEFAULT 0,
  found        INTEGER NOT NULL DEFAULT 0,
  deleted      INTEGER NOT NULL DEFAULT 0,
  failed       INTEGER NOT NULL DEFAULT 0,
  skipped      INTEGER NOT NULL DEFAULT 0,
  error        TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX jobs_status ON jobs(status);
", "
ALTER TABLE jobs ADD COLUMN guild_id TEXT;
ALTER TABLE jobs ADD COLUMN source_scan_id INTEGER;
-- Messages a scan found, kept so the user can review them and a delete job can
-- remove exactly that list. `preview` is a short snippet of the user's own text.
CREATE TABLE job_messages (
  job_id      INTEGER NOT NULL,
  message_id  TEXT    NOT NULL,
  ts          INTEGER NOT NULL,
  preview     TEXT,
  attachments INTEGER NOT NULL DEFAULT 0,
  excluded    INTEGER NOT NULL DEFAULT 0,
  state       TEXT    NOT NULL DEFAULT 'pending',
  PRIMARY KEY (job_id, message_id)
) WITHOUT ROWID;
CREATE INDEX job_messages_pending ON job_messages(job_id, state, excluded);
CREATE TABLE protected (
  kind TEXT NOT NULL,
  id   TEXT NOT NULL,
  name TEXT NOT NULL,
  PRIMARY KEY (kind, id)
);
"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Count the user's messages; never deletes.
    Scan,
    Delete,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Scan => "scan",
            JobKind::Delete => "delete",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "scan" => Some(JobKind::Scan),
            "delete" => Some(JobKind::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Paused => "paused",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Cancelled => "cancelled",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => JobStatus::Queued,
            "running" => JobStatus::Running,
            "paused" => JobStatus::Paused,
            "completed" => JobStatus::Completed,
            "failed" => JobStatus::Failed,
            "cancelled" => JobStatus::Cancelled,
            _ => return None,
        })
    }
}

/// A job row. `kind`/`status` are strings in the DB and in the UI payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub channel_id: String,
    pub channel_name: String,
    pub location: String,
    pub status: String,
    pub author_id: String,
    pub options: String,
    pub cursor: Option<String>,
    pub scanned: i64,
    pub found: i64,
    pub deleted: i64,
    pub failed: i64,
    pub skipped: i64,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub guild_id: Option<String>,
    /// For a delete job: the finished scan whose reviewed list it deletes.
    pub source_scan_id: Option<i64>,
}

impl Job {
    pub fn kind(&self) -> Option<JobKind> {
        JobKind::parse(&self.kind)
    }
    pub fn status(&self) -> Option<JobStatus> {
        JobStatus::parse(&self.status)
    }
}

#[derive(Debug, Clone)]
pub struct NewJob {
    pub kind: JobKind,
    pub channel_id: String,
    pub channel_name: String,
    pub location: String,
    pub author_id: String,
    pub options: String,
    pub guild_id: Option<String>,
    pub source_scan_id: Option<i64>,
}

/// A message found by a scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct Candidate {
    pub message_id: String,
    pub ts: i64,
    pub preview: Option<String>,
    pub attachments: i64,
    pub excluded: bool,
    /// pending | deleted | skipped | failed
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct NewCandidate {
    pub message_id: String,
    pub ts: i64,
    pub preview: String,
    pub attachments: i64,
}

/// A channel or server the user told Concord never to touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct Protected {
    /// "channel" or "guild"
    pub kind: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub scanned: i64,
    pub found: i64,
    pub deleted: i64,
    pub failed: i64,
    pub skipped: i64,
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Open (creating if needed) the database file and apply migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(opts).await?;
        Self::migrate(&pool).await?;
        Ok(Self { pool })
    }

    /// Private in-memory database (tests).
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self> {
        let opts = <SqliteConnectOptions as std::str::FromStr>::from_str("sqlite::memory:")?;
        // One connection that never expires, or the memory DB would vanish.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await?;
        Self::migrate(&pool).await?;
        Ok(Self { pool })
    }

    async fn migrate(pool: &SqlitePool) -> Result<()> {
        let current: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(pool).await?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let mut tx = pool.begin().await?;
            sqlx::raw_sql(sql).execute(&mut *tx).await?;
            // PRAGMA can't be bound; the number is ours, not user input.
            sqlx::raw_sql(&format!("PRAGMA user_version = {}", i + 1)).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }

    pub async fn create(&self, j: NewJob) -> Result<Job> {
        let now = now_ms();
        let id = sqlx::query(
            "INSERT INTO jobs (kind, channel_id, channel_name, location, status, author_id, options, created_at, updated_at, guild_id, source_scan_id)
             VALUES (?, ?, ?, ?, 'queued', ?, ?, ?, ?, ?, ?)",
        )
        .bind(j.kind.as_str())
        .bind(&j.channel_id)
        .bind(&j.channel_name)
        .bind(&j.location)
        .bind(&j.author_id)
        .bind(&j.options)
        .bind(now)
        .bind(now)
        .bind(&j.guild_id)
        .bind(j.source_scan_id)
        .execute(&self.pool)
        .await?
        .last_insert_rowid();
        Ok(self.get(id).await?.expect("row was just inserted"))
    }

    pub async fn get(&self, id: i64) -> Result<Option<Job>> {
        sqlx::query_as::<_, Job>("SELECT * FROM jobs WHERE id = ?").bind(id).fetch_optional(&self.pool).await
    }

    /// Newest first.
    pub async fn list(&self) -> Result<Vec<Job>> {
        sqlx::query_as::<_, Job>("SELECT * FROM jobs ORDER BY id DESC").fetch_all(&self.pool).await
    }

    pub async fn status(&self, id: i64) -> Result<Option<JobStatus>> {
        let s: Option<String> =
            sqlx::query_scalar("SELECT status FROM jobs WHERE id = ?").bind(id).fetch_optional(&self.pool).await?;
        Ok(s.and_then(|s| JobStatus::parse(&s)))
    }

    /// Oldest queued job -> running. Returns it, or `None` if nothing is queued.
    pub async fn claim_next(&self) -> Result<Option<Job>> {
        loop {
            let next: Option<i64> =
                sqlx::query_scalar("SELECT id FROM jobs WHERE status = 'queued' ORDER BY id LIMIT 1")
                    .fetch_optional(&self.pool)
                    .await?;
            let Some(id) = next else { return Ok(None) };
            if self.transition(id, &[JobStatus::Queued], JobStatus::Running, None).await? {
                return self.get(id).await;
            }
            // Lost a race with pause/cancel: look again.
        }
    }

    /// Move a job to `to` only if it is currently one of `from`. Returns whether it moved.
    /// `error` is stored (or cleared when `None`).
    pub async fn transition(&self, id: i64, from: &[JobStatus], to: JobStatus, error: Option<&str>) -> Result<bool> {
        let marks = vec!["?"; from.len()].join(",");
        let sql = format!("UPDATE jobs SET status = ?, error = ?, updated_at = ? WHERE id = ? AND status IN ({marks})");
        let mut q = sqlx::query(&sql).bind(to.as_str()).bind(error).bind(now_ms()).bind(id);
        for s in from {
            q = q.bind(s.as_str());
        }
        Ok(q.execute(&self.pool).await?.rows_affected() > 0)
    }

    pub async fn save_progress(&self, id: i64, cursor: Option<&str>, c: &Counters) -> Result<()> {
        sqlx::query(
            "UPDATE jobs SET cursor = ?, scanned = ?, found = ?, deleted = ?, failed = ?, skipped = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(cursor)
        .bind(c.scanned)
        .bind(c.found)
        .bind(c.deleted)
        .bind(c.failed)
        .bind(c.skipped)
        .bind(now_ms())
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// After a crash/quit: jobs left `running` become `paused` (never auto-resume deletions).
    pub async fn recover_interrupted(&self) -> Result<u64> {
        Ok(sqlx::query(
            "UPDATE jobs SET status = 'paused', error = 'Interrupted when the app closed. Resume to continue.', updated_at = ?
             WHERE status = 'running'",
        )
        .bind(now_ms())
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// Delete the record of a finished job, and any messages it collected.
    /// A scan still needed by an unfinished delete job is kept (returns false).
    pub async fn remove_finished(&self, id: i64) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let removed = sqlx::query(
            "DELETE FROM jobs WHERE id = ? AND status IN ('completed','failed','cancelled')
               AND NOT EXISTS (SELECT 1 FROM jobs d WHERE d.source_scan_id = ?1 AND d.status IN ('queued','running','paused'))",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
            > 0;
        if removed {
            sqlx::query("DELETE FROM job_messages WHERE job_id = ?").bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    // ------------------------------------------------------ scan candidates

    /// Save found messages (idempotent: re-scanning a page never duplicates).
    pub async fn add_candidates(&self, job_id: i64, items: &[NewCandidate]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for c in items {
            sqlx::query(
                "INSERT OR IGNORE INTO job_messages (job_id, message_id, ts, preview, attachments) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(job_id)
            .bind(&c.message_id)
            .bind(c.ts)
            .bind(&c.preview)
            .bind(c.attachments)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// `LIKE` pattern for a text search; `%`, `_` and `\` in the input are literal.
    fn like(search: &str) -> String {
        let mut out = String::from("%");
        for ch in search.chars() {
            if matches!(ch, '%' | '_' | '\\') {
                out.push('\\');
            }
            out.push(ch);
        }
        out.push('%');
        out
    }

    /// Newest first.
    pub async fn candidates(&self, job_id: i64, offset: i64, limit: i64, search: Option<&str>) -> Result<Vec<Candidate>> {
        let pattern = search.map(Self::like).unwrap_or_else(|| "%".into());
        sqlx::query_as::<_, Candidate>(
            "SELECT message_id, ts, preview, attachments, excluded, state FROM job_messages
             WHERE job_id = ? AND COALESCE(preview, '') LIKE ? ESCAPE '\\'
             ORDER BY ts DESC, message_id DESC LIMIT ? OFFSET ?",
        )
        .bind(job_id)
        .bind(pattern)
        .bind(limit.clamp(1, 500))
        .bind(offset.max(0))
        .fetch_all(&self.pool)
        .await
    }

    pub async fn count_candidates(&self, job_id: i64, search: Option<&str>) -> Result<i64> {
        let pattern = search.map(Self::like).unwrap_or_else(|| "%".into());
        sqlx::query_scalar("SELECT COUNT(*) FROM job_messages WHERE job_id = ? AND COALESCE(preview, '') LIKE ? ESCAPE '\\'")
            .bind(job_id)
            .bind(pattern)
            .fetch_one(&self.pool)
            .await
    }

    /// Messages that would be deleted: pending and not excluded.
    pub async fn count_pending(&self, job_id: i64) -> Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM job_messages WHERE job_id = ? AND state = 'pending' AND excluded = 0")
            .bind(job_id)
            .fetch_one(&self.pool)
            .await
    }

    /// Next messages a delete job should remove (newest first).
    pub async fn next_pending(&self, job_id: i64, limit: i64) -> Result<Vec<Candidate>> {
        sqlx::query_as::<_, Candidate>(
            "SELECT message_id, ts, preview, attachments, excluded, state FROM job_messages
             WHERE job_id = ? AND state = 'pending' AND excluded = 0
             ORDER BY ts DESC, message_id DESC LIMIT ?",
        )
        .bind(job_id)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await
    }

    pub async fn mark_candidate(&self, job_id: i64, message_id: &str, state: &str) -> Result<bool> {
        Ok(sqlx::query("UPDATE job_messages SET state = ? WHERE job_id = ? AND message_id = ?")
            .bind(state)
            .bind(job_id)
            .bind(message_id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    /// Exclude/include specific messages (only ones still pending).
    pub async fn set_excluded(&self, job_id: i64, ids: &[String], excluded: bool) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let mut n = 0;
        for id in ids {
            n += sqlx::query("UPDATE job_messages SET excluded = ? WHERE job_id = ? AND message_id = ? AND state = 'pending'")
                .bind(excluded)
                .bind(job_id)
                .bind(id)
                .execute(&mut *tx)
                .await?
                .rows_affected();
        }
        tx.commit().await?;
        Ok(n)
    }

    /// Exclude/include every pending message matching the text search (or all).
    pub async fn set_excluded_matching(&self, job_id: i64, search: Option<&str>, excluded: bool) -> Result<u64> {
        let pattern = search.map(Self::like).unwrap_or_else(|| "%".into());
        Ok(sqlx::query(
            "UPDATE job_messages SET excluded = ? WHERE job_id = ? AND state = 'pending' AND COALESCE(preview, '') LIKE ? ESCAPE '\\'",
        )
        .bind(excluded)
        .bind(job_id)
        .bind(pattern)
        .execute(&self.pool)
        .await?
        .rows_affected())
    }

    /// Forget the message text (keeps ids and states). Called once deleting is done.
    pub async fn purge_previews(&self, job_id: i64) -> Result<()> {
        sqlx::query("UPDATE job_messages SET preview = NULL WHERE job_id = ?").bind(job_id).execute(&self.pool).await?;
        Ok(())
    }

    /// Is a non-failed, non-cancelled delete job already based on this scan?
    pub async fn has_delete_for(&self, scan_id: i64) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM jobs WHERE kind = 'delete' AND source_scan_id = ? AND status NOT IN ('cancelled','failed')",
        )
        .bind(scan_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    // ------------------------------------------------------------ protect-list

    pub async fn protect_add(&self, kind: &str, id: &str, name: &str) -> Result<()> {
        sqlx::query("INSERT INTO protected (kind, id, name) VALUES (?, ?, ?) ON CONFLICT(kind, id) DO UPDATE SET name = excluded.name")
            .bind(kind)
            .bind(id)
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn protect_remove(&self, kind: &str, id: &str) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM protected WHERE kind = ? AND id = ?")
            .bind(kind)
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            > 0)
    }

    pub async fn protect_list(&self) -> Result<Vec<Protected>> {
        sqlx::query_as::<_, Protected>("SELECT kind, id, name FROM protected ORDER BY kind, name")
            .fetch_all(&self.pool)
            .await
    }

    /// True if the channel, or the server it belongs to, is on the protect-list.
    pub async fn is_protected(&self, guild_id: Option<&str>, channel_id: &str) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM protected WHERE (kind = 'channel' AND id = ?1) OR (kind = 'guild' AND id = ?2)",
        )
        .bind(channel_id)
        .bind(guild_id.unwrap_or(""))
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_job(kind: JobKind, ch: &str) -> NewJob {
        NewJob {
            kind,
            channel_id: ch.into(),
            channel_name: "general".into(),
            location: "Zebra Club".into(),
            author_id: "42".into(),
            options: "{}".into(),
            guild_id: None,
            source_scan_id: None,
        }
    }

    #[tokio::test]
    async fn create_get_list_roundtrip() {
        let s = Store::in_memory().await.unwrap();
        let a = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        let b = s.create(new_job(JobKind::Delete, "2")).await.unwrap();
        assert_eq!(a.status(), Some(JobStatus::Queued));
        assert_eq!(a.kind(), Some(JobKind::Scan));
        assert_eq!((a.found, a.deleted, a.cursor.clone()), (0, 0, None));
        let list = s.list().await.unwrap();
        assert_eq!(list.iter().map(|j| j.id).collect::<Vec<_>>(), vec![b.id, a.id]); // newest first
        assert_eq!(s.get(999).await.unwrap(), None);
    }

    #[tokio::test]
    async fn claim_is_fifo_and_exclusive() {
        let s = Store::in_memory().await.unwrap();
        let a = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        let b = s.create(new_job(JobKind::Scan, "2")).await.unwrap();
        assert_eq!(s.claim_next().await.unwrap().unwrap().id, a.id);
        assert_eq!(s.status(a.id).await.unwrap(), Some(JobStatus::Running));
        assert_eq!(s.claim_next().await.unwrap().unwrap().id, b.id);
        assert_eq!(s.claim_next().await.unwrap(), None);
    }

    #[tokio::test]
    async fn transition_is_conditional() {
        let s = Store::in_memory().await.unwrap();
        let j = s.create(new_job(JobKind::Delete, "1")).await.unwrap();
        // Can't complete a job that isn't running.
        assert!(!s.transition(j.id, &[JobStatus::Running], JobStatus::Completed, None).await.unwrap());
        s.claim_next().await.unwrap();
        // User pauses while the worker runs; the worker's later "completed" must not override it.
        assert!(s.transition(j.id, &[JobStatus::Running, JobStatus::Queued], JobStatus::Paused, None).await.unwrap());
        assert!(!s.transition(j.id, &[JobStatus::Running], JobStatus::Completed, None).await.unwrap());
        assert_eq!(s.status(j.id).await.unwrap(), Some(JobStatus::Paused));
        // Error text is stored and cleared.
        s.transition(j.id, &[JobStatus::Paused], JobStatus::Failed, Some("boom")).await.unwrap();
        assert_eq!(s.get(j.id).await.unwrap().unwrap().error.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn progress_and_cursor_persist() {
        let s = Store::in_memory().await.unwrap();
        let j = s.create(new_job(JobKind::Delete, "1")).await.unwrap();
        let c = Counters { scanned: 100, found: 40, deleted: 38, failed: 1, skipped: 1 };
        s.save_progress(j.id, Some("123456"), &c).await.unwrap();
        let g = s.get(j.id).await.unwrap().unwrap();
        assert_eq!((g.cursor.as_deref(), g.scanned, g.found, g.deleted, g.failed, g.skipped), (Some("123456"), 100, 40, 38, 1, 1));
    }

    #[tokio::test]
    async fn interrupted_jobs_become_paused_not_resumed() {
        let s = Store::in_memory().await.unwrap();
        let a = s.create(new_job(JobKind::Delete, "1")).await.unwrap();
        let b = s.create(new_job(JobKind::Delete, "2")).await.unwrap();
        s.claim_next().await.unwrap(); // a -> running
        assert_eq!(s.recover_interrupted().await.unwrap(), 1);
        assert_eq!(s.status(a.id).await.unwrap(), Some(JobStatus::Paused));
        assert!(s.get(a.id).await.unwrap().unwrap().error.unwrap().contains("Interrupted"));
        assert_eq!(s.status(b.id).await.unwrap(), Some(JobStatus::Queued)); // untouched
    }

    #[tokio::test]
    async fn only_finished_jobs_can_be_removed() {
        let s = Store::in_memory().await.unwrap();
        let j = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        assert!(!s.remove_finished(j.id).await.unwrap());
        s.transition(j.id, &[JobStatus::Queued], JobStatus::Cancelled, None).await.unwrap();
        assert!(s.remove_finished(j.id).await.unwrap());
        assert_eq!(s.get(j.id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn migrations_are_idempotent_on_a_real_file() {
        let dir = std::env::temp_dir().join(format!("concord-test-{}", std::process::id()));
        let path = dir.join("t.db");
        let s1 = Store::open(&path).await.unwrap();
        s1.create(new_job(JobKind::Scan, "1")).await.unwrap();
        drop(s1);
        let s2 = Store::open(&path).await.unwrap(); // reopening must not re-run CREATE TABLE
        assert_eq!(s2.list().await.unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn cand(id: &str, ts: i64, text: &str) -> NewCandidate {
        NewCandidate { message_id: id.into(), ts, preview: text.into(), attachments: 0 }
    }

    #[tokio::test]
    async fn candidates_are_idempotent_sorted_and_searchable() {
        let s = Store::in_memory().await.unwrap();
        let j = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        s.add_candidates(j.id, &[cand("a", 10, "hello world"), cand("b", 30, "100% sure"), cand("c", 20, "snake_case")]).await.unwrap();
        s.add_candidates(j.id, &[cand("a", 10, "hello world")]).await.unwrap(); // re-scan: no duplicate
        assert_eq!(s.count_candidates(j.id, None).await.unwrap(), 3);
        let all = s.candidates(j.id, 0, 10, None).await.unwrap();
        assert_eq!(all.iter().map(|c| c.message_id.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]); // newest first
        // LIKE wildcards in the search text are literal.
        assert_eq!(s.count_candidates(j.id, Some("100%")).await.unwrap(), 1);
        assert_eq!(s.count_candidates(j.id, Some("snake_c")).await.unwrap(), 1);
        assert_eq!(s.count_candidates(j.id, Some("%")).await.unwrap(), 1); // only the message containing a literal %
        assert_eq!(s.count_candidates(j.id, Some("_")).await.unwrap(), 1);
        assert_eq!(s.count_candidates(j.id, Some("HELLO")).await.unwrap(), 1); // case-insensitive (ASCII)
        // Paging.
        assert_eq!(s.candidates(j.id, 1, 1, None).await.unwrap()[0].message_id, "c");
        assert_eq!(s.candidates(j.id, 5, 10, None).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn exclusion_controls_what_is_pending() {
        let s = Store::in_memory().await.unwrap();
        let j = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        s.add_candidates(j.id, &[cand("a", 1, "keep me"), cand("b", 2, "drop"), cand("c", 3, "drop too")]).await.unwrap();
        assert_eq!(s.count_pending(j.id).await.unwrap(), 3);
        assert_eq!(s.set_excluded(j.id, &["a".into()], true).await.unwrap(), 1);
        assert_eq!(s.count_pending(j.id).await.unwrap(), 2);
        assert_eq!(s.next_pending(j.id, 10).await.unwrap().iter().map(|c| c.message_id.clone()).collect::<Vec<_>>(), ["c", "b"]);
        // Exclude everything matching a search, then re-include one.
        assert_eq!(s.set_excluded_matching(j.id, Some("drop"), true).await.unwrap(), 2);
        assert_eq!(s.count_pending(j.id).await.unwrap(), 0);
        s.set_excluded(j.id, &["b".into()], false).await.unwrap();
        assert_eq!(s.next_pending(j.id, 10).await.unwrap()[0].message_id, "b");
        // Done messages can't be (re-)excluded and don't come back.
        s.mark_candidate(j.id, "b", "deleted").await.unwrap();
        assert_eq!(s.set_excluded(j.id, &["b".into()], true).await.unwrap(), 0);
        assert_eq!(s.count_pending(j.id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn purge_keeps_ids_and_states_but_drops_text() {
        let s = Store::in_memory().await.unwrap();
        let j = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        s.add_candidates(j.id, &[cand("a", 1, "private text")]).await.unwrap();
        s.mark_candidate(j.id, "a", "deleted").await.unwrap();
        s.purge_previews(j.id).await.unwrap();
        let c = &s.candidates(j.id, 0, 10, None).await.unwrap()[0];
        assert_eq!((c.preview.clone(), c.state.as_str()), (None, "deleted"));
    }

    #[tokio::test]
    async fn removing_a_job_removes_its_messages_but_scans_in_use_stay() {
        let s = Store::in_memory().await.unwrap();
        let scan = s.create(new_job(JobKind::Scan, "1")).await.unwrap();
        s.add_candidates(scan.id, &[cand("a", 1, "x")]).await.unwrap();
        s.transition(scan.id, &[JobStatus::Queued], JobStatus::Completed, None).await.unwrap();
        // An unfinished delete job needs this scan's list.
        let mut nd = new_job(JobKind::Delete, "1");
        nd.source_scan_id = Some(scan.id);
        let del = s.create(nd).await.unwrap();
        assert!(s.has_delete_for(scan.id).await.unwrap());
        assert!(!s.remove_finished(scan.id).await.unwrap(), "scan in use must stay");
        assert_eq!(s.count_candidates(scan.id, None).await.unwrap(), 1);
        // Once the delete job is cancelled, the scan can go, and its messages with it.
        s.transition(del.id, &[JobStatus::Queued], JobStatus::Cancelled, None).await.unwrap();
        assert!(!s.has_delete_for(scan.id).await.unwrap());
        assert!(s.remove_finished(scan.id).await.unwrap());
        assert_eq!(s.count_candidates(scan.id, None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn protect_list_covers_channels_and_servers() {
        let s = Store::in_memory().await.unwrap();
        assert!(!s.is_protected(Some("g1"), "c1").await.unwrap());
        s.protect_add("channel", "c1", "#general").await.unwrap();
        s.protect_add("guild", "g2", "Family").await.unwrap();
        s.protect_add("guild", "g2", "Family 2").await.unwrap(); // upsert renames
        assert!(s.is_protected(Some("g1"), "c1").await.unwrap(), "protected channel");
        assert!(s.is_protected(Some("g2"), "c9").await.unwrap(), "any channel of a protected server");
        assert!(!s.is_protected(Some("g1"), "c9").await.unwrap());
        assert!(!s.is_protected(None, "c9").await.unwrap(), "no guild (DM) and channel not protected");
        let l = s.protect_list().await.unwrap();
        assert_eq!(l.len(), 2);
        assert!(l.iter().any(|p| p.kind == "guild" && p.name == "Family 2"));
        assert!(s.protect_remove("guild", "g2").await.unwrap());
        assert!(!s.protect_remove("guild", "g2").await.unwrap());
        assert!(!s.is_protected(Some("g2"), "c9").await.unwrap());
    }

    #[tokio::test]
    async fn guild_and_source_columns_roundtrip() {
        let s = Store::in_memory().await.unwrap();
        let mut nj = new_job(JobKind::Delete, "5");
        nj.guild_id = Some("77".into());
        nj.source_scan_id = Some(3);
        let j = s.create(nj).await.unwrap();
        assert_eq!((j.guild_id.as_deref(), j.source_scan_id), (Some("77"), Some(3)));
    }
}
