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

#[tokio::test]
async fn the_flag_clears_on_the_next_success_and_when_a_new_key_is_saved_and_the_endpoint_lists_it() {
    use askbar_server::mgmt;
    let s = stack("m-fail", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    assert!(is_done(&s.run("hello there").await));

    // The management endpoint lists it, with a code and no provider text.
    let dir = s._dir.clone();
    let hash = dir.join("token.sha256");
    let token = "mgt_health-test-token";
    use sha2::Digest;
    let hex: String = sha2::Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&hash, hex).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = mgmt::router(s.state.clone(), hash);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let get = || async { http.get(format!("{url}/mgmt/v1/providers/health")).bearer_auth(token).send().await.unwrap().json::<serde_json::Value>().await.unwrap() };
    let v = get().await;
    assert_eq!(v["providers"].as_array().unwrap().len(), 1);
    assert_eq!((v["providers"][0]["provider_id"].as_str(), v["providers"][0]["status"].as_str(), v["providers"][0]["error_code"].as_str()), (Some("p1"), Some("auth_failed"), Some("http_401")));
    assert!(!v.to_string().contains("Incorrect API key"), "the provider's own words are not kept: {v}");
    let r = http.get(format!("{url}/mgmt/v1/providers/health")).send().await.unwrap();
    assert_eq!(r.status(), 401, "needs the management token");

    // Saving a new key for p1 clears the flag.
    let r = http.put(format!("{url}/mgmt/v1/providers/p1/key")).bearer_auth(token).json(&serde_json::json!({ "key": "sk-new" })).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert!(get().await["providers"].as_array().unwrap().is_empty());

    // A flagged provider that then works is cleared by the success. (p1 is now fine: point it at a good model.)
    s.state.db.lock().unwrap().flag_provider_auth_failed("p2", "http_403").unwrap();
    assert!(is_done(&s.run("another one").await)); // p1 still fails (m-fail) and p2 serves it
    let flagged: Vec<String> = s.state.db.lock().unwrap().provider_health().unwrap().into_iter().map(|h| h.0).collect();
    assert_eq!(flagged, ["p1"], "p2 worked, so its flag is gone; p1 failed again and is flagged again");
}

#[tokio::test]
async fn the_generation_row_and_its_token_usage_name_the_provider_that_finally_served_it() {
    let s = stack("m-500", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    assert!(is_done(&s.run("please answer").await));
    assert_eq!(s.rows()[0].1.as_deref(), Some("p2"));
    let usage_provider: String = s.state.db.lock().unwrap().conn().query_row("SELECT provider_id FROM token_usage", [], |r| r.get(0)).unwrap();
    assert_eq!(usage_provider, "p2", "cost follows the provider that did the work, not the one that was asked");
    let n: i64 = s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM token_usage", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "the failed tries used nothing and recorded nothing");
}

#[tokio::test]
async fn the_provider_is_announced_before_the_first_delta_and_again_after_a_fallback() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let events = s.run("hello").await;
    assert_eq!(events[0], GenEvent::Provider { id: "p1".into(), name: "p1".into() }, "first of all");
    assert!(matches!(events[1], GenEvent::Delta { seq: 1, .. }), "then the text, counted from 1: {:?}", &events[..3]);
    assert_eq!(events.iter().filter(|e| matches!(e, GenEvent::Provider { .. })).count(), 1);

    // After a fallback both are announced, in order, still before any text.
    let s = stack("m-500", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    let events = s.run("please answer").await;
    assert_eq!(
        events[..3],
        [
            GenEvent::Provider { id: "p1".into(), name: "p1".into() },
            GenEvent::Provider { id: "p2".into(), name: "p2".into() },
            events[2].clone()
        ]
    );
    assert!(matches!(events[2], GenEvent::Delta { seq: 1, .. }));

    // No change of provider is ever announced once text has gone out.
    let s = stack("m-reset", "m", serde_json::json!({ "allow_as_fallback": true })).await;
    let events = s.run("tell me something").await;
    assert_eq!(events.iter().filter(|e| matches!(e, GenEvent::Provider { .. })).count(), 1);
}

#[tokio::test]
async fn a_subscriber_who_already_has_text_is_not_sent_the_announcements_again() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let sub = s.submit("hello").ok().expect("accepted");
    let all: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    let rest: Vec<GenEvent> = sub.live.subscribe(3).collect().await;
    assert!(rest.iter().all(|e| !matches!(e, GenEvent::Provider { .. })), "{rest:?}");
    assert_eq!(rest.len(), all.len() - 1 - 3, "everything after delta 3, without the announcement");
}

#[tokio::test]
async fn the_kill_switch_gate_leaves_a_span_whether_it_lets_a_generation_through_or_not() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let sub = s.submit("works").ok().expect("accepted");
    let trace = sub.generation_id.clone();
    let _: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.kill_switch = true);
    assert_eq!(s.submit("blocked").err(), Some(SubmitError::KillSwitch));
    s.state.audit.flush().await;

    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let gates: Vec<(String, String, Option<String>, String)> = db
        .prepare("SELECT trace_id, status, error_code, attrs_json FROM spans WHERE kind = 'gate' AND name = 'kill_switch' ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(gates.len(), 2);
    assert_eq!((gates[0].0.as_str(), gates[0].1.as_str(), gates[0].2.as_deref()), (trace.as_str(), "done", None), "passed, inside the generation's own trace");
    assert!(gates[0].3.contains(r#""kill_switch":false"#));
    assert_eq!((gates[1].1.as_str(), gates[1].2.as_deref()), ("blocked", Some("kill_switch")));
    assert_ne!(gates[1].0, trace, "a refusal has no generation, so a trace of its own");
    assert!(gates[1].3.contains(r#""kill_switch":true"#), "the snapshot flag is recorded");
    // Turning it off again lets the next one through.
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.kill_switch = false);
    assert!(is_done(&s.run("works again").await));
}

/// The management API for `s`, with a token, so a test can push a snapshot the way the admin does.
async fn mgmt(s: &Stack) -> (String, String) {
    use sha2::Digest;
    let token = "mgt_router-test-token".to_string();
    let hash = s._dir.join("token.sha256");
    std::fs::write(&hash, sha2::Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::mgmt::router(s.state.clone(), hash);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, token)
}

/// Pushes the current snapshot back with the kill switch on and `aborts` as given.
async fn switch_off(s: &Stack, aborts: bool) {
    let (url, token) = mgmt(s).await;
    let mut snap = askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap();
    snap.kill_switch = true;
    snap.kill_switch_aborts_running = aborts;
    let r = reqwest::Client::new().put(format!("{url}/mgmt/v1/snapshot")).bearer_auth(token).json(&snap).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn turning_the_kill_switch_on_with_abort_stops_running_replies_and_clears_the_queue() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let first = s.submit("please be slow").ok().expect("accepted");
    let queued = askbar_server::generation::submit(
        &s.state,
        Submit { user_id: s.user.clone(), device_id: "d1".into(), conversation_id: Some(first.conversation_id.clone()), provider_id: "p1".into(), prompt: "waiting".into(), client_request_id: None },
    )
    .ok()
    .expect("queued");
    assert!(queued.queued);
    tokio::time::sleep(Duration::from_millis(1200)).await; // it has produced some text

    switch_off(&s, true).await;
    let a: Vec<GenEvent> = first.live.subscribe(0).collect().await;
    let b: Vec<GenEvent> = queued.live.subscribe(0).collect().await;
    assert!(matches!(a.last(), Some(GenEvent::Error { message }) if message.contains("switched off")), "{:?}", a.last());
    assert!(matches!(b.last(), Some(GenEvent::Error { .. })) && !b.iter().any(|e| matches!(e, GenEvent::Delta { .. })), "the waiting one never ran: {b:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;

    let rows = s.rows();
    assert_eq!((rows[0].0.as_str(), rows[1].0.as_str()), ("cancelled", "cancelled"));
    assert!(rows[0].2.starts_with("Streaming slowly"), "the text so far is kept: {:?}", rows[0].2);
    let (messages, user_msgs): (i64, i64) = {
        let db = s.state.db.lock().unwrap();
        (
            db.conn().query_row("SELECT COUNT(*) FROM messages WHERE role = 'assistant'", [], |r| r.get(0)).unwrap(),
            db.conn().query_row("SELECT COUNT(*) FROM messages WHERE role = 'user'", [], |r| r.get(0)).unwrap(),
        )
    };
    assert_eq!((messages, user_msgs), (1, 1), "the partial reply is saved; the queued question never was");
    assert_eq!(s.m1.requests(), 1, "nothing further went upstream");
    s.state.audit.flush().await;
    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let code: Option<String> = db.query_row("SELECT error_code FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&first.generation_id], |r| r.get(0)).unwrap();
    let status: String = db.query_row("SELECT status FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&first.generation_id], |r| r.get(0)).unwrap();
    assert_eq!((status.as_str(), code.as_deref()), ("cancelled", Some("kill_switch")));
    assert!(matches!(s.submit("and now").err(), Some(SubmitError::KillSwitch)), "and nothing new starts");
}

#[tokio::test]
async fn turning_it_on_without_abort_lets_running_replies_finish_but_still_clears_the_queue() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let first = s.submit("please be slow").ok().expect("accepted");
    let queued = askbar_server::generation::submit(
        &s.state,
        Submit { user_id: s.user.clone(), device_id: "d1".into(), conversation_id: Some(first.conversation_id.clone()), provider_id: "p1".into(), prompt: "waiting".into(), client_request_id: None },
    )
    .ok()
    .expect("queued");
    tokio::time::sleep(Duration::from_millis(800)).await;

    switch_off(&s, false).await;
    let b: Vec<GenEvent> = queued.live.subscribe(0).collect().await;
    assert!(matches!(b.last(), Some(GenEvent::Error { .. })), "the queue is cleared either way: {b:?}");
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(s.rows()[0].0, "running", "the reply already running was left alone");
    assert!(!first.live.is_finished());
    assert!(matches!(s.submit("new one").err(), Some(SubmitError::KillSwitch)));
    // (It would finish on its own in a minute; end it here.)
    s.state.generations.get(&first.generation_id).unwrap().cancel();
    let _: Vec<GenEvent> = first.live.subscribe(0).collect().await;
    assert_eq!(s.rows()[0].0, "cancelled");
    assert_eq!(s.rows()[1].0, "cancelled", "the queued one stayed cleared and did not start when the first ended");
    assert_eq!(s.m1.requests(), 1);
}

#[tokio::test]
async fn the_monthly_limit_and_subscription_providers_are_handled_by_the_budget_gate() {
    // Monthly limit: 150 used of 100 allowed -> refused as "monthly" (the daily one is not set).
    let s = stack("m", "m", serde_json::json!({})).await;
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.budget.monthly_tokens = Some(100));
    assert!(is_done(&s.run("first").await));
    let refused = s.submit("second").err().expect("over the monthly limit");
    assert_eq!(refused, SubmitError::BudgetExceeded("monthly"));
    let body = axum::body::to_bytes(refused.into_response().into_body(), 4096).await.unwrap();
    assert!(String::from_utf8_lossy(&body).contains(r#""period":"monthly""#));
    // A limit above the use lets it through, and with no limit nothing is checked.
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.budget.monthly_tokens = Some(151));
    assert!(s.submit("under it").is_ok());

    // Tokens used by a subscription provider cost nothing, so they do not count toward the budget.
    let s = stack("m", "m", serde_json::json!({})).await;
    {
        let db = s.state.db.lock().unwrap();
        let sub = |snap: &mut askbar_proto::Snapshot| {
            snap.providers.iter_mut().find(|p| p.id == "p1").unwrap().billing = askbar_proto::Billing::Subscription;
            snap.budget.daily_tokens = Some(100);
        };
        askbar_server::testing::update_snapshot(&db, sub);
    }
    assert!(is_done(&s.run("on a subscription").await)); // 150 tokens, but free
    assert!(s.submit("again").is_ok(), "subscription usage does not exhaust a budget, and is never refused for it");
}

#[tokio::test]
async fn the_budget_gate_leaves_a_span_with_its_numbers() {
    let s = stack("m", "m", serde_json::json!({})).await;
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.budget.daily_tokens = Some(100));
    let sub = s.submit("first").ok().expect("under the limit");
    let trace = sub.generation_id.clone();
    let _: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    assert!(s.submit("second").is_err());
    s.state.audit.flush().await;
    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let rows: Vec<(String, String, Option<String>, String)> = db
        .prepare("SELECT trace_id, status, error_code, attrs_json FROM spans WHERE kind = 'gate' AND name = 'budget' ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!((rows[0].0.as_str(), rows[0].1.as_str()), (trace.as_str(), "done"));
    assert!(rows[0].3.contains(r#""daily_used":0"#) && rows[0].3.contains(r#""daily_limit":100"#), "{}", rows[0].3);
    assert_eq!((rows[1].1.as_str(), rows[1].2.as_deref()), ("blocked", Some("budget_exceeded")));
    assert!(rows[1].3.contains(r#""daily_used":150"#), "{}", rows[1].3);
}
