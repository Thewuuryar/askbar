//! The management listener: a small HTTP API for the admin service, on its own
//! address and its own router. The device API and this one share no routes.
//!
//! Every request carries `Authorization: Bearer <token>`. The server never holds the
//! token, only its SHA-256, which the admin service writes to a file the server mounts
//! read-only. The file is read on every request, so a replaced hash takes effect at once,
//! and if it is missing or unreadable every request is refused (fail closed).

use crate::AppState;
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::get,
    Router,
};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Where the SHA-256 (hex) of the management token is.
pub fn token_hash_path() -> PathBuf {
    std::env::var("ASKBAR_MGMT_TOKEN_HASH_FILE")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/mgmt/token.sha256"))
}

/// The management listener's address from `ASKBAR_MGMT_ADDR`: a concrete address,
/// never a wildcard (`0.0.0.0` or `::`), so it can't end up on every network.
pub fn parse_addr(raw: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = raw
        .trim()
        .parse()
        .map_err(|_| format!("ASKBAR_MGMT_ADDR is not a valid socket address: {raw} (example: 172.30.0.2:8082)"))?;
    if addr.ip().is_unspecified() {
        return Err(format!("ASKBAR_MGMT_ADDR must be one specific address, not {}: the management API must not listen on every network", addr.ip()));
    }
    Ok(addr)
}

#[derive(Clone)]
struct Guard {
    hash_file: PathBuf,
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// True only if the file holds a well-formed hash and the token hashes to it.
fn token_matches(hash_file: &std::path::Path, token: &str) -> bool {
    let Ok(stored) = std::fs::read_to_string(hash_file) else { return false };
    let stored = stored.trim().to_ascii_lowercase();
    if stored.len() != 64 || !stored.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    sha256_hex(token).as_bytes().ct_eq(stored.as_bytes()).into()
}

async fn require_token(State(guard): State<Guard>, req: Request, next: Next) -> Result<Response, StatusCode> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, t)| scheme.eq_ignore_ascii_case("bearer") && !t.trim().is_empty())
        .map(|(_, t)| t.trim().to_string())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let file = guard.hash_file.clone();
    let ok = tokio::task::spawn_blocking(move || token_matches(&file, &token)).await.unwrap_or(false);
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// The management routes behind the token check.
pub fn router(state: Arc<AppState>, hash_file: PathBuf) -> Router {
    Router::new()
        .route("/mgmt/v1/health", get(|| async { StatusCode::OK }))
        .layer(middleware::from_fn_with_state(Guard { hash_file }, require_token))
        .with_state(state)
}
