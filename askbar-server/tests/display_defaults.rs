//! Per-user display defaults: they arrive in the snapshot and clients read their own.

use askbar_proto::DisplaySettings;
use askbar_server::{auth, secrets::Secrets, testing, AppState};
use std::sync::Arc;

struct T {
    api: String,
    http: reqwest::Client,
    state: Arc<AppState>,
    user_a: String,
    token_a: String,
    token_b: String,
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user_a = db.seed_server().unwrap();
    db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
    let (ca, cb) = (testing::add_pair_code(&db, &user_a, "a"), testing::add_pair_code(&db, "B", "b"));
    let state = AppState::new(db, Secrets::new(testing::TEST_MASTER_KEY).unwrap(), askbar_server::MIN_CLIENT);
    let (token_a, token_b) = {
        let db = state.db.lock().unwrap();
        (auth::redeem_pair_code(&db, &ca).unwrap().unwrap(), auth::redeem_pair_code(&db, &cb).unwrap().unwrap())
    };
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::api_router(state.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { api, http: reqwest::Client::new(), state, user_a, token_a, token_b }
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
    fn push(&self, f: impl FnOnce(&mut askbar_proto::Snapshot)) {
        testing::update_snapshot(&self.state.db.lock().unwrap(), f);
    }
}

fn look(theme: &str, width: u32, hide: bool) -> DisplaySettings {
    DisplaySettings { theme: theme.into(), width, max_height: 700, history_width: 300, top_percent: 0.3, hide_on_blur: hide }
}

#[tokio::test]
async fn defaults_differ_per_user() {
    let t = start().await;
    let built_in = DisplaySettings::default();
    assert_eq!(t.defaults(&t.token_a).await, (200, built_in.clone()), "nothing pushed yet: the built-ins");
    assert_eq!(t.defaults(&t.token_b).await, (200, built_in.clone()));

    let user_a = t.user_a.clone();
    t.push(|s| {
        s.user_display_defaults.insert(user_a, look("dark", 900, false));
    });
    let (_, a) = t.defaults(&t.token_a).await;
    assert_eq!((a.theme.as_str(), a.width, a.hide_on_blur), ("dark", 900, false));
    assert_eq!(t.defaults(&t.token_b).await.1, built_in, "the other user still has the defaults");

    t.push(|s| {
        s.user_display_defaults.insert("B".into(), look("light", 500, true));
    });
    let (_, b) = t.defaults(&t.token_b).await;
    assert_eq!((b.theme.as_str(), b.width), ("light", 500));
    assert_eq!(t.defaults(&t.token_a).await.1.width, 900, "B's defaults left A's alone");
}

#[tokio::test]
async fn a_user_without_their_own_follows_the_server_wide_defaults() {
    let t = start().await;
    t.push(|s| s.display = look("dark", 800, true));
    assert_eq!(t.defaults(&t.token_a).await.1.width, 800);
    assert_eq!(t.defaults(&t.token_b).await.1.width, 800);
    let user_a = t.user_a.clone();
    t.push(|s| {
        s.user_display_defaults.insert(user_a, look("light", 600, true));
    });
    assert_eq!((t.defaults(&t.token_a).await.1.width, t.defaults(&t.token_b).await.1.width), (600, 800));
    assert_eq!(t.defaults("abt_not_a_token").await.0, 401);
}
