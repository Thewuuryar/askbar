//! The router: one place decides which provider serves a generation, with retries, fallbacks,
//! a kill switch and a budget gate. Written before the code (step 6.1); each test names the step
//! that enables it. Failures are driven by the mock LLM's modes, chosen here by model name
//! ("m-429", "m-fail", "m-reset"), so two providers on one mock can behave differently.

use askbar_server::generation::{submit, GenEvent, Submit, SubmitError};
use axum::response::IntoResponse;
use futures_util::StreamExt;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

struct Mock {
    child: Child,
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Mock {
    fn start() -> Mock {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
        let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
        let log = Arc::new(Mutex::new(Vec::new()));
        let out = BufReader::new(child.stdout.take().unwrap());
        let l = log.clone();
        std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                l.lock().unwrap().push(line);
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
            assert!(Instant::now() < deadline, "mock did not start");
            std::thread::sleep(Duration::from_millis(50));
        }
        Mock { child, port, log }
    }
    /// How many chat requests this mock has seen.
    fn requests(&self) -> usize {
        self.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).count()
    }
}

/// Two providers on two mocks: `p1` (requested first) and `p2` (the fallback). `model1` and
/// `model2` pick each one's mock mode.
struct Stack {
    state: Arc<askbar_server::AppState>,
    m1: Mock,
    m2: Mock,
    user: String,
    _dir: std::path::PathBuf,
}

fn provider(id: &str, port: u16, model: &str, extra: serde_json::Value) -> askbar_proto::SnapshotProvider {
    let mut v = serde_json::json!({
        "id": id, "name": id, "type": "openai", "base_url": format!("http://127.0.0.1:{port}/v1"), "model": model, "headers": {}
    });
    v.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
    serde_json::from_value(v).unwrap()
}

async fn stack(model1: &str, model2: &str, p2_extra: serde_json::Value) -> Stack {
    let (m1, m2) = (Mock::start(), Mock::start());
    let (state, dir) = askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{}/v1", m1.port));
    let user = {
        let db = state.db.lock().unwrap();
        let secrets = state.secrets();
        askbar_server::testing::add_provider(&db, provider("p1", m1.port, model1, serde_json::json!({})));
        askbar_server::testing::add_provider(&db, provider("p2", m2.port, model2, p2_extra));
        askbar_server::testing::store_key(&db, &secrets, "p1", "sk-1");
        askbar_server::testing::store_key(&db, &secrets, "p2", "sk-2");
        askbar_server::testing::update_snapshot(&db, |s| s.fallback_order = vec!["p2".into()]);
        db.seed_server().unwrap()
    };
    Stack { state, m1, m2, user, _dir: dir }
}

impl Stack {
    fn submit(&self, prompt: &str) -> Result<askbar_server::generation::Submitted, SubmitError> {
        submit(
            &self.state,
            Submit {
                user_id: self.user.clone(),
                device_id: "d1".into(),
                conversation_id: None,
                provider_id: "p1".into(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
    }

    /// Sends and waits for the end; returns the events.
    async fn run(&self, prompt: &str) -> Vec<GenEvent> {
        let sub = self.submit(prompt).ok().expect("accepted");
        sub.live.subscribe(0).collect().await
    }

    /// (status, provider_id, text) of the generation rows, oldest first.
    fn rows(&self) -> Vec<(String, Option<String>, String)> {
        let db = self.state.db.lock().unwrap();
        let mut stmt = db.conn().prepare("SELECT status, provider_id, text FROM generations ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
        rows
    }
}

fn is_done(events: &[GenEvent]) -> bool {
    matches!(events.last(), Some(GenEvent::Done))
}

#[tokio::test]
async fn a_429_is_retried_and_the_reply_then_succeeds_on_the_same_provider() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let events = s.run("hello 429once retry-A").await;
    assert!(is_done(&events), "{events:?}");
    assert_eq!(s.m1.requests(), 2, "refused once, then served");
    assert_eq!(s.m2.requests(), 0, "no fallback was needed");
    assert_eq!(s.rows()[0].1.as_deref(), Some("p1"));
}

#[tokio::test]
async fn the_wait_before_a_retry_is_the_providers_retry_after_not_the_default_backoff() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let started = Instant::now();
    let events = s.run("hello 429once retry-B").await;
    let took = started.elapsed();
    assert!(is_done(&events));
    assert!(took >= Duration::from_millis(950), "waited the mock's 1 second Retry-After, took {took:?}");
    assert!(took < Duration::from_millis(2500), "and did not wait longer: {took:?}");
}

#[tokio::test]
async fn when_retries_run_out_the_next_provider_serves_it() {
    let s = stack("m-500", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    let events = s.run("please answer").await;
    assert!(is_done(&events), "{events:?}");
    assert_eq!(s.m1.requests(), 3, "the first try and two retries");
    assert_eq!(s.m2.requests(), 1);
    assert_eq!(s.rows()[0].1.as_deref(), Some("p2"), "the row records the provider that finally served it");
}

#[tokio::test]
async fn once_text_has_been_sent_the_router_never_switches_provider() {
    let s = stack("m-reset", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    let events = s.run("tell me something").await;
    assert!(matches!(events.last(), Some(GenEvent::Error { .. })), "the failure ends the generation: {events:?}");
    assert_eq!(s.m2.requests(), 0, "no second provider after a delta was sent");
    assert_eq!(s.m1.requests(), 1, "and no retry either");
    let rows = s.rows();
    assert_eq!((rows[0].0.as_str(), rows[0].2.as_str()), ("error", "Reset midway"), "what arrived is kept");
}

#[tokio::test]
#[ignore = "enabled in 6.7"]
async fn a_401_falls_back_and_flags_the_provider_as_having_bad_credentials() {
    let s = stack("m-fail", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    let events = s.run("hello there").await;
    assert!(is_done(&events), "{events:?}");
    assert_eq!(s.m1.requests(), 1, "a bad key is not retried");
    assert_eq!(s.m2.requests(), 1);
    let flag: String = s.state.db.lock().unwrap().conn().query_row("SELECT status FROM provider_health WHERE provider_id = 'p1'", [], |r| r.get(0)).unwrap();
    assert_eq!(flag, "auth_failed");
}

#[tokio::test]
async fn a_metered_provider_is_skipped_as_a_fallback_unless_it_allows_it() {
    // p2 is metered (the default) and has not said it may be a fallback.
    let s = stack("m-500", "m", serde_json::json!({})).await;
    let events = s.run("please answer").await;
    assert!(matches!(events.last(), Some(GenEvent::Error { .. })), "{events:?}");
    assert_eq!(s.m2.requests(), 0, "money is not spent behind the administrator's back");

    // With allow_as_fallback it is used; a subscription provider needs no such permission.
    let s = stack("m-500", "m", serde_json::json!({ "billing": "subscription" })).await;
    assert!(is_done(&s.run("please answer").await));
    assert_eq!(s.m2.requests(), 1);
    let s = stack("m-500", "m", serde_json::json!({ "billing": "metered", "allow_as_fallback": true })).await;
    assert!(is_done(&s.run("please answer").await));
    assert_eq!(s.m2.requests(), 1);
}

#[tokio::test]
#[ignore = "enabled in 6.10"]
async fn the_kill_switch_refuses_new_generations_with_503() {
    let s = stack("m", "m", serde_json::json!({})).await;
    assert!(is_done(&s.run("works before").await));
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.kill_switch = true);
    let refused = s.submit("and now").err().expect("refused");
    let response = refused.into_response();
    assert_eq!(response.status(), 503);
    let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("kill_switch"), "{body:?}");
    assert_eq!(s.m1.requests(), 1, "nothing more was sent upstream");
}

#[tokio::test]
#[ignore = "enabled in 6.12"]
async fn an_exhausted_token_budget_refuses_new_generations_with_429() {
    let s = stack("m", "m", serde_json::json!({})).await;
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.budget.daily_tokens = Some(100));
    assert!(is_done(&s.run("first").await), "under the limit it runs"); // the mock reports 150 tokens
    let refused = s.submit("second").err().expect("the 150 tokens used are over the 100 allowed");
    let response = refused.into_response();
    assert_eq!(response.status(), 429);
    let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains("budget_exceeded"), "{body:?}");
}

#[tokio::test]
async fn a_stop_while_waiting_to_retry_ends_the_generation_at_once() {
    let s = stack("m-429", "m", serde_json::json!({})).await;
    let sub = s.submit("hello").ok().expect("accepted");
    tokio::time::sleep(Duration::from_millis(300)).await; // the first try was refused; it is now waiting
    let asked = Instant::now();
    assert!(s.state.generations.get(&sub.generation_id).unwrap().cancel());
    let events: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    assert!(asked.elapsed() < Duration::from_millis(500), "did not wait out the retry: {:?}", asked.elapsed());
    assert!(is_done(&events), "a stop ends cleanly: {events:?}");
    assert_eq!(s.rows()[0].0, "cancelled");
    assert_eq!(s.m1.requests(), 1, "no further try was made");
}

#[tokio::test]
async fn the_audit_trail_shows_every_try_and_which_provider_the_new_one_stood_in_for() {
    let s = stack("m-500", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    let sub = s.submit("please answer").ok().expect("accepted");
    let trace = sub.generation_id.clone();
    let _: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    s.state.audit.flush().await;
    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let mut stmt = db
        .prepare("SELECT provider_id, status, json_extract(attrs_json, '$.retry_index'), json_extract(attrs_json, '$.fallback_from') FROM spans WHERE trace_id = ?1 AND kind = 'attempt' ORDER BY id")
        .unwrap();
    let rows: Vec<(String, String, i64, Option<String>)> =
        stmt.query_map([&trace], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
    let want = |p: &str, st: &str, i: i64, from: Option<&str>| (p.to_string(), st.to_string(), i, from.map(str::to_string));
    assert_eq!(
        rows,
        [
            want("p1", "error", 0, None),
            want("p1", "error", 1, None),
            want("p1", "error", 2, None),
            want("p2", "done", 0, Some("p1")),
        ]
    );
}
