//! Cleanup jobs: a persistent queue, one worker, and the commands the UI uses.
//!
//! Jobs run one at a time (never in parallel) to stay well inside Discord's
//! rate limits. Pause/cancel take effect between requests. On start-up, jobs
//! that were running are set to `paused`, never resumed automatically.
//!
//! The flow is scan -> review -> delete:
//!  1. a **scan** job collects the user's own messages that match the filters;
//!  2. the user reviews the list and can exclude individual messages;
//!  3. a **delete** job removes exactly that reviewed list.

pub mod engine;
pub mod filters;
pub mod pacing;
pub mod store;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

use crate::auth::{AuthState, EVENT_AUTH_CHANGED};
use crate::discord::{model, ApiError, CurrentUser};
use engine::{Engine, JobOptions, Reporter};
use filters::Filters;
use pacing::Speed;
use store::{Candidate, Job, JobKind, JobStatus, NewJob, Protected, Store};

pub const EVENT_JOB_UPDATED: &str = "job-updated";
const MAX_JOBS_PER_REQUEST: usize = 500;
/// Typed by the user to start a job that deletes messages.
pub const DELETE_CONFIRMATION: &str = "DELETE";

pub struct JobsState {
    store: Store,
    /// Wakes the worker when there may be queued work.
    queue: Arc<Notify>,
    /// Wakes a running job's waits after pause/cancel.
    control: Arc<Notify>,
}

struct TauriReporter {
    app: AppHandle,
}

impl Reporter for TauriReporter {
    fn job_updated(&self, job: &Job) {
        let _ = self.app.emit(EVENT_JOB_UPDATED, job);
    }

    fn signed_out(&self) {
        let auth = self.app.state::<AuthState>();
        let _ = auth.store.delete();
        let _ = self.app.emit(EVENT_AUTH_CHANGED, Option::<CurrentUser>::None);
    }
}

/// Open the database, recover interrupted jobs, and start the worker.
pub async fn init(app: &AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let store = Store::open(&dir.join("concord.db")).await.map_err(|e| e.to_string())?;
    store.recover_interrupted().await.map_err(|e| e.to_string())?;

    let queue = Arc::new(Notify::new());
    let control = Arc::new(Notify::new());
    let auth = app.state::<AuthState>();
    let engine = Engine {
        store: store.clone(),
        client: auth.client.clone(),
        tokens: auth.store.clone(),
        reporter: Arc::new(TauriReporter { app: app.clone() }),
        control: control.clone(),
        pacing_override: None,
    };

    tauri::async_runtime::spawn(worker_loop(engine, queue.clone()));

    app.manage(JobsState { store, queue, control });
    Ok(())
}

/// The single worker: claim the oldest queued job, run it to a stop, repeat.
/// Paused/cancelled jobs are never claimed, so they are simply skipped.
pub async fn worker_loop(engine: Engine, queue: Arc<Notify>) {
    loop {
        match engine.store.claim_next().await {
            Ok(Some(job)) => engine.run(job).await,
            Ok(None) => queue.notified().await,
            Err(_) => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }
}

/// Pause every queued or running job. Returns the jobs that were paused.
pub async fn pause_all_jobs(store: &Store) -> Result<Vec<Job>, sqlx::Error> {
    let mut paused = Vec::new();
    for j in store.list().await? {
        if matches!(j.status(), Some(JobStatus::Queued | JobStatus::Running))
            && store.transition(j.id, &[JobStatus::Queued, JobStatus::Running], JobStatus::Paused, None).await?
        {
            if let Some(updated) = store.get(j.id).await? {
                paused.push(updated);
            }
        }
    }
    Ok(paused)
}

/// The tray's "Pause all jobs".
pub async fn pause_all(app: AppHandle) {
    let Some(jobs) = app.try_state::<JobsState>() else { return };
    if let Ok(paused) = pause_all_jobs(&jobs.store).await {
        for j in &paused {
            emit(&app, j);
        }
        jobs.control.notify_one(); // wake a running job's waits so it stops promptly
    }
}

// ------------------------------------------------------------ job planning

#[derive(Debug, Clone, Deserialize)]
pub struct ChannelRef {
    pub id: String,
    pub name: String,
    pub location: String,
    /// The server the channel belongs to; `None` for DMs.
    #[serde(default)]
    pub guild_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateJobs {
    /// "scan" or "delete".
    pub kind: String,
    /// For a scan: the channels to scan.
    #[serde(default)]
    pub channels: Vec<ChannelRef>,
    /// For a delete: the finished scans whose reviewed lists to delete.
    #[serde(default)]
    pub scan_ids: Vec<i64>,
    pub speed: Option<Speed>,
    pub skip_pinned: Option<bool>,
    pub filters: Option<Filters>,
    /// Must equal `DELETE_CONFIRMATION` for kind = "delete".
    pub confirm: Option<String>,
}

fn clip(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn options_json(o: &JobOptions) -> Result<String, String> {
    serde_json::to_string(o).map_err(|e| e.to_string())
}

fn db_err(e: sqlx::Error) -> String {
    format!("Database error: {e}")
}

/// Decide which jobs to create, or why not. Every rule that must hold before a
/// job exists lives here, separate from Discord and Tauri, so it is tested.
pub async fn plan_jobs(store: &Store, me: &str, req: &CreateJobs) -> Result<Vec<NewJob>, String> {
    match JobKind::parse(&req.kind).ok_or("Unknown job type")? {
        JobKind::Scan => plan_scans(store, me, req).await,
        JobKind::Delete => plan_deletes(store, me, req).await,
    }
}

async fn plan_scans(store: &Store, me: &str, req: &CreateJobs) -> Result<Vec<NewJob>, String> {
    if req.channels.is_empty() {
        return Err("Pick at least one channel.".into());
    }
    if req.channels.len() > MAX_JOBS_PER_REQUEST {
        return Err(format!("Too many channels at once (max {MAX_JOBS_PER_REQUEST})."));
    }
    let options = options_json(&JobOptions {
        speed: req.speed.unwrap_or_default(),
        skip_pinned: req.skip_pinned.unwrap_or(true),
        filters: req.filters.clone().unwrap_or_default().normalized()?,
    })?;

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for c in &req.channels {
        if !model::is_snowflake(&c.id) || c.guild_id.as_deref().is_some_and(|g| !model::is_snowflake(g)) {
            return Err("Invalid channel id.".into());
        }
        if !seen.insert(c.id.clone()) {
            continue;
        }
        if store.is_protected(c.guild_id.as_deref(), &c.id).await.map_err(db_err)? {
            return Err(format!("\"{}\" is on your protect-list. Remove it from the list first.", clip(&c.name, 60)));
        }
        out.push(NewJob {
            kind: JobKind::Scan,
            channel_id: c.id.clone(),
            channel_name: clip(&c.name, 100),
            location: clip(&c.location, 100),
            author_id: me.to_owned(),
            options: options.clone(),
            guild_id: c.guild_id.clone(),
            source_scan_id: None,
        });
    }
    Ok(out)
}

async fn plan_deletes(store: &Store, me: &str, req: &CreateJobs) -> Result<Vec<NewJob>, String> {
    if req.confirm.as_deref() != Some(DELETE_CONFIRMATION) {
        return Err(format!("Deleting needs the confirmation word \"{DELETE_CONFIRMATION}\"."));
    }
    if req.scan_ids.is_empty() {
        return Err("Pick at least one reviewed scan to delete from.".into());
    }
    if req.scan_ids.len() > MAX_JOBS_PER_REQUEST {
        return Err(format!("Too many scans at once (max {MAX_JOBS_PER_REQUEST})."));
    }

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &id in &req.scan_ids {
        if !seen.insert(id) {
            continue;
        }
        let scan = store.get(id).await.map_err(db_err)?.ok_or("That scan no longer exists.")?;
        let label = clip(&scan.channel_name, 60);
        if scan.kind() != Some(JobKind::Scan) || scan.status() != Some(JobStatus::Completed) {
            return Err(format!("The scan of \"{label}\" isn't finished yet."));
        }
        // The author is fixed when the scan is created; never delete across accounts.
        if scan.author_id != me {
            return Err(format!("The scan of \"{label}\" was made with a different account."));
        }
        if store.has_delete_for(scan.id).await.map_err(db_err)? {
            return Err(format!("\"{label}\" is already being deleted."));
        }
        if store.is_protected(scan.guild_id.as_deref(), &scan.channel_id).await.map_err(db_err)? {
            return Err(format!("\"{label}\" is on your protect-list. Remove it from the list first."));
        }
        if store.count_pending(scan.id).await.map_err(db_err)? == 0 {
            continue; // nothing left to delete there (all excluded or already done)
        }
        let mut opts = serde_json::from_str::<JobOptions>(&scan.options).unwrap_or_default();
        if let Some(speed) = req.speed {
            opts.speed = speed;
        }
        out.push(NewJob {
            kind: JobKind::Delete,
            channel_id: scan.channel_id.clone(),
            channel_name: scan.channel_name.clone(),
            location: scan.location.clone(),
            author_id: scan.author_id.clone(),
            options: options_json(&opts)?,
            guild_id: scan.guild_id.clone(),
            source_scan_id: Some(scan.id),
        });
    }
    if out.is_empty() {
        return Err("Nothing is selected for deletion.".into());
    }
    Ok(out)
}

/// A scan whose list can still be edited: finished, and not being deleted yet.
async fn ensure_reviewable(store: &Store, scan_id: i64) -> Result<(), String> {
    let scan = store.get(scan_id).await.map_err(db_err)?.ok_or("That scan no longer exists.")?;
    if scan.kind() != Some(JobKind::Scan) || scan.status() != Some(JobStatus::Completed) {
        return Err("Only a finished scan can be reviewed.".into());
    }
    if store.has_delete_for(scan_id).await.map_err(db_err)? {
        return Err("This list is already being deleted and can't be changed.".into());
    }
    Ok(())
}

// ------------------------------------------------------------------ commands

fn emit(app: &AppHandle, job: &Job) {
    let _ = app.emit(EVENT_JOB_UPDATED, job);
}

#[tauri::command]
pub async fn jobs_list(jobs: State<'_, JobsState>) -> Result<Vec<Job>, String> {
    jobs.store.list().await.map_err(db_err)
}

#[tauri::command]
pub async fn jobs_create(
    app: AppHandle,
    auth: State<'_, AuthState>,
    jobs: State<'_, JobsState>,
    req: CreateJobs,
) -> Result<Vec<Job>, String> {
    // The author id is the signed-in user: the engine will touch only their messages.
    let token = auth.store.load().map_err(|e| e.to_string())?.ok_or("Not signed in")?;
    let me = match auth.client.current_user(&token).await {
        Ok(u) => u,
        Err(ApiError::Unauthorized) => {
            let _ = auth.store.delete();
            let _ = app.emit(EVENT_AUTH_CHANGED, Option::<CurrentUser>::None);
            return Err(ApiError::Unauthorized.to_string());
        }
        Err(e) => return Err(e.to_string()),
    };

    let plan = plan_jobs(&jobs.store, &me.id, &req).await?;
    let mut created = Vec::new();
    for nj in plan {
        let job = jobs.store.create(nj).await.map_err(db_err)?;
        emit(&app, &job);
        created.push(job);
    }
    jobs.queue.notify_one();
    Ok(created)
}

async fn change(
    app: &AppHandle,
    jobs: &JobsState,
    id: i64,
    from: &[JobStatus],
    to: JobStatus,
) -> Result<(), String> {
    let moved = jobs.store.transition(id, from, to, None).await.map_err(db_err)?;
    if !moved {
        return Err("That job can't be changed right now.".into());
    }
    if let Some(job) = jobs.store.get(id).await.map_err(db_err)? {
        emit(app, &job);
    }
    Ok(())
}

#[tauri::command]
pub async fn job_pause(app: AppHandle, jobs: State<'_, JobsState>, id: i64) -> Result<(), String> {
    change(&app, &jobs, id, &[JobStatus::Queued, JobStatus::Running], JobStatus::Paused).await?;
    jobs.control.notify_one();
    Ok(())
}

#[tauri::command]
pub async fn job_resume(app: AppHandle, jobs: State<'_, JobsState>, id: i64) -> Result<(), String> {
    change(&app, &jobs, id, &[JobStatus::Paused], JobStatus::Queued).await?;
    jobs.queue.notify_one();
    Ok(())
}

#[tauri::command]
pub async fn job_cancel(app: AppHandle, jobs: State<'_, JobsState>, id: i64) -> Result<(), String> {
    change(&app, &jobs, id, &[JobStatus::Queued, JobStatus::Running, JobStatus::Paused], JobStatus::Cancelled).await?;
    jobs.control.notify_one();
    Ok(())
}

/// Remove the record of a finished job (and the message list it collected).
#[tauri::command]
pub async fn job_remove(jobs: State<'_, JobsState>, id: i64) -> Result<(), String> {
    if jobs.store.remove_finished(id).await.map_err(db_err)? {
        Ok(())
    } else {
        Err("That job can't be removed right now (still running, or a delete is using its list).".into())
    }
}

#[derive(Debug, Serialize)]
pub struct CandidatePage {
    /// Messages matching the search.
    pub total: i64,
    /// Messages that will be deleted: pending and not excluded (whole list, ignoring the search).
    pub included: i64,
    pub items: Vec<Candidate>,
}

fn search_text(s: Option<String>) -> Option<String> {
    s.map(|s| clip(s.trim(), 100)).filter(|s| !s.is_empty())
}

#[tauri::command]
pub async fn job_candidates(
    jobs: State<'_, JobsState>,
    id: i64,
    offset: i64,
    limit: i64,
    search: Option<String>,
) -> Result<CandidatePage, String> {
    let search = search_text(search);
    let items = jobs.store.candidates(id, offset, limit, search.as_deref()).await.map_err(db_err)?;
    let total = jobs.store.count_candidates(id, search.as_deref()).await.map_err(db_err)?;
    let included = jobs.store.count_pending(id).await.map_err(db_err)?;
    Ok(CandidatePage { total, included, items })
}

/// Exclude (`excluded = true`) or include messages in a scan's list. Returns how many will be deleted.
#[tauri::command]
pub async fn job_candidates_exclude(
    jobs: State<'_, JobsState>,
    id: i64,
    message_ids: Vec<String>,
    excluded: bool,
) -> Result<i64, String> {
    ensure_reviewable(&jobs.store, id).await?;
    if message_ids.len() > 1000 || message_ids.iter().any(|m| !model::is_snowflake(m)) {
        return Err("Invalid message selection.".into());
    }
    jobs.store.set_excluded(id, &message_ids, excluded).await.map_err(db_err)?;
    jobs.store.count_pending(id).await.map_err(db_err)
}

/// Exclude/include every message matching the search text (or the whole list).
#[tauri::command]
pub async fn job_candidates_exclude_matching(
    jobs: State<'_, JobsState>,
    id: i64,
    search: Option<String>,
    excluded: bool,
) -> Result<i64, String> {
    ensure_reviewable(&jobs.store, id).await?;
    let search = search_text(search);
    jobs.store.set_excluded_matching(id, search.as_deref(), excluded).await.map_err(db_err)?;
    jobs.store.count_pending(id).await.map_err(db_err)
}

#[tauri::command]
pub async fn protect_list(jobs: State<'_, JobsState>) -> Result<Vec<Protected>, String> {
    jobs.store.protect_list().await.map_err(db_err)
}

fn check_protect_target(kind: &str, id: &str) -> Result<(), String> {
    if !matches!(kind, "channel" | "guild") || !model::is_snowflake(id) {
        return Err("Invalid protect-list entry.".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn protect_add(jobs: State<'_, JobsState>, kind: String, id: String, name: String) -> Result<(), String> {
    check_protect_target(&kind, &id)?;
    jobs.store.protect_add(&kind, &id, &clip(&name, 100)).await.map_err(db_err)
}

#[tauri::command]
pub async fn protect_remove(jobs: State<'_, JobsState>, kind: String, id: String) -> Result<(), String> {
    check_protect_target(&kind, &id)?;
    jobs.store.protect_remove(&kind, &id).await.map_err(db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::token_store::{MemoryStore, TokenStore};
    use pacing::PacingConfig;
    use store::{Counters, NewCandidate};
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ME: &str = "42";

    fn ch(id: &str, guild: Option<&str>) -> ChannelRef {
        ChannelRef { id: id.into(), name: "general".into(), location: "Zebra".into(), guild_id: guild.map(Into::into) }
    }

    fn scan_req(channels: Vec<ChannelRef>) -> CreateJobs {
        CreateJobs {
            kind: "scan".into(),
            channels,
            scan_ids: vec![],
            speed: None,
            skip_pinned: None,
            filters: None,
            confirm: None,
        }
    }

    fn delete_req(scan_ids: Vec<i64>, confirm: Option<&str>) -> CreateJobs {
        CreateJobs {
            kind: "delete".into(),
            channels: vec![],
            scan_ids,
            speed: None,
            skip_pinned: None,
            filters: None,
            confirm: confirm.map(Into::into),
        }
    }

    fn nj(kind: JobKind, ch: &str, source: Option<i64>) -> NewJob {
        NewJob {
            kind,
            channel_id: ch.into(),
            channel_name: "c".into(),
            location: "L".into(),
            author_id: ME.into(),
            options: "{}".into(),
            guild_id: None,
            source_scan_id: source,
        }
    }

    fn cand(id: &str) -> NewCandidate {
        NewCandidate { message_id: id.into(), ts: 1, preview: "x".into(), attachments: 0 }
    }

    /// A finished scan of channel `ch` by `ME` with candidates 10 and 11.
    async fn finished_scan(store: &Store, ch: &str, guild: Option<&str>) -> i64 {
        let mut n = nj(JobKind::Scan, ch, None);
        n.guild_id = guild.map(Into::into);
        let j = store.create(n).await.unwrap();
        store.add_candidates(j.id, &[cand("10"), cand("11")]).await.unwrap();
        store.transition(j.id, &[JobStatus::Queued], JobStatus::Completed, None).await.unwrap();
        j.id
    }

    // ---- pause all (tray)

    #[tokio::test]
    async fn pause_all_pauses_only_queued_and_running_jobs() {
        let s = Store::in_memory().await.unwrap();
        let running = s.create(nj(JobKind::Scan, "1", None)).await.unwrap();
        s.claim_next().await.unwrap(); // -> running
        let queued = s.create(nj(JobKind::Scan, "2", None)).await.unwrap();
        let paused = s.create(nj(JobKind::Scan, "3", None)).await.unwrap();
        s.transition(paused.id, &[JobStatus::Queued], JobStatus::Paused, Some("by user")).await.unwrap();
        let done = s.create(nj(JobKind::Scan, "4", None)).await.unwrap();
        s.transition(done.id, &[JobStatus::Queued], JobStatus::Completed, None).await.unwrap();

        let changed = pause_all_jobs(&s).await.unwrap();
        let mut ids: Vec<i64> = changed.iter().map(|j| j.id).collect();
        ids.sort();
        assert_eq!(ids, vec![running.id, queued.id]);
        assert!(changed.iter().all(|j| j.status() == Some(JobStatus::Paused)));
        assert_eq!(s.status(running.id).await.unwrap(), Some(JobStatus::Paused));
        assert_eq!(s.status(done.id).await.unwrap(), Some(JobStatus::Completed), "finished jobs untouched");
        assert_eq!(s.get(paused.id).await.unwrap().unwrap().error.as_deref(), Some("by user"), "already-paused job left alone");
        assert!(pause_all_jobs(&s).await.unwrap().is_empty(), "idempotent");
    }

    // ---- planning scans

    #[tokio::test]
    async fn scan_plan_applies_defaults_and_filters() {
        let s = Store::in_memory().await.unwrap();
        let mut req = scan_req(vec![ch("1", Some("9")), ch("2", None)]);
        req.filters = Some(Filters { contains: Some("  hi ".into()), only_with_links: true, ..Default::default() });
        req.speed = Some(Speed::Normal);
        let plan = plan_jobs(&s, ME, &req).await.unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!((plan[0].kind, plan[0].author_id.as_str(), plan[0].guild_id.as_deref()), (JobKind::Scan, ME, Some("9")));
        let o: JobOptions = serde_json::from_str(&plan[0].options).unwrap();
        assert_eq!((o.speed, o.skip_pinned), (Speed::Normal, true), "pinned kept by default");
        assert_eq!(o.filters.contains.as_deref(), Some("hi"), "keyword trimmed");
        assert!(o.filters.only_with_links);
    }

    #[tokio::test]
    async fn scan_needs_no_confirmation_but_rejects_bad_input() {
        let s = Store::in_memory().await.unwrap();
        assert!(plan_jobs(&s, ME, &scan_req(vec![ch("1", None)])).await.is_ok());
        assert!(plan_jobs(&s, ME, &scan_req(vec![])).await.is_err());
        assert!(plan_jobs(&s, ME, &scan_req(vec![ch("../x", None)])).await.is_err());
        assert!(plan_jobs(&s, ME, &scan_req(vec![ch("1", Some("not-a-number"))])).await.is_err());
        let mut bad = scan_req(vec![ch("1", None)]);
        bad.kind = "nuke".into();
        assert!(plan_jobs(&s, ME, &bad).await.is_err());
        let many: Vec<_> = (0..=MAX_JOBS_PER_REQUEST).map(|i| ch(&(i + 1).to_string(), None)).collect();
        assert!(plan_jobs(&s, ME, &scan_req(many)).await.is_err());
        // Impossible date range.
        let mut r = scan_req(vec![ch("1", None)]);
        r.filters = Some(Filters { after_ms: Some(10), before_ms: Some(5), ..Default::default() });
        assert!(plan_jobs(&s, ME, &r).await.is_err());
    }

    #[tokio::test]
    async fn scan_plan_dedupes_and_clips_names() {
        let s = Store::in_memory().await.unwrap();
        let mut long = ch("5", None);
        long.name = "x".repeat(300);
        let plan = plan_jobs(&s, ME, &scan_req(vec![ch("5", None), long, ch("6", None)])).await.unwrap();
        assert_eq!(plan.iter().map(|p| p.channel_id.as_str()).collect::<Vec<_>>(), ["5", "6"]);
        assert_eq!(plan[0].channel_name, "general");
    }

    #[tokio::test]
    async fn protected_channels_and_servers_cannot_be_scanned() {
        let s = Store::in_memory().await.unwrap();
        s.protect_add("channel", "1", "#general").await.unwrap();
        s.protect_add("guild", "9", "Family").await.unwrap();
        let e1 = plan_jobs(&s, ME, &scan_req(vec![ch("1", None)])).await.unwrap_err();
        assert!(e1.contains("protect-list"));
        assert!(plan_jobs(&s, ME, &scan_req(vec![ch("2", Some("9"))])).await.is_err(), "any channel of a protected server");
        assert!(plan_jobs(&s, ME, &scan_req(vec![ch("2", Some("8"))])).await.is_ok());
    }

    // ---- planning deletes

    #[tokio::test]
    async fn delete_requires_the_exact_confirmation_word() {
        let s = Store::in_memory().await.unwrap();
        let scan = finished_scan(&s, "1", None).await;
        for bad in [None, Some(""), Some("delete"), Some("yes"), Some("DELETE ")] {
            assert!(plan_jobs(&s, ME, &delete_req(vec![scan], bad)).await.is_err(), "{bad:?}");
        }
        assert!(plan_jobs(&s, ME, &delete_req(vec![scan], Some("DELETE"))).await.is_ok());
    }

    #[tokio::test]
    async fn delete_plan_is_built_from_the_scan_not_from_the_request() {
        let s = Store::in_memory().await.unwrap();
        let scan = finished_scan(&s, "77", Some("5")).await;
        let mut req = delete_req(vec![scan, scan], Some("DELETE")); // duplicate id
        req.speed = Some(Speed::Normal);
        let plan = plan_jobs(&s, ME, &req).await.unwrap();
        assert_eq!(plan.len(), 1, "deduped");
        let p = &plan[0];
        assert_eq!((p.kind, p.channel_id.as_str(), p.guild_id.as_deref(), p.source_scan_id), (JobKind::Delete, "77", Some("5"), Some(scan)));
        assert_eq!(p.author_id, ME);
        assert_eq!(serde_json::from_str::<JobOptions>(&p.options).unwrap().speed, Speed::Normal);
    }

    #[tokio::test]
    async fn delete_refuses_missing_unfinished_foreign_busy_and_protected_scans() {
        let s = Store::in_memory().await.unwrap();
        let ok = |id: i64| delete_req(vec![id], Some("DELETE"));
        assert!(plan_jobs(&s, ME, &ok(999)).await.is_err(), "unknown scan");

        // Unfinished scan.
        let running = s.create(nj(JobKind::Scan, "1", None)).await.unwrap();
        assert!(plan_jobs(&s, ME, &ok(running.id)).await.unwrap_err().contains("isn't finished"));

        // A DELETE job id is not a scan.
        let scan = finished_scan(&s, "2", None).await;
        let del = s.create(nj(JobKind::Delete, "2", Some(scan))).await.unwrap();
        assert!(plan_jobs(&s, ME, &ok(del.id)).await.is_err());

        // Already being deleted (the delete job above is queued).
        assert!(plan_jobs(&s, ME, &ok(scan)).await.unwrap_err().contains("already being deleted"));
        s.transition(del.id, &[JobStatus::Queued], JobStatus::Cancelled, None).await.unwrap();
        assert!(plan_jobs(&s, ME, &ok(scan)).await.is_ok(), "cancelled delete frees the scan");

        // Different account.
        assert!(plan_jobs(&s, "7", &ok(scan)).await.unwrap_err().contains("different account"));

        // Protected after the scan.
        s.protect_add("channel", "2", "x").await.unwrap();
        assert!(plan_jobs(&s, ME, &ok(scan)).await.unwrap_err().contains("protect-list"));
    }

    #[tokio::test]
    async fn delete_skips_scans_with_nothing_left_and_errors_if_nothing_remains() {
        let s = Store::in_memory().await.unwrap();
        let a = finished_scan(&s, "1", None).await;
        let b = finished_scan(&s, "2", None).await;
        s.set_excluded_matching(a, None, true).await.unwrap(); // user excluded everything in A
        let plan = plan_jobs(&s, ME, &delete_req(vec![a, b], Some("DELETE"))).await.unwrap();
        assert_eq!(plan.iter().map(|p| p.channel_id.as_str()).collect::<Vec<_>>(), ["2"]);
        let err = plan_jobs(&s, ME, &delete_req(vec![a], Some("DELETE"))).await.unwrap_err();
        assert!(err.contains("Nothing is selected"));
    }

    #[tokio::test]
    async fn review_edits_only_allowed_on_a_finished_scan_not_yet_being_deleted() {
        let s = Store::in_memory().await.unwrap();
        let running = s.create(nj(JobKind::Scan, "1", None)).await.unwrap();
        assert!(ensure_reviewable(&s, running.id).await.is_err());
        let scan = finished_scan(&s, "2", None).await;
        assert!(ensure_reviewable(&s, scan).await.is_ok());
        let del = s.create(nj(JobKind::Delete, "2", Some(scan))).await.unwrap();
        assert!(ensure_reviewable(&s, scan).await.unwrap_err().contains("already being deleted"));
        assert!(ensure_reviewable(&s, del.id).await.is_err(), "a delete job is not reviewable");
        assert!(ensure_reviewable(&s, 12345).await.is_err());
    }

    #[test]
    fn protect_targets_are_validated() {
        assert!(check_protect_target("channel", "123").is_ok());
        assert!(check_protect_target("guild", "123").is_ok());
        assert!(check_protect_target("user", "123").is_err());
        assert!(check_protect_target("guild", "../x").is_err());
        assert!(check_protect_target("guild", "").is_err());
    }

    #[test]
    fn search_text_is_trimmed_clipped_and_empty_means_none() {
        assert_eq!(search_text(Some("  hi ".into())).as_deref(), Some("hi"));
        assert_eq!(search_text(Some("   ".into())), None);
        assert_eq!(search_text(None), None);
        assert_eq!(search_text(Some("x".repeat(500))).unwrap().chars().count(), 100);
    }

    // ---- worker integration (real worker loop + engine + store, mock Discord)

    struct NullReporter;
    impl Reporter for NullReporter {
        fn job_updated(&self, _: &Job) {}
        fn signed_out(&self) {}
    }

    /// Mock Discord (every channel empty, so scans finish at once), an in-memory
    /// store, and an engine. The worker is NOT started: tests set up first.
    async fn worker_fixture() -> (MockServer, Store, Arc<Notify>, Engine) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/channels/\d+/messages$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;
        let tokens = MemoryStore::default();
        tokens.save("tok").unwrap();
        let store = Store::in_memory().await.unwrap();
        let engine = Engine {
            store: store.clone(),
            client: crate::discord::DiscordClient::new(server.uri()),
            tokens: Arc::new(tokens),
            reporter: Arc::new(NullReporter),
            control: Arc::new(Notify::new()),
            pacing_override: Some(PacingConfig {
                list_gap: Duration::ZERO,
                delete_gap: Duration::ZERO,
                margin: Duration::from_millis(5),
                backoff_base: Duration::from_millis(1),
                jitter: false,
            }),
        };
        (server, store, Arc::new(Notify::new()), engine)
    }

    async fn wait_for_status(store: &Store, id: i64, want: JobStatus) -> bool {
        for _ in 0..100 {
            if store.status(id).await.unwrap() == Some(want) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        false
    }

    #[tokio::test]
    async fn worker_skips_a_paused_job_and_runs_the_next() {
        let (_server, store, queue, engine) = worker_fixture().await;
        let a = store.create(nj(JobKind::Scan, "501", None)).await.unwrap();
        let b = store.create(nj(JobKind::Scan, "502", None)).await.unwrap();
        assert!(store.transition(a.id, &[JobStatus::Queued], JobStatus::Paused, None).await.unwrap());
        let handle = tokio::spawn(worker_loop(engine, queue.clone()));
        assert!(wait_for_status(&store, b.id, JobStatus::Completed).await, "next job must run");
        assert_eq!(store.status(a.id).await.unwrap(), Some(JobStatus::Paused), "paused job stays paused");
        handle.abort();
    }

    #[tokio::test]
    async fn worker_wakes_for_jobs_created_while_idle() {
        let (_server, store, queue, engine) = worker_fixture().await;
        let handle = tokio::spawn(worker_loop(engine, queue.clone()));
        tokio::time::sleep(Duration::from_millis(150)).await; // worker is idle, waiting
        let j = store.create(nj(JobKind::Scan, "503", None)).await.unwrap();
        queue.notify_one();
        assert!(wait_for_status(&store, j.id, JobStatus::Completed).await);
        handle.abort();
    }

    #[tokio::test]
    async fn worker_runs_queued_jobs_one_after_another() {
        let (_server, store, queue, engine) = worker_fixture().await;
        let mut ids = Vec::new();
        for c in ["601", "602", "603"] {
            ids.push(store.create(nj(JobKind::Scan, c, None)).await.unwrap().id);
        }
        let handle = tokio::spawn(worker_loop(engine, queue.clone()));
        for id in &ids {
            assert!(wait_for_status(&store, *id, JobStatus::Completed).await, "job {id}");
        }
        handle.abort();
    }

    #[tokio::test]
    async fn resumed_job_is_picked_up_again_with_its_saved_progress() {
        let (_server, store, queue, engine) = worker_fixture().await;
        let j = store.create(nj(JobKind::Scan, "700", None)).await.unwrap();
        assert!(store.transition(j.id, &[JobStatus::Queued], JobStatus::Paused, None).await.unwrap());
        store.save_progress(j.id, Some("123"), &Counters { scanned: 9, found: 0, ..Default::default() }).await.unwrap();
        let handle = tokio::spawn(worker_loop(engine, queue.clone()));
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(store.status(j.id).await.unwrap(), Some(JobStatus::Paused), "not run while paused");
        // User resumes: paused -> queued, then the worker is woken.
        assert!(store.transition(j.id, &[JobStatus::Paused], JobStatus::Queued, None).await.unwrap());
        queue.notify_one();
        assert!(wait_for_status(&store, j.id, JobStatus::Completed).await);
        let g = store.get(j.id).await.unwrap().unwrap();
        assert_eq!((g.scanned, g.cursor.as_deref()), (9, Some("123")), "earlier progress kept");
        handle.abort();
    }
}
