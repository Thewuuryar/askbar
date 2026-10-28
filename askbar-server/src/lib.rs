//! Askbar server. Two separate routers: the device API and the admin site.

pub mod admin;
pub mod admin_api;
pub mod api;
pub mod auth;
pub mod chat;
pub mod import;
pub mod secrets;
pub mod store;

use askbar_core::Db;
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
pub const MIN_CLIENT: &str = "0.1.0";

pub struct AppState {
    pub db: Mutex<Db>,
    pub secrets: Secrets,
    pub http: askbar_core::HttpClient,
    pub admin: admin::AdminAuth,
    /// Oldest client version the API still serves.
    pub min_client: String,
}

impl AppState {
    pub fn new(db: Db, secrets: Secrets) -> Arc<AppState> {
        AppState::with_min_client(db, secrets, MIN_CLIENT)
    }

    pub fn with_min_client(db: Db, secrets: Secrets, min_client: &str) -> Arc<AppState> {
        Arc::new(AppState {
            db: Mutex::new(db),
            secrets,
            http: askbar_core::http_client(),
            admin: admin::AdminAuth::default(),
            min_client: min_client.to_string(),
        })
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
        .route_layer(middleware::from_fn_with_state(state.clone(), admin::require_session));
    Router::new()
        .route("/", get(|| async { Redirect::to("/admin/") }))
        .route("/admin", get(|| async { Redirect::to("/admin/") }))
        .route("/admin/", get(|| async { admin_asset("text/html; charset=utf-8", include_str!("admin_ui/index.html")) }))
        .route("/admin/app.js", get(|| async { admin_asset("text/javascript; charset=utf-8", include_str!("admin_ui/app.js")) }))
        .route("/admin/style.css", get(|| async { admin_asset("text/css; charset=utf-8", include_str!("admin_ui/style.css")) }))
        .route("/admin/health", get(admin_health))
        .route("/admin/login", post(admin::login))
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
