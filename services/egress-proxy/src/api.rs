//! `/api/egress-proxy/v1`, for the admin only: `GET /status`, `PUT /config`, `POST /restart`, and `GET /health` with no token.
//! The proxy answers `403` to everything until a config has been pushed, so there is no "not configured" gate here.

use crate::Proxy;
use askbar_proto::egress::{EgressConfig, EgressStatus, MAX_CONFIG_BYTES};
use askbar_proto::http::ApiError;
use askbar_proto::links::ADMIN_EGRESS_PROXY;
use askbar_proto::paths::EGRESS_PROXY_V1;
use askbar_proto::ErrorCode;
pub use askbar_service::links::RouteEntry;
use askbar_service::links::{self, LinkDef, Links, Routes};
use axum::body::Bytes;
use axum::extract::State;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ADMIN_ROUTES: Routes = &[("GET", "/status"), ("PUT", "/config"), ("POST", "/restart")];

/// The one link: name as `bootstrap` names it (the file `accept/<name>.sha256`), the caller, and the routes.
pub const LINKS: &[LinkDef] = &[(ADMIN_EGRESS_PROXY, "admin", ADMIN_ROUTES)];

fn full(rest: &str) -> String {
    links::full(EGRESS_PROXY_V1, rest)
}

/// The check on the admin's listener: only the admin's token.
pub fn admin_links(token_dir: &Path) -> Links {
    links::links_for("egress-proxy", EGRESS_PROXY_V1, token_dir, &LINKS[0])
}

/// The credential volume, read-only: `ASKBAR_EGRESS_PROXY_TOKEN_DIR`, default `/egress-proxy-token`.
pub fn token_dir() -> PathBuf {
    links::token_dir("ASKBAR_EGRESS_PROXY_TOKEN_DIR", "/egress-proxy-token")
}

/// Every route, method and full path. The OpenAPI file describes exactly these.
pub fn routes() -> Vec<RouteEntry> {
    links::routes(EGRESS_PROXY_V1, LINKS)
}

pub fn admin_router(p: Arc<Proxy>, links: Links) -> Router {
    let restart = p.restarter.clone();
    let protected = Router::new()
        .route(&full("/status"), get(status))
        .route(&full("/config"), put(put_config))
        .route(&full("/restart"), post(askbar_service::restart::handler).with_state(restart))
        .layer(axum::middleware::from_fn_with_state(links, askbar_service::links::require));
    links::shell(EGRESS_PROXY_V1, MAX_CONFIG_BYTES, p, protected)
}

async fn status(State(p): State<Arc<Proxy>>) -> Json<EgressStatus> {
    Json(p.status())
}

/// Replace the whole config. A refusal changes nothing and names no value.
async fn put_config(State(p): State<Arc<Proxy>>, body: Bytes) -> Result<Json<serde_json::Value>, ApiError> {
    let c: EgressConfig = serde_json::from_slice(&body)
        .map_err(|_| ApiError::new(ErrorCode::BadRequest, "The body is not a config: an unknown field or a value out of range."))?;
    p.check(&c).map_err(|why| ApiError::new(ErrorCode::BadRequest, format!("The config is refused: {why}.")))?;
    Ok(Json(serde_json::json!({ "version": p.apply(c) })))
}
