//! The admin site's API: thin handlers over `admin.db` and the server's management API.
//!
//! The rule for anything that changes configuration: write `admin.db`, send the server the new
//! snapshot, and only keep the change if the server accepted it. If the server is not there the
//! answer is `502` and nothing was changed, so the two never disagree. API keys are the one
//! thing `admin.db` never holds: they go straight to the server.

use crate::mgmt::{build_snapshot, PushError, ServerLink};
use crate::web::{err, Failure};
use crate::AdminState;
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

fn from_push(e: PushError) -> Failure {
    match e {
        PushError::Unreachable(_) => offline(),
        PushError::Refused { status: 400, body } => {
            let why = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v["error"].as_str().map(str::to_string));
            bad(&why.unwrap_or_else(|| "The server did not accept that.".into()))
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
async fn mutate<T>(state: &AdminState, keys: Vec<(String, String)>, apply: impl FnOnce(&Connection) -> Result<T, Failure>) -> Result<T, Failure> {
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
        .prepare("SELECT id, name, type, base_url, model, max_tokens, temperature, has_key FROM providers ORDER BY name")
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
            })
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(rows))
}

pub async fn create_provider(State(state): St, Json(i): Json<ProviderInput>) -> Result<(StatusCode, Json<serde_json::Value>), Failure> {
    validate(&i)?;
    let id = ulid::Ulid::new().to_string();
    let keys: Vec<(String, String)> = i.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(|k| vec![(id.clone(), k.to_string())]).unwrap_or_default();
    let new_id = id.clone();
    mutate(&state, keys, move |c| {
        c.execute(
            "INSERT INTO providers (id, name, type, base_url, model, max_tokens, temperature, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![new_id, i.name.trim(), i.kind, i.base_url.trim(), i.model.trim(), i.max_tokens, i.temperature, crate::auth::now()],
        )
        .map(|_| ())
        .map_err(clash_or_internal)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(serde_json::json!({ "id": id }))))
}

pub async fn update_provider(State(state): St, Path(id): Path<String>, Json(i): Json<ProviderInput>) -> Result<StatusCode, Failure> {
    validate(&i)?;
    let keys: Vec<(String, String)> = i.api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()).map(|k| vec![(id.clone(), k.to_string())]).unwrap_or_default();
    mutate(&state, keys, move |c| {
        let n = c
            .execute(
                "UPDATE providers SET name = ?2, type = ?3, base_url = ?4, model = ?5, max_tokens = ?6, temperature = ?7 WHERE id = ?1",
                params![id, i.name.trim(), i.kind, i.base_url.trim(), i.model.trim(), i.max_tokens, i.temperature],
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
        .request(reqwest::Method::POST, &format!("/mgmt/v1/providers/{id}/test"), None)
        .await
        .map_err(from_push)?;
    if status != 200 {
        return Err(err(StatusCode::BAD_GATEWAY, &format!("The server answered {status}.")));
    }
    json_response(body)
}

fn json_response(body: Vec<u8>) -> Result<Response, Failure> {
    Response::builder().header(header::CONTENT_TYPE, "application/json").body(axum::body::Body::from(body)).map_err(internal)
}

// ------------------------------------------------------------------ devices

async fn revoked_ids(state: &AdminState) -> Vec<String> {
    let db = state.db.lock().await;
    db.conn()
        .prepare("SELECT device_id FROM revoked_devices")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default()
}

/// The server's devices, with the ones this admin has revoked marked as such.
async fn devices(state: &AdminState) -> Result<Vec<serde_json::Value>, Failure> {
    let (status, body) = link(state)?.request(reqwest::Method::GET, "/mgmt/v1/devices", None).await.map_err(from_push)?;
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

pub async fn list_devices(State(state): St) -> Result<Json<Vec<serde_json::Value>>, Failure> {
    devices(&state).await.map(Json)
}

/// Revoke a device by putting its id in the snapshot; the server refuses it from then on.
pub async fn revoke_device(State(state): St, Path(id): Path<String>) -> Result<StatusCode, Failure> {
    if !devices(&state).await?.iter().any(|d| d["id"].as_str() == Some(id.as_str())) {
        return Err(err(StatusCode::NOT_FOUND, "No such device."));
    }
    mutate(&state, vec![], move |c| {
        c.execute("INSERT OR IGNORE INTO revoked_devices (device_id, revoked_at) VALUES (?1, ?2)", params![id, crate::auth::now()]).map(|_| ()).map_err(internal)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
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

/// The address devices should pair with: the public HTTPS name when HTTPS is on, else the
/// configured fallback.
fn public_url(conn: &Connection) -> String {
    let net: Option<(i64, String)> = conn.query_row("SELECT tls_enabled, hostname FROM network_settings WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).ok();
    match net {
        Some((1, host)) if !host.is_empty() => format!("https://{host}"),
        _ => std::env::var("ASKBAR_PUBLIC_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "http://localhost:8080".into()).trim_end_matches('/').to_string(),
    }
}

/// A one-time pairing string for a new device, good for ten minutes. Only the code's hash is stored.
pub async fn create_pairing(State(state): St, Json(i): Json<PairInput>) -> Result<Json<PairingOut>, Failure> {
    let name = i.name.trim().to_string();
    if name.chars().count() > 60 {
        return Err(bad("The device name is too long."));
    }
    let (status, body) = link(&state)?.request(reqwest::Method::GET, "/mgmt/v1/users", None).await.map_err(from_push)?;
    let user = (status == 200)
        .then(|| serde_json::from_slice::<serde_json::Value>(&body).ok())
        .flatten()
        .and_then(|v| v[0]["id"].as_str().map(str::to_string))
        .ok_or_else(|| err(StatusCode::BAD_GATEWAY, "The server has no user to pair a device with."))?;
    let code = random_code();
    let hash = crate::mgmt::sha256_hex(&code);
    let url = mutate(&state, vec![], move |c| {
        let now = crate::auth::now();
        c.execute("DELETE FROM pair_codes WHERE expires_at <= ?1", [now]).map_err(internal)?;
        c.execute(
            "INSERT INTO pair_codes (hash, user_id, device_name, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![hash, user, name, now + PAIR_CODE_TTL_SECS, now],
        )
        .map_err(internal)?;
        Ok(public_url(c))
    })
    .await?;
    Ok(Json(PairingOut { pairing: askbar_proto::encode_pairing(&url, &code), expires_in: PAIR_CODE_TTL_SECS }))
}

// ----------------------------------------------------------------- settings

#[derive(Serialize, Deserialize)]
pub struct AdminSettings {
    system_prompt: String,
    display: DisplaySettings,
}

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
    })
}

pub async fn put_settings(State(state): St, Json(s): Json<AdminSettings>) -> Result<StatusCode, Failure> {
    if s.system_prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(bad("The system prompt is too long."));
    }
    check_display(&s.display)?;
    let display = serde_json::to_string(&s.display).map_err(|_| bad("Invalid display settings."))?;
    mutate(&state, vec![], move |c| {
        for (k, v) in [("system_prompt", s.system_prompt.as_str()), ("display", display.as_str())] {
            c.execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", params![k, v]).map_err(internal)?;
        }
        Ok(())
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One user's display defaults, instead of the server-wide ones.
pub async fn put_user_display(State(state): St, Path(user_id): Path<String>, Json(d): Json<DisplaySettings>) -> Result<StatusCode, Failure> {
    check_display(&d)?;
    let value = serde_json::to_string(&d).map_err(|_| bad("Invalid display settings."))?;
    let (status, body) = link(&state)?.request(reqwest::Method::GET, "/mgmt/v1/users", None).await.map_err(from_push)?;
    let known = status == 200
        && serde_json::from_slice::<serde_json::Value>(&body).ok().is_some_and(|v| v.as_array().is_some_and(|a| a.iter().any(|u| u["id"].as_str() == Some(user_id.as_str()))));
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such user."));
    }
    mutate(&state, vec![], move |c| {
        c.execute("INSERT INTO display_defaults (user_id, value) VALUES (?1, ?2) ON CONFLICT(user_id) DO UPDATE SET value = excluded.value", params![user_id, value])
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

/// A copy of `admin.db` with the password hash, the Cloudflare token and pairing-code hashes removed.
fn admin_db_copy(db: &crate::AdminDb) -> Result<Vec<u8>, Failure> {
    let path = std::env::temp_dir().join(format!("askbar-admin-backup-{}.db", ulid::Ulid::new()));
    let result = (|| -> rusqlite::Result<Vec<u8>> {
        db.conn().execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
        let copy = Connection::open(&path)?;
        copy.execute_batch(
            "DELETE FROM admin_credentials;
             UPDATE network_settings SET cloudflare_token = NULL;
             DELETE FROM pair_codes;
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
    let (status, server_db) = link(&state)?.request(reqwest::Method::GET, "/mgmt/v1/backup", None).await.map_err(from_push)?;
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
