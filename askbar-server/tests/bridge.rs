//! The temporary bridge: the old admin site's changes reach the snapshot, and the device
//! API reads only the snapshot.

use askbar_proto::ProviderInfo;
use askbar_server::{admin, auth, secrets::Secrets, AppState};
use reqwest::Method as M;
use serde_json::{json, Value};

const PW: &str = "correct horse battery";

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

struct T {
    api: String,
    adm: String,
    cookie: String,
    token: String,
    http: reqwest::Client,
    state: std::sync::Arc<AppState>,
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    admin::init_password(&db, PW).unwrap();
    let code = auth::create_pair_code(&db, &user, "t", 600).unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080");
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let (api, adm) = (serve(askbar_server::api_router(state.clone())).await, serve(askbar_server::admin_router(state.clone())).await);
    let http = reqwest::Client::new();
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { api, adm, cookie, token, http, state }
}

impl T {
    async fn admin(&self, m: M, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.http.request(m, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").header("cookie", &self.cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        (resp.status().as_u16(), serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
    async fn providers(&self) -> Vec<ProviderInfo> {
        self.http
            .get(format!("{}/api/v1/providers", self.api))
            .bearer_auth(&self.token)
            .header("x-askbar-client-version", "0.3.0")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
}

fn input(name: &str, model: &str) -> Value {
    json!({ "name": name, "type": "openai", "base_url": "http://x/v1", "model": model, "api_key": "sk-bridge-secret" })
}

#[tokio::test]
async fn changing_a_provider_in_the_old_admin_changes_the_device_api() {
    let t = start().await;
    assert!(t.providers().await.is_empty());

    let (st, created) = t.admin(M::POST, "/admin/api/providers", Some(input("first", "m1"))).await;
    assert_eq!(st, 201);
    let id = created["id"].as_str().unwrap().to_string();
    let list = t.providers().await;
    assert_eq!((list.len(), list[0].id.as_str(), list[0].name.as_str(), list[0].model.as_str()), (1, id.as_str(), "first", "m1"));

    assert_eq!(t.admin(M::PUT, &format!("/admin/api/providers/{id}"), Some(input("renamed", "m2"))).await.0, 204);
    let list = t.providers().await;
    assert_eq!((list[0].name.as_str(), list[0].model.as_str()), ("renamed", "m2"), "an edit shows up at once");

    t.admin(M::POST, "/admin/api/providers", Some(input("another", "m3"))).await;
    assert_eq!(t.providers().await.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["another", "renamed"]);

    assert_eq!(t.admin(M::DELETE, &format!("/admin/api/providers/{id}"), None).await.0, 204);
    assert_eq!(t.providers().await.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["another"]);

    // A refused change does not touch the snapshot.
    let before = askbar_server::snapshot::current(&t.state.db.lock().unwrap()).unwrap().unwrap().version;
    assert_eq!(t.admin(M::POST, "/admin/api/providers", Some(input("another", "dup"))).await.0, 409);
    let after = askbar_server::snapshot::current(&t.state.db.lock().unwrap()).unwrap().unwrap().version;
    assert_eq!(before, after);
}

#[tokio::test]
async fn the_snapshot_holds_no_key_and_is_rebuilt_at_startup() {
    let t = start().await;
    t.admin(M::POST, "/admin/api/providers", Some(input("first", "m1"))).await;
    let doc: String = t.state.db.lock().unwrap().conn().query_row("SELECT body FROM config_snapshot", [], |r| r.get(0)).unwrap();
    assert!(!doc.contains("sk-bridge-secret") && !doc.contains("api_key"), "{doc}");

    // Rows written while the server was down appear once it starts and rebuilds.
    t.state.db.lock().unwrap().conn().execute("INSERT INTO settings VALUES ('system_prompt', 'Be terse.')", []).unwrap();
    assert_eq!(askbar_server::snapshot::current(&t.state.db.lock().unwrap()).unwrap().unwrap().system_prompt, None);
    askbar_server::bridge::rebuild(&t.state.db.lock().unwrap()).unwrap();
    assert_eq!(askbar_server::snapshot::current(&t.state.db.lock().unwrap()).unwrap().unwrap().system_prompt.as_deref(), Some("Be terse."));
}

#[tokio::test]
async fn the_order_clients_see_follows_the_snapshots_default_and_fallbacks() {
    let t = start().await;
    let ids: Vec<String> = {
        let mut v = Vec::new();
        for n in ["a", "b", "c"] {
            let (_, c) = t.admin(M::POST, "/admin/api/providers", Some(input(n, "m"))).await;
            v.push(c["id"].as_str().unwrap().to_string());
        }
        v
    };
    assert_eq!(t.providers().await.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
    {
        let db = t.state.db.lock().unwrap();
        let mut s = askbar_server::snapshot::current(&db).unwrap().unwrap();
        s.default_provider_id = Some(ids[2].clone());
        s.fallback_order = vec![ids[1].clone()];
        // Stored the way the admin service will: through the management API's validation and save.
        askbar_server::snapshot::validate(&s).unwrap();
        db.conn().execute("UPDATE config_snapshot SET body = ?1", [serde_json::to_string(&s).unwrap()]).unwrap();
    }
    assert_eq!(t.providers().await.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["c", "b", "a"], "default, fallbacks, rest");
}
