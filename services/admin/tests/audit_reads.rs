//! The admin's Audit pages read the auditor (ADR 0051): they need a session, forward only the query parameters they
//! know with the admin's own read token, price the figures with the admin's prices, and say so when the auditor is away.
//! The auditor here is a stub that records what it was asked.

mod common;
use askbar_admin::auditor_link::AuditorLink;
use askbar_admin::{server_link::ServerLink, web, AdminDb, AdminState};
use axum::{extract::State, http::StatusCode, routing::get, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Seen {
    /// (method, path, query, authorization header) of every request the stub got.
    requests: Mutex<Vec<(String, String, String, String)>>,
    /// The retention the stub auditor holds.
    retention: Mutex<u32>,
}

type Req = (State<Arc<Seen>>, axum::http::Method, axum::http::Uri, axum::http::HeaderMap);

fn record(seen: &Seen, m: &axum::http::Method, uri: &axum::http::Uri, h: &axum::http::HeaderMap) {
    seen.requests.lock().unwrap().push((
        m.to_string(),
        uri.path().to_string(),
        uri.query().unwrap_or("").to_string(),
        h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string(),
    ));
}

/// A stub auditor: one trace `t1` served by provider `p1` (1M tokens in, 250k out), stats with the day and provider
/// breakdown, and a retention it keeps.
async fn auditor_stub() -> (String, Arc<Seen>) {
    let seen = Arc::new(Seen { retention: Mutex::new(30), ..Default::default() });
    let app = Router::new()
        .route(
            "/api/audit/v1/traces",
            get(|(State(s), m, uri, h): Req| async move {
                record(&s, &m, &uri, &h);
                axum::Json(json!([{ "trace_id": "t1", "status": "done", "provider_id": "p1", "tokens_in": 1_000_000, "tokens_out": 250_000, "tokens_cached": 0 }]))
            }),
        )
        .route(
            "/api/audit/v1/traces/{id}",
            get(|(State(s), m, uri, h): Req| async move {
                record(&s, &m, &uri, &h);
                if uri.path().ends_with("/missing") {
                    return Err(StatusCode::NOT_FOUND);
                }
                if uri.path().ends_with("/empty") {
                    return Ok(axum::Json(json!([])));
                }
                Ok(axum::Json(json!([
                    { "id": "s1", "kind": "turn", "provider_id": null, "tokens_in": 1_000_000, "tokens_out": 250_000, "tokens_cached": 0 },
                    { "id": "s2", "kind": "attempt", "provider_id": "p1", "tokens_in": null, "tokens_out": null, "tokens_cached": null }
                ])))
            }),
        )
        .route(
            "/api/audit/v1/spans/{id}/payloads",
            get(|(State(s), m, uri, h): Req| async move {
                record(&s, &m, &uri, &h);
                axum::Json(json!([]))
            }),
        )
        .route(
            "/api/audit/v1/stats",
            get(|(State(s), m, uri, h): Req| async move {
                record(&s, &m, &uri, &h);
                axum::Json(json!({
                    "requests": 1, "cost": null, "cost_unknown_requests": 0,
                    "by_provider": [{ "provider_id": "p1", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 1_000_000, "out": 250_000, "cached": 0 } }],
                    "by_day": [{ "day": "2026-11-10", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 1_000_000, "out": 250_000, "cached": 0 } }],
                    "by_day_provider": [{ "day": "2026-11-10", "provider_id": "p1", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 1_000_000, "out": 250_000, "cached": 0 } }]
                }))
            }),
        )
        .route(
            "/api/audit/v1/retention",
            get(|(State(s), m, uri, h): Req| async move {
                record(&s, &m, &uri, &h);
                axum::Json(json!({ "days": *s.retention.lock().unwrap(), "floor_days": 7 }))
            })
            .put(|(State(s), m, uri, h): Req, axum::Json(body): axum::Json<Value>| async move {
                record(&s, &m, &uri, &h);
                *s.retention.lock().unwrap() = body["days"].as_u64().unwrap() as u32;
                axum::Json(body)
            }),
        )
        .with_state(seen.clone());
    (common::serve(app).await, seen)
}

/// A stub server that only answers the provider health the Audit pages still ask it for.
async fn server_stub() -> String {
    let app = Router::new().route(
        "/api/server/v1/providers/health",
        get(|| async {
            axum::Json(json!({ "providers": [{ "provider_id": "p1", "status": "auth_failed", "since": 5, "error_code": "http_401" }] }))
        }),
    );
    common::serve(app).await
}

struct A {
    adm: String,
    cookie: String,
    http: reqwest::Client,
    state: Arc<AdminState>,
}

async fn admin_for(server_url: &str, auditor_url: &str) -> A {
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, common::PW).unwrap();
    // Provider p1 is priced at 2.0 per million tokens in and 4.0 per million out.
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, type, base_url, model, created_at, billing, price_in, price_out) VALUES ('p1', 'p1', 'openai', 'http://x/v1', 'm', 0, 'metered', 2.0, 4.0)",
            [],
        )
        .unwrap();
    let state = AdminState::with_server(db, ServerLink::new(server_url, "mgt_stub-token".into()));
    state.auditor.set(AuditorLink::new(auditor_url, "admrd_stub-token".into())).ok().unwrap();
    let adm = common::serve(askbar_admin::router(state.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    A { adm, cookie, http, state }
}

impl A {
    async fn get(&self, path: &str, signed_in: bool, with_header: bool) -> (u16, Value) {
        let mut r = self.http.get(format!("{}{path}", self.adm));
        if with_header {
            r = r.header("x-askbar-admin", "1");
        }
        if signed_in {
            r = r.header("cookie", &self.cookie);
        }
        let resp = r.send().await.unwrap();
        (resp.status().as_u16(), serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
}

fn close(v: &Value, want: f64) -> bool {
    v.as_f64().is_some_and(|x| (x - want).abs() < 1e-9)
}

#[tokio::test]
async fn the_pages_need_a_session_like_every_other_admin_route() {
    let (url, seen) = auditor_stub().await;
    let a = admin_for(&server_stub().await, &url).await;
    for p in ["/api/admin/v1/audit/traces", "/api/admin/v1/audit/traces/t1", "/api/admin/v1/audit/stats"] {
        assert_eq!(a.get(p, false, true).await.0, 401, "{p} without a session");
        assert_eq!(a.get(p, false, false).await.0, 401, "{p} with neither");
        // (The admin-header rule applies to changing requests; reads need only the session, as elsewhere.)
        assert_eq!(a.get(p, true, false).await.0, 200, "{p} with a session");
    }
    seen.requests.lock().unwrap().clear();
    for p in ["/api/admin/v1/audit/traces", "/api/admin/v1/audit/stats"] {
        assert_eq!(a.get(p, false, true).await.0, 401);
    }
    assert!(seen.requests.lock().unwrap().is_empty(), "nothing reaches the auditor without a session");
}

#[tokio::test]
async fn known_parameters_are_forwarded_with_the_read_token_and_unknown_ones_are_dropped() {
    let (url, seen) = auditor_stub().await;
    let a = admin_for(&server_stub().await, &url).await;
    let (st, body) = a.get("/api/admin/v1/audit/traces?status=error&provider=p1&before=01ABC&limit=25&evil=1&since=5", true, true).await;
    assert_eq!(st, 200);
    assert_eq!(body["traces"][0]["trace_id"], "t1", "the auditor's answer is passed on");
    let (method, path, query, auth) = seen.requests.lock().unwrap()[0].clone();
    assert_eq!((method.as_str(), path.as_str()), ("GET", "/api/audit/v1/traces"));
    let pairs: Vec<&str> = query.split('&').collect();
    for want in ["status=error", "provider=p1", "before=01ABC", "limit=25", "since=5"] {
        assert!(pairs.contains(&want), "{want} in {query}");
    }
    assert!(!query.contains("evil"), "unknown parameters are not passed on: {query}");
    assert_eq!(auth, "Bearer admrd_stub-token", "the admin speaks to the auditor with its own read token");

    // Odd characters in a value are encoded, not interpreted.
    a.get("/api/admin/v1/audit/traces?status=a%26b%3Dc", true, true).await;
    assert!(seen.requests.lock().unwrap()[1].2.contains("status=a%26b%3Dc"), "{:?}", seen.requests.lock().unwrap()[1]);
    let (st, _) = a.get("/api/admin/v1/audit/stats?since=100", true, true).await;
    assert_eq!(st, 200);
    assert_eq!(seen.requests.lock().unwrap()[2].2, "since=100");
}

#[tokio::test]
async fn cost_is_applied_here_from_the_admins_prices_never_taken_from_the_auditor() {
    let (url, _seen) = auditor_stub().await;
    let a = admin_for(&server_stub().await, &url).await;
    // 1M in * 2.0 + 250k out * 4.0 = 3.0.
    let (_, body) = a.get("/api/admin/v1/audit/traces", true, true).await;
    assert!(close(&body["traces"][0]["cost"], 3.0), "{body}");
    let (_, trace) = a.get("/api/admin/v1/audit/traces/t1", true, true).await;
    assert!(close(&trace["cost"], 3.0), "the trace is priced by the provider that served it: {trace}");
    assert!(trace["spans"][1]["cost"].is_null(), "an attempt with no counts has no cost");
    let (_, stats) = a.get("/api/admin/v1/audit/stats", true, true).await;
    assert!(
        close(&stats["cost"], 3.0) && close(&stats["by_provider"][0]["cost"], 3.0) && close(&stats["by_day"][0]["cost"], 3.0),
        "{stats}"
    );
    assert!(stats.get("by_day_provider").is_none(), "the auditor's working figures are not for the page");

    // A corrected price changes every figure at once: nothing about cost is stored.
    a.state.db.lock().await.conn().execute("UPDATE providers SET price_in = 4.0 WHERE id = 'p1'", []).unwrap();
    let (_, stats) = a.get("/api/admin/v1/audit/stats", true, true).await;
    assert!(close(&stats["cost"], 5.0), "{stats}");
    // A provider with no price leaves the cost unknown, never zero.
    a.state.db.lock().await.conn().execute("UPDATE providers SET price_out = NULL WHERE id = 'p1'", []).unwrap();
    let (_, stats) = a.get("/api/admin/v1/audit/stats", true, true).await;
    assert!(stats["cost"].is_null() && stats["cost_unknown_requests"] == 1, "{stats}");
}

#[tokio::test]
async fn one_trace_is_fetched_by_id_unknown_and_empty_ones_are_404_and_odd_ids_never_reach_the_auditor() {
    let (url, seen) = auditor_stub().await;
    let a = admin_for(&server_stub().await, &url).await;
    let (st, body) = a.get("/api/admin/v1/audit/traces/01HZXTRACE", true, true).await;
    assert_eq!((st, &body["trace_id"]), (200, &json!("01HZXTRACE")));
    assert_eq!(seen.requests.lock().unwrap()[0].1, "/api/audit/v1/traces/01HZXTRACE");
    assert_eq!(a.get("/api/admin/v1/audit/traces/missing", true, true).await.0, 404);
    assert_eq!(a.get("/api/admin/v1/audit/traces/empty", true, true).await.0, 404, "no spans, no trace");
    let before = seen.requests.lock().unwrap().len();
    for odd in ["..%2F..%2Fsnapshot", "a%20b", "x%3Fy%3D1"] {
        assert_eq!(a.get(&format!("/api/admin/v1/audit/traces/{odd}"), true, true).await.0, 404, "{odd}");
    }
    assert_eq!(seen.requests.lock().unwrap().len(), before, "an id that is not an id is not sent anywhere");
}

#[tokio::test]
async fn provider_health_still_comes_from_the_server_behind_the_session() {
    let (url, _seen) = auditor_stub().await;
    let a = admin_for(&server_stub().await, &url).await;
    assert_eq!(a.get("/api/admin/v1/audit/provider-health", false, true).await.0, 401);
    let (st, body) = a.get("/api/admin/v1/audit/provider-health", true, true).await;
    assert_eq!((st, &body["providers"][0]["status"]), (200, &json!("auth_failed")));
}

#[tokio::test]
async fn an_unreachable_auditor_is_a_503_with_a_message_and_an_unreachable_server_a_502_for_provider_health() {
    let dead = || {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let a = admin_for(&server_stub().await, &dead()).await;
    for p in ["/api/admin/v1/audit/traces", "/api/admin/v1/audit/traces/t1", "/api/admin/v1/audit/stats"] {
        let (st, body) = a.get(p, true, true).await;
        assert_eq!(st, 503, "{p}");
        assert!(body["detail"].as_str().unwrap_or("").contains("auditor cannot be reached"), "{body}");
    }
    let (_, url) = (0, auditor_stub().await.0);
    let b = admin_for(&dead(), &url).await;
    let (st, body) = b.get("/api/admin/v1/audit/provider-health", true, true).await;
    assert_eq!((st, body["error"].as_str()), (502, Some("server_unavailable")));
}

#[tokio::test]
async fn the_retention_setting_is_pushed_to_the_auditor_when_it_differs_and_not_when_it_matches() {
    let (url, seen) = auditor_stub().await;
    let a = admin_for(&server_stub().await, &url).await;
    let link = a.state.auditor.get().unwrap().clone();
    // The admin's setting is 30 days until changed, and the auditor holds 30: nothing to send.
    assert!(link.sync_retention(&a.state).await);
    assert!(!seen.requests.lock().unwrap().iter().any(|r| r.0 == "PUT"));
    // Changed to 45: pushed, with the read token.
    a.state.db.lock().await.conn().execute("INSERT INTO settings (key, value) VALUES ('audit_retention_days', '45')", []).unwrap();
    assert!(link.sync_retention(&a.state).await);
    assert_eq!(*seen.retention.lock().unwrap(), 45);
    let put = seen.requests.lock().unwrap().iter().find(|r| r.0 == "PUT").cloned().unwrap();
    assert_eq!((put.1.as_str(), put.3.as_str()), ("/api/audit/v1/retention", "Bearer admrd_stub-token"));
    // An auditor that was away and came back with another value is set again.
    *seen.retention.lock().unwrap() = 30;
    assert!(link.sync_retention(&a.state).await);
    assert_eq!(*seen.retention.lock().unwrap(), 45);
}
