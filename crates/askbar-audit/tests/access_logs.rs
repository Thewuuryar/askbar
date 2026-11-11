//! Caddy's access lines in the store (plan step 2.9): kept to a short list of fields, chained, never changed, pruned with
//! the rest, best effort when the queue is busy, and a version 4 file with a chain upgrades with its chain whole.

use askbar_audit::chain;
use askbar_audit::rusqlite::Connection;
use askbar_audit::{AccessLogWrite, AuditDb, Record, Recorder, SpanStart};
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::temp_dir().join(format!("askbar-audit-access-{}", ulid::Ulid::new()))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn line(at: i64, path: &str) -> AccessLogWrite {
    AccessLogWrite {
        at,
        method: "GET".into(),
        host: "askbar.example".into(),
        path: path.into(),
        status: 200,
        bytes: 512,
        duration_ms: 7,
        remote_ip: "203.0.113.9".into(),
        proto: "HTTP/2.0".into(),
        user_agent: Some("curl/8".into()),
    }
}

#[tokio::test]
async fn an_access_line_is_stored_chained_and_found_again_when_edited() {
    let d = dir();
    let path = d.join("audit.db");
    let rec = Recorder::spawn(AuditDb::open(&path).unwrap(), 100);
    assert!(rec.access_log(line(now(), "/api/client/v1/health")));
    assert!(rec.access_log(line(now(), "/admin/")));
    rec.flush().await;
    let c = Connection::open(&path).unwrap();
    let rows = askbar_audit::list_access_logs(&c, None, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["path"], "/admin/", "newest first");
    assert_eq!((rows[1]["status"].clone(), rows[1]["remote_ip"].clone()), (serde_json::json!(200), serde_json::json!("203.0.113.9")));
    // Paging: `before` is the id of the last row seen.
    let id = rows[0]["id"].as_i64().unwrap();
    assert_eq!(askbar_audit::list_access_logs(&c, Some(id), Some(10)).unwrap().len(), 1);
    assert_eq!(askbar_audit::list_access_logs(&c, None, Some(1)).unwrap().len(), 1);

    let kinds: i64 = c.query_row("SELECT COUNT(*) FROM chain WHERE kind = 'access_log'", [], |r| r.get(0)).unwrap();
    assert_eq!(kinds, 2, "each line has an entry in the chain");
    assert!(chain::verify(&c).unwrap().ok);

    // The store does not let it be changed, and a forger who takes the triggers away is found.
    assert!(c.execute("UPDATE access_logs SET status = 500", []).is_err());
    let names: Vec<String> = c
        .prepare("SELECT name FROM sqlite_master WHERE type = 'trigger'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    for n in names {
        c.execute_batch(&format!("DROP TRIGGER {n}")).unwrap();
    }
    c.execute("UPDATE access_logs SET status = 500 WHERE id = ?1", [id]).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.kind == "access_log" && p.reason.contains("changed after")), "{report:?}");
}

#[tokio::test]
async fn retention_prunes_old_lines_and_the_prune_explains_them_and_a_young_line_cannot_be_deleted() {
    let d = dir();
    let path = d.join("audit.db");
    let rec = Recorder::spawn(AuditDb::open(&path).unwrap(), 100);
    rec.access_log(line(now() - 40 * 86400, "/old"));
    rec.access_log(line(now(), "/new"));
    rec.flush().await;
    rec.prune(30, now()).await.unwrap();
    let c = Connection::open(&path).unwrap();
    let left: Vec<String> =
        askbar_audit::list_access_logs(&c, None, None).unwrap().iter().map(|r| r["path"].as_str().unwrap().to_string()).collect();
    assert_eq!(left, ["/new"]);
    assert!(chain::verify(&c).unwrap().ok, "the removed line is explained by the prune");
    assert!(c.execute("DELETE FROM access_logs", []).is_err(), "a line younger than a week cannot be deleted");
}

#[tokio::test]
async fn access_lines_never_take_more_than_half_the_queue_so_they_cannot_crowd_out_what_fails_closed() {
    let d = dir();
    let rec = Recorder::spawn(AuditDb::open(&d.join("audit.db")).unwrap(), 10);
    rec.pause_writer(true);
    // The writer is stuck: only five lines fit, and the rest are dropped and counted.
    let queued = (0..50).filter(|i| rec.access_log(line(now(), &format!("/p{i}")))).count();
    assert!((4..=6).contains(&queued), "{queued}");
    assert_eq!(rec.access_dropped() as usize, 50 - queued);
    // A record that fails closed still gets in.
    let mut s = SpanStart::new("t", askbar_audit::Kind::Gate, "kill_switch");
    s.id = "g1".into();
    s.started_at = now();
    let pending = tokio::spawn({
        let rec = rec.clone();
        async move { rec.append(Record::Start(s)).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    rec.pause_writer(false);
    assert!(pending.await.unwrap().is_ok(), "the span was taken while the lines were waiting");
}

#[test]
fn a_version_4_file_with_its_chain_upgrades_and_the_chain_stays_whole() {
    let d = dir();
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("audit.db");
    let t = now();
    {
        // Open (migrating to the newest), then put the file back to what version 4 was: no access_logs, the old chain table.
        let db = AuditDb::open(&path).unwrap();
        let c = db.conn();
        c.execute(
            "INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at, service, boot_id) VALUES ('p1','t','turn','reply','done',?1,?1,'model-gateway','b')",
            [t],
        )
        .unwrap();
        chain::backfill_for_tests(c).unwrap();
        c.execute_batch(
            "DROP TRIGGER chain_is_never_deleted; DROP TRIGGER chain_is_never_changed; DROP TRIGGER access_logs_are_never_changed; DROP TRIGGER access_logs_keep_a_week;
             DROP TRIGGER alert_deliveries_are_never_changed; DROP TRIGGER alert_deliveries_are_never_deleted; DROP TABLE alert_deliveries;
             DROP TABLE access_logs; ALTER TABLE chain RENAME TO chain_wide;
             CREATE TABLE chain (seq INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('span_start','span_finish','payload','admin_action','prune')),
               ref TEXT NOT NULL, row_at INTEGER NOT NULL, digest TEXT NOT NULL, prev_hash TEXT NOT NULL, hash TEXT NOT NULL);
             INSERT INTO chain SELECT * FROM chain_wide; DROP TABLE chain_wide;
             CREATE TRIGGER chain_is_never_changed BEFORE UPDATE ON chain BEGIN SELECT RAISE(ABORT, 'chain entries cannot be changed'); END;
             CREATE TRIGGER chain_is_never_deleted BEFORE DELETE ON chain BEGIN SELECT RAISE(ABORT, 'chain entries cannot be deleted'); END;
             PRAGMA user_version = 4;",
        )
        .unwrap();
    }
    let before = {
        let c = Connection::open(&path).unwrap();
        chain::head(&c).unwrap()
    };
    assert!(before.seq >= 1);
    let db = AuditDb::open(&path).unwrap();
    let c = db.conn();
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, 6, "migrations 5 and 6 both ran");
    assert_eq!(chain::head(c).unwrap(), before, "every entry was copied as it was");
    assert!(chain::verify(c).unwrap().ok);
    // The new kind is accepted, and the chain still refuses to be changed.
    c.execute("INSERT INTO access_logs (at, method, host, path, status, bytes, duration_ms, remote_ip, proto) VALUES (?1,'GET','h','/',200,1,1,'1.2.3.4','HTTP/1.1')", [t]).unwrap();
    chain::record_for_tests(c, "access_log", "1").unwrap();
    assert!(chain::verify(c).unwrap().ok);
    assert!(c.execute("UPDATE chain SET hash = 'x'", []).is_err());
}
