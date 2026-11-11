//! The client API lives at /api/client/v1; /api/v1 is gone except for one answer that tells an old app to update.
use askbar_proto::Problem;
use askbar_server::{auth, AppState};

struct T {
    url: String,
    token: String,
    http: reqwest::Client,
    code: String,
    /// The user's personal space.
    space: String,
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
    let code2 = askbar_server::testing::add_pair_code(&db, &user, "second");
    let state = AppState::new(db, askbar_server::MIN_CLIENT);
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let space = askbar_server::testing::personal_space(&state, &user).id.clone();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::client_router(state);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, token, http: reqwest::Client::new(), code: code2, space }
}

impl T {
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(format!("{}{path}", self.url)).bearer_auth(&self.token).header("x-askbar-client-version", askbar_server::MIN_CLIENT)
    }
}

#[tokio::test]
async fn the_client_api_answers_at_its_new_paths() {
    let t = start().await;
    let h: askbar_proto::Health = t.http.get(format!("{}/api/client/v1/health", t.url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(h.min_client, "0.6.0");
    assert_eq!(t.get("/api/client/v1/me").send().await.unwrap().status(), 204, "who am I: a token that is still accepted");
    let s = &t.space;
    for path in [
        "/api/client/v1/providers".to_string(),
        "/api/client/v1/spaces".to_string(),
        format!("/api/client/v1/spaces/{s}/conversations"),
        "/api/client/v1/me/prefs".to_string(),
        "/api/client/v1/me/display-defaults".to_string(),
        "/api/client/v1/settings".to_string(),
        format!("/api/client/v1/spaces/{s}/generations?status=running"),
        format!("/api/client/v1/spaces/{s}/search?q=x"),
    ] {
        assert_eq!(t.get(&path).send().await.unwrap().status(), 200, "{path}");
    }
    // Pairing is a pairings resource: POST creates a token.
    let r = t
        .http
        .post(format!("{}/api/client/v1/pairings", t.url))
        .header("x-askbar-client-version", "0.6.0")
        .json(&serde_json::json!({ "code": t.code }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.json::<askbar_proto::PairResponse>().await.unwrap().token.starts_with("abt_"));
}

#[tokio::test]
async fn the_minimum_client_is_0_6_0_and_an_older_app_on_a_new_path_is_told_to_update() {
    assert_eq!(askbar_server::MIN_CLIENT, "0.6.0");
    let t = start().await;
    let old = t
        .http
        .get(format!("{}/api/client/v1/me", t.url))
        .bearer_auth(&t.token)
        .header("x-askbar-client-version", "0.4.0")
        .send()
        .await
        .unwrap();
    assert_eq!(old.status(), 426);
    assert_eq!(old.json::<Problem>().await.unwrap().error, "client_too_old");
}

#[tokio::test]
async fn the_old_names_are_gone_and_old_paths_are_404_except_the_one_that_says_update() {
    let t = start().await;
    for gone in [
        "/api/client/v1/pair",
        "/api/client/v1/whoami",
        "/api/client/v2/health",
        "/api/client/health",
        "/mgmt/v1/health",
        "/api/server/v1/health",
        "/api/health",
        // The flat routes of 0.5.x, before spaces (ADR 0009, ADR 0022).
        "/api/client/v1/conversations",
        "/api/client/v1/conversations/x",
        "/api/client/v1/search?q=x",
        "/api/client/v1/generations?status=running",
        "/api/client/v1/generations/x/stream",
    ] {
        // names-ok: proves the old path is gone
        let r = t.get(gone).send().await.unwrap();
        assert_eq!(r.status(), 404, "{gone}");
        assert_eq!(r.json::<Problem>().await.unwrap().error, "not_found");
    }
}

#[tokio::test]
async fn the_first_generation_of_the_api_answers_426_update_the_app_whatever_is_asked() {
    let t = start().await;
    for (method, path) in [
        (reqwest::Method::GET, "/api/v1/health"),
        (reqwest::Method::GET, "/api/v1/conversations"),
        (reqwest::Method::POST, "/api/v1/chat"),
        (reqwest::Method::POST, "/api/v1/pair"),
        (reqwest::Method::GET, "/api/v1/whoami"),
        (reqwest::Method::DELETE, "/api/v1/conversations/x"),
    ] {
        // No token, no version header: an old app that will never send the new ones still hears it.
        let r = t.http.request(method.clone(), format!("{}{path}", t.url)).send().await.unwrap();
        assert_eq!(r.status(), 426, "{method} {path}");
        assert_eq!(r.headers()["content-type"], "application/problem+json");
        let p: Problem = r.json().await.unwrap();
        assert_eq!(p.error, "client_too_old");
        assert!(p.detail.to_lowercase().contains("update"), "{}", p.detail);
    }
}
