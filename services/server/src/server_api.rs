//! The server API listener: a small HTTP API for the admin service, on its own
//! address and its own router. The client API and this one share no routes.
//!
//! Every request carries `Authorization: Bearer <token>`, checked per link by
//! `askbar_service::links`: the server never holds the token, only its SHA-256, which the
//! admin service writes to a file the server mounts read-only. The file is read on every
//! request, so a replaced hash takes effect at once, and if it is missing or unreadable every
//! request is refused (fail closed).

use crate::error::{ApiError, ApiJson};
use crate::AppState;
use askbar_proto::problem::ErrorCode;
use askbar_proto::Snapshot;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::Response,
    Json, Router,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

/// Where the SHA-256 (hex) of the admin's token is: the one `bootstrap` made for the `admin-server` link.
pub fn token_hash_path() -> PathBuf {
    std::env::var("ASKBAR_SERVER_TOKEN_HASH_FILE")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/server-token/accept/admin-server.sha256"))
}

/// Start-up check: the hash file `bootstrap` makes must be there. An install from before `bootstrap` has
/// only `token.sha256`, which nothing reads now; the message says what to run (ADR 0049).
pub fn check_token_hash_file(path: &std::path::Path) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    let old = path.parent().and_then(|accept| accept.parent()).map(|volume| volume.join("token.sha256"));
    if old.as_ref().is_some_and(|o| o.exists()) {
        return Err(format!(
            "{} is missing: this install has only the old token.sha256, which is no longer read. Run `docker compose up -d`: bootstrap makes the new file and the admin picks up its token",
            path.display()
        ));
    }
    Err(format!("{} is missing: bootstrap makes it. Run `docker compose up -d`, which runs bootstrap first", path.display()))
}

/// The server listener's address from `ASKBAR_SERVER_API_ADDR`: `host:port` or `ip:port`. A host
/// name is resolved once, at start-up, and must give exactly one address. In Compose it is an
/// alias that exists only on the `server-api` network, so it resolves to the server's own address
/// there and the listener is on that network alone, with no address written down. Never a
/// wildcard (`0.0.0.0` or `::`), so it can't end up on every network.
pub fn parse_addr(raw: &str) -> Result<SocketAddr, String> {
    use std::net::ToSocketAddrs;
    let raw = raw.trim();
    let addrs: Vec<SocketAddr> = match raw.parse::<SocketAddr>() {
        Ok(addr) => vec![addr],
        Err(_) => raw
            .to_socket_addrs()
            .map_err(|e| {
                format!("ASKBAR_SERVER_API_ADDR is not an address this server can resolve: {raw} ({e}; example: server-api-host:8082)")
            })?
            .collect(),
    };
    let addr = one_addr(raw, addrs)?;
    check_not_wildcard(&addr)?;
    Ok(addr)
}

/// The one address a name resolved to. Several would leave it unclear which network the
/// listener is on, so they are refused rather than picked from.
pub fn one_addr(raw: &str, mut addrs: Vec<SocketAddr>) -> Result<SocketAddr, String> {
    addrs.sort();
    addrs.dedup();
    match addrs.as_slice() {
        [addr] => Ok(*addr),
        [] => Err(format!("ASKBAR_SERVER_API_ADDR resolved to no address: {raw}")),
        many => Err(format!(
            "ASKBAR_SERVER_API_ADDR must resolve to one address, but {raw} gives {}: the server API must listen on one network",
            many.iter().map(|a| a.ip().to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// The server address must be one specific address, not `0.0.0.0` or `::`.
pub fn check_not_wildcard(addr: &SocketAddr) -> Result<(), String> {
    if addr.ip().is_unspecified() {
        return Err(format!(
            "ASKBAR_SERVER_API_ADDR must be one specific address, not {}: the server API must not listen on every network",
            addr.ip()
        ));
    }
    Ok(())
}

/// The links that may call the server API (ADR 0004): today only the admin, on every route of
/// this listener (a path that is no route is then a 404). Each later caller gets its own link and
/// only its own routes here.
pub fn links(hash_file: PathBuf) -> askbar_service::links::Links {
    use askbar_service::links::{Allow, Link, Links};
    Links::new(vec![Link { caller: "admin", hash_file, allow: vec![Allow::any("/**")] }]).named("server")
}

/// Replace the configuration snapshot. The version in the body is ignored; the server
/// assigns the next one and returns it.
async fn put_snapshot(State(state): State<Arc<AppState>>, ApiJson(s): ApiJson<Snapshot>) -> Result<Json<serde_json::Value>, ApiError> {
    let applied = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        // A new user's personal space gets its file now, with every migration (ADR 0009 mechanism 4).
        crate::snapshot::apply(&db, &s).and_then(|v| {
            state.spaces.open_all(&db).map(|_| v).map_err(|_| crate::snapshot::ApplyError::Store(rusqlite::Error::InvalidQuery))
        })
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

/// `GET /api/server/v1/providers/health`: providers the gateway has found unusable (it refused their key), so the admin
/// can say so. Only providers with a problem are listed; the reason is a code, never the provider's text. The server
/// holds no keys (ADR 0054), so whether one is missing is the admin's to say.
async fn providers_health(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let rows = db.provider_health().map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Could not read provider health."))?;
    Ok(Json(serde_json::json!({
        "providers": rows.into_iter().map(|(id, status, since, code)| serde_json::json!({ "provider_id": id, "status": status, "since": since, "error_code": code })).collect::<Vec<_>>(),
    })))
}

/// The users, oldest first: `[{"id", "name"}]`. Pairing codes are made for one of these.
async fn list_users(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let read = || -> rusqlite::Result<Vec<serde_json::Value>> {
        let mut stmt = db.conn().prepare("SELECT id, name FROM users ORDER BY created_at, id")?;
        let rows =
            stmt.query_map([], |r| Ok(serde_json::json!({ "id": r.get::<_, String>(0)?, "name": r.get::<_, String>(1)? })))?.collect();
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
    fail(StatusCode::SERVICE_UNAVAILABLE, "The audit store cannot be reached.")
}

/// One read of the audit store, through the auditor (ADR 0051), as JSON. `404` stays `404`; anything else the auditor
/// answers, or no answer, is `503`: the pages say the store cannot be read, never what it said.
async fn audit_read(state: &AppState, path_and_query: &str) -> Result<serde_json::Value, Failure> {
    let reader = state.audit_reader.clone().ok_or_else(no_audit)?;
    match reader.get(path_and_query).await {
        Some((200, body)) => Ok(body),
        Some((404, _)) => Err(fail(StatusCode::NOT_FOUND, "No such record.")),
        _ => Err(no_audit()),
    }
}

/// `path` with the query pairs that have a value.
fn with_query(path: &str, pairs: &[(&str, Option<String>)]) -> String {
    let mut url = reqwest::Url::parse("http://audit.invalid/").expect("a URL");
    for (k, v) in pairs {
        if let Some(v) = v {
            url.query_pairs_mut().append_pair(k, v);
        }
    }
    match url.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
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

fn add_cost(row: &mut serde_json::Value, price_of: crate::cost::PriceOf) {
    let n = |k: &str| row[k].as_i64();
    let cost = crate::cost::cost_of(price_of, row["provider_id"].as_str(), n("tokens_in"), n("tokens_out"), n("tokens_cached"));
    row["cost"] = serde_json::json!(cost);
}

/// `GET /api/server/v1/audit/traces`: one summary per generation, newest first. Metadata only.
async fn audit_traces(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<TracesQuery>,
) -> Result<Json<serde_json::Value>, Failure> {
    let path = with_query(
        "/traces",
        &[("status", q.status), ("provider", q.provider), ("before", q.before), ("limit", q.limit.map(|l| l.to_string()))],
    );
    let mut rows = audit_read(&state, &path).await?;
    let price_of = price_lookup(&state);
    rows.as_array_mut().into_iter().flatten().for_each(|r| add_cost(r, &price_of));
    Ok(Json(serde_json::json!({ "traces": rows })))
}

/// `GET /api/server/v1/audit/traces/{id}`: all spans of one generation.
async fn audit_trace(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    let path = with_query(&format!("/traces/{}", id.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'), "")), &[]);
    let mut spans = audit_read(&state, &path).await?;
    let list = spans.as_array_mut().ok_or_else(no_audit)?;
    if list.is_empty() {
        return Err(fail(StatusCode::NOT_FOUND, "No such trace."));
    }
    let price_of = price_lookup(&state);
    // Each attempt is priced by its own provider; the trace as a whole by the provider that served it.
    list.iter_mut().for_each(|s| add_cost(s, &price_of));
    let final_provider = list.iter().rev().find(|s| s["kind"] == "attempt").and_then(|s| s["provider_id"].as_str().map(str::to_string));
    let mut cost = None;
    if let Some(turn) = list.iter().find(|s| s["kind"] == "turn") {
        let n = |k: &str| turn[k].as_i64();
        cost = crate::cost::cost_of(&price_of, final_provider.as_deref(), n("tokens_in"), n("tokens_out"), n("tokens_cached"));
    }
    Ok(Json(serde_json::json!({ "trace_id": id, "cost": cost, "spans": spans })))
}

/// `GET /api/server/v1/audit/payloads/{span_id}`: the sealed content of one span, as ciphertext.
/// Only the admin, with the passphrase, can turn it into text; this endpoint cannot.
async fn audit_payloads(State(state): State<Arc<AppState>>, Path(span_id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    let id = span_id.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'), "");
    audit_read(&state, &format!("/spans/{id}/payloads")).await.map(Json)
}

#[derive(serde::Deserialize)]
struct StatsQuery {
    since: Option<i64>,
}

/// The cost of one group of requests (one provider, perhaps one day): known, or unknown with how many requests it
/// leaves unknown. A group with no counts costs nothing to price.
fn group_cost(price_of: crate::cost::PriceOf, provider: Option<&str>, tokens: &serde_json::Value, with_tokens: i64) -> (Option<f64>, i64) {
    if with_tokens == 0 {
        return (Some(0.0), 0);
    }
    let n = |k: &str| tokens[k].as_i64();
    match crate::cost::cost_of(price_of, provider, n("in"), n("out"), n("cached")) {
        Some(c) => (Some(c), 0),
        None => (None, with_tokens),
    }
}

/// The auditor's figures with this server's prices applied: it holds none, so every cost it sends is null. Each
/// provider and each day is priced from its tokens, so a corrected price changes every figure at once.
fn price_stats(mut v: serde_json::Value, price_of: crate::cost::PriceOf) -> serde_json::Value {
    let (mut total_known, mut total_unknown) = (0.0, 0);
    if let Some(rows) = v["by_provider"].as_array_mut() {
        for r in rows {
            let (cost, unknown) =
                group_cost(price_of, r["provider_id"].as_str(), &r["tokens"], r["requests_with_tokens"].as_i64().unwrap_or(0));
            r["cost"] = serde_json::json!(cost);
            r["cost_unknown_requests"] = unknown.into();
            total_known += cost.unwrap_or(0.0);
            total_unknown += unknown;
        }
    }
    v["cost"] = serde_json::json!((total_unknown == 0).then_some(total_known));
    v["cost_unknown_requests"] = total_unknown.into();
    let groups = v["by_day_provider"].as_array().cloned().unwrap_or_default();
    if let Some(days) = v["by_day"].as_array_mut() {
        for d in days {
            let (mut known, mut unknown) = (0.0, 0);
            for g in groups.iter().filter(|g| g["day"] == d["day"]) {
                let (cost, u) =
                    group_cost(price_of, g["provider_id"].as_str(), &g["tokens"], g["requests_with_tokens"].as_i64().unwrap_or(0));
                known += cost.unwrap_or(0.0);
                unknown += u;
            }
            d["cost"] = serde_json::json!((unknown == 0).then_some(known));
            d["cost_unknown_requests"] = unknown.into();
        }
    }
    // The pieces the pages never needed: they are the auditor's working figures.
    if let Some(o) = v.as_object_mut() {
        o.remove("by_day_provider");
    }
    if let Some(rows) = v["by_provider"].as_array_mut() {
        rows.iter_mut().for_each(|r| {
            r.as_object_mut().map(|o| o.remove("requests_with_tokens"));
        });
    }
    if let Some(rows) = v["by_day"].as_array_mut() {
        rows.iter_mut().for_each(|r| {
            r.as_object_mut().map(|o| o.remove("requests_with_tokens"));
        });
    }
    v
}

/// `GET /api/server/v1/audit/stats?since=<unix seconds>`: counts, error rate, latency percentiles
/// and token totals. `since` defaults to the last 24 hours.
async fn audit_stats(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<StatsQuery>,
) -> Result<Json<serde_json::Value>, Failure> {
    let path = with_query("/stats", &[("since", q.since.map(|s| s.to_string()))]);
    let v = audit_read(&state, &path).await?;
    let price_of = price_lookup(&state);
    Ok(Json(price_stats(v, &price_of)))
}

/// Counts for the admin service's overview. Users, clients and tokens come from the control
/// database; conversations and messages are summed over every space's file (ADR 0009).
async fn usage(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, Failure> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let count = |sql: &str| db.conn().query_row(sql, [], |r| r.get::<_, i64>(0));
    let in_spaces = |sql: &str| -> rusqlite::Result<i64> {
        let spaces = state.spaces.open_all(&db).map_err(|_| rusqlite::Error::InvalidQuery)?;
        spaces.iter().map(|s| s.lock().conn().query_row(sql, [], |r| r.get::<_, i64>(0))).sum()
    };
    let read = || -> rusqlite::Result<serde_json::Value> {
        Ok(serde_json::json!({
            "users": count("SELECT COUNT(*) FROM users")?,
            "conversations": in_spaces("SELECT COUNT(*) FROM conversations")?,
            "messages": in_spaces("SELECT COUNT(*) FROM messages")?,
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

/// `GET /status`: which configuration this process is running, and which process it is. The admin
/// compares both with what it pushed: no version (`config_version: null`) or another boot ID means
/// it should push again (ADR 0028). Changes nothing.
async fn status(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let version = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        crate::snapshot::current(&db).map_err(|_| ApiError::code(ErrorCode::Internal))?.map(|s| s.version)
    };
    Ok(Json(askbar_service::status::body(version, &state.boot_id)))
}

/// `POST /restart` (ADR 0005): answer, save running replies, exit; Docker starts the server again.
async fn restart(State(state): State<Arc<AppState>>) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let r = state.restarter.get().cloned().ok_or_else(|| ApiError::new(ErrorCode::Internal, "Restart is not set up in this process."))?;
    askbar_service::restart::handler(State(r)).await
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
        .get(&v1(askbar_service::routes::HEALTH), || async { StatusCode::OK })
        .get(&v1(askbar_service::routes::STATUS), status)
        .put(&v1(askbar_service::routes::CONFIG), put_snapshot)
        .post(&v1(askbar_service::routes::RESTART), restart)
        .get(&v1("/providers/health"), providers_health)
        .get(&v1("/users"), list_users)
        .get(&v1("/clients"), list_clients)
        .get(&v1("/usage"), usage)
        // Unencrypted until phase 6 makes exports encrypted (ADR 0013).
        .post(&v1("/exports"), backup)
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
        .layer(middleware::from_fn_with_state(links(hash_file), askbar_service::links::require))
        .with_state(state)
}
