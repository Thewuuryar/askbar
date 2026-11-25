//! `POST /tools/{tool}/call`: the signed approval is checked first, each failure has its own code and is counted, a valid
//! approval reaches the next check (a 501 for now), and one approval raced by many calls lets exactly one through.

use askbar_mcp_gateway::registry::{handler, Registry};
use askbar_mcp_gateway::{admin_links, admin_router, server_links, server_router, Gateway};
use askbar_proto::approval::{Provenance, ToolAction};
use askbar_proto::ErrorCode;
use askbar_service::approval::{generate_keypair, sign};
use askbar_service::audit_client::AuditClient;
use askbar_service::status::BootId;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};

const ADMIN: &str = "admmcp_secret";
const SERVER: &str = "srvmcp_secret";

/// What the stand-in auditor was asked: path, body and how many times a tool had run when it was asked.
type Seen = Arc<Mutex<Vec<(String, Value, u64)>>>;

#[derive(Clone, Copy, PartialEq)]
enum Auditor {
    Up,
    /// Answers `503` to a span start, as a full backlog does.
    Busy,
    /// Nothing listens.
    Down,
    /// No audit client at all (a native run).
    None,
}

struct Rig {
    seen: Seen,
    url: String,
    admin_url: String,
    g: Arc<Gateway>,
    sk: Vec<u8>,
    pk: Vec<u8>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

async fn fake_auditor(mode: Auditor, gw: Arc<OnceLock<Arc<Gateway>>>, seen: Seen) -> String {
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let (gw, seen) = (gw.clone(), seen.clone());
        async move {
            let path = req.uri().path().to_string();
            let body = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
            let runs = gw.get().map_or(0, |g| g.tool_runs.load(Ordering::SeqCst));
            seen.lock().unwrap().push((path.clone(), serde_json::from_slice(&body).unwrap_or(Value::Null), runs));
            let status = if mode == Auditor::Busy && path.ends_with("/spans") { 503 } else { 200 };
            (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(json!({"interrupted": 0})))
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    if mode != Auditor::Down {
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    }
    url
}

async fn start(push_key: bool) -> Rig {
    start_with(push_key, Auditor::Up).await
}

async fn start_with(push_key: bool, mode: Auditor) -> Rig {
    // Both tools are stubs that answer 501: these tests are about the checks before a tool runs, not the tools.
    let stub = || handler(|_| async { Err(ErrorCode::ToolNotAvailable.into()) });
    let query = json!({"type": "object", "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 200}}});
    let url = json!({"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}});
    let stub = Registry::new([("search", query, stub()), ("fetch", url, stub())]);
    start_full(push_key, mode, stub, 30).await
}

async fn start_full(push_key: bool, mode: Auditor, registry: Registry, timeout_secs: u32) -> Rig {
    let dir = std::env::temp_dir().join(format!("askbar-mcp-call-{}", BootId::new().as_str()));
    std::fs::create_dir_all(dir.join("accept")).unwrap();
    for (name, t) in [("admin-mcp-gateway", ADMIN), ("server-mcp-gateway", SERVER)] {
        std::fs::write(dir.join("accept").join(format!("{name}.sha256")), format!("{}\n", hex(&Sha256::digest(t.as_bytes())))).unwrap();
    }
    std::fs::create_dir_all(dir.join("call")).unwrap();
    std::fs::write(dir.join("call/mcp-gateway-auditor.token"), "mcpaud_secret\n").unwrap();
    let (cell, seen): (Arc<OnceLock<Arc<Gateway>>>, Seen) = Default::default();
    let audit_url = fake_auditor(mode, cell.clone(), seen.clone()).await;
    let audit = (mode != Auditor::None).then(|| AuditClient::new(&audit_url, dir.clone(), "mcp-gateway-auditor"));
    let boot = BootId::new();
    let g = Gateway::with_registry(boot.clone(), None, audit.clone(), registry);
    cell.set(g.clone()).ok();
    if let Some(a) = &audit {
        tokio::spawn(a.clone().hello_until_done(boot.as_str().to_string()));
        tokio::spawn(a.clone().drain_forever());
        while !a.ready() && mode != Auditor::Down {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
    let serve = |app: axum::Router| async move {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/mcp-gateway/v1", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    };
    let url = serve(server_router(g.clone(), server_links(&dir))).await;
    let admin_url = serve(admin_router(g.clone(), admin_links(&dir))).await;
    let (sk, pk) = generate_keypair().unwrap();
    let rig = Rig { seen, url, admin_url, g, sk, pk };
    if push_key {
        let cfg = json!({"fetch_max_bytes": 1048576, "fetch_timeout_secs": 20, "fetch_redirects": 3, "tool_call_timeout_secs": timeout_secs,
            "one_time_id_window_secs": 120, "search_results": 8, "remembered_urls": 50,
            "approval_public_key": hex(&rig.pk), "approval_key_version": 1,
            "permissions": {"alice": {"allowed": ["fetch", "search"]}, "bob": {"allowed": ["fetch"], "denied": ["fetch"]}}});
        let r = reqwest::Client::new().put(format!("{}/config", rig.admin_url)).bearer_auth(ADMIN).json(&cfg).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }
    rig
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn action(url: &str) -> ToolAction {
    ToolAction {
        tool: "fetch".into(),
        arguments: json!({"url": url}),
        destination: "example.com".into(),
        provenance: Provenance::User,
        query_data: false,
    }
}

/// The body for `url`, approved with `sk`.
fn body(rig_sk: &[u8], signed_for: &str, sent: &str, id: &str, expires_at: i64) -> Value {
    body_for("alice", rig_sk, signed_for, sent, id, expires_at)
}

fn body_for(user: &str, rig_sk: &[u8], signed_for: &str, sent: &str, id: &str, expires_at: i64) -> Value {
    let a = sign(&action(signed_for), "task1", user, id, expires_at, rig_sk).unwrap();
    json!({"arguments": {"url": sent}, "destination": "example.com", "provenance": "user", "approval": a})
}

async fn post(rig: &Rig, token: &str, body: &Value) -> (u16, Value) {
    let r = reqwest::Client::new().post(format!("{}/tools/fetch/call", rig.url)).bearer_auth(token).json(body).send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
}

fn code(r: &(u16, Value)) -> (u16, &str) {
    (r.0, r.1["error"].as_str().unwrap_or(""))
}

#[tokio::test]
async fn each_bad_approval_is_refused_with_its_own_code_and_counted() {
    let rig = start(true).await;
    let u = "https://example.com/a";
    let mut unsigned = body(&rig.sk, u, u, "o1", now() + 60);
    unsigned["approval"]["signature"] = json!("");
    let swapped = body(&rig.sk, u, "https://example.com/other", "o2", now() + 60);
    let expired = body(&rig.sk, u, u, "o3", now() - 1);
    let (other_sk, _) = generate_keypair().unwrap();
    let wrong_key = body(&other_sk, u, u, "o4", now() + 60);
    let mut floaty = body(&rig.sk, u, u, "o5", now() + 60);
    floaty["arguments"] = json!({"url": u, "n": 1.5});

    assert_eq!(code(&post(&rig, SERVER, &unsigned).await), (403, "approval_unsigned"));
    assert_eq!(code(&post(&rig, SERVER, &swapped).await), (403, "approval_action_swapped"));
    assert_eq!(code(&post(&rig, SERVER, &expired).await), (403, "approval_expired"));
    assert_eq!(code(&post(&rig, SERVER, &wrong_key).await), (403, "approval_signature_invalid"));
    assert_eq!(code(&post(&rig, SERVER, &floaty).await), (400, "approval_unsignable"));
    assert_eq!(code(&post(&rig, SERVER, &json!({"arguments": {}})).await), (400, "bad_request"));

    let ok = body(&rig.sk, u, u, "o6", now() + 60);
    assert_eq!(code(&post(&rig, SERVER, &ok).await), (501, "tool_not_available"), "a valid approval reaches the next check");
    assert_eq!(code(&post(&rig, SERVER, &ok).await), (409, "approval_replayed"));

    let st = rig.g.status().calls;
    for (k, n) in [
        ("refused_approval_unsigned", 1),
        ("refused_approval_action_swapped", 1),
        ("refused_approval_expired", 1),
        ("refused_approval_signature_invalid", 1),
        ("refused_approval_unsignable", 1),
        ("refused_approval_replayed", 1),
        ("refused_tool_not_available", 1),
    ] {
        assert_eq!(st.get(k), Some(&n), "{k}");
    }
    let text = serde_json::to_string(&st).unwrap();
    let (pk, sig) = (hex(&rig.pk), ok["approval"]["signature"].as_str().unwrap());
    for secret in [u, "example.com", "o6", &pk, sig] {
        assert!(!text.contains(secret), "status leaked {secret}");
    }
}

#[tokio::test]
async fn fifty_racing_calls_with_one_approval_let_exactly_one_through() {
    let rig = Arc::new(start(true).await);
    let u = "https://example.com/race";
    let b = Arc::new(body(&rig.sk, u, u, "race", now() + 60));
    let tasks: Vec<_> = (0..50)
        .map(|_| {
            let (rig, b) = (rig.clone(), b.clone());
            tokio::spawn(async move { post(&rig, SERVER, &b).await })
        })
        .collect();
    let mut codes = Vec::new();
    for t in tasks {
        let r = t.await.unwrap();
        codes.push((r.0, r.1["error"].as_str().unwrap().to_string()));
    }
    assert_eq!(codes.iter().filter(|c| c.0 == 501).count(), 1, "exactly one passes to the next check");
    assert_eq!(codes.iter().filter(|c| **c == (409, "approval_replayed".to_string())).count(), 49);
}

#[tokio::test]
async fn the_admin_link_gets_401_and_a_call_without_config_or_key_is_refused() {
    let rig = start(true).await;
    let u = "https://example.com/a";
    let b = body(&rig.sk, u, u, "adm", now() + 60);
    assert_eq!(code(&post(&rig, ADMIN, &b).await), (401, "unauthorized"));
    assert_eq!(code(&post(&rig, "nope", &b).await), (401, "unauthorized"));

    let none = start(false).await;
    assert_eq!(code(&post(&none, SERVER, &b).await), (503, "not_configured"), "before the first push");
    let cfg = json!({"fetch_max_bytes": 1048576, "fetch_timeout_secs": 20, "fetch_redirects": 3, "tool_call_timeout_secs": 30,
        "one_time_id_window_secs": 120, "search_results": 8, "remembered_urls": 50});
    let r = reqwest::Client::new().put(format!("{}/config", none.admin_url)).bearer_auth(ADMIN).json(&cfg).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(code(&post(&none, SERVER, &b).await), (503, "not_configured"), "a config without a key still fails closed");
    let bad = json!({"approval_public_key": "zz"});
    let r = reqwest::Client::new().put(format!("{}/config", none.admin_url)).bearer_auth(ADMIN).json(&bad).send().await.unwrap();
    assert_eq!(r.status(), 400);
}

/// The span paths and the tool-run count at the time of the last start, once the finish has arrived.
async fn settled(rig: &Rig) -> Vec<(String, Value, u64)> {
    for _ in 0..100 {
        if rig.seen.lock().unwrap().iter().any(|(p, _, _)| p.ends_with("/finish")) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    rig.seen.lock().unwrap().iter().filter(|(p, _, _)| p.contains("/spans")).cloned().collect()
}

#[tokio::test]
async fn the_span_is_acknowledged_before_the_tool_step_and_closed_with_the_outcome_and_holds_no_argument() {
    let rig = start(true).await;
    let u = "https://example.com/marker-argument";
    let b = body(&rig.sk, u, u, "span1", now() + 60);
    assert_eq!(code(&post(&rig, SERVER, &b).await), (501, "tool_not_available"));
    assert_eq!(rig.g.tool_runs.load(Ordering::SeqCst), 1);

    let spans = settled(&rig).await;
    let paths: Vec<&str> = spans.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(paths[0], "/api/audit/v1/spans");
    assert!(paths[1].ends_with("/finish"), "{paths:?}");
    assert_eq!(spans[0].2, 0, "the tool had not run when the auditor was asked");
    let (start, finish) = (&spans[0].1, &spans[1].1);
    assert_eq!((start["kind"].as_str(), start["name"].as_str()), (Some("tool"), Some("fetch")));
    assert_eq!((start["user_id"].as_str(), start["trace_id"].as_str()), (Some("alice"), Some("task1")));
    assert_eq!(start["attrs"]["one_time_id"], "span1");
    assert_eq!(start["attrs"]["action_hash"], b["approval"]["action_hash"]);
    assert_eq!((finish["status"].as_str(), finish["error_code"].as_str()), (Some("error"), Some("tool_not_available")));
    let wire = format!("{start}{finish}");
    for secret in ["marker-argument", "example.com", b["approval"]["signature"].as_str().unwrap(), &hex(&rig.pk)] {
        assert!(!wire.contains(secret), "the span leaked {secret}");
    }
}

#[tokio::test]
async fn an_auditor_that_does_not_acknowledge_refuses_the_call_and_the_tool_never_runs() {
    for mode in [Auditor::Busy, Auditor::Down, Auditor::None] {
        let rig = start_with(true, mode).await;
        let u = "https://example.com/a";
        let r = post(&rig, SERVER, &body(&rig.sk, u, u, "noaudit", now() + 60)).await;
        assert_eq!(code(&r), (503, "audit_backlog"));
        assert_eq!(rig.g.tool_runs.load(Ordering::SeqCst), 0, "the tool did not run");
        assert_eq!(rig.g.status().calls.get("refused_audit_unavailable"), Some(&1));
        assert!(!rig.g.status().calls.contains_key("refused_tool_not_available"));
    }
}

#[tokio::test]
async fn a_tool_the_user_does_not_hold_is_refused_before_any_span_and_nothing_runs() {
    let rig = start(true).await;
    let u = "https://example.com/a";
    // bob lists fetch in both lists (deny wins); carol has no entry (holds nothing).
    for (user, id) in [("bob", "p1"), ("carol", "p2")] {
        let b = body_for(user, &rig.sk, u, u, id, now() + 60);
        assert_eq!(code(&post(&rig, SERVER, &b).await), (403, "tool_not_permitted"), "{user}");
    }
    // The approval check still comes first: a bad approval for a tool alice holds, and for one carol does not.
    let mut bad = body_for("carol", &rig.sk, u, u, "p3", now() + 60);
    bad["approval"]["signature"] = json!("");
    assert_eq!(code(&post(&rig, SERVER, &bad).await), (403, "approval_unsigned"));
    assert_eq!(
        code(&post(&rig, SERVER, &body(&rig.sk, u, "https://example.com/x", "p4", now() + 60)).await),
        (403, "approval_action_swapped")
    );
    assert_eq!(rig.g.status().calls.get("refused_permission"), Some(&2));
    assert_eq!(rig.g.tool_runs.load(Ordering::SeqCst), 0);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(rig.seen.lock().unwrap().iter().all(|(p, _, _)| !p.contains("/spans")), "no span was opened");
    // alice holds fetch and still reaches the next step.
    assert_eq!(code(&post(&rig, SERVER, &body(&rig.sk, u, u, "p5", now() + 60)).await), (501, "tool_not_available"));
}

#[tokio::test]
async fn the_list_shows_only_the_tools_the_user_holds() {
    let rig = start(true).await;
    let list = |user: &'static str| {
        let url = format!("{}/tools", rig.url);
        async move {
            let r = reqwest::Client::new().get(url).query(&[("user", user)]).bearer_auth(SERVER).send().await.unwrap();
            (r.status().as_u16(), r.json::<Value>().await.unwrap())
        }
    };
    assert_eq!(
        list("alice").await,
        (
            200,
            json!({"tools": [
        {"name": "search", "schema": {"type": "object", "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 200}}}},
        {"name": "fetch", "schema": {"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}}}]})
        )
    );
    assert_eq!(list("bob").await.1, json!({"tools": []}), "deny wins");
    assert_eq!(list("carol").await.1, json!({"tools": []}), "no entry, nothing held");
    let r = reqwest::Client::new().get(format!("{}/tools?user=alice", rig.url)).bearer_auth(ADMIN).send().await.unwrap();
    assert_eq!(r.status(), 401, "the admin's token is not the server's");
    let bad = json!({"permissions": {"a": {"allowed": ["shell"]}}});
    let r = reqwest::Client::new().put(format!("{}/config", rig.admin_url)).bearer_auth(ADMIN).json(&bad).send().await.unwrap();
    assert_eq!(r.status(), 400, "an unknown tool is refused at the push");
}

/// A registry whose `fetch` is the given handler (`search` is not registered).
fn fetch_only(h: askbar_mcp_gateway::registry::Handler) -> Registry {
    Registry::new([("fetch", json!({"type": "object"}), h)])
}

/// The finish the auditor was sent: `(status, error_code)`.
async fn finished(rig: &Rig) -> (Option<String>, Option<String>) {
    let spans = settled(rig).await;
    let f = &spans.iter().find(|s| s.0.ends_with("/finish")).expect("a finish").1;
    (f["status"].as_str().map(Into::into), f["error_code"].as_str().map(Into::into))
}

#[tokio::test]
async fn an_approved_permitted_call_runs_the_handler_once_and_returns_its_result_untrusted() {
    let rig = start_full(true, Auditor::Up, fetch_only(handler(|a| async move { Ok(json!({"echo": a.arguments["url"]})) })), 30).await;
    let u = "https://example.com/ok";
    let r = post(&rig, SERVER, &body(&rig.sk, u, u, "run1", now() + 60)).await;
    assert_eq!(r, (200, json!({"result": {"echo": u}, "untrusted": true})));
    assert_eq!(rig.g.tool_runs.load(Ordering::SeqCst), 1);
    assert_eq!(finished(&rig).await, (Some("ok".into()), None));
    let spans = settled(&rig).await;
    assert!(!format!("{}", spans[1].1).contains("example.com"), "the result is not in the span");
    assert_eq!(rig.g.status().calls.get("tool_ok"), Some(&1));
}

#[tokio::test]
async fn a_handler_that_fails_closes_the_span_as_error() {
    let rig = start_full(true, Auditor::Up, fetch_only(handler(|_| async { Err(ErrorCode::Internal.into()) })), 30).await;
    let u = "https://example.com/e";
    assert_eq!(code(&post(&rig, SERVER, &body(&rig.sk, u, u, "err1", now() + 60)).await), (500, "internal"));
    assert_eq!(finished(&rig).await, (Some("error".into()), Some("internal".into())));
    assert_eq!(rig.g.status().calls.get("tool_error"), Some(&1));
}

#[tokio::test]
async fn a_handler_past_the_time_limit_gives_tool_timeout_and_a_timeout_span() {
    let slow = handler(|_| async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        Ok(json!(1))
    });
    let rig = start_full(true, Auditor::Up, fetch_only(slow), 5).await; // 5 s is the lowest limit a push may set
    let u = "https://example.com/slow";
    assert_eq!(code(&post(&rig, SERVER, &body(&rig.sk, u, u, "slow1", now() + 60)).await), (504, "tool_timeout"));
    assert_eq!(finished(&rig).await, (Some("timeout".into()), Some("tool_timeout".into())));
    assert_eq!(rig.g.status().calls.get("tool_timeout"), Some(&1));
}

#[tokio::test]
async fn an_unregistered_tool_is_refused_before_any_span_and_is_not_listed() {
    let rig = start_full(true, Auditor::Up, fetch_only(handler(|_| async { Ok(json!(1)) })), 30).await;
    let u = "https://example.com/a";
    let mut b = body(&rig.sk, u, u, "unreg1", now() + 60);
    // The approval is for `search`, which alice holds but this registry lacks.
    let a = sign(&ToolAction { tool: "search".into(), ..action(u) }, "task1", "alice", "unreg1", now() + 60, &rig.sk).unwrap();
    b["approval"] = json!(a);
    let r = reqwest::Client::new().post(format!("{}/tools/search/call", rig.url)).bearer_auth(SERVER).json(&b).send().await.unwrap();
    assert_eq!(r.status(), 501);
    assert_eq!(rig.g.tool_runs.load(Ordering::SeqCst), 0);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(rig.seen.lock().unwrap().iter().all(|(p, _, _)| !p.contains("/spans")), "no span was opened");
    let r = reqwest::Client::new().get(format!("{}/tools?user=alice", rig.url)).bearer_auth(SERVER).send().await.unwrap();
    assert_eq!(r.json::<Value>().await.unwrap(), json!({"tools": [{"name": "fetch", "schema": {"type": "object"}}]}));
}

#[tokio::test]
async fn no_refusal_path_runs_the_handler() {
    let counted = || fetch_only(handler(|_| async { Ok(json!(1)) }));
    let u = "https://example.com/a";
    let rig = start_full(true, Auditor::Up, counted(), 30).await;
    let mut unsigned = body(&rig.sk, u, u, "n1", now() + 60);
    unsigned["approval"]["signature"] = json!("");
    assert_eq!(code(&post(&rig, SERVER, &unsigned).await).1, "approval_unsigned");
    assert_eq!(code(&post(&rig, SERVER, &body_for("bob", &rig.sk, u, u, "n2", now() + 60)).await).1, "tool_not_permitted");
    assert_eq!(rig.g.tool_runs.load(Ordering::SeqCst), 0);
    let down = start_full(true, Auditor::Busy, counted(), 30).await;
    assert_eq!(code(&post(&down, SERVER, &body(&down.sk, u, u, "n3", now() + 60)).await).1, "audit_backlog");
    assert_eq!(down.g.tool_runs.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_oversize_result_is_not_returned_and_closes_the_span_as_error() {
    // The default limit is 1 MiB; the config in the rig does not push one.
    let big = handler(|_| async { Ok(json!("x".repeat(1024 * 1024 + 1))) });
    let rig = start_full(true, Auditor::Up, fetch_only(big), 30).await;
    let u = "https://example.com/big";
    let r = post(&rig, SERVER, &body(&rig.sk, u, u, "big1", now() + 60)).await;
    assert_eq!(code(&r), (502, "tool_result_too_large"));
    assert!(!r.1.to_string().contains("xxxx"), "the result is not in the answer");
    assert_eq!(finished(&rig).await, (Some("error".into()), Some("tool_result_too_large".into())));
    assert_eq!(rig.g.status().calls.get("tool_error"), Some(&1));
}

// ---- the native search tool (P5A.14) ----

/// A stand-in `searxng`: what it was sent (content type, URI query, form body), and an answer chosen by the query `q`.
type Sent = Arc<Mutex<Vec<(String, String, String)>>>;

async fn fake_searxng() -> (String, Sent) {
    let sent: Sent = Default::default();
    let s2 = sent.clone();
    let app = axum::Router::new().route(
        "/search",
        axum::routing::post(move |h: axum::http::HeaderMap, uri: axum::http::Uri, b: String| {
            let s2 = s2.clone();
            async move {
                let ct = h.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                s2.lock().unwrap().push((ct, uri.query().unwrap_or("").to_string(), b.clone()));
                if b.contains("q=boom") {
                    return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom".to_string());
                }
                if b.contains("q=garbage") {
                    return (axum::http::StatusCode::OK, "<html>not json".to_string());
                }
                if b.contains("q=slow") {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
                let mut results = vec![json!({"title": "bad", "url": "javascript:alert(1)", "content": "x"})];
                results.extend(
                    (0..12).map(|i| json!({"title": "t".repeat(300), "url": format!("https://r.example/{i}"), "content": "c".repeat(900)})),
                );
                (axum::http::StatusCode::OK, json!({"results": results}).to_string())
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (addr, sent)
}

async fn search(rig: &Rig, task: &str, query: &str, id: &str) -> (u16, Value) {
    let args = json!({"query": query});
    let act = ToolAction {
        tool: "search".into(),
        arguments: args.clone(),
        destination: "searxng".into(),
        provenance: Provenance::User,
        query_data: false,
    };
    let a = sign(&act, task, "alice", id, now() + 60, &rig.sk).unwrap();
    let body = json!({"arguments": args, "destination": "searxng", "provenance": "user", "approval": a});
    let r = reqwest::Client::new().post(format!("{}/tools/search/call", rig.url)).bearer_auth(SERVER).json(&body).send().await.unwrap();
    (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
async fn search_returns_at_most_n_trimmed_untrusted_results_sent_as_a_post_form() {
    let (addr, sent) = fake_searxng().await;
    let rig = start_full(true, Auditor::Up, Registry::builtin(Some(addr)), 30).await;
    let secret_query = "very private question";
    let (st, v) = search(&rig, "task-a", secret_query, "s1").await;
    assert_eq!(st, 200);
    assert_eq!(v["untrusted"], true);
    let results = v["result"]["results"].as_array().unwrap();
    assert_eq!(results.len(), 8, "the configured count, and the javascript: result is dropped");
    assert_eq!(results[0]["url"], "https://r.example/0");
    assert_eq!(results[0]["title"].as_str().unwrap().chars().count(), 200);
    assert_eq!(results[0]["snippet"].as_str().unwrap().chars().count(), 500);

    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].0.starts_with("application/x-www-form-urlencoded"));
    assert_eq!(sent[0].1, "", "the query is in no URL");
    assert!(sent[0].2.contains("q=very+private+question") && sent[0].2.contains("format=json"));

    let (st, _) = search(&rig, "task-a", "again", "s2").await;
    assert_eq!(st, 200);
    let text = serde_json::to_string(&rig.g.status().calls).unwrap();
    assert!(!text.contains(secret_query) && !text.contains("r.example"), "status leaked search content");
    let last = rig.g.status().last_search.expect("a search was made");
    assert_eq!(last.outcome, "ok");
    assert!(!serde_json::to_string(&rig.g.status()).unwrap().contains(secret_query), "the status holds no query text");
}

#[tokio::test]
async fn search_failures_are_typed_and_a_long_query_is_refused() {
    let (addr, _) = fake_searxng().await;
    let rig = start_full(true, Auditor::Up, Registry::builtin(Some(addr)), 5).await;
    assert!(rig.g.status().last_search.is_none(), "no search yet");
    for (q, id) in [("boom", "e1"), ("garbage", "e2"), ("slow", "e3")] {
        let r = search(&rig, "t", q, id).await;
        assert_eq!(code(&r), (502, "tool_upstream_failed"), "{q}");
        let last = rig.g.status().last_search.expect("the failure is recorded");
        assert_eq!(last.outcome, "tool_upstream_failed", "{q}");
        assert!(!serde_json::to_string(&last).unwrap().contains(q) || q == "slow", "no query text in the status");
        assert!(!r.1.to_string().contains(q) || q == "slow", "the answer repeats the query");
    }
    assert_eq!(code(&search(&rig, "t", &"x".repeat(201), "e4").await), (400, "bad_request"));
    assert_eq!(code(&search(&rig, "t", "  ", "e5").await), (400, "bad_request"));
    assert_eq!(search(&rig, "t", &"x".repeat(200), "e6").await.0, 200);
    assert_eq!(rig.g.status().last_search.unwrap().outcome, "ok", "the last search decides");
}

#[tokio::test]
async fn search_without_searxng_is_not_available() {
    let rig = start_full(true, Auditor::Up, Registry::builtin(None), 30).await;
    assert_eq!(code(&search(&rig, "t", "q", "n1").await), (501, "tool_not_available"));
}
