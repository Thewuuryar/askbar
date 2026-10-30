//! Shared set-up for the admin tests that need a server, a fake Caddy, or both.
#![allow(dead_code)]

use askbar_admin::caddy::CaddyLink;
use askbar_admin::mgmt::{self, ServerLink};
use askbar_admin::secrets::AdminSecrets;
use askbar_admin::{web, AdminDb, AdminState};
use askbar_server::{secrets::Secrets, AppState};
use axum::{extract::State, http::{HeaderMap, StatusCode}, routing::post, Router};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const PW: &str = "first password 123";
pub const TOKEN: &str = "cf_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

/// A stand-in for Caddy's admin API, on a unix socket: it records what it is sent.
#[derive(Default)]
pub struct FakeCaddy {
    pub loads: Mutex<Vec<(String, String)>>, // (content-type, body)
    pub refuse: Mutex<bool>,
}

async fn load(State(f): State<Arc<FakeCaddy>>, headers: HeaderMap, body: String) -> (StatusCode, String) {
    f.loads.lock().unwrap().push((headers["content-type"].to_str().unwrap().to_string(), body));
    if *f.refuse.lock().unwrap() {
        (StatusCode::BAD_REQUEST, "adapting config using caddyfile: unknown directive".into())
    } else {
        (StatusCode::OK, String::new())
    }
}

pub struct T {
    pub adm: String,
    pub cookie: String,
    pub http: reqwest::Client,
    pub admin: Arc<AdminState>,
    pub server: Arc<AppState>,
    pub fake: Arc<FakeCaddy>,
    pub dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub caddy_socket: PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub async fn serve(app: Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

/// An admin wired to a real in-process server and a fake Caddy. `caddy_up = false` leaves the
/// Caddy socket path in place with nothing listening on it.
pub async fn start(caddy_up: bool) -> T {
    let dir = std::env::temp_dir().join(format!("ab-{}", &ulid::Ulid::new().to_string()[16..]));
    std::fs::create_dir_all(&dir).unwrap();
    let (admin_secrets, mgmt_dir) = (dir.join("admin-secrets"), dir.join("askbar-mgmt"));
    let token = mgmt::ensure_token(&admin_secrets, &mgmt_dir).unwrap();

    let sdb = askbar_server::Db::open_in_memory().unwrap();
    sdb.seed_server().unwrap();
    let server = AppState::new(sdb, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    let mgmt_url = serve(askbar_server::mgmt::router(server.clone(), mgmt_dir.join(mgmt::HASH_FILE))).await;

    let fake = Arc::new(FakeCaddy::default());
    let caddy_socket = dir.join("c").join("a.sock");
    std::fs::create_dir_all(caddy_socket.parent().unwrap()).unwrap();
    if caddy_up {
        let app = Router::new().route("/load", post(load)).with_state(fake.clone());
        let listener = tokio::net::UnixListener::bind(&caddy_socket).unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    }
    let secrets_dir = dir.join("caddy-secrets");

    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PW).unwrap();
    let admin = AdminState::build(db, Some(ServerLink::new(&mgmt_url, token)), AdminSecrets::ephemeral());
    admin.set_caddy(CaddyLink { socket: Some(caddy_socket.clone()), secrets_dir: Some(secrets_dir.clone()) });
    let adm = serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { adm, cookie, http, admin, server, fake, dir, secrets_dir, caddy_socket }
}

impl T {
    pub async fn call(&self, m: M, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.request(m, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").header("cookie", &self.cookie).json(&body).send().await.unwrap();
        let st = r.status().as_u16();
        (st, serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null))
    }
    pub async fn save(&self, body: Value) -> (u16, Value) {
        self.call(M::PUT, "/admin/api/network", body).await
    }
    pub async fn stored_token(&self) -> Option<Vec<u8>> {
        let db = self.admin.db.lock().await;
        askbar_admin::network::stored_token(db.conn())
    }
    pub fn token_file(&self) -> Option<String> {
        std::fs::read_to_string(self.secrets_dir.join("cf_token")).ok()
    }
}
