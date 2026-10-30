//! Per-user display defaults: read by clients, set (for now) through the admin API.

use askbar_proto::DisplaySettings;
use askbar_server::{admin, auth, secrets::Secrets, AppState};
use serde_json::json;

const PASSWORD: &str = "correct horse battery";

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

struct T {
    api: String,
    admin: String,
    cookie: String,
    http: reqwest::Client,
    user_a: String,
    token_a: String,
    token_b: String,
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user_a = db.seed_server().unwrap();
    admin::init_password(&db, PASSWORD).unwrap();
    db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
    let (ca, cb) = (auth::create_pair_code(&db, &user_a, "a", 600).unwrap(), auth::create_pair_code(&db, "B", "b", 600).unwrap());
    askbar_server::bridge::rebuild(&db).unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080");
    let (token_a, token_b) = {
        let db = state.db.lock().unwrap();
        (auth::redeem_pair_code(&db, &ca).unwrap().unwrap(), auth::redeem_pair_code(&db, &cb).unwrap().unwrap())
    };
    let (api, admin) = (serve(askbar_server::api_router(state.clone())).await, serve(askbar_server::admin_router(state)).await);
    let http = reqwest::Client::new();
    let r = http.post(format!("{admin}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PASSWORD })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { api, admin, cookie, http, user_a, token_a, token_b }
}

impl T {
    async fn defaults(&self, token: &str) -> (u16, DisplaySettings) {
        let r = self
            .http
            .get(format!("{}/api/v1/me/display-defaults", self.api))
            .bearer_auth(token)
            .header("x-askbar-client-version", "0.3.0")
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or_default())
    }
    async fn set(&self, user: &str, body: serde_json::Value) -> u16 {
        self.http
            .put(format!("{}/admin/api/users/{user}/display-defaults", self.admin))
            .header("x-askbar-admin", "1")
            .header("cookie", &self.cookie)
            .json(&body)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16()
    }
}

#[tokio::test]
async fn defaults_differ_per_user() {
    let t = start().await;
    let built_in = DisplaySettings::default();
    assert_eq!(t.defaults(&t.token_a).await, (200, built_in.clone()));
    assert_eq!(t.defaults(&t.token_b).await, (200, built_in.clone()));

    let wide = json!({ "theme": "dark", "width": 900, "max_height": 700, "history_width": 300, "top_percent": 0.3, "hide_on_blur": false });
    assert_eq!(t.set(&t.user_a, wide).await, 204);
    let (_, a) = t.defaults(&t.token_a).await;
    assert_eq!((a.theme.as_str(), a.width, a.hide_on_blur), ("dark", 900, false));
    assert_eq!(t.defaults(&t.token_b).await.1, built_in, "the other user still has the defaults");

    let narrow = json!({ "theme": "light", "width": 500, "max_height": 400, "history_width": 200, "top_percent": 0.1, "hide_on_blur": true });
    assert_eq!(t.set("B", narrow).await, 204);
    let (_, b) = t.defaults(&t.token_b).await;
    assert_eq!((b.theme.as_str(), b.width), ("light", 500));
    assert_eq!(t.defaults(&t.token_a).await.1.width, 900, "setting B's defaults left A's alone");
}

#[tokio::test]
async fn the_setter_validates_and_the_endpoint_needs_a_token() {
    let t = start().await;
    assert_eq!(t.set(&t.user_a, json!({ "theme": "neon" })).await, 400);
    assert_eq!(t.set(&t.user_a, json!({ "width": 5 })).await, 400);
    assert_eq!(t.set("nobody", json!({})).await, 404);
    assert_eq!(t.defaults(&t.token_a).await.1, DisplaySettings::default(), "refused values changed nothing");
    assert_eq!(t.defaults("abt_not_a_token").await.0, 401);

    // The setter is admin-only: a device token is no use there.
    let r = t
        .http
        .put(format!("{}/admin/api/users/{}/display-defaults", t.admin, t.user_a))
        .header("x-askbar-admin", "1")
        .bearer_auth(&t.token_a)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}
