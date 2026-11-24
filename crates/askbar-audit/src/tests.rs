use super::*;

fn tables(db: &AuditDb) -> Vec<String> {
    let mut stmt = db.conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

fn columns(db: &AuditDb, table: &str) -> Vec<String> {
    let mut stmt = db.conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid")).unwrap();
    stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

#[test]
fn open_creates_the_schema() {
    let db = AuditDb::open_in_memory().unwrap();
    assert_eq!(
        tables(&db),
        ["access_logs", "admin_actions", "alert_deliveries", "chain", "payloads", "prunes", "settings", "spans", "sqlite_sequence"]
    );
    assert_eq!(
        columns(&db, "spans"),
        [
            "id",
            "trace_id",
            "parent_id",
            "kind",
            "name",
            "status",
            "started_at",
            "ended_at",
            "provider_id",
            "model",
            "tokens_in",
            "tokens_out",
            "tokens_cached",
            "ttft_ms",
            "error_code",
            "user_id",
            "client_id",
            "attrs_json",
            "service",
            "boot_id"
        ]
    );
    assert_eq!(columns(&db, "admin_actions"), ["id", "at", "service", "category", "action", "outcome", "detail_json"]);
    assert_eq!(columns(&db, "prunes"), ["id", "at", "cutoff", "spans", "payloads"]);
    assert_eq!(columns(&db, "payloads"), ["span_id", "role", "key_id", "ciphertext", "created_at"]);
    let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v as usize, MIGRATIONS.len());
    let app: i64 = db.conn.query_row("PRAGMA application_id", [], |r| r.get(0)).unwrap();
    assert_eq!(app, APPLICATION_ID);
}

#[test]
fn reopening_is_idempotent_and_keeps_the_rows() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-open-{}", ulid::Ulid::new()));
    let path = dir.join("nested").join("audit.db");
    {
        let db = AuditDb::open(&path).unwrap();
        db.conn
            .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('s','t','turn','chat','done',1)", [])
            .unwrap();
    }
    let db = AuditDb::open(&path).unwrap();
    let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v as usize, MIGRATIONS.len(), "migrations ran once");
    let _ = std::fs::remove_dir_all(dir);
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn span(db: &AuditDb, id: &str, status: &str, started_at: i64) {
    db.conn
        .execute(
            "INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t','turn','chat',?2,?3)",
            rusqlite::params![id, status, started_at],
        )
        .unwrap();
}

#[test]
fn spans_and_payloads_younger_than_seven_days_cannot_be_deleted() {
    let db = AuditDb::open_in_memory().unwrap();
    let n = now();
    span(&db, "young", "done", n - 6 * 86400);
    span(&db, "old", "done", n - 8 * 86400);
    let e = db.conn.execute("DELETE FROM spans WHERE id = 'young'", []).unwrap_err();
    assert!(e.to_string().contains("7 days"), "{e}");
    assert!(db.conn.execute("DELETE FROM spans", []).is_err(), "a bulk delete that includes a young span fails whole");
    assert_eq!(db.conn.execute("DELETE FROM spans WHERE id = 'old'", []).unwrap(), 1);

    for (id, created) in [("young", n), ("old", n - 9 * 86400)] {
        db.conn
            .execute(
                "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,'request','k',x'00',?2)",
                rusqlite::params![id, created],
            )
            .unwrap();
    }
    assert!(db.conn.execute("DELETE FROM payloads WHERE span_id = 'young'", []).unwrap_err().to_string().contains("7 days"));
    assert_eq!(db.conn.execute("DELETE FROM payloads WHERE span_id = 'old'", []).unwrap(), 1);
    let left: i64 = db.conn.query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 1);
}

#[test]
fn a_span_can_change_only_while_it_is_running() {
    let db = AuditDb::open_in_memory().unwrap();
    span(&db, "s", "running", now());
    // Allowed: a running span is finished...
    assert_eq!(db.conn.execute("UPDATE spans SET status = 'done', ended_at = 5 WHERE id = 's'", []).unwrap(), 1);
    // ...and then it is fixed, whatever the change.
    for sql in [
        "UPDATE spans SET status = 'error' WHERE id = 's'",
        "UPDATE spans SET tokens_in = 9 WHERE id = 's'",
        "UPDATE spans SET status = 'running' WHERE id = 's'",
    ] {
        let e = db.conn.execute(sql, []).unwrap_err();
        assert!(e.to_string().contains("no longer running"), "{sql}: {e}");
    }
    // Marking a leftover running span interrupted is the same allowed path.
    span(&db, "left-over", "running", now());
    assert_eq!(db.conn.execute("UPDATE spans SET status = 'interrupted' WHERE status = 'running'", []).unwrap(), 1);
    let status: String = db.conn.query_row("SELECT status FROM spans WHERE id = 's'", [], |r| r.get(0)).unwrap();
    assert_eq!(status, "done");
}

#[test]
fn the_sweep_interrupts_only_running_spans_and_only_once() {
    let db = AuditDb::open_in_memory().unwrap();
    span(&db, "a", "running", now());
    span(&db, "b", "running", now());
    span(&db, "c", "done", now());
    assert_eq!(db.sweep_running().unwrap(), 2);
    let status = |id: &str| -> (String, Option<i64>, Option<String>) {
        db.conn
            .query_row("SELECT status, ended_at, error_code FROM spans WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
    };
    let (s, ended, code) = status("a");
    assert_eq!((s.as_str(), code.as_deref()), ("interrupted", Some("interrupted")));
    assert!(ended.is_some());
    assert_eq!(status("c"), ("done".into(), None, None), "finished spans are untouched");
    assert_eq!(db.sweep_running().unwrap(), 0);
}

#[test]
fn a_span_kind_outside_the_fixed_set_is_refused() {
    let db = AuditDb::open_in_memory().unwrap();
    for kind in ["turn", "gate", "attempt", "tool", "agent_step", "step"] {
        db.conn
            .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t',?1,'n','done',1)", [kind])
            .unwrap();
    }
    assert!(db
        .conn
        .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('x','t','banana','n','done',1)", [])
        .is_err());
}

#[test]
fn another_programs_database_or_a_newer_one_is_refused() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-ver-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let newer = dir.join("newer.db");
    {
        let db = AuditDb::open(&newer).unwrap();
        db.conn.execute_batch("PRAGMA user_version = 99").unwrap();
    }
    assert!(matches!(AuditDb::open(&newer), Err(AuditError::UnexpectedVersion { found: 99, .. })));
    let foreign = dir.join("foreign.db");
    Connection::open(&foreign).unwrap().execute_batch("CREATE TABLE t (x); PRAGMA user_version = 1;").unwrap();
    assert!(matches!(AuditDb::open(&foreign), Err(AuditError::UnexpectedVersion { found: 1, .. })));
    let _ = std::fs::remove_dir_all(dir);
}

fn temp_db() -> (std::path::PathBuf, AuditDb) {
    let dir = std::env::temp_dir().join(format!("askbar-audit-rec-{}", ulid::Ulid::new()));
    let db = AuditDb::open(&dir.join("audit.db")).unwrap();
    (dir, db)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn new_span(id: &str, kind: &str) -> SpanStart {
    SpanStart {
        id: id.into(),
        trace_id: "t".into(),
        kind: kind.into(),
        name: "n".into(),
        started_at: now_secs(),
        started_ms: now_ms(),
        ..Default::default()
    }
}

fn finish(id: &str) -> SpanFinish {
    SpanFinish { id: id.into(), status: "done".into(), ended_at: now_secs(), ended_ms: now_ms(), ..Default::default() }
}

#[tokio::test]
async fn spans_keep_millisecond_times_in_their_attributes_and_attributes_merge_on_finish() {
    let (dir, db) = temp_db();
    let rec = Recorder::spawn(db, 64);
    let id = "a1".to_string();
    let attrs0 = serde_json::json!({ "retry_index": 1, "keep": "start" }).to_string();
    rec.append(Record::Start(SpanStart { attrs_json: Some(attrs0), ..new_span("a1", "attempt") })).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let attrs1 = serde_json::json!({ "latency_ms": 31, "keep": "finish" }).to_string();
    rec.append(Record::Finish(SpanFinish { attrs_json: Some(attrs1), ..finish("a1") })).await.unwrap();
    let bare = "g1".to_string(); // no attributes of its own
    rec.append(Record::Start(new_span("g1", "gate"))).await.unwrap();
    rec.append(Record::Finish(finish("g1"))).await.unwrap();
    let conn = Connection::open(dir.join("audit.db")).unwrap();
    let attrs = |id: &str| -> serde_json::Value {
        let a: String = conn.query_row("SELECT attrs_json FROM spans WHERE id = ?1", [id], |r| r.get(0)).unwrap();
        serde_json::from_str(&a).unwrap()
    };
    let a = attrs(&id);
    assert_eq!(
        (a["retry_index"].as_i64(), a["latency_ms"].as_i64(), a["keep"].as_str()),
        (Some(1), Some(31), Some("finish")),
        "start attributes survive, finish ones are added or win"
    );
    let (s, e) = (a["started_ms"].as_i64().unwrap(), a["ended_ms"].as_i64().unwrap());
    assert!(e - s >= 25 && e - s < 2000, "the span took about 30 ms: {}", e - s);
    let b = attrs(&bare);
    assert!(b["started_ms"].is_i64() && b["ended_ms"].is_i64(), "even a span with no attributes of its own has its times: {b}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_cutoff_follows_the_setting_never_closer_than_a_week_to_the_real_clock() {
    let real = now_secs();
    assert_eq!(retention_cutoff(real, 30), real - 30 * 86_400);
    assert_eq!(retention_cutoff(real, 7), real - 7 * 86_400);
    assert_eq!(retention_cutoff(real, 1), real - 7 * 86_400, "below the floor, the floor applies");
    assert_eq!(retention_cutoff(real, 0), real - 7 * 86_400);
    // A clock far in the future would ask for more than the database allows: clamped.
    assert_eq!(retention_cutoff(real + 100 * 86_400, 7), real - 7 * 86_400);
    // A clock in the past is not clamped further.
    assert_eq!(retention_cutoff(real - 100 * 86_400, 30), real - 130 * 86_400);
}

#[tokio::test]
async fn retention_deletes_across_the_boundary_and_the_seven_day_rule_still_holds() {
    let (dir, db) = temp_db();
    let rec = Recorder::spawn(db, 64);
    let real = now_secs();
    let day = 86_400;
    // Spans of ages 40, 29.5, 10, 3 and 0 days, each with a payload of the same age.
    let ages = [("a40", 40 * day), ("a29", 29 * day + day / 2), ("a10", 10 * day), ("a3", 3 * day), ("a0", 0)];
    {
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        for (id, age) in ages {
            conn.execute(
                "INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at) VALUES (?1,'t','turn','c','done',?2,?2)",
                rusqlite::params![id, real - age],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,'reply','k',x'00',?2)",
                rusqlite::params![id, real - age],
            )
            .unwrap();
        }
    }
    let left = |dir: &std::path::Path| -> Vec<String> {
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        let mut stmt = conn.prepare("SELECT id FROM spans ORDER BY started_at").unwrap();
        let ids = stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        ids
    };

    // 30 days of retention: only the 40-day-old goes (spans and payloads).
    assert_eq!(rec.prune(30, real).await.unwrap(), (1, 1));
    assert_eq!(left(&dir), ["a29", "a10", "a3", "a0"]);
    // Two days later by the injected clock, the 29.5-day-old is past 30 days too.
    assert_eq!(rec.prune(30, real + 2 * day).await.unwrap(), (1, 1));
    assert_eq!(left(&dir), ["a10", "a3", "a0"]);
    // The shortest setting is 7 days, even if asked for 1: the 3-day-old and the new one stay.
    assert_eq!(rec.prune(1, real).await.unwrap(), (1, 1));
    assert_eq!(left(&dir), ["a3", "a0"]);
    // And a clock jumping far ahead cannot make it delete what is under a week old by the real
    // clock: the database's own rule holds as well.
    assert_eq!(rec.prune(7, real + 400 * day).await.unwrap(), (0, 0));
    assert_eq!(left(&dir), ["a3", "a0"]);
    let conn = Connection::open(dir.join("audit.db")).unwrap();
    assert!(conn.execute("DELETE FROM spans WHERE id = 'a3'", []).unwrap_err().to_string().contains("7 days"));
    let _ = std::fs::remove_dir_all(dir);
}

// ------------------------------------------------------------ the auditor's side (ADR 0051)

fn temp_recorder(capacity: usize) -> (Recorder, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("askbar-audit-ack-{}", ulid::Ulid::new()));
    let db = AuditDb::open(&dir.join("audit.db")).unwrap();
    (Recorder::spawn(db, capacity), dir)
}

fn started(id: &str, trace: &str, kind: &str, service: &str, boot: &str) -> SpanStart {
    SpanStart {
        id: id.into(),
        trace_id: trace.into(),
        kind: kind.into(),
        name: "n".into(),
        started_at: now_secs(),
        started_ms: now_ms(),
        service: Some(service.into()),
        boot_id: Some(boot.into()),
        ..Default::default()
    }
}

fn count(rec: &Recorder, sql: &str) -> i64 {
    rec.reader().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn an_acknowledged_append_is_committed_when_it_answers_and_a_retry_records_it_once() {
    let (rec, dir) = temp_recorder(64);
    let start = started("s1", "t1", "turn", "server", "b1");
    assert_eq!(rec.append(Record::Start(start.clone())).await, Ok(0));
    // Committed already: a separate reader sees it, with no flush in between.
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 's1' AND service = 'server' AND boot_id = 'b1'"), 1);
    assert_eq!(rec.append(Record::Start(start)).await, Ok(0), "a retry of a stored record is fine");
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 1);

    let fin = SpanFinish { tokens_in: Some(5), ..finish("s1") };
    assert_eq!(rec.append(Record::Finish(fin.clone())).await, Ok(0));
    assert_eq!(rec.append(Record::Finish(fin)).await, Ok(0));
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE status = 'done' AND tokens_in = 5"), 1);

    let payload =
        PayloadWrite { span_id: "s1".into(), role: "reply".into(), key_id: "k".into(), ciphertext: vec![1, 2, 3], created_at: now_secs() };
    assert_eq!(rec.append(Record::Payload(payload.clone())).await, Ok(0));
    assert_eq!(rec.append(Record::Payload(payload)).await, Ok(0));
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM payloads"), 1, "the same sealed bytes twice are one payload");

    assert_eq!(rec.append(Record::Finish(finish("nope"))).await, Err(AppendError::NoSuchSpan));
    let orphan =
        PayloadWrite { span_id: "nope".into(), role: "reply".into(), key_id: "k".into(), ciphertext: vec![9], created_at: now_secs() };
    assert_eq!(rec.append(Record::Payload(orphan)).await, Err(AppendError::NoSuchSpan));
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM payloads"), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn hello_ends_that_services_earlier_running_spans_and_nobody_elses() {
    let (rec, dir) = temp_recorder(64);
    for (id, svc, boot) in [("a1", "server", "b1"), ("a2", "server", "b1"), ("a3", "server", "b2"), ("g1", "model-gateway", "b1")] {
        rec.append(Record::Start(started(id, "t", "gate", svc, boot))).await.unwrap();
    }
    rec.append(Record::Finish(finish("a2"))).await.unwrap();
    let hello = |svc: &str, boot: &str| Record::Hello { service: svc.into(), boot_id: boot.into() };
    assert_eq!(rec.append(hello("server", "b2")).await, Ok(1), "only a1: a2 is finished, a3 is this process's");
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 'a1' AND status = 'interrupted' AND error_code = 'interrupted'"), 1);
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE status = 'running'"), 2, "a3 and the gateway's g1");
    assert_eq!(rec.append(hello("server", "b2")).await, Ok(0), "a second hello ends nothing");
    assert_eq!(rec.append(hello("model-gateway", "b9")).await, Ok(1));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn what_is_queued_together_is_committed_together_and_every_caller_is_answered() {
    let (rec, dir) = temp_recorder(64);
    rec.stall_writer().await;
    let tasks: Vec<_> = (0..20)
        .map(|i| {
            let rec = rec.clone();
            tokio::spawn(async move { rec.append(Record::Start(started(&format!("s{i:02}"), "t", "gate", "server", "b"))).await })
        })
        .collect();
    // Not committed while the writer is held.
    for _ in 0..50 {
        if rec.backlog().queued >= 21 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 0);
    rec.pause_writer(false);
    for t in tasks {
        assert_eq!(t.await.unwrap(), Ok(0));
    }
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 20);
    assert_eq!(rec.backlog().queued, 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_backlog_counts_queued_records_and_how_long_the_oldest_has_waited() {
    let (rec, dir) = temp_recorder(8);
    assert_eq!(rec.backlog(), Backlog { queued: 0, oldest: Duration::ZERO });
    rec.stall_writer().await;
    let held = rec.clone();
    let task = tokio::spawn(async move { held.append(Record::Start(started("s1", "t", "gate", "server", "b"))).await });
    for _ in 0..100 {
        if rec.backlog().queued >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(120)).await;
    let b = rec.backlog();
    assert_eq!(b.queued, 2, "the event the writer holds and the one behind it");
    assert!(b.oldest >= Duration::from_millis(100), "{b:?}");
    rec.pause_writer(false);
    assert_eq!(task.await.unwrap(), Ok(0));
    assert_eq!(rec.backlog().queued, 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_full_queue_refuses_an_append_and_records_nothing() {
    let (rec, dir) = temp_recorder(2);
    rec.stall_writer().await;
    let tasks: Vec<_> = (0..2)
        .map(|i| {
            let rec = rec.clone();
            tokio::spawn(async move { rec.append(Record::Start(started(&format!("s{i}"), "t", "gate", "server", "b"))).await })
        })
        .collect();
    for _ in 0..100 {
        if !rec.healthy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(rec.append(Record::Start(started("s9", "t", "gate", "server", "b"))).await, Err(AppendError::Full));
    rec.pause_writer(false);
    for t in tasks {
        assert_eq!(t.await.unwrap(), Ok(0));
    }
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 's9'"), 0);
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn an_admin_action_is_stored_once_with_its_caller_and_can_never_be_changed_or_deleted() {
    let (rec, dir) = temp_recorder(16);
    let action = AdminActionWrite {
        id: "01A".into(),
        at: now_secs(),
        service: "admin".into(),
        category: "settings".into(),
        action: "kill switch on".into(),
        outcome: "done".into(),
        detail_json: Some(r#"{"kill_switch":true}"#.into()),
    };
    assert_eq!(rec.append(Record::AdminAction(action.clone())).await, Ok(0));
    assert_eq!(rec.append(Record::AdminAction(action)).await, Ok(0));
    let conn = rec.reader().unwrap();
    let rows = list_admin_actions(&conn, None, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["service"].clone(), rows[0]["detail"]["kill_switch"].clone()), ("admin".into(), true.into()));
    // The store itself refuses a change or a delete, for any connection.
    let db = AuditDb::open(&dir.join("audit.db")).unwrap();
    assert!(db.conn().execute("UPDATE admin_actions SET outcome = 'done' WHERE id = '01A'", []).is_err());
    assert!(db.conn().execute("DELETE FROM admin_actions", []).is_err());
    rec.prune(30, now_secs()).await.unwrap();
    assert!(db.conn().execute("UPDATE prunes SET spans = 99", []).is_err(), "a prune record cannot be changed");
    assert!(db.conn().execute("DELETE FROM prunes", []).is_err(), "nor deleted");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn an_alert_delivery_is_stored_once_with_its_caller_chained_and_can_never_be_changed_or_deleted() {
    let (rec, dir) = temp_recorder(16);
    let d = AlertDeliveryWrite {
        id: "01D".into(),
        at: now_secs(),
        caller: "alerter".into(),
        kind: "unsealed".into(),
        channel: "ntfy".into(),
        outcome: "sent".into(),
        attempts: 2,
    };
    assert_eq!(rec.append(Record::AlertDelivery(d.clone())).await, Ok(0));
    assert_eq!(rec.append(Record::AlertDelivery(d)).await, Ok(0), "the same id twice is one record");
    let conn = rec.reader().unwrap();
    let rows = list_alert_deliveries(&conn, None, None).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0]["caller"].clone(), rows[0]["kind"].clone(), rows[0]["attempts"].clone()),
        ("alerter".into(), "unsealed".into(), 2.into())
    );
    let chained: i64 =
        conn.query_row("SELECT COUNT(*) FROM chain WHERE kind = 'alert_delivery' AND ref = '01D'", [], |r| r.get(0)).unwrap();
    assert_eq!(chained, 1, "it has one entry in the chain, written with it");
    let report = chain::verify(&conn).unwrap();
    assert!(report.ok, "{:?}", report.problems);
    let db = AuditDb::open(&dir.join("audit.db")).unwrap();
    assert!(db.conn().execute("UPDATE alert_deliveries SET outcome = 'failed'", []).is_err());
    assert!(db.conn().execute("DELETE FROM alert_deliveries", []).is_err());
    // A row with no entry in the chain is a problem the check finds.
    db.conn()
        .execute("INSERT INTO alert_deliveries (id, caller, at, kind, channel, outcome, attempts) VALUES ('01E','alerter',1,'x','ntfy','sent',1)", [])
        .unwrap();
    assert!(!chain::verify(&conn).unwrap().ok);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn migration_6_adds_alert_deliveries_and_keeps_the_chain_a_file_from_migration_5_had() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-m6-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audit.db");
    {
        // A file as version 5 left it: the first five migrations, an admin action and its chain entry.
        let conn = Connection::open(&path).unwrap();
        for (i, sql) in MIGRATIONS.iter().take(5).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 3 {
                chain::backfill(&conn).unwrap();
            }
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 5").unwrap();
        conn.execute(
            "INSERT INTO admin_actions (id, at, service, category, action, outcome) VALUES ('01A', 1, 'admin', 'settings', 'x', 'done')",
            [],
        )
        .unwrap();
        chain::record(&conn, "admin_action", "01A").unwrap();
    }
    let db = AuditDb::open(&path).unwrap();
    let version: i64 = db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version as usize, MIGRATIONS.len());
    assert!(MIGRATIONS.len() >= 6);
    assert_eq!(db.conn().query_row("SELECT COUNT(*) FROM alert_deliveries", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    assert_eq!(db.conn().query_row("SELECT COUNT(*) FROM chain WHERE kind = 'admin_action'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
    assert!(chain::verify(db.conn()).unwrap().ok, "the old entry still verifies");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn migration_7_adds_the_step_kind_and_old_records_verify_unchanged() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-m7-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("audit.db");
    {
        // A file as version 6 left it, holding a finished span with its chain entries.
        let conn = Connection::open(&path).unwrap();
        for (i, sql) in MIGRATIONS.iter().take(6).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 3 {
                chain::backfill(&conn).unwrap();
            }
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 6").unwrap();
        conn.execute(
            "INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at, service, boot_id)
             VALUES ('old','t','attempt','n','done',1,2,'model-gateway','b')",
            [],
        )
        .unwrap();
        chain::record(&conn, "span_start", "old").unwrap();
        chain::record(&conn, "span_finish", "old").unwrap();
    }
    let db = AuditDb::open(&path).unwrap();
    assert!(MIGRATIONS.len() >= 7);
    let c = db.conn();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM spans WHERE id = 'old' AND service = 'model-gateway' AND boot_id = 'b'", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(chain::verify(c).unwrap().ok, "records from before the step kind verify unchanged");
    c.execute(
        "INSERT INTO spans (id, trace_id, parent_id, kind, name, status, started_at) VALUES ('s','t','old','step','n','running',1)",
        [],
    )
    .unwrap();
    assert!(c.execute("UPDATE spans SET name = 'x' WHERE id = 'old'", []).is_err(), "the triggers were made again");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_step_under_an_attempt_is_stored_and_chained() {
    let (rec, dir) = temp_recorder(64);
    rec.append(Record::Start(started("a1", "t", "attempt", "model-gateway", "b"))).await.unwrap();
    let step = SpanStart { parent_id: Some("a1".into()), ..started("st1", "t", "step", "model-gateway", "b") };
    rec.append(Record::Start(step)).await.unwrap();
    rec.append(Record::Finish(finish("st1"))).await.unwrap();
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 'st1' AND kind = 'step' AND parent_id = 'a1'"), 1);
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM chain WHERE ref = 'st1' AND kind IN ('span_start','span_finish')"), 2);
    let conn = Connection::open(dir.join("audit.db")).unwrap();
    assert!(chain::verify(&conn).unwrap().ok);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_prune_writes_its_record_with_the_delete_and_the_record_stays() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-prune-{}", ulid::Ulid::new()));
    let db = AuditDb::open(&dir.join("audit.db")).unwrap();
    let old = now_secs() - 40 * 86_400;
    db.conn
        .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('old','t','turn','n','done',?1)", [old])
        .unwrap();
    db.conn
        .execute("INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES ('old','reply','k',x'01',?1)", [old])
        .unwrap();
    db.conn
        .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('young','t','turn','n','done',?1)", [now_secs()])
        .unwrap();
    let rec = Recorder::spawn(db, 16);
    assert_eq!(rec.prune(30, now_secs()).await, Ok((1, 1)));
    assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 1, "only the young one is left");
    let prunes = list_prunes(&rec.reader().unwrap()).unwrap();
    assert_eq!(prunes.len(), 1);
    assert_eq!((prunes[0]["spans"].clone(), prunes[0]["payloads"].clone()), (1.into(), 1.into()));
    assert!(prunes[0]["cutoff"].as_i64().unwrap() <= now_secs() - 29 * 86_400);
    // Nothing to delete is still a prune, and still has its record.
    assert_eq!(rec.prune(30, now_secs()).await, Ok((0, 0)));
    assert_eq!(list_prunes(&rec.reader().unwrap()).unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn token_counts_group_by_the_provider_and_model_of_the_last_attempt_and_hold_no_prices() {
    let (rec, dir) = temp_recorder(64);
    let mut n = 0;
    for (prov, model, tin, tout) in [("p1", "m1", 10, 20), ("p1", "m1", 5, 5), ("p2", "m9", 1, 2)] {
        n += 1;
        let (turn, attempt) = (format!("t{n}"), format!("a{n}"));
        rec.append(Record::Start(started(&turn, &turn, "turn", "model-gateway", "b"))).await.unwrap();
        let mut a = started(&attempt, &turn, "attempt", "model-gateway", "b");
        a.provider_id = Some(prov.into());
        a.model = Some(model.into());
        rec.append(Record::Start(a)).await.unwrap();
        rec.append(Record::Finish(SpanFinish { tokens_in: Some(tin), tokens_out: Some(tout), tokens_cached: Some(0), ..finish(&turn) }))
            .await
            .unwrap();
    }
    let rows = token_counts(&rec.reader().unwrap(), 0).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        serde_json::json!({ "provider_id": "p1", "model": "m1", "requests": 2, "tokens_in": 15, "tokens_out": 25, "tokens_cached": 0 })
    );
    assert_eq!(rows[1]["provider_id"], "p2");
    assert!(rows.iter().all(|r| r.get("cost").is_none()), "the auditor holds no prices");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_setting_is_stored_through_the_writer_and_replaced() {
    let (rec, dir) = temp_recorder(8);
    assert_eq!(get_setting(&rec.reader().unwrap(), "retention_days").unwrap(), None);
    rec.append(Record::Setting { key: "retention_days".into(), value: "45".into() }).await.unwrap();
    rec.append(Record::Setting { key: "retention_days".into(), value: "60".into() }).await.unwrap();
    assert_eq!(get_setting(&rec.reader().unwrap(), "retention_days").unwrap().as_deref(), Some("60"));
    let _ = std::fs::remove_dir_all(dir);
}
