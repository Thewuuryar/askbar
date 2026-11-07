//! Client tokens and pairing codes.
//!
//! Tokens and codes are random, shown once, and stored only as SHA-256 hashes.
//! The API reads `Authorization: Bearer` and nothing else (no cookies).

use crate::error::ApiError;
use crate::AppState;
use askbar_proto::problem::ErrorCode;
use crate::Db;
use axum::{
    extract::{Request, State},
    http::header,
    middleware::Next,
    response::Response,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Inserted into request extensions by `require_client_token`.
#[derive(Clone, Debug)]
pub struct ClientAuth {
    pub user_id: String,
    pub client_id: String,
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

/// Exchange a valid, unexpired, unused code for a new client token.
///
/// The code must be listed (by its hash) in the current snapshot. Redeeming records the hash
/// in `redeemed_codes`, whose primary key makes a second redemption impossible even when two
/// requests race.
pub fn redeem_pair_code(db: &Db, code: &str) -> rusqlite::Result<Option<String>> {
    let h = hash(code);
    // A code the admin made (in the snapshot), or one a paired client asked for (this server's own).
    let from_admin = crate::snapshot::current(db)?.and_then(|s| s.pair_codes.into_iter().find(|c| c.hash == h));
    let entry = match from_admin {
        Some(e) => e,
        None => {
            let own = db
                .conn()
                .query_row("SELECT user_id, client_name, expires_at FROM client_pair_codes WHERE hash = ?1", [&h], |r| {
                    Ok(askbar_proto::SnapshotPairCode { hash: h.clone(), user_id: r.get(0)?, client_name: r.get(1)?, expires_at: r.get(2)? })
                })
                .optional()?;
            let Some(e) = own else { return Ok(None) };
            e
        }
    };
    if entry.expires_at <= now() {
        return Ok(None);
    }
    // The user must exist here (the admin pushes users with the codes); otherwise the code is no good.
    let user_known = db.conn().query_row("SELECT 1 FROM users WHERE id = ?1", [&entry.user_id], |_| Ok(())).optional()?.is_some();
    if !user_known {
        return Ok(None);
    }
    let tx = db.conn().unchecked_transaction()?;
    let claimed = tx.execute("INSERT OR IGNORE INTO redeemed_codes (hash, client_id, redeemed_at) VALUES (?1, NULL, ?2)", params![h, now()])?;
    if claimed != 1 {
        return Ok(None);
    }
    let token = random_secret("abt_");
    let name = if entry.client_name.trim().is_empty() { "client".to_string() } else { entry.client_name };
    let client_id = crate::ids::new_id();
    tx.execute(
        "INSERT INTO clients (id, user_id, name, hash, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![client_id, entry.user_id, name, hash(&token), now()],
    )?;
    tx.execute("UPDATE redeemed_codes SET client_id = ?1 WHERE hash = ?2", params![client_id, h])?;
    tx.commit()?;
    Ok(Some(token))
}

/// How long a client's own pairing code lasts, and how many may be open per user at once.
pub const CLIENT_PAIR_CODE_TTL_SECS: i64 = 10 * 60;
pub const MAX_OPEN_CLIENT_PAIR_CODES: i64 = 5;

/// A paired client asks for a code to pair another client of its user (ADR 0008). `None` when the user
/// already has `MAX_OPEN_CLIENT_PAIR_CODES` unexpired, unused codes. Only the code's hash is stored.
pub fn new_client_pair_code(db: &Db, auth: &ClientAuth, client_name: &str) -> rusqlite::Result<Option<String>> {
    let now = now();
    db.conn().execute("DELETE FROM client_pair_codes WHERE expires_at <= ?1", [now])?;
    let open: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM client_pair_codes c WHERE c.user_id = ?1 AND NOT EXISTS (SELECT 1 FROM redeemed_codes r WHERE r.hash = c.hash)",
        [&auth.user_id],
        |r| r.get(0),
    )?;
    if open >= MAX_OPEN_CLIENT_PAIR_CODES {
        return Ok(None);
    }
    let code = random_secret("");
    db.conn().execute(
        "INSERT INTO client_pair_codes (hash, user_id, client_name, created_by, expires_at, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![hash(&code), auth.user_id, client_name.trim(), auth.client_id, now + CLIENT_PAIR_CODE_TTL_SECS, now],
    )?;
    Ok(Some(code))
}

/// Look up a live token and record that it was just used. A client is refused if it is
/// revoked in the database or listed as revoked in the current snapshot.
pub fn authenticate(db: &Db, token: &str) -> rusqlite::Result<Option<ClientAuth>> {
    let found = db
        .conn()
        .query_row(
            "SELECT id, user_id FROM clients WHERE hash = ?1 AND revoked_at IS NULL",
            [hash(token)],
            |r| Ok(ClientAuth { client_id: r.get(0)?, user_id: r.get(1)? }),
        )
        .optional()?;
    let Some(auth) = found else { return Ok(None) };
    if crate::snapshot::current(db)?.is_some_and(|s| s.revoked_client_ids.contains(&auth.client_id)) {
        return Ok(None);
    }
    db.conn().execute("UPDATE clients SET last_seen = ?1 WHERE id = ?2", params![now(), auth.client_id])?;
    Ok(Some(auth))
}

fn bearer(req: &Request) -> Option<&str> {
    let v = req.headers().get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = v.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("bearer") && !token.trim().is_empty()).then(|| token.trim())
}

pub async fn require_client_token(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let token = bearer(&req).ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "Send the client token as `Authorization: Bearer <token>`."))?;
    let auth = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        authenticate(&db, token).map_err(|_| ApiError::code(ErrorCode::Internal))?
    };
    let auth = auth.ok_or_else(|| ApiError::new(ErrorCode::Unauthorized, "The server does not accept this token. It may have been revoked."))?;
    req.extensions_mut().insert(auth);
    Ok(next.run(req).await)
}

/// A paired client. The token itself is never stored, so it can't be shown.
#[derive(serde::Serialize)]
pub struct ClientRow {
    pub id: String,
    /// The user the client belongs to (ADR 0008).
    pub user_id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen: Option<i64>,
    pub revoked: bool,
}

/// All clients, newest first.
pub fn list_clients(db: &Db) -> rusqlite::Result<Vec<ClientRow>> {
    let mut stmt =
        db.conn().prepare("SELECT id, user_id, name, created_at, last_seen, revoked_at IS NOT NULL FROM clients ORDER BY created_at DESC, id DESC")?;
    let rows = stmt.query_map([], |r| Ok(ClientRow { id: r.get(0)?, user_id: r.get(1)?, name: r.get(2)?, created_at: r.get(3)?, last_seen: r.get(4)?, revoked: r.get(5)? }))?;
    rows.collect()
}
