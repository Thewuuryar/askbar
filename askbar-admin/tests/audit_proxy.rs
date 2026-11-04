//! The admin's read-only proxy of the server's audit endpoints: it needs a session, forwards only
//! the query parameters it knows, passes the server's answer through, and says so when the server
//! is away. The server here is a stub that records what it was asked.

mod common;
use askbar_admin::{server_link::ServerLink, web, AdminDb, AdminState};
use axum::{extract::State, http::StatusCode, routing::get, Router};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Seen {
    /// (path, query, authorization header) of every request the stub got.
    requests: Mutex<Vec<(String, String, String)>>,
}

async fn stub() -> (String, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let record = |uri: &axum::http::Uri, h: &axum::http::HeaderMap, seen: &Seen| {
        seen.requests.lock().unwrap().push((
            uri.path().to_string(),
            uri.query().unwrap_or("").to_string(),
            h.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string(),
        ));
    };
    let app = Router::new()
        .route(
            "/api/server/v1/audit/traces",
            get(move |State(s): State<Arc<Seen>>, uri: axum::http::Uri, h: axum::http::HeaderMap| async move {
                record(&uri, &h, &s);
                axum::Json(json!({ "traces": [{ "trace_id": "t1", "status": "done" }] }))
            }),
        )
        .route(
            "/api/server/v1/audit/traces/{id}",
            get(move |State(s): State<Arc<Seen>>, uri: axum::http::Uri, h: axum::http::HeaderMap| async move {
                record(&uri, &h, &s);
                if uri.path().ends_with("/missing") {
                    return Err(StatusCode::NOT_FOUND);
                }
                Ok(axum::Json(json!({ "trace_id": "t1", "spans": [] })))
            }),
        )
        .route(
            "/api/server/v1/providers/health",
            get(move |State(s): State<Arc<Seen>>, uri: axum::http::Uri, h: axum::http::HeaderMap| async move {
                record(&uri, &h, &s);
                axum::Json(json!({ "providers": [{ "provider_id": "p1", "status": "auth_failed", "since": 5, "error_code": "http_401" }] }))
            }),
        )
        .route(
            "/api/server/v1/audit/stats",
            get(move |State(s): State<Arc<Seen>>, uri: axum::http::Uri, h: axum::http::HeaderMap| async move {
                record(&uri, &h, &s);
                axum::Json(json!({ "requests": 3 }))
            }),
        )
        .with_state(seen.clone());
    (common::serve(app).await, seen)
}

struct A {
    adm: String,
    cookie: String,
    http: reqwest::Client,
}

async fn admin_for(server_url: &str) -> A {
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, common::PW).unwrap();
    let state = AdminState::with_server(db, ServerLink::new(server_url, "mgt_stub-token".into()));
    let adm = common::serve(askbar_admin::router(state)).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/admin/api/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": common::PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    A { adm, cookie, http }
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

#[tokio::test]
async fn the_proxy_needs_a_session_like_every_other_admin_route() {
    let (url, seen) = stub().await;
    let a = admin_for(&url).await;
    for p in ["/admin/api/v1/audit/traces", "/admin/api/v1/audit/traces/t1", "/admin/api/v1/audit/stats"] {
        assert_eq!(a.get(p, false, true).await.0, 401, "{p} without a session");
        assert_eq!(a.get(p, false, false).await.0, 401, "{p} with neither");
        // (The admin-header rule applies to changing requests; reads need only the session, as elsewhere.)
        assert_eq!(a.get(p, true, false).await.0, 200, "{p} with a session");
    }
    seen.requests.lock().unwrap().clear();
    for p in ["/admin/api/v1/audit/traces", "/admin/api/v1/audit/stats"] {
        assert_eq!(a.get(p, false, true).await.0, 401);
    }
    assert!(seen.requests.lock().unwrap().is_empty(), "nothing reaches the server without a session");
}

#[tokio::test]
async fn known_parameters_are_forwarded_with_the_server_token_and_unknown_ones_are_dropped() {
    let (url, seen) = stub().await;
    let a = admin_for(&url).await;
    let (st, body) = a.get("/admin/api/v1/audit/traces?status=error&provider=p1&before=01ABC&limit=25&evil=1&since=5", true, true).await;
    assert_eq!(st, 200);
    assert_eq!(body["traces"][0]["trace_id"], "t1", "the server's answer is passed through");
    let (path, query, auth) = seen.requests.lock().unwrap()[0].clone();
    assert_eq!(path, "/api/server/v1/audit/traces");
    let pairs: Vec<&str> = query.split('&').collect();
    for want in ["status=error", "provider=p1", "before=01ABC", "limit=25", "since=5"] {
        assert!(pairs.contains(&want), "{want} in {query}");
    }
    assert!(!query.contains("evil"), "unknown parameters are not passed on: {query}");
    assert_eq!(auth, "Bearer mgt_stub-token", "the admin speaks to the server with its own token");

    // Odd characters in a value are encoded, not interpreted.
    a.get("/admin/api/v1/audit/traces?status=a%26b%3Dc", true, true).await;
    assert!(seen.requests.lock().unwrap()[1].1.contains("status=a%26b%3Dc"), "{:?}", seen.requests.lock().unwrap()[1]);

    let (st, _) = a.get("/admin/api/v1/audit/stats?since=100", true, true).await;
    assert_eq!(st, 200);
    assert_eq!(seen.requests.lock().unwrap()[2].1, "since=100");
}

#[tokio::test]
async fn one_trace_is_fetched_by_id_unknown_ones_are_404_and_odd_ids_never_reach_the_server() {
    let (url, seen) = stub().await;
    let a = admin_for(&url).await;
    let (st, body) = a.get("/admin/api/v1/audit/traces/01HZXTRACE", true, true).await;
    assert_eq!((st, &body["trace_id"]), (200, &json!("t1")));
    assert_eq!(seen.requests.lock().unwrap()[0].0, "/api/server/v1/audit/traces/01HZXTRACE");
    assert_eq!(a.get("/admin/api/v1/audit/traces/missing", true, true).await.0, 404);
    let before = seen.requests.lock().unwrap().len();
    for odd in ["..%2F..%2Fsnapshot", "a%20b", "x%3Fy%3D1"] {
        assert_eq!(a.get(&format!("/admin/api/v1/audit/traces/{odd}"), true, true).await.0, 404, "{odd}");
    }
    assert_eq!(seen.requests.lock().unwrap().len(), before, "an id that is not an id is not sent anywhere");
}

#[tokio::test]
async fn provider_health_is_proxied_behind_the_session() {
    let (url, seen) = stub().await;
    let a = admin_for(&url).await;
    assert_eq!(a.get("/admin/api/v1/audit/provider-health", false, true).await.0, 401);
    assert!(seen.requests.lock().unwrap().is_empty());
    let (st, body) = a.get("/admin/api/v1/audit/provider-health", true, true).await;
    assert_eq!((st, &body["providers"][0]["status"]), (200, &json!("auth_failed")));
    assert_eq!(seen.requests.lock().unwrap()[0].0, "/api/server/v1/providers/health");
}

#[tokio::test]
async fn an_unreachable_server_is_a_502_with_a_message() {
    // A port that nothing listens on.
    let dead = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let a = admin_for(&dead).await;
    for p in
        ["/admin/api/v1/audit/traces", "/admin/api/v1/audit/traces/t1", "/admin/api/v1/audit/stats", "/admin/api/v1/audit/provider-health"]
    {
        let (st, body) = a.get(p, true, true).await;
        assert_eq!(st, 502, "{p}");
        assert!(body["detail"].as_str().unwrap_or("").contains("offline"), "{body}");
        assert_eq!(body["error"], "server_unavailable", "{p}");
    }
}
