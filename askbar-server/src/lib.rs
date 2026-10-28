//! Askbar server. Two separate routers: the device API and the admin site.

pub mod auth;
pub mod import;
pub mod secrets;

use askbar_core::Db;
use askbar_proto::{Health, PairRequest, PairResponse};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware,
    middleware::Next,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use secrets::Secrets;
use std::sync::{Arc, Mutex};

pub const API_VERSION: u32 = 1;
pub const MIN_CLIENT: &str = "0.1.0";

pub struct AppState {
    pub db: Mutex<Db>,
    pub secrets: Secrets,
}

impl AppState {
    pub fn new(db: Db, secrets: Secrets) -> Arc<AppState> {
        Arc::new(AppState { db: Mutex::new(db), secrets })
    }
}

pub fn api_router(state: Arc<AppState>) -> Router {
    // Everything that needs a device token goes in `protected`.
    let protected: Router<Arc<AppState>> = Router::new()
        // Lets a client check that its token is still accepted.
        .route("/api/v1/whoami", get(|| async { StatusCode::NO_CONTENT }))
        .route_layer(middleware::from_fn_with_state(state.clone(), auth::require_device_token));
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/pair", post(pair))
        .merge(protected)
        .with_state(state)
        .layer(middleware::from_fn(log_requests))
}

pub fn admin_router(_state: Arc<AppState>) -> Router {
    Router::new().route("/admin/health", get(admin_health)).layer(middleware::from_fn(log_requests))
}

async fn health() -> Json<Health> {
    Json(Health { api_version: API_VERSION, min_client: MIN_CLIENT.into() })
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
