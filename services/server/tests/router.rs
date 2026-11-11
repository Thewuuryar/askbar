//! The router: one place decides how a generation is served, with retries on the provider asked
//! for (never a fallback to another one, ADR 0014), a kill switch and a budget gate. Written before the code (step 6.1); each test names the step
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
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
        let mut child =
            Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
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

/// Two providers on two mocks: `p1` (the one requests ask for) and `p2`, another provider that
/// must never be used in its place. `model1` and `model2` pick each one's mock mode.
struct Stack {
    state: Arc<askbar_server::AppState>,
    m1: Mock,
    m2: Mock,
    user: String,
    /// The model each provider approves (the catalog has just that one): p1's, then p2's.
    models: [String; 2],
    _dir: std::path::PathBuf,
    /// The in-process gateway and auditor the state's replies go through (kept for `sync` when a test changes a provider).
    #[allow(dead_code)]
    rig: &'static askbar_server::testing::Rig,
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
    let (state, dir, rig) = askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{}/v1", m1.port)).await;
    let user = {
        let db = state.db.lock().unwrap();
        askbar_server::testing::add_provider(&db, provider("p1", m1.port, model1, serde_json::json!({})));
        askbar_server::testing::add_provider(&db, provider("p2", m2.port, model2, p2_extra));
        rig.set_key("p1", "sk-1");
        rig.set_key("p2", "sk-2");
        db.seed_server().unwrap()
    };
    // The gateway holds the keys and the providers: give it what was just stored (ADR 0054).
    rig.sync(&state);
    Stack { state, m1, m2, user, models: [model1.to_string(), model2.to_string()], _dir: dir, rig }
}

impl Stack {
    fn submit(&self, prompt: &str) -> Result<askbar_server::generation::Submitted, SubmitError> {
        self.submit_to("p1", prompt)
    }

    fn submit_to(&self, provider_id: &str, prompt: &str) -> Result<askbar_server::generation::Submitted, SubmitError> {
        submit(
            &self.state,
            Submit {
                space: askbar_server::testing::personal_space(&self.state, &self.user),
                user_id: self.user.clone(),
                client_id: "d1".into(),
                conversation_id: None,
                provider_id: provider_id.into(),
                model: self.models[usize::from(provider_id == "p2")].clone(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
    }

    /// Sends and waits for the end; returns the events.
    async fn run(&self, prompt: &str) -> Vec<GenEvent> {
        let sub = self.submit(prompt).expect("accepted");
        sub.live.subscribe(0).collect().await
    }

    /// (status, provider_id, text) of the generation rows, oldest first.
    fn rows(&self) -> Vec<(String, Option<String>, String)> {
        let space = askbar_server::testing::personal_space(&self.state, &self.user);
        let db = space.lock();
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
    assert_eq!(s.m2.requests(), 0, "the other provider is never asked");
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
async fn when_retries_run_out_the_reply_fails_and_no_other_provider_is_tried() {
    let s = stack("m-500", "m", serde_json::json!({})).await;
    let events = s.run("please answer").await;
    assert!(matches!(events.last(), Some(GenEvent::Error { code: "provider_error", .. })), "{events:?}");
    assert_eq!(s.m1.requests(), 3, "the first try and two retries");
    assert_eq!(s.m2.requests(), 0, "no fallback (ADR 0014)");
    let rows = s.rows();
    assert_eq!((rows[0].0.as_str(), rows[0].1.as_deref()), ("error", Some("p1")));
}

#[tokio::test]
async fn even_a_subscription_provider_is_never_used_in_place_of_the_one_asked_for() {
    let s = stack("m-500", "m", serde_json::json!({ "billing": "subscription" })).await;
    let events = s.run("please answer").await;
    assert!(matches!(events.last(), Some(GenEvent::Error { .. })), "{events:?}");
    assert_eq!(s.m2.requests(), 0);
}

#[tokio::test]
async fn once_text_has_been_sent_the_router_does_not_retry() {
    let s = stack("m-reset", "m", serde_json::json!({})).await;
    let events = s.run("tell me something").await;
    assert!(matches!(events.last(), Some(GenEvent::Error { .. })), "the failure ends the generation: {events:?}");
    assert_eq!(s.m2.requests(), 0, "no other provider");
    assert_eq!(s.m1.requests(), 1, "and no retry either");
    let rows = s.rows();
    assert_eq!((rows[0].0.as_str(), rows[0].2.as_str()), ("error", "Reset midway"), "what arrived is kept");
}

#[tokio::test]
async fn a_401_is_not_retried_ends_the_reply_and_flags_the_provider_as_having_bad_credentials() {
    let s = stack("m-fail", "m", serde_json::json!({})).await;
    let events = s.run("hello there").await;
    assert!(matches!(events.last(), Some(GenEvent::Error { .. })), "{events:?}");
    assert_eq!(s.m1.requests(), 1, "a bad key is not retried");
    assert_eq!(s.m2.requests(), 0, "and no other provider stands in");
    let flag: String = s
        .state
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT status FROM provider_health WHERE provider_id = 'p1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(flag, "auth_failed");
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
    let p: askbar_proto::Problem = serde_json::from_slice(&body).unwrap();
    assert_eq!((p.error.as_str(), p.extra["period"].as_str()), ("budget_exceeded", Some("daily")));
    assert!(p.detail.contains("today"), "{}", p.detail);
}

#[tokio::test]
async fn every_chat_refusal_is_a_problem_with_its_code_status_and_retry_after() {
    use askbar_server::generation::SubmitError as E;
    let cases: [(E, u16, &str, Option<&str>); 8] = [
        (E::NotFound, 404, "not_found", None),
        (E::QueueFull, 409, "queue_full", None),
        (E::KillSwitch, 503, "kill_switch", None),
        (E::UserPaused, 403, "user_paused", None),
        (E::AuditKeyMissing, 503, "audit_key_missing", None),
        (E::AuditBacklog, 503, "audit_backlog", Some("5")),
        (E::BudgetExceeded("monthly"), 429, "budget_exceeded", None),
        (E::Internal, 500, "internal", None),
    ];
    for (err, status, code, retry_after) in cases {
        let r = err.into_response();
        assert_eq!(r.status().as_u16(), status, "{code}");
        assert_eq!(r.headers()["content-type"], "application/problem+json");
        assert_eq!(r.headers().get("retry-after").map(|v| v.to_str().unwrap()), retry_after, "{code}");
        let body = axum::body::to_bytes(r.into_body(), 4096).await.unwrap();
        let p: askbar_proto::Problem = serde_json::from_slice(&body).unwrap();
        assert_eq!((p.error.as_str(), p.status), (code, status));
    }
}

#[tokio::test]
async fn a_stop_while_waiting_to_retry_ends_the_generation_at_once() {
    let s = stack("m-429", "m", serde_json::json!({})).await;
    let sub = s.submit("hello").expect("accepted");
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
async fn the_audit_trail_shows_every_try_on_the_one_provider() {
    let s = stack("m-500", "m", serde_json::json!({})).await;
    let sub = s.submit("please answer").expect("accepted");
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
    assert_eq!(rows, [want("p1", "error", 0, None), want("p1", "error", 1, None), want("p1", "error", 2, None),]);
}

#[tokio::test]
async fn the_flag_clears_on_the_next_success_and_the_endpoint_lists_it() {
    use askbar_server::server_api;
    let s = stack("m-fail", "m", serde_json::json!({})).await;
    assert!(matches!(s.run("hello there").await.last(), Some(GenEvent::Error { .. })));

    // The server endpoint lists it, with a code and no provider text.
    let dir = s._dir.clone();
    let hash = dir.join("token.sha256");
    let token = "mgt_health-test-token";
    use sha2::Digest;
    let hex: String = sha2::Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&hash, hex).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(s.state.clone(), hash);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let get = || async {
        http.get(format!("{url}/api/server/v1/providers/health"))
            .bearer_auth(token)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    };
    let v = get().await;
    assert_eq!(v["providers"].as_array().unwrap().len(), 1);
    assert_eq!(
        (v["providers"][0]["provider_id"].as_str(), v["providers"][0]["status"].as_str(), v["providers"][0]["error_code"].as_str()),
        (Some("p1"), Some("auth_failed"), Some("http_401"))
    );
    assert!(!v.to_string().contains("Incorrect API key"), "the provider's own words are not kept: {v}");
    let r = http.get(format!("{url}/api/server/v1/providers/health")).send().await.unwrap();
    assert_eq!(r.status(), 401, "needs the server token");

    // A flagged provider that then works is cleared by the success.
    s.state.db.lock().unwrap().flag_provider_auth_failed("p2", "http_403").unwrap();
    assert!(matches!(s.run("another one").await.last(), Some(GenEvent::Error { .. }))); // p1 still fails (m-fail)
    let sub = s.submit_to("p2", "on p2").expect("accepted");
    assert!(is_done(&sub.live.subscribe(0).collect::<Vec<_>>().await));
    let flagged: Vec<String> = s.state.db.lock().unwrap().provider_health().unwrap().into_iter().map(|h| h.0).collect();
    assert_eq!(flagged, ["p1"], "p2 worked, so its flag is gone; p1 failed again and is flagged again");
}

#[tokio::test]
async fn after_a_retry_the_generation_row_and_its_token_usage_name_the_provider_once() {
    let s = stack("m", "m", serde_json::json!({})).await;
    assert!(is_done(&s.run("hello 429once retry-D").await));
    assert_eq!(s.rows()[0].1.as_deref(), Some("p1"));
    let usage_provider: String =
        s.state.db.lock().unwrap().conn().query_row("SELECT provider_id FROM token_usage", [], |r| r.get(0)).unwrap();
    assert_eq!(usage_provider, "p1");
    let n: i64 = s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM token_usage", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 1, "the failed tries used nothing and recorded nothing");
}

#[tokio::test]
async fn the_provider_is_announced_once_before_the_first_delta_even_after_retries() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let events = s.run("hello").await;
    assert_eq!(events[0], GenEvent::Provider { id: "p1".into(), name: "p1".into(), model: "m".into() }, "first of all");
    assert!(matches!(events[1], GenEvent::Delta { seq: 1, .. }), "then the text, counted from 1: {:?}", &events[..3]);
    assert_eq!(events.iter().filter(|e| matches!(e, GenEvent::Provider { .. })).count(), 1);

    // A retry does not announce it again.
    let s = stack("m", "m", serde_json::json!({})).await;
    let events = s.run("hello 429once retry-C").await;
    assert_eq!(events[0], GenEvent::Provider { id: "p1".into(), name: "p1".into(), model: "m".into() });
    assert!(matches!(events[1], GenEvent::Delta { seq: 1, .. }), "{:?}", &events[..2]);
    assert_eq!(events.iter().filter(|e| matches!(e, GenEvent::Provider { .. })).count(), 1);

    // Nor does a failure after text has gone out.
    let s = stack("m-reset", "m", serde_json::json!({})).await;
    let events = s.run("tell me something").await;
    assert_eq!(events.iter().filter(|e| matches!(e, GenEvent::Provider { .. })).count(), 1);
}

#[tokio::test]
async fn a_subscriber_who_already_has_text_is_not_sent_the_announcements_again() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let sub = s.submit("hello").expect("accepted");
    let all: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    let rest: Vec<GenEvent> = sub.live.subscribe(3).collect().await;
    assert!(rest.iter().all(|e| !matches!(e, GenEvent::Provider { .. })), "{rest:?}");
    assert_eq!(rest.len(), all.len() - 1 - 3, "everything after delta 3, without the announcement");
}

#[tokio::test]
async fn the_kill_switch_gate_leaves_a_span_whether_it_lets_a_generation_through_or_not() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let sub = s.submit("works").expect("accepted");
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
    assert_eq!(
        (gates[0].0.as_str(), gates[0].1.as_str(), gates[0].2.as_deref()),
        (trace.as_str(), "done", None),
        "passed, inside the generation's own trace"
    );
    assert!(gates[0].3.contains(r#""kill_switch":false"#));
    assert_eq!((gates[1].1.as_str(), gates[1].2.as_deref()), ("blocked", Some("kill_switch")));
    assert_ne!(gates[1].0, trace, "a refusal has no generation, so a trace of its own");
    assert!(gates[1].3.contains(r#""kill_switch":true"#), "the snapshot flag is recorded");
    // Turning it off again lets the next one through.
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.kill_switch = false);
    assert!(is_done(&s.run("works again").await));
}

/// The server API for `s`, with a token, so a test can push a snapshot the way the admin does.
async fn server_api(s: &Stack) -> (String, String) {
    use sha2::Digest;
    let token = "mgt_router-test-token".to_string();
    let hash = s._dir.join("token.sha256");
    std::fs::write(&hash, sha2::Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect::<String>()).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::server_api::router(s.state.clone(), hash);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, token)
}

/// Pushes the current snapshot back with the kill switch on and `aborts` as given.
async fn switch_off(s: &Stack, aborts: bool) {
    let (url, token) = server_api(s).await;
    let mut snap = askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap();
    snap.kill_switch = true;
    snap.kill_switch_aborts_running = aborts;
    let r = reqwest::Client::new().put(format!("{url}/api/server/v1/config")).bearer_auth(token).json(&snap).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn turning_the_kill_switch_on_with_abort_stops_running_replies_and_clears_the_queue() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let first = s.submit("please be slow").expect("accepted");
    let queued = askbar_server::generation::submit(
        &s.state,
        Submit {
            space: askbar_server::testing::personal_space(&s.state, &s.user),
            user_id: s.user.clone(),
            client_id: "d1".into(),
            conversation_id: Some(first.conversation_id.clone()),
            provider_id: "p1".into(),
            model: s.models[0].clone(),
            prompt: "waiting".into(),
            client_request_id: None,
        },
    )
    .expect("queued");
    assert!(queued.queued);
    tokio::time::sleep(Duration::from_millis(1200)).await; // it has produced some text

    switch_off(&s, true).await;
    let a: Vec<GenEvent> = first.live.subscribe(0).collect().await;
    let b: Vec<GenEvent> = queued.live.subscribe(0).collect().await;
    assert!(matches!(a.last(), Some(GenEvent::Error { message, .. }) if message.contains("switched off")), "{:?}", a.last());
    assert!(matches!(a.last(), Some(GenEvent::Error { code: "kill_switch", .. })), "{:?}", a.last());
    assert!(
        matches!(b.last(), Some(GenEvent::Error { .. })) && !b.iter().any(|e| matches!(e, GenEvent::Delta { .. })),
        "the waiting one never ran: {b:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;

    let rows = s.rows();
    assert_eq!((rows[0].0.as_str(), rows[1].0.as_str()), ("cancelled", "cancelled"));
    assert!(rows[0].2.starts_with("Streaming slowly"), "the text so far is kept: {:?}", rows[0].2);
    let (messages, user_msgs): (i64, i64) = {
        let space = askbar_server::testing::personal_space(&s.state, &s.user);
        let db = space.lock();
        (
            db.conn().query_row("SELECT COUNT(*) FROM messages WHERE role = 'assistant'", [], |r| r.get(0)).unwrap(),
            db.conn().query_row("SELECT COUNT(*) FROM messages WHERE role = 'user'", [], |r| r.get(0)).unwrap(),
        )
    };
    assert_eq!((messages, user_msgs), (1, 1), "the partial reply is saved; the queued question never was");
    assert_eq!(s.m1.requests(), 1, "nothing further went upstream");
    s.state.audit.flush().await;
    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let code: Option<String> =
        db.query_row("SELECT error_code FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&first.generation_id], |r| r.get(0)).unwrap();
    let status: String =
        db.query_row("SELECT status FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&first.generation_id], |r| r.get(0)).unwrap();
    assert_eq!((status.as_str(), code.as_deref()), ("cancelled", Some("kill_switch")));
    assert!(matches!(s.submit("and now").err(), Some(SubmitError::KillSwitch)), "and nothing new starts");
}

#[tokio::test]
async fn turning_it_on_without_abort_lets_running_replies_finish_but_still_clears_the_queue() {
    let s = stack("m", "m", serde_json::json!({})).await;
    let first = s.submit("please be slow").expect("accepted");
    let queued = askbar_server::generation::submit(
        &s.state,
        Submit {
            space: askbar_server::testing::personal_space(&s.state, &s.user),
            user_id: s.user.clone(),
            client_id: "d1".into(),
            conversation_id: Some(first.conversation_id.clone()),
            provider_id: "p1".into(),
            model: s.models[0].clone(),
            prompt: "waiting".into(),
            client_request_id: None,
        },
    )
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
    let sub = s.submit("first").expect("under the limit");
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

#[tokio::test]
async fn cached_tokens_an_anthropic_provider_reports_reach_the_attempt_span_and_the_trace() {
    let s = stack("m", "m", serde_json::json!({})).await;
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| {
        snap.providers.iter_mut().find(|p| p.id == "p1").unwrap().kind = "anthropic".into();
    });
    let sub = s.submit("a cached prompt").expect("accepted");
    let trace = sub.generation_id.clone();
    let events: Vec<GenEvent> = sub.live.subscribe(0).collect().await;
    assert!(is_done(&events), "{events:?}");
    s.state.audit.flush().await;
    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let (kind, tin, tout, cached): (String, i64, i64, i64) = db
        .query_row("SELECT kind, tokens_in, tokens_out, tokens_cached FROM spans WHERE trace_id = ?1 AND kind = 'attempt'", [&trace], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .unwrap();
    assert_eq!((kind.as_str(), tin, tout, cached), ("attempt", 100, 50, 64), "the mock's message_start reports 64 cache-read tokens");
    let turn_cached: i64 =
        db.query_row("SELECT tokens_cached FROM spans WHERE trace_id = ?1 AND kind = 'turn'", [&trace], |r| r.get(0)).unwrap();
    assert_eq!(turn_cached, 64);
}

#[tokio::test]
async fn a_paused_user_is_refused_early_with_a_gate_span_and_others_are_not() {
    let s = stack("m", "m", serde_json::json!({})).await;
    assert!(is_done(&s.run("works before").await));
    let user = s.state.db.lock().unwrap().seed_server().unwrap();
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.paused_user_ids = vec![user.clone()]);
    let refused = s.submit("and now").err().expect("refused");
    assert_eq!(refused, SubmitError::UserPaused);
    let response = SubmitError::UserPaused.into_response();
    assert_eq!(response.status(), 403);
    let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
    let p: askbar_proto::Problem = serde_json::from_slice(&body).unwrap();
    assert_eq!(p.error, "user_paused");
    assert!(p.detail.contains("paused for you"), "{}", p.detail);
    assert_eq!(s.m1.requests(), 1, "nothing more was sent upstream");

    s.state.audit.flush().await;
    let db = rusqlite::Connection::open(s._dir.join("audit.db")).unwrap();
    let blocked: (String, Option<String>, String) = db
        .query_row("SELECT status, error_code, attrs_json FROM spans WHERE kind = 'gate' AND name = 'user_pause'", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((blocked.0.as_str(), blocked.1.as_deref()), ("blocked", Some("user_paused")));
    assert!(blocked.2.contains(r#""paused":true"#));

    // Pausing somebody else does not stop this user, and resuming does.
    askbar_server::testing::update_snapshot(&s.state.db.lock().unwrap(), |snap| snap.paused_user_ids = vec!["someone-else".into()]);
    assert!(is_done(&s.run("works again").await));
}
