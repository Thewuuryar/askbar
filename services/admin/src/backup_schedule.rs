//! Scheduled backups (ADR 0013, D27 to D29, plan step 6.4): the admin makes a backup every day at 03:30 UTC and keeps it on the `backup` volume
//! (`/backup`), where the host may have mounted a NAS share.
//!
//! - **All or nothing.** A backup is written to `.askbar-backup-<time>.tar.partial`, synced, and renamed to `askbar-backup-<time>.tar`. A
//!   partial file is never taken for a backup, and one left by a crash is removed at the next run.
//! - **An unavailable destination does not keep the admin down.** A share that has gone away makes filesystem calls hang, not fail. Every
//!   call on `/backup` runs on a blocking thread with a time limit; one that does not answer in time is given up on, the run fails with a
//!   sentence, and while an earlier call is still stuck no new one is started, so at most two threads are ever held. Nothing the admin does
//!   for anyone else waits on this.
//! - **Retention.** After a backup, the files are kept while they are among the newest `keep_files` (5) and no older than `keep_days` (30);
//!   the newest is always kept, whatever the limits say. The admin can change both.
//! - **Alerts.** `backup_failed` when a run fails (once a day for a failure that goes on), and `backup_missed` when no backup has succeeded
//!   for 26 hours (once for each miss). Neither carries a path or a reason, only the kind and, for a miss, the minutes.
//! - **The restore drill** is due every 90 days: the status says so, and "drill done" starts the count again.
//! - **Nothing without a key.** With no backup key set, no backup is made and none is missed.
//!
//! What a person sees: whether `/backup` is writable, whether it is on the same disk as Askbar (`st_dev`: a backup there does not survive
//! the disk), the files in it, the limits, when the last backup was tried and how it ended, and when the next is due.

use crate::admin_api::St;
use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::alerter::{Fields, Kind};
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::ErrorCode;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 03:30 UTC, in seconds after midnight.
pub const RUN_AT_SECS: i64 = 3 * 3600 + 30 * 60;
/// No successful backup for this long is a missed one.
pub const MISSED_AFTER_SECS: i64 = 26 * 3600;
/// The restore drill is due this often.
pub const DRILL_EVERY_SECS: i64 = 90 * 24 * 3600;
/// After a failed run, the next try is this much later (until the next day's slot).
const RETRY_AFTER_SECS: i64 = 3600;
/// A failure that goes on is alerted about once in this long.
const FAILURE_ALERT_EVERY_SECS: i64 = 23 * 3600;
const PREFIX: &str = "askbar-backup-";
const SUFFIX: &str = ".tar";
const PARTIAL: &str = ".partial";
const PROBE: &str = ".askbar-write-test";

/// How long a call on the destination may take before it is given up on.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// One small call: a `stat`, a listing, a rename, a delete.
    pub call: Duration,
    /// Writing the whole backup file.
    pub write: Duration,
}

impl Default for Timing {
    fn default() -> Timing {
        Timing { call: Duration::from_secs(10), write: Duration::from_secs(30 * 60) }
    }
}

/// What the admin holds for scheduled backups: where, how long a call may take, and what is stuck.
pub struct Backups {
    dir: Mutex<PathBuf>,
    timing: Mutex<Timing>,
    /// Calls on the destination that were given up on and have not come back: at most two are ever held.
    stuck: Arc<AtomicUsize>,
    running: AtomicBool,
}

impl Default for Backups {
    fn default() -> Backups {
        let dir = std::env::var("ASKBAR_BACKUP_DIR").ok().filter(|d| !d.trim().is_empty()).unwrap_or_else(|| "/backup".into());
        Backups {
            dir: Mutex::new(PathBuf::from(dir)),
            timing: Mutex::new(Timing::default()),
            stuck: Arc::default(),
            running: AtomicBool::new(false),
        }
    }
}

impl Backups {
    /// Tests point the destination somewhere else and shorten the time limits.
    pub fn set(&self, dir: PathBuf, timing: Timing) {
        *self.dir.lock().unwrap_or_else(|e| e.into_inner()) = dir;
        *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = timing;
    }

    fn dir(&self) -> PathBuf {
        self.dir.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn timing(&self) -> Timing {
        *self.timing.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Runs `f` on a blocking thread and waits at most `limit`. A call that does not come back is given up on (its thread is left to finish
    /// or hang, counted as stuck), and while two are stuck no new call is started.
    #[doc(hidden)]
    pub async fn boxed<T: Send + 'static>(
        &self,
        limit: Duration,
        f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
    ) -> Result<T, &'static str> {
        const NOT_ANSWERING: &str = "The backup destination is not answering.";
        if self.stuck.load(Ordering::SeqCst) >= 2 {
            return Err(NOT_ANSWERING);
        }
        let (stuck, gave_up) = (self.stuck.clone(), Arc::new(AtomicBool::new(false)));
        let flag = gave_up.clone();
        let task = tokio::task::spawn_blocking(move || {
            let out = f();
            // Back after the caller gave up: the thread is free again.
            if flag.load(Ordering::SeqCst) {
                stuck.fetch_sub(1, Ordering::SeqCst);
            }
            out
        });
        match tokio::time::timeout(limit, task).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(_))) => Err("The backup destination refused the call."),
            Ok(Err(_)) => Err("The backup destination could not be used."),
            Err(_) => {
                // Mark it before the count, so a call that returns in between is not counted twice.
                self.stuck.fetch_add(1, Ordering::SeqCst);
                gave_up.store(true, Ordering::SeqCst);
                Err(NOT_ANSWERING)
            }
        }
    }
}

/// The settings and what the last runs left, as the database keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    pub keep_files: i64,
    pub keep_days: i64,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_success_name: Option<String>,
    pub last_failure_at: Option<i64>,
    pub last_failure_reason: Option<String>,
    pub failure_alerted_at: Option<i64>,
    pub missed_alerted_at: Option<i64>,
    pub drill_done_at: Option<i64>,
}

pub fn row(conn: &Connection) -> rusqlite::Result<Row> {
    conn.query_row(
        "SELECT keep_files, keep_days, last_attempt_at, last_success_at, last_success_name, last_failure_at, last_failure_reason,
                failure_alerted_at, missed_alerted_at, drill_done_at FROM backup_schedule WHERE id = 1",
        [],
        |r| {
            Ok(Row {
                keep_files: r.get(0)?,
                keep_days: r.get(1)?,
                last_attempt_at: r.get(2)?,
                last_success_at: r.get(3)?,
                last_success_name: r.get(4)?,
                last_failure_at: r.get(5)?,
                last_failure_reason: r.get(6)?,
                failure_alerted_at: r.get(7)?,
                missed_alerted_at: r.get(8)?,
                drill_done_at: r.get(9)?,
            })
        },
    )
}

/// The most recent 03:30 UTC at or before `now`.
pub fn slot_at_or_before(now: i64) -> i64 {
    (now - RUN_AT_SECS).div_euclid(86_400) * 86_400 + RUN_AT_SECS
}

/// The next 03:30 UTC after `now`.
pub fn next_slot_after(now: i64) -> i64 {
    slot_at_or_before(now) + 86_400
}

/// The name of the file a backup made at `at` is kept as.
fn file_name(at: i64) -> String {
    format!("{PREFIX}{at}{SUFFIX}")
}

/// When a file name says it was made, if it is one of ours (`askbar-backup-<unix seconds>.tar`).
fn made_at(name: &str) -> Option<i64> {
    name.strip_prefix(PREFIX)?.strip_suffix(SUFFIX)?.parse::<i64>().ok().filter(|t| *t > 0)
}

/// One backup on the destination.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct BackupFile {
    pub name: String,
    pub bytes: u64,
    pub made_at: i64,
}

/// The backups in `dir`, newest first.
fn list_files(dir: &Path) -> std::io::Result<Vec<BackupFile>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(made_at) = made_at(&name) {
            if e.file_type()?.is_file() {
                out.push(BackupFile { name, bytes: e.metadata()?.len(), made_at });
            }
        }
    }
    out.sort_by(|a, b| b.made_at.cmp(&a.made_at).then(b.name.cmp(&a.name)));
    Ok(out)
}

/// The files that retention would delete: not among the newest `keep_files`, or older than `keep_days`, and never the newest.
pub fn doomed(files: &[BackupFile], keep_files: i64, keep_days: i64, now: i64) -> Vec<String> {
    files
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(i, f)| *i as i64 >= keep_files || now - f.made_at > keep_days * 86_400)
        .map(|(_, f)| f.name.clone())
        .collect()
}

/// What the Backup page shows about the destination.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Destination {
    pub path: String,
    pub writable: bool,
    /// The destination is on the same disk as the admin's own data: a backup there does not survive the disk.
    pub same_disk: Option<bool>,
    /// A sentence for a person when something is wrong with it.
    pub problem: Option<String>,
    pub files: Vec<BackupFile>,
}

async fn destination(state: &AdminState) -> Destination {
    let b = &state.backups;
    let (dir, t) = (b.dir(), b.timing());
    let path = dir.display().to_string();
    let own = crate::backup_bundle::work_parent(state).await;
    let (d, o) = (dir.clone(), own);
    let probed = b
        .boxed(t.call, move || {
            use std::os::unix::fs::MetadataExt;
            let there = std::fs::metadata(&d)?;
            if !there.is_dir() {
                return Err(std::io::Error::other("not a directory"));
            }
            let same = std::fs::metadata(&o).map(|m| m.dev() == there.dev()).ok();
            // Writable means a file can be made and removed (a read-only mount, a full share and a wrong owner all say no here).
            let probe = d.join(PROBE);
            let wrote = std::fs::File::create(&probe).and_then(|mut f| f.write_all(b"ok")).and_then(|_| std::fs::remove_file(&probe));
            Ok((same, wrote.is_ok(), list_files(&d).unwrap_or_default()))
        })
        .await;
    match probed {
        Ok((same_disk, writable, files)) => Destination {
            path,
            writable,
            same_disk,
            problem: if !writable {
                Some("Backups cannot be written to it (read-only, full, or not owned by the admin).".into())
            } else {
                None
            },
            files,
        },
        Err(why) => Destination { path, writable: false, same_disk: None, problem: Some(why.into()), files: vec![] },
    }
}

/// What a run made.
#[derive(Debug)]
pub struct Made {
    pub name: String,
    pub bytes: u64,
    pub removed: usize,
}

/// Writes a bundle to the destination, all or nothing, then applies retention. `Err` is a sentence for a person.
pub async fn run_once(state: &AdminState) -> Result<Made, String> {
    let b = &state.backups;
    if b.running.swap(true, Ordering::SeqCst) {
        return Err("A backup is already running.".into());
    }
    struct Running<'a>(&'a AtomicBool);
    impl Drop for Running<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _running = Running(&b.running);
    let (dir, t) = (b.dir(), b.timing());
    // Before the work of making the bundle: a destination that is not there is found in seconds, not after the parts are staged.
    let d = dir.clone();
    b.boxed(t.call, move || {
        let m = std::fs::metadata(&d)?;
        if m.is_dir() {
            Ok(())
        } else {
            Err(std::io::Error::other("not a directory"))
        }
    })
    .await
    .map_err(String::from)?;

    let built = crate::backup_bundle::build(state).await.map_err(|e| e.problem.detail)?;
    let (name, partial) = (file_name(built.created_at()), format!("{}{PARTIAL}", file_name(built.created_at())));
    let (d, p) = (dir.clone(), partial.clone());
    let bytes = b
        .boxed(t.write, move || {
            // Leftovers of a run that crashed or was given up on.
            if let Ok(rd) = std::fs::read_dir(&d) {
                for e in rd.flatten() {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n.starts_with(PREFIX) && n.ends_with(PARTIAL) {
                        let _ = std::fs::remove_file(e.path());
                    }
                }
            }
            let mut f = std::io::BufWriter::new(std::fs::File::create(d.join(&p))?);
            built.write(&mut f)?;
            let f = f.into_inner().map_err(|e| e.into_error())?;
            f.sync_all()?;
            Ok(f.metadata()?.len())
        })
        .await;
    let bytes = match bytes {
        Ok(n) => n,
        Err(why) => {
            // Take the partial file away if the destination will let us (best effort, and time-boxed too).
            let (d, p) = (dir.clone(), partial.clone());
            let _ = b.boxed(t.call, move || std::fs::remove_file(d.join(p))).await;
            return Err(why.to_string());
        }
    };
    let (d, from, to) = (dir.clone(), partial.clone(), name.clone());
    b.boxed(t.call, move || std::fs::rename(d.join(from), d.join(to))).await.map_err(String::from)?;

    // Retention, once there is a new backup in place. A failure here is not a failed backup.
    let (keep_files, keep_days) = {
        let db = state.db.lock().await;
        row(db.conn()).map(|r| (r.keep_files, r.keep_days)).unwrap_or((5, 30))
    };
    let (d, now) = (dir, crate::auth::now());
    let removed = b
        .boxed(t.call, move || {
            let files = list_files(&d)?;
            let doomed = doomed(&files, keep_files, keep_days, now);
            let mut n = 0;
            for f in &doomed {
                if std::fs::remove_file(d.join(f)).is_ok() {
                    n += 1;
                }
            }
            Ok(n)
        })
        .await
        .unwrap_or(0);
    Ok(Made { name, bytes, removed })
}

/// Records how a run ended, and alerts for a failure (once a day for one that goes on).
async fn record_outcome(state: &AdminState, now: i64, result: &Result<Made, String>) {
    let db = state.db.lock().await;
    let conn = db.conn();
    let saved = match result {
        Ok(m) => conn.execute(
            "UPDATE backup_schedule SET last_attempt_at = ?1, last_success_at = ?1, last_success_name = ?2, failure_alerted_at = NULL, missed_alerted_at = NULL WHERE id = 1",
            rusqlite::params![now, m.name],
        ),
        Err(why) => {
            let alerted = row(conn).ok().and_then(|r| r.failure_alerted_at);
            let alert = alerted.is_none_or(|t| now - t >= FAILURE_ALERT_EVERY_SECS);
            if alert {
                state.notifier.notify(Kind::BackupFailed, Kind::BackupFailed.severity(), Fields::default());
            }
            conn.execute(
                "UPDATE backup_schedule SET last_attempt_at = ?1, last_failure_at = ?1, last_failure_reason = ?2,
                        failure_alerted_at = CASE WHEN ?3 THEN ?1 ELSE failure_alerted_at END WHERE id = 1",
                rusqlite::params![now, why, alert],
            )
        }
    };
    if let Err(e) = saved {
        tracing::warn!("could not record how a backup ended: {e}");
    }
}

/// One backup now, as the schedule makes it (or a person asks for it): run, record how it ended, alert for a failure.
pub async fn backup_now(state: &AdminState) -> Result<Made, String> {
    let now = crate::auth::now();
    let result = run_once(state).await;
    record_outcome(state, now, &result).await;
    result
}

/// Whether a backup is due: the day's slot has come and nothing has been tried since it (a failed one is tried again an hour later).
pub fn due(r: &Row, now: i64) -> bool {
    let slot = slot_at_or_before(now);
    match r.last_attempt_at {
        None => true,
        Some(at) if at < slot => true,
        Some(at) => r.last_success_at != Some(at) && now - at >= RETRY_AFTER_SECS && r.last_failure_at == Some(at) && at >= slot,
    }
}

/// When the next backup will be tried: now if one is due, an hour after a failed one, else the next slot.
pub fn next_run(r: &Row, now: i64) -> i64 {
    if due(r, now) {
        return now;
    }
    match (r.last_failure_at, r.last_attempt_at) {
        (Some(f), Some(a)) if f == a && a >= slot_at_or_before(now) => (a + RETRY_AFTER_SECS).min(next_slot_after(now)),
        _ => next_slot_after(now),
    }
}

/// When the last backup is counted from for "missed": the last success, or else when the backup key was made.
async fn reference_time(state: &AdminState, r: &Row) -> Option<i64> {
    if let Some(t) = r.last_success_at {
        return Some(t);
    }
    let db = state.db.lock().await;
    crate::db::active_backup_key(db.conn()).ok().flatten().map(|k| k.created_at)
}

/// One look at the schedule at `now`: make a backup if one is due, and say so if none has succeeded for 26 hours. Nothing without a key.
pub async fn tick(state: &AdminState, now: i64) {
    let (has_key, r) = {
        let db = state.db.lock().await;
        (crate::db::active_backup_key(db.conn()).ok().flatten().is_some(), row(db.conn()))
    };
    let (true, Ok(r)) = (has_key, r) else { return };
    if due(&r, now) {
        // A scheduled backup is an action of the admin's own: recorded before it is made, and not made if the record cannot be (ADR 0010).
        // Then nothing has been tried, so it is looked at again at the next tick; a day without a success is a miss, below.
        match state.actions.get() {
            Some(log) => {
                let recorded = log
                    .around(
                        "backup",
                        "scheduled",
                        json!({ "by": "schedule" }),
                        run_scheduled(state, now),
                        |res: &Result<String, String>| match res {
                            Ok(name) => json!({ "file": name }),
                            Err(_) => json!({}),
                        },
                    )
                    .await;
                if let Err(why) = recorded {
                    tracing::warn!("a scheduled backup was not made: {}", why.sentence());
                }
            }
            None => {
                let _ = run_scheduled(state, now).await;
            }
        }
    }
    // Missed: no success for 26 hours, once for each miss.
    let r = {
        let db = state.db.lock().await;
        row(db.conn()).unwrap_or(r)
    };
    if let Some(reference) = reference_time(state, &r).await {
        let missed = now - reference >= MISSED_AFTER_SECS;
        let said = r.missed_alerted_at.is_some_and(|t| t >= reference);
        if missed && !said {
            let minutes = u32::try_from((now - reference) / 60).unwrap_or(u32::MAX);
            state.notifier.notify(
                Kind::BackupMissed,
                Kind::BackupMissed.severity(),
                Fields { minutes: Some(minutes), ..Fields::default() },
            );
            let db = state.db.lock().await;
            let _ = db.conn().execute("UPDATE backup_schedule SET missed_alerted_at = ?1 WHERE id = 1", [now]);
        }
    }
}

async fn run_scheduled(state: &AdminState, now: i64) -> Result<String, String> {
    let result = run_once(state).await;
    record_outcome(state, now, &result).await;
    result.map(|m| m.name)
}

/// The scheduler: looks at the schedule every half minute, for as long as the process runs. It never holds anything another request needs.
pub async fn run(state: Arc<AdminState>) {
    loop {
        tick(&state, crate::auth::now()).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
}

// ------------------------------------------------------------------ the routes

fn internal() -> Failure {
    err(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the admin side.")
}

async fn view(state: &AdminState) -> Result<Value, Failure> {
    let (r, has_key, key_made) = {
        let db = state.db.lock().await;
        let r = row(db.conn()).map_err(|_| internal())?;
        let key = crate::db::active_backup_key(db.conn()).ok().flatten();
        (r, key.is_some(), key.map(|k| k.created_at))
    };
    let now = crate::auth::now();
    let dest = destination(state).await;
    let drill_from = r.drill_done_at.or(key_made);
    let missed = has_key && reference_time(state, &r).await.is_some_and(|t| now - t >= MISSED_AFTER_SECS);
    Ok(json!({
        "enabled": has_key,
        "destination": dest,
        "keep_files": r.keep_files,
        "keep_days": r.keep_days,
        "next_run_at": has_key.then(|| next_run(&r, now)),
        "last_attempt_at": r.last_attempt_at,
        "last_success_at": r.last_success_at,
        "last_success_name": r.last_success_name,
        "last_failure_at": r.last_failure_at,
        "last_failure_reason": r.last_failure_reason,
        "missed": missed,
        "drill_done_at": r.drill_done_at,
        "drill_due": drill_from.is_some_and(|t| now - t >= DRILL_EVERY_SECS),
        "drill_every_days": DRILL_EVERY_SECS / 86_400,
    }))
}

/// `GET /api/admin/v1/backup/schedule`: the destination, the limits, the last run and the reminders.
pub async fn get_schedule(State(state): St) -> Result<Json<Value>, Failure> {
    view(&state).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub keep_files: i64,
    pub keep_days: i64,
}

/// `PUT /api/admin/v1/backup/schedule`: how many backups and how long to keep them. Needs a fresh sign-in: lowering them deletes backups
/// at the next run. At least one file and one day.
pub async fn put_schedule(
    State(state): St,
    headers: HeaderMap,
    notes: Option<Extension<crate::actions::ActionNotes>>,
    ApiJson(l): ApiJson<Limits>,
) -> Result<Json<Value>, Failure> {
    crate::web::require_fresh_signin(&state, &headers)?;
    if !(1..=365).contains(&l.keep_files) {
        return Err(err(StatusCode::BAD_REQUEST, "Keep between 1 and 365 backups."));
    }
    if !(1..=3650).contains(&l.keep_days) {
        return Err(err(StatusCode::BAD_REQUEST, "Keep backups between 1 and 3650 days."));
    }
    {
        let db = state.db.lock().await;
        db.conn()
            .execute("UPDATE backup_schedule SET keep_files = ?1, keep_days = ?2 WHERE id = 1", [l.keep_files, l.keep_days])
            .map_err(|_| internal())?;
    }
    if let Some(Extension(n)) = &notes {
        n.add("keep_files", json!(l.keep_files));
        n.add("keep_days", json!(l.keep_days));
    }
    view(&state).await.map(Json)
}

/// `POST /api/admin/v1/backup/run`: a backup to the destination now, as the schedule makes one (the browser download is `GET /backup`).
pub async fn run_now(State(state): St, notes: Option<Extension<crate::actions::ActionNotes>>) -> Result<Json<Value>, Failure> {
    if crate::db::active_backup_key(state.db.lock().await.conn()).ok().flatten().is_none() {
        return Err(ApiError::new(
            ErrorCode::BackupKeyMissing,
            "Set a backup passphrase first (the Backup page): nothing is backed up without one.",
        ));
    }
    match backup_now(&state).await {
        Ok(m) => {
            if let Some(Extension(n)) = &notes {
                n.add("file", json!(m.name));
            }
            Ok(Json(json!({ "file": m.name, "bytes": m.bytes, "removed": m.removed })))
        }
        Err(why) => Err(err(StatusCode::BAD_GATEWAY, &format!("{why} Nothing was backed up."))),
    }
}

/// `POST /api/admin/v1/backup/drill`: a restore drill was done; the 90 days start again.
pub async fn drill_done(State(state): St) -> Result<Json<Value>, Failure> {
    {
        let db = state.db.lock().await;
        db.conn().execute("UPDATE backup_schedule SET drill_done_at = ?1 WHERE id = 1", [crate::auth::now()]).map_err(|_| internal())?;
    }
    view(&state).await.map(Json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slot_is_half_past_three_utc_and_the_next_is_a_day_later() {
        // 2026-11-12 00:00:00 UTC = 1_794_441_600? Use a round day: 86_400 * 20_000.
        let day = 86_400 * 20_000;
        assert_eq!(slot_at_or_before(day + RUN_AT_SECS), day + RUN_AT_SECS);
        assert_eq!(slot_at_or_before(day + RUN_AT_SECS - 1), day - 86_400 + RUN_AT_SECS);
        assert_eq!(slot_at_or_before(day + 86_399), day + RUN_AT_SECS);
        assert_eq!(next_slot_after(day + 100), day + RUN_AT_SECS);
        assert_eq!(next_slot_after(day + RUN_AT_SECS), day + 86_400 + RUN_AT_SECS);
    }

    #[test]
    fn only_our_files_are_backups_and_the_newest_is_never_deleted() {
        assert_eq!(made_at("askbar-backup-1700000000.tar"), Some(1_700_000_000));
        for no in [
            "askbar-backup-1700000000.tar.partial",
            "askbar-backup-x.tar",
            "backup-1.tar",
            "askbar-backup-0.tar",
            ".askbar-write-test",
            "askbar-backup-1.tar.age",
        ] {
            assert_eq!(made_at(no), None, "{no}");
        }
        let day = 86_400;
        let files: Vec<BackupFile> =
            (0..8).map(|i| BackupFile { name: file_name(1_000_000 - i * day), bytes: 1, made_at: 1_000_000 - i * day }).collect();
        // Five kept by count.
        assert_eq!(
            doomed(&files, 5, 3650, 1_000_000),
            [file_name(1_000_000 - 5 * day), file_name(1_000_000 - 6 * day), file_name(1_000_000 - 7 * day)]
        );
        // Older than 3 days by age, though only 8 files.
        assert_eq!(doomed(&files, 365, 3, 1_000_000).len(), 4);
        // Whatever the limits, the newest stays, even when it is itself too old.
        assert!(!doomed(&files, 1, 1, 1_000_000 + 400 * day).contains(&files[0].name));
        assert_eq!(doomed(&files, 1, 1, 1_000_000 + 400 * day).len(), 7);
        assert!(doomed(&files[..1], 1, 1, 1_000_000 + 400 * day).is_empty());
    }

    #[test]
    fn a_backup_is_due_at_the_slot_and_a_failed_one_is_tried_again_an_hour_later() {
        let day = 86_400 * 20_000;
        let slot = day + RUN_AT_SECS;
        let none = Row::default();
        assert!(due(&none, slot), "never tried");
        let ok = Row { last_attempt_at: Some(slot + 5), last_success_at: Some(slot + 5), ..Row::default() };
        assert!(!due(&ok, slot + 3600) && !due(&ok, slot + 86_399), "done for the day");
        assert!(due(&ok, slot + 86_400), "the next slot");
        let failed = Row { last_attempt_at: Some(slot + 5), last_failure_at: Some(slot + 5), ..Row::default() };
        assert!(!due(&failed, slot + 600), "not at once");
        assert!(due(&failed, slot + 5 + RETRY_AFTER_SECS), "an hour later");
        assert!(due(&failed, slot + 86_400), "or at the next slot");
    }
}
