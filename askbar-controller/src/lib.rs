//! The Askbar controller: the one container that holds the Docker socket. It does three things for the admin:
//! say how each service is doing, and start, stop or restart one. It cannot create, remove or exec
//! anything, and it can only name the services in `ALLOWED`, found by Compose label inside its own project.
//!
//! Every request carries `Authorization: Bearer <token>`. The controller holds only the token's SHA-256, in a
//! file the admin writes and the controller mounts read-only; the file is read on every request and a missing
//! or unreadable one refuses everything (fail closed), exactly as the server does for its server API.

pub mod docker;

use askbar_proto::{http::ApiError, problem::ErrorCode};
use axum::{
    extract::{Path, Request, State},
    http::header,
    middleware::{self, Next},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use docker::{Action, Docker, DockerError};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// The services the admin may control. The admin and the controller are not here, on purpose: stopping the
/// admin would end the request that asked, and the controller cannot restart itself usefully.
pub const ALLOWED: [&str; 3] = ["server", "caddy", "cpa"];

pub struct Config {
    pub docker: Docker,
    /// Only containers of this Compose project are ever touched.
    pub project: String,
    pub token_hash_file: PathBuf,
}

pub fn token_hash_path() -> PathBuf {
    std::env::var("ASKBAR_CONTROL_TOKEN_HASH_FILE").ok().filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/control-token/token.sha256"))
}

pub fn socket_path() -> PathBuf {
    std::env::var("ASKBAR_DOCKER_SOCKET").ok().filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/var/run/docker.sock"))
}

/// One specific address, never `0.0.0.0` or `::`, so the controller cannot end up on every network.
pub fn parse_addr(raw: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = raw.trim().parse().map_err(|_| format!("ASKBAR_CONTROL_ADDR is not a valid socket address: {raw} (example: 172.32.0.2:8083)"))?;
    if addr.ip().is_unspecified() {
        return Err(format!("ASKBAR_CONTROL_ADDR must be one specific address, not {}", addr.ip()));
    }
    Ok(addr)
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn token_matches(hash_file: &std::path::Path, token: &str) -> bool {
    let Ok(stored) = std::fs::read_to_string(hash_file) else { return false };
    let stored = stored.trim().to_ascii_lowercase();
    if stored.len() != 64 || !stored.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    sha256_hex(token).as_bytes().ct_eq(stored.as_bytes()).into()
}

async fn require_token(State(cfg): State<Arc<Config>>, req: Request, next: Next) -> Result<Response, ApiError> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, t)| scheme.eq_ignore_ascii_case("bearer") && !t.trim().is_empty())
        .map(|(_, t)| t.trim().to_string())
        .ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "Send the controller token as `Authorization: Bearer <token>`."))?;
    let file = cfg.token_hash_file.clone();
    let ok = tokio::task::spawn_blocking(move || token_matches(&file, &token)).await.unwrap_or(false);
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(ApiError::new(ErrorCode::Unauthorized, "The controller does not accept this token."))
    }
}

fn docker_failure(e: DockerError) -> ApiError {
    tracing::warn!(error = %e, "docker call failed");
    ApiError::new(ErrorCode::ServerUnavailable, e.to_string())
}

fn allowed(name: &str) -> Result<(), ApiError> {
    if ALLOWED.contains(&name) {
        Ok(())
    } else {
        Err(ApiError::new(ErrorCode::NotFound, "No such service."))
    }
}

fn describe(name: &str, c: Option<&docker::Container>) -> serde_json::Value {
    match c {
        Some(c) => serde_json::json!({ "name": name, "state": c.state, "status": c.status, "health": c.health() }),
        None => serde_json::json!({ "name": name, "state": "missing", "status": "", "health": null }),
    }
}

/// `GET /api/control/v1/services`: each allowed service and how it is doing.
async fn list(State(cfg): State<Arc<Config>>) -> Result<Json<serde_json::Value>, ApiError> {
    let all = cfg.docker.project_containers(&cfg.project).await.map_err(docker_failure)?;
    let rows: Vec<_> = ALLOWED.iter().map(|n| describe(n, all.iter().find(|c| c.service == *n))).collect();
    Ok(Json(serde_json::json!({ "services": rows })))
}

/// `POST /api/control/v1/services/{name}/{action}`: start, stop or restart. Answers with the service as it is afterwards.
async fn act(State(cfg): State<Arc<Config>>, Path((name, action)): Path<(String, String)>) -> Result<Json<serde_json::Value>, ApiError> {
    allowed(&name)?;
    let action = Action::parse(&action).ok_or_else(|| ApiError::new(ErrorCode::NotFound, "No such action. Use start, stop or restart."))?;
    let all = cfg.docker.project_containers(&cfg.project).await.map_err(docker_failure)?;
    let mut mine = all.iter().filter(|c| c.service == name);
    let (Some(c), None) = (mine.next(), mine.next()) else {
        return Err(ApiError::new(ErrorCode::NotFound, format!("The {name} service is not there, or there is more than one container for it.")));
    };
    tracing::info!(service = %name, ?action, "acting");
    cfg.docker.act(&c.id, action).await.map_err(docker_failure)?;
    let after = cfg.docker.project_containers(&cfg.project).await.map_err(docker_failure)?;
    Ok(Json(describe(&name, after.iter().find(|c| c.service == name))))
}

/// The routes, as `(method, path)`, for the OpenAPI check.
pub fn route_table() -> Vec<(&'static str, String)> {
    let v1 = askbar_proto::paths::CONTROL_V1;
    vec![("GET", format!("{v1}/health")), ("GET", format!("{v1}/services")), ("POST", format!("{v1}/services/{{name}}/{{action}}"))]
}

pub fn router(cfg: Arc<Config>) -> Router {
    let v1 = askbar_proto::paths::CONTROL_V1;
    // Health needs no token so that a Docker health check can use it; it says nothing but "up".
    let guarded = Router::new()
        .route(&format!("{v1}/services"), get(list))
        .route(&format!("{v1}/services/{{name}}/{{action}}"), post(act))
        .route_layer(middleware::from_fn_with_state(cfg.clone(), require_token));
    Router::new()
        .route(&format!("{v1}/health"), get(|| async { "ok" }))
        .merge(guarded)
        .fallback(askbar_proto::http::not_found)
        .method_not_allowed_fallback(askbar_proto::http::method_not_allowed)
        .with_state(cfg)
}
