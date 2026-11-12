//! The admin's check of the audit chain (ADR 0010 rule 7): it keeps the newest head it has checked, and finds a chain
//! that is shorter (a rollback), an entry that is not the one it stored (a rewrite) and records that no longer match
//! their entries (an edit). A stub auditor drives each finding; two tests use the real auditor, one with a row edited in
//! `audit.db` and one with an older copy of the file put back.

mod common;
use askbar_admin::auditor_link::AuditorLink;
use askbar_admin::chain_check::State;
use askbar_admin::{server_link::ServerLink, web, AdminDb, AdminState};
use axum::{extract::Path, extract::State as AxState, http::StatusCode, routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Stub {
    up: Mutex<bool>,
    head: Mutex<(i64, String)>,
    /// What `/chain/entries/{n}` answers: `seq -> hash`; a number not in it is a 404.
    entries: Mutex<std::collections::BTreeMap<i64, String>>,
    verify: Mutex<Value>,
    verifies: Mutex<u32>,
}

async fn stub() -> (String, Arc<Stub>) {
    let s = Arc::new(Stub {
        up: Mutex::new(true),
        verify: Mutex::new(json!({ "ok": true, "problems": [], "more": false })),
        ..Default::default()
    });
    *s.head.lock().unwrap() = (0, "0".repeat(64));
    let app = Router::new()
        .route(
            "/api/audit/v1/chain/head",
            get(|AxState(s): AxState<Arc<Stub>>| async move {
                if !*s.up.lock().unwrap() {
                    return Err(StatusCode::SERVICE_UNAVAILABLE);
                }
                let (seq, hash) = s.head.lock().unwrap().clone();
                Ok(Json(json!({ "seq": seq, "hash": hash })))
            }),
        )
        .route(
            "/api/audit/v1/chain/entries/{id}",
            get(|AxState(s): AxState<Arc<Stub>>, Path(id): Path<i64>| async move {
                match s.entries.lock().unwrap().get(&id) {
                    Some(h) => Ok(Json(json!({ "seq": id, "hash": h }))),
                    None => Err(StatusCode::NOT_FOUND),
                }
            }),
        )
        .route(
            "/api/audit/v1/chain/verify",
            get(|AxState(s): AxState<Arc<Stub>>| async move {
                *s.verifies.lock().unwrap() += 1;
                Json(s.verify.lock().unwrap().clone())
            }),
        )
        .with_state(s.clone());
    (common::serve(app).await, s)
}

impl Stub {
    /// The chain has `seq` entries, the newest with hash `h<seq>`.
    fn grow_to(&self, seq: i64) {
        let mut e = self.entries.lock().unwrap();
        for n in 1..=seq {
            e.entry(n).or_insert_with(|| format!("h{n}"));
        }
        *self.head.lock().unwrap() = (seq, format!("h{seq}"));
    }
}

async fn admin(auditor_url: &str) -> Arc<AdminState> {
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, common::PW).unwrap();
    let state = AdminState::with_server(db, ServerLink::new("http://127.0.0.1:9", "mgt_x".into()));
    state.auditor.set(AuditorLink::new(auditor_url, "admrd_stub".into())).ok().unwrap();
    state
}

fn stored(state: &AdminState) -> Option<(i64, String)> {
    let db = state.db.try_lock().unwrap();
    db.conn().query_row("SELECT seq, hash FROM audit_chain_head WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok()
}

#[tokio::test]
async fn the_first_check_stores_the_head_and_a_chain_that_only_grows_stays_ok() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    assert_eq!(a.chain.state(), State::Unknown, "nothing is claimed before the first check");
    s.grow_to(3);
    a.chain.check(&a, &link, false).await;
    assert_eq!((a.chain.state(), stored(&a)), (State::Ok, Some((3, "h3".into()))));
    s.grow_to(7);
    a.chain.check(&a, &link, false).await;
    assert_eq!((a.chain.state(), stored(&a)), (State::Ok, Some((7, "h7".into()))), "the stored head moves forward with the chain");
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.state(), State::Ok, "the same head again is fine");
}

#[tokio::test]
async fn a_shorter_chain_is_a_rollback_and_the_stored_head_stays_where_it_was() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    s.grow_to(10);
    a.chain.check(&a, &link, false).await;
    // An older copy of the file was put back: its chain ends at 6.
    s.entries.lock().unwrap().retain(|n, _| *n <= 6);
    *s.head.lock().unwrap() = (6, "h6".into());
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.state(), State::Rollback { stored: 10, current: 6 });
    assert_eq!(stored(&a), Some((10, "h10".into())), "the evidence is kept, not overwritten by the shorter chain");
    let status = a.chain.status();
    assert_eq!(status["state"], "rollback");
    assert!(status["detail"].as_str().unwrap().contains("entry 6") && status["detail"].as_str().unwrap().contains("entry 10"));

    // Once the auditor has the chain it had, and more, the finding goes away.
    s.grow_to(12);
    a.chain.check(&a, &link, false).await;
    assert_eq!((a.chain.state(), stored(&a)), (State::Ok, Some((12, "h12".into()))));
}

#[tokio::test]
async fn an_entry_with_another_hash_or_no_entry_at_all_is_a_rewrite() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    s.grow_to(5);
    a.chain.check(&a, &link, false).await;
    // The chain is as long as before, but entry 5 is another one: it was made again.
    s.entries.lock().unwrap().insert(5, "other".into());
    *s.head.lock().unwrap() = (6, "h6".into());
    s.entries.lock().unwrap().insert(6, "h6".into());
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.state(), State::Rewritten { seq: 5 });
    assert_eq!(stored(&a), Some((5, "h5".into())));
    assert_eq!(a.chain.status()["state"], "rewritten");

    // An entry that is gone altogether is the same finding.
    s.entries.lock().unwrap().remove(&5);
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.state(), State::Rewritten { seq: 5 });
}

#[tokio::test]
async fn a_full_check_that_finds_problems_is_an_edit_until_one_finds_none() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    s.grow_to(4);
    a.chain.check(&a, &link, false).await;
    assert_eq!(*s.verifies.lock().unwrap(), 1, "the first check also checks the whole store");
    a.chain.check(&a, &link, false).await;
    assert_eq!(*s.verifies.lock().unwrap(), 1, "and the whole store is not read again at every check");

    *s.verify.lock().unwrap() = json!({ "ok": false, "problems": [{ "seq": 2, "kind": "payload", "ref": "9", "reason": "the record was changed after it was written" }], "more": false });
    a.chain.check(&a, &link, true).await;
    assert_eq!(*s.verifies.lock().unwrap(), 2);
    match a.chain.state() {
        State::Edited { problems, first } => {
            assert_eq!(problems, 1);
            assert!(first.contains("payload") && first.contains("changed after"), "{first}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(a.chain.status()["state"], "edited");
    // A cheap check in between does not clear it: only a full check that finds nothing does.
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.status()["state"], "edited");
    *s.verify.lock().unwrap() = json!({ "ok": true, "problems": [], "more": false });
    a.chain.check(&a, &link, true).await;
    assert_eq!(a.chain.state(), State::Ok);
}

#[tokio::test]
async fn an_auditor_that_does_not_answer_is_not_a_finding_and_changes_nothing() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    s.grow_to(3);
    a.chain.check(&a, &link, false).await;
    *s.up.lock().unwrap() = false;
    s.grow_to(2); // would be a rollback, if it were seen
    a.chain.check(&a, &link, false).await;
    assert_eq!((a.chain.state(), stored(&a)), (State::Ok, Some((3, "h3".into()))));

    // And an auditor that was never reached says nothing at all.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let a2 = admin(&dead).await;
    let link2 = a2.auditor.get().unwrap().clone();
    a2.chain.check(&a2, &link2, false).await;
    assert_eq!((a2.chain.state(), stored(&a2)), (State::Unknown, None));
}

#[tokio::test]
async fn the_status_endpoint_carries_the_finding_for_the_banner() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    let adm = common::serve(askbar_admin::router(a.clone())).await;
    let http = reqwest::Client::new();
    let login = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = login.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let status = || async {
        let r = http.get(format!("{adm}/api/admin/v1/status")).header("cookie", &cookie).send().await.unwrap();
        r.json::<Value>().await.unwrap()
    };
    assert_eq!(status().await["audit_chain"]["state"], "unknown");
    s.grow_to(5);
    a.chain.check(&a, &link, false).await;
    assert_eq!(status().await["audit_chain"], json!({ "state": "ok", "detail": "" }));
    *s.head.lock().unwrap() = (2, "h2".into());
    a.chain.check(&a, &link, false).await;
    let st = status().await;
    assert_eq!(st["audit_chain"]["state"], "rollback");
    assert!(st["audit_chain"]["detail"].as_str().unwrap().starts_with("The audit trail is shorter"));
}

// ------------------------------------------------------------------ the real auditor

use askbar_server::audit::Kind;
use askbar_server::testing::Rig;

fn read_token(rig: &Rig) -> String {
    std::fs::read_to_string(rig.admin_tokens.join("call/admin-auditor-read.token")).unwrap().trim().to_string()
}

#[tokio::test]
async fn a_row_edited_in_audit_db_is_found_by_the_full_check() {
    let rig = Rig::start().await;
    let a = admin(&rig.audit_url).await;
    let link = AuditorLink::new(&rig.audit_url, read_token(&rig));
    rig.seed("t1", Kind::Turn, "reply", None, true).await;
    rig.seed("t2", Kind::Turn, "reply", None, true).await;
    a.chain.check(&a, &link, true).await;
    assert_eq!(a.chain.state(), State::Ok, "an untouched store checks out");
    assert_eq!(stored(&a).unwrap().0, 4, "two spans, each started and finished");

    // Someone with the file edits a finished span (the store's own triggers are the first thing they remove).
    let conn = askbar_audit::rusqlite::Connection::open(rig.dir.join("audit/audit.db")).unwrap();
    conn.execute_batch(
        "DROP TRIGGER spans_change_only_while_running; UPDATE spans SET status = 'error', error_code = 'edited' WHERE trace_id = 't1';",
    )
    .unwrap();
    a.chain.check(&a, &link, true).await;
    match a.chain.state() {
        State::Edited { problems, first } => {
            assert!(problems >= 1);
            assert!(first.contains("span_finish") && first.contains("changed after"), "{first}");
        }
        other => panic!("an edited row was not found: {other:?}"),
    }
    assert!(a.chain.status()["detail"].as_str().unwrap().contains("do not match their entries"));
}

#[tokio::test]
async fn an_older_copy_of_audit_db_put_back_is_a_rollback() {
    let rig = Rig::start().await;
    let a = admin(&rig.audit_url).await;
    let link = AuditorLink::new(&rig.audit_url, read_token(&rig));
    rig.seed("t1", Kind::Turn, "reply", None, true).await;
    // A copy of the file as it is now (the backup someone will restore later).
    let older = rig.dir.join("older.db");
    askbar_audit::rusqlite::Connection::open(rig.dir.join("audit/audit.db"))
        .unwrap()
        .execute("VACUUM INTO ?1", [older.to_str().unwrap()])
        .unwrap();
    rig.seed("t2", Kind::Turn, "reply", None, true).await;
    rig.seed("t3", Kind::Turn, "reply", None, true).await;
    a.chain.check(&a, &link, true).await;
    assert_eq!((a.chain.state(), stored(&a).unwrap().0), (State::Ok, 6));

    // A second auditor starts on the older copy: a whole, valid chain, only shorter.
    let tokens = rig.dir.join("audit-token");
    let recorder = askbar_audit::Recorder::spawn(askbar_audit::AuditDb::open(&older).unwrap(), 1000);
    let free: askbar_auditor::disk::FreeSpace = Arc::new(|| Some(100 * 1024 * 1024 * 1024));
    let auditor = askbar_auditor::Auditor::new(recorder, askbar_auditor::Limits::default(), free);
    let restored_url = common::serve(askbar_auditor::router(auditor, askbar_auditor::links(&tokens))).await;
    let restored = AuditorLink::new(&restored_url, read_token(&rig));
    a.chain.check(&a, &restored, true).await;
    assert_eq!(a.chain.state(), State::Rollback { stored: 6, current: 2 });
    assert_eq!(stored(&a).unwrap().0, 6, "the head the admin kept is not replaced by the older one");

    // The real auditor again: nothing was lost there, so the finding clears.
    a.chain.check(&a, &link, true).await;
    assert_eq!(a.chain.state(), State::Ok);
}

#[tokio::test]
async fn a_finding_is_one_alert_however_many_checks_see_it_and_a_new_one_after_it_clears() {
    let (url, s) = stub().await;
    let a = admin(&url).await;
    let link = a.auditor.get().unwrap().clone();
    let alerts = || a.notifier.notes().iter().filter(|n| n.kind == "chain_mismatch").count();
    s.grow_to(10);
    a.chain.check(&a, &link, false).await;
    assert_eq!(alerts(), 0, "a chain that only grows is not an alert");
    // A rollback, seen by three checks in a row: one alert.
    s.entries.lock().unwrap().retain(|n, _| *n <= 6);
    *s.head.lock().unwrap() = (6, "h6".into());
    for _ in 0..3 {
        a.chain.check(&a, &link, false).await;
    }
    assert_eq!(alerts(), 1);
    // It turns into a rewrite (the same fault, seen differently): still one.
    s.grow_to(12);
    s.entries.lock().unwrap().insert(10, "other".into());
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.state(), State::Rewritten { seq: 10 });
    assert_eq!(alerts(), 1, "one fault is one alert");
    // It clears, and a later one is a new alert.
    s.entries.lock().unwrap().insert(10, "h10".into());
    a.chain.check(&a, &link, false).await;
    assert_eq!(a.chain.state(), State::Ok);
    s.entries.lock().unwrap().retain(|n, _| *n <= 5);
    *s.head.lock().unwrap() = (5, "h5".into());
    a.chain.check(&a, &link, false).await;
    assert_eq!(alerts(), 2);
}
