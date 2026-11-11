//! The hash chain (ADR 0010 rule 7): every record is chained as it is taken, a full check finds an edit, an insertion and
//! a deletion, a prune is told from a deletion, and a file from before the chain is chained when it is opened.

use askbar_audit::chain::{self, GENESIS};
use askbar_audit::rusqlite::Connection;
use askbar_audit::{AdminActionWrite, AuditDb, Kind, PayloadWrite, Record, Recorder, SpanFinish, SpanStart};
use std::path::PathBuf;

fn dir() -> PathBuf {
    std::env::temp_dir().join(format!("askbar-audit-chain-{}", ulid::Ulid::new()))
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn start(id: &str, at: i64) -> SpanStart {
    let mut s = SpanStart::new("t1", Kind::Turn, "reply").who("u1", Some("c1"));
    s.id = id.into();
    s.started_at = at;
    s.service = Some("model-gateway".into());
    s.boot_id = Some("boot-1".into());
    s
}

fn payload(span: &str, at: i64) -> PayloadWrite {
    PayloadWrite { span_id: span.into(), role: "reply".into(), key_id: "k1".into(), ciphertext: vec![1, 2, 3, 4], created_at: at }
}

fn action(id: &str) -> AdminActionWrite {
    AdminActionWrite {
        id: id.into(),
        at: now(),
        service: "admin".into(),
        category: "settings".into(),
        action: "update".into(),
        outcome: "done".into(),
        detail_json: Some("{\"changed\":[\"history_turns\"]}".into()),
    }
}

/// A recorder over a file, with one of each record in it: a finished span with a payload, a running span, an admin action.
async fn filled() -> (Recorder, PathBuf) {
    let d = dir();
    let path = d.join("audit.db");
    let rec = Recorder::spawn(AuditDb::open(&path).unwrap(), 100);
    let t = now();
    rec.append(Record::Start(start("s1", t))).await.unwrap();
    rec.append(Record::Payload(payload("s1", t))).await.unwrap();
    rec.append(Record::Finish(SpanFinish::new("s1", "done"))).await.unwrap();
    rec.append(Record::Start(start("s2", t))).await.unwrap();
    rec.append(Record::AdminAction(action("a1"))).await.unwrap();
    (rec, path)
}

fn reader(path: &PathBuf) -> Connection {
    Connection::open(path).unwrap()
}

/// What someone with the file can do that the store itself forbids: take the triggers away first.
fn without_triggers(path: &PathBuf) -> Connection {
    let c = reader(path);
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
    c
}

#[tokio::test]
async fn every_record_is_chained_in_order_and_a_retry_adds_nothing() {
    let (rec, path) = filled().await;
    let c = reader(&path);
    let kinds: Vec<String> =
        c.prepare("SELECT kind FROM chain ORDER BY seq").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(kinds, ["span_start", "payload", "span_finish", "span_start", "admin_action"]);
    let first: String = c.query_row("SELECT prev_hash FROM chain WHERE seq = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(first, GENESIS, "the first entry follows the genesis hash");
    let links: i64 =
        c.query_row("SELECT COUNT(*) FROM chain a JOIN chain b ON b.seq = a.seq + 1 WHERE b.prev_hash = a.hash", [], |r| r.get(0)).unwrap();
    assert_eq!(links, 4, "each entry names the hash of the one before it");

    // The same record again (a caller whose answer was lost) is stored once, so it is chained once.
    let t = now();
    rec.append(Record::Start(start("s1", t))).await.unwrap();
    rec.append(Record::AdminAction(action("a1"))).await.unwrap();
    rec.append(Record::Finish(SpanFinish::new("s1", "done"))).await.unwrap();
    let n: i64 = c.query_row("SELECT COUNT(*) FROM chain", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 5);

    let report = chain::verify(&c).unwrap();
    assert!(report.ok, "{report:?}");
    assert_eq!((report.entries, report.head.seq), (5, 5));
    assert_eq!(chain::head(&c).unwrap(), report.head);
    assert_eq!(chain::entry_hash(&c, 3).unwrap().unwrap().len(), 64);
    assert_eq!(chain::entry_hash(&c, 6).unwrap(), None);
}

#[tokio::test]
async fn the_chain_itself_cannot_be_changed_or_deleted_through_the_store() {
    let (_rec, path) = filled().await;
    let c = reader(&path);
    assert!(c.execute("UPDATE chain SET hash = 'x' WHERE seq = 1", []).is_err());
    assert!(c.execute("DELETE FROM chain WHERE seq = 5", []).is_err());
    assert!(chain::verify(&c).unwrap().ok);
}

#[tokio::test]
async fn an_edited_record_is_found_whichever_table_it_is_in() {
    for (what, sql) in [
        ("a sealed payload", "UPDATE payloads SET ciphertext = x'ff' WHERE span_id = 's1'"),
        ("a finished span", "UPDATE spans SET status = 'error', error_code = 'x' WHERE id = 's1'"),
        ("an admin action", "UPDATE admin_actions SET outcome = 'refused' WHERE id = 'a1'"),
    ] {
        let (_rec, path) = filled().await;
        let c = without_triggers(&path);
        c.execute(sql, []).unwrap();
        let report = chain::verify(&c).unwrap();
        assert!(!report.ok, "{what} was edited and the check passed");
        assert!(report.problems.iter().any(|p| p.reason.contains("changed after")), "{what}: {report:?}");
    }
}

#[tokio::test]
async fn an_edited_entry_breaks_the_check_at_that_entry_and_after_it() {
    let (_rec, path) = filled().await;
    let c = without_triggers(&path);
    c.execute("UPDATE chain SET digest = ?1 WHERE seq = 2", [GENESIS]).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(!report.ok);
    assert!(report.problems.iter().any(|p| p.seq == Some(2) && p.reason.contains("entry itself")), "{report:?}");
    // A forger who fixes that entry's own hash still leaves the next one pointing at the old hash.
    let prev: String = c.query_row("SELECT prev_hash FROM chain WHERE seq = 2", [], |r| r.get(0)).unwrap();
    assert_eq!(prev.len(), 64);
}

#[tokio::test]
async fn a_deleted_record_or_entry_and_an_inserted_record_are_found() {
    // A record deleted from the table: the chain still has its entry and no prune explains it.
    let (_rec, path) = filled().await;
    let c = without_triggers(&path);
    c.execute("DELETE FROM admin_actions WHERE id = 'a1'", []).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.kind == "admin_action" && p.reason.contains("no prune explains")), "{report:?}");

    // An entry deleted from the middle of the chain: the gap, and the entry after it no longer follows.
    let (_rec, path) = filled().await;
    let c = without_triggers(&path);
    c.execute("DELETE FROM chain WHERE seq = 3", []).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.seq == Some(4) && p.reason.contains("missing")), "{report:?}");
    assert!(report.problems.iter().any(|p| p.kind == "span_finish" && p.reason.contains("no entry")), "{report:?}");

    // The newest entry deleted: the chain is intact but the record has no entry.
    let (_rec, path) = filled().await;
    let c = without_triggers(&path);
    c.execute("DELETE FROM chain WHERE seq = 5", []).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.kind == "admin_action" && p.reason.contains("no entry")), "{report:?}");

    // A record put in by hand.
    let (_rec, path) = filled().await;
    let c = reader(&path);
    c.execute(
        "INSERT INTO admin_actions (id, at, service, category, action, outcome) VALUES ('forged', ?1, 'admin', 'users', 'delete', 'done')",
        [now()],
    )
    .unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.reference == "forged" && p.reason.contains("no entry")), "{report:?}");
}

#[tokio::test]
async fn a_prune_is_chained_and_explains_what_it_removed_and_nothing_else() {
    let d = dir();
    let path = d.join("audit.db");
    let rec = Recorder::spawn(AuditDb::open(&path).unwrap(), 100);
    let old = now() - 40 * 86400;
    rec.append(Record::Start(start("old", old))).await.unwrap();
    rec.append(Record::Payload(payload("old", old))).await.unwrap();
    rec.append(Record::Finish(SpanFinish::new("old", "done"))).await.unwrap();
    rec.append(Record::Start(start("new", now()))).await.unwrap();
    rec.append(Record::AdminAction(action("a1"))).await.unwrap();
    let before = chain::head(&reader(&path)).unwrap().seq;
    let (spans, payloads) = rec.prune(30, now()).await.unwrap();
    assert_eq!((spans, payloads), (1, 1));

    let c = reader(&path);
    let head = chain::head(&c).unwrap();
    assert_eq!(head.seq, before + 1, "the prune is one more entry");
    let kind: String = c.query_row("SELECT kind FROM chain WHERE seq = ?1", [head.seq], |r| r.get(0)).unwrap();
    assert_eq!(kind, "prune");
    let kept: i64 = c.query_row("SELECT COUNT(*) FROM chain", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, head.seq, "no entry is removed with its record: entries are digests");
    let report = chain::verify(&c).unwrap();
    assert!(report.ok, "the removed records are explained by the prune: {report:?}");

    // Something young removed by hand is not explained: the prune's cutoff is 30 days back.
    let c = without_triggers(&path);
    c.execute("DELETE FROM spans WHERE id = 'new'", []).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.reference == "new" && p.reason.contains("no prune explains")), "{report:?}");
    // And a prune record whose numbers were changed shows too.
    let (_rec, path) = (rec, path);
    let c = without_triggers(&path);
    c.execute("UPDATE prunes SET spans = 0", []).unwrap();
    let report = chain::verify(&c).unwrap();
    assert!(report.problems.iter().any(|p| p.kind == "prune" && p.reason.contains("changed after")), "{report:?}");
}

#[tokio::test]
async fn spans_ended_by_a_hello_or_a_restart_are_chained_too() {
    let (rec, path) = filled().await;
    let n = rec.append(Record::Hello { service: "model-gateway".into(), boot_id: "boot-2".into() }).await.unwrap();
    assert_eq!(n, 1, "s2 was running under boot-1");
    let c = reader(&path);
    let report = chain::verify(&c).unwrap();
    assert!(report.ok, "{report:?}");
    let finishes: i64 = c.query_row("SELECT COUNT(*) FROM chain WHERE kind = 'span_finish' AND ref = 's2'", [], |r| r.get(0)).unwrap();
    assert_eq!(finishes, 1);

    // The sweep at start-up does the same.
    let d = dir();
    let path = d.join("audit.db");
    {
        let rec = Recorder::spawn(AuditDb::open(&path).unwrap(), 100);
        rec.append(Record::Start(start("r1", now()))).await.unwrap();
    }
    let db = AuditDb::open(&path).unwrap();
    assert_eq!(db.sweep_running().unwrap(), 1);
    assert!(chain::verify(db.conn()).unwrap().ok);
    assert_eq!(db.chain_head().unwrap().seq, 2, "the start and the interrupted finish");
}

#[test]
fn a_file_from_before_the_chain_is_chained_when_it_is_opened() {
    let d = dir();
    std::fs::create_dir_all(&d).unwrap();
    let path = d.join("audit.db");
    // Build version 3 by hand: open, then take the chain away as if the file had never had it.
    {
        let db = AuditDb::open(&path).unwrap();
        let c = db.conn();
        let t = now();
        c.execute(
            "INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at, service, boot_id) VALUES ('p1','t','turn','reply','done',?1,?1,'model-gateway','b')",
            [t],
        )
        .unwrap();
        c.execute("INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES ('p1','reply','k',x'0102',?1)", [t])
            .unwrap();
        c.execute(
            "INSERT INTO admin_actions (id, at, service, category, action, outcome) VALUES ('a0',?1,'admin','settings','update','done')",
            [t],
        )
        .unwrap();
        c.execute_batch("DROP TRIGGER chain_is_never_deleted; DELETE FROM chain; DROP TABLE chain; DROP TRIGGER access_logs_are_never_changed; DROP TRIGGER access_logs_keep_a_week; DROP TABLE access_logs; DROP TRIGGER alert_deliveries_are_never_changed; DROP TRIGGER alert_deliveries_are_never_deleted; DROP TABLE alert_deliveries; PRAGMA user_version = 3;").unwrap();
    }
    let db = AuditDb::open(&path).unwrap();
    let c = db.conn();
    let report = chain::verify(c).unwrap();
    assert!(report.ok, "{report:?}");
    assert_eq!(report.entries, 4, "the span's start and finish, the payload and the admin action");
    assert_ne!(report.head.hash, GENESIS);
    drop(db);
    // Opening it again does not chain it a second time.
    let db = AuditDb::open(&path).unwrap();
    assert_eq!(db.chain_head().unwrap().seq, 4);
}

#[test]
fn the_head_of_an_empty_chain_is_the_genesis_hash() {
    let db = AuditDb::open_in_memory().unwrap();
    let h = db.chain_head().unwrap();
    assert_eq!((h.seq, h.hash.as_str()), (0, GENESIS));
    let report = chain::verify(db.conn()).unwrap();
    assert!(report.ok && report.entries == 0);
}
