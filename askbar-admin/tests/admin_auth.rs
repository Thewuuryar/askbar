use askbar_admin::{web, AdminDb, AdminState};
use std::net::SocketAddr;
use std::sync::Arc;

const PASSWORD: &str = "correct horse battery";

struct T {
    admin: SocketAddr,
    state: Arc<AdminState>,
    http: reqwest::Client,
}

async fn spawn(app: axum::Router) -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

async fn start(password: &str) -> T {
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, password).unwrap();
    let state = AdminState::new(db);
    T { admin: spawn(askbar_admin::router(state.clone())).await, state, http: reqwest::Client::new() }
}

impl T {
    async fn login(&self, password: &str) -> reqwest::Response {
        self.http
            .post(format!("http://{}/admin/login", self.admin))
            .header("x-askbar-admin", "1")
            .json(&serde_json::json!({ "password": password }))
            .send()
            .await
            .unwrap()
    }
    /// Log in and return the `name=value` cookie pair.
    async fn session(&self) -> String {
        let r = self.login(PASSWORD).await;
        assert_eq!(r.status(), 204);
        let c = r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().to_string();
        c.split(';').next().unwrap().to_string()
    }
    async fn admin_get(&self, path: &str, cookie: Option<&str>, bearer: Option<&str>) -> u16 {
        let mut r = self.http.get(format!("http://{}{path}", self.admin));
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        if let Some(b) = bearer {
            r = r.bearer_auth(b);
        }
        r.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test]
async fn password_is_stored_as_an_argon2_hash_and_set_only_once() {
    let t = start(PASSWORD).await;
    let stored: String = t.state.db.lock().unwrap().conn().query_row("SELECT password_hash FROM admin_credentials", [], |r| r.get(0)).unwrap();
    assert!(stored.starts_with("$argon2") && !stored.contains(PASSWORD));
    // A later start with a different value does not overwrite it.
    assert!(!web::init_password(&t.state.db.lock().unwrap(), "something else").unwrap());
    assert_eq!(t.login(PASSWORD).await.status(), 204);
}

#[tokio::test]
async fn login_sets_a_locked_down_cookie_and_opens_the_session() {
    let t = start(PASSWORD).await;
    let r = t.login(PASSWORD).await;
    assert_eq!(r.status(), 204);
    let c = r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().to_string();
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Strict") && c.contains("Path=/admin"), "{c}");
    assert!(!c.contains("Secure"), "plain http: no Secure flag");
    let cookie = c.split(';').next().unwrap();
    assert_eq!(t.admin_get("/admin/session", Some(cookie), None).await, 204);
}

#[tokio::test]
async fn wrong_password_is_401_then_locks_out_even_the_right_one() {
    let t = start(PASSWORD).await;
    for _ in 0..4 {
        assert_eq!(t.login("nope").await.status(), 401);
    }
    assert_eq!(t.login("nope").await.status(), 401);
    let r = t.login(PASSWORD).await;
    assert_eq!(r.status(), 429, "locked out after repeated failures");
    assert!(r.headers().contains_key("retry-after"));
}

#[tokio::test]
async fn a_success_resets_the_failure_count() {
    let t = start(PASSWORD).await;
    for _ in 0..3 {
        t.login("nope").await;
    }
    assert_eq!(t.login(PASSWORD).await.status(), 204);
    for _ in 0..3 {
        assert_eq!(t.login("nope").await.status(), 401);
    }
    assert_eq!(t.login(PASSWORD).await.status(), 204);
}

#[tokio::test]
async fn no_configured_password_means_no_login() {
    let t = start("").await;
    assert_eq!(t.login("").await.status(), 503);
    assert_eq!(t.login("anything").await.status(), 503);
}

#[tokio::test]
async fn missing_or_bogus_session_is_401() {
    let t = start(PASSWORD).await;
    assert_eq!(t.admin_get("/admin/session", None, None).await, 401);
    assert_eq!(t.admin_get("/admin/session", Some("askbar_admin=bogus"), None).await, 401);
    assert_eq!(t.admin_get("/health", None, None).await, 200, "health stays public");
}

#[tokio::test]
async fn bearer_tokens_never_open_the_admin() {
    let t = start(PASSWORD).await;
    // The management token and a device token look alike to this site: neither is a session.
    for token in ["mgt_the-management-token", "abt_a-device-token", PASSWORD] {
        assert_eq!(t.admin_get("/admin/session", None, Some(token)).await, 401);
        assert_eq!(t.admin_get("/admin/session", Some("askbar_admin=bogus"), Some(token)).await, 401);
    }
}

#[tokio::test]
async fn state_changing_requests_need_the_admin_header() {
    let t = start(PASSWORD).await;
    let cookie = t.session().await;
    let post = |path: &str, header: bool, cookie: Option<&str>| {
        let mut r = t.http.post(format!("http://{}{path}", t.admin)).json(&serde_json::json!({ "password": PASSWORD }));
        if header {
            r = r.header("x-askbar-admin", "1");
        }
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        r.send()
    };
    assert_eq!(post("/admin/login", false, None).await.unwrap().status(), 403);
    assert_eq!(post("/admin/logout", false, Some(&cookie)).await.unwrap().status(), 403);
    assert_eq!(t.admin_get("/admin/session", Some(&cookie), None).await, 204, "a rejected logout didn't sign us out");
}

#[tokio::test]
async fn logout_ends_the_session() {
    let t = start(PASSWORD).await;
    let cookie = t.session().await;
    let r = t.http.post(format!("http://{}/admin/logout", t.admin)).header("x-askbar-admin", "1").header("cookie", &cookie).send().await.unwrap();
    assert_eq!(r.status(), 204);
    assert!(r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(t.admin_get("/admin/session", Some(&cookie), None).await, 401);
}

#[tokio::test]
async fn cookie_is_marked_secure_behind_the_tls_proxy() {
    let t = start(PASSWORD).await;
    let r = t
        .http
        .post(format!("http://{}/admin/login", t.admin))
        .header("x-askbar-admin", "1")
        .header("x-forwarded-proto", "https")
        .json(&serde_json::json!({ "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert!(r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().contains("; Secure"));
}

#[tokio::test]
async fn every_response_carries_the_locked_down_headers() {
    let t = start(PASSWORD).await;
    let r = t.http.get(format!("http://{}/health", t.admin)).send().await.unwrap();
    let h = r.headers();
    assert!(h["content-security-policy"].to_str().unwrap().contains("frame-ancestors 'none'"));
    assert_eq!(h["x-frame-options"], "DENY");
    assert_eq!(h["cache-control"], "no-store");
}
