//! Askbar server: the client API and, on its own listener, the server API the admin service uses.

pub mod audit;
pub mod auth;
pub mod backup;
pub mod chat;
pub mod client_api;
pub mod cost;
pub mod db;
pub mod error;
pub mod generation;
pub use askbar_service::ids;
pub mod llm;
pub mod models;
pub mod provider_test;
pub mod router;
pub mod routes;
pub mod seal;
pub mod secrets;
pub mod server_api;
pub mod snapshot;
pub mod space;
pub mod store;
#[doc(hidden)]
pub mod testing;

use askbar_proto::problem::ErrorCode;
use askbar_proto::{Health, PairRequest, PairResponse};
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware,
    middleware::Next,
    response::Response,
    Json, Router,
};
pub use db::{make_title, Db, DbError};
use error::{ApiError, ApiJson};
use secrets::Secrets;
pub use space::{ConversationSummary, Generation, ReplyBy, SearchHit, Space, SpaceStore, Spaces, StoredMessage};
use std::sync::{Arc, Mutex};

pub const API_VERSION: u32 = 1;
pub const MIN_CLIENT: &str = "0.6.0";

pub struct AppState {
    /// The control database (`control.db`). Lock it before a space, never while holding one.
    pub db: Mutex<Db>,
    /// The space files (ADR 0009). A handler gets one through `Spaces::for_member`.
    pub spaces: Spaces,
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
    /// New on every start of the process, so the admin can tell a restart from a long silence
    /// (`GET /api/server/v1/status`, ADR 0028).
    pub boot_id: askbar_service::status::BootId,
    /// What `POST /api/server/v1/restart` does; set once by `main` (tests set their own or none).
    pub restarter: std::sync::OnceLock<askbar_service::restart::Restarter>,
}

impl AppState {
    pub fn new(db: Db, secrets: Secrets, min_client: &str) -> Arc<AppState> {
        Arc::new(AppState {
            db: Mutex::new(db),
            spaces: Spaces::in_memory(),
            secrets: Arc::new(secrets),
            http: llm::http_client(),
            min_client: min_client.to_string(),
            generations: Default::default(),
            audit: audit::Recorder::noop(),
            generation_timeout_override: Mutex::new(None),
            boot_id: askbar_service::status::BootId::new(),
            restarter: Default::default(),
        })
    }

    /// Gives a fresh state an audit recorder. `new` keeps its signature and records nothing, so
    /// code and tests that do not care about the audit store are unchanged.
    pub fn with_audit(self: Arc<Self>, recorder: audit::Recorder) -> Arc<AppState> {
        let mut state = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_audit is for a state nothing else holds yet"));
        state.audit = recorder;
        Arc::new(state)
    }

    /// Gives a fresh state its space files (`Spaces::on_disk`); `new` keeps them in memory.
    pub fn with_spaces(self: Arc<Self>, spaces: Spaces) -> Arc<AppState> {
        let mut state = Arc::try_unwrap(self).unwrap_or_else(|_| panic!("with_spaces is for a state nothing else holds yet"));
        state.spaces = spaces;
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

/// All of the client API's routes, with the list of what was registered (see `routes`).
pub fn client_routes(state: &Arc<AppState>) -> routes::Routes {
    use routes::Routes;
    // Every route is under `/api/client/v1` (askbar_proto::paths::CLIENT_V1).
    let v1 = |rest: &str| askbar_proto::paths::join(askbar_proto::paths::CLIENT_V1, rest);
    let token = || middleware::from_fn_with_state(state.clone(), auth::require_client_token);
    let version = || middleware::from_fn_with_state(state.clone(), require_client_version);
    // Everything that needs a client token.
    let protected = Routes::new()
        // Lets a client check that its token is still accepted.
        .get(&v1("/me"), || async { StatusCode::NO_CONTENT })
        .get(&v1("/providers"), client_api::providers)
        .get(&v1("/me/prefs"), client_api::get_prefs)
        .put(&v1("/me/prefs"), client_api::put_prefs)
        .get(&v1("/me/display-defaults"), client_api::display_defaults)
        .post(&v1("/me/pairing-keys"), client_api::new_pairing_key)
        .get(&v1("/settings"), client_api::settings)
        // Spaces (ADR 0009, ADR 0022): every route below `{space_id}` takes `MemberSpace`, which
        // checks membership before the space's file is opened.
        .get(&v1("/spaces"), client_api::spaces)
        .get(&v1("/spaces/{space_id}/conversations"), client_api::conversations)
        .get(&v1("/spaces/{space_id}/conversations/{id}"), client_api::conversation)
        .delete(&v1("/spaces/{space_id}/conversations/{id}"), client_api::delete_conversation)
        .get(&v1("/spaces/{space_id}/search"), client_api::search)
        .post(&v1("/spaces/{space_id}/chat"), chat::chat)
        .get(&v1("/spaces/{space_id}/generations"), client_api::generations)
        .get(&v1("/spaces/{space_id}/generations/{id}/stream"), generation::stream)
        .post(&v1("/spaces/{space_id}/generations/{id}/cancel"), generation::cancel)
        .guarded(|r| r.route_layer(token()));
    // Everything except health checks, in this order: the client version (an old app is told to
    // update), then whether the admin has pushed configuration yet (503 not_configured, ADR 0028
    // rule 3; checked before the token, so an app is never told its token is refused while the
    // server waits for its settings), then the token.
    let gate = configured_gate(state);
    let gated = Routes::new()
        .post(&v1("/pairings"), pair)
        .merge(protected)
        .guarded(|r| r.route_layer(middleware::from_fn_with_state(gate, askbar_service::configured::require)))
        .guarded(|r| r.route_layer(version()));
    Routes::new()
        .get(&v1("/health"), health)
        // The first generation of the API lived at /api/v1. It serves nothing now; every request there, of any
        // method and with or without a token, is told to update the app.
        .any(&askbar_proto::paths::join(askbar_proto::paths::TOMBSTONE_V1, "/{*rest}"), update_the_app)
        .merge(gated)
}

pub fn client_router(state: Arc<AppState>) -> Router {
    let (router, _) = client_routes(&state).into_parts();
    router
        .fallback(error::not_found)
        .method_not_allowed_fallback(error::method_not_allowed)
        .with_state(state)
        .layer(middleware::from_fn(log_requests))
        .layer(middleware::from_fn(nosniff))
}

/// Open once a configuration has been stored: it is kept in `control.db`, so a restart stays
/// configured, and only a server that has never been sent one (a fresh volume) is closed. Health
/// and the old prefix are outside the gated routes, so no path needs to be left open here.
fn configured_gate(state: &Arc<AppState>) -> askbar_service::configured::Gate {
    let s = state.clone();
    askbar_service::configured::Gate::new(move || snapshot::exists(&s.db.lock().unwrap_or_else(|e| e.into_inner())), |_| false)
}

/// Every client API answer is JSON or the documented event stream; no browser may guess it is HTML
/// or script (ADR 0029 rule 4). Set on every response, errors and 404s included.
async fn nosniff(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    res.headers_mut().insert("x-content-type-options", axum::http::HeaderValue::from_static("nosniff"));
    res
}

/// The answer to anything under the old `/api/v1` prefix.
async fn update_the_app(State(state): State<Arc<AppState>>) -> ApiError {
    ApiError::code(ErrorCode::ClientTooOld).with("min_client", state.min_client.clone())
}

/// 400 if the client sends no usable version header, 426 if it is older than `min_client`.
async fn require_client_version(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Result<Response, ApiError> {
    let sent = req
        .headers()
        .get(askbar_proto::CLIENT_VERSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(askbar_proto::parse_version)
        .ok_or_else(|| ApiError::code(ErrorCode::ClientVersionMissing))?;
    match askbar_proto::parse_version(&state.min_client) {
        Some(min) if sent < min => Err(ApiError::code(ErrorCode::ClientTooOld).with("min_client", state.min_client.clone())),
        _ => Ok(next.run(req).await),
    }
}

async fn health(State(state): State<Arc<AppState>>) -> Json<Health> {
    Json(Health { api_version: API_VERSION, min_client: state.min_client.clone() })
}

async fn pair(State(state): State<Arc<AppState>>, ApiJson(req): ApiJson<PairRequest>) -> Result<Json<PairResponse>, ApiError> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match auth::redeem_pair_code(&db, req.code.trim()) {
        Ok(Some(token)) => Ok(Json(PairResponse { token })),
        Ok(None) => {
            Err(ApiError::new(ErrorCode::Unauthorized, "That pairing code is not valid. It may have expired or been used already."))
        }
        Err(_) => Err(ApiError::code(ErrorCode::Internal)),
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
