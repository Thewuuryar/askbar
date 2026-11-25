//! `mcp-gateway`'s front (ADR 0057): `/api/mcp-gateway/v1`, built on `askbar-service`.
//!
//! - **Config by push.** The admin pushes the tool limits in `PUT /config` (ADR 0028). A bad push is refused and the old
//!   config stays live. Until the first push every tool call is refused ([`Gateway::admit`]) and the defaults apply.
//! - **One tool route.** `POST /tools/{tool}/call` checks the signed approval, then the user's tool permission, then writes a `tool` span and waits for
//!   the auditor's acknowledgement (no acknowledgement: `503 audit_backlog`, nothing runs), then refuses
//!   (`tool_not_available`): the tools follow. `GET /tools?user=` lists the tools a user holds.
//! - **Links.** One listener, two token links ([`LINKS`]): the admin's (status, config) and the server's (the tool call). A token that matches no link, or a route the link may not call, is refused.

pub mod approval;
mod call;
pub mod config;
mod constants;
pub mod permissions;
pub mod status;

use askbar_proto::http::ApiError;
use askbar_proto::links::{ADMIN_MCP_GATEWAY, SERVER_MCP_GATEWAY};
use askbar_proto::paths::MCP_GATEWAY_V1;
use askbar_proto::ErrorCode;
use askbar_service::audit_client::AuditClient;
use askbar_service::configured::Gate;
use askbar_service::links::{self, Allow, Link, LinkDef, Links, Routes};
use askbar_service::status::BootId;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::Method;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use config::Config;
use status::Status;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, RwLock};

const MAX_BODY: usize = 64 * 1024;
pub const DEFAULT_PORT: u16 = 8087;

pub struct Gateway {
    pub boot_id: BootId,
    /// `host:port` of `searxng` (an address, never a secret); unset means search has nowhere to go.
    pub searxng_addr: Option<String>,
    /// The link to the auditor; `None` (a native run) refuses every call, as no call may run unrecorded.
    audit: Option<Arc<AuditClient>>,
    /// How many times a tool has run, for the tests: it must stay 0 when the audit span is not acknowledged.
    pub tool_runs: AtomicU64,
    config: RwLock<Option<Arc<Config>>>,
    calls: Mutex<BTreeMap<&'static str, u64>>,
    one_time_ids: approval::OneTimeStore,
}

impl Gateway {
    pub fn new(boot_id: BootId, searxng_addr: Option<String>, audit: Option<Arc<AuditClient>>) -> Arc<Gateway> {
        Arc::new(Gateway {
            boot_id,
            searxng_addr,
            audit,
            tool_runs: AtomicU64::new(0),
            config: RwLock::new(None),
            calls: Mutex::new(BTreeMap::new()),
            one_time_ids: Default::default(),
        })
    }

    fn applied(&self) -> Option<Arc<Config>> {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn audit(&self) -> Option<&Arc<AuditClient>> {
        self.audit.as_ref()
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

const SERVER_ROUTES: Routes = &[("POST", "/tools/*/call"), ("GET", "/tools")];
const ADMIN_ROUTES: Routes = &[("GET", "/status"), ("PUT", "/config")];

/// The two links: name as `bootstrap` names it (the file `accept/<name>.sha256`), the caller, and the routes.
pub const LINKS: &[LinkDef] = &[(SERVER_MCP_GATEWAY, "server", SERVER_ROUTES), (ADMIN_MCP_GATEWAY, "admin", ADMIN_ROUTES)];

/// The token check for the hashes in `token_dir/accept`: both links on the one listener. A link whose hash file is
/// missing accepts nothing.
pub fn links(token_dir: &Path) -> Links {
    let link = |(name, caller, routes): &LinkDef| Link {
        caller,
        hash_file: token_dir.join("accept").join(format!("{name}.sha256")),
        allow: routes.iter().map(|(m, p)| Allow::method(Method::from_bytes(m.as_bytes()).expect("a method"), &full(p))).collect(),
    };
    Links::new(LINKS.iter().map(link).collect()).named("mcp-gateway")
}

/// The credential volume, read-only: `ASKBAR_MCP_GATEWAY_TOKEN_DIR`, default `/mcp-gateway-token`.
pub fn token_dir() -> PathBuf {
    links::token_dir("ASKBAR_MCP_GATEWAY_TOKEN_DIR", "/mcp-gateway-token")
}

/// The routes, for the OpenAPI test.
pub fn routes() -> Vec<links::RouteEntry> {
    links::routes(MCP_GATEWAY_V1, LINKS)
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
        .route(&full("/tools/{tool}/call"), post(call::call))
        .route(&full("/tools"), get(call::tools))
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
