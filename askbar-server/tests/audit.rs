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

/// Opens `audit.db` in `dir` the way the server does at start-up (migrations, then the sweep of
/// spans left `running`, which step 4.11 adds).
fn start_up_audit(dir: &Path) {
    askbar_server::audit::AuditDb::open(&dir.join("audit.db")).unwrap();
}

fn read(dir: &Path) -> Connection {
    Connection::open(dir.join("audit.db")).expect("audit.db exists")
}

fn start_mock() -> (std::process::Child, u16) {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
    let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    (child, port)
}

#[tokio::test]
#[ignore = "enabled in 4.7 and 4.8"]
async fn a_finished_generation_leaves_one_turn_and_one_attempt_span() {
    let (mut mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    let trace = run_chat(&state, "hello").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await; // the writer is asynchronous
    let db = read(&dir);
    let count = |kind: &str| -> i64 {
        db.query_row("SELECT COUNT(*) FROM spans WHERE trace_id = ?1 AND kind = ?2", [&trace, kind], |r| r.get(0)).unwrap()
    };
    assert_eq!((count("turn"), count("attempt")), (1, 1));
    let status: String = db.query_row("SELECT status FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| r.get(0)).unwrap();
    assert_eq!(status, "done");
    let _ = mock.kill();
}

#[tokio::test]
#[ignore = "enabled in 4.8"]
async fn audit_db_contains_none_of_the_prompt_or_reply_text() {
    let (mut mock, port) = start_mock();
    let (state, dir) = audit_state(port);
    run_chat(&state, "SECRET-PROMPT-MARKER-7731 please").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    // The database file and its write-ahead log, byte for byte.
    for name in ["audit.db", "audit.db-wal"] {
        let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
        let hay = String::from_utf8_lossy(&bytes);
        assert!(!hay.contains("SECRET-PROMPT-MARKER-7731"), "{name} holds the prompt");
        assert!(!hay.contains("Anything else?"), "{name} holds the reply (the mock's reply ends with this)");
        assert!(!hay.contains("quick answer with"), "{name} holds the reply");
    }
    let _ = mock.kill();
}

#[test]
#[ignore = "enabled in 4.11"]
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
