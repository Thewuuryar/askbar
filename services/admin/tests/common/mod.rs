//! Shared set-up for the admin tests that need a server, a fake Caddy, or both.
#![allow(dead_code)]

use askbar_admin::caddy::CaddyLink;
use askbar_admin::secrets::AdminSecrets;
use askbar_admin::server_link::ServerLink;
use askbar_admin::{web, AdminDb, AdminState};
use askbar_server::{secrets::Secrets, AppState};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
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
    /// The in-process auditor and gateway the server records to and reads from.
    pub rig: askbar_server::testing::Rig,
    /// The in-process gateway the admin pushes to and asks for models and provider tests (ADR 0054).
    pub gateway: Arc<askbar_model_gateway::Gateway>,
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

/// A real gateway on the admin's listener, with the token and hash files `bootstrap` made under `dir`, and the
/// admin's link to it. Nothing records to an auditor here: the routes the admin uses (configuration, models, the
/// provider test) need none. `up = false` leaves the link pointing where nothing listens.
pub async fn start_gateway(
    dir: &std::path::Path,
    up: bool,
) -> (Arc<askbar_model_gateway::Gateway>, Arc<askbar_admin::gateway_link::GatewayLink>) {
    let gw_tokens = dir.join("model-gateway-token");
    let boot = askbar_service::status::BootId::new();
    let audit = askbar_service::audit_client::AuditClient::new("http://127.0.0.1:1", gw_tokens.clone(), "model-gateway-auditor");
    let restarter = askbar_service::restart::Restarter::for_tests(
        boot.clone(),
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        || {},
        || {},
    );
    let gateway = askbar_model_gateway::Gateway::new(audit, boot, restarter);
    let url = if up {
        serve(askbar_model_gateway::admin_router(gateway.clone(), askbar_model_gateway::admin_links(&gw_tokens))).await
    } else {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap()) // dropped: nothing listens
    };
    let token = askbar_admin::gateway_link::read_token(&dir.join("admin-token")).unwrap();
    (gateway, askbar_admin::gateway_link::GatewayLink::new(&url, token))
}

/// An admin wired to a real in-process server and a fake Caddy. `caddy_up = false` leaves the
/// Caddy socket path in place with nothing listening on it.
pub async fn start(caddy_up: bool) -> T {
    start_with(caddy_up, true).await
}

/// `with_gateway = false` leaves the admin's gateway link unset, for a test that sets its own.
pub async fn start_with(caddy_up: bool, with_gateway: bool) -> T {
    let dir = std::env::temp_dir().join(format!("ab-{}", &ulid::Ulid::new().to_string()[16..]));
    std::fs::create_dir_all(&dir).unwrap();
    let (token_file, hash_file) = askbar_bootstrap::install_for_tests(&dir);
    let token = std::fs::read_to_string(token_file).unwrap();

    let sdb = askbar_server::Db::open_in_memory().unwrap();
    sdb.seed_server().unwrap();
    // The server reads an in-process auditor, so tests can put sealed content there and open it (`rig.seed`).
    let rig = askbar_server::testing::Rig::start().await;
    let server = rig.attach(AppState::new(sdb, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT)).await;
    let server_api_url = serve(askbar_server::server_api::router(server.clone(), hash_file)).await;

    let fake = Arc::new(FakeCaddy::default());
    let caddy_socket = dir.join("c").join("a.sock");
    std::fs::create_dir_all(caddy_socket.parent().unwrap()).unwrap();
    if caddy_up {
        let app =
            Router::new().route("/load", post(load)).route("/config/", axum::routing::get(|| async { "{}" })).with_state(fake.clone());
        let listener = tokio::net::UnixListener::bind(&caddy_socket).unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    }
    let secrets_dir = dir.join("caddy-secrets");

    let (gateway, gateway_link) = start_gateway(&dir, true).await;

    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PW).unwrap();
    db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U1', 'tester', 0)", []).unwrap();
    let admin = AdminState::build(db, Some(ServerLink::new(&server_api_url, token)), AdminSecrets::ephemeral());
    if with_gateway {
        admin.gateway.set(gateway_link).ok().unwrap();
    }
    admin.set_caddy(CaddyLink { socket: Some(caddy_socket.clone()), secrets_dir: Some(secrets_dir.clone()) });
    let adm = serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": PW }))
        .send()
        .await
        .unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { rig, gateway, adm, cookie, http, admin, server, fake, dir, secrets_dir, caddy_socket }
}

impl T {
    pub async fn call(&self, m: M, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .request(m, format!("{}{path}", self.adm))
            .header("x-askbar-admin", "1")
            .header("cookie", &self.cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        let st = r.status().as_u16();
        (st, serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null))
    }
    pub async fn save(&self, body: Value) -> (u16, Value) {
        self.call(M::PUT, "/api/admin/v1/network", body).await
    }
    pub async fn stored_token(&self) -> Option<Vec<u8>> {
        let db = self.admin.db.lock().await;
        askbar_admin::network::stored_token(db.conn())
    }
    pub fn token_file(&self) -> Option<String> {
        std::fs::read_to_string(self.secrets_dir.join("cf_token")).ok()
    }
}
