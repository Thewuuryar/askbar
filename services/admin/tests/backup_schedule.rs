#![cfg(unix)]
//! Scheduled backups (ADR 0013, D27 to D29, plan step 6.4): written all or nothing to the backup volume, kept by count and age with the
//! newest always kept, alerted about when they fail or are missed, and a destination that does not answer does not keep the admin down.

mod common;
use askbar_admin::backup_schedule::{self, Timing};
use askbar_service::backup;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

const PASS: &str = "a backup passphrase of its own 1";

/// A rig with a backup key and a destination of its own, with short time limits.
async fn rig() -> (common::T, std::path::PathBuf) {
    let t = common::start(true).await;
    let b = json!({ "passphrase": PASS, "confirm": PASS });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/backup-key", b).await.0, 200);
    let dest = t.dir.join("backups");
    std::fs::create_dir_all(&dest).unwrap();
    t.admin.backups.set(dest.clone(), Timing { call: Duration::from_millis(300), write: Duration::from_secs(30) });
    (t, dest)
}

fn files(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> =
        std::fs::read_dir(dir).unwrap().filter_map(Result::ok).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

fn notes(t: &common::T, kind: &str) -> usize {
    t.admin.notifier.notes().iter().filter(|n| n.kind == kind).count()
}

#[tokio::test]
async fn a_backup_is_written_all_or_nothing_opens_with_the_key_and_shows_in_the_status() {
    let (t, dest) = rig().await;
    let (status, made) = t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await;
    assert_eq!(status, 200, "{made}");
    let name = made["file"].as_str().unwrap().to_string();
    assert!(name.starts_with("askbar-backup-") && name.ends_with(".tar"), "{name}");
    // One file, no partial left, and it is a bundle the passphrase opens.
    assert_eq!(files(&dest), std::slice::from_ref(&name));
    let bytes = std::fs::read(dest.join(&name)).unwrap();
    assert_eq!(made["bytes"], bytes.len() as u64);
    let entries = askbar_service::tar::read(&bytes).unwrap();
    assert_eq!(entries[0].0, "manifest.json");
    let identity = backup::unlock(&entries[1].1, PASS).unwrap();
    assert!(backup::decrypt(&identity, &entries.iter().find(|(n, _)| n == "admin.age").unwrap().1).is_ok());
    // The status: writable, a file, the last success, the next run at 03:30 UTC, no reminders yet.
    let (_, v) = t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await;
    assert_eq!(v["enabled"], true);
    assert_eq!((v["destination"]["writable"].clone(), v["destination"]["files"][0]["name"].as_str()), (json!(true), Some(name.as_str())));
    assert_eq!(v["destination"]["same_disk"], true, "a test directory is on the same disk as the admin's work files");
    assert_eq!((v["keep_files"].clone(), v["keep_days"].clone()), (json!(5), json!(30)));
    assert_eq!(v["last_success_name"], name.as_str());
    let next = v["next_run_at"].as_i64().unwrap();
    assert_eq!(next % 86_400, backup_schedule::RUN_AT_SECS, "03:30 UTC");
    assert!(next > askbar_admin::auth::now());
    assert_eq!((v["missed"].clone(), v["drill_due"].clone()), (json!(false), json!(false)));
    // The run is recorded like any backup.
    let rows = t.rig.rows("SELECT outcome, detail_json FROM admin_actions WHERE category = 'backup' AND action = 'run'");
    assert!(format!("{rows:?}").contains(&name), "{rows:?}");
    // A partial file is never counted as a backup, and the next run removes it.
    std::fs::write(dest.join("askbar-backup-1.tar.partial"), b"half").unwrap();
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await.0, 200);
    assert!(files(&dest).iter().all(|f| !f.ends_with(".partial")), "{:?}", files(&dest));
}

#[tokio::test]
async fn only_the_newest_within_the_limits_are_kept_and_the_newest_is_never_deleted() {
    let (t, dest) = rig().await;
    let now = askbar_admin::auth::now();
    // Old backups of ours, and files that are not.
    for age_days in [1, 2, 3, 10, 40, 60] {
        std::fs::write(dest.join(format!("askbar-backup-{}.tar", now - age_days * 86_400)), b"old").unwrap();
    }
    std::fs::write(dest.join("notes.txt"), b"mine").unwrap();
    // Two files and 30 days: the new one and one old one stay.
    let (status, _) = t.call(M::PUT, "/api/admin/v1/backup/schedule", json!({ "keep_files": 2, "keep_days": 30 })).await;
    assert_eq!(status, 200);
    let (_, made) = t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await;
    assert_eq!(made["removed"], 5, "the five older than the newest two, which are not among them: {made}");
    let left = files(&dest);
    assert_eq!(left.len(), 3, "{left:?}");
    assert!(left.contains(&"notes.txt".to_string()), "a file that is not a backup is never touched");
    assert!(left.contains(&made["file"].as_str().unwrap().to_string()));
    assert!(left.contains(&format!("askbar-backup-{}.tar", now - 86_400)));
    // Limits at their lowest still keep the newest.
    assert_eq!(t.call(M::PUT, "/api/admin/v1/backup/schedule", json!({ "keep_files": 1, "keep_days": 1 })).await.0, 200);
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await.0, 200);
    let backups: Vec<String> = files(&dest).into_iter().filter(|f| f.starts_with("askbar-backup-")).collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
}

#[tokio::test]
async fn the_limits_are_bounded_and_need_a_fresh_sign_in_from_a_person() {
    let (t, _dest) = rig().await;
    for bad in [
        json!({ "keep_files": 0, "keep_days": 30 }),
        json!({ "keep_files": 5, "keep_days": 0 }),
        json!({ "keep_files": 400, "keep_days": 30 }),
        json!({ "keep_files": 5, "keep_days": 4000 }),
    ] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/backup/schedule", bad).await.0, 400);
    }
    assert_eq!(t.call(M::PUT, "/api/admin/v1/backup/schedule", json!({ "keep_files": 5 })).await.0, 400, "both are needed");
    let key = {
        let db = t.admin.db.lock().await;
        askbar_admin::api_key::create(db.conn(), "ci", askbar_admin::auth::now()).unwrap().1
    };
    let r = t
        .http
        .put(format!("{}/api/admin/v1/backup/schedule", t.adm))
        .bearer_auth(&key)
        .json(&json!({ "keep_files": 5, "keep_days": 30 }))
        .send()
        .await
        .unwrap();
    assert!(matches!(r.status().as_u16(), 401 | 403), "an API key may not lower what is kept: {}", r.status());
    t.admin.auth.backdate_session(t.cookie.split_once('=').unwrap().1, 11 * 60);
    assert_eq!(
        t.call(M::PUT, "/api/admin/v1/backup/schedule", json!({ "keep_files": 5, "keep_days": 30 })).await.0,
        403,
        "a stale sign-in"
    );
}

#[tokio::test]
async fn a_failed_backup_is_alerted_once_a_day_and_tried_again_an_hour_later() {
    let (t, dest) = rig().await;
    std::fs::remove_dir_all(&dest).unwrap(); // the destination is gone
    let slot = backup_schedule::slot_at_or_before(askbar_admin::auth::now());
    backup_schedule::tick(&t.admin, slot + 10).await;
    assert_eq!(notes(&t, "backup_failed"), 1);
    let r = { backup_schedule::row(t.admin.db.lock().await.conn()).unwrap() };
    assert_eq!((r.last_success_at, r.last_failure_reason.is_some()), (None, true));
    // Not again within the hour, and an hour later it is tried again; a failure that goes on is not alerted about again that day.
    backup_schedule::tick(&t.admin, slot + 600).await;
    assert_eq!(notes(&t, "backup_failed"), 1);
    let (before, _) = (backup_schedule::row(t.admin.db.lock().await.conn()).unwrap().last_attempt_at, ());
    backup_schedule::tick(&t.admin, slot + 10 + 3600).await;
    assert_ne!(backup_schedule::row(t.admin.db.lock().await.conn()).unwrap().last_attempt_at, before, "tried again");
    assert_eq!(notes(&t, "backup_failed"), 1, "once a day");
    // The destination comes back: the next try works, and what it left says so.
    std::fs::create_dir_all(&dest).unwrap();
    backup_schedule::tick(&t.admin, slot + 10 + 7200).await;
    let r = backup_schedule::row(t.admin.db.lock().await.conn()).unwrap();
    assert!(r.last_success_at.is_some() && r.failure_alerted_at.is_none());
    assert_eq!(files(&dest).len(), 1);
}

#[tokio::test]
async fn a_backup_missed_for_26_hours_is_alerted_once_each_time_and_nothing_is_missed_without_a_key() {
    let t = common::start(true).await;
    // No key: nothing is made and nothing is missed, however long it has been.
    backup_schedule::tick(&t.admin, askbar_admin::auth::now() + 10 * 86_400).await;
    assert_eq!((notes(&t, "backup_missed"), notes(&t, "backup_failed")), (0, 0));
    let (t, dest) = rig().await;
    let now = askbar_admin::auth::now();
    // A success 27 hours ago (and today's slot already tried, so it is not made now): missed once, not on every look.
    t.admin
        .db
        .lock()
        .await
        .conn()
        .execute("UPDATE backup_schedule SET last_success_at = ?1, last_attempt_at = ?2 WHERE id = 1", [now - 27 * 3600, now + 3600])
        .unwrap();
    backup_schedule::tick(&t.admin, now + 1).await;
    backup_schedule::tick(&t.admin, now + 60).await;
    assert_eq!(notes(&t, "backup_missed"), 1);
    let (_, v) = t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await;
    assert_eq!(v["missed"], true);
    // A backup that works clears it, and a later miss is alerted again.
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await.0, 200);
    assert_eq!(t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await.1["missed"], false);
    // 27 hours on, the day's backup is tried and the destination is gone: it fails, and the miss is alerted about again.
    std::fs::remove_dir_all(&dest).unwrap();
    let later = askbar_admin::auth::now() + 27 * 3600;
    backup_schedule::tick(&t.admin, later).await;
    assert_eq!(notes(&t, "backup_missed"), 2);
}

#[tokio::test]
async fn the_restore_drill_is_due_every_90_days_and_drill_done_starts_the_count_again() {
    let (t, _dest) = rig().await;
    let (_, v) = t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await;
    assert_eq!((v["drill_due"].clone(), v["drill_every_days"].clone()), (json!(false), json!(90)));
    let ago = askbar_admin::auth::now() - 91 * 86_400;
    t.admin.db.lock().await.conn().execute("UPDATE backup_schedule SET drill_done_at = ?1 WHERE id = 1", [ago]).unwrap();
    assert_eq!(t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await.1["drill_due"], true);
    let (status, v) = t.call(M::POST, "/api/admin/v1/backup/drill", json!(null)).await;
    assert_eq!((status, v["drill_due"].clone()), (200, json!(false)), "{v}");
    assert!(v["drill_done_at"].as_i64().unwrap() > ago);
}

/// A destination whose calls never come back, as a share that has gone away does: a named pipe where the write test is made blocks the
/// opener until something reads it.
#[tokio::test]
async fn a_destination_that_does_not_answer_does_not_keep_the_admin_down() {
    let (t, dest) = rig().await;
    let fifo = dest.join(".askbar-write-test");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success(), "mkfifo is needed");
    // The status gives up on the destination within the limit, says so, and the admin goes on answering.
    let started = Instant::now();
    let (status, v) = t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await;
    assert_eq!(status, 200);
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(v["destination"]["writable"], false);
    assert!(v["destination"]["problem"].as_str().unwrap().contains("not answering"), "{v}");
    let started = Instant::now();
    assert_eq!(t.call(M::GET, "/api/admin/v1/status", json!(null)).await.0, 200, "other pages are not held up");
    assert!(started.elapsed() < Duration::from_secs(2));
    // A second look also gives up, and now two calls are stuck: nothing more is started on the destination, so a backup fails at once with
    // a sentence and no thread piles up.
    assert_eq!(t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await.0, 200);
    let started = Instant::now();
    let (status, body) = t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await;
    assert_eq!(status, 502, "{body}");
    assert!(body["detail"].as_str().unwrap().contains("not answering"), "{body}");
    assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    assert_eq!(notes(&t, "backup_failed"), 1);
    // The share comes back (the reader opens the pipe): the stuck calls finish, and the next backup works.
    let _reader = std::fs::File::open(&fifo).unwrap();
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if t.admin.backups.boxed(Duration::from_secs(1), || Ok(())).await.is_ok() {
            break;
        }
    }
    let _ = std::fs::remove_file(&fifo); // the write test removes it once it gets through
    assert_eq!(t.call(M::POST, "/api/admin/v1/backup/run", Value::Null).await.0, 200);
}

#[tokio::test]
async fn a_backup_needs_a_key_and_the_routes_need_a_signed_in_person() {
    let t = common::start(true).await;
    t.admin.backups.set(t.dir.join("backups"), Timing::default());
    let (status, body) = t.call(M::POST, "/api/admin/v1/backup/run", json!(null)).await;
    assert_eq!((status, body["error"].as_str()), (503, Some("backup_key_missing")), "{body}");
    assert_eq!(t.call(M::GET, "/api/admin/v1/backup/schedule", json!(null)).await.1["enabled"], false);
    for (m, p) in [
        (M::GET, "/api/admin/v1/backup/schedule"),
        (M::POST, "/api/admin/v1/backup/run"),
        (M::POST, "/api/admin/v1/backup/drill"),
        (M::PUT, "/api/admin/v1/backup/schedule"),
    ] {
        let r = t.http.request(m.clone(), format!("{}{p}", t.adm)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{m} {p} signed out");
    }
}
