//! Device tokens and pairing codes.
//!
//! Tokens and codes are random, shown once, and stored only as SHA-256 hashes.
//! The API reads `Authorization: Bearer` and nothing else (no cookies).

use crate::AppState;
use crate::Db;
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PAIR_CODE_TTL_SECS: i64 = 10 * 60;

/// Inserted into request extensions by `require_device_token`.
#[derive(Clone, Debug)]
pub struct DeviceAuth {
    pub user_id: String,
    pub device_id: String,
}

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn random_secret(prefix: &str) -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    format!("{prefix}{}", URL_SAFE_NO_PAD.encode(b))
}

fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Create a one-time pairing code for `user_id`. Returns the plaintext code.
pub fn create_pair_code(db: &Db, user_id: &str, device_name: &str, ttl_secs: i64) -> rusqlite::Result<String> {
    let code = random_secret("");
    db.conn().execute(
        "INSERT INTO pair_codes (hash, user_id, device_name, expires_at, used) VALUES (?1, ?2, ?3, ?4, 0)",
        params![hash(&code), user_id, device_name, now() + ttl_secs],
    )?;
    Ok(code)
}

/// Exchange a valid, unexpired, unused code for a new device token.
///
/// The code must be listed (by its hash) in the current snapshot. Redeeming records the hash
/// in `redeemed_codes`, whose primary key makes a second redemption impossible even when two
/// requests race.
pub fn redeem_pair_code(db: &Db, code: &str) -> rusqlite::Result<Option<String>> {
    let h = hash(code);
    let Some(entry) = crate::snapshot::current(db)?.and_then(|s| s.pair_codes.into_iter().find(|c| c.hash == h)) else {
        return Ok(None);
    };
    if entry.expires_at <= now() {
        return Ok(None);
    }
    let tx = db.conn().unchecked_transaction()?;
    let claimed = tx.execute("INSERT OR IGNORE INTO redeemed_codes (hash, device_id, redeemed_at) VALUES (?1, NULL, ?2)", params![h, now()])?;
    if claimed != 1 {
        return Ok(None);
    }
    let token = random_secret("abt_");
    let name = if entry.device_name.trim().is_empty() { "device".to_string() } else { entry.device_name };
    let device_id = crate::ids::new_id();
    tx.execute(
        "INSERT INTO devices (id, user_id, name, hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![device_id, entry.user_id, name, hash(&token), now()],
    )?;
    tx.execute("UPDATE redeemed_codes SET device_id = ?1 WHERE hash = ?2", params![device_id, h])?;
    tx.commit()?;
    Ok(Some(token))
}

/// Look up a live token and record that it was just used. A device is refused if it is
/// revoked in the database or listed as revoked in the current snapshot.
pub fn authenticate(db: &Db, token: &str) -> rusqlite::Result<Option<DeviceAuth>> {
    let found = db
        .conn()
        .query_row(
            "SELECT id, user_id FROM devices WHERE hash = ?1 AND revoked_at IS NULL",
            [hash(token)],
            |r| Ok(DeviceAuth { device_id: r.get(0)?, user_id: r.get(1)? }),
        )
        .optional()?;
    let Some(auth) = found else { return Ok(None) };
    if crate::snapshot::current(db)?.is_some_and(|s| s.revoked_device_ids.iter().any(|id| *id == auth.device_id)) {
        return Ok(None);
    }
    db.conn().execute("UPDATE devices SET last_seen = ?1 WHERE id = ?2", params![now(), auth.device_id])?;
    Ok(Some(auth))
}

fn bearer(req: &Request) -> Option<&str> {
    let v = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

pub async fn require_device_token(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = bearer(&req).ok_or(StatusCode::UNAUTHORIZED)?;
    let auth = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        authenticate(&db, token).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let auth = auth.ok_or(StatusCode::UNAUTHORIZED)?;
    req.extensions_mut().insert(auth);
    Ok(next.run(req).await)
}

/// A paired device. The token itself is never stored, so it can't be shown.
#[derive(serde::Serialize)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub revoked: bool,
}

/// All devices, newest first.
pub fn list_devices(db: &Db) -> rusqlite::Result<Vec<DeviceRow>> {
    let mut stmt =
        db.conn().prepare("SELECT id, name, created_at, last_seen, revoked_at IS NOT NULL FROM devices ORDER BY created_at DESC, id DESC")?;
    let rows = stmt.query_map([], |r| Ok(DeviceRow { id: r.get(0)?, name: r.get(1)?, created_at: r.get(2)?, last_seen: r.get(3)?, revoked: r.get(4)? }))?;
    rows.collect()
}
