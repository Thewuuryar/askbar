//! Admin site authentication: a password, an HttpOnly session cookie, and a
//! required header on anything that changes state.
//!
//! This router never looks at `Authorization`, so a device token is useless
//! here; the API router never looks at cookies, so a leaked admin cookie is
//! useless there.

use crate::{auth::now, AppState};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::rand_core::RngCore;
use rusqlite::OptionalExtension;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const COOKIE_NAME: &str = "askbar_admin";
pub const CSRF_HEADER: &str = "x-askbar-admin";
pub const SESSION_TTL_SECS: i64 = 12 * 60 * 60;
const MAX_FAILURES: usize = 5;
const FAILURE_WINDOW_SECS: i64 = 15 * 60;
const LOCKOUT_SECS: i64 = 15 * 60;
const HASH_KEY: &str = "admin_password_hash";

#[derive(Default)]
pub struct AdminAuth {
    /// sha256(session token) -> expiry. In memory: a restart signs everyone out.
    sessions: Mutex<HashMap<String, i64>>,
    failures: Mutex<Vec<i64>>,
    locked_until: Mutex<i64>,
}

fn sha(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt)?.to_string())
}

pub const MIN_PASSWORD_CHARS: usize = 10;
const MAX_PASSWORD_CHARS: usize = 200;

/// Store the argon2 hash of `password` unless a password already exists (first one wins).
/// Returns true if a hash was written. The server itself uses the setup endpoints; this is for tests and tools.
pub fn init_password(db: &crate::Db, password: &str) -> rusqlite::Result<bool> {
    let has: Option<String> =
        db.conn().query_row("SELECT value FROM settings WHERE key = ?1", [HASH_KEY], |r| r.get(0)).optional()?;
    if has.is_some() || password.is_empty() {
        return Ok(false);
    }
    let hash = hash_password(password).map_err(|_| rusqlite::Error::InvalidQuery)?;
    db.conn().execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", rusqlite::params![HASH_KEY, hash])?;
    Ok(true)
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == COOKIE_NAME && !v.is_empty()).then(|| v.to_string())
    })
}

impl AdminAuth {
    fn valid_session(&self, token: &str) -> bool {
        let mut s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        s.retain(|_, exp| *exp > t);
        s.contains_key(&sha(token))
    }

    fn new_session(&self) -> String {
        let mut b = [0u8; 32];
        OsRng.fill_bytes(&mut b);
        let token = URL_SAFE_NO_PAD.encode(b);
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(sha(&token), now() + SESSION_TTL_SECS);
        token
    }

    /// Seconds left on a lockout, if one is active.
    fn lockout_remaining(&self) -> Option<i64> {
        let until = *self.locked_until.lock().unwrap_or_else(|e| e.into_inner());
        (until > now()).then(|| until - now())
    }

    fn record_failure(&self) {
        let t = now();
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        f.retain(|x| *x > t - FAILURE_WINDOW_SECS);
        f.push(t);
        if f.len() >= MAX_FAILURES {
            *self.locked_until.lock().unwrap_or_else(|e| e.into_inner()) = t + LOCKOUT_SECS;
            f.clear();
        }
    }

    /// Sign out every session except `keep`.
    fn keep_only(&self, keep: Option<&str>) {
        let keep = keep.map(sha);
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| Some(k) == keep.as_ref());
    }

    fn clear_failures(&self) {
        self.failures.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// State-changing requests must carry `X-Askbar-Admin: 1`. A cross-site form or
/// simple request can't add custom headers, and cross-origin scripts need a CORS
/// preflight we never approve.
pub async fn require_admin_header(req: Request, next: Next) -> Result<Response, StatusCode> {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !safe && req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(req).await)
}

/// The admin site loads only its own files, and may not be framed.
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// Cookie only. `Authorization` is deliberately ignored.
pub async fn require_session(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Result<Response, StatusCode> {
    let token = session_cookie(req.headers()).ok_or(StatusCode::UNAUTHORIZED)?;
    if !state.admin.valid_session(&token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

#[derive(Deserialize)]
pub struct Login {
    password: String,
}

fn set_cookie(headers: &HeaderMap, value: &str, max_age: i64) -> HeaderValue {
    // Behind the TLS proxy the app itself sees plain http; trust its hint to mark the cookie Secure.
    let secure = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https");
    let c = format!(
        "{COOKIE_NAME}={value}; HttpOnly; SameSite=Strict; Path=/admin; Max-Age={max_age}{}",
        if secure { "; Secure" } else { "" }
    );
    HeaderValue::from_str(&c).expect("cookie is plain ASCII")
}

pub async fn login(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(body): Json<Login>) -> Response {
    if let Some(secs) = state.admin.lockout_remaining() {
        let mut r = StatusCode::TOO_MANY_REQUESTS.into_response();
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(secs));
        return r;
    }
    let Some(stored) = stored_hash(&state) else {
        // No password yet: the setup page has to create one first.
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !verify(stored, body.password).await {
        state.admin.record_failure();
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.admin.clear_failures();
    let token = state.admin.new_session();
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, &token, SESSION_TTL_SECS));
    r
}

pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Some(t) = session_cookie(&headers) {
        state.admin.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&sha(&t));
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, "", 0));
    r
}

fn stored_hash(state: &AppState) -> Option<String> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db.conn().query_row("SELECT value FROM settings WHERE key = ?1", [HASH_KEY], |r| r.get(0)).optional().ok().flatten()
}

async fn verify(stored: String, password: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored)
            .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn check_new_password(pw: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let n = pw.chars().count();
    let msg = if n < MIN_PASSWORD_CHARS {
        format!("The password must be at least {MIN_PASSWORD_CHARS} characters.")
    } else if n > MAX_PASSWORD_CHARS {
        "The password is too long.".to_string()
    } else {
        return Ok(());
    };
    Err((StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg }))))
}

pub(crate) type Failure = (StatusCode, Json<serde_json::Value>);

pub(crate) fn err(status: StatusCode, msg: &str) -> Failure {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// What the setup page needs to decide which screen to show. Booleans only.
pub async fn setup_status(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "password_set": stored_hash(&state).is_some(), "key_set": state.setup_complete() }))
}

/// First run: the first visitor chooses the admin password and is signed in. Refused once one exists.
pub async fn setup_password(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(body): Json<Login>) -> Result<Response, Failure> {
    check_new_password(&body.password)?;
    let password = body.password;
    let hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not hash the password."))?;
    {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        // INSERT OR IGNORE: if two visitors race, exactly one row lands.
        let n = db
            .conn()
            .execute("INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)", rusqlite::params![HASH_KEY, hash])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the password."))?;
        if n != 1 {
            return Err(err(StatusCode::CONFLICT, "An admin password is already set."));
        }
    }
    let token = state.admin.new_session();
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, &token, SESSION_TTL_SECS));
    Ok(r)
}

#[derive(Deserialize)]
pub struct MasterKeyInput {
    /// A key the user already has. Missing means "generate one".
    #[serde(default)]
    key: Option<String>,
}

/// Create the master key file and return the key, once, so it can be saved in a password manager.
/// Needs a session; refused once a key exists.
pub async fn setup_master_key(State(state): State<Arc<AppState>>, Json(body): Json<MasterKeyInput>) -> Result<Json<serde_json::Value>, Failure> {
    if state.setup_complete() {
        return Err(err(StatusCode::CONFLICT, "A master key already exists."));
    }
    let path = state.key_path.clone();
    let provided = body.key.filter(|k| !k.trim().is_empty());
    if let Some(k) = &provided {
        if k.trim().chars().count() < crate::secrets::MIN_MASTER_KEY_LEN {
            return Err(err(StatusCode::BAD_REQUEST, "A master key must be at least 32 characters."));
        }
    }
    let made = tokio::task::spawn_blocking(move || match provided {
        Some(k) => crate::secrets::write_key_file(&path, &k).map(|s| (s, k.trim().to_string())),
        None => crate::secrets::generate_key(&path),
    })
    .await;
    match made {
        Ok(Ok((secrets, key))) => {
            state.set_secrets(secrets);
            Ok(Json(serde_json::json!({ "key": key })))
        }
        Ok(Err(e)) => {
            tracing::error!("could not create the master key: {e}");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the master key file. Check that the secrets volume is writable."))
        }
        Err(_) => Err(err(StatusCode::INTERNAL_SERVER_ERROR, "Could not create the master key file.")),
    }
}

#[derive(Deserialize)]
pub struct ChangePassword {
    current: String,
    new: String,
}

/// Change the password. The current one is required; every other session is signed out.
pub async fn change_password(State(state): State<Arc<AppState>>, headers: HeaderMap, Json(body): Json<ChangePassword>) -> Result<StatusCode, Failure> {
    if state.admin.lockout_remaining().is_some() {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "Too many attempts. Try again later."));
    }
    let stored = stored_hash(&state).ok_or_else(|| err(StatusCode::CONFLICT, "No password is set."))?;
    if !verify(stored, body.current).await {
        state.admin.record_failure();
        // 403, not 401: the page treats 401 as "signed out".
        return Err(err(StatusCode::FORBIDDEN, "The current password is wrong."));
    }
    state.admin.clear_failures();
    check_new_password(&body.new)?;
    let new = body.new;
    let hash = tokio::task::spawn_blocking(move || hash_password(&new))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not hash the password."))?;
    {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db.conn()
            .execute("UPDATE settings SET value = ?1 WHERE key = ?2", rusqlite::params![hash, HASH_KEY])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the password."))?;
    }
    state.admin.keep_only(session_cookie(&headers).as_deref());
    Ok(StatusCode::NO_CONTENT)
}
