use askbar_server::AppState;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::Arc;

struct T {
    adm: String,
    state: Arc<AppState>,
    dir: std::path::PathBuf,
    http: reqwest::Client,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

/// A server straight after first start: no admin password yet.
async fn fresh() -> T {
    let dir = std::env::temp_dir().join(format!("askbar-setup-{}", ulid::Ulid::new()));
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::build(db, askbar_server::secrets::Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.3.0", "http://localhost:8080");
    T {
        adm: serve(askbar_server::admin_router(state.clone())).await,
        state,
        dir,
        http: reqwest::Client::new(),
    }
}

const PW: &str = "first password 123";

impl T {
    async fn call(&self, method: M, path: &str, cookie: Option<&str>, body: Value) -> (u16, Option<String>, Value) {
        let mut r = self.http.request(method, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").json(&body);
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        let resp = r.send().await.unwrap();
        let st = resp.status().as_u16();
        let cookie = resp.headers().get("set-cookie").map(|v| v.to_str().unwrap().split(';').next().unwrap().to_string());
        (st, cookie, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
    async fn status(&self) -> Value {
        self.call(M::GET, "/admin/api/setup-status", None, json!({})).await.2
    }
    async fn first_password(&self) -> String {
        let (st, cookie, _) = self.call(M::POST, "/admin/api/setup/password", None, json!({ "password": PW })).await;
        assert_eq!(st, 204);
        cookie.expect("setting the password signs you in")
    }
}

#[tokio::test]
async fn first_run_password() {
    let t = fresh().await;
    assert_eq!(t.status().await, json!({ "password_set": false }));
    assert_eq!(t.call(M::POST, "/admin/login", None, json!({ "password": "x" })).await.0, 503, "no password set yet, so no login");

    // Too short is refused and sets nothing.
    assert_eq!(t.call(M::POST, "/admin/api/setup/password", None, json!({ "password": "short" })).await.0, 400);
    assert_eq!(t.status().await["password_set"], false);

    t.first_password().await;
    let stored: String = t.state.db.lock().unwrap().conn().query_row("SELECT value FROM settings WHERE key='admin_password_hash'", [], |r| r.get(0)).unwrap();
    assert!(stored.starts_with("$argon2") && !stored.contains(PW));
    assert_eq!(t.call(M::POST, "/admin/api/setup/password", None, json!({ "password": "someone else 999" })).await.0, 409, "only the first visitor can set it");
    assert_eq!(t.call(M::POST, "/admin/login", None, json!({ "password": PW })).await.0, 204);
}

#[tokio::test]
async fn racing_first_visitors_get_exactly_one_password() {
    let t = fresh().await;
    let attempt = |n: u32| {
        let t = &t;
        async move { t.call(M::POST, "/admin/api/setup/password", None, json!({ "password": format!("racer password {n}") })).await.0 }
    };
    let results = futures_util::future::join_all((0..4).map(attempt)).await;
    assert_eq!(results.iter().filter(|s| **s == 204).count(), 1, "{results:?}");
    assert_eq!(results.iter().filter(|s| **s == 409).count(), 3, "{results:?}");
}

#[tokio::test]
async fn changing_the_password() {
    let t = fresh().await;
    let c1 = t.first_password().await;
    let (_, c2, _) = t.call(M::POST, "/admin/login", None, json!({ "password": PW })).await;
    let c2 = c2.unwrap();

    let change = |cur: &str, new: &str| json!({ "current": cur, "new": new });
    let (st, _, e) = t.call(M::POST, "/admin/api/password", Some(&c1), change("wrong", "new password 456")).await;
    assert_eq!(st, 403, "403 (not 401) so the page doesn't think it was signed out");
    assert!(e["error"].as_str().unwrap().contains("current password"));
    assert_eq!(t.call(M::POST, "/admin/api/password", Some(&c1), change(PW, "short")).await.0, 400);
    assert_eq!(t.call(M::POST, "/admin/api/password", None, change(PW, "new password 456")).await.0, 401);

    assert_eq!(t.call(M::POST, "/admin/api/password", Some(&c1), change(PW, "new password 456")).await.0, 204);
    assert_eq!(t.call(M::GET, "/admin/session", Some(&c1), json!({})).await.0, 204, "this session stays");
    assert_eq!(t.call(M::GET, "/admin/session", Some(&c2), json!({})).await.0, 401, "other sessions are signed out");
    assert_eq!(t.call(M::POST, "/admin/login", None, json!({ "password": PW })).await.0, 401);
    assert_eq!(t.call(M::POST, "/admin/login", None, json!({ "password": "new password 456" })).await.0, 204);
}

#[tokio::test]
async fn the_setup_screens_do_not_leak_secrets_in_status() {
    let t = fresh().await;
    let text = t.status().await.to_string();
    assert!(!text.contains("argon") && !text.contains("key\":\""));
}
