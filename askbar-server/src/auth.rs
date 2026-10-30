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
pub fn redeem_pair_code(db: &Db, code: &str) -> rusqlite::Result<Option<String>> {
    let tx = db.conn().unchecked_transaction()?;
    let h = hash(code);
    // Marking it used and checking validity is one statement, so a code can't be redeemed twice.
    let claimed = tx.execute(
        "UPDATE pair_codes SET used = 1 WHERE hash = ?1 AND used = 0 AND expires_at > ?2",
        params![h, now()],
    )?;
    if claimed != 1 {
        return Ok(None);
    }
    let (user_id, name): (String, String) =
        tx.query_row("SELECT user_id, device_name FROM pair_codes WHERE hash = ?1", [&h], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let token = random_secret("abt_");
    let name = if name.trim().is_empty() { "device".to_string() } else { name };
    tx.execute(
        "INSERT INTO devices (id, user_id, name, hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![crate::ids::new_id(), user_id, name, hash(&token), now()],
    )?;
    tx.commit()?;
    Ok(Some(token))
}

/// Look up a live token and record that it was just used.
pub fn authenticate(db: &Db, token: &str) -> rusqlite::Result<Option<DeviceAuth>> {
    let found = db
        .conn()
        .query_row(
            "SELECT id, user_id FROM devices WHERE hash = ?1 AND revoked_at IS NULL",
            [hash(token)],
            |r| Ok(DeviceAuth { device_id: r.get(0)?, user_id: r.get(1)? }),
        )
        .optional()?;
    if let Some(a) = &found {
        db.conn().execute("UPDATE devices SET last_seen = ?1 WHERE id = ?2", params![now(), a.device_id])?;
    }
    Ok(found)
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
