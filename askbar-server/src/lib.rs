//! Askbar server: the device API and, on its own listener, the management API the admin service uses.

pub mod api;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod chat;
pub mod db;
pub mod generation;
pub mod ids;
pub mod llm;
pub mod mgmt;
pub mod provider_test;
pub mod secrets;
pub mod snapshot;
pub mod store;
#[doc(hidden)]
pub mod testing;

pub use db::{make_title, ConversationSummary, Db, DbError, Generation, SearchHit, StoredMessage};
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
pub const MIN_CLIENT: &str = "0.4.0";

pub struct AppState {
    pub db: Mutex<Db>,
    secrets: Arc<Secrets>,
    pub http: llm::HttpClient,
    /// Oldest client version the API still serves.
    pub min_client: String,
    /// Running generations and their buffered output.
    pub generations: generation::Generations,
    /// Where generations leave their audit trace. Records nothing unless `with_audit` set one.
    pub audit: audit::Recorder,
    /// Tests only: replaces the configured generation timeout (see `testing`).
    generation_timeout_override: Mutex<Option<std::time::Duration>>,
}

impl AppState {
    pub fn new(db: Db, secrets: Secrets, min_client: &str) -> Arc<AppState> {
        Arc::new(AppState { db: Mutex::new(db), secrets: Arc::new(secrets), http: llm::http_client(), min_client: min_client.to_string(), generations: Default::default(), audit: audit::Recorder::noop(), generation_timeout_override: Mutex::new(None) })
    }

    /// Gives a fresh state an audit recorder. `new` keeps its signature and records nothing, so
    /// code and tests that do not care about the audit store are unchanged.
    pub fn with_audit(self: Arc<Self>, recorder: audit::Recorder) -> Arc<AppState> {
        let mut state = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_audit is for a state nothing else holds yet"));
        state.audit = recorder;
        Arc::new(state)
    }

    /// Tests only: make every generation started from now on use `d` as its timeout, whatever the
    /// snapshot says, so a test need not wait 30 seconds.
    #[doc(hidden)]
    pub fn set_generation_timeout_override(&self, d: Option<std::time::Duration>) {
        *self.generation_timeout_override.lock().unwrap_or_else(|e| e.into_inner()) = d;
    }

    #[doc(hidden)]
    pub fn generation_timeout_override(&self) -> Option<std::time::Duration> {
        *self.generation_timeout_override.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn secrets(&self) -> Arc<Secrets> {
        self.secrets.clone()
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
        .route("/api/v1/generations", get(api::generations))
        .route("/api/v1/generations/{id}/stream", get(generation::stream))
        .route("/api/v1/generations/{id}/cancel", post(generation::cancel))
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

/// Logs method, path and status only: no query string, headers or bodies.
async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let res = next.run(req).await;
    tracing::info!(%method, %path, status = res.status().as_u16(), "request");
    res
}

