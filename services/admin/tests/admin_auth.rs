#![cfg(unix)]
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
            .post(format!("http://{}/api/admin/v1/session", self.admin))
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
    let stored: String = t.state.db.lock().await.conn().query_row("SELECT password_hash FROM admin_credentials", [], |r| r.get(0)).unwrap();
    assert!(stored.starts_with("$argon2") && !stored.contains(PASSWORD));
    // A later start with a different value does not overwrite it.
    assert!(!web::init_password(&*t.state.db.lock().await, "something else").unwrap());
    assert_eq!(t.login(PASSWORD).await.status(), 204);
}

#[tokio::test]
async fn login_sets_a_locked_down_cookie_and_opens_the_session() {
    // ADR 0029 rule 1: `__Secure-` prefix, Secure, HttpOnly, SameSite=Strict, Path=/api/admin.
    let t = start(PASSWORD).await;
    let r = t.login(PASSWORD).await;
    assert_eq!(r.status(), 204);
    let c = r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().to_string();
    let parts: Vec<&str> = c.split(';').map(str::trim).collect();
    assert!(parts[0].starts_with("__Secure-askbar_admin="), "{c}");
    for want in ["Secure", "HttpOnly", "SameSite=Strict", "Path=/api/admin"] {
        assert!(parts.contains(&want), "missing {want}: {c}");
    }
    assert!(!parts.iter().any(|p| p.starts_with("Domain")), "host-only: {c}");
    let cookie = parts[0];
    assert_eq!(t.admin_get("/api/admin/v1/session", Some(cookie), None).await, 204);
    // The cookie name from before the move is not a session.
    let old = cookie.replacen("__Secure-askbar_admin", "askbar_admin", 1);
    assert_eq!(t.admin_get("/api/admin/v1/session", Some(&old), None).await, 401);
}

#[tokio::test]
async fn a_change_from_another_site_is_refused_even_with_the_header_and_a_session() {
    // ADR 0029 rule 2: the CSRF header, and Sec-Fetch-Site when the browser sends it.
    let t = start(PASSWORD).await;
    let cookie = t.session().await;
    let change = |site: Option<&str>| {
        let mut r =
            t.http.delete(format!("http://{}/api/admin/v1/session", t.admin)).header("x-askbar-admin", "1").header("cookie", &cookie);
        if let Some(s) = site {
            r = r.header("sec-fetch-site", s);
        }
        r
    };
    for site in ["cross-site", "same-site", "bogus"] {
        let r = change(Some(site)).send().await.unwrap();
        assert_eq!(r.status(), 403, "{site}");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!(p.error, "forbidden", "{site}");
        assert_eq!(t.admin_get("/api/admin/v1/session", Some(&cookie), None).await, 204, "nothing changed: still signed in after {site}");
    }
    // Signing in from another site is refused too (it is a change, before any session exists).
    let r = t
        .http
        .post(format!("http://{}/api/admin/v1/session", t.admin))
        .header("x-askbar-admin", "1")
        .header("sec-fetch-site", "cross-site")
        .json(&serde_json::json!({ "password": PASSWORD }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // A read from another site is not a change, so the rule does not apply (the cookie's SameSite
    // keeps it from being sent cross-site anyway).
    let r = t
        .http
        .get(format!("http://{}/api/admin/v1/session", t.admin))
        .header("cookie", &cookie)
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    // The admin site's own page (same-origin) and a typed request (none) may change things.
    assert_eq!(change(Some("none")).send().await.unwrap().status(), 204);
    let cookie2 = t.session().await;
    let r = t
        .http
        .delete(format!("http://{}/api/admin/v1/session", t.admin))
        .header("x-askbar-admin", "1")
        .header("cookie", &cookie2)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
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
    assert_eq!(t.admin_get("/api/admin/v1/session", None, None).await, 401);
    assert_eq!(t.admin_get("/api/admin/v1/session", Some("__Secure-askbar_admin=bogus"), None).await, 401);
    assert_eq!(t.admin_get("/api/admin/v1/health", None, None).await, 200, "health stays public");
    assert_eq!(t.admin_get("/health", None, None).await, 404, "the old health path is gone");
    assert_eq!(t.admin_get("/", None, None).await, 404, "nothing answers at / any more");
}

#[tokio::test]
async fn bearer_tokens_never_open_the_admin() {
    let t = start(PASSWORD).await;
    // The server token and a client token look alike to this site: neither is a session.
    for token in ["mgt_the-server-token", "abt_a-client-token", PASSWORD] {
        assert_eq!(t.admin_get("/api/admin/v1/session", None, Some(token)).await, 401);
        assert_eq!(t.admin_get("/api/admin/v1/session", Some("__Secure-askbar_admin=bogus"), Some(token)).await, 401);
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
    assert_eq!(post("/api/admin/v1/session", false, None).await.unwrap().status(), 403);
    assert_eq!(post("/api/admin/v1/session", false, Some(&cookie)).await.unwrap().status(), 403);
    assert_eq!(t.admin_get("/api/admin/v1/session", Some(&cookie), None).await, 204, "a rejected logout didn't sign us out");
}

#[tokio::test]
async fn logout_ends_the_session() {
    let t = start(PASSWORD).await;
    let cookie = t.session().await;
    let r = t
        .http
        .delete(format!("http://{}/api/admin/v1/session", t.admin))
        .header("x-askbar-admin", "1")
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert!(r.headers()[reqwest::header::SET_COOKIE].to_str().unwrap().contains("Max-Age=0"));
    assert_eq!(t.admin_get("/api/admin/v1/session", Some(&cookie), None).await, 401);
}

#[tokio::test]
async fn every_response_carries_the_locked_down_headers() {
    let t = start(PASSWORD).await;
    let r = t.http.get(format!("http://{}/health", t.admin)).send().await.unwrap();
    let h = r.headers();
    // ADR 0029 rule 5: a strict CSP, on the site and the API alike.
    let csp = h["content-security-policy"].to_str().unwrap();
    // D17: Trusted Types are required and no policy can be made.
    for want in [
        "default-src 'none'",
        "script-src 'self'",
        "frame-ancestors 'none'",
        "base-uri 'none'",
        "require-trusted-types-for 'script'",
        "trusted-types 'none'",
    ] {
        assert!(csp.contains(want), "{want}: {csp}");
    }
    assert!(!csp.contains("unsafe-inline") && !csp.contains("unsafe-eval"), "{csp}");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(h["x-frame-options"], "DENY");
    assert_eq!(h["cache-control"], "no-store");
}

#[tokio::test]
async fn every_admin_refusal_is_a_problem_with_a_code_a_sentence_and_no_bearer_challenge() {
    let t = start(PASSWORD).await;
    async fn expect(r: reqwest::Response, status: u16, code: &str) -> askbar_proto::Problem {
        assert_eq!(r.status().as_u16(), status, "{code}");
        assert_eq!(r.headers()["content-type"], "application/problem+json", "{code}");
        assert!(!r.headers().contains_key("www-authenticate"), "{code}: the admin signs in with a cookie");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!((p.error.as_str(), p.status), (code, status));
        assert!(!p.detail.is_empty(), "{code}: a sentence for a person");
        p
    }
    let url = |p: &str| format!("http://{}{p}", t.admin);
    // Signed out, and a change without the header.
    expect(t.http.get(url("/api/admin/v1/providers")).send().await.unwrap(), 401, "unauthorized").await;
    expect(t.http.post(url("/api/admin/v1/session")).json(&serde_json::json!({ "password": "x" })).send().await.unwrap(), 403, "forbidden")
        .await;
    // Unknown path, wrong method, a body that is not JSON.
    expect(t.http.get(url("/admin/nope")).send().await.unwrap(), 404, "not_found").await;
    expect(t.http.patch(url("/api/admin/v1/session")).header("x-askbar-admin", "1").send().await.unwrap(), 405, "method_not_allowed").await;
    let bad = t
        .http
        .post(url("/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .header("content-type", "application/json")
        .body("nope")
        .send()
        .await
        .unwrap();
    expect(bad, 400, "bad_request").await;
    // A wrong password, then the lockout with its wait.
    let p = expect(t.login("wrong password").await, 401, "unauthorized").await;
    assert!(p.detail.contains("wrong"));
    for _ in 0..10 {
        let _ = t.login("wrong password").await;
    }
    let r = t.login("wrong password").await;
    let wait: u64 = r.headers()[reqwest::header::RETRY_AFTER].to_str().unwrap().parse().unwrap();
    assert!(wait > 0);
    let p = expect(r, 429, "too_many_attempts").await;
    assert!(p.detail.contains("minute"), "{}", p.detail);
}

#[tokio::test]
async fn signing_in_before_a_password_exists_says_setup_is_required() {
    let db = AdminDb::open_in_memory().unwrap();
    let state = AdminState::new(db);
    let admin = spawn(askbar_admin::router(state)).await;
    let r = reqwest::Client::new()
        .post(format!("http://{admin}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&serde_json::json!({ "password": "whatever it is" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "setup_required");
}

// ------------------------------------------------------------------ the admin API key

async fn make_key(t: &T) -> (String, String) {
    let db = t.state.db.lock().await;
    let (row, key) = askbar_admin::api_key::create(db.conn(), "ci", askbar_admin::auth::now()).unwrap();
    (row.id, key)
}

#[tokio::test]
async fn a_key_opens_the_api_without_a_cookie_or_the_browser_header() {
    let t = start(PASSWORD).await;
    let (_, key) = make_key(&t).await;
    let url = |p: &str| format!("http://{}{p}", t.admin);
    // A read.
    assert_eq!(t.http.get(url("/api/admin/v1/providers")).bearer_auth(&key).send().await.unwrap().status(), 200);
    // A change, with no X-Askbar-Admin header: it gets past the checks and fails on its own body (400), not on auth (401/403).
    let r = t.http.put(url("/api/admin/v1/network")).bearer_auth(&key).json(&serde_json::json!({})).send().await.unwrap();
    assert_eq!(r.status(), 400, "past auth and the browser header");
    // The key's use is noted.
    let used: Option<i64> = t.state.db.lock().await.conn().query_row("SELECT last_used_at FROM admin_api_keys", [], |r| r.get(0)).unwrap();
    assert!(used.is_some());
}

#[tokio::test]
async fn a_key_cannot_open_a_session_change_the_password_or_stand_in_for_one() {
    let t = start(PASSWORD).await;
    let (_, key) = make_key(&t).await;
    let url = |p: &str| format!("http://{}{p}", t.admin);
    for (method, path) in [
        (reqwest::Method::GET, "/api/admin/v1/session"),
        (reqwest::Method::DELETE, "/api/admin/v1/session"),
        (reqwest::Method::PUT, "/api/admin/v1/password"),
    ] {
        let r = t.http.request(method.clone(), url(path)).bearer_auth(&key).header("x-askbar-admin", "1").send().await.unwrap();
        assert_eq!(r.status(), 401, "{method} {path} needs the person's own session");
        assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "unauthorized");
    }
    // And the key is not a way to sign in.
    let r = t
        .http
        .post(url("/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .bearer_auth(&key)
        .json(&serde_json::json!({ "password": "wrong password here" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    assert!(!r.headers().contains_key(reqwest::header::SET_COOKIE));
}

#[tokio::test]
async fn a_wrong_or_revoked_key_is_401_and_is_not_rescued_by_a_cookie() {
    let t = start(PASSWORD).await;
    let (id, key) = make_key(&t).await;
    let cookie = t.session().await;
    let get = |auth: Option<&str>, cookie: Option<&str>| {
        let mut r = t.http.get(format!("http://{}/api/admin/v1/providers", t.admin));
        if let Some(a) = auth {
            r = r.bearer_auth(a);
        }
        if let Some(c) = cookie {
            r = r.header("cookie", c);
        }
        r.send()
    };
    assert_eq!(get(Some("askbar-admin-not-a-key"), None).await.unwrap().status(), 401);
    assert_eq!(get(Some("abt_a-client-token"), None).await.unwrap().status(), 401);
    assert_eq!(
        get(Some("askbar-admin-not-a-key"), Some(&cookie)).await.unwrap().status(),
        401,
        "a valid cookie does not rescue a wrong key"
    );
    assert_eq!(get(None, Some(&cookie)).await.unwrap().status(), 200, "the cookie alone still works");
    assert_eq!(get(Some(&key), None).await.unwrap().status(), 200);
    askbar_admin::api_key::revoke(t.state.db.lock().await.conn(), &id, askbar_admin::auth::now()).unwrap();
    assert_eq!(get(Some(&key), None).await.unwrap().status(), 401, "revoked");
}

#[tokio::test]
async fn wrong_keys_count_toward_the_same_lockout_as_wrong_passwords() {
    let t = start(PASSWORD).await;
    let (_, key) = make_key(&t).await;
    let get = |k: &str| t.http.get(format!("http://{}/api/admin/v1/providers", t.admin)).bearer_auth(k).send();
    for _ in 0..askbar_admin::auth::MAX_FAILURES {
        assert_eq!(get("askbar-admin-wrong").await.unwrap().status(), 401);
    }
    let r = get(&key).await.unwrap();
    assert_eq!(r.status(), 429, "locked out, even for the right key");
    assert!(r.headers().contains_key(reqwest::header::RETRY_AFTER));
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "too_many_attempts");
    // The same lock stops a wrong password, which is the point of one lock.
    assert_eq!(t.login(PASSWORD).await.status(), 429);
}

// ------------------------------------------------------------------ a fresh sign-in (ADR 0040)

impl T {
    async fn change(&self, method: reqwest::Method, path: &str, cookie: &str, body: serde_json::Value) -> (u16, serde_json::Value) {
        let r = self
            .http
            .request(method, format!("http://{}{path}", self.admin))
            .header("cookie", cookie)
            .header("x-askbar-admin", "1")
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or_default())
    }
    fn age(&self, cookie: &str, secs: i64) {
        self.state.auth.backdate_session(cookie.split_once('=').unwrap().1, secs);
    }
}

fn settings(days: u32) -> serde_json::Value {
    serde_json::json!({ "system_prompt": "p", "display": askbar_proto::DisplaySettings::default(), "audit_retention_days": days })
}

#[tokio::test]
async fn an_old_sign_in_cannot_change_the_password_make_a_key_or_lower_retention() {
    let t = start(PASSWORD).await;
    let c = t.session().await;
    t.age(&c, 11 * 60);
    let change = serde_json::json!({ "current": PASSWORD, "new": "another long password" });
    for (method, path, body) in [
        (reqwest::Method::PUT, "/api/admin/v1/password", change),
        (reqwest::Method::POST, "/api/admin/v1/keys", serde_json::json!({ "name": "ci" })),
        (reqwest::Method::PUT, "/api/admin/v1/settings", settings(7)),
    ] {
        let (status, problem) = t.change(method.clone(), path, &c, body).await;
        assert_eq!(status, 403, "{method} {path}");
        assert!(problem["detail"].as_str().unwrap().contains("fresh sign-in"), "{problem}");
    }
    // Nothing changed: the old password still works, there is no key, and the retention is what it was.
    assert_eq!(t.login(PASSWORD).await.status(), 204);
    let keys: i64 = t.state.db.lock().await.conn().query_row("SELECT COUNT(*) FROM admin_api_keys", [], |r| r.get(0)).unwrap();
    assert_eq!((keys, askbar_admin::admin_api::audit_retention_days(t.state.db.lock().await.conn())), (0, 30));
}

#[tokio::test]
async fn an_old_sign_in_may_still_raise_retention_and_a_fresh_one_does_all_three() {
    let t = start(PASSWORD).await;
    let old = t.session().await;
    t.age(&old, 11 * 60);
    // This rig has no server to push to, so a saved change ends in 502: what matters here is that it is not 403.
    assert_ne!(t.change(reqwest::Method::PUT, "/api/admin/v1/settings", &old, settings(90)).await.0, 403, "more is safe");
    assert_ne!(t.change(reqwest::Method::PUT, "/api/admin/v1/settings", &old, settings(30)).await.0, 403, "the same is safe");
    let fresh = t.session().await;
    assert_ne!(t.change(reqwest::Method::PUT, "/api/admin/v1/settings", &fresh, settings(14)).await.0, 403);
    assert_eq!(t.change(reqwest::Method::POST, "/api/admin/v1/keys", &fresh, serde_json::json!({ "name": "ci" })).await.0, 201);
    let change = serde_json::json!({ "current": PASSWORD, "new": "another long password" });
    assert_eq!(t.change(reqwest::Method::PUT, "/api/admin/v1/password", &fresh, change).await.0, 204);
}

#[tokio::test]
async fn an_api_key_cannot_lower_retention_even_beside_a_fresh_cookie() {
    let t = start(PASSWORD).await;
    let (_, key) = make_key(&t).await;
    let c = t.session().await;
    let put = |days: u32, cookie: Option<&str>| {
        let mut r = t.http.put(format!("http://{}/api/admin/v1/settings", t.admin)).bearer_auth(&key).json(&settings(days));
        if let Some(c) = cookie {
            r = r.header("cookie", c).header("x-askbar-admin", "1");
        }
        async move { r.send().await.unwrap().status().as_u16() }
    };
    assert_eq!(put(7, None).await, 403);
    assert_eq!(put(7, Some(&c)).await, 403, "the Authorization header alone is judged");
    assert_ne!(put(60, None).await, 403, "a key may still raise it (no server here, so it ends in 502)");
}
