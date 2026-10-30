//! The management listener: a small HTTP API for the admin service, on its own
//! address and its own router. The device API and this one share no routes.
//!
//! Every request carries `Authorization: Bearer <token>`. The server never holds the
//! token, only its SHA-256, which the admin service writes to a file the server mounts
//! read-only. The file is read on every request, so a replaced hash takes effect at once,
//! and if it is missing or unreadable every request is refused (fail closed).

use crate::AppState;
use askbar_proto::Snapshot;
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    routing::{get, post, put},
    Json, Router,
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

/// Replace the configuration snapshot. The version in the body is ignored; the server
/// assigns the next one and returns it.
async fn put_snapshot(State(state): State<Arc<AppState>>, Json(s): Json<Snapshot>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if let Err(why) = crate::snapshot::validate(&s) {
        return Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": why }))));
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match crate::snapshot::save(&db, &s) {
        Ok(version) => Ok(Json(serde_json::json!({ "version": version }))),
        Err(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "could not store the snapshot" })))),
    }
}

type Failure = (StatusCode, Json<serde_json::Value>);

fn fail(status: StatusCode, msg: &str) -> Failure {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(serde::Deserialize)]
struct KeyInput {
    key: String,
}

const MAX_KEY_CHARS: usize = 4096;

/// Store a provider's API key, encrypted with the master key. Write-only: no endpoint
/// ever returns it, and this one answers `204` with no body. A provider that is in the
/// current snapshot but has no row yet gets one.
async fn put_provider_key(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(input): Json<KeyInput>) -> Result<StatusCode, Failure> {
    let key = input.key.trim();
    if key.is_empty() || key.chars().count() > MAX_KEY_CHARS {
        return Err(fail(StatusCode::BAD_REQUEST, "The key must be 1 to 4096 characters."));
    }
    let enc = state.secrets().encrypt(key);
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let internal = |_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the key.");
    let updated = db.conn().execute("UPDATE providers SET api_key_enc = ?1 WHERE id = ?2", rusqlite::params![enc, id]).map_err(internal)?;
    if updated == 0 {
        let snapshot = crate::snapshot::current(&db).map_err(internal)?;
        let p = snapshot
            .and_then(|s| s.providers.into_iter().find(|p| p.id == id))
            .ok_or_else(|| fail(StatusCode::NOT_FOUND, "No such provider."))?;
        db.conn()
            .execute(
                "INSERT INTO providers (id, name, type, base_url, model, api_key_enc, max_tokens, temperature, headers, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    p.id,
                    p.name,
                    p.kind,
                    p.base_url,
                    p.model,
                    enc,
                    p.max_tokens,
                    p.temperature,
                    serde_json::to_string(&p.headers).unwrap_or_else(|_| "{}".into()),
                    crate::auth::now()
                ],
            )
            .map_err(|e| match e {
                rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation => {
                    fail(StatusCode::CONFLICT, "A provider with that name already exists.")
                }
                other => internal(other),
            })?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Send one short message through the provider and report what happened.
async fn test_provider(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<crate::provider_test::TestResult>, Failure> {
    crate::provider_test::run(&state, &id)
        .await
        .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not run the test."))?
        .map(Json)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "No such provider."))
}

/// The management routes behind the token check.
pub fn router(state: Arc<AppState>, hash_file: PathBuf) -> Router {
    Router::new()
        .route("/mgmt/v1/health", get(|| async { StatusCode::OK }))
        .route("/mgmt/v1/snapshot", put(put_snapshot))
        .route("/mgmt/v1/providers/{id}/key", put(put_provider_key))
        .route("/mgmt/v1/providers/{id}/test", post(test_provider))
        .layer(middleware::from_fn_with_state(Guard { hash_file }, require_token))
        .with_state(state)
}
