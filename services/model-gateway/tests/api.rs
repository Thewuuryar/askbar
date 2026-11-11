//! The gateway against a real in-process auditor and the real mock LLM (ADR 0054, ADR 0010): what must be on
//! record before a provider is called, what the server gets back, which token opens which listener, and where
//! the keys and the provider's words never go.

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_model_gateway::{admin_links, admin_router, server_links, server_router, Gateway};
use askbar_proto::gateway::{ChatEvent, ChatMessage, ChatRequest, FailureClass, GatewayConfig, GatewayProvider};
use askbar_proto::{AuditKey, SseParser};
use askbar_service::audit_client::AuditClient;
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use futures_util::StreamExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const AUDIT_SECRET: [u8; 32] = [7; 32];
const MARKER_KEY: &str = "sk-marker-must-never-leave-memory";
const SRV: &str = "tok_srvgw_secret";
const ADM: &str = "tok_admgw_secret";

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------------------ the mock LLM

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

fn start_mock() -> Mock {
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

// ------------------------------------------------------------------ the rig

struct Rig {
    dir: PathBuf,
    auditor: Arc<Auditor>,
    audit_url: String,
    gw: Arc<Gateway>,
    server: String,
    admin: String,
    http: reqwest::Client,
    free: Arc<AtomicU64>,
    exited: Arc<AtomicBool>,
    mock: Mock,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn audit_key() -> AuditKey {
    let public = *crypto_box::SecretKey::from(AUDIT_SECRET).public_key().as_bytes();
    AuditKey { key_id: askbar_model_gateway::seal::key_id(&public), public_key: public.iter().map(|b| format!("{b:02x}")).collect() }
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

async fn rig() -> Rig {
    rig_with(true).await
}

/// `auditor_up` false points the gateway at an address nothing listens on.
async fn rig_with(auditor_up: bool) -> Rig {
    rig_full(auditor_up, None).await
}

/// A rig whose auditor sits behind a proxy that holds each `POST /spans` for as many milliseconds as `delay_ms` says.
async fn rig_slow(delay_ms: Arc<AtomicU64>) -> Rig {
    rig_full(true, Some(delay_ms)).await
}

async fn rig_full(auditor_up: bool, delay_ms: Option<Arc<AtomicU64>>) -> Rig {
    let dir = std::env::temp_dir().join(format!("askbar-gateway-{}", ulid::Ulid::new()));
    let audit_tokens = dir.join("audit/tokens");
    let gw_tokens = dir.join("gw/tokens");
    std::fs::create_dir_all(audit_tokens.join("accept")).unwrap();
    std::fs::create_dir_all(gw_tokens.join("accept")).unwrap();
    std::fs::create_dir_all(gw_tokens.join("call")).unwrap();
    for link in ["server-auditor", "model-gateway-auditor", "admin-auditor-append", "admin-auditor-read"] {
        std::fs::write(audit_tokens.join("accept").join(format!("{link}.sha256")), format!("{}\n", hex(&format!("tok_{link}")))).unwrap();
    }
    std::fs::write(gw_tokens.join("accept/server-model-gateway.sha256"), format!("{}\n", hex(SRV))).unwrap();
    std::fs::write(gw_tokens.join("accept/admin-model-gateway.sha256"), format!("{}\n", hex(ADM))).unwrap();
    std::fs::write(gw_tokens.join("call/model-gateway-auditor.token"), "tok_model-gateway-auditor\n").unwrap();

    let limits = Limits::default();
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("audit/audit.db")).unwrap(), limits.max_queued);
    let free = Arc::new(AtomicU64::new(100 * 1024 * 1024 * 1024));
    let f = free.clone();
    let free_space: FreeSpace = Arc::new(move || Some(f.load(Ordering::Relaxed)));
    let auditor = Auditor::new(recorder, limits, free_space);
    let audit_url = if auditor_up {
        let real = serve(askbar_auditor::router(auditor.clone(), askbar_auditor::links(&audit_tokens))).await;
        match delay_ms {
            Some(d) => slow_proxy(real, d).await,
            None => real,
        }
    } else {
        let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        format!("http://127.0.0.1:{dead}")
    };

    let client = AuditClient::new(&audit_url, gw_tokens.clone(), "model-gateway-auditor");
    let boot = BootId::new();
    let exited = Arc::new(AtomicBool::new(false));
    let e = exited.clone();
    let restarter =
        Restarter::for_tests(boot.clone(), Duration::ZERO, Duration::from_secs(1), || {}, move || e.store(true, Ordering::SeqCst));
    let gw = Gateway::new(client.clone(), boot.clone(), restarter);
    tokio::spawn(client.clone().hello_until_done(boot.as_str().to_string()));
    tokio::spawn(client.drain_forever());
    let server = serve(server_router(gw.clone(), server_links(&gw_tokens))).await;
    let admin = serve(admin_router(gw.clone(), admin_links(&gw_tokens))).await;
    let rig = Rig { dir, auditor, audit_url, gw, server, admin, http: reqwest::Client::new(), free, exited, mock: start_mock() };
    if auditor_up {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !rig.gw.audit().ready() {
            assert!(Instant::now() < deadline, "the gateway never said hello");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    rig
}

/// A proxy in front of `real` that holds `POST /spans` for `delay_ms` before passing it on: a slow acknowledgment.
async fn slow_proxy(real: String, delay_ms: Arc<AtomicU64>) -> String {
    use axum::body::Bytes;
    use axum::extract::{Request, State};
    use axum::http::StatusCode;
    async fn forward(State((real, delay)): State<(String, Arc<AtomicU64>)>, req: Request) -> (StatusCode, Bytes) {
        let (parts, body) = req.into_parts();
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        let path = parts.uri.path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
        if parts.method == axum::http::Method::POST && path.ends_with("/spans") {
            tokio::time::sleep(Duration::from_millis(delay.load(Ordering::Relaxed))).await;
        }
        let mut r = reqwest::Client::new().request(parts.method, format!("{real}{path}")).body(bytes);
        if let Some(a) = parts.headers.get("authorization") {
            r = r.header("authorization", a);
        }
        if let Some(c) = parts.headers.get("content-type") {
            r = r.header("content-type", c);
        }
        match r.send().await {
            Ok(resp) => (StatusCode::from_u16(resp.status().as_u16()).unwrap(), resp.bytes().await.unwrap_or_default()),
            Err(_) => (StatusCode::BAD_GATEWAY, Bytes::new()),
        }
    }
    serve(axum::Router::new().fallback(forward).with_state((real, delay_ms))).await
}

fn provider(rig: &Rig, id: &str, model: &str) -> GatewayProvider {
    GatewayProvider {
        id: id.into(),
        name: format!("mock-{id}"),
        kind: "openai".into(),
        base_url: format!("http://127.0.0.1:{}/v1", rig.mock.port),
        model: model.into(),
        max_tokens: None,
        temperature: None,
        headers: Default::default(),
        api_key: MARKER_KEY.into(),
    }
}

fn config(rig: &Rig, version: u64, models: &[(&str, &str)]) -> GatewayConfig {
    GatewayConfig {
        version,
        providers: models.iter().map(|(id, m)| provider(rig, id, m)).collect(),
        audit_public_key: Some(audit_key()),
        generation_timeout_secs: 300,
    }
}

impl Rig {
    async fn push(&self, c: &GatewayConfig) -> reqwest::Response {
        self.http.put(format!("{}/api/model-gateway/v1/config", self.admin)).bearer_auth(ADM).json(c).send().await.unwrap()
    }

    fn request(&self, provider_id: &str, generation: &str, text: &str) -> ChatRequest {
        ChatRequest {
            provider_id: provider_id.into(),
            user_id: "user1".into(),
            client_id: Some("client1".into()),
            space_id: Some("space1".into()),
            conversation_id: Some("conv1".into()),
            generation_id: generation.into(),
            system: "be brief".into(),
            messages: vec![ChatMessage { role: "user".into(), content: text.into() }],
        }
    }

    async fn chat_raw(&self, req: &ChatRequest) -> reqwest::Response {
        self.http.post(format!("{}/api/model-gateway/v1/chat", self.server)).bearer_auth(SRV).json(req).send().await.unwrap()
    }

    /// Every event of a stream, up to its end.
    async fn chat(&self, req: &ChatRequest) -> Vec<ChatEvent> {
        let resp = self.chat_raw(req).await;
        assert_eq!(resp.status(), 200, "{:?}", resp.text().await);
        let mut stream = resp.bytes_stream();
        let mut parser = SseParser::default();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            for ev in parser.feed(&chunk.unwrap()) {
                out.push(serde_json::from_str::<ChatEvent>(&ev.data).unwrap());
            }
        }
        out
    }

    fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        let conn = self.auditor.recorder.reader().unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        let n = stmt.column_count();
        stmt.query_map([], |r| {
            Ok((0..n)
                .map(|i| match r.get_ref(i).unwrap() {
                    askbar_audit::rusqlite::types::ValueRef::Null => Value::Null,
                    askbar_audit::rusqlite::types::ValueRef::Integer(v) => json!(v),
                    askbar_audit::rusqlite::types::ValueRef::Real(v) => json!(v),
                    askbar_audit::rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    askbar_audit::rusqlite::types::ValueRef::Blob(b) => json!(format!("blob:{}", b.len())),
                })
                .collect())
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// Wait until `sql` returns at least one row (the gateway's later records arrive through its queue).
    async fn wait_rows(&self, sql: &str) -> Vec<Vec<Value>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let r = self.rows(sql);
            if !r.is_empty() {
                return r;
            }
            assert!(Instant::now() < deadline, "no rows for: {sql}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn provider_calls(&self) -> usize {
        self.mock.log.lock().unwrap().iter().filter(|l| l.contains("POST")).count()
    }
}

fn text_of(events: &[ChatEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Delta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ configuration and links

#[tokio::test]
async fn it_answers_not_configured_until_the_admin_pushes_and_status_says_which_version() {
    let rig = rig().await;
    let status = |tok: &'static str| {
        let (http, url) = (rig.http.clone(), format!("{}/api/model-gateway/v1/status", rig.admin));
        async move { http.get(url).bearer_auth(tok).send().await.unwrap() }
    };
    let s: Value = status(ADM).await.json().await.unwrap();
    assert_eq!(s["config_version"], Value::Null);
    assert_eq!(s["boot_id"], rig.gw.boot_id.as_str());

    let r = rig.chat_raw(&rig.request("p", "gen1", "hi")).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "not_configured");

    assert_eq!(rig.push(&config(&rig, 7, &[("p", "m")])).await.status(), 200);
    let s: Value = status(ADM).await.json().await.unwrap();
    assert_eq!(s["config_version"], 7);
    // The gateway records the version it now runs, with its own token (ADR 0028 rule 5).
    let rows = rig.wait_rows("SELECT service, category, action, outcome, detail_json FROM admin_actions").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][..4], [json!("model-gateway"), json!("config"), json!("applied"), json!("done")]);
    assert_eq!(rows[0][4], json!("{\"version\":7}"));
    assert_eq!(rig.http.get(format!("{}/api/model-gateway/v1/health", rig.server)).send().await.unwrap().status(), 200);
}

#[tokio::test]
async fn each_listener_takes_only_its_own_token_and_routes() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let admin = |m: reqwest::Method, p: &str, t: Option<&str>| {
        let mut r = rig.http.request(m, format!("{}/api/model-gateway/v1{p}", rig.admin));
        if let Some(t) = t {
            r = r.bearer_auth(t);
        }
        async move { r.send().await.unwrap().status().as_u16() }
    };
    let server = |m: reqwest::Method, p: &str, t: Option<&str>| {
        let mut r = rig.http.request(m, format!("{}/api/model-gateway/v1{p}", rig.server));
        if let Some(t) = t {
            r = r.bearer_auth(t);
        }
        async move { r.send().await.unwrap().status().as_u16() }
    };
    use reqwest::Method as M;
    assert_eq!(admin(M::GET, "/status", None).await, 401);
    assert_eq!(admin(M::GET, "/status", Some("tok_wrong")).await, 401);
    // The server's token is no good on the admin's routes, nor the admin's on the server's.
    assert_eq!(admin(M::GET, "/status", Some(SRV)).await, 401);
    assert_eq!(server(M::POST, "/chat", Some(ADM)).await, 401);
    // The admin's listener has no chat route at all, so even the right token finds nothing there.
    let mut both = rig.http.post(format!("{}/api/model-gateway/v1/chat", rig.admin));
    both = both.bearer_auth(ADM);
    assert_eq!(both.send().await.unwrap().status().as_u16(), 404);
    // Each listener serves only its own routes.
    assert_eq!(server(M::GET, "/status", Some(SRV)).await, 404);
    assert_eq!(admin(M::PUT, "/config", Some(SRV)).await, 401);
}

#[tokio::test]
async fn a_bad_configuration_is_refused_and_changes_nothing() {
    let rig = rig().await;
    assert_eq!(rig.push(&config(&rig, 1, &[("p", "m")])).await.status(), 200);
    let mut bad = config(&rig, 2, &[("p", "m")]);
    bad.audit_public_key = None;
    assert_eq!(rig.push(&bad).await.status(), 400, "no audit key: it could not record what a provider says");
    let mut bad = config(&rig, 2, &[("p", "m")]);
    bad.audit_public_key.as_mut().unwrap().key_id = "0000000000000000".into();
    assert_eq!(rig.push(&bad).await.status(), 400);
    let mut bad = config(&rig, 2, &[("p", "m"), ("p", "m2")]);
    assert_eq!(rig.push(&bad).await.status(), 400, "two providers with one id");
    bad = config(&rig, 2, &[("p", "m")]);
    bad.providers[0].kind = "carrier-pigeon".into();
    assert_eq!(rig.push(&bad).await.status(), 400);
    assert_eq!(rig.gw.config_version(), Some(1), "a refused push leaves the last good configuration in place");
}

#[tokio::test]
async fn restart_exits_the_process_and_nothing_else_does() {
    let rig = rig().await;
    let r = rig.http.post(format!("{}/api/model-gateway/v1/restart", rig.admin)).bearer_auth(ADM).send().await.unwrap();
    assert_eq!(r.status(), 202);
    assert_eq!(r.json::<Value>().await.unwrap()["boot_id"], rig.gw.boot_id.as_str());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !rig.exited.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "it did not stop");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ------------------------------------------------------------------ a reply

#[tokio::test]
async fn a_reply_is_on_record_before_the_provider_is_called_and_the_stream_starts_with_the_span() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let events = rig.chat(&rig.request("p", "gen-happy", "hello there")).await;

    let ChatEvent::Span { span_id, trace_id } = &events[0] else { panic!("the first event is the span: {events:?}") };
    assert_eq!(trace_id, "gen-happy");
    assert!(text_of(&events).contains("quick answer"), "{}", text_of(&events));
    assert!(events.iter().any(|e| matches!(e, ChatEvent::Usage { .. })));
    let ChatEvent::Done { attempts, .. } = events.last().unwrap() else { panic!("{events:?}") };
    assert_eq!(*attempts, 1);

    // The turn span is in the auditor, running (the server finishes it), opened under the gateway's token.
    let turn =
        rig.rows(&format!("SELECT kind, status, user_id, client_id, provider_id, model, trace_id FROM spans WHERE id = '{span_id}'"));
    assert_eq!(turn.len(), 1, "{turn:?}");
    assert_eq!(
        turn[0],
        vec![json!("turn"), json!("running"), json!("user1"), json!("client1"), json!("p"), json!("m"), json!("gen-happy")]
    );
    let service: Vec<Vec<Value>> = rig.rows(&format!("SELECT service FROM spans WHERE id = '{span_id}'"));
    assert_eq!(service[0][0], "model-gateway");

    // And the attempt span, finished, with the tokens the provider counted.
    let attempt = rig.wait_rows("SELECT status, tokens_in, tokens_out, parent_id FROM spans WHERE kind = 'attempt' AND trace_id = 'gen-happy' AND status = 'done'").await;
    assert_eq!(attempt[0][3], json!(span_id));
    assert!(attempt[0][1].as_i64().unwrap() > 0 && attempt[0][2].as_i64().unwrap() > 0, "{attempt:?}");
}

#[tokio::test]
async fn no_provider_is_called_when_the_auditor_refuses_and_the_refusal_is_recorded_later() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    // Below the disk floor the auditor refuses new spans.
    rig.free.store(1024, Ordering::Relaxed);
    let r = rig.chat_raw(&rig.request("p", "gen-refused", "hello")).await;
    assert_eq!(r.status(), 503);
    assert!(r.headers().contains_key("retry-after"));
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "audit_backlog");
    assert_eq!(rig.provider_calls(), 0, "no provider was called");

    // When the auditor is back, the record of the refusal arrives.
    rig.free.store(100 * 1024 * 1024 * 1024, Ordering::Relaxed);
    let row = rig.wait_rows("SELECT status, error_code, trace_id FROM spans WHERE trace_id = 'gen-refused'").await;
    assert_eq!(row[0], vec![json!("refused"), json!("audit_backlog"), json!("gen-refused")]);
}

#[tokio::test]
async fn no_provider_is_called_when_the_auditor_cannot_be_reached() {
    let rig = rig_with(false).await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let r = rig.chat_raw(&rig.request("p", "gen-down", "hello")).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "audit_backlog");
    assert_eq!(rig.provider_calls(), 0);
    // The configuration it applied (ADR 0028 rule 5) and the refusal (a span that began and ended) wait for the auditor.
    assert_eq!(rig.gw.audit().queued(), 3, "the applied version and the refusal wait for the auditor");
    let _ = &rig.audit_url;
}

#[tokio::test]
async fn a_provider_error_reaches_the_server_as_a_code_and_the_auditor_as_sealed_text() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("bad", "m-fail")])).await;
    let events = rig.chat(&rig.request("bad", "gen-401", "hello")).await;
    let ChatEvent::Error { code, class, attempts, .. } = events.last().unwrap() else { panic!("{events:?}") };
    assert_eq!((code.as_str(), *class, *attempts), ("http_401", FailureClass::AuthFailed, 1), "an auth failure is not retried");

    // The wire carries no provider words at all.
    let wire = serde_json::to_string(&events).unwrap();
    assert!(!wire.to_lowercase().contains("invalid") && !wire.contains(MARKER_KEY), "{wire}");

    // The auditor has them, sealed to the audit key: only the private key opens them.
    let rows = rig
        .wait_rows("SELECT p.role, p.key_id, p.ciphertext FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = 'gen-401'")
        .await;
    assert_eq!(rows[0][0], "error");
    assert_eq!(rows[0][1], json!(audit_key().key_id));
    let conn = rig.auditor.recorder.reader().unwrap();
    let blob: Vec<u8> = conn
        .query_row("SELECT p.ciphertext FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = 'gen-401'", [], |r| r.get(0))
        .unwrap();
    let opened = String::from_utf8(crypto_box::SecretKey::from(AUDIT_SECRET).unseal(&blob).unwrap()).unwrap();
    assert!(opened.contains("401"), "the provider's own words, sealed: {opened}");
    // The payload is queued before the finish record, so wait for the finish too.
    let attempt =
        rig.wait_rows("SELECT status, error_code FROM spans WHERE trace_id = 'gen-401' AND kind = 'attempt' AND status != 'running'").await;
    assert_eq!(attempt[0], vec![json!("error"), json!("http_401")]);
}

#[tokio::test]
async fn a_failing_provider_is_tried_three_times_and_each_try_is_an_attempt_span() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("down", "m-500")])).await;
    let events = rig.chat(&rig.request("down", "gen-500", "hello")).await;
    let ChatEvent::Error { code, class, attempts, .. } = events.last().unwrap() else { panic!("{events:?}") };
    assert_eq!((code.as_str(), *class, *attempts), ("http_500", FailureClass::Retryable, 3));
    let spans = rig.rows("SELECT json_extract(attrs_json, '$.retry_index') FROM spans WHERE trace_id = 'gen-500' AND kind = 'attempt' ORDER BY started_at, id");
    assert_eq!(spans.len(), 3, "{spans:?}");
    assert_eq!(rig.provider_calls(), 3);
}

#[tokio::test]
async fn a_rate_limit_that_clears_gives_a_reply_on_the_second_try() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let events = rig.chat(&rig.request("p", "gen-429", "429once please")).await;
    let ChatEvent::Done { attempts, .. } = events.last().unwrap() else { panic!("{events:?}") };
    assert_eq!(*attempts, 2);
    assert!(text_of(&events).contains("quick answer"));
    let spans = rig.wait_rows("SELECT status FROM spans WHERE trace_id = 'gen-429' AND kind = 'attempt' AND status = 'done'").await;
    assert_eq!(spans.len(), 1);
    assert_eq!(rig.rows("SELECT status FROM spans WHERE trace_id = 'gen-429' AND kind = 'attempt'").len(), 2);
}

#[tokio::test]
async fn a_caller_that_goes_away_cancels_the_request_and_the_attempt_ends_cancelled() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let resp = rig.chat_raw(&rig.request("p", "gen-cancel", "slow answer please")).await;
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::default();
    let mut deltas = 0;
    while deltas < 2 {
        let chunk = stream.next().await.unwrap().unwrap();
        deltas += parser.feed(&chunk).iter().filter(|e| e.event.as_deref() == Some("delta")).count();
    }
    drop(stream);
    let rows = rig.wait_rows("SELECT status FROM spans WHERE trace_id = 'gen-cancel' AND kind = 'attempt' AND status != 'running'").await;
    assert_eq!(rows[0][0], "cancelled");
}

#[tokio::test]
async fn a_turn_the_server_never_finishes_is_ended_as_interrupted_and_a_finish_it_did_send_stands() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let a = rig.chat(&rig.request("p", "gen-stale-a", "hello")).await;
    let b = rig.chat(&rig.request("p", "gen-stale-b", "hello")).await;
    let id = |e: &[ChatEvent]| match &e[0] {
        ChatEvent::Span { span_id, .. } => span_id.clone(),
        other => panic!("{other:?}"),
    };
    let (a, b) = (id(&a), id(&b));
    // The server finishes b with its own token, as it will.
    let r = rig
        .http
        .post(format!("{}/api/audit/v1/spans/{b}/finish", rig.audit_url))
        .bearer_auth("tok_server-auditor")
        .json(&json!({ "status": "done", "ended_at": 1, "ended_ms": 1000 }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());

    // Not yet: the limit and the 60 s have not passed.
    assert_eq!(rig.gw.finish_stale_turns(Instant::now()), 0);
    // Past them, both are due; a's is recorded, and b stays as the server finished it.
    assert_eq!(rig.gw.finish_stale_turns(Instant::now() + Duration::from_secs(300 + 61)), 2);
    let a_row = rig.wait_rows(&format!("SELECT status, error_code FROM spans WHERE id = '{a}' AND status != 'running'")).await;
    assert_eq!(a_row[0], vec![json!("interrupted"), json!("stale_turn")]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let b_row = rig.rows(&format!("SELECT status FROM spans WHERE id = '{b}'"));
    assert_eq!(b_row[0][0], "done", "the first finish is the one that counts");
}

#[tokio::test]
async fn a_key_appears_nowhere_a_caller_or_the_auditor_can_see() {
    let rig = rig().await;
    let pushed = rig.push(&config(&rig, 1, &[("p", "m"), ("bad", "m-fail")])).await;
    let mut seen = pushed.text().await.unwrap();
    for (provider, model) in [("p", "m"), ("bad", "m-fail")] {
        let events = rig.chat(&rig.request(provider, &format!("gen-{model}"), "hello")).await;
        seen.push_str(&serde_json::to_string(&events).unwrap());
    }
    let status = rig.http.get(format!("{}/api/model-gateway/v1/status", rig.admin)).bearer_auth(ADM).send().await.unwrap();
    seen.push_str(&status.text().await.unwrap());
    let refused = rig.chat_raw(&rig.request("nobody", "gen-x", "hello")).await;
    seen.push_str(&refused.text().await.unwrap());
    assert!(!seen.contains(MARKER_KEY), "{seen}");

    // Wait for the gateway's queue to empty, then look at everything the auditor stored.
    let deadline = Instant::now() + Duration::from_secs(10);
    while rig.gw.audit().queued() > 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for table in ["spans", "payloads"] {
        let dump = format!("{:?}", rig.rows(&format!("SELECT * FROM {table}")));
        assert!(!dump.contains(MARKER_KEY), "{table}");
    }
}

#[tokio::test]
async fn bad_requests_are_refused_before_anything_is_recorded() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    assert_eq!(rig.chat_raw(&rig.request("nobody", "g1", "hi")).await.status(), 404);
    let mut r = rig.request("p", "g 2", "hi");
    assert_eq!(rig.chat_raw(&r).await.status(), 400, "an id with a space");
    r = rig.request("p", "g3", "   ");
    assert_eq!(rig.chat_raw(&r).await.status(), 400, "nothing to send");
    assert_eq!(rig.rows("SELECT * FROM spans WHERE kind = 'turn'").len(), 0);
    assert_eq!(rig.provider_calls(), 0);
}

#[tokio::test]
async fn an_acknowledgment_slower_than_a_second_is_a_refusal_and_no_provider_is_called() {
    let delay = Arc::new(AtomicU64::new(1500));
    let rig = rig_slow(delay.clone()).await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let started = Instant::now();
    let r = rig.chat_raw(&rig.request("p", "gen-slow", "hello")).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.json::<Value>().await.unwrap()["error"], "audit_backlog");
    assert!(started.elapsed() < Duration::from_millis(1400), "it gave up at one second, not when the answer came: {:?}", started.elapsed());
    assert_eq!(rig.provider_calls(), 0, "no provider was called");

    // The auditor catches up: the same request works, and the span the slow append left behind is the same one the
    // retry of a lost answer would make, so nothing is recorded twice.
    delay.store(0, Ordering::Relaxed);
    let events = rig.chat(&rig.request("p", "gen-slow-2", "hello again")).await;
    assert!(matches!(events.last(), Some(ChatEvent::Done { .. })), "{events:?}");
    assert_eq!(rig.provider_calls(), 1);
}
