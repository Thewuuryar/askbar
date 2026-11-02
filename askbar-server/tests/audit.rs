//! The audit store: every generation leaves a trace of spans in a separate `audit.db`, holding
//! metadata only (no prompt or reply text until Phase 5 seals it).
//!
//! These tests were written before the code (step 4.1) and are `#[ignore]`d until the step that
//! makes each pass. The helpers at the top are filled in by step 4.5, which adds the audit
//! recorder to `AppState`.

use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// A server state whose audit recorder writes `audit.db` in a fresh temp directory, with a
/// provider pointing at the mock on `mock_port`.
fn audit_state(mock_port: u16) -> (std::sync::Arc<askbar_server::AppState>, PathBuf) {
    askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{mock_port}/v1"))
}

/// Runs one chat to the end through the generation code and returns its generation id.
async fn run_chat(state: &std::sync::Arc<askbar_server::AppState>, prompt: &str) -> String {
    use futures_util::StreamExt;
    let (user, device) = {
        let db = state.db.lock().unwrap();
        let user = db.seed_server().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let device: String = db.conn().query_row("SELECT id FROM devices ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        (user, device)
    };
    let sub = askbar_server::generation::submit(
        state,
        askbar_server::generation::Submit {
            user_id: user,
            device_id: device,
            conversation_id: None,
            provider_id: "p1".into(),
            prompt: prompt.into(),
            client_request_id: None,
        },
    )
    .ok()
    .expect("the generation starts");
    let _: Vec<_> = sub.live.subscribe(0).collect().await; // to the end
    sub.generation_id
}

/// Opens `audit.db` in `dir` the way the server does at start-up: migrations, then the sweep of
/// spans left `running`.
fn start_up_audit(dir: &Path) {
    askbar_server::audit::AuditDb::open(&dir.join("audit.db")).unwrap().sweep_running().unwrap();
}

fn read(dir: &Path) -> Connection {
    Connection::open(dir.join("audit.db")).expect("audit.db exists")
}

/// Kills the mock when dropped, so a failing test cannot leave it running.
struct Mock(std::process::Child);
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_mock() -> (Mock, u16) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
    let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    // Keep reading: the mock logs every request, and a closed pipe would break it.
    std::thread::spawn(move || for _ in out.lines() {});
    (Mock(child), port)
}

#[tokio::test]
async fn a_finished_generation_leaves_one_turn_span_with_its_final_status() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    let trace = run_chat(&state, "hello").await;
    state.audit.flush().await;
    let db = read(&dir);
    let (n, status, user, device, error_code): (i64, String, Option<String>, Option<String>, Option<String>) = db
        .query_row(
            "SELECT COUNT(*), MAX(status), MAX(user_id), MAX(device_id), MAX(error_code) FROM spans WHERE trace_id = ?1 AND kind = 'turn'",
            [&trace],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!((n, status.as_str(), error_code), (1, "done", None));
    assert!(user.is_some() && device.is_some(), "who it was for");
    let (parent, ended): (Option<String>, Option<i64>) =
        db.query_row("SELECT parent_id, ended_at FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert!(parent.is_none() && ended.is_some());
}

#[tokio::test]
async fn a_failed_generation_ends_its_turn_span_with_an_error_code() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    let trace = run_chat(&state, "please fail").await;
    state.audit.flush().await;
    let (status, code): (String, Option<String>) = read(&dir)
        .query_row("SELECT status, error_code FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert_eq!((status.as_str(), code.as_deref()), ("error", Some("http_401")));
}

#[tokio::test]
async fn a_finished_generation_leaves_one_attempt_span_under_its_turn() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    let trace = run_chat(&state, "hello").await;
    state.audit.flush().await;
    let db = read(&dir);
    let n: i64 = db.query_row("SELECT COUNT(*) FROM spans WHERE trace_id = ?1 AND kind = 'attempt'", [&trace], |r| r.get(0)).unwrap();
    assert_eq!(n, 1);
    let (parent, turn): (String, String) = db
        .query_row(
            "SELECT a.parent_id, t.id FROM spans a JOIN spans t ON t.trace_id = a.trace_id AND t.kind = 'turn' WHERE a.trace_id = ?1 AND a.kind = 'attempt'",
            [&trace],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(parent, turn);

    // What the attempt records: provider, model, timings, usage from Phase 3, no error.
    #[allow(clippy::type_complexity)]
    let (status, provider, model, ttft, tin, tout, code, attrs): (String, String, String, Option<i64>, Option<i64>, Option<i64>, Option<String>, String) = db
        .query_row(
            "SELECT status, provider_id, model, ttft_ms, tokens_in, tokens_out, error_code, attrs_json FROM spans WHERE trace_id = ?1 AND kind = 'attempt'",
            [&trace],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?)),
        )
        .unwrap();
    assert_eq!((status.as_str(), provider.as_str(), model.as_str()), ("done", "p1", "m"));
    assert!(ttft.is_some(), "time to first token");
    assert_eq!((tin, tout, code), (Some(100), Some(50), None), "the mock's usage chunk");
    let attrs: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    assert!(attrs["latency_ms"].as_u64().is_some() && attrs["http_status"] == 200, "{attrs}");
}

#[tokio::test]
async fn a_failed_attempt_records_its_error_code_and_status() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    let trace = run_chat(&state, "please fail").await;
    state.audit.flush().await;
    let (status, code, attrs): (String, Option<String>, String) = read(&dir)
        .query_row("SELECT status, error_code, attrs_json FROM spans WHERE trace_id = ?1 AND kind = 'attempt'", [&trace], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((status.as_str(), code.as_deref()), ("error", Some("http_401")));
    assert!(attrs.contains("401"), "{attrs}");
}

#[tokio::test]
async fn audit_db_contains_none_of_the_prompt_or_reply_text() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    run_chat(&state, "SECRET-PROMPT-MARKER-7731 please").await;
    state.audit.flush().await;
    // The database file and its write-ahead log, byte for byte.
    for name in ["audit.db", "audit.db-wal"] {
        let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
        let hay = String::from_utf8_lossy(&bytes);
        assert!(!hay.contains("SECRET-PROMPT-MARKER-7731"), "{name} holds the prompt");
        assert!(!hay.contains("Anything else?"), "{name} holds the reply (the mock's reply ends with this)");
        assert!(!hay.contains("quick answer with"), "{name} holds the reply");
    }
}

#[test]
fn a_span_still_running_at_startup_becomes_interrupted() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-sweep-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    start_up_audit(&dir); // creates the schema
    {
        let db = read(&dir);
        db.execute(
            "INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('s1', 't1', 'turn', 'chat', 'running', 1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at) VALUES ('s2', 't1', 'attempt', 'x', 'done', 1, 2)",
            [],
        )
        .unwrap();
    }
    start_up_audit(&dir); // a restart
    let db = read(&dir);
    let status = |id: &str| -> String { db.query_row("SELECT status FROM spans WHERE id = ?1", [id], |r| r.get(0)).unwrap() };
    assert_eq!((status("s1").as_str(), status("s2").as_str()), ("interrupted", "done"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn deleting_a_span_younger_than_seven_days_fails() {
    let dir = std::env::temp_dir().join(format!("askbar-audit-del-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    start_up_audit(&dir);
    let db = read(&dir);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    db.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at) VALUES ('new', 't', 'turn', 'chat', 'done', ?1, ?1)", [now]).unwrap();
    db.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at) VALUES ('old', 't', 'turn', 'chat', 'done', ?1, ?1)", [now - 8 * 86400]).unwrap();
    let e = db.execute("DELETE FROM spans WHERE id = 'new'", []).unwrap_err();
    assert!(e.to_string().contains("7 days"), "{e}");
    assert_eq!(db.execute("DELETE FROM spans WHERE id = 'old'", []).unwrap(), 1, "older than a week may go");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_queued_message_records_a_gate_span_named_queue_from_enqueue_to_start() {
    use futures_util::StreamExt;
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    let (user, device) = {
        let db = state.db.lock().unwrap();
        let user = db.seed_server().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let device: String = db.conn().query_row("SELECT id FROM devices", [], |r| r.get(0)).unwrap();
        (user, device)
    };
    let send = |conversation: Option<String>, prompt: &str| {
        askbar_server::generation::submit(
            &state,
            askbar_server::generation::Submit {
                user_id: user.clone(),
                device_id: device.clone(),
                conversation_id: conversation,
                provider_id: "p1".into(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
        .ok()
        .expect("accepted")
    };
    let first = send(None, "please be slow");
    let second = send(Some(first.conversation_id.clone()), "second");
    assert!(second.queued && !first.queued);
    // The first has to be going before it can be stopped; the wait is real time.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    state.generations.get(&first.generation_id).unwrap().cancel();
    let _: Vec<_> = second.live.subscribe(0).collect().await; // the queued one runs to its end
    state.audit.flush().await;

    let db = read(&dir);
    let (n, status, started, ended): (i64, String, i64, Option<i64>) = db
        .query_row(
            "SELECT COUNT(*), MAX(status), MAX(started_at), MAX(ended_at) FROM spans WHERE trace_id = ?1 AND kind = 'gate' AND name = 'queue'",
            [&second.generation_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!((n, status.as_str()), (1, "done"));
    assert!(ended.unwrap() >= started, "it ends after it begins");
    let none: i64 = db.query_row("SELECT COUNT(*) FROM spans WHERE trace_id = ?1 AND kind = 'gate'", [&first.generation_id], |r| r.get(0)).unwrap();
    assert_eq!(none, 0, "a message that did not wait has no queue span");
    // Its turn and attempt come after the wait, in the same trace.
    let kinds: Vec<String> = db
        .prepare("SELECT kind FROM spans WHERE trace_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([&second.generation_id], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(kinds, ["gate", "turn", "attempt"]);
}

#[tokio::test]
async fn a_backed_up_audit_writer_refuses_new_generations_with_503_and_retry_after_but_lets_running_ones_finish() {
    use askbar_server::audit::{AuditDb, Recorder};
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    let (_mock, port) = start_mock();
    let dir = std::env::temp_dir().join(format!("askbar-audit-backlog-{}", ulid::Ulid::new()));
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("audit.db")).unwrap(), 2); // a tiny queue
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let secrets = askbar_server::secrets::Secrets::new(askbar_server::testing::TEST_MASTER_KEY).unwrap();
    askbar_server::testing::add_provider(&db, askbar_server::testing::provider("p1", "mock", &format!("http://127.0.0.1:{port}/v1"), "m"));
    askbar_server::testing::store_key(&db, &secrets, "p1", "sk-test");
    let state = askbar_server::AppState::new(db, secrets, askbar_server::MIN_CLIENT).with_audit(recorder.clone());
    let send = |prompt: &str| {
        askbar_server::generation::submit(
            &state,
            askbar_server::generation::Submit {
                user_id: user.clone(),
                device_id: "d1".into(),
                conversation_id: None,
                provider_id: "p1".into(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
    };

    assert!(recorder.healthy());
    recorder.pause_writer(true); // the writer stalls
    let running = send("hello").ok().expect("accepted while the queue has room");
    for _ in 0..50 {
        if !recorder.healthy() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(!recorder.healthy(), "the stalled writer let the queue fill");

    let refused = send("one more").err().expect("refused");
    assert_eq!(refused, askbar_server::generation::SubmitError::AuditBacklog);
    let response = refused.into_response();
    assert_eq!(response.status(), 503);
    assert_eq!(response.headers()["retry-after"], askbar_server::generation::AUDIT_BACKLOG_RETRY_SECS.to_string());

    // The one that was running finishes normally, whatever the audit store is doing.
    let events: Vec<_> = running.live.subscribe(0).collect().await;
    assert!(matches!(events.last(), Some(askbar_server::generation::GenEvent::Done)));
    let status: String = state.db.lock().unwrap().conn().query_row("SELECT status FROM generations", [], |r| r.get(0)).unwrap();
    assert_eq!(status, "done");

    // Once the writer catches up, new generations are accepted again.
    recorder.pause_writer(false);
    recorder.flush().await;
    assert!(recorder.healthy());
    assert!(send("and now").is_ok());
    let _ = std::fs::remove_dir_all(dir);
}
