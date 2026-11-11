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
async fn audit_state(mock_port: u16) -> (std::sync::Arc<askbar_server::AppState>, PathBuf) {
    let (state, dir, _rig) = askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{mock_port}/v1")).await;
    (state, dir)
}

/// Runs one chat to the end through the generation code and returns its generation id.
async fn run_chat(state: &std::sync::Arc<askbar_server::AppState>, prompt: &str) -> String {
    use futures_util::StreamExt;
    let (user, client) = {
        let db = state.db.lock().unwrap();
        let user = db.seed_server().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let client: String = db.conn().query_row("SELECT id FROM clients ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        (user, client)
    };
    let sub = askbar_server::generation::submit(
        state,
        askbar_server::generation::Submit {
            space: askbar_server::testing::personal_space(state, &user),
            user_id: user,
            client_id: client,
            conversation_id: None,
            provider_id: "p1".into(),
            model: "m".into(),
            prompt: prompt.into(),
            client_request_id: None,
        },
    )
    .expect("the generation starts");
    let _: Vec<_> = sub.live.subscribe(0).collect().await; // to the end
    sub.generation_id
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
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
    let mut child =
        Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
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
    let (state, dir) = audit_state(port).await;
    let trace = run_chat(&state, "hello").await;
    state.audit.flush().await;
    let db = read(&dir);
    let (n, status, user, client, error_code): (i64, String, Option<String>, Option<String>, Option<String>) = db
        .query_row(
            "SELECT COUNT(*), MAX(status), MAX(user_id), MAX(client_id), MAX(error_code) FROM spans WHERE trace_id = ?1 AND kind = 'turn'",
            [&trace],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!((n, status.as_str(), error_code), (1, "done", None));
    assert!(user.is_some() && client.is_some(), "who it was for");
    let (parent, ended): (Option<String>, Option<i64>) = db
        .query_row("SELECT parent_id, ended_at FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap();
    assert!(parent.is_none() && ended.is_some());
}

/// ADR 0009 mechanism 5: the audit record for each reply says which space it ran in.
#[tokio::test]
async fn the_turn_span_names_the_space_and_the_conversation() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let trace = run_chat(&state, "hello").await;
    state.audit.flush().await;
    let attrs: String =
        read(&dir).query_row("SELECT attrs_json FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| r.get(0)).unwrap();
    let attrs: serde_json::Value = serde_json::from_str(&attrs).unwrap();
    let user: String = state.db.lock().unwrap().conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
    let space = askbar_server::testing::personal_space(&state, &user);
    assert_eq!(attrs["space_id"], space.id.as_str());
    let conversation: String = space.lock().conn().query_row("SELECT id FROM conversations", [], |r| r.get(0)).unwrap();
    assert_eq!(attrs["conversation_id"], conversation.as_str());
}

#[tokio::test]
async fn a_failed_generation_ends_its_turn_span_with_an_error_code() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
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
    let (state, dir) = audit_state(port).await;
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
    let (state, dir) = audit_state(port).await;
    let trace = run_chat(&state, "please fail").await;
    state.audit.flush().await;
    let (status, code, attrs): (String, Option<String>, String) = read(&dir)
        .query_row("SELECT status, error_code, attrs_json FROM spans WHERE trace_id = ?1 AND kind = 'attempt'", [&trace], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((status.as_str(), code.as_deref()), ("error", Some("http_401")));
    assert!(attrs.contains("401"), "{attrs}");
}

#[tokio::test]
async fn audit_db_contains_none_of_the_prompt_or_reply_text() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
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

#[tokio::test]
async fn a_queued_message_records_a_gate_span_named_queue_from_enqueue_to_start() {
    use futures_util::StreamExt;
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let (user, client) = {
        let db = state.db.lock().unwrap();
        let user = db.seed_server().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let client: String = db.conn().query_row("SELECT id FROM clients", [], |r| r.get(0)).unwrap();
        (user, client)
    };
    let send = |conversation: Option<String>, prompt: &str| {
        askbar_server::generation::submit(
            &state,
            askbar_server::generation::Submit {
                space: askbar_server::testing::personal_space(&state, &user),
                user_id: user.clone(),
                client_id: client.clone(),
                conversation_id: conversation,
                provider_id: "p1".into(),
                model: "m".into(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
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
    let none: i64 = db
        .query_row("SELECT COUNT(*) FROM spans WHERE trace_id = ?1 AND kind = 'gate' AND name = 'queue'", [&first.generation_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(none, 0, "a message that did not wait has no queue span");
    // Its gates (the kill switch it passed, then the wait), turn and attempt, in the same trace.
    let names: Vec<(String, String)> = db
        .prepare("SELECT kind, name FROM spans WHERE trace_id = ?1 ORDER BY id")
        .unwrap()
        .query_map([&second.generation_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let want = |k: &str, n: &str| (k.to_string(), n.to_string());
    assert_eq!(names[..2], [want("gate", "kill_switch"), want("gate", "queue")]);
    assert_eq!(names[2].0, "turn");
    assert_eq!(names[3].0, "attempt");
}

// ------------------------------------------------------------------ sealed content (Phase 5)

/// Gives `state` the test audit key, as the admin's snapshot would, and returns the matching secret
/// key so a test can read what was sealed. (The test helpers already include it; this makes it
/// explicit and survives a snapshot that was changed.)
#[allow(dead_code)]
fn install_audit_key(state: &std::sync::Arc<askbar_server::AppState>) -> [u8; 32] {
    askbar_server::testing::update_snapshot(&state.db.lock().unwrap(), |s| {
        s.audit_public_key = Some(askbar_server::testing::test_audit_key())
    });
    askbar_server::testing::TEST_AUDIT_SECRET
}

#[tokio::test]
async fn sealed_content_is_stored_but_a_marker_never_appears_in_the_bytes_of_audit_db_or_its_wal() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let _secret = install_audit_key(&state);
    let trace = run_chat(&state, "SECRET-PROMPT-MARKER-5512 tell me something").await;
    state.audit.flush().await;
    let payloads: i64 = read(&dir)
        .query_row("SELECT COUNT(*) FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = ?1", [&trace], |r| r.get(0))
        .unwrap();
    assert!(payloads >= 2, "the request and the reply are stored, sealed ({payloads})");
    for name in ["audit.db", "audit.db-wal"] {
        let Ok(bytes) = std::fs::read(dir.join(name)) else { continue };
        let hay = String::from_utf8_lossy(&bytes);
        assert!(!hay.contains("SECRET-PROMPT-MARKER-5512"), "{name} holds the prompt in the clear");
        assert!(!hay.contains("Anything else?"), "{name} holds the reply in the clear");
    }
}

#[tokio::test]
async fn a_generation_with_no_audit_key_is_refused_with_503_audit_key_missing() {
    use axum::response::IntoResponse;
    let (_mock, port) = start_mock();
    let (state, _dir) = audit_state(port).await;
    askbar_server::testing::update_snapshot(&state.db.lock().unwrap(), |s| s.audit_public_key = None); // the admin has not made a key
    let (user, client) = {
        let db = state.db.lock().unwrap();
        (db.seed_server().unwrap(), "d1".to_string())
    };
    let refused = askbar_server::generation::submit(
        &state,
        askbar_server::generation::Submit {
            space: askbar_server::testing::personal_space(&state, &user),
            user_id: user,
            client_id: client,
            conversation_id: None,
            provider_id: "p1".into(),
            model: "m".into(),
            prompt: "hi".into(),
            client_request_id: None,
        },
    )
    .err()
    .expect("no key, no generation");
    let response = refused.into_response();
    assert_eq!(response.status(), 503);
    let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("audit_key_missing"), "{body:?}");

    // Once the key arrives, generations run again.
    install_audit_key(&state);
    let (user, client) = {
        let db = state.db.lock().unwrap();
        (db.seed_server().unwrap(), "d1".to_string())
    };
    assert!(askbar_server::generation::submit(
        &state,
        askbar_server::generation::Submit {
            space: askbar_server::testing::personal_space(&state, &user),
            user_id: user,
            client_id: client,
            conversation_id: None,
            provider_id: "p1".into(),
            model: "m".into(),
            prompt: "hi".into(),
            client_request_id: None
        },
    )
    .is_ok());
}

/// Every payload of a trace, opened with the test secret: (role, key_id, plaintext), by span kind.
fn open_payloads(dir: &Path, trace: &str, secret: [u8; 32]) -> Vec<(String, String, String, String)> {
    let key = crypto_box::SecretKey::from(secret);
    let db = read(dir);
    let mut stmt = db
        .prepare("SELECT s.kind, p.role, p.key_id, p.ciphertext FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = ?1 ORDER BY p.rowid")
        .unwrap();
    let rows = stmt
        .query_map([trace], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Vec<u8>>(3)?)))
        .unwrap()
        .map(|r| {
            let (kind, role, key_id, ct) = r.unwrap();
            (kind, role, key_id, String::from_utf8(key.unseal(&ct).expect("opens with the test key")).unwrap())
        })
        .collect();
    rows
}

#[tokio::test]
async fn the_request_and_the_reply_are_stored_sealed_on_their_spans_and_open_with_the_key() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let secret = install_audit_key(&state);
    let trace = run_chat(&state, "What is the capital of Atlantis?").await;
    state.audit.flush().await;

    let opened = open_payloads(&dir, &trace, secret);
    let key_id = askbar_server::testing::test_audit_key().key_id;
    assert!(opened.iter().all(|p| p.2 == key_id), "each payload names the key that sealed it");
    let request = opened.iter().find(|p| p.1 == "request").expect("a request payload");
    assert_eq!(request.0, "turn", "the request belongs to the turn the gateway opened: the server only knows that span");
    let request: serde_json::Value = serde_json::from_str(&request.3).unwrap();
    assert!(request["system"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(request["messages"][0]["content"], "What is the capital of Atlantis?");
    let reply = opened.iter().find(|p| p.1 == "reply").expect("a reply payload");
    assert_eq!(reply.0, "turn", "the reply belongs to the turn");
    assert!(reply.3.ends_with("Anything else?"), "{:?}", reply.3);
    assert_eq!(opened.iter().filter(|p| p.1 == "error").count(), 0);
}

#[tokio::test]
async fn a_provider_failure_seals_its_error_text_on_the_attempt() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let secret = install_audit_key(&state);
    let trace = run_chat(&state, "please fail").await;
    state.audit.flush().await;
    let opened = open_payloads(&dir, &trace, secret);
    let err = opened.iter().find(|p| p.1 == "error").expect("an error payload");
    assert_eq!(err.0, "attempt");
    assert!(err.3.contains("Incorrect API key"), "{:?}", err.3);
    assert!(opened.iter().all(|p| p.1 != "reply"), "no reply text, no reply payload");
    let bytes = std::fs::read(dir.join("audit.db")).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Incorrect API key"));
}

#[tokio::test]
async fn an_oversized_prompt_is_cut_at_256_kb_and_the_turn_is_marked_request_truncated() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let secret = install_audit_key(&state);
    let prompt = format!("{}{}", "word ".repeat(70_000), "END-OF-PROMPT"); // 350 KB
    let trace = run_chat(&state, &prompt).await;
    state.audit.flush().await;

    let opened = open_payloads(&dir, &trace, secret);
    let request = opened.iter().find(|p| p.1 == "request").unwrap();
    assert_eq!(request.3.len(), 256 * 1024, "cut to the cap");
    assert!(!request.3.contains("END-OF-PROMPT"), "the end was cut off");
    let attrs = |kind: &str| -> serde_json::Value {
        let a: Option<String> =
            read(&dir).query_row("SELECT attrs_json FROM spans WHERE trace_id = ?1 AND kind = ?2", [&trace, kind], |r| r.get(0)).unwrap();
        serde_json::from_str(&a.unwrap()).unwrap()
    };
    assert_eq!(attrs("turn")["request_truncated"], true);
    assert_eq!(attrs("attempt")["http_status"], 200, "the attempt's own attributes (the gateway's) are still there");
    assert!(attrs("turn").get("truncated").is_none(), "the reply was small");

    // A normal prompt is not marked.
    let small = run_chat(&state, "short").await;
    state.audit.flush().await;
    let a: Option<String> =
        read(&dir).query_row("SELECT attrs_json FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&small], |r| r.get(0)).unwrap();
    assert!(!a.unwrap().contains("truncated"));
}

#[tokio::test]
async fn each_try_is_an_attempt_span_with_its_retry_index() {
    let (_mock, port) = start_mock();
    let (state, dir) = audit_state(port).await;
    let trace = run_chat(&state, "hello 429once audit-retry").await;
    state.audit.flush().await;
    let db = read(&dir);
    let mut stmt = db
        .prepare("SELECT status, error_code, json_extract(attrs_json, '$.retry_index'), json_extract(attrs_json, '$.http_status') FROM spans WHERE trace_id = ?1 AND kind = 'attempt' ORDER BY id")
        .unwrap();
    let rows: Vec<(String, Option<String>, i64, i64)> =
        stmt.query_map([&trace], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(
        rows,
        [("error".to_string(), Some("http_429".to_string()), 0, 429), ("done".to_string(), None, 1, 200)],
        "the refused try, then the one that served it"
    );
    let turn: String = db.query_row("SELECT status FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| r.get(0)).unwrap();
    assert_eq!(turn, "done");
}
