//! The server API listener: a small HTTP API for the admin service, on its own
//! address and its own router. The client API and this one share no routes.
//!
//! Every request carries `Authorization: Bearer <token>`. The server never holds the
//! token, only its SHA-256, which the admin service writes to a file the server mounts
//! read-only. The file is read on every request, so a replaced hash takes effect at once,
//! and if it is missing or unreadable every request is refused (fail closed).

use crate::error::{ApiError, ApiJson};
use crate::AppState;
use askbar_proto::problem::ErrorCode;
use askbar_proto::Snapshot;
use axum::{
    extract::{Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    Json, Router,
};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use subtle::ConstantTimeEq;

/// Where the SHA-256 (hex) of the server token is.
pub fn token_hash_path() -> PathBuf {
    std::env::var("ASKBAR_SERVER_TOKEN_HASH_FILE")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/server-token/token.sha256"))
}

/// The server listener's address from `ASKBAR_SERVER_API_ADDR`: a concrete address,
/// never a wildcard (`0.0.0.0` or `::`), so it can't end up on every network.
pub fn parse_addr(raw: &str) -> Result<SocketAddr, String> {
    let addr: SocketAddr = raw
        .trim()
        .parse()
        .map_err(|_| format!("ASKBAR_SERVER_API_ADDR is not a valid socket address: {raw} (example: 172.30.0.2:8082)"))?;
    check_not_wildcard(&addr)?;
    Ok(addr)
}

/// The server address must be one specific address, not `0.0.0.0` or `::`.
pub fn check_not_wildcard(addr: &SocketAddr) -> Result<(), String> {
    if addr.ip().is_unspecified() {
        return Err(format!("ASKBAR_SERVER_API_ADDR must be one specific address, not {}: the server API must not listen on every network", addr.ip()));
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

async fn require_token(State(guard): State<Guard>, req: Request, next: Next) -> Result<Response, ApiError> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, t)| scheme.eq_ignore_ascii_case("bearer") && !t.trim().is_empty())
        .map(|(_, t)| t.trim().to_string())
        .ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "Send the server token as `Authorization: Bearer <token>`."))?;
    let file = guard.hash_file.clone();
    let ok = tokio::task::spawn_blocking(move || token_matches(&file, &token)).await.unwrap_or(false);
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(ApiError::new(ErrorCode::Unauthorized, "The server does not accept this token."))
    }
}

/// Replace the configuration snapshot. The version in the body is ignored; the server
/// assigns the next one and returns it.
async fn put_snapshot(State(state): State<Arc<AppState>>, ApiJson(s): ApiJson<Snapshot>) -> Result<Json<serde_json::Value>, ApiError> {
    let applied = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::snapshot::apply(&db, &s)
    };
    // A snapshot with the kill switch on acts at once, on what is already queued or running.
    if applied.is_ok() && s.kill_switch {
        crate::generation::apply_kill_switch(&state, s.kill_switch_aborts_running);
    }
    match applied {
        Ok(version) => Ok(Json(serde_json::json!({ "version": version }))),
        Err(crate::snapshot::ApplyError::Invalid(why)) => Err(ApiError::new(ErrorCode::BadRequest, why)),
        Err(crate::snapshot::ApplyError::Store(_)) => Err(ApiError::new(ErrorCode::Internal, "Could not store the snapshot.")),
    }
}

type Failure = ApiError;

/// An error with the sentence `msg`; the status picks the code.
fn fail(status: StatusCode, msg: &str) -> Failure {
    ApiError::from(status).detail(msg)
}

#[derive(serde::Deserialize)]
struct KeyInput {
    key: String,
}

const MAX_KEY_CHARS: usize = 4096;

/// Store a provider's API key, encrypted with the master key. Write-only: no endpoint
/// ever returns it, and this one answers `204` with no body.
async fn put_provider_key(State(state): State<Arc<AppState>>, Path(id): Path<String>, ApiJson(input): ApiJson<KeyInput>) -> Result<StatusCode, Failure> {
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
    // A new key deserves a fresh start: whatever flagged the old one is cleared.
    let _ = db.clear_provider_health(&id);
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/server/v1/providers/health`: providers the router has found unusable, so the admin can say
/// so. Only providers with a problem are listed; the reason is a code, never the provider's text.
async fn providers_health(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let rows = db.provider_health().map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read provider health."))?;
    Ok(Json(serde_json::json!({
        "providers": rows.into_iter().map(|(id, status, since, code)| serde_json::json!({ "provider_id": id, "status": status, "since": since, "error_code": code })).collect::<Vec<_>>()
    })))
}

/// Send one short message through the provider and report what happened.
async fn test_provider(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<crate::provider_test::TestResult>, Failure> {
    crate::provider_test::run(&state, &id)
        .await
        .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not run the test."))?
        .map(Json)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "No such provider."))
}

/// Ask a provider which models it offers. A failure at the provider is a 200 with `ok: false`, like the test.
async fn list_models(State(state): State<Arc<AppState>>, ApiJson(req): ApiJson<crate::models::ModelsRequest>) -> Result<Json<crate::models::ModelsResult>, Failure> {
    crate::models::run(&state, req)
        .await
        .map(Json)
        .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not list the models."))
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

/// Every paired client, newest first.
async fn list_clients(State(state): State<Arc<AppState>>) -> Result<Json<Vec<crate::auth::ClientRow>>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    crate::auth::list_clients(&db).map(Json).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the clients."))
}

// ------------------------------------------------------------------------ audit

fn no_audit() -> Failure {
    fail(StatusCode::SERVICE_UNAVAILABLE, "No audit store is recording on this server.")
}

#[derive(serde::Deserialize)]
struct TracesQuery {
    status: Option<String>,
    provider: Option<String>,
    before: Option<String>,
    limit: Option<usize>,
}

/// Prices as the snapshot has them now, by provider id. Read per request, so a corrected price
/// changes every figure at once; nothing about cost is stored.
fn price_lookup(state: &AppState) -> impl Fn(&str) -> Option<crate::cost::Prices> {
    let providers = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::snapshot::current(&db).ok().flatten().map(|s| s.providers).unwrap_or_default()
    };
    move |id: &str| providers.iter().find(|p| p.id == id).map(crate::cost::Prices::of)
}

fn add_cost(row: &mut serde_json::Value, price_of: crate::audit::PriceOf) {
    let n = |k: &str| row[k].as_i64();
    let cost = crate::audit::cost_of(price_of, row["provider_id"].as_str(), n("tokens_in"), n("tokens_out"), n("tokens_cached"));
    row["cost"] = serde_json::json!(cost);
}

/// `GET /api/server/v1/audit/traces`: one summary per generation, newest first. Metadata only.
async fn audit_traces(State(state): State<Arc<AppState>>, axum::extract::Query(q): axum::extract::Query<TracesQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let conn = state.audit.reader().ok_or_else(no_audit)?;
    let filter = crate::audit::TraceFilter { status: q.status, provider: q.provider, before: q.before, limit: q.limit };
    let mut rows = crate::audit::list_traces(&conn, &filter).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the audit store."))?;
    let price_of = price_lookup(&state);
    rows.iter_mut().for_each(|r| add_cost(r, &price_of));
    Ok(Json(serde_json::json!({ "traces": rows })))
}

/// `GET /api/server/v1/audit/traces/{id}`: all spans of one generation.
async fn audit_trace(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    let conn = state.audit.reader().ok_or_else(no_audit)?;
    let mut spans = crate::audit::trace_spans(&conn, &id).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the audit store."))?;
    if spans.is_empty() {
        return Err(fail(StatusCode::NOT_FOUND, "No such trace."));
    }
    let price_of = price_lookup(&state);
    // Each attempt is priced by its own provider; the trace as a whole by the provider that served it.
    spans.iter_mut().for_each(|s| add_cost(s, &price_of));
    let final_provider = spans.iter().rev().find(|s| s["kind"] == "attempt").and_then(|s| s["provider_id"].as_str().map(str::to_string));
    let mut cost = None;
    if let Some(turn) = spans.iter().find(|s| s["kind"] == "turn") {
        let n = |k: &str| turn[k].as_i64();
        cost = crate::audit::cost_of(&price_of, final_provider.as_deref(), n("tokens_in"), n("tokens_out"), n("tokens_cached"));
    }
    Ok(Json(serde_json::json!({ "trace_id": id, "cost": cost, "spans": spans })))
}

/// `GET /api/server/v1/audit/payloads/{span_id}`: the sealed content of one span, as ciphertext.
/// Only the admin, with the passphrase, can turn it into text; this endpoint cannot.
async fn audit_payloads(State(state): State<Arc<AppState>>, Path(span_id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    let conn = state.audit.reader().ok_or_else(no_audit)?;
    match crate::audit::span_payloads(&conn, &span_id).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the audit store."))? {
        Some(list) => Ok(Json(serde_json::Value::Array(list))),
        None => Err(fail(StatusCode::NOT_FOUND, "No such span.")),
    }
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    since: Option<i64>,
}

/// `GET /api/server/v1/audit/stats?since=<unix seconds>`: counts, error rate, latency percentiles
/// and token totals. `since` defaults to the last 24 hours.
async fn audit_stats(State(state): State<Arc<AppState>>, axum::extract::Query(q): axum::extract::Query<StatsQuery>) -> Result<Json<serde_json::Value>, Failure> {
    let conn = state.audit.reader().ok_or_else(no_audit)?;
    let since = q.since.unwrap_or_else(|| crate::auth::now() - 24 * 3600);
    let price_of = price_lookup(&state);
    crate::audit::stats(&conn, since, &price_of).map(Json).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the audit store."))
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
            "clients": {
                "active": count("SELECT COUNT(*) FROM clients WHERE revoked_at IS NULL")?,
                "revoked": count("SELECT COUNT(*) FROM clients WHERE revoked_at IS NOT NULL")?,
            },
            "tokens_in": count("SELECT COALESCE(SUM(tokens_in), 0) FROM token_usage")?,
            "tokens_out": count("SELECT COALESCE(SUM(tokens_out), 0) FROM token_usage")?,
        }))
    };
    read().map(Json).map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read the usage."))
}

/// The database as a download, with provider keys, the Cloudflare token, the admin password
/// hash and pairing-code hashes taken out.
async fn backup(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    crate::backup::response(state).await
}

/// All of the server API's routes, with the list of what was registered (see `routes`).
pub fn routes() -> crate::routes::Routes {
    use crate::routes::Routes;
    // Every route is under `/api/server/v1` (askbar_proto::paths::SERVER_V1).
    let v1 = |rest: &str| askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, rest);
    Routes::new()
        .get(&v1("/health"), || async { StatusCode::OK })
        .put(&v1("/snapshot"), put_snapshot)
        .put(&v1("/providers/{id}/key"), put_provider_key)
        .get(&v1("/providers/health"), providers_health)
        .post(&v1("/providers/{id}/test"), test_provider)
        .post(&v1("/models/list"), list_models)
        .get(&v1("/users"), list_users)
        .get(&v1("/clients"), list_clients)
        .get(&v1("/usage"), usage)
        .get(&v1("/backup"), backup)
        .get(&v1("/audit/traces"), audit_traces)
        .get(&v1("/audit/traces/{id}"), audit_trace)
        .get(&v1("/audit/stats"), audit_stats)
        .get(&v1("/audit/payloads/{span_id}"), audit_payloads)
}

/// The server API's routes behind the token check.
pub fn router(state: Arc<AppState>, hash_file: PathBuf) -> Router {
    let (router, _) = routes().into_parts();
    router
        .fallback(crate::error::not_found)
        .method_not_allowed_fallback(crate::error::method_not_allowed)
        .layer(middleware::from_fn_with_state(Guard { hash_file }, require_token))
        .with_state(state)
}
