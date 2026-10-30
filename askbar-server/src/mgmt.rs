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
    check_not_wildcard(&addr)?;
    Ok(addr)
}

/// The management address must be one specific address, not `0.0.0.0` or `::`.
pub fn check_not_wildcard(addr: &SocketAddr) -> Result<(), String> {
    if addr.ip().is_unspecified() {
        return Err(format!("ASKBAR_MGMT_ADDR must be one specific address, not {}: the management API must not listen on every network", addr.ip()));
    }
    Ok(())
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
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match crate::snapshot::apply(&db, &s) {
        Ok(version) => Ok(Json(serde_json::json!({ "version": version }))),
        Err(crate::snapshot::ApplyError::Invalid(why)) => Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": why })))),
        Err(crate::snapshot::ApplyError::Store(_)) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": "could not store the snapshot" })))),
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
/// ever returns it, and this one answers `204` with no body.
async fn put_provider_key(State(state): State<Arc<AppState>>, Path(id): Path<String>, Json(input): Json<KeyInput>) -> Result<StatusCode, Failure> {
    let key = input.key.trim();
    if key.is_empty() || key.chars().count() > MAX_KEY_CHARS {
        return Err(fail(StatusCode::BAD_REQUEST, "The key must be 1 to 4096 characters."));
    }
    let enc = state.secrets().encrypt(key);
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let internal = |_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not store the key.");
    // Only a provider the snapshot lists can have a key.
    let listed = crate::snapshot::current(&db).map_err(internal)?.is_some_and(|s| s.providers.iter().any(|p| p.id == id));
    if !listed {
        return Err(fail(StatusCode::NOT_FOUND, "No such provider."));
    }
    db.conn()
        .execute(
            "INSERT INTO providers (id, api_key_enc) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET api_key_enc = excluded.api_key_enc",
            rusqlite::params![id, enc],
        )
        .map_err(internal)?;
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

/// The users, oldest first: `[{"id", "name"}]`. Pairing codes are made for one of these.
async fn list_users(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let read = || -> rusqlite::Result<Vec<serde_json::Value>> {
        let mut stmt = db.conn().prepare("SELECT id, name FROM users ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |r| Ok(serde_json::json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)? })))?.collect();
        rows
    };
    read().map(|v| Json(serde_json::Value::Array(v))).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the users."))
}

/// Every paired device, newest first.
async fn list_devices(State(state): State<Arc<AppState>>) -> Result<Json<Vec<crate::auth::DeviceRow>>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    crate::auth::list_devices(&db).map(Json).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the devices."))
}

/// Counts for the admin service's overview. Token totals come from `token_usage`, which
/// nothing fills in yet, so they are 0 for now.
async fn usage(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let count = |sql: &str| db.conn().query_row(sql, [], |r| r.get::<_, i64>(0));
    let read = || -> rusqlite::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "users": count("SELECT COUNT(*) FROM users")?,
            "conversations": count("SELECT COUNT(*) FROM conversations")?,
            "messages": count("SELECT COUNT(*) FROM messages")?,
            "devices": {
                "active": count("SELECT COUNT(*) FROM devices WHERE revoked_at IS NULL")?,
                "revoked": count("SELECT COUNT(*) FROM devices WHERE revoked_at IS NOT NULL")?,
            },
            "tokens_in": count("SELECT COALESCE(SUM(tokens_in), 0) FROM token_usage")?,
            "tokens_out": count("SELECT COALESCE(SUM(tokens_out), 0) FROM token_usage")?,
        }))
    };
    read().map(Json).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the usage."))
}

/// The database as a download, with provider keys, the Cloudflare token, the admin password
/// hash and pairing-code hashes taken out.
async fn backup(State(state): State<Arc<AppState>>) -> Result<Response, StatusCode> {
    crate::backup::response(state).await
}

/// The management routes behind the token check.
pub fn router(state: Arc<AppState>, hash_file: PathBuf) -> Router {
    Router::new()
        .route("/mgmt/v1/health", get(|| async { StatusCode::OK }))
        .route("/mgmt/v1/snapshot", put(put_snapshot))
        .route("/mgmt/v1/providers/{id}/key", put(put_provider_key))
        .route("/mgmt/v1/providers/{id}/test", post(test_provider))
        .route("/mgmt/v1/users", get(list_users))
        .route("/mgmt/v1/devices", get(list_devices))
        .route("/mgmt/v1/usage", get(usage))
        .route("/mgmt/v1/backup", get(backup))
        .layer(middleware::from_fn_with_state(Guard { hash_file }, require_token))
        .with_state(state)
}
