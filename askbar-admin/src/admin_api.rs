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

type St = State<Arc<AdminState>>;

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

fn link(state: &AdminState) -> Result<&Arc<ServerLink>, Failure> {
    state.server.as_ref().ok_or_else(offline)
}

/// Apply `apply` to `admin.db`, send the server the resulting snapshot, then send it each of
/// `keys` (provider id, API key), and commit only if all of that worked.
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
    let snapshot = match build_snapshot(conn) {
        Ok(s) => s,
        Err(e) => {
            rollback(conn);
            return Err(internal(e));
        }
    };
    if let Err(e) = link.push(&snapshot).await {
        rollback(db.conn());
        return Err(from_push(e));
    }
    for (id, key) in keys {
        if let Err(e) = link.put_key(&id, &key).await {
            rollback(db.conn());
            // The server already has the snapshot with this change; send the truth again.
            link.mark_dirty();
            return Err(from_push(e));
        }
        let _ = db.conn().execute("UPDATE providers SET has_key = 1 WHERE id = ?1", [&id]);
    }
    db.conn().execute_batch("COMMIT").map_err(internal)?;
    Ok(out)
}

// ------------------------------------------------------------------- status

/// For the "server offline" banner.
pub async fn status(State(state): St) -> Json<serde_json::Value> {
    let (online, pending, error) = match &state.server {
        Some(l) => {
            l.check().await;
            (l.server_online(), l.pending(), l.last_error())
        }
        None => (false, false, None),
    };
    Json(serde_json::json!({ "server_online": online, "pending": pending, "error": error }))
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
    allow_as_fallback: bool,
    /// Price per million tokens; absent means unknown.
    price_in: Option<f64>,
    price_out: Option<f64>,
    price_cached_in: Option<f64>,
    /// Made and kept in step by the admin itself (the bundled CLIProxyAPI): name, address and key are fixed.
    managed: bool,
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
    allow_as_fallback: bool,
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
        .prepare("SELECT id, name, type, base_url, model, max_tokens, temperature, has_key, billing, allow_as_fallback, price_in, price_out, price_cached_in, managed FROM providers ORDER BY name")
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
                allow_as_fallback: r.get::<_, i64>(9)? != 0,
                price_in: r.get(10)?,
                price_out: r.get(11)?,
                price_cached_in: r.get(12)?,
                managed: r.get::<_, i64>(13)? != 0,
            })
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
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
            "INSERT INTO providers (id, name, type, base_url, model, max_tokens, temperature, created_at, billing, allow_as_fallback, price_in, price_out, price_cached_in)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                new_id, i.name.trim(), i.kind, i.base_url.trim(), i.model.trim(), i.max_tokens, i.temperature, crate::auth::now(),
                i.billing.as_deref().unwrap_or("metered"), i.allow_as_fallback as i64, i.price_in, i.price_out, i.price_cached_in
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
                        billing = ?8, allow_as_fallback = ?9, price_in = ?10, price_out = ?11, price_cached_in = ?12 WHERE id = ?1",
                params![
                    id,
                    i.name.trim(),
                    i.kind,
                    i.base_url.trim(),
                    i.model.trim(),
                    i.max_tokens,
                    i.temperature,
                    i.billing.as_deref().unwrap_or("metered"),
                    i.allow_as_fallback as i64,
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
        let n = c.execute("DELETE FROM providers WHERE id = ?1", [&id]).map_err(internal)?;
        if n == 0 {
            return Err(err(StatusCode::NOT_FOUND, "No such provider."));
        }
        // Don't leave a dangling default or fallback behind.
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
    let (status, body) = link(&state)?
        .request(reqwest::Method::POST, &format!("{}/providers/{id}/test", askbar_proto::paths::SERVER_V1), None)
        .await
        .map_err(from_push)?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
    }
    json_response(body)
}

#[derive(Deserialize, serde::Serialize)]
pub struct ModelsInput {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    base_url: String,
    /// Write-only, used once by the server for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    /// A saved provider whose stored key the server may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_id: Option<String>,
}

/// Ask the server which models a provider offers (it has the keys and the network). Nothing is saved.
pub async fn list_models(State(state): St, ApiJson(i): ApiJson<ModelsInput>) -> Result<Response, Failure> {
    if !matches!(i.kind.as_str(), "openai" | "anthropic") {
        return Err(bad("Type must be openai or anthropic."));
    }
    let body = serde_json::to_value(&i).map_err(internal)?;
    let (status, bytes) = link(&state)?
        .request(reqwest::Method::POST, &format!("{}/models/list", askbar_proto::paths::SERVER_V1), Some(&body))
        .await
        .map_err(from_push)?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
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

/// `POST /admin/api/v1/keys`: make the admin API key. 409 `api_key_exists` while one is active.
pub async fn create_key(State(state): St, ApiJson(b): ApiJson<NewKey>) -> Result<(StatusCode, Json<CreatedKey>), Failure> {
    use crate::api_key::{create, CreateError};
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

/// `GET /admin/api/v1/keys`: the key that can be used, by name. Never the key or its hash.
pub async fn list_keys(State(state): St) -> Result<Json<Vec<crate::api_key::KeyRow>>, Failure> {
    let db = state.db.lock().await;
    crate::api_key::list(db.conn()).map(Json).map_err(internal)
}

/// `DELETE /admin/api/v1/keys/{id}`: revoke it. It stops working at once.
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

/// The address clients should pair with: the public HTTPS name when HTTPS is on, else the
/// configured fallback.
pub(crate) fn public_url(conn: &Connection) -> String {
    let net: Option<(i64, String)> =
        conn.query_row("SELECT tls_enabled, hostname FROM network_settings WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok();
    match net {
        Some((1, host)) if !host.is_empty() => format!("https://{host}"),
        _ => std::env::var("ASKBAR_PUBLIC_URL")
            .ok()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| "http://localhost:8080".into())
            .trim_end_matches('/')
            .to_string(),
    }
}

/// A one-time pairing string for a new client, good for ten minutes. Only the code's hash is stored.
pub async fn create_pairing(State(state): St, ApiJson(i): ApiJson<PairInput>) -> Result<Json<PairingOut>, Failure> {
    let name = i.name.trim().to_string();
    if name.chars().count() > 60 {
        return Err(bad("The client name is too long."));
    }
    let (status, body) = link(&state)?
        .request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/users"), None)
        .await
        .map_err(from_push)?;
    let user = (status == 200)
        .then(|| serde_json::from_slice::<serde_json::Value>(&body).ok())
        .flatten()
        .and_then(|v| v[0]["id"].as_str().map(str::to_string))
        .ok_or_else(|| err(StatusCode::BAD_GATEWAY, "The server has no user to pair a client with."))?;
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
        Ok(public_url(c))
    })
    .await?;
    Ok(Json(PairingOut { pairing: askbar_proto::encode_pairing(&url, &code), expires_in: PAIR_CODE_TTL_SECS }))
}

// -------------------------------------------------------------- audit key setup

#[derive(Deserialize)]
pub struct SetupAuditKey {
    passphrase: String,
    confirm: String,
}

/// `POST /admin/api/v1/setup/audit-key`: make the first audit key from a passphrase. The passphrase
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

/// Asks the server's server API for `path` (one of its `/api/server/v1/audit/...` endpoints) with
/// the allowed query parameters, and hands back what it answered. The admin never reads
/// `audit.db` itself and never pushes anything about it: it only pulls, and only metadata.
async fn audit_get(
    state: &AdminState,
    path: &str,
    query: &std::collections::HashMap<String, String>,
) -> Result<Json<serde_json::Value>, Failure> {
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
    let (status, body) = link(state)?.request(reqwest::Method::GET, &target, None).await.map_err(from_push)?;
    match status {
        200 => serde_json::from_slice(&body).map(Json).map_err(internal),
        404 => Err(err(StatusCode::NOT_FOUND, &server_detail(&body).unwrap_or_else(|| "No such trace.".into()))),
        503 => Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            &server_detail(&body).unwrap_or_else(|| "The server has no audit store recording.".into()),
        )),
        s => Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {s}."))),
    }
}

pub async fn audit_traces(
    State(state): St,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<serde_json::Value>, Failure> {
    audit_get(&state, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/audit/traces"), &q).await
}

pub async fn audit_trace(State(state): St, Path(id): Path<String>) -> Result<Json<serde_json::Value>, Failure> {
    // The id becomes part of a path: only the characters of an id (letters and digits, dashes) pass.
    if id.is_empty() || id.len() > 64 || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::NOT_FOUND, "No such trace."));
    }
    audit_get(&state, &format!("{}/audit/traces/{id}", askbar_proto::paths::SERVER_V1), &Default::default()).await
}

/// Providers the router found unusable (bad credentials), from the server.
pub async fn audit_provider_health(State(state): St) -> Result<Json<serde_json::Value>, Failure> {
    audit_get(&state, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/providers/health"), &Default::default()).await
}

#[derive(Deserialize)]
pub struct AccessQuery {
    limit: Option<usize>,
}

/// `GET /admin/api/v1/audit/access`: who opened sealed content and when, newest first (up to 100, at
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
    audit_get(&state, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/audit/stats"), &q).await
}

// ------------------------------------------------------------------ rotation

#[derive(Deserialize)]
pub struct RotateKey {
    current: String,
    new: String,
    #[serde(default)]
    confirm: Option<String>,
}

/// `POST /admin/api/v1/audit/rotate-key`: prove the current passphrase, then switch to a new one. A
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
    passphrase: String,
}

/// `POST /admin/api/v1/audit/payloads/{span_id}/open`: fetch the sealed content of one span from the
/// server, derive the key from the passphrase in the body, open it, and drop the key. The
/// passphrase is never stored or logged. Before any content is returned an `audit_access` row is
/// written; a wrong passphrase is recorded too, answers 403, and is rate limited (429).
pub async fn open_audit_payloads(
    State(state): St,
    Path(span_id): Path<String>,
    ApiJson(b): ApiJson<OpenRequest>,
) -> Result<Json<serde_json::Value>, Failure> {
    use crate::audit_key::{derive, key_id, open_sealed, KdfParams};
    use base64::Engine;
    let now = crate::auth::now();
    if let Some(wait) = state.audit_opens.locked_for(now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, &format!("Too many wrong passphrases. Try again in {} minutes.", (wait + 59) / 60))
            .retry_after(wait.max(1) as u64));
    }
    let (status, body) = link(&state)?
        .request(reqwest::Method::GET, &format!("{}/audit/payloads/{span_id}", askbar_proto::paths::SERVER_V1), None)
        .await
        .map_err(from_push)?;
    match status {
        200 => {}
        404 => return Err(err(StatusCode::NOT_FOUND, "No such span.")),
        s => return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {s}."))),
    }
    let list: Vec<serde_json::Value> = serde_json::from_slice(&body).map_err(internal)?;
    if list.is_empty() {
        return Ok(Json(serde_json::json!({ "span_id": span_id, "payloads": [] })));
    }

    // One key per key id the payloads name; each is derived once and dropped when this returns.
    let mut keys: std::collections::BTreeMap<String, Option<crate::audit_key::AuditKeypair>> = std::collections::BTreeMap::new();
    let passphrase = b.passphrase;
    let mut all_right = true;
    for p in &list {
        let kid = p["key_id"].as_str().unwrap_or("").to_string();
        if keys.contains_key(&kid) {
            continue;
        }
        let row = state.db.lock().await.audit_key(&kid).map_err(internal)?;
        let derived = match row {
            None => None,
            Some(row) => {
                let (pass, salt, params) =
                    (passphrase.clone(), row.salt, KdfParams::parse(&row.argon_params).unwrap_or(KdfParams::CURRENT));
                let kp = tokio::task::spawn_blocking(move || derive(&pass, &salt, params)).await.map_err(internal)?.map_err(internal)?;
                // Right passphrase only if it gives back the very key the payload names.
                (key_id(&kp.public_bytes()) == kid).then_some(kp)
            }
        };
        all_right &= derived.is_some();
        keys.insert(kid, derived);
    }
    let first_key = list[0]["key_id"].as_str().unwrap_or("").to_string();
    // The access is written before anything is returned; if it cannot be written, nothing is.
    {
        let db = state.db.lock().await;
        crate::db::record_audit_access(db.conn(), &span_id, &first_key, now, all_right).map_err(internal)?;
    }
    if !all_right {
        state.audit_opens.record_wrong(now);
        // 403, not 401: the page treats 401 as "signed out".
        return Err(err(StatusCode::FORBIDDEN, "That passphrase does not open this content."));
    }
    state.audit_opens.record_right();
    let mut out = Vec::new();
    for p in &list {
        let kid = p["key_id"].as_str().unwrap_or("");
        let ct = base64::engine::general_purpose::STANDARD.decode(p["ciphertext_b64"].as_str().unwrap_or("")).map_err(internal)?;
        let kp = keys.get(kid).and_then(|k| k.as_ref()).ok_or_else(|| internal(()))?;
        let text = open_sealed(&kp.secret, &ct).map(|b| String::from_utf8_lossy(&b).into_owned());
        out.push(serde_json::json!({ "role": p["role"], "key_id": kid, "text": text }));
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
    /// The id of the key the server seals with now. Read-only: changing it is the rotate action.
    #[serde(default, skip_deserializing)]
    audit_key_id: Option<String>,
}

/// The same floor the server enforces (and the audit database's own trigger): a week.
const MIN_AUDIT_RETENTION_DAYS: u32 = 7;
const MAX_AUDIT_RETENTION_DAYS: u32 = 3650;

/// The same bounds the server enforces on the snapshot.
const MIN_GENERATION_TIMEOUT_SECS: u32 = 30;
const MAX_GENERATION_TIMEOUT_SECS: u32 = 3600;

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

fn setting(conn: &Connection, key: &str) -> Option<String> {
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
        kill_switch: Some(
            c.query_row("SELECT kill_switch FROM budgets WHERE id = 1", [], |r| r.get::<_, i64>(0)).map(|v| v != 0).unwrap_or(false),
        ),
        kill_switch_aborts_running: Some(
            setting(c, "kill_switch_aborts_running").map(|v| !matches!(v.trim(), "0" | "false")).unwrap_or(true),
        ),
        audit_retention_days: Some(setting(c, "audit_retention_days").and_then(|s| s.trim().parse().ok()).unwrap_or(30)),
        audit_key_id: crate::db::active_audit_key(c).ok().flatten().map(|k| k.key_id),
    })
}

pub async fn put_settings(State(state): St, ApiJson(s): ApiJson<AdminSettings>) -> Result<StatusCode, Failure> {
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
    if let Some(days) = s.audit_retention_days {
        if days < MIN_AUDIT_RETENTION_DAYS {
            return Err(bad(&format!("Audit records are kept for at least {MIN_AUDIT_RETENTION_DAYS} days.")));
        }
        if days > MAX_AUDIT_RETENTION_DAYS {
            return Err(bad(&format!("Audit records are kept for at most {MAX_AUDIT_RETENTION_DAYS} days.")));
        }
    }
    let retention = s.audit_retention_days.map(|n| n.to_string());
    let aborts = s.kill_switch_aborts_running.map(|b| if b { "1" } else { "0" });
    let kill = s.kill_switch;
    mutate(&state, vec![], move |c| {
        let mut pairs = vec![("system_prompt", s.system_prompt.as_str()), ("display", display.as_str())];
        if let Some(t) = timeout.as_deref() {
            pairs.push(("generation_timeout_secs", t));
        }
        if let Some(a) = aborts {
            pairs.push(("kill_switch_aborts_running", a));
        }
        if let Some(r) = retention.as_deref() {
            pairs.push(("audit_retention_days", r));
        }
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

/// One file in a tar archive (ustar, regular files only).
fn tar_entry(out: &mut Vec<u8>, name: &str, data: &[u8], mtime: i64) {
    let mut h = [0u8; 512];
    h[..name.len()].copy_from_slice(name.as_bytes());
    let put = |h: &mut [u8; 512], at: usize, s: String| h[at..at + s.len()].copy_from_slice(s.as_bytes());
    put(&mut h, 100, "0000644\0".into());
    put(&mut h, 108, "0000000\0".into());
    put(&mut h, 116, "0000000\0".into());
    put(&mut h, 124, format!("{:011o}\0", data.len()));
    put(&mut h, 136, format!("{:011o}\0", mtime.max(0)));
    h[148..156].copy_from_slice(b"        ");
    h[156] = b'0';
    put(&mut h, 257, "ustar\0".into());
    put(&mut h, 263, "00".into());
    let sum: u32 = h.iter().map(|b| *b as u32).sum();
    put(&mut h, 148, format!("{sum:06o}\0 "));
    out.extend_from_slice(&h);
    out.extend_from_slice(data);
    out.resize(out.len().div_ceil(512) * 512, 0);
}

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

/// The server's backup and the admin's own, in one tar file: `server.db` and `admin.db`.
pub async fn backup(State(state): St) -> Result<Response, Failure> {
    let (status, server_db) = link(&state)?
        .request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/backup"), None)
        .await
        .map_err(from_push)?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
    }
    let admin_db = admin_db_copy(&*state.db.lock().await)?;
    let now = crate::auth::now();
    let mut tar = Vec::new();
    tar_entry(&mut tar, "server.db", &server_db, now);
    tar_entry(&mut tar, "admin.db", &admin_db, now);
    tar.extend_from_slice(&[0u8; 1024]);
    Response::builder()
        .header(header::CONTENT_TYPE, "application/x-tar")
        .header(header::CONTENT_LENGTH, tar.len())
        .header(header::CONTENT_DISPOSITION, format!("attachment; filename=\"askbar-backup-{now}.tar\""))
        .body(axum::body::Body::from(tar))
        .map_err(internal)
}
