//! Admin site authentication: a password, an HttpOnly session cookie, and a required
//! header on anything that changes state. Only the cookie counts; `Authorization` is
//! ignored, so no token of any kind opens the admin.

use crate::{auth::SESSION_TTL_SECS, AdminState};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::{
    extract::{Request, State},
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::OptionalExtension;
use serde::Deserialize;
use std::sync::Arc;

pub const COOKIE_NAME: &str = "askbar_admin";
pub const CSRF_HEADER: &str = "x-askbar-admin";
pub const MIN_PASSWORD_CHARS: usize = 10;
const MAX_PASSWORD_CHARS: usize = 200;

pub type Failure = (StatusCode, Json<serde_json::Value>);

pub fn err(status: StatusCode, msg: &str) -> Failure {
    (status, Json(serde_json::json!({ "error": msg })))
}

fn hash_password(password: &str) -> Result<String, argon2::password_hash::Error> {
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default().hash_password(password.as_bytes(), &salt)?.to_string())
}

/// Store the argon2 hash of `password` unless a password already exists (the first one wins).
/// Returns true if a hash was written. Tests and tools use this; the site uses the setup endpoint.
pub fn init_password(db: &crate::AdminDb, password: &str) -> rusqlite::Result<bool> {
    if password.is_empty() {
        return Ok(false);
    }
    let hash = hash_password(password).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let n = db.conn().execute(
        "INSERT OR IGNORE INTO admin_credentials (id, password_hash, updated_at) VALUES (1, ?1, ?2)",
        rusqlite::params![hash, crate::auth::now()],
    )?;
    Ok(n == 1)
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == COOKIE_NAME && !v.is_empty()).then(|| v.to_string())
    })
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

fn stored_hash(state: &AdminState) -> Option<String> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    db.conn().query_row("SELECT password_hash FROM admin_credentials WHERE id = 1", [], |r| r.get(0)).optional().ok().flatten()
}

async fn verify(stored: String, password: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&stored).map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok()).unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn check_new_password(pw: &str) -> Result<(), Failure> {
    let n = pw.chars().count();
    let msg = if n < MIN_PASSWORD_CHARS {
        format!("The password must be at least {MIN_PASSWORD_CHARS} characters.")
    } else if n > MAX_PASSWORD_CHARS {
        "The password is too long.".to_string()
    } else {
        return Ok(());
    };
    Err(err(StatusCode::BAD_REQUEST, &msg))
}

// ------------------------------------------------------------------ middleware

/// State-changing requests must carry `X-Askbar-Admin: 1`. A cross-site form or simple
/// request can't add custom headers, and cross-origin scripts need a CORS preflight we
/// never approve.
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
pub async fn require_session(State(state): State<Arc<AdminState>>, req: Request, next: Next) -> Result<Response, StatusCode> {
    let token = session_cookie(req.headers()).ok_or(StatusCode::UNAUTHORIZED)?;
    if !state.auth.valid_session(&token) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(req).await)
}

// -------------------------------------------------------------------- handlers

#[derive(Deserialize)]
pub struct Login {
    password: String,
}

pub async fn login(State(state): State<Arc<AdminState>>, headers: HeaderMap, Json(body): Json<Login>) -> Response {
    if let Some(secs) = state.auth.lockout_remaining() {
        let mut r = StatusCode::TOO_MANY_REQUESTS.into_response();
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(secs));
        return r;
    }
    let Some(stored) = stored_hash(&state) else {
        // No password yet: the setup page has to create one first.
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !verify(stored, body.password).await {
        state.auth.record_failure();
        return StatusCode::UNAUTHORIZED.into_response();
    }
    state.auth.clear_failures();
    let token = state.auth.new_session();
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, &token, SESSION_TTL_SECS));
    r
}

pub async fn logout(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if let Some(t) = session_cookie(&headers) {
        state.auth.end_session(&t);
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, "", 0));
    r
}

/// What the setup page needs to decide which screen to show. Booleans only.
pub async fn setup_status(State(state): State<Arc<AdminState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({ "password_set": stored_hash(&state).is_some() }))
}

/// First run: the first visitor chooses the admin password and is signed in. Refused once one exists.
pub async fn setup_password(State(state): State<Arc<AdminState>>, headers: HeaderMap, Json(body): Json<Login>) -> Result<Response, Failure> {
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
            .execute(
                "INSERT OR IGNORE INTO admin_credentials (id, password_hash, updated_at) VALUES (1, ?1, ?2)",
                rusqlite::params![hash, crate::auth::now()],
            )
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the password."))?;
        if n != 1 {
            return Err(err(StatusCode::CONFLICT, "An admin password is already set."));
        }
    }
    let token = state.auth.new_session();
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&headers, &token, SESSION_TTL_SECS));
    Ok(r)
}

#[derive(Deserialize)]
pub struct ChangePassword {
    current: String,
    new: String,
}

/// Change the password. The current one is required; every other session is signed out.
pub async fn change_password(State(state): State<Arc<AdminState>>, headers: HeaderMap, Json(body): Json<ChangePassword>) -> Result<StatusCode, Failure> {
    if state.auth.lockout_remaining().is_some() {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, "Too many attempts. Try again later."));
    }
    let stored = stored_hash(&state).ok_or_else(|| err(StatusCode::CONFLICT, "No password is set."))?;
    if !verify(stored, body.current).await {
        state.auth.record_failure();
        // 403, not 401: the page treats 401 as "signed out".
        return Err(err(StatusCode::FORBIDDEN, "The current password is wrong."));
    }
    state.auth.clear_failures();
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
            .execute("UPDATE admin_credentials SET password_hash = ?1, updated_at = ?2 WHERE id = 1", rusqlite::params![hash, crate::auth::now()])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the password."))?;
    }
    state.auth.keep_only(session_cookie(&headers).as_deref());
    Ok(StatusCode::NO_CONTENT)
}
