use askbar_server::{admin, caddy::CaddyLink, secrets::Secrets, AppState};
use axum::{extract::State, http::{HeaderMap, StatusCode}, routing::post, Router};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const PW: &str = "first password 123";
const TOKEN: &str = "cf_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

#[derive(Default)]
struct Fake {
    loads: Mutex<Vec<(String, String)>>, // (content-type, body)
    refuse: Mutex<bool>,
}

async fn load(State(f): State<Arc<Fake>>, headers: HeaderMap, body: String) -> (StatusCode, String) {
    f.loads.lock().unwrap().push((headers["content-type"].to_str().unwrap().to_string(), body));
    if *f.refuse.lock().unwrap() {
        (StatusCode::BAD_REQUEST, "adapting config using caddyfile: unknown directive".into())
    } else {
        (StatusCode::OK, String::new())
    }
}

struct T {
    adm: String,
    state: Arc<AppState>,
    fake: Arc<Fake>,
    cookie: String,
    secrets_dir: std::path::PathBuf,
    http: reqwest::Client,
}
impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.secrets_dir);
    }
}

async fn start(caddy_up: bool) -> T {
    let fake = Arc::new(Fake::default());
    let app = Router::new().route("/load", post(load)).with_state(fake.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut admin_url = format!("http://{}", l.local_addr().unwrap());
    if caddy_up {
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    } else {
        drop(l); // nothing listens: connection refused
        admin_url = format!("http://127.0.0.1:{}", std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port());
    }
    let secrets_dir = std::env::temp_dir().join(format!("askbar-caddy-secrets-{}", ulid::Ulid::new()));

    let db = askbar_core::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    admin::init_password(&db, PW).unwrap();
    let state = AppState::with_config(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0", "http://localhost:8080");
    state.set_caddy(CaddyLink { admin_url: Some(admin_url), secrets_dir: Some(secrets_dir.clone()) });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let adm = format!("http://{}", l.local_addr().unwrap());
    let router = askbar_server::admin_router(state.clone());
    tokio::spawn(async move { axum::serve(l, router).await.unwrap() });
    let http = reqwest::Client::new();
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { adm, state, fake, cookie, secrets_dir, http }
}

impl T {
    async fn call(&self, m: M, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.request(m, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").header("cookie", &self.cookie).json(&body).send().await.unwrap();
        let st = r.status().as_u16();
        (st, serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null))
    }
    fn token_file(&self) -> Option<String> {
        std::fs::read_to_string(self.secrets_dir.join("cf_token")).ok()
    }
}

fn settings(tls: bool) -> Value {
    json!({ "tls_enabled": tls, "hostname": "askbar.orraitu.com", "admin_cidr": "192.168.1.0/24", "acme_staging": true, "cloudflare_token": TOKEN })
}

#[tokio::test]
async fn saving_pushes_the_config_and_writes_the_token_file() {
    let t = start(true).await;
    let (st, v) = t.call(M::PUT, "/admin/api/network", settings(true)).await;
    assert_eq!(st, 200);
    assert_eq!(v["proxy"]["ok"], true, "{v}");

    let loads = t.fake.loads.lock().unwrap().clone();
    assert_eq!(loads.len(), 1);
    assert_eq!(loads[0].0, "text/caddyfile");
    let body = &loads[0].1;
    assert!(body.contains("askbar.orraitu.com:8443") && body.contains("remote_ip 192.168.1.0/24") && body.contains(askbar_server::caddy::STAGING_CA));
    assert!(!body.contains(TOKEN), "the token never goes into the config");
    assert_eq!(t.token_file().as_deref(), Some(TOKEN));
    #[cfg(unix)]
    assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(t.secrets_dir.join("cf_token")).unwrap().permissions()) & 0o777, 0o600);
    // The status is remembered and served back.
    assert_eq!(t.call(M::GET, "/admin/api/network", json!({})).await.1["proxy"]["ok"], true);
}

#[tokio::test]
async fn turning_https_off_serves_nothing_and_removes_the_token_file() {
    let t = start(true).await;
    t.call(M::PUT, "/admin/api/network", settings(true)).await;
    assert!(t.token_file().is_some());
    let mut off = settings(false);
    off.as_object_mut().unwrap().remove("cloudflare_token");
    let (_, v) = t.call(M::PUT, "/admin/api/network", off).await;
    assert_eq!(v["proxy"]["ok"], true);
    assert!(t.token_file().is_none(), "no plaintext token lying around while HTTPS is off");
    let last = t.fake.loads.lock().unwrap().last().unwrap().1.clone();
    assert!(!last.contains("askbar.orraitu.com"));
    // The encrypted copy is still stored, so turning HTTPS back on needs no re-entry.
    assert_eq!(t.call(M::GET, "/admin/api/network", json!({})).await.1["has_token"], true);
}

#[tokio::test]
async fn a_refusal_from_caddy_is_reported_but_the_settings_stay_saved() {
    let t = start(true).await;
    *t.fake.refuse.lock().unwrap() = true;
    let (st, v) = t.call(M::PUT, "/admin/api/network", settings(true)).await;
    assert_eq!(st, 200);
    assert_eq!(v["proxy"]["ok"], false);
    let msg = v["proxy"]["message"].as_str().unwrap();
    assert!(msg.contains("Caddy refused") && msg.contains("unknown directive"), "{msg}");
    assert_eq!(v["hostname"], "askbar.orraitu.com", "saved anyway");

    // Fixing Caddy and pressing Apply works without re-entering anything.
    *t.fake.refuse.lock().unwrap() = false;
    let (st, v) = t.call(M::POST, "/admin/api/network/apply", json!({})).await;
    assert_eq!((st, v["ok"].as_bool()), (200, Some(true)));
}

#[tokio::test]
async fn caddy_being_down_is_a_message_not_an_error() {
    let t = start(false).await;
    let (st, v) = t.call(M::PUT, "/admin/api/network", settings(true)).await;
    assert_eq!(st, 200);
    assert_eq!(v["proxy"]["ok"], false);
    assert!(v["proxy"]["message"].as_str().unwrap().contains("Could not reach Caddy"));
}

#[tokio::test]
async fn without_a_caddy_link_the_status_says_so() {
    let t = start(true).await;
    t.state.set_caddy(CaddyLink::default());
    let (_, v) = t.call(M::POST, "/admin/api/network/apply", json!({})).await;
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().contains("tls compose profile"));
    assert!(t.fake.loads.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_token_that_cannot_be_read_is_not_pushed() {
    let t = start(true).await;
    t.call(M::PUT, "/admin/api/network", settings(true)).await;
    t.state.db.lock().unwrap().conn().execute("UPDATE settings SET value = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA' WHERE key = 'cloudflare_token'", []).unwrap();
    t.fake.loads.lock().unwrap().clear();
    let (_, v) = t.call(M::POST, "/admin/api/network/apply", json!({})).await;
    assert_eq!(v["ok"], false);
    assert!(v["message"].as_str().unwrap().contains("Cloudflare token"));
    assert!(t.fake.loads.lock().unwrap().is_empty());
}
