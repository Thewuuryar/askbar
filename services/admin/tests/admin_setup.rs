#![cfg(unix)]
use askbar_admin::{web, AdminDb, AdminState};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::Arc;

struct T {
    adm: String,
    state: Arc<AdminState>,
    http: reqwest::Client,
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

/// An admin straight after first start: the published password, to be changed (ADR 0007).
async fn fresh() -> T {
    let db = AdminDb::open_in_memory().unwrap();
    assert!(web::ensure_default_credentials(&db).unwrap(), "a first start stores the published password");
    assert!(!web::ensure_default_credentials(&db).unwrap(), "and never replaces one");
    let state = AdminState::new(db);
    T { adm: serve(askbar_admin::router(state.clone())).await, state, http: reqwest::Client::new() }
}

const PW: &str = "first password 123";
const DEFAULT: &str = web::DEFAULT_PASSWORD;

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
        self.call(M::GET, "/api/admin/v1/setup", None, json!({})).await.2
    }
    async fn sign_in(&self, username: Option<&str>, password: &str) -> (u16, Option<String>) {
        let body = match username {
            Some(u) => json!({ "username": u, "password": password }),
            None => json!({ "password": password }),
        };
        let (st, c, _) = self.call(M::POST, "/api/admin/v1/session", None, body).await;
        (st, c)
    }
    /// Sign in with the published password and change it to `PW`. Returns the session cookie.
    async fn first_run(&self) -> String {
        let (st, c) = self.sign_in(Some("admin"), DEFAULT).await;
        assert_eq!(st, 204);
        let c = c.unwrap();
        assert_eq!(self.call(M::PUT, "/api/admin/v1/password", Some(&c), json!({ "current": DEFAULT, "new": PW })).await.0, 204);
        c
    }
}

#[tokio::test]
async fn first_run_signs_in_with_the_published_password_and_must_change_it_before_anything_else() {
    let t = fresh().await;
    assert_eq!(t.status().await, json!({ "password_change_required": true, "audit_key_set": false }));
    let stored: String = t.state.db.lock().await.conn().query_row("SELECT password_hash FROM admin_credentials", [], |r| r.get(0)).unwrap();
    assert!(stored.starts_with("$argon2") && !stored.contains(DEFAULT));

    let (st, c) = t.sign_in(Some("admin"), DEFAULT).await;
    assert_eq!(st, 204, "the published login works");
    let c = c.unwrap();
    // Everything but the session and the password change is refused, with its own code.
    for (m, path) in [
        (M::GET, "/api/admin/v1/providers"),
        (M::GET, "/api/admin/v1/settings"),
        (M::PUT, "/api/admin/v1/setup/audit-key"),
        (M::POST, "/api/admin/v1/keys"),
        (M::GET, "/api/admin/v1/services"),
    ] {
        let (st, _, e) = t.call(m.clone(), path, Some(&c), json!({})).await;
        assert_eq!((st, e["error"].as_str()), (403, Some("password_change_required")), "{m} {path}");
    }
    assert_eq!(t.call(M::GET, "/api/admin/v1/session", Some(&c), json!({})).await.0, 204, "the session itself is fine");

    // The published password, the username and the current password are refused as new ones.
    // The published password and the username (both `admin`, in any case or spacing) are refused.
    for bad in [DEFAULT, "ADMIN", " admin "] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/password", Some(&c), json!({ "current": DEFAULT, "new": bad })).await.0, 400, "{bad}");
    }
    assert_eq!(t.call(M::PUT, "/api/admin/v1/password", Some(&c), json!({ "current": DEFAULT, "new": PW })).await.0, 204);
    assert_eq!(t.status().await["password_change_required"], false);
    assert_eq!(t.call(M::GET, "/api/admin/v1/providers", Some(&c), json!({})).await.0, 200, "now the rest opens");
    assert_eq!(t.sign_in(None, DEFAULT).await.0, 401, "the published password no longer works");
    assert_eq!(t.sign_in(None, PW).await.0, 204);
}

#[tokio::test]
async fn a_wrong_username_and_a_wrong_password_get_the_same_answer_and_both_count_toward_the_lockout() {
    let t = fresh().await;
    let (a, _, wrong_name) = t.call(M::POST, "/api/admin/v1/session", None, json!({ "username": "root", "password": DEFAULT })).await;
    let (b, _, wrong_pw) = t.call(M::POST, "/api/admin/v1/session", None, json!({ "username": "admin", "password": "nope" })).await;
    assert_eq!((a, b), (401, 401));
    assert_eq!(wrong_name, wrong_pw, "identical errors");
    for _ in 0..3 {
        t.sign_in(Some("root"), DEFAULT).await;
    }
    assert_eq!(t.sign_in(Some("admin"), DEFAULT).await.0, 429, "five failures of either kind lock sign-in");
}

#[tokio::test]
async fn reset_puts_the_published_password_back_and_records_it() {
    let t = fresh().await;
    let c = t.first_run().await;
    web::reset_to_default(&*t.state.db.lock().await).unwrap();
    assert_eq!(t.status().await["password_change_required"], true);
    assert_eq!(
        t.call(M::GET, "/api/admin/v1/providers", Some(&c), json!({})).await.2["error"],
        "password_change_required",
        "even an old session must change it first"
    );
    assert_eq!(t.sign_in(None, PW).await.0, 401);
    assert_eq!(t.sign_in(Some("admin"), DEFAULT).await.0, 204);
    let (service, action, ok): (String, String, i64) = t
        .state
        .db
        .lock()
        .await
        .conn()
        .query_row("SELECT service, action, ok FROM service_actions ORDER BY id DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((service.as_str(), action.as_str(), ok), ("admin", "reset", 1));
}

#[tokio::test]
async fn the_first_visitor_setup_route_is_gone() {
    let t = fresh().await;
    assert_eq!(t.call(M::PUT, "/api/admin/v1/setup/password", None, json!({ "password": "someone else 999" })).await.0, 404);
}

#[tokio::test]
async fn an_install_from_before_keeps_its_own_password_with_nothing_to_change() {
    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PW).unwrap(); // as an earlier release stored it
    assert!(!web::ensure_default_credentials(&db).unwrap());
    let state = AdminState::new(db);
    let t = T { adm: serve(askbar_admin::router(state.clone())).await, state, http: reqwest::Client::new() };
    assert_eq!(t.status().await["password_change_required"], false);
    assert_eq!(t.sign_in(None, DEFAULT).await.0, 401);
    assert_eq!(t.sign_in(None, PW).await.0, 204);
}

#[tokio::test]
async fn changing_the_password() {
    let t = fresh().await;
    let c1 = t.first_run().await;
    let (_, c2) = t.sign_in(None, PW).await;
    let c2 = c2.unwrap();

    let change = |cur: &str, new: &str| json!({ "current": cur, "new": new });
    let (st, _, e) = t.call(M::PUT, "/api/admin/v1/password", Some(&c1), change("wrong", "new password 456")).await;
    assert_eq!(st, 403, "403 (not 401) so the page doesn't think it was signed out");
    assert!(e["detail"].as_str().unwrap().contains("current password"));
    assert_eq!(t.call(M::PUT, "/api/admin/v1/password", Some(&c1), change(PW, "short")).await.0, 400);
    let (st, _, e) = t.call(M::PUT, "/api/admin/v1/password", Some(&c1), change(PW, PW)).await;
    assert_eq!(st, 400, "the current password is not a new one");
    assert!(e["detail"].as_str().unwrap().contains("not the current one"), "{e}");
    assert_eq!(t.call(M::PUT, "/api/admin/v1/password", None, change(PW, "new password 456")).await.0, 401);

    assert_eq!(t.call(M::PUT, "/api/admin/v1/password", Some(&c1), change(PW, "new password 456")).await.0, 204);
    assert_eq!(t.call(M::GET, "/api/admin/v1/session", Some(&c1), json!({})).await.0, 204, "this session stays");
    assert_eq!(t.call(M::GET, "/api/admin/v1/session", Some(&c2), json!({})).await.0, 401, "other sessions are signed out");
    assert_eq!(t.sign_in(None, PW).await.0, 401);
    assert_eq!(t.sign_in(None, "new password 456").await.0, 204);
}

#[tokio::test]
async fn the_setup_screens_do_not_leak_secrets_in_status() {
    let t = fresh().await;
    let text = t.status().await.to_string();
    assert!(!text.contains("argon") && !text.contains("key\":\"") && !text.contains(DEFAULT));
}
