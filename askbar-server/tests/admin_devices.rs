use askbar_proto::PairResponse;
use askbar_server::{admin, secrets::Secrets, AppState};
use reqwest::Method as M;
use serde_json::{json, Value};

const PASSWORD: &str = "correct horse battery";
const PUBLIC: &str = "https://askbar.example.com:8443";

struct T {
    api: String,
    admin: String,
    cookie: String,
    http: reqwest::Client,
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    admin::init_password(&db, PASSWORD).unwrap();
    let state = AppState::with_config(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0", &format!("{PUBLIC}/"));
    let (api, admin_url) = (serve(askbar_server::api_router(state.clone())).await, serve(askbar_server::admin_router(state)).await);
    let http = reqwest::Client::new();
    let r = http.post(format!("{admin_url}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PASSWORD })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { api, admin: admin_url, cookie, http }
}

impl T {
    async fn admin(&self, method: M, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.http.request(method, format!("{}{path}", self.admin)).header("x-askbar-admin", "1").header("cookie", &self.cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let st = resp.status().as_u16();
        (st, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
    async fn pair_from_string(&self, pairing: &str) -> reqwest::Response {
        let (url, code) = askbar_proto::decode_pairing(pairing).expect("a pairing string");
        assert_eq!(url, PUBLIC, "trailing slash trimmed, public URL used");
        self.http
            .post(format!("{}/api/v1/pair", self.api))
            .header("x-askbar-client-version", "0.1.0")
            .json(&json!({ "code": code }))
            .send()
            .await
            .unwrap()
    }
    async fn whoami(&self, token: &str) -> u16 {
        self.http.get(format!("{}/api/v1/whoami", self.api)).bearer_auth(token).header("x-askbar-client-version", "0.1.0").send().await.unwrap().status().as_u16()
    }
}

#[tokio::test]
async fn pair_from_the_page_then_revoke() {
    let t = start().await;
    assert_eq!(t.admin(M::GET, "/admin/api/devices", None).await.1, json!([]));

    let (st, out) = t.admin(M::POST, "/admin/api/pair", Some(json!({ "name": "Work laptop" }))).await;
    assert_eq!(st, 200);
    assert_eq!(out["expires_in"], 600);
    let pairing = out["pairing"].as_str().unwrap().to_string();

    let r = t.pair_from_string(&pairing).await;
    assert_eq!(r.status(), 200);
    let token = r.json::<PairResponse>().await.unwrap().token;
    assert_eq!(t.whoami(&token).await, 204);
    assert_eq!(t.pair_from_string(&pairing).await.status(), 401, "single use");

    let (_, list) = t.admin(M::GET, "/admin/api/devices", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1);
    let d = &list[0];
    assert_eq!((d["name"].as_str(), d["revoked"].as_bool()), (Some("Work laptop"), Some(false)));
    assert!(d["last_seen"].is_i64(), "whoami updated last_seen");
    assert!(!list.to_string().contains(&token) && !list.to_string().contains("hash"));

    let id = d["id"].as_str().unwrap();
    assert_eq!(t.admin(M::POST, &format!("/admin/api/devices/{id}/revoke"), None).await.0, 204);
    assert_eq!(t.whoami(&token).await, 401, "the revoked device is rejected");
    assert_eq!(t.admin(M::GET, "/admin/api/devices", None).await.1[0]["revoked"], true);
    assert_eq!(t.admin(M::POST, "/admin/api/devices/nope/revoke", None).await.0, 404);
}

#[tokio::test]
async fn blank_names_and_long_names() {
    let t = start().await;
    let (st, out) = t.admin(M::POST, "/admin/api/pair", Some(json!({}))).await;
    assert_eq!(st, 200);
    let r = t.pair_from_string(out["pairing"].as_str().unwrap()).await;
    assert_eq!(r.status(), 200);
    assert_eq!(t.admin(M::GET, "/admin/api/devices", None).await.1[0]["name"], "device");
    assert_eq!(t.admin(M::POST, "/admin/api/pair", Some(json!({ "name": "x".repeat(61) }))).await.0, 400);
}

#[tokio::test]
async fn devices_need_an_admin_session() {
    let t = start().await;
    for (m, p) in [(M::GET, "/admin/api/devices"), (M::POST, "/admin/api/pair"), (M::POST, "/admin/api/devices/x/revoke")] {
        let r = t.http.request(m, format!("{}{p}", t.admin)).header("x-askbar-admin", "1").json(&json!({})).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
}
