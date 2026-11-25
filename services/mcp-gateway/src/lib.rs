//! `mcp-gateway`'s front (ADR 0057): `/api/mcp-gateway/v1`, built on `askbar-service`.
//!
//! - **Config by push.** The admin pushes the tool limits in `PUT /config` (ADR 0028). A bad push is refused and the old
//!   config stays live. Until the first push every tool call is refused ([`Gateway::admit`]) and the defaults apply.
//! - **No tool route yet.** Only health, status and config exist; the tools, the approval check and the audit spans follow.
//! - **Links.** The token links (`server-mcp-gateway`, `admin-mcp-gateway`) are P5A.6a. Until then [`no_links`] accepts
//!   no token, so every route but health answers `401`.

pub mod config;
pub mod status;

use askbar_proto::http::ApiError;
use askbar_proto::paths::MCP_GATEWAY_V1;
use askbar_proto::ErrorCode;
use askbar_service::configured::Gate;
use askbar_service::links::{self, Links};
use askbar_service::status::BootId;
use axum::body::Bytes;
use axum::extract::State;
use axum::routing::{get, put};
use axum::{Json, Router};
use config::Config;
use status::Status;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

const MAX_BODY: usize = 64 * 1024;
pub const DEFAULT_PORT: u16 = 8087;

pub struct Gateway {
    pub boot_id: BootId,
    /// `host:port` of `searxng` (an address, never a secret); unset means search has nowhere to go.
    pub searxng_addr: Option<String>,
    config: RwLock<Option<Arc<Config>>>,
    calls: Mutex<BTreeMap<&'static str, u64>>,
}

impl Gateway {
    pub fn new(boot_id: BootId, searxng_addr: Option<String>) -> Arc<Gateway> {
        Arc::new(Gateway { boot_id, searxng_addr, config: RwLock::new(None), calls: Mutex::new(BTreeMap::new()) })
    }

    fn applied(&self) -> Option<Arc<Config>> {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn is_configured(&self) -> bool {
        self.applied().is_some()
    }

    /// The config in force: the pushed one, else the `askbar-proto` defaults.
    pub fn config(&self) -> Arc<Config> {
        self.applied().unwrap_or_default()
    }

    /// Counts one call that ended in `outcome`.
    pub fn count(&self, outcome: &'static str) {
        *self.calls.lock().unwrap_or_else(|e| e.into_inner()).entry(outcome).or_default() += 1;
    }

    /// The first check on a tool call: fail closed until the admin has pushed a config.
    pub fn admit(&self) -> Result<Arc<Config>, ApiError> {
        self.applied().ok_or_else(|| {
            self.count("refused_not_configured");
            ApiError::new(ErrorCode::NotConfigured, "The gateway has not received its settings yet.").retry_after(5)
        })
    }

    /// Replaces the config. It is checked first; a refused push changes nothing.
    pub fn apply_config(&self, c: Config) -> Result<u64, &'static str> {
        c.validate()?;
        let version = c.version();
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(c));
        tracing::info!(version, "configuration applied");
        Ok(version)
    }

    pub fn status(&self) -> Status {
        let calls = self.calls.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, v)| (k.to_string(), *v)).collect();
        Status { config_version: self.applied().map(|c| c.version()), boot_id: self.boot_id.as_str().to_string(), calls }
    }
}

/// No link yet (P5A.6a adds them): accepts no token, so nothing but health is reachable.
pub fn no_links() -> Links {
    Links::new(vec![]).named("mcp-gateway")
}

/// The routes, for the OpenAPI test.
pub fn routes() -> Vec<links::RouteEntry> {
    let mut r = links::routes(MCP_GATEWAY_V1, &[]);
    r.extend(
        [("GET", "/status"), ("PUT", "/config")].map(|(method, p)| links::RouteEntry { method, path: links::full(MCP_GATEWAY_V1, p) }),
    );
    r
}

fn full(rest: &str) -> String {
    links::full(MCP_GATEWAY_V1, rest)
}

pub fn router(g: Arc<Gateway>, links: Links) -> Router {
    let (a, open) = (g.clone(), [full("/health"), full("/status"), full("/config")]);
    let gate = Gate::new(move || a.is_configured(), move |p| open.iter().any(|o| o == p));
    let protected = Router::new()
        .route(&full("/status"), get(status))
        .route(&full("/config"), put(put_config))
        .layer(axum::middleware::from_fn_with_state(gate, askbar_service::configured::require))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    links::shell(MCP_GATEWAY_V1, MAX_BODY, g, protected)
}

async fn status(State(g): State<Arc<Gateway>>) -> Json<Status> {
    Json(g.status())
}

/// The answer holds the version only.
async fn put_config(State(g): State<Arc<Gateway>>, body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let bad = |d: &str| ApiError::new(ErrorCode::BadRequest, d);
    let c: Config = serde_json::from_slice(&body).map_err(|_| bad("The body is not a config: an unknown field or a wrong type."))?;
    let version = g.apply_config(c).map_err(bad)?;
    Ok(Json(serde_json::json!({ "version": version })))
}
