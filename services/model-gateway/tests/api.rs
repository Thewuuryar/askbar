//! The gateway against a real in-process auditor and the real mock LLM (ADR 0054, ADR 0010): what must be on
//! record before a provider is called, what the server gets back, which token opens which listener, and where
//! the keys and the provider's words never go.

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_model_gateway::{admin_links, admin_router, server_links, server_router, Gateway};
use askbar_proto::audit::{SpanKind, SpanStart};
use askbar_proto::gateway::{ChatEvent, ChatMessage, ChatRequest, FailureClass, GatewayConfig, GatewayProvider, Role};
use askbar_proto::{model_id, AuditKey, CatalogEntry, ModelList, SseParser};
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
    for link in ["server-auditor", "model-gateway-auditor", "model-gateway-auditor-read", "admin-auditor-append", "admin-auditor-read"] {
        std::fs::write(audit_tokens.join("accept").join(format!("{link}.sha256")), format!("{}\n", hex(&format!("tok_{link}")))).unwrap();
    }
    std::fs::write(gw_tokens.join("accept/server-model-gateway.sha256"), format!("{}\n", hex(SRV))).unwrap();
    std::fs::write(gw_tokens.join("accept/admin-model-gateway.sha256"), format!("{}\n", hex(ADM))).unwrap();
    std::fs::write(gw_tokens.join("call/model-gateway-auditor.token"), "tok_model-gateway-auditor\n").unwrap();
    std::fs::write(gw_tokens.join("call/model-gateway-auditor-read.token"), "tok_model-gateway-auditor-read\n").unwrap();

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
    // The budget is read from the auditor on request here (`refresh_usage`), so a test says when the gateway knows.
    gw.set_usage_reader(askbar_model_gateway::usage::Reader::new(&audit_url, gw_tokens.clone()));
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
        billing: Default::default(),
    }
}

fn config(rig: &Rig, version: u64, models: &[(&str, &str)]) -> GatewayConfig {
    GatewayConfig {
        role_pins: Default::default(),
        quick_search: Default::default(),
        quick_search_enabled: true,
        task_limits: Default::default(),
        research_enabled: true,
        version,
        providers: models.iter().map(|(id, m)| provider(rig, id, m)).collect(),
        audit_public_key: Some(audit_key()),
        generation_timeout_secs: 300,
        budget: Default::default(),
        kill_switch: false,
        kill_switch_aborts_running: true,
        paused_users: vec![],
        // Every pair is approved, the first the default (ADR 0014).
        catalog: models.iter().map(|(id, m)| CatalogEntry { provider_id: (*id).into(), model: (*m).into() }).collect(),
        default_model: models.first().map(|(id, m)| model_id(id, m)),
        approval_key: None,
        approval_ttl_secs: askbar_proto::DEFAULT_APPROVAL_TTL_SECS,
    }
}

/// The model the tests' configurations approve for a provider id.
fn model_of(provider_id: &str) -> &'static str {
    match provider_id {
        "bad" => "m-fail",
        "down" => "m-500",
        _ => "m",
    }
}

impl Rig {
    async fn push(&self, c: &GatewayConfig) -> reqwest::Response {
        self.http.put(format!("{}/api/model-gateway/v1/config", self.admin)).bearer_auth(ADM).json(c).send().await.unwrap()
    }

    fn request(&self, provider_id: &str, generation: &str, text: &str) -> ChatRequest {
        ChatRequest {
            role: None,
            provider_id: provider_id.into(),
            model: model_of(provider_id).into(),
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
    let row = rig.wait_rows("SELECT status, error_code, trace_id FROM spans WHERE trace_id = 'gen-refused' AND status != 'running'").await;
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
        .wait_rows("SELECT p.role, p.key_id, p.ciphertext FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = 'gen-401' AND p.role = 'error'")
        .await;
    assert_eq!(rows[0][0], "error");
    assert_eq!(rows[0][1], json!(audit_key().key_id));
    let conn = rig.auditor.recorder.reader().unwrap();
    let blob: Vec<u8> = conn
        .query_row(
            "SELECT p.ciphertext FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = 'gen-401' AND p.role = 'error'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let opened = String::from_utf8(crypto_box::SecretKey::from(AUDIT_SECRET).unseal(&blob).unwrap()).unwrap();
    assert!(opened.contains("401"), "the provider's own words, sealed: {opened}");
    // The payload is queued before the finish record, so wait for the finish too.
    let attempt =
        rig.wait_rows("SELECT status, error_code FROM spans WHERE trace_id = 'gen-401' AND kind = 'attempt' AND status != 'running'").await;
    assert_eq!(attempt[0], vec![json!("error"), json!("http_401")]);
}

#[tokio::test]
async fn the_provider_request_is_sealed_on_the_attempt_and_a_step_under_it_is_stored_and_chained() {
    const PLANTED: &str = "planted-secret-words";
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    rig.chat(&rig.request("p", "gen-pr", PLANTED)).await;
    let attempt = rig.wait_rows("SELECT id FROM spans WHERE trace_id = 'gen-pr' AND kind = 'attempt' AND status = 'done'").await;
    let attempt_id = attempt[0][0].as_str().unwrap().to_string();

    // The body is sealed to the audit key, exactly as built, and the private key opens it.
    let sql = format!("SELECT ciphertext FROM payloads WHERE span_id = '{attempt_id}' AND role = 'provider_request'");
    rig.wait_rows(&sql).await;
    let blob: Vec<u8> = rig.auditor.recorder.reader().unwrap().query_row(&sql, [], |r| r.get(0)).unwrap();
    let body: Value = serde_json::from_slice(&crypto_box::SecretKey::from(AUDIT_SECRET).unseal(&blob).unwrap()).unwrap();
    assert_eq!((body["model"].as_str(), body["stream"].as_bool()), (Some("m"), Some(true)), "{body}");
    assert!(body.to_string().contains(PLANTED) && body.to_string().contains("be brief"), "{body}");

    // The gateway can write a step under the attempt; it is stored and the chain still checks.
    let (at, ms) = (1_700_000_000, 0);
    let step = SpanStart {
        id: "step-1".into(),
        trace_id: "gen-pr".into(),
        parent_id: Some(attempt_id),
        kind: SpanKind::Step,
        name: "route".into(),
        started_at: at,
        started_ms: ms,
        provider_id: None,
        model: None,
        user_id: None,
        client_id: None,
        attrs: Some(json!({ "step": "route" })),
    };
    rig.gw.audit().open(&step).await.unwrap();
    let rows = rig.rows("SELECT kind, parent_id IS NOT NULL FROM spans WHERE id = 'step-1'");
    assert_eq!(rows, vec![vec![json!("step"), json!(1)]]);
    assert!(askbar_audit::chain::verify(&rig.auditor.recorder.reader().unwrap()).unwrap().ok);

    // No span's attributes, the step's included, carry the request's words; nor does the status.
    let attrs = rig.rows("SELECT attrs_json FROM spans WHERE trace_id = 'gen-pr'");
    assert!(!format!("{attrs:?}").contains(PLANTED), "{attrs:?}");
    let status = rig.http.get(format!("{}/api/model-gateway/v1/status", rig.admin)).bearer_auth(ADM).send().await.unwrap();
    assert!(!status.text().await.unwrap().contains(PLANTED));
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
async fn the_approval_key_is_held_after_a_push_that_carries_it_and_appears_nowhere() {
    const APPROVAL: &str = "marker-approval-key-bytes";
    let rig = rig().await;
    assert!(rig.gw.approval_key().is_none(), "fail closed before the first push");
    let mut c = config(&rig, 1, &[("p", "m")]);
    rig.push(&c).await;
    assert!(rig.gw.approval_key().is_none(), "a push without one holds none");
    c.version = 2;
    c.approval_key = Some(askbar_proto::gateway::ApprovalSigningKey { version: 3, pkcs8: APPROVAL.into() });
    let pushed = rig.push(&c).await;
    let mut seen = pushed.text().await.unwrap();
    assert_eq!(rig.gw.approval_key().map(|k| k.version), Some(3));
    let status = rig.http.get(format!("{}/api/model-gateway/v1/status", rig.admin)).bearer_auth(ADM).send().await.unwrap();
    seen.push_str(&status.text().await.unwrap());
    seen.push_str(&format!("{:?}", rig.rows("SELECT * FROM admin_actions")));
    c.approval_key = Some(askbar_proto::gateway::ApprovalSigningKey { version: 4, pkcs8: String::new() });
    assert_eq!(rig.push(&c).await.status(), 400, "an empty key is refused");
    assert!(!seen.contains(APPROVAL), "{seen}");
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

// ------------------------------------------------------------------ the stops and the budget (plan step 3.2)

fn as_user(mut req: ChatRequest, user: &str) -> ChatRequest {
    req.user_id = user.into();
    req
}

async fn problem_of(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    (status, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn the_kill_switch_refuses_a_new_reply_before_any_call_and_leaves_a_gate_span() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.kill_switch = true;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let (status, body) = problem_of(rig.chat_raw(&rig.request("p", "gen-killed", "hello")).await).await;
    assert_eq!((status, body["error"].as_str()), (503, Some("kill_switch")));
    assert_eq!(rig.provider_calls(), 0, "no provider was called");
    let rows = rig
        .wait_rows("SELECT kind, name, status, error_code, user_id FROM spans WHERE trace_id = 'gen-killed' AND status != 'running'")
        .await;
    assert_eq!(rows[0], vec![json!("gate"), json!("kill_switch"), json!("blocked"), json!("kill_switch"), json!("user1")]);
    assert_eq!(
        rig.rows("SELECT COUNT(*) FROM spans WHERE trace_id = 'gen-killed' AND kind IN ('turn', 'attempt')")[0][0],
        0,
        "no turn was opened"
    );

    // Off again, and the next one runs.
    cfg.kill_switch = false;
    cfg.version = 2;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    assert!(matches!(rig.chat(&rig.request("p", "gen-back", "hello")).await.last(), Some(ChatEvent::Done { .. })));
}

#[tokio::test]
async fn a_paused_user_is_refused_and_everyone_else_is_not() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.paused_users = vec!["user1".into()];
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let (status, body) = problem_of(rig.chat_raw(&rig.request("p", "gen-paused", "hello")).await).await;
    assert_eq!((status, body["error"].as_str()), (403, Some("user_paused")));
    assert_eq!(rig.provider_calls(), 0);
    let rows = rig.wait_rows("SELECT name, status, error_code FROM spans WHERE trace_id = 'gen-paused' AND status != 'running'").await;
    assert_eq!(rows[0], vec![json!("user_pause"), json!("blocked"), json!("user_paused")]);
    assert!(
        matches!(rig.chat(&as_user(rig.request("p", "gen-other", "hello"), "user2")).await.last(), Some(ChatEvent::Done { .. })),
        "another user is not paused"
    );
}

#[tokio::test]
async fn a_budget_is_held_against_what_the_gateway_served_and_a_subscription_is_never_counted() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m"), ("flat", "m")]);
    cfg.providers[1].billing = askbar_proto::Billing::Subscription;
    cfg.budget.daily_tokens = Some(100);
    assert_eq!(rig.push(&cfg).await.status(), 200);

    // Before it has read how much was used, it cannot hold a budget: ask again, and nothing is called.
    let r = rig.chat_raw(&rig.request("p", "gen-unknown", "hello")).await;
    assert_eq!(r.headers()["retry-after"], "5");
    let (status, body) = problem_of(r).await;
    assert_eq!((status, body["error"].as_str()), (503, Some("audit_backlog")));
    assert_eq!(rig.provider_calls(), 0);
    assert!(rig.gw.refresh_usage().await, "the auditor answers the gateway's read link");

    // Under the limit it runs (the mock reports 150 tokens), and then the 150 it served are over the 100 allowed.
    assert!(matches!(rig.chat(&rig.request("p", "gen-1", "hello")).await.last(), Some(ChatEvent::Done { .. })));
    let (status, body) = problem_of(rig.chat_raw(&rig.request("p", "gen-2", "hello")).await).await;
    assert_eq!((status, body["error"].as_str(), body["period"].as_str()), (429, Some("budget_exceeded"), Some("daily")));
    assert_eq!(rig.provider_calls(), 1, "the second was refused before any call");
    let rows = rig.wait_rows("SELECT status, error_code, attrs_json FROM spans WHERE trace_id = 'gen-2' AND name = 'budget'").await;
    assert_eq!((rows[0][0].clone(), rows[0][1].clone()), (json!("blocked"), json!("budget_exceeded")));
    assert!(
        rows[0][2].as_str().unwrap().contains("\"daily_used\":150") && rows[0][2].as_str().unwrap().contains("\"daily_limit\":100"),
        "{:?}",
        rows[0][2]
    );

    // A subscription costs nothing per token, so the budget does not apply to it.
    assert!(matches!(rig.chat(&rig.request("flat", "gen-flat", "hello")).await.last(), Some(ChatEvent::Done { .. })));
    // And more room is more room.
    cfg.budget.daily_tokens = Some(10_000);
    cfg.version = 2;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    assert!(matches!(rig.chat(&rig.request("p", "gen-3", "hello")).await.last(), Some(ChatEvent::Done { .. })));
}

#[tokio::test]
async fn a_restart_cannot_reset_a_budget_because_the_auditor_remembers_what_was_used() {
    use askbar_audit::{Record, SpanFinish, SpanStart};
    let rig = rig().await;
    // The auditor already holds a turn that used 100 tokens today (what an earlier run of the gateway served).
    let now = askbar_service::audit_client::now_parts().0;
    let start = SpanStart {
        id: "earlier-turn".into(),
        trace_id: "earlier".into(),
        kind: "turn".into(),
        name: "chat".into(),
        started_at: now,
        ..Default::default()
    };
    rig.auditor.recorder.append(Record::Start(start)).await.unwrap();
    let end = SpanFinish {
        id: "earlier-turn".into(),
        status: "done".into(),
        ended_at: now,
        tokens_in: Some(90),
        tokens_out: Some(10),
        ..Default::default()
    };
    rig.auditor.recorder.append(Record::Finish(end)).await.unwrap();

    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.budget.daily_tokens = Some(100);
    assert_eq!(rig.push(&cfg).await.status(), 200);
    assert!(rig.gw.refresh_usage().await);
    let (status, body) = problem_of(rig.chat_raw(&rig.request("p", "gen-after-restart", "hello")).await).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (429, Some("budget_exceeded")),
        "the gateway itself has served nothing, and still refuses"
    );
    assert_eq!(rig.provider_calls(), 0);
}

#[tokio::test]
async fn a_pause_and_the_kill_switch_stop_the_replies_that_are_running_and_say_why() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    assert_eq!(rig.push(&cfg).await.status(), 200);

    // Start a slow reply and read its first words.
    let resp = rig.chat_raw(&rig.request("p", "gen-running", "slow please")).await;
    assert_eq!(resp.status(), 200);
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::default();
    let mut events: Vec<ChatEvent> = Vec::new();
    let read_until = |events: &Vec<ChatEvent>, want: fn(&ChatEvent) -> bool| events.iter().any(want);
    while !read_until(&events, |e| matches!(e, ChatEvent::Delta { .. })) {
        let chunk = stream.next().await.expect("the stream goes on").unwrap();
        events.extend(parser.feed(&chunk).into_iter().map(|ev| serde_json::from_str::<ChatEvent>(&ev.data).unwrap()));
    }
    // The user is paused: the reply ends at once, with the reason.
    cfg.paused_users = vec!["user1".into()];
    cfg.version = 2;
    let paused_at = Instant::now();
    assert_eq!(rig.push(&cfg).await.status(), 200);
    while let Some(chunk) = stream.next().await {
        events.extend(parser.feed(&chunk.unwrap()).into_iter().map(|ev| serde_json::from_str::<ChatEvent>(&ev.data).unwrap()));
    }
    assert!(paused_at.elapsed() < Duration::from_secs(5), "it was stopped, not waited out: {:?}", paused_at.elapsed());
    assert!(
        matches!(events.last(), Some(ChatEvent::Error { code, class: FailureClass::Fatal, .. }) if code == "user_paused"),
        "{events:?}"
    );
    let rows = rig
        .wait_rows("SELECT status, error_code FROM spans WHERE trace_id = 'gen-running' AND kind = 'attempt' AND status != 'running'")
        .await;
    assert_eq!(rows[0], vec![json!("cancelled"), json!("user_paused")]);

    // The kill switch, set to stop what is running.
    cfg.paused_users.clear();
    cfg.version = 3;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let resp = rig.chat_raw(&rig.request("p", "gen-running-2", "slow please")).await;
    let mut stream = resp.bytes_stream();
    let mut events: Vec<ChatEvent> = Vec::new();
    let mut parser = SseParser::default();
    while !events.iter().any(|e| matches!(e, ChatEvent::Delta { .. })) {
        let chunk = stream.next().await.unwrap().unwrap();
        events.extend(parser.feed(&chunk).into_iter().map(|ev| serde_json::from_str::<ChatEvent>(&ev.data).unwrap()));
    }
    cfg.kill_switch = true;
    cfg.version = 4;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    while let Some(chunk) = stream.next().await {
        events.extend(parser.feed(&chunk.unwrap()).into_iter().map(|ev| serde_json::from_str::<ChatEvent>(&ev.data).unwrap()));
    }
    assert!(matches!(events.last(), Some(ChatEvent::Error { code, .. }) if code == "kill_switch"), "{events:?}");
}

#[tokio::test]
async fn a_kill_switch_set_not_to_stop_running_replies_lets_them_finish_and_refuses_new_ones() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let resp = rig.chat_raw(&rig.request("p", "gen-finishes", "brisk please")).await;
    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::default();
    let mut events: Vec<ChatEvent> = Vec::new();
    while !events.iter().any(|e| matches!(e, ChatEvent::Delta { .. })) {
        let chunk = stream.next().await.unwrap().unwrap();
        events.extend(parser.feed(&chunk).into_iter().map(|ev| serde_json::from_str::<ChatEvent>(&ev.data).unwrap()));
    }
    cfg.kill_switch = true;
    cfg.kill_switch_aborts_running = false;
    cfg.version = 2;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let (status, body) = problem_of(rig.chat_raw(&rig.request("p", "gen-refused", "hello")).await).await;
    assert_eq!((status, body["error"].as_str()), (503, Some("kill_switch")), "new ones are refused");
    while let Some(chunk) = stream.next().await {
        events.extend(parser.feed(&chunk.unwrap()).into_iter().map(|ev| serde_json::from_str::<ChatEvent>(&ev.data).unwrap()));
    }
    assert!(matches!(events.last(), Some(ChatEvent::Done { .. })), "the running one finished: {events:?}");
}

// ------------------------------------------------------------------ the model catalog (ADR 0014)

async fn catalog_of(rig: &Rig, base: &str, token: &str) -> reqwest::Response {
    rig.http.get(format!("{base}/api/model-gateway/v1/catalog")).bearer_auth(token).send().await.unwrap()
}

#[tokio::test]
async fn the_catalog_is_served_to_the_server_with_its_default() {
    let rig = rig().await;
    assert_eq!(catalog_of(&rig, &rig.server, SRV).await.status(), 503, "no settings yet, no catalog");
    let mut cfg = config(&rig, 1, &[("p", "m"), ("bad", "m-fail")]);
    cfg.default_model = Some(model_id("bad", "m-fail"));
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let list: ModelList = catalog_of(&rig, &rig.server, SRV).await.json().await.unwrap();
    assert_eq!(list.default.as_deref(), Some("bad/m-fail"));
    let ids: Vec<_> = list.models.iter().map(|m| (m.id.as_str(), m.provider_name.as_str(), m.model.as_str())).collect();
    assert_eq!(ids, [("p/m", "mock-p", "m"), ("bad/m-fail", "mock-bad", "m-fail")]);
    // The admin's listener has no such route: it pushes the catalog, the server reads it.
    assert_eq!(catalog_of(&rig, &rig.admin, ADM).await.status(), 404);
}

#[tokio::test]
async fn a_catalog_needs_a_default_that_is_in_it_and_only_names_known_providers() {
    let rig = rig().await;
    let mut no_default = config(&rig, 1, &[("p", "m")]);
    no_default.default_model = None;
    let mut outside = config(&rig, 1, &[("p", "m")]);
    outside.default_model = Some("p/other".into());
    let mut stranger = config(&rig, 1, &[("p", "m")]);
    stranger.catalog.push(CatalogEntry { provider_id: "nobody".into(), model: "m".into() });
    let mut twice = config(&rig, 1, &[("p", "m")]);
    twice.catalog.push(CatalogEntry { provider_id: "p".into(), model: "m".into() });
    let mut empty_with_default = config(&rig, 1, &[("p", "m")]);
    empty_with_default.catalog.clear();
    let mut blank = config(&rig, 1, &[("p", "m")]);
    blank.catalog[0].model = "  ".into();
    for (why, bad) in [
        ("no default", no_default),
        ("a default outside the catalog", outside),
        ("an unknown provider", stranger),
        ("a model listed twice", twice),
        ("a default with nothing approved", empty_with_default),
        ("a blank model", blank),
    ] {
        assert_eq!(rig.push(&bad).await.status(), 400, "{why}");
    }
    // Nothing approved at all is allowed: it is how an install starts, and every reply is refused until the admin chooses.
    let mut none = config(&rig, 2, &[("p", "m")]);
    none.catalog.clear();
    none.default_model = None;
    assert_eq!(rig.push(&none).await.status(), 200);
    let (status, body) = problem_of(rig.chat_raw(&rig.request("p", "gen-nothing-approved", "hello")).await).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("model_not_approved")));
}

#[tokio::test]
async fn a_model_outside_the_catalog_is_refused_with_a_gate_span_and_no_provider_call() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let mut req = rig.request("p", "gen-unapproved", "hello");
    req.model = "m-other".into();
    let (status, body) = problem_of(rig.chat_raw(&req).await).await;
    assert_eq!((status, body["error"].as_str()), (400, Some("model_not_approved")));
    assert_eq!(rig.provider_calls(), 0, "no provider was called");
    let rows =
        rig.wait_rows("SELECT kind, name, status, error_code FROM spans WHERE trace_id = 'gen-unapproved' AND status != 'running'").await;
    assert_eq!(rows[0], vec![json!("gate"), json!("model_catalog"), json!("blocked"), json!("model_not_approved")]);
    assert_eq!(rig.rows("SELECT COUNT(*) FROM spans WHERE trace_id = 'gen-unapproved' AND kind IN ('turn', 'attempt')")[0][0], 0);
    // The approved one still works.
    assert!(matches!(rig.chat(&rig.request("p", "gen-approved", "hello")).await.last(), Some(ChatEvent::Done { .. })));
}

#[tokio::test]
async fn the_model_called_is_the_one_asked_for_not_the_providers_own() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.catalog.push(CatalogEntry { provider_id: "p".into(), model: "m-second".into() });
    assert_eq!(rig.push(&cfg).await.status(), 200);
    let mut req = rig.request("p", "gen-second", "hello");
    req.model = "m-second".into();
    assert!(matches!(rig.chat(&req).await.last(), Some(ChatEvent::Done { .. })));
    let rows = rig.wait_rows("SELECT DISTINCT model FROM spans WHERE trace_id = 'gen-second' AND kind IN ('turn', 'attempt')").await;
    assert_eq!(rows, vec![vec![json!("m-second")]]);
}

#[tokio::test]
async fn a_request_with_a_role_uses_the_pinned_model_and_its_spans_carry_the_role() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.catalog.push(CatalogEntry { provider_id: "p".into(), model: "m-pinned".into() });
    cfg.role_pins.insert(Role::SearchQuery, "p/m-pinned".into());
    assert_eq!(rig.push(&cfg).await.status(), 200);

    // The pin wins over the model the request names; the turn and the attempt record the role.
    let mut req = rig.request("p", "gen-role", "hello");
    req.role = Some(Role::SearchQuery);
    assert!(matches!(rig.chat(&req).await.last(), Some(ChatEvent::Done { .. })));
    rig.wait_rows("SELECT 1 FROM spans WHERE trace_id = 'gen-role' AND kind = 'attempt' AND status = 'done'").await;
    let rows = rig.rows("SELECT model, attrs_json FROM spans WHERE trace_id = 'gen-role' AND kind IN ('turn', 'attempt')");
    assert_eq!(rows.len(), 2, "{rows:?}");
    for r in &rows {
        assert_eq!(r[0], json!("m-pinned"));
        assert!(r[1].as_str().unwrap().contains(r#""role":"search.query""#), "{r:?}");
    }

    // No role: unchanged, the named model is used and no span names a role.
    let mut plain = rig.request("p", "gen-norole", "hello");
    plain.model = "m".into();
    assert!(matches!(rig.chat(&plain).await.last(), Some(ChatEvent::Done { .. })));
    let rows = rig.wait_rows("SELECT DISTINCT model FROM spans WHERE trace_id = 'gen-norole' AND kind IN ('turn', 'attempt')").await;
    assert_eq!(rows, vec![vec![json!("m")]]);
    let attrs = rig.rows("SELECT attrs_json FROM spans WHERE trace_id = 'gen-norole'");
    assert!(!format!("{attrs:?}").contains("role"), "{attrs:?}");
}

// ------------------------------------------------------------------ the provider test (plan step 3.4, ADR 0014)

impl Rig {
    async fn provider_test(&self, id: &str) -> (u16, Value) {
        let r = self.http.post(format!("{}/api/model-gateway/v1/providers/{id}/test", self.admin)).bearer_auth(ADM).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn a_passing_test_streams_reports_the_time_to_first_text_and_keeps_the_reply_to_itself() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let (status, v) = rig.provider_test("p").await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert!(v["ttft_ms"].is_u64() && v["message"].as_str().unwrap().starts_with("Streaming works"), "{v}");
    assert_eq!(
        (v["details"]["method"].as_str(), v["details"]["path"].as_str(), v["details"]["status"].as_u64()),
        (Some("POST"), Some("/v1/chat/completions"), Some(200))
    );
    assert!(v["details"]["error_type"].is_null() && v["details"]["total_ms"].is_u64() && v["details"]["ttft_ms"].is_u64(), "{v}");
    // Nothing the provider said is in the answer, and neither is the key.
    let wire = v.to_string();
    assert!(!wire.contains("quick answer") && !wire.contains(MARKER_KEY), "{wire}");

    // On record: one turn span for the test, finished with the first-text time, and no payload of any kind.
    let rows = rig.wait_rows("SELECT status, provider_id, model, ttft_ms IS NOT NULL, error_code FROM spans WHERE name = 'provider_test' AND status = 'done'").await;
    assert_eq!(rows[0], vec![json!("done"), json!("p"), json!("m"), json!(1), Value::Null]);
    assert_eq!(rig.rows("SELECT COUNT(*) FROM payloads")[0][0], 0, "metadata only");
}

#[tokio::test]
async fn a_failing_test_says_why_in_our_words_and_never_repeats_the_providers() {
    let rig = rig().await;
    rig.push(&config(&rig, 1, &[("bad", "m-fail")])).await;
    let (status, v) = rig.provider_test("bad").await;
    assert_eq!(status, 200, "{v}");
    assert_eq!((v["ok"].as_bool(), v["message"].as_str()), (Some(false), Some("The provider refused the key.")), "{v}");
    assert_eq!((v["details"]["status"].as_u64(), v["details"]["error_type"].as_str()), (Some(401), Some("auth")));
    assert!(v["ttft_ms"].is_null());
    let wire = v.to_string();
    assert!(!wire.contains("Incorrect") && !wire.contains(MARKER_KEY), "the provider's own words stay out: {wire}");
    let rows = rig.wait_rows("SELECT status, error_code, ttft_ms FROM spans WHERE name = 'provider_test' AND status = 'error'").await;
    assert_eq!(rows[0], vec![json!("error"), json!("auth"), Value::Null]);
    assert_eq!(rig.rows("SELECT COUNT(*) FROM payloads")[0][0], 0, "not even the error text is kept");
}

#[tokio::test]
async fn a_test_waits_for_the_auditor_like_any_model_call_and_an_unknown_provider_is_404() {
    let rig = rig_with(false).await;
    rig.push(&config(&rig, 1, &[("p", "m")])).await;
    let (status, v) = rig.provider_test("p").await;
    assert_eq!((status, v["ok"].as_bool(), v["details"]["error_type"].as_str()), (200, Some(false), Some("audit")), "{v}");
    assert_eq!(rig.provider_calls(), 0, "the provider was not called");
    assert_eq!(rig.provider_test("nobody").await.0, 404);
    // Only the admin's listener has the route.
    let on_server = rig.http.post(format!("{}/api/model-gateway/v1/providers/p/test", rig.server)).bearer_auth(SRV).send().await.unwrap();
    assert_eq!(on_server.status(), 404);
}

#[tokio::test]
async fn approvals_are_signed_fresh_and_leak_nothing_and_fail_closed_without_a_key() {
    use askbar_model_gateway::approval::{approve, ApproveError};
    use askbar_proto::approval::{Provenance, ToolAction};
    use askbar_service::approval::{generate_keypair, verify, OneTimeIds};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    struct Any;
    impl OneTimeIds for Any {
        fn consume(&self, _: &str, _: i64) -> bool {
            true
        }
    }
    let action = |url: &str| ToolAction {
        tool: "fetch".into(),
        arguments: json!({"url": url}),
        destination: "example.com".into(),
        provenance: Provenance::User,
        query_data: false,
    };
    let rig = rig().await;
    let mut c = config(&rig, 1, &[("p", "m")]);
    rig.push(&c).await;
    assert_eq!(approve(&rig.gw, &action("https://a.example"), "t", "u").unwrap_err(), ApproveError::NoKey);

    let (pkcs8, public) = generate_keypair().unwrap();
    let b64 = URL_SAFE_NO_PAD.encode(&pkcs8);
    c.version = 2;
    c.approval_key = Some(askbar_proto::gateway::ApprovalSigningKey { version: 1, pkcs8: b64.clone() });
    rig.push(&c).await;
    let now = askbar_service::audit_client::now_parts().0;
    let (a, attrs) = approve(&rig.gw, &action("https://a.example"), "t", "u").unwrap();
    let (b, _) = approve(&rig.gw, &action("https://a.example"), "t", "u").unwrap();
    let (other, _) = approve(&rig.gw, &action("https://b.example"), "t", "u").unwrap();
    assert_eq!(a.action_hash, b.action_hash);
    assert_ne!(a.action_hash, other.action_hash, "a changed argument changes the hash");
    assert_ne!(a.one_time_id, b.one_time_id);
    assert!(!a.one_time_id.is_empty() && (now + 59..=now + 61).contains(&a.expires_at), "{}", a.expires_at);
    verify(&a, &action("https://a.example"), &public, now, &Any).unwrap();
    assert!(verify(&a, &action("https://b.example"), &public, now, &Any).is_err());
    let seen = format!("{attrs} {:?} {:?}", rig.gw.approval_key(), approve(&rig.gw, &action("x"), "t", "u").map(|_| ()));
    assert!(!seen.contains(&b64) && !seen.contains(&a.signature) && !seen.contains("https://a.example"), "{seen}");
    assert_eq!(attrs["one_time_id"], a.one_time_id.as_str());

    c.version = 3;
    c.approval_ttl_secs = 0;
    assert_eq!(rig.push(&c).await.status(), 400, "a TTL out of bounds is refused");
    c.approval_ttl_secs = askbar_proto::DEFAULT_APPROVAL_TTL_SECS;
    c.quick_search.max_queries = askbar_proto::quick_search::MAX_QUERIES + 1;
    assert_eq!(rig.push(&c).await.status(), 400, "a quick search limit out of bounds is refused");
    c.quick_search = Default::default();
    c.task_limits.token_cap = askbar_proto::research::MIN_TOKEN_CAP - 1;
    assert_eq!(rig.push(&c).await.status(), 400, "a task limit out of bounds is refused");
}

// ------------------------------------------------------------------ quick search approvals (plan W5.2)

const QS_MARKER: &str = "marker-secret-query-text";

/// A rig with an approval key pushed; returns it and the public key.
async fn qs_rig(auditor_up: bool, tweak: impl FnOnce(&mut GatewayConfig)) -> (Rig, Vec<u8>) {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let rig = rig_with(auditor_up).await;
    let (pkcs8, public) = askbar_service::approval::generate_keypair().unwrap();
    let mut c = config(&rig, 1, &[("p", "m")]);
    c.approval_key = Some(askbar_proto::gateway::ApprovalSigningKey { version: 1, pkcs8: URL_SAFE_NO_PAD.encode(pkcs8) });
    tweak(&mut c);
    assert_eq!(rig.push(&c).await.status(), 200);
    (rig, public)
}

impl Rig {
    async fn qs(&self, generation: &str, user: &str, tool: &str, arguments: Value) -> (u16, Value) {
        let body = json!({"generation_id": generation, "user": user, "tool": tool, "arguments": arguments});
        let r = self
            .http
            .post(format!("{}/api/model-gateway/v1/approvals/quick-search", self.server))
            .bearer_auth(SRV)
            .json(&body)
            .send()
            .await
            .unwrap();
        problem_of(r).await
    }
}

#[tokio::test]
async fn quick_search_refusals_each_have_their_own_code_and_sign_nothing() {
    let (rig, _) = qs_rig(true, |c| {
        c.quick_search.max_queries = 2;
        c.quick_search.max_pages = 1;
        c.quick_search.max_query_chars = 20;
    })
    .await;
    let long = json!({"query": QS_MARKER.repeat(3)});
    let cases = [
        ("gen-q1", "fetch2", json!({"url": "https://a.example/"}), 400, "quick_search_tool"),
        ("gen-q2", "search", long, 400, "quick_search_arguments"),
        ("gen-q3", "search", json!({"query": "ok", "extra": 1}), 400, "quick_search_arguments"),
        ("gen-q4", "fetch", json!({"url": 7}), 400, "quick_search_arguments"),
        ("gen-q5", "fetch", json!({"query": "x"}), 400, "quick_search_arguments"),
    ];
    for (generation, tool, args, status, code) in cases {
        let tool = if tool == "fetch2" { "shell" } else { tool };
        let (s, b) = rig.qs(generation, "user1", tool, args).await;
        assert_eq!((s, b["error"].as_str()), (status, Some(code)), "{b}");
        assert!(b.get("approval").is_none());
    }
    // Per reply: two searches, one fetch; a refused request does not count.
    for _ in 0..2 {
        assert_eq!(rig.qs("gen-c", "user1", "search", json!({"query": "a"})).await.0, 200);
    }
    assert_eq!(rig.qs("gen-c", "user1", "search", json!({"query": QS_MARKER.repeat(3)})).await.1["error"], "quick_search_arguments");
    let (s, b) = rig.qs("gen-c", "user1", "search", json!({"query": "a"})).await;
    assert_eq!((s, b["error"].as_str()), (429, Some("quick_search_query_count")));
    assert_eq!(rig.qs("gen-c", "user1", "fetch", json!({"url": "https://a.example/x"})).await.0, 200);
    let (s, b) = rig.qs("gen-c", "user1", "fetch", json!({"url": "https://a.example/y"})).await;
    assert_eq!((s, b["error"].as_str()), (429, Some("quick_search_fetch_count")));
    assert_eq!(
        rig.qs("gen-other", "user1", "fetch", json!({"url": "https://a.example/y"})).await.0,
        200,
        "another reply has its own count"
    );
    let rows = rig.wait_rows("SELECT kind, status, error_code FROM spans WHERE trace_id = 'gen-q1' AND status != 'running'").await;
    assert_eq!(rows[0], vec![json!("gate"), json!("blocked"), json!("quick_search_tool")]);
    assert_eq!(rig.provider_calls(), 0, "no model was called");
}

#[tokio::test]
async fn quick_search_hourly_limit_counts_searches_per_user() {
    let (rig, _) = qs_rig(true, |c| c.quick_search.searches_per_user_hour = 2).await;
    for g in ["g1", "g2"] {
        assert_eq!(rig.qs(g, "u1", "search", json!({"query": "a"})).await.0, 200);
    }
    let (s, b) = rig.qs("g3", "u1", "search", json!({"query": "a"})).await;
    assert_eq!((s, b["error"].as_str()), (429, Some("quick_search_hourly_limit")));
    assert_eq!(rig.qs("g3", "u1", "fetch", json!({"url": "https://a.example/"})).await.0, 200, "fetches are not searches");
    assert_eq!(rig.qs("g4", "u2", "search", json!({"query": "a"})).await.0, 200, "another user has their own hour");
}

#[tokio::test]
async fn quick_search_refuses_when_paused_or_killed() {
    let (rig, _) = qs_rig(true, |c| c.paused_users = vec!["u1".into()]).await;
    let (s, b) = rig.qs("g1", "u1", "search", json!({"query": "a"})).await;
    assert_eq!((s, b["error"].as_str()), (403, Some("user_paused")));
    assert_eq!(rig.qs("g2", "u2", "search", json!({"query": "a"})).await.0, 200);
    let (rig, _) = qs_rig(true, |c| c.kill_switch = true).await;
    let (s, b) = rig.qs("g1", "u2", "search", json!({"query": "a"})).await;
    assert_eq!((s, b["error"].as_str()), (503, Some("kill_switch")));
}

#[tokio::test]
async fn quick_search_turned_off_is_refused_with_its_own_code_and_a_gate_span_without_text() {
    let (rig, _) = qs_rig(true, |c| c.quick_search_enabled = false).await;
    let (s, b) = rig.qs("g-off", "u1", "search", json!({"query": QS_MARKER})).await;
    assert_eq!((s, b["error"].as_str()), (503, Some("quick_search_disabled")));
    assert!(!b.to_string().contains(QS_MARKER));
    let rows =
        rig.wait_rows("SELECT kind, status, error_code, attrs_json FROM spans WHERE trace_id = 'g-off' AND status != 'running'").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0][..3], [json!("gate"), json!("blocked"), json!("quick_search_disabled")]);
    assert!(!rows[0][3].to_string().contains(QS_MARKER));
}

#[tokio::test]
async fn a_quick_search_approval_verifies_at_the_mcp_front_and_leaves_no_text() {
    use askbar_proto::approval::{Approval, Provenance, ToolAction};
    use askbar_service::approval::{verify, OneTimeIds};
    struct Any;
    impl OneTimeIds for Any {
        fn consume(&self, _: &str, _: i64) -> bool {
            true
        }
    }
    let (rig, public) = qs_rig(true, |_| {}).await;
    let now = askbar_service::audit_client::now_parts().0;
    for (tool, args) in
        [("search", json!({"query": QS_MARKER})), ("fetch", json!({"url": format!("https://Docs.Example:443/{QS_MARKER}?q=1")}))]
    {
        let (s, b) = rig.qs("gen-ok", "user1", tool, args.clone()).await;
        assert_eq!(s, 200, "{b}");
        let a: Approval = serde_json::from_value(b["approval"].clone()).unwrap();
        let action = ToolAction {
            tool: tool.into(),
            arguments: args,
            destination: b["destination"].as_str().unwrap().into(),
            provenance: serde_json::from_value::<Provenance>(b["provenance"].clone()).unwrap(),
            query_data: b["query_data"].as_bool().unwrap(),
        };
        assert_eq!((action.provenance, action.query_data), (Provenance::Search, false));
        assert_eq!(action.destination, if tool == "fetch" { "docs.example" } else { "" });
        verify(&a, &action, &public, now, &Any).unwrap();
        assert_eq!(a.task_id, "gen-ok");
        // The span was acknowledged before the approval came back, and holds the hash and ID, not the text.
        let rows = rig.rows(&format!(
            "SELECT status, attrs_json FROM spans WHERE trace_id = 'gen-ok' AND name = 'quick_search_approval' AND attrs_json LIKE '%{}%'",
            a.one_time_id
        ));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0][1].as_str().unwrap().contains(&a.action_hash));
    }
    let all = format!("{:?}", rig.rows("SELECT attrs_json, name FROM spans"));
    assert!(!all.contains(QS_MARKER) && !all.contains("docs.example"), "{all}");
}

#[tokio::test]
async fn quick_search_signs_nothing_when_the_span_is_not_acknowledged() {
    let (rig, _) = qs_rig(false, |_| {}).await;
    let (s, b) = rig.qs("g1", "u1", "search", json!({"query": "a"})).await;
    assert_eq!((s, b["error"].as_str()), (503, Some("audit_backlog")), "{b}");
    assert!(b.get("approval").is_none());
    // The failed attempt did not use up a count.
    let (rig, _) = qs_rig(false, |c| c.quick_search.max_queries = 1).await;
    for _ in 0..2 {
        assert_eq!(rig.qs("g1", "u1", "search", json!({"query": "a"})).await.1["error"], "audit_backlog");
    }
}

// ------------------------------------------------------------------ the task token cap and tool-call limit (plan P5.10)

async fn status_of(rig: &Rig) -> Value {
    rig.http.get(format!("{}/api/model-gateway/v1/status", rig.admin)).bearer_auth(ADM).send().await.unwrap().json().await.unwrap()
}

fn task_req(rig: &Rig, generation: &str, role: Role) -> ChatRequest {
    let mut req = rig.request("p", generation, "hello");
    req.role = Some(role);
    req
}

#[tokio::test]
async fn a_task_at_its_cap_is_refused_with_no_call_and_other_roles_and_tasks_are_not_counted() {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.role_pins.insert(Role::ResearchResearcher, "p/m".into());
    cfg.role_pins.insert(Role::ChatManual, "p/m".into());
    cfg.task_limits.token_cap = 20_000;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    assert_eq!(status_of(&rig).await["task_cap_75"], 0);

    // The mock reports 150 tokens a call; a real call is counted, a chat.manual call is not.
    assert!(matches!(rig.chat(&task_req(&rig, "task-a", Role::ResearchResearcher)).await.last(), Some(ChatEvent::Done { .. })));
    assert!(matches!(rig.chat(&task_req(&rig, "task-a", Role::ChatManual)).await.last(), Some(ChatEvent::Done { .. })));
    // Fill task-a to 14,850, then each call crosses a mark once: 75% is 15,000, 90% is 18,000.
    rig.gw.tasks.lock().unwrap().add_tokens("task-a", 14_700, 20_000);
    assert_eq!(status_of(&rig).await["task_cap_75"], 0, "150 + 14,700 is below 75%, and chat.manual did not count");
    rig.chat(&task_req(&rig, "task-a", Role::ResearchResearcher)).await;
    let s = status_of(&rig).await;
    assert_eq!((s["task_cap_75"].as_u64(), s["task_cap_90"].as_u64()), (Some(1), Some(0)));
    rig.chat(&task_req(&rig, "task-a", Role::ResearchResearcher)).await;
    assert_eq!(status_of(&rig).await["task_cap_75"], 1, "counted once");
    rig.gw.tasks.lock().unwrap().add_tokens("task-a", 3_000, 20_000);
    rig.gw.tasks.lock().unwrap().add_tokens("task-a", 2_000, 20_000);
    let s = status_of(&rig).await;
    assert_eq!((s["task_cap_75"].as_u64(), s["task_cap_90"].as_u64()), (Some(1), Some(1)));
    assert!(s.as_object().unwrap().keys().all(|k| ["config_version", "boot_id", "task_cap_75", "task_cap_90"].contains(&k.as_str())));
    assert!(!s.to_string().contains("task-a") && !s.to_string().contains("user1"));

    // At the cap: refused, no provider called, with its own code and a gate span. Another task is not.
    let calls = rig.provider_calls();
    let (status, body) = problem_of(rig.chat_raw(&task_req(&rig, "task-a", Role::ResearchResearcher)).await).await;
    assert_eq!((status, body["error"].as_str()), (402, Some("task_token_cap")));
    assert_eq!(rig.provider_calls(), calls);
    let rows =
        rig.wait_rows("SELECT kind, name, status, error_code FROM spans WHERE trace_id = 'task-a' AND name = 'task_token_cap' AND status != 'running'").await;
    assert_eq!(rows[0], vec![json!("gate"), json!("task_token_cap"), json!("blocked"), json!("task_token_cap")]);
    assert!(matches!(rig.chat(&task_req(&rig, "task-b", Role::ResearchResearcher)).await.last(), Some(ChatEvent::Done { .. })));
    // A non-task role on the same id is still served.
    assert!(matches!(rig.chat(&task_req(&rig, "task-a", Role::ChatManual)).await.last(), Some(ChatEvent::Done { .. })));
}

#[tokio::test]
async fn reserve_tool_call_refuses_past_the_pushed_limit_and_uses_the_default_before_a_push() {
    let rig = rig().await;
    for _ in 0..askbar_proto::research::DEFAULT_TOOL_CALLS {
        assert!(rig.gw.reserve_tool_call("t").is_ok());
    }
    assert_eq!(rig.gw.reserve_tool_call("t"), Err(askbar_proto::ErrorCode::TaskToolCalls));
    rig.gw.release_tool_call("t");
    assert!(rig.gw.reserve_tool_call("t").is_ok());
    let mut cfg = config(&rig, 1, &[("p", "m")]);
    cfg.task_limits.tool_calls = 1;
    assert_eq!(rig.push(&cfg).await.status(), 200);
    assert!(rig.gw.reserve_tool_call("u").is_ok());
    assert_eq!(rig.gw.reserve_tool_call("u"), Err(askbar_proto::ErrorCode::TaskToolCalls));
    assert!(rig.gw.reserve_tool_call("v").is_ok(), "tasks do not share");
}

// ------------------------------------------------------------------ research approvals (plan P5B.2)

impl Rig {
    async fn rs(&self, task: &str, user: &str, tool: &str, arguments: Value) -> (u16, Value) {
        let body = json!({"task_id": task, "user": user, "tool": tool, "arguments": arguments});
        let url = format!("{}/api/model-gateway/v1/approvals/research", self.server);
        problem_of(self.http.post(url).bearer_auth(SRV).json(&body).send().await.unwrap()).await
    }
}

#[tokio::test]
async fn research_refusals_have_their_own_codes_and_only_an_approval_uses_a_tool_call() {
    let (rig, _) = qs_rig(true, |c| {
        c.task_limits.tool_calls = 2;
        c.quick_search.max_query_chars = 20;
        c.quick_search_enabled = false; // the route does not look at it
    })
    .await;
    let long = json!({"query": QS_MARKER.repeat(3)});
    let cases = [
        ("shell", json!({"url": "https://a.example/"}), 400, "quick_search_tool"),
        ("search", long, 400, "quick_search_arguments"),
        ("fetch", json!({"query": "x"}), 400, "quick_search_arguments"),
    ];
    for (tool, args, status, code) in cases {
        let (s, b) = rig.rs("t1", "user1", tool, args).await;
        assert_eq!((s, b["error"].as_str()), (status, Some(code)), "{b}");
        assert!(b.get("approval").is_none());
    }
    for _ in 0..2 {
        assert_eq!(rig.rs("t1", "user1", "search", json!({"query": "a"})).await.0, 200, "refusals used no call");
    }
    let (s, b) = rig.rs("t1", "user1", "fetch", json!({"url": "https://a.example/"})).await;
    assert_eq!((s, b["error"].as_str()), (429, Some("task_tool_calls")));
    assert_eq!(rig.rs("t2", "user1", "search", json!({"query": "a"})).await.0, 200, "another task has its own limit");
    let rows = rig.wait_rows("SELECT kind, name, status, error_code FROM spans WHERE trace_id = 't1' AND status = 'blocked'").await;
    assert_eq!(rows[0], vec![json!("gate"), json!("research_approval"), json!("blocked"), json!("quick_search_tool")]);
    assert_eq!(rig.provider_calls(), 0);
}

#[tokio::test]
async fn research_is_refused_when_off_paused_or_killed_and_by_decide_too() {
    let (rig, _) = qs_rig(true, |c| c.research_enabled = false).await;
    let (s, b) = rig.rs("g1", "u1", "search", json!({"query": QS_MARKER})).await;
    assert_eq!((s, b["error"].as_str()), (503, Some("research_disabled")));
    let (s, b) = rig.judge("g1", "u1", r#"{"verdict":"approve"}"#).await;
    assert_eq!((s, b["error"].as_str()), (503, Some("research_disabled")));
    let rows = rig.wait_rows("SELECT attrs_json FROM spans WHERE trace_id = 'g1'").await;
    assert!(!format!("{rows:?}").contains(QS_MARKER));
    let (rig, _) = qs_rig(true, |c| c.paused_users = vec!["u1".into()]).await;
    assert_eq!(rig.rs("g1", "u1", "search", json!({"query": "a"})).await.1["error"], "user_paused");
    assert_eq!(rig.rs("g2", "u2", "search", json!({"query": "a"})).await.0, 200);
    let (rig, _) = qs_rig(true, |c| c.kill_switch = true).await;
    assert_eq!(rig.rs("g1", "u2", "search", json!({"query": "a"})).await.1["error"], "kill_switch");
}

#[tokio::test]
async fn a_research_approval_verifies_at_the_mcp_front_and_leaves_no_text() {
    use askbar_proto::approval::{Approval, Provenance, ToolAction};
    use askbar_service::approval::{verify, OneTimeIds};
    struct Any;
    impl OneTimeIds for Any {
        fn consume(&self, _: &str, _: i64) -> bool {
            true
        }
    }
    let (rig, public) = qs_rig(true, |_| {}).await;
    let now = askbar_service::audit_client::now_parts().0;
    for (tool, args) in
        [("search", json!({"query": QS_MARKER})), ("fetch", json!({"url": format!("https://Docs.Example:443/{QS_MARKER}?q=1")}))]
    {
        let (s, b) = rig.rs("task-ok", "user1", tool, args.clone()).await;
        assert_eq!(s, 200, "{b}");
        let a: Approval = serde_json::from_value(b["approval"].clone()).unwrap();
        let action = ToolAction {
            tool: tool.into(),
            arguments: args,
            destination: b["destination"].as_str().unwrap().into(),
            provenance: serde_json::from_value::<Provenance>(b["provenance"].clone()).unwrap(),
            query_data: b["query_data"].as_bool().unwrap(),
        };
        assert_eq!((action.provenance, action.query_data), (Provenance::Search, false));
        assert_eq!(action.destination, if tool == "fetch" { "docs.example" } else { "" });
        verify(&a, &action, &public, now, &Any).unwrap();
        assert_eq!(a.task_id, "task-ok");
        let rows = rig.rows(&format!(
            "SELECT attrs_json FROM spans WHERE trace_id = 'task-ok' AND name = 'research_approval' AND attrs_json LIKE '%{}%'",
            a.one_time_id
        ));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0][0].as_str().unwrap().contains(&a.action_hash));
    }
    let all = format!("{:?}", rig.rows("SELECT attrs_json, name FROM spans"));
    assert!(!all.contains(QS_MARKER) && !all.contains("docs.example"), "{all}");
}

#[tokio::test]
async fn a_research_approval_whose_span_is_not_acknowledged_signs_nothing_and_gives_the_call_back() {
    let (rig, _) = qs_rig(false, |c| c.task_limits.tool_calls = 1).await;
    for _ in 0..3 {
        let (s, b) = rig.rs("t1", "u1", "search", json!({"query": "a"})).await;
        assert_eq!((s, b["error"].as_str()), (503, Some("audit_backlog")), "{b}");
        assert!(b.get("approval").is_none());
    }
}

// ------------------------------------------------------------------ POST /decide, the judge (plan P5.6)

const JUDGE_MARKER: &str = "judge-marker-words";

impl Rig {
    /// A judge decision whose draft makes the mock answer `reply`.
    async fn judge(&self, task: &str, user: &str, reply: &str) -> (u16, Value) {
        let input = json!({"task_id": task, "user_id": user, "question": format!("what is {JUDGE_MARKER}?"),
            "summaries": [{"source": 1, "urls": ["https://e.example/"], "text": "summary one"}],
            "draft": format!("{JUDGE_MARKER} [Source 1]\nMOCK-REPLY {reply}")});
        self.decide(json!({"decision": "judge", "input": input, "generation_id": task, "user_id": user})).await
    }

    async fn decide(&self, body: Value) -> (u16, Value) {
        problem_of(
            self.http.post(format!("{}/api/model-gateway/v1/decide", self.server)).bearer_auth(SRV).json(&body).send().await.unwrap(),
        )
        .await
    }
}

async fn judge_rig(tweak: impl FnOnce(&mut GatewayConfig)) -> Rig {
    let rig = rig().await;
    let mut cfg = config(&rig, 1, &[("p", "m"), ("bad", "m-fail")]);
    cfg.role_pins.insert(Role::JudgeReview, "p/m".into());
    tweak(&mut cfg);
    assert_eq!(rig.push(&cfg).await.status(), 200);
    rig
}

#[tokio::test]
async fn the_judge_approves_or_refuses_with_each_reason_and_the_turn_span_holds_only_the_verdict() {
    let rig = judge_rig(|_| {}).await;
    let (s, b) = rig.judge("task-ok", "user1", r#"{"verdict":"approve"}"#).await;
    assert_eq!((s, b), (200, json!({"decision": "judge", "output": {"verdict": "approve"}})));
    for reason in ["unsupported_claim", "contradicts_source", "off_topic", "unsafe", "bad_citation"] {
        let task = format!("task-{reason}");
        let (s, b) = rig.judge(&task, "user1", &format!(r#"{{"verdict":"refuse","reason":"{reason}"}}"#)).await;
        assert_eq!((s, &b["output"]), (200, &json!({"verdict": "refuse", "reason": reason})));
        let rows = rig
            .wait_rows(&format!("SELECT attrs_json, status FROM spans WHERE trace_id = '{task}' AND kind = 'turn' AND status = 'done'"))
            .await;
        let attrs: Value = serde_json::from_str(rows[0][0].as_str().unwrap()).unwrap();
        assert_eq!((&attrs["role"], &attrs["verdict"], &attrs["reason"]), (&json!("judge.review"), &json!("refuse"), &json!(reason)));
    }
    let rows = rig.wait_rows("SELECT attrs_json FROM spans WHERE trace_id = 'task-ok' AND kind = 'turn' AND status = 'done'").await;
    let attrs: Value = serde_json::from_str(rows[0][0].as_str().unwrap()).unwrap();
    assert_eq!((&attrs["role"], &attrs["verdict"], attrs.get("reason")), (&json!("judge.review"), &json!("approve"), None));
}

#[tokio::test]
async fn an_answer_off_the_fixed_format_is_a_refusal_with_bad_format() {
    let rig = judge_rig(|_| {}).await;
    for (i, reply) in [
        r#"Sure. {"verdict":"approve"}"#,
        r#"{"verdict":"approve"} thanks"#,
        r#"{"verdict":"approve","note":"x"}"#,
        r#"{"verdict":"approve","reason":"off_topic"}"#,
        r#"{"verdict":"refuse","reason":"rude"}"#,
        r#"{"verdict":"refuse"}"#,
        r#"{"verdict":"maybe"}"#,
        "approve",
        "",
    ]
    .iter()
    .enumerate()
    {
        let (s, b) = rig.judge(&format!("task-bad{i}"), "user1", reply).await;
        assert_eq!((s, &b["output"]), (200, &json!({"verdict": "refuse", "reason": "bad_format"})), "{reply:?}");
    }
}

#[tokio::test]
async fn an_unpinned_judge_is_role_not_pinned_and_no_provider_is_called() {
    let rig = judge_rig(|c| {
        c.role_pins.clear();
    })
    .await;
    let (s, b) = rig.judge("task-np", "user1", r#"{"verdict":"approve"}"#).await;
    assert_eq!((s, b["error"].as_str()), (400, Some("role_not_pinned")));
    assert_eq!(rig.provider_calls(), 0);
    rig.wait_rows("SELECT 1 FROM spans WHERE trace_id = 'task-np' AND kind = 'gate' AND error_code = 'role_not_pinned'").await;
}

#[tokio::test]
async fn a_provider_error_an_over_cap_task_a_pause_and_the_kill_switch_are_problems_not_verdicts() {
    let rig = judge_rig(|c| {
        c.role_pins.insert(Role::JudgeReview, "bad/m-fail".into());
    })
    .await;
    let (s, b) = rig.judge("task-pe", "user1", r#"{"verdict":"approve"}"#).await;
    assert!(s >= 400 && b.get("output").is_none() && b.get("verdict").is_none(), "{s} {b}");
    rig.wait_rows("SELECT 1 FROM spans WHERE trace_id = 'task-pe' AND kind = 'turn' AND status = 'error'").await;

    let rig = judge_rig(|c| c.task_limits.token_cap = 20_000).await;
    rig.gw.tasks.lock().unwrap().add_tokens("task-cap", 20_000, 20_000);
    let (s, b) = rig.judge("task-cap", "user1", r#"{"verdict":"approve"}"#).await;
    assert_eq!((s, b["error"].as_str()), (402, Some("task_token_cap")));
    assert_eq!(rig.provider_calls(), 0);

    let rig = judge_rig(|c| c.paused_users.push("user1".into())).await;
    let (s, b) = rig.judge("task-p", "user1", r#"{"verdict":"approve"}"#).await;
    assert_eq!((s, b["error"].as_str()), (403, Some("user_paused")));
    let rig = judge_rig(|c| c.kill_switch = true).await;
    let (s, b) = rig.judge("task-k", "user1", r#"{"verdict":"approve"}"#).await;
    assert_eq!((s, b["error"].as_str()), (503, Some("kill_switch")));
    assert_eq!(rig.provider_calls(), 0);
}

#[tokio::test]
async fn other_decisions_and_bad_inputs_are_refused_before_any_call() {
    let rig = judge_rig(|_| {}).await;
    for decision in ["step_check", "research"] {
        let (s, b) = rig.decide(json!({"decision": decision, "input": {}, "generation_id": "t", "user_id": "user1"})).await;
        assert_eq!((s, b["error"].as_str()), (400, Some("decision_unsupported")));
    }
    let ok = json!({"task_id": "t", "user_id": "user1", "question": "q", "summaries": [], "draft": "d"});
    let mut extra = ok.clone();
    extra["x"] = json!(1);
    let mut huge = ok.clone();
    huge["draft"] = json!("x".repeat(300 * 1024));
    let mut other = ok.clone();
    other["task_id"] = json!("u");
    for (input, gen) in [(extra, "t"), (huge, "t"), (other, "t"), (json!("text"), "t")] {
        let (s, b) = rig.decide(json!({"decision": "judge", "input": input, "generation_id": gen, "user_id": "user1"})).await;
        assert_eq!((s, b["error"].as_str()), (400, Some("bad_request")));
    }
    let (s, _) = rig.decide(json!({"decision": "judge", "input": ok})).await;
    assert_eq!(s, 400, "a judge decision needs its ids");
    assert_eq!(rig.provider_calls(), 0);
}

#[tokio::test]
async fn the_judges_text_is_only_in_the_sealed_payloads() {
    let rig = judge_rig(|_| {}).await;
    let (s, _) = rig.judge("task-seal", "user1", r#"{"verdict":"approve"}"#).await;
    assert_eq!(s, 200);
    let spans = rig.wait_rows("SELECT id FROM spans WHERE trace_id = 'task-seal' AND kind = 'turn' AND status = 'done'").await;
    let sql = format!("SELECT ciphertext FROM payloads WHERE span_id = '{}' AND role = 'prompt'", spans[0][0].as_str().unwrap());
    rig.wait_rows(&sql).await;
    let blob: Vec<u8> = rig.auditor.recorder.reader().unwrap().query_row(&sql, [], |r| r.get(0)).unwrap();
    let prompt = String::from_utf8(crypto_box::SecretKey::from(AUDIT_SECRET).unseal(&blob).unwrap()).unwrap();
    assert!(prompt.contains(JUDGE_MARKER) && prompt.contains("summary one") && prompt.contains("[Source 1]"), "{prompt}");
    let all = format!("{:?}", rig.rows("SELECT attrs_json, name, error_code FROM spans"));
    assert!(!all.contains(JUDGE_MARKER) && !all.contains("summary one"), "{all}");
    let status = rig.http.get(format!("{}/api/model-gateway/v1/status", rig.admin)).bearer_auth(ADM).send().await.unwrap();
    assert!(!status.text().await.unwrap().contains(JUDGE_MARKER));
}
