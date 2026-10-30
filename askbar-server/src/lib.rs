//! Askbar server. Two separate routers: the device API and the admin site.

pub mod admin;
pub mod admin_api;
pub mod api;
pub mod auth;
pub mod caddy;
pub mod chat;
pub mod db;
pub mod ids;
pub mod llm;
pub mod mgmt;
pub mod network;
pub mod secrets;
pub mod store;

pub use db::{make_title, ConversationSummary, Db, DbError, SearchHit, StoredMessage};
use askbar_proto::{Health, PairRequest, PairResponse};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware,
    middleware::Next,
    response::Response,
    response::Redirect,
    routing::{get, post, put},
    Json, Router,
};
use secrets::Secrets;
use std::sync::{Arc, Mutex};

pub const API_VERSION: u32 = 1;
pub const MIN_CLIENT: &str = "0.3.0";

pub struct AppState {
    pub db: Mutex<Db>,
    secrets: Arc<Secrets>,
    pub http: llm::HttpClient,
    pub admin: admin::AdminAuth,
    /// Oldest client version the API still serves.
    pub min_client: String,
    /// Where clients reach the API when TLS is off; goes into pairing strings.
    pub public_url: String,
    /// Host ports of the HTTPS front end (they decide the address in pairing strings).
    pub caddy: Mutex<caddy::CaddyLink>,
    /// How the last attempt to configure Caddy went.
    pub proxy: Mutex<caddy::ProxyStatus>,
    pub tls_api_port: std::sync::atomic::AtomicU16,
    pub tls_admin_port: std::sync::atomic::AtomicU16,
}

impl AppState {
    pub fn new(db: Db, secrets: Secrets, min_client: &str, public_url: &str) -> Arc<AppState> {
        Arc::new(AppState {
            db: Mutex::new(db),
            secrets: Arc::new(secrets),
            http: llm::http_client(),
            admin: admin::AdminAuth::default(),
            min_client: min_client.to_string(),
            public_url: public_url.trim_end_matches('/').to_string(),
            caddy: Mutex::new(caddy::CaddyLink::default()),
            proxy: Mutex::new(caddy::ProxyStatus::default()),
            tls_api_port: 8443.into(),
            tls_admin_port: 9443.into(),
        })
    }

    pub fn secrets(&self) -> Arc<Secrets> {
        self.secrets.clone()
    }

    pub fn set_caddy(&self, link: caddy::CaddyLink) {
        *self.caddy.lock().unwrap_or_else(|e| e.into_inner()) = link;
    }

    pub fn set_tls_ports(&self, api: u16, admin: u16) {
        self.tls_api_port.store(api, std::sync::atomic::Ordering::Relaxed);
        self.tls_admin_port.store(admin, std::sync::atomic::Ordering::Relaxed);
    }
}

pub fn api_router(state: Arc<AppState>) -> Router {
    // Everything that needs a device token goes in `protected`.
    let protected: Router<Arc<AppState>> = Router::new()
        // Lets a client check that its token is still accepted.
        .route("/api/v1/whoami", get(|| async { StatusCode::NO_CONTENT }))
        .route("/api/v1/providers", get(api::providers))
        .route("/api/v1/conversations", get(api::conversations))
        .route("/api/v1/conversations/{id}", get(api::conversation).delete(api::delete_conversation))
        .route("/api/v1/me/prefs", get(api::get_prefs).put(api::put_prefs))
        .route("/api/v1/me/display-defaults", get(api::display_defaults))
        .route("/api/v1/search", get(api::search))
        .route("/api/v1/settings", get(api::settings))
        .route("/api/v1/chat", post(chat::chat))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_device_token));
    // Everything except health checks the client version first.
    let gated = Router::new()
        .route("/api/v1/pair", post(pair))
        .merge(protected)
        .route_layer(middleware::from_fn_with_state(state.clone(), require_client_version));
    Router::new()
        .route("/api/v1/health", get(health))
        .merge(gated)
        .with_state(state)
        .layer(middleware::from_fn(log_requests))
}

/// 400 if the client sends no usable version header, 426 if it is older than `min_client`.
async fn require_client_version(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Result<Response, StatusCode> {
    let sent = req
        .headers()
        .get(askbar_proto::CLIENT_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(askbar_proto::parse_version)
        .ok_or(StatusCode::BAD_REQUEST)?;
    match askbar_proto::parse_version(&state.min_client) {
        Some(min) if sent < min => Err(StatusCode::UPGRADE_REQUIRED),
        _ => Ok(next.run(req).await),
    }
}

pub fn admin_router(state: Arc<AppState>) -> Router {
    let protected: Router<Arc<AppState>> = Router::new()
        .route("/admin/session", get(|| async { StatusCode::NO_CONTENT }))
        .route("/admin/logout", post(admin::logout))
        .route("/admin/api/providers", get(admin_api::list_providers).post(admin_api::create_provider))
        .route("/admin/api/providers/{id}", put(admin_api::update_provider).delete(admin_api::delete_provider))
        .route("/admin/api/providers/{id}/test", post(admin_api::test_provider))
        .route("/admin/api/password", post(admin::change_password))
        .route("/admin/api/network", get(network::get_network).put(network::put_network))
        .route("/admin/api/network/apply", post(network::apply_network))
        .route("/admin/api/backup", get(admin_api::backup))
        .route("/admin/api/devices", get(admin_api::list_devices))
        .route("/admin/api/devices/{id}/revoke", post(admin_api::revoke_device))
        .route("/admin/api/pair", post(admin_api::create_pairing))
        .route("/admin/api/settings", get(admin_api::get_settings).put(admin_api::put_settings))
        .route("/admin/api/users/{id}/display-defaults", put(admin_api::put_user_display))
        .route_layer(middleware::from_fn_with_state(state.clone(), admin::require_session));
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin/") }))
        .route("/admin", get(|| async { Redirect::to("/admin/") }))
        .route("/admin/", get(|| async { admin_asset("text/html; charset=utf-8", include_str!("admin_ui/index.html")) }))
        .route("/admin/app.js", get(|| async { admin_asset("text/javascript; charset=utf-8", include_str!("admin_ui/app.js")) }))
        .route("/admin/style.css", get(|| async { admin_asset("text/css; charset=utf-8", include_str!("admin_ui/style.css")) }))
        .route("/admin/health", get(admin_health))
        .route("/admin/login", post(admin::login))
        .route("/admin/api/setup-status", get(admin::setup_status))
        .route("/admin/api/setup/password", post(admin::setup_password))
        .merge(protected)
        .layer(middleware::from_fn(admin::require_admin_header))
        .layer(middleware::from_fn(admin::security_headers))
        .with_state(state)
        .layer(middleware::from_fn(log_requests))
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Health> {
    Json(Health { api_version: API_VERSION, min_client: state.min_client.clone() })
}

async fn pair(State(state): State<Arc<AppState>>, Json(req): Json<PairRequest>) -> Result<Json<PairResponse>, StatusCode> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match auth::redeem_pair_code(&db, req.code.trim()) {
        Ok(Some(token)) => Ok(Json(PairResponse { token })),
        Ok(None) => Err(StatusCode::UNAUTHORIZED),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn admin_health() -> &'static str {
    "ok"
}

/// Logs method, path and status only: no query string, headers or bodies.
async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let res = next.run(req).await;
    tracing::info!(%method, %path, status = res.status().as_u16(), "request");
    res
}

fn admin_asset(content_type: &'static str, body: &'static str) -> impl axum::response::IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, content_type)], body)
}
