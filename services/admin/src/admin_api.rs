//! The admin API: thin handlers over `admin.db` and the server API.
//!
//! The rule for anything that changes configuration: write `admin.db`, send the server the new
//! snapshot, and only keep the change if the server accepted it. If the server is not there the
//! answer is `502` and nothing was changed, so the two never disagree. API keys are the one
//! thing `admin.db` never holds: they go straight to the server.

use crate::server_link::{build_snapshot, PushError, ServerLink};
use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::http::ApiJson;
use askbar_proto::DisplaySettings;
use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) type St = State<Arc<AdminState>>;

const MAX_PROMPT_CHARS: usize = 20_000;
const PAIR_CODE_TTL_SECS: i64 = 10 * 60;

fn internal<E>(_: E) -> Failure {
    err(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong on the admin side.")
}

fn bad(msg: &str) -> Failure {
    err(StatusCode::BAD_REQUEST, msg)
}

fn offline() -> Failure {
    err(StatusCode::BAD_GATEWAY, "The server is offline. Nothing was changed.")
}

/// The sentence in a problem the server answered with, if the body is one.
fn server_detail(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<askbar_proto::Problem>(body).ok().map(|p| p.detail).filter(|d| !d.is_empty())
}

fn from_push(e: PushError) -> Failure {
    match e {
        PushError::Unreachable(_) => offline(),
        PushError::Refused { status: 400, body } => {
            bad(&server_detail(body.as_bytes()).unwrap_or_else(|| "The server did not accept that.".into()))
        }
        PushError::Refused { status, .. } => err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}. Nothing was changed.")),
        PushError::Build(_) => internal(()),
    }
}

fn gateway(state: &AdminState) -> Result<&Arc<crate::gateway_link::GatewayLink>, Failure> {
    state.gateway.get().ok_or_else(|| err(StatusCode::BAD_GATEWAY, "No link to the gateway is set up."))
}

fn link(state: &AdminState) -> Result<&Arc<ServerLink>, Failure> {
    state.server.as_ref().ok_or_else(offline)
}

/// Send the server the snapshot. The push is an action of its own (`config_push`): recorded before it is made, and not
/// made when it cannot be recorded (ADR 0010). The server records the version it applied itself (ADR 0028 rule 5).
async fn push_snapshot(state: &AdminState, link: &ServerLink, snapshot: &askbar_proto::Snapshot) -> Result<u64, Failure> {
    let Some(log) = state.actions.get() else { return link.push(snapshot).await.map_err(from_push) };
    let done = log
        .around("config_push", "server", serde_json::json!({ "target": "server" }), link.push(snapshot), |r| match r {
            Ok(v) => serde_json::json!({ "version": v }),
            Err(_) => serde_json::json!({}),
        })
        .await;
    match done {
        Err(why) => Err(err(StatusCode::SERVICE_UNAVAILABLE, why.sentence())),
        Ok(pushed) => pushed.map_err(from_push),
    }
}

/// Apply `apply` to `admin.db` and keep each of `keys` (provider id, API key), encrypted, send the server the resulting
/// snapshot, and commit only if that worked. The keys go to the gateway, not the server (ADR 0054): the gateway is
/// given them after the commit.
pub(crate) async fn mutate<T>(
    state: &AdminState,
    keys: Vec<(String, String)>,
    apply: impl FnOnce(&Connection) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let link = link(state)?;
    let db = state.db.lock().await;
    let conn = db.conn();
    conn.execute_batch("BEGIN IMMEDIATE").map_err(internal)?;
    let rollback = |conn: &Connection| {
        let _ = conn.execute_batch("ROLLBACK");
    };
    let out = match apply(conn) {
        Ok(v) => v,
        Err(e) => {
            rollback(conn);
            return Err(e);
        }
    };
    // The keys are kept here too, encrypted, so `model-gateway` can be given them (ADR 0054). In the same
    // transaction: a change the server refuses takes its keys back out with the rest.
    if let Err(e) = crate::gateway_link::keep_keys(conn, &state.secrets, &keys) {
        rollback(conn);
        return Err(e);
    }
    let snapshot = match build_snapshot(conn) {
        Ok(s) => s,
        Err(e) => {
            rollback(conn);
            return Err(internal(e));
        }
    };
    if let Err(e) = push_snapshot(state, link, &snapshot).await {
        rollback(db.conn());
        return Err(e);
    }
    db.conn().execute_batch("COMMIT").map_err(internal)?;
    // The gateway gets the new truth now, not at its next check.
    if let Some(g) = state.gateway.get() {
        g.mark_dirty();
    }
    Ok(out)
}

// ------------------------------------------------------------------- status

/// For the "server offline", "token rejected" and audit chain banners.
pub async fn status(State(state): St, headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
    let (online, pending, error, rejected) = match &state.server {
        Some(l) => {
            l.check().await;
            (l.server_online(), l.pending(), l.last_error(), l.token_rejected())
        }
        None => (false, false, None, false),
    };
    Json(serde_json::json!({
        "server_online": online,
        "pending": pending,
        "error": error,
        "token_rejected": rejected,
        "audit_chain": state.chain.status(),
        // Whether alerts are being delivered: `ok`, `not_set_up`, `failing` (for half an hour) or `unreachable` (ADR 0055).
        "alerts": crate::alerts::view(&state).await.banner,
        "sealed_content": banner_status(&state, &headers),
    }))
}

// ---------------------------------------------------------------- providers

#[derive(Serialize)]
pub struct ProviderRow {
    id: String,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    base_url: String,
    model: String,
    max_tokens: Option<u32>,
    temperature: Option<f32>,
    /// Whether the server holds a key. The key itself is never shown.
    has_key: bool,
    /// "metered" or "subscription".
    billing: String,
    /// Price per million tokens; absent means unknown.
    price_in: Option<f64>,
    price_out: Option<f64>,
    price_cached_in: Option<f64>,
    /// Made and kept in step by the admin itself (the bundled CLIProxyAPI): name, address and key are fixed.
    managed: bool,
    /// The admin sent a key, but the server reports it holds none (its database was replaced, say). The
    /// person enters it again. Never set for the managed provider: the admin resends that key itself.
    key_missing: bool,
}

#[derive(Deserialize)]
pub struct ProviderInput {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    base_url: String,
    model: String,
    /// Write-only. Missing or empty on update means "keep the current key".
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    temperature: Option<f32>,
    /// "metered" (the default) or "subscription".
    #[serde(default)]
    billing: Option<String>,
    #[serde(default)]
    price_in: Option<f64>,
    #[serde(default)]
    price_out: Option<f64>,
    #[serde(default)]
    price_cached_in: Option<f64>,
}

fn validate(i: &ProviderInput) -> Result<(), Failure> {
    if i.name.trim().is_empty() {
        return Err(bad("Name is required."));
    }
    if !matches!(i.kind.as_str(), "openai" | "anthropic") {
        return Err(bad("Type must be openai or anthropic."));
    }
    if i.model.trim().is_empty() {
        return Err(bad("Model is required."));
    }
    let url = i.base_url.trim();
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(bad("Base URL must start with http:// or https://."));
    }
    if !matches!(i.billing.as_deref().unwrap_or("metered"), "metered" | "subscription") {
        return Err(bad("Billing must be metered or subscription."));
    }
    // The same rule the server applies to the snapshot.
    for (label, price) in [("Input price", i.price_in), ("Output price", i.price_out), ("Cached input price", i.price_cached_in)] {
        if price.is_some_and(|v| !v.is_finite() || v < 0.0) {
            return Err(bad(&format!("{label} must be zero or more.")));
        }
    }
    Ok(())
}

fn name_taken(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation)
}

fn clash_or_internal(e: rusqlite::Error) -> Failure {
    if name_taken(&e) {
        err(StatusCode::CONFLICT, "A provider with that name already exists.")
    } else {
        internal(e)
    }
}

pub async fn list_providers(State(state): St) -> Result<Json<Vec<ProviderRow>>, Failure> {
    let db = state.db.lock().await;
    let mut stmt = db
        .conn()
        .prepare("SELECT id, name, type, base_url, model, max_tokens, temperature, EXISTS (SELECT 1 FROM provider_keys k WHERE k.provider_id = providers.id), billing, price_in, price_out, price_cached_in, managed, has_key FROM providers ORDER BY name")
        .map_err(internal)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(ProviderRow {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                base_url: r.get(3)?,
                model: r.get(4)?,
                max_tokens: r.get(5)?,
                temperature: r.get(6)?,
                has_key: r.get::<_, i64>(7)? != 0,
                billing: r.get(8)?,
                price_in: r.get(9)?,
                price_out: r.get(10)?,
                price_cached_in: r.get(11)?,
                managed: r.get::<_, i64>(12)? != 0,
                // A key the server held before the gateway existed (the old column says so) that the admin cannot read:
                // it is entered again (ADR 0054).
                key_missing: r.get::<_, i64>(7)? == 0 && r.get::<_, i64>(13)? != 0,
            })
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    // The managed provider's key is the admin's own file: never missing.
    let rows =
        rows.into_iter().map(|p| ProviderRow { has_key: p.has_key || p.managed, key_missing: p.key_missing && !p.managed, ..p }).collect();
    Ok(Json(rows))
}

pub async fn create_provider(
    State(state): St,
    ApiJson(i): ApiJson<ProviderInput>,
) -> Result<(StatusCode, Json<serde_json::Value>), Failure> {
    validate(&i)?;
    let id = ulid::Ulid::new().to_string();
    let keys: Vec<(String, String)> =
        i.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(|k| vec![(id.clone(), k.to_string())]).unwrap_or_default();
    let new_id = id.clone();
    mutate(&state, keys, move |c| {
        c.execute(
            "INSERT INTO providers (id, name, type, base_url, model, max_tokens, temperature, created_at, billing, price_in, price_out, price_cached_in)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                new_id, i.name.trim(), i.kind, i.base_url.trim(), i.model.trim(), i.max_tokens, i.temperature, crate::auth::now(),
                i.billing.as_deref().unwrap_or("metered"), i.price_in, i.price_out, i.price_cached_in
            ],
        )
        .map(|_| ())
        .map_err(clash_or_internal)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))))
}

pub async fn update_provider(
    State(state): St,
    Path(id): Path<String>,
    ApiJson(mut i): ApiJson<ProviderInput>,
) -> Result<StatusCode, Failure> {
    validate(&i)?;
    // A managed provider keeps the name, type, address and key the admin gave it.
    let managed_row: Option<(String, String, String)> = {
        let db = state.db.lock().await;
        db.conn()
            .query_row("SELECT name, type, base_url FROM providers WHERE id = ?1 AND managed = 1", [&id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .ok()
    };
    if let Some((name, kind, base_url)) = &managed_row {
        (i.name, i.kind, i.base_url, i.api_key) = (name.clone(), kind.clone(), base_url.clone(), None);
    }
    let keys: Vec<(String, String)> =
        i.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(|k| vec![(id.clone(), k.to_string())]).unwrap_or_default();
    mutate(&state, keys, move |c| {
        let n = c
            .execute(
                "UPDATE providers SET name = ?2, type = ?3, base_url = ?4, model = ?5, max_tokens = ?6, temperature = ?7,
                        billing = ?8, price_in = ?9, price_out = ?10, price_cached_in = ?11 WHERE id = ?1",
                params![
                    id,
                    i.name.trim(),
                    i.kind,
                    i.base_url.trim(),
                    i.model.trim(),
                    i.max_tokens,
                    i.temperature,
                    i.billing.as_deref().unwrap_or("metered"),
                    i.price_in,
                    i.price_out,
                    i.price_cached_in
                ],
            )
            .map_err(clash_or_internal)?;
        if n == 0 {
            return Err(err(StatusCode::NOT_FOUND, "No such provider."));
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_provider(State(state): St, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    mutate(&state, vec![], move |c| {
        if c.query_row("SELECT 1 FROM providers WHERE id = ?1 AND managed = 1", [&id], |_| Ok(())).is_ok() {
            return Err(err(
                StatusCode::CONFLICT,
                "This provider is kept by the admin and cannot be deleted. Disconnect the accounts on the Services tab instead.",
            ));
        }
        // Its approved models go with it, and not while it holds the default of a catalog that has others.
        crate::catalog::drop_provider(c, &id)?;
        let n = c.execute("DELETE FROM providers WHERE id = ?1", [&id]).map_err(internal)?;
        if n == 0 {
            return Err(err(StatusCode::NOT_FOUND, "No such provider."));
        }
        // Don't leave a dangling default behind.
        c.execute("DELETE FROM settings WHERE key = 'default_provider_id' AND value = ?1", [&id]).map_err(internal)?;
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Send one short message through the provider (on the server, where the key is) and report.
pub async fn test_provider(State(state): St, Path(id): Path<String>) -> Result<Response, Failure> {
    let known = {
        let db = state.db.lock().await;
        db.conn().query_row("SELECT 1 FROM providers WHERE id = ?1", [&id], |_| Ok(())).is_ok()
    };
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such provider."));
    }
    // The gateway holds the keys and has the route out (ADR 0054).
    let (status, body) = gateway(&state)?.call(&state, reqwest::Method::POST, &format!("/providers/{id}/test"), None, true).await?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The gateway answered {status}.")));
    }
    json_response(body)
}

#[derive(Deserialize, serde::Serialize)]
pub struct ModelsInput {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    base_url: String,
    /// Write-only, used once by the gateway for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    /// A saved provider whose key the gateway may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_id: Option<String>,
}

/// Ask the gateway which models a provider offers (it has the keys and the route out). Nothing is saved.
pub async fn list_models(State(state): St, ApiJson(i): ApiJson<ModelsInput>) -> Result<Response, Failure> {
    if !matches!(i.kind.as_str(), "openai" | "anthropic") {
        return Err(bad("Type must be openai or anthropic."));
    }
    let body = serde_json::to_value(&i).map_err(internal)?;
    let needs_config = i.provider_id.is_some();
    let (status, bytes) = gateway(&state)?.call(&state, reqwest::Method::POST, "/models", Some(&body), needs_config).await?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The gateway answered {status}.")));
    }
    json_response(bytes)
}

fn json_response(body: Vec<u8>) -> Result<Response, Failure> {
    Response::builder().header(header::CONTENT_TYPE, "application/json").body(axum::body::Body::from(body)).map_err(internal)
}

// ------------------------------------------------------------------ clients

async fn revoked_ids(state: &AdminState) -> Vec<String> {
    let db = state.db.lock().await;
    db.conn()
        .prepare("SELECT client_id FROM revoked_clients")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default()
}

/// The server's clients, with the ones this admin has revoked marked as such.
async fn clients(state: &AdminState) -> Result<Vec<serde_json::Value>, Failure> {
    let (status, body) = link(state)?
        .request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/clients"), None)
        .await
        .map_err(from_push)?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
    }
    let mut list: Vec<serde_json::Value> = serde_json::from_slice(&body).map_err(internal)?;
    let revoked = revoked_ids(state).await;
    for d in &mut list {
        if d["id"].as_str().is_some_and(|id| revoked.iter().any(|r| r == id)) {
            d["revoked"] = serde_json::json!(true);
        }
    }
    Ok(list)
}

pub async fn list_clients(State(state): St) -> Result<Json<Vec<serde_json::Value>>, Failure> {
    clients(&state).await.map(Json)
}

/// Revoke a client by putting its id in the snapshot; the server refuses it from then on.
pub async fn revoke_client(State(state): St, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    if !clients(&state).await?.iter().any(|d| d["id"].as_str() == Some(id.as_str())) {
        return Err(err(StatusCode::NOT_FOUND, "No such client."));
    }
    mutate(&state, vec![], move |c| {
        c.execute("INSERT OR IGNORE INTO revoked_clients (client_id, revoked_at) VALUES (?1, ?2)", params![id, crate::auth::now()])
            .map(|_| ())
            .map_err(internal)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------ the admin API key

#[derive(Deserialize)]
pub struct NewKey {
    #[serde(default)]
    name: String,
}

/// The one response that carries the key itself. Nothing returns it again.
#[derive(Serialize)]
pub struct CreatedKey {
    #[serde(flatten)]
    row: crate::api_key::KeyRow,
    /// Shown once: copy it now.
    key: String,
}

/// `POST /api/admin/v1/keys`: make the admin API key. 409 `api_key_exists` while one is active.
pub async fn create_key(
    State(state): St,
    headers: axum::http::HeaderMap,
    ApiJson(b): ApiJson<NewKey>,
) -> Result<(StatusCode, Json<CreatedKey>), Failure> {
    use crate::api_key::{create, CreateError};
    crate::web::require_fresh_signin(&state, &headers)?;
    let db = state.db.lock().await;
    match create(db.conn(), &b.name, crate::auth::now()) {
        Ok((row, key)) => Ok((StatusCode::CREATED, Json(CreatedKey { row, key }))),
        Err(CreateError::Exists) => Err(askbar_proto::http::ApiError::new(
            askbar_proto::ErrorCode::ApiKeyExists,
            "An API key already exists. Revoke it before making another.",
        )
        .no_challenge()),
        Err(CreateError::BadName) => Err(bad("The name must be 1 to 60 characters.")),
        Err(CreateError::Db(_)) => Err(internal(())),
    }
}

/// `GET /api/admin/v1/keys`: the key that can be used, by name. Never the key or its hash.
pub async fn list_keys(State(state): St) -> Result<Json<Vec<crate::api_key::KeyRow>>, Failure> {
    let db = state.db.lock().await;
    crate::api_key::list(db.conn()).map(Json).map_err(internal)
}

/// `DELETE /api/admin/v1/keys/{id}`: revoke it. It stops working at once.
pub async fn revoke_key(State(state): St, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    let db = state.db.lock().await;
    match crate::api_key::revoke(db.conn(), &id, crate::auth::now()) {
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Ok(false) => Err(err(StatusCode::NOT_FOUND, "There is no such key.")),
        Err(e) => Err(internal(e)),
    }
}

// ------------------------------------------------------------------ pairing

#[derive(Deserialize)]
pub struct PairInput {
    #[serde(default)]
    name: String,
}

#[derive(Serialize)]
pub struct PairingOut {
    pairing: String,
    expires_in: i64,
}

fn random_code() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

/// The address clients should pair with: the host name when HTTPS with a real certificate is on;
/// else `ASKBAR_PUBLIC_URL` if it is set; else the address the admin site was opened with (`host`,
/// the request's `Host` header, which Caddy passes through), over HTTPS with the install's own
/// certificate.
pub(crate) fn public_url(conn: &Connection, host: Option<&str>) -> String {
    if let Some(name) = real_host_name(conn) {
        return format!("https://{name}");
    }
    if let Some(u) = std::env::var("ASKBAR_PUBLIC_URL").ok().filter(|u| !u.trim().is_empty()) {
        return u.trim().trim_end_matches('/').to_string();
    }
    format!("https://{}", host.filter(|h| valid_host(h)).unwrap_or("localhost"))
}

/// The host name, when HTTPS with a real certificate is on. `None` means Caddy serves the install's
/// own certificate.
fn real_host_name(conn: &Connection) -> Option<String> {
    let (tls, name): (i64, String) =
        conn.query_row("SELECT tls_enabled, hostname FROM network_settings WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok()?;
    (tls == 1 && !name.is_empty()).then_some(name)
}

/// A `Host` header worth putting in a pairing key: a name or address, an optional port, nothing else.
fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 255 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// The request's `Host` header, if it is plain text.
pub(crate) fn request_host(headers: &axum::http::HeaderMap) -> Option<String> {
    headers.get(axum::http::header::HOST)?.to_str().ok().map(str::to_string)
}

/// A one-time pairing key for a new client, good for ten minutes. Only the code's hash is stored.
#[derive(Serialize)]
pub struct UserRow {
    id: String,
    name: String,
    created_at: i64,
    /// Replies are stopped for this user (the emergency stop for one person, plan step 3.2).
    paused: bool,
}

fn read_users(c: &Connection) -> rusqlite::Result<Vec<UserRow>> {
    let mut stmt = c.prepare("SELECT id, name, created_at, paused FROM users ORDER BY created_at, name")?;
    let rows = stmt
        .query_map([], |r| Ok(UserRow { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, paused: r.get::<_, i64>(3)? != 0 }))?
        .collect();
    rows
}

/// An install from before 1.9 has users only on the server (one, "owner", with its paired clients). The
/// first time the admin has none, it takes them over, ids and all, so those clients keep working.
async fn adopt_server_users(state: &AdminState) {
    let Some(link) = state.server.clone() else { return };
    let Ok((200, body)) =
        link.request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/users"), None).await
    else {
        return;
    };
    let Ok(serde_json::Value::Array(users)) = serde_json::from_slice::<serde_json::Value>(&body) else { return };
    let db = state.db.lock().await;
    for u in users {
        if let (Some(id), Some(name)) = (u["id"].as_str(), u["name"].as_str()) {
            let _ = db
                .conn()
                .execute("INSERT OR IGNORE INTO users (id, name, created_at) VALUES (?1, ?2, ?3)", params![id, name, crate::auth::now()]);
        }
    }
}

/// `GET /users`, oldest first.
pub async fn list_users(State(state): St) -> Result<Json<Vec<UserRow>>, Failure> {
    let empty = state.db.lock().await.conn().query_row("SELECT COUNT(*) FROM users", [], |r| r.get::<_, i64>(0)).map_err(internal)? == 0;
    if empty {
        adopt_server_users(&state).await;
    }
    let db = state.db.lock().await;
    read_users(db.conn()).map(Json).map_err(internal)
}

#[derive(Deserialize)]
pub struct UserInput {
    name: String,
}

/// `POST /users`: a new user, pushed to the server with everything else (ADR 0008).
pub async fn create_user(State(state): St, ApiJson(i): ApiJson<UserInput>) -> Result<(StatusCode, Json<serde_json::Value>), Failure> {
    let name = i.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 60 {
        return Err(bad("A user's name is 1 to 60 characters."));
    }
    let id = ulid::Ulid::new().to_string();
    let new_id = id.clone();
    mutate(&state, vec![], move |c| {
        c.execute("INSERT INTO users (id, name, created_at) VALUES (?1, ?2, ?3)", params![new_id, name, crate::auth::now()])
            .map(|_| ())
            .map_err(clash_or_internal)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))))
}

/// `DELETE /users/{id}`: the person is gone from everywhere (ADR 0012, D16). The server goes first, because it holds
/// what is worth deleting (the personal space, the clients and their token hashes); then this database's rows for
/// them, in the same step as the snapshot that no longer names them. The audit records stay. A person's own browser
/// session only, signed in within the last 10 minutes: a key cannot do it. The page asks for the name to be typed.
///
/// If the server is deleted from but the rest fails, the user is still listed here: do it again. The server answers
/// 404 for a user it no longer has, which counts as done.
pub async fn delete_user(State(state): St, headers: axum::http::HeaderMap, Path(user): Path<String>) -> Result<StatusCode, Failure> {
    crate::web::require_fresh_signin(&state, &headers)?;
    let link = link(&state)?;
    let known = state
        .db
        .lock()
        .await
        .conn()
        .query_row("SELECT 1 FROM users WHERE id = ?1", [&user], |_| Ok(()))
        .map(|_| true)
        .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(false) } else { Err(e) })
        .map_err(internal)?;
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such user."));
    }
    let path = askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, &format!("/users/{user}"));
    match link.request(reqwest::Method::DELETE, &path, None).await {
        Ok((204 | 404, _)) => {}
        Ok((status, _)) => return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}. Nothing was deleted."))),
        Err(_) => return Err(offline()),
    }
    let id = user.clone();
    mutate(&state, vec![], move |c| {
        c.execute("DELETE FROM display_defaults WHERE user_id = ?1", [&id]).map_err(internal)?;
        c.execute("DELETE FROM pair_codes WHERE user_id = ?1", [&id]).map_err(internal)?;
        c.execute("DELETE FROM users WHERE id = ?1", [&id]).map_err(internal)?;
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /users/{id}/pause` and `/resume`: the emergency stop for one person. A paused user gets no new reply, the
/// gateway stops the ones running, and queued messages are refused when they come up. It is pushed to the server and the
/// gateway like every change, and recorded like every change.
async fn set_paused(state: &AdminState, user: String, paused: bool) -> Result<StatusCode, Failure> {
    mutate(state, vec![], move |c| {
        let n = c.execute("UPDATE users SET paused = ?2 WHERE id = ?1", params![user, paused as i64]).map_err(internal)?;
        if n == 0 {
            return Err(err(StatusCode::NOT_FOUND, "No such user."));
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn pause_user(State(state): St, Path(user): Path<String>) -> Result<StatusCode, Failure> {
    set_paused(&state, user, true).await
}

pub async fn resume_user(State(state): St, Path(user): Path<String>) -> Result<StatusCode, Failure> {
    set_paused(&state, user, false).await
}

/// `POST /users/{id}/pairing-keys`: a pairing key that registers one client for this user.
pub async fn create_pairing_key(
    State(state): St,
    Path(user): Path<String>,
    headers: axum::http::HeaderMap,
    ApiJson(i): ApiJson<PairInput>,
) -> Result<Json<PairingOut>, Failure> {
    let host = request_host(&headers);
    let name = i.name.trim().to_string();
    if name.chars().count() > 60 {
        return Err(bad("The client name is too long."));
    }
    let known = state
        .db
        .lock()
        .await
        .conn()
        .query_row("SELECT 1 FROM users WHERE id = ?1", [&user], |_| Ok(()))
        .map(|_| true)
        .or_else(|e| if e == rusqlite::Error::QueryReturnedNoRows { Ok(false) } else { Err(e) })
        .map_err(internal)?;
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such user."));
    }
    let code = random_code();
    let hash = crate::server_link::sha256_hex(&code);
    let url = mutate(&state, vec![], move |c| {
        let now = crate::auth::now();
        c.execute("DELETE FROM pair_codes WHERE expires_at <= ?1", [now]).map_err(internal)?;
        c.execute(
            "INSERT INTO pair_codes (hash, user_id, client_name, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash, user, name, now + PAIR_CODE_TTL_SECS, now],
        )
        .map_err(internal)?;
        Ok((public_url(c, host.as_deref()), real_host_name(c).is_none()))
    })
    .await?;
    let (url, own_cert) = url;
    // While Caddy serves the install's own certificate, the string names it, and the app trusts exactly
    // that certificate for this server (ADR 0034). Without Caddy (a server on the same machine, over
    // http) there is nothing to pin.
    let secrets_dir = state.caddy.lock().unwrap_or_else(|e| e.into_inner()).secrets_dir.clone();
    let cert_sha256 = match (own_cert && url.starts_with("https://"), secrets_dir) {
        (true, Some(dir)) => Some(
            crate::self_cert::ensure(&dir)
                .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not read this install's certificate. Nothing was paired."))?,
        ),
        _ => None,
    };
    let pairing = askbar_proto::Pairing { url, code, cert_sha256 }.encode();
    Ok(Json(PairingOut { pairing, expires_in: PAIR_CODE_TTL_SECS }))
}

// -------------------------------------------------------------- audit key setup

#[derive(Deserialize)]
pub struct SetupAuditKey {
    passphrase: String,
    confirm: String,
}

/// `POST /api/admin/v1/setup/audit-key`: make the first audit key from a passphrase. The passphrase
/// is turned into a keypair, the public half and the salt are kept, and the passphrase is
/// forgotten: nothing here stores or logs it. A second call is refused (rotation is its own
/// step); losing the passphrase makes the sealed content unreadable for good.
pub async fn setup_audit_key(State(state): St, ApiJson(b): ApiJson<SetupAuditKey>) -> Result<Json<serde_json::Value>, Failure> {
    use crate::audit_key::{derive_for_setup, key_id, new_salt, KdfParams, KeyError};
    if b.passphrase != b.confirm {
        return Err(bad("The two passphrases do not match."));
    }
    if state.db.lock().await.active_audit_key().map_err(internal)?.is_some() {
        return Err(err(StatusCode::CONFLICT, "An audit key already exists."));
    }
    let salt = new_salt();
    let passphrase = b.passphrase;
    let params = KdfParams::CURRENT;
    let kp = tokio::task::spawn_blocking(move || derive_for_setup(&passphrase, &salt, params))
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not derive the key."))?
        .map_err(|e| match e {
            KeyError::TooShort => bad("The passphrase must be at least 20 characters."),
            KeyError::Kdf(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "Could not derive the key."),
        })?;
    let public = kp.public_bytes();
    let id = key_id(&public);
    let row = crate::db::AuditKeyRow {
        key_id: id.clone(),
        public_key: public,
        salt,
        argon_params: params.to_text(),
        created_at: crate::auth::now(),
        retired_at: None,
    };
    // Saved and pushed to the server together: if the server cannot be reached the key is not kept
    // either (the same rule as every other setting). The unique index on active keys also settles
    // two calls at once: one wins, the other is a 409.
    mutate(&state, vec![], move |c| {
        crate::db::insert_audit_key(c, &row).map_err(|_| err(StatusCode::CONFLICT, "An audit key already exists."))
    })
    .await?;
    Ok(Json(serde_json::json!({ "key_id": id })))
}

// ------------------------------------------------------------- audit (read)

/// Query keys the audit endpoints understand; anything else is dropped rather than passed on.
const AUDIT_QUERY_KEYS: [&str; 5] = ["status", "provider", "before", "limit", "since"];

/// The retention the admin has set, in days (30 until it is changed).
pub fn audit_retention_days(conn: &Connection) -> u32 {
    setting(conn, "audit_retention_days").and_then(|s| s.trim().parse().ok()).unwrap_or(30)
}

pub(crate) fn auditor(state: &AdminState) -> Result<&Arc<crate::auditor_link::AuditorLink>, Failure> {
    state.auditor.get().ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "No link to the auditor is set up."))
}

/// One read of the audit store, through the auditor (ADR 0051): the whitelisted query parameters only, and what it
/// answered. The admin never reads `audit.db` itself and never pushes anything about it but the retention.
async fn audit_read(
    state: &AdminState,
    path: &str,
    query: &std::collections::HashMap<String, String>,
) -> Result<serde_json::Value, Failure> {
    let mut url = reqwest::Url::parse("http://x/").map_err(internal)?;
    url.set_path(path);
    {
        let mut pairs = url.query_pairs_mut();
        for key in AUDIT_QUERY_KEYS {
            if let Some(v) = query.get(key) {
                pairs.append_pair(key, v);
            }
        }
    }
    let target = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    match auditor(state)?.get(&target).await {
        Some((200, body)) => Ok(body),
        Some((404, _)) => Err(err(StatusCode::NOT_FOUND, "No such record.")),
        Some(_) | None => {
            Err(err(StatusCode::SERVICE_UNAVAILABLE, "The auditor cannot be reached, so the audit trail cannot be read now."))
        }
    }
}

/// The providers as they are now, for prices: read per request, so a corrected price changes every figure at once.
async fn price_of(state: &AdminState) -> Result<impl Fn(&str) -> Option<crate::cost::Prices>, Failure> {
    let providers = {
        let db = state.db.lock().await;
        build_snapshot(db.conn()).map_err(internal)?.providers
    };
    Ok(crate::audit_read::price_lookup(providers))
}

pub async fn audit_traces(
    State(state): St,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, Failure> {
    let mut rows = audit_read(&state, "/traces", &q).await?;
    let price_of = price_of(&state).await?;
    rows.as_array_mut().into_iter().flatten().for_each(|r| crate::audit_read::add_cost(r, &price_of));
    Ok(Json(serde_json::json!({ "traces": rows })))
}

pub async fn audit_trace(State(state): St, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    // The id becomes part of a path: only the characters of an id (letters and digits, dashes) pass.
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::NOT_FOUND, "No such trace."));
    }
    let mut spans = audit_read(&state, &format!("/traces/{id}"), &Default::default()).await?;
    let list = spans.as_array_mut().filter(|l| !l.is_empty()).ok_or_else(|| err(StatusCode::NOT_FOUND, "No such trace."))?;
    let price_of = price_of(&state).await?;
    // Each attempt is priced by its own provider; the trace as a whole by the provider that served it.
    list.iter_mut().for_each(|s| crate::audit_read::add_cost(s, &price_of));
    let final_provider = list.iter().rev().find(|s| s["kind"] == "attempt").and_then(|s| s["provider_id"].as_str().map(str::to_string));
    let mut cost = None;
    if let Some(turn) = list.iter().find(|s| s["kind"] == "turn") {
        let n = |k: &str| turn[k].as_i64();
        cost = crate::cost::cost_of(&price_of, final_provider.as_deref(), n("tokens_in"), n("tokens_out"), n("tokens_cached"));
    }
    Ok(Json(serde_json::json!({ "trace_id": id, "cost": cost, "spans": spans })))
}

/// Providers the gateway found unusable (bad credentials), from the server.
pub async fn audit_provider_health(State(state): St) -> Result<Json<serde_json::Value>, Failure> {
    let (status, body) = link(&state)?
        .request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/providers/health"), None)
        .await
        .map_err(from_push)?;
    match status {
        200 => serde_json::from_slice(&body).map(Json).map_err(internal),
        s => Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {s}."))),
    }
}

#[derive(Deserialize)]
pub struct AccessQuery {
    limit: Option<usize>,
}

/// `GET /api/admin/v1/audit/access`: who opened sealed content and when, newest first (up to 100, at
/// most 500). Rows are kept in `admin.db`, written before any content is returned (see
/// `open_audit_payloads`); they hold which span and key, never content or passphrases.
pub async fn audit_access(
    State(state): St,
    axum::extract::Query(q): axum::extract::Query<AccessQuery>,
) -> Result<Json<serde_json::Value>, Failure> {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    let db = state.db.lock().await;
    let mut stmt =
        db.conn().prepare("SELECT id, span_id, key_id, opened_at, ok FROM audit_access ORDER BY id DESC LIMIT ?1").map_err(internal)?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            Ok(serde_json::json!({ "id": r.get::<_, i64>(0)?, "span_id": r.get::<_, String>(1)?, "key_id": r.get::<_, String>(2)?, "opened_at": r.get::<_, i64>(3)?, "ok": r.get::<_, i64>(4)? != 0 }))
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "access": rows })))
}

pub async fn audit_stats(
    State(state): St,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, Failure> {
    let v = audit_read(&state, "/stats", &q).await?;
    let price_of = price_of(&state).await?;
    Ok(Json(crate::audit_read::price_stats(v, &price_of)))
}

// ------------------------------------------------------------------ rotation

#[derive(Deserialize)]
pub struct RotateKey {
    current: String,
    new: String,
    #[serde(default)]
    confirm: Option<String>,
}

/// `POST /api/admin/v1/audit/rotate-key`: prove the current passphrase, then switch to a new one. A
/// new key is made and pushed (the server seals everything from then on with it); the old key is
/// retired but kept, so what it sealed still opens, with its own passphrase. Nothing already
/// sealed is touched or re-sealed.
pub async fn rotate_audit_key(State(state): St, ApiJson(b): ApiJson<RotateKey>) -> Result<Json<serde_json::Value>, Failure> {
    use crate::audit_key::{derive, derive_for_setup, key_id, new_salt, KdfParams, KeyError};
    let now = crate::auth::now();
    if let Some(wait) = state.audit_opens.locked_for(now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, &format!("Too many wrong passphrases. Try again in {} minutes.", (wait + 59) / 60))
            .retry_after(wait.max(1) as u64));
    }
    if b.confirm.as_deref().is_some_and(|c| c != b.new) {
        return Err(bad("The two new passphrases do not match."));
    }
    if b.new == b.current {
        return Err(bad("The new passphrase must be different from the current one."));
    }
    let Some(active) = state.db.lock().await.active_audit_key().map_err(internal)? else {
        return Err(err(StatusCode::CONFLICT, "There is no audit key to rotate yet."));
    };

    // The current passphrase must give the active key.
    let (cur, salt, params) = (b.current, active.salt, KdfParams::parse(&active.argon_params).unwrap_or(KdfParams::CURRENT));
    let derived = tokio::task::spawn_blocking(move || derive(&cur, &salt, params)).await.map_err(internal)?.map_err(internal)?;
    if key_id(&derived.public_bytes()) != active.key_id {
        state.audit_opens.record_wrong(now);
        return Err(err(StatusCode::FORBIDDEN, "The current passphrase is wrong."));
    }
    state.audit_opens.record_right();
    drop(derived);

    let new_salt_bytes = new_salt();
    let (pass, new_params) = (b.new, KdfParams::CURRENT);
    let kp = tokio::task::spawn_blocking(move || derive_for_setup(&pass, &new_salt_bytes, new_params)).await.map_err(internal)?.map_err(
        |e| match e {
            KeyError::TooShort => bad("The new passphrase must be at least 20 characters."),
            KeyError::Kdf(_) => internal(()),
        },
    )?;
    let public = kp.public_bytes();
    let row = crate::db::AuditKeyRow {
        key_id: key_id(&public),
        public_key: public,
        salt: new_salt_bytes,
        argon_params: new_params.to_text(),
        created_at: now,
        retired_at: None,
    };
    let (new_id, old_id) = (row.key_id.clone(), active.key_id.clone());
    // Saved and pushed together, like every other setting: if the server is away, nothing changes.
    mutate(&state, vec![], move |c| {
        crate::db::replace_active_audit_key_in(c, &row, now).map_err(|_| err(StatusCode::CONFLICT, "That key already exists."))
    })
    .await?;
    Ok(Json(serde_json::json!({ "key_id": new_id, "retired_key_id": old_id })))
}

// ------------------------------------------------------- opening sealed content

#[derive(Deserialize)]
pub struct OpenRequest {
    pub(crate) passphrase: String,
}

/// What the page and the banner need to know about the sealed-content session, for the browser that asks: whether one is
/// open and for how long, and the key registry with which keys it holds.
async fn session_status(state: &AdminState, headers: &axum::http::HeaderMap) -> serde_json::Value {
    use crate::sealed::{limits, FRESH_SIGNIN_SECS};
    let token = crate::web::session_cookie(headers);
    let browser = token.as_deref().map(crate::auth::AdminAuth::hash_token);
    let view = browser.as_deref().and_then(|b| state.sealed.view(b));
    let (idle, total) = limits(state.db.lock().await.conn());
    let keys = state.db.lock().await.audit_keys().unwrap_or_default();
    let held: Vec<String> = view.as_ref().map(|v| v.key_ids.clone()).unwrap_or_default();
    let fresh_left = token.as_deref().and_then(|t| state.auth.session_age_secs(t)).map(|age| (FRESH_SIGNIN_SECS - age).max(0));
    serde_json::json!({
        "open": view.is_some(),
        "idle_left_secs": view.as_ref().map(|v| v.idle_left_secs),
        "total_left_secs": view.as_ref().map(|v| v.total_left_secs),
        "idle_limit_secs": view.as_ref().map_or(idle.as_secs(), |v| v.idle_limit_secs),
        "total_limit_secs": view.as_ref().map_or(total.as_secs(), |v| v.total_limit_secs),
        "fresh_signin_secs_left": fresh_left,
        "keys": keys.iter().map(|k| serde_json::json!({
            "key_id": k.key_id, "created_at": k.created_at, "retired_at": k.retired_at, "open": held.contains(&k.key_id),
        })).collect::<Vec<_>>(),
    })
}

/// The banner's part of the status: whether this browser has a session and how long it has left, and the alerts of the
/// last day. Nothing here names a key or content.
pub(crate) fn banner_status(state: &AdminState, headers: &axum::http::HeaderMap) -> serde_json::Value {
    let browser = crate::web::session_cookie(headers).map(|t| crate::auth::AdminAuth::hash_token(&t));
    let view = browser.as_deref().and_then(|b| state.sealed.view(b));
    // What the sealed-content session raised and what the admin's own alerts raised, oldest first.
    let mut all = state.sealed.alerts();
    all.extend(state.notifier.notes());
    all.sort_by_key(|a| a.at);
    let alerts: Vec<_> = all.iter().map(|a| serde_json::json!({ "kind": a.kind, "at": a.at, "text": a.text })).collect();
    serde_json::json!({
        "open": view.is_some(),
        "idle_left_secs": view.as_ref().map(|v| v.idle_left_secs),
        "total_left_secs": view.as_ref().map(|v| v.total_left_secs),
        "alerts": alerts,
    })
}

/// `GET /api/admin/v1/audit/session`: is sealed content open for this browser, and for how long.
pub async fn get_unseal_session(State(state): St, headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
    // A session that is due ends now, and says so in the record, before anyone is told it is open.
    crate::sealed::sweep(&state).await;
    Json(session_status(&state, &headers).await)
}

/// `POST /api/admin/v1/audit/session`: the passphrase, entered after a fresh sign-in, opens sealed content for a
/// while (ADR 0011 rule 8). The keys it derives live in this process's memory only. The request is recorded before it
/// is handled, so an unseal that cannot be recorded does not happen.
pub async fn start_unseal_session(
    State(state): St,
    headers: axum::http::HeaderMap,
    ApiJson(b): ApiJson<OpenRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), Failure> {
    use crate::audit_key::{derive, KdfParams};
    use crate::sealed::{is_that_key, limits, FRESH_SIGNIN_SECS};
    let now = crate::auth::now();
    let token = crate::web::session_cookie(&headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Sign in again."))?;
    if state.auth.session_age_secs(&token).is_none_or(|age| age > FRESH_SIGNIN_SECS) {
        return Err(err(
            StatusCode::FORBIDDEN,
            &format!(
                "Sealed content needs a fresh sign-in: sign out, sign in again and enter the passphrase within {} minutes.",
                FRESH_SIGNIN_SECS / 60
            ),
        ));
    }
    if let Some(wait) = state.audit_opens.locked_for(now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, &format!("Too many wrong passphrases. Try again in {} minutes.", (wait + 59) / 60))
            .retry_after(wait.max(1) as u64));
    }
    let rows = state.db.lock().await.audit_keys().map_err(internal)?;
    if rows.is_empty() {
        return Err(err(StatusCode::CONFLICT, "There is no audit key yet."));
    }
    // One key for each key in the registry that this passphrase makes; each is derived once and kept only in the session.
    let mut keys = std::collections::BTreeMap::new();
    for row in &rows {
        let (pass, salt, params) = (b.passphrase.clone(), row.salt, KdfParams::parse(&row.argon_params).unwrap_or(KdfParams::CURRENT));
        let kp = tokio::task::spawn_blocking(move || derive(&pass, &salt, params)).await.map_err(internal)?.map_err(internal)?;
        if is_that_key(&row.key_id, &kp) {
            keys.insert(row.key_id.clone(), kp);
        }
    }
    // Written before anything is opened; if it cannot be written, nothing is.
    {
        let db = state.db.lock().await;
        let named = keys.keys().next().cloned().or_else(|| rows.last().map(|r| r.key_id.clone())).unwrap_or_default();
        crate::db::record_audit_access(db.conn(), "(session)", &named, now, !keys.is_empty()).map_err(internal)?;
    }
    if keys.is_empty() {
        state.audit_opens.record_wrong(now);
        // 403, not 401: the page treats 401 as "signed out".
        return Err(err(StatusCode::FORBIDDEN, "That passphrase does not open any audit key."));
    }
    state.audit_opens.record_right();
    let (idle, total) = limits(state.db.lock().await.conn());
    let browser = crate::auth::AdminAuth::hash_token(&token);
    let count = keys.len();
    let fresh = state.sealed.start(&browser, keys, idle, total);
    state.sealed.alert(
        "unsealed",
        format!(
            "Sealed audit content was {} ({} key{}); it is sealed again after {} minutes without use or {} in all.",
            if fresh { "unsealed" } else { "unsealed with another passphrase" },
            count,
            if count == 1 { "" } else { "s" },
            idle.as_secs() / 60,
            total.as_secs() / 60
        ),
    );
    let status = session_status(&state, &headers).await;
    Ok((if fresh { StatusCode::CREATED } else { StatusCode::OK }, Json(status)))
}

/// `DELETE /api/admin/v1/audit/session`: Seal. Drops the keys. The request is its record.
pub async fn end_unseal_session(State(state): St) -> StatusCode {
    state.end_sealed(crate::sealed::Ended::Seal, false).await;
    StatusCode::NO_CONTENT
}

/// `GET /api/admin/v1/audit/payloads/{span_id}`: the content of one span, opened with the session's keys. Each view is
/// recorded before it is made (so one that cannot be recorded is refused), and counts as use. Without a session there is
/// nothing to open with: `403`.
pub async fn view_audit_payloads(
    State(state): St,
    headers: axum::http::HeaderMap,
    Path(span_id): Path<String>,
) -> Result<Json<serde_json::Value>, Failure> {
    use base64::Engine;
    if span_id.is_empty() || span_id.len() > 64 || !span_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::NOT_FOUND, "No such span."));
    }
    let token = crate::web::session_cookie(&headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Sign in again."))?;
    let browser = crate::auth::AdminAuth::hash_token(&token);
    crate::sealed::sweep(&state).await;
    if !state.sealed.touch(&browser) {
        return Err(err(StatusCode::FORBIDDEN, "Sealed content is sealed. Enter the audit passphrase to open it."));
    }
    let body = match auditor(&state)?.get(&format!("/spans/{span_id}/payloads")).await {
        Some((200, body)) => body,
        Some((404, _)) => return Err(err(StatusCode::NOT_FOUND, "No such span.")),
        Some(_) | None => {
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "The auditor cannot be reached, so the sealed content cannot be read now."))
        }
    };
    let list: Vec<serde_json::Value> = serde_json::from_value(body).map_err(internal)?;
    if list.is_empty() {
        return Ok(Json(serde_json::json!({ "span_id": span_id, "payloads": [] })));
    }
    let registry = state.db.lock().await.audit_keys().map_err(internal)?;
    let mut out = Vec::new();
    let mut any = false;
    for p in &list {
        let kid = p["key_id"].as_str().unwrap_or("");
        let ct = base64::engine::general_purpose::STANDARD.decode(p["ciphertext_b64"].as_str().unwrap_or("")).map_err(internal)?;
        let text = state.sealed.open_one(&browser, kid, &ct).map(|b| String::from_utf8_lossy(&b).into_owned());
        any |= text.is_some();
        // A payload the session has no key for names the key, so the person knows which passphrase it needs.
        let named = registry.iter().find(|k| k.key_id == kid);
        out.push(serde_json::json!({
            "role": p["role"], "key_id": kid, "text": text,
            "key_created_at": named.map(|k| k.created_at), "key_retired_at": named.and_then(|k| k.retired_at),
        }));
    }
    {
        let first = list[0]["key_id"].as_str().unwrap_or("").to_string();
        let db = state.db.lock().await;
        crate::db::record_audit_access(db.conn(), &span_id, &first, crate::auth::now(), any).map_err(internal)?;
    }
    Ok(Json(serde_json::json!({ "span_id": span_id, "payloads": out })))
}

// ----------------------------------------------------------------- settings

#[derive(Serialize, Deserialize)]
pub struct AdminSettings {
    system_prompt: String,
    display: DisplaySettings,
    /// Seconds a reply may run (30 to 3600). Always present when read; when a save leaves it out,
    /// the stored value is kept.
    #[serde(default)]
    generation_timeout_secs: Option<u32>,
    /// How many earlier questions, each with its reply, go with a new one (0 to 1000; 0 sends the
    /// whole conversation). Always present when read; when a save leaves it out, it is kept.
    #[serde(default)]
    history_turns: Option<u32>,
    /// The kill switch: while on, the server starts no replies. Always present when read; when a
    /// save leaves it out, the stored value is kept.
    #[serde(default)]
    kill_switch: Option<bool>,
    /// When the switch goes on: stop the replies already running (the default), or let them finish.
    #[serde(default)]
    kill_switch_aborts_running: Option<bool>,
    /// Days audit records are kept (7 to 3650). Always present when read; left out on a save keeps it.
    #[serde(default)]
    audit_retention_days: Option<u32>,
    /// Minutes without use before sealed content seals itself (1 to 720). Always present when read; a save that leaves it
    /// out keeps it.
    #[serde(default)]
    unseal_idle_minutes: Option<u32>,
    /// Minutes in all before it seals itself (1 to 720), however it is used.
    #[serde(default)]
    unseal_total_minutes: Option<u32>,
    /// The id of the key the server seals with now. Read-only: changing it is the rotate action.
    #[serde(default, skip_deserializing)]
    audit_key_id: Option<String>,
    /// The privacy notice clients show after pairing and whenever it changes (ADR 0012): on unless switched off. Always present
    /// when read; a save that leaves it out keeps it.
    #[serde(default)]
    privacy_notice_enabled: Option<bool>,
    /// Its wording. Reads as the default wording until the administrator writes their own; a save of the default wording, or of
    /// nothing, goes back to the default.
    #[serde(default)]
    privacy_notice_text: Option<String>,
    /// The warning shown above the input box: on unless switched off.
    #[serde(default)]
    ai_warning_enabled: Option<bool>,
    #[serde(default)]
    ai_warning_text: Option<String>,
}

/// The most characters the wording of the notice and of the warning may have.
const MAX_NOTICE_CHARS: usize = 2000;
const MAX_WARNING_CHARS: usize = 300;

/// The value to keep for a wording: what the administrator wrote, trimmed, or nothing when it is empty or is the default.
fn wording_to_keep(text: &str, default: &str) -> String {
    let t = text.trim();
    if t == default {
        String::new()
    } else {
        t.to_string()
    }
}

/// The same floor the server enforces (and the audit database's own trigger): a week.
const MIN_AUDIT_RETENTION_DAYS: u32 = 7;
const MAX_AUDIT_RETENTION_DAYS: u32 = 3650;

/// The same bounds the server enforces on the snapshot.
const MIN_GENERATION_TIMEOUT_SECS: u32 = 30;
const MAX_GENERATION_TIMEOUT_SECS: u32 = 3600;
const MAX_HISTORY_TURNS: u32 = 1000;

fn check_display(d: &DisplaySettings) -> Result<(), Failure> {
    if !matches!(d.theme.as_str(), "auto" | "light" | "dark") {
        return Err(bad("Theme must be auto, light or dark."));
    }
    let in_range = |v: u32, lo: u32, hi: u32| (lo..=hi).contains(&v);
    if !in_range(d.width, 300, 4000) || !in_range(d.max_height, 200, 4000) || !in_range(d.history_width, 120, 1000) {
        return Err(bad("Width must be 300-4000, max height 200-4000 and history width 120-1000."));
    }
    if !(0.0..=0.8).contains(&d.top_percent) {
        return Err(bad("Top percent must be between 0 and 0.8."));
    }
    Ok(())
}

/// The wording to show: the one stored, or the default when none (or an empty one) is.
fn wording_or_default(stored: Option<String>, default: &str) -> String {
    stored.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

pub(crate) fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).ok()
}

pub async fn get_settings(State(state): St) -> Json<AdminSettings> {
    let db = state.db.lock().await;
    let c = db.conn();
    Json(AdminSettings {
        system_prompt: setting(c, "system_prompt").unwrap_or_else(|| askbar_proto::DEFAULT_SYSTEM_PROMPT.to_string()),
        display: setting(c, "display").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        generation_timeout_secs: Some(
            setting(c, "generation_timeout_secs")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(askbar_proto::DEFAULT_GENERATION_TIMEOUT_SECS),
        ),
        history_turns: Some(setting(c, "history_turns").and_then(|s| s.trim().parse().ok()).unwrap_or(0)),
        kill_switch: Some(
            c.query_row("SELECT kill_switch FROM budgets WHERE id = 1", [], |r| r.get::<_, i64>(0)).map(|v| v != 0).unwrap_or(false),
        ),
        kill_switch_aborts_running: Some(
            setting(c, "kill_switch_aborts_running").map(|v| !matches!(v.trim(), "0" | "false")).unwrap_or(true),
        ),
        audit_retention_days: Some(setting(c, "audit_retention_days").and_then(|s| s.trim().parse().ok()).unwrap_or(30)),
        unseal_idle_minutes: Some((crate::sealed::limits(c).0.as_secs() / 60) as u32),
        unseal_total_minutes: Some((crate::sealed::limits(c).1.as_secs() / 60) as u32),
        audit_key_id: crate::db::active_audit_key(c).ok().flatten().map(|k| k.key_id),
        privacy_notice_enabled: Some(setting(c, "privacy_notice_enabled").is_none_or(|v| v.trim() != "0")),
        privacy_notice_text: Some(wording_or_default(setting(c, "privacy_notice_text"), askbar_proto::DEFAULT_PRIVACY_NOTICE)),
        ai_warning_enabled: Some(setting(c, "ai_warning_enabled").is_none_or(|v| v.trim() != "0")),
        ai_warning_text: Some(wording_or_default(setting(c, "ai_warning_text"), askbar_proto::DEFAULT_AI_WARNING)),
    })
}

pub async fn put_settings(
    State(state): St,
    headers: axum::http::HeaderMap,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    ApiJson(s): ApiJson<AdminSettings>,
) -> Result<StatusCode, Failure> {
    // What is worth keeping in the record of this change: the two settings that change what the stack does at once.
    // The prompt and the display are not copied into it.
    if let Some(axum::Extension(n)) = &notes {
        if let Some(days) = s.audit_retention_days {
            n.add("audit_retention_days", serde_json::json!(days));
        }
        if let Some(on) = s.kill_switch {
            n.add("kill_switch", serde_json::json!(on));
        }
        if let Some(m) = s.unseal_idle_minutes {
            n.add("unseal_idle_minutes", serde_json::json!(m));
        }
        if let Some(m) = s.unseal_total_minutes {
            n.add("unseal_total_minutes", serde_json::json!(m));
        }
        // Whether each is on is worth keeping; the wording is not copied into the record (it is in the settings).
        if let Some(on) = s.privacy_notice_enabled {
            n.add("privacy_notice_enabled", serde_json::json!(on));
        }
        if let Some(on) = s.ai_warning_enabled {
            n.add("ai_warning_enabled", serde_json::json!(on));
        }
    }
    if s.privacy_notice_text.as_deref().is_some_and(|t| t.chars().count() > MAX_NOTICE_CHARS) {
        return Err(bad(&format!("The privacy notice is at most {MAX_NOTICE_CHARS} characters.")));
    }
    if s.ai_warning_text.as_deref().is_some_and(|t| t.chars().count() > MAX_WARNING_CHARS) {
        return Err(bad(&format!("The AI warning is at most {MAX_WARNING_CHARS} characters.")));
    }
    let notice_text = s.privacy_notice_text.as_deref().map(|t| wording_to_keep(t, askbar_proto::DEFAULT_PRIVACY_NOTICE));
    let warning_text = s.ai_warning_text.as_deref().map(|t| wording_to_keep(t, askbar_proto::DEFAULT_AI_WARNING));
    let (notice_on, warning_on) =
        (s.privacy_notice_enabled.map(|b| if b { "1" } else { "0" }), s.ai_warning_enabled.map(|b| if b { "1" } else { "0" }));
    if s.system_prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(bad("The system prompt is too long."));
    }
    check_display(&s.display)?;
    if let Some(secs) = s.generation_timeout_secs {
        if !(MIN_GENERATION_TIMEOUT_SECS..=MAX_GENERATION_TIMEOUT_SECS).contains(&secs) {
            return Err(bad(&format!(
                "The generation timeout must be between {MIN_GENERATION_TIMEOUT_SECS} and {MAX_GENERATION_TIMEOUT_SECS} seconds."
            )));
        }
    }
    let display = serde_json::to_string(&s.display).map_err(|_| bad("Invalid display settings."))?;
    let timeout = s.generation_timeout_secs.map(|n| n.to_string());
    if s.history_turns.is_some_and(|n| n > MAX_HISTORY_TURNS) {
        return Err(bad(&format!("History sent with each question must be between 0 and {MAX_HISTORY_TURNS} questions.")));
    }
    let turns = s.history_turns.map(|n| n.to_string());
    if let Some(days) = s.audit_retention_days {
        if days < MIN_AUDIT_RETENTION_DAYS {
            return Err(bad(&format!("Audit records are kept for at least {MIN_AUDIT_RETENTION_DAYS} days.")));
        }
        if days > MAX_AUDIT_RETENTION_DAYS {
            return Err(bad(&format!("Audit records are kept for at most {MAX_AUDIT_RETENTION_DAYS} days.")));
        }
    }
    // Keeping records for less time destroys evidence, so it takes a fresh sign-in and no API key (ADR 0040).
    let current_days = audit_retention_days(state.db.lock().await.conn());
    if s.audit_retention_days.is_some_and(|days| days < current_days) {
        crate::web::require_fresh_signin(&state, &headers)?;
    }
    let retention = s.audit_retention_days.map(|n| n.to_string());
    // How long sealed content stays open (ADR 0011 rule 11): up to 12 hours each, and the idle time may not exceed the total.
    let (old_idle, old_total) = crate::sealed::limits(state.db.lock().await.conn());
    let (old_idle_m, old_total_m) = ((old_idle.as_secs() / 60) as u32, (old_total.as_secs() / 60) as u32);
    let (idle_m, total_m) = (s.unseal_idle_minutes.unwrap_or(old_idle_m), s.unseal_total_minutes.unwrap_or(old_total_m));
    for (n, what) in [(idle_m, "The idle time"), (total_m, "The total time")] {
        if !(1..=crate::sealed::MAX_MINUTES).contains(&n) {
            return Err(bad(&format!("{what} for sealed content must be between 1 and {} minutes.", crate::sealed::MAX_MINUTES)));
        }
    }
    if idle_m > total_m {
        return Err(bad("Sealed content cannot stay open longer without use than in all."));
    }
    let (idle_text, total_text) = (idle_m.to_string(), total_m.to_string());
    let aborts = s.kill_switch_aborts_running.map(|b| if b { "1" } else { "0" });
    let kill = s.kill_switch;
    mutate(&state, vec![], move |c| {
        let mut pairs = vec![("system_prompt", s.system_prompt.as_str()), ("display", display.as_str())];
        if let Some(t) = timeout.as_deref() {
            pairs.push(("generation_timeout_secs", t));
        }
        if let Some(n) = turns.as_deref() {
            pairs.push(("history_turns", n));
        }
        if let Some(a) = aborts {
            pairs.push(("kill_switch_aborts_running", a));
        }
        if let Some(r) = retention.as_deref() {
            pairs.push(("audit_retention_days", r));
        }
        for (key, value) in [
            ("privacy_notice_enabled", notice_on),
            ("privacy_notice_text", notice_text.as_deref()),
            ("ai_warning_enabled", warning_on),
            ("ai_warning_text", warning_text.as_deref()),
        ] {
            if let Some(v) = value {
                pairs.push((key, v));
            }
        }
        pairs.push(("unseal_idle_minutes", idle_text.as_str()));
        pairs.push(("unseal_total_minutes", total_text.as_str()));
        if let Some(k) = kill {
            c.execute("UPDATE budgets SET kill_switch = ?1 WHERE id = 1", [k as i64]).map_err(internal)?;
        }
        for (k, v) in pairs {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )
            .map_err(internal)?;
        }
        Ok(())
    })
    .await?;
    // The auditor keeps the records for as long as this says: give it the setting now, not at its next round.
    if let Some(a) = state.auditor.get() {
        a.mark_dirty();
    }
    // A change of how long content stays unsealed is an alert, and applies to a session that is open (ADR 0011 rule 12).
    if (idle_m, total_m) != (old_idle_m, old_total_m) {
        state
            .sealed
            .set_limits(std::time::Duration::from_secs(u64::from(idle_m) * 60), std::time::Duration::from_secs(u64::from(total_m) * 60));
        state.sealed.alert(
            "timeouts_changed",
            format!("The time sealed content stays open was changed: {idle_m} minutes without use, {total_m} in all (it was {old_idle_m} and {old_total_m})."),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// One user's display defaults, instead of the server-wide ones.
pub async fn put_user_display(
    State(state): St,
    Path(user_id): Path<String>,
    ApiJson(d): ApiJson<DisplaySettings>,
) -> Result<StatusCode, Failure> {
    check_display(&d)?;
    let value = serde_json::to_string(&d).map_err(|_| bad("Invalid display settings."))?;
    let (status, body) = link(&state)?
        .request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/users"), None)
        .await
        .map_err(from_push)?;
    let known = status == 200
        && serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .is_some_and(|v| v.as_array().is_some_and(|a| a.iter().any(|u| u["id"].as_str() == Some(user_id.as_str()))));
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such user."));
    }
    mutate(&state, vec![], move |c| {
        c.execute(
            "INSERT INTO display_defaults (user_id, value) VALUES (?1, ?2) ON CONFLICT(user_id) DO UPDATE SET value = excluded.value",
            params![user_id, value],
        )
        .map(|_| ())
        .map_err(internal)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ------------------------------------------------------------------- backup

/// A copy of `admin.db` with the password hash, the Cloudflare token, pairing-code hashes and the admin
/// API key's hash removed.
fn admin_db_copy(db: &crate::AdminDb) -> Result<Vec<u8>, Failure> {
    let path = std::env::temp_dir().join(format!("askbar-admin-backup-{}.db", ulid::Ulid::new()));
    let result = (|| -> rusqlite::Result<Vec<u8>> {
        db.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
        let copy = Connection::open(&path)?;
        copy.execute_batch(
            "DELETE FROM admin_credentials;
             UPDATE network_settings SET cloudflare_token = NULL;
             DELETE FROM pair_codes;
             DELETE FROM admin_api_keys;
             PRAGMA journal_mode = DELETE;
             VACUUM;",
        )?;
        drop(copy);
        std::fs::read(&path).map_err(|_| rusqlite::Error::InvalidQuery)
    })();
    let _ = std::fs::remove_file(&path);
    result.map_err(internal)
}

/// The server's backup and the admin's own, in one tar file: the server's files under `server/`
/// (`server/control.db` and `server/spaces/<space id>.db`, ADR 0009), then `admin.db`.
pub async fn backup(State(state): St) -> Result<Response, Failure> {
    let (status, server_tar) = link(&state)?
        .request(reqwest::Method::POST, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/exports"), None)
        .await
        .map_err(from_push)?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
    }
    let server_files =
        askbar_service::tar::read(&server_tar).ok_or_else(|| err(StatusCode::BAD_GATEWAY, "The server's backup could not be read."))?;
    let admin_db = admin_db_copy(&*state.db.lock().await)?;
    let now = crate::auth::now();
    let mut tar = Vec::new();
    for (name, data) in &server_files {
        askbar_service::tar::append(&mut tar, &format!("server/{name}"), data, now);
    }
    askbar_service::tar::append(&mut tar, "admin.db", &admin_db, now);
    askbar_service::tar::finish(&mut tar);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-tar")
        .header(header::CONTENT_LENGTH, tar.len())
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"askbar-backup-{now}.tar\""))
        .body(axum::body::Body::from(tar))
        .map_err(internal)
}
