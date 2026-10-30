use askbar_server::AppState;
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::Arc;

struct T {
    api: String,
    adm: String,
    state: Arc<AppState>,
    key_path: std::path::PathBuf,
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

/// A server straight after first start: no password, no master key.
async fn fresh() -> T {
    let dir = std::env::temp_dir().join(format!("askbar-setup-{}", ulid::Ulid::new()));
    let key_path = dir.join("secrets").join("master.key");
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::build(db, None, "0.1.0", "http://localhost:8080", key_path.clone());
    T {
        api: serve(askbar_server::api_router(state.clone())).await,
        adm: serve(askbar_server::admin_router(state.clone())).await,
        state,
        key_path,
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
async fn first_run_password_then_master_key() {
    let t = fresh().await;
    assert_eq!(t.status().await, json!({ "password_set": false, "key_set": false }));
    assert_eq!(t.call(M::POST, "/admin/login", None, json!({ "password": "x" })).await.0, 503);
    assert_eq!(t.call(M::POST, "/admin/api/setup/master-key", None, json!({})).await.0, 401, "the key step needs the session");

    // Too short is refused and sets nothing.
    assert_eq!(t.call(M::POST, "/admin/api/setup/password", None, json!({ "password": "short" })).await.0, 400);
    assert_eq!(t.status().await["password_set"], false);

    let cookie = t.first_password().await;
    assert_eq!(t.status().await, json!({ "password_set": true, "key_set": false }));
    let stored: String = t.state.db.lock().unwrap().conn().query_row("SELECT value FROM settings WHERE key='admin_password_hash'", [], |r| r.get(0)).unwrap();
    assert!(stored.starts_with("$argon2") && !stored.contains(PW));
    assert_eq!(t.call(M::POST, "/admin/api/setup/password", None, json!({ "password": "someone else 999" })).await.0, 409, "only the first visitor can set it");
    assert_eq!(t.call(M::POST, "/admin/login", None, json!({ "password": PW })).await.0, 204);

    // Generating the key shows it once and writes the same text to the secrets volume.
    assert!(!t.key_path.exists());
    let (st, _, body) = t.call(M::POST, "/admin/api/setup/master-key", Some(&cookie), json!({})).await;
    assert_eq!(st, 200);
    let key = body["key"].as_str().unwrap().to_string();
    assert!(key.len() >= 32);
    assert_eq!(std::fs::read_to_string(&t.key_path).unwrap().trim(), key);
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&t.key_path).unwrap().permissions()) & 0o777, 0o600);
    assert_eq!(t.status().await, json!({ "password_set": true, "key_set": true }));
    let s = t.state.secrets().expect("live without a restart");
    let reloaded = askbar_server::secrets::load_key_file(&t.key_path).unwrap().unwrap();
    assert_eq!(reloaded.decrypt(&s.encrypt("x")).unwrap(), "x", "the shown key is the one in use");

    // It is shown only at creation: asking again is refused and nothing else returns it.
    assert_eq!(t.call(M::POST, "/admin/api/setup/master-key", Some(&cookie), json!({})).await.0, 409);
    let page = t.http.get(format!("{}/admin/api/providers", t.adm)).header("cookie", &cookie).send().await.unwrap().text().await.unwrap();
    assert!(!page.contains(&key));
    // And the API is out of setup mode.
    let r = t.http.get(format!("{}/api/v1/whoami", t.api)).header("x-askbar-client-version", "0.1.0").send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn an_existing_key_can_be_restored_instead() {
    let t = fresh().await;
    let cookie = t.first_password().await;
    let (st, _, e) = t.call(M::POST, "/admin/api/setup/master-key", Some(&cookie), json!({ "key": "too short" })).await;
    assert_eq!(st, 400);
    assert!(e["error"].as_str().unwrap().contains("32"));
    assert!(!t.key_path.exists());

    let mine = "my-saved-master-key-from-bitwarden-0123456789";
    let (st, _, body) = t.call(M::POST, "/admin/api/setup/master-key", Some(&cookie), json!({ "key": format!("  {mine}\n") })).await;
    assert_eq!(st, 200);
    assert_eq!(body["key"], mine);
    assert_eq!(std::fs::read_to_string(&t.key_path).unwrap(), mine);
    // Data encrypted earlier under that key is readable again.
    let other = askbar_server::secrets::Secrets::new(mine).unwrap();
    assert_eq!(t.state.secrets().unwrap().decrypt(&other.encrypt("old provider key")).unwrap(), "old provider key");
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
