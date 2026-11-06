//! Admin site authentication: a password, an HttpOnly session cookie, and a required
//! header on anything that changes state. Only the cookie counts; `Authorization` is
//! ignored, so no token of any kind opens the admin.

use crate::{auth::SESSION_TTL_SECS, AdminState};
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::problem::ErrorCode;
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

pub type Failure = ApiError;

/// An error with the sentence `msg`; the status picks the code. The admin signs in with a cookie, so a
/// 401 carries no `WWW-Authenticate` challenge.
pub fn err(status: StatusCode, msg: &str) -> Failure {
    ApiError::from(status).detail(msg).no_challenge()
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

async fn stored_hash(state: &AdminState) -> Option<String> {
    let db = state.db.lock().await;
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

/// A change made from a browser must carry `X-Askbar-Admin: 1`. A cross-site form or simple request
/// can't add custom headers, and cross-origin scripts need a CORS preflight we never approve. Reads
/// need nothing. (An API key is not a cookie, so a request made with one skips this: see `require_admin_auth`.)
fn csrf_missing(req: &Request) -> bool {
    let safe = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    !safe && req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1")
}

fn csrf_error() -> ApiError {
    ApiError::new(ErrorCode::Forbidden, "A change needs the header `X-Askbar-Admin: 1`.").no_challenge()
}

/// For the open routes that change things (signing in, first-run setup).
pub async fn require_admin_header(req: Request, next: Next) -> Result<Response, ApiError> {
    if csrf_missing(&req) {
        return Err(csrf_error());
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

fn signed_out() -> ApiError {
    ApiError::new(ErrorCode::Unauthorized, "Sign in first.").no_challenge()
}

/// The person's own session: the cookie, and the browser header on a change. `Authorization` is
/// deliberately ignored, so no key or token opens the routes that manage keys, the password or the session.
pub async fn require_session(State(state): State<Arc<AdminState>>, req: Request, next: Next) -> Result<Response, ApiError> {
    let token = session_cookie(req.headers()).ok_or_else(signed_out)?;
    if !state.auth.valid_session(&token) {
        return Err(signed_out());
    }
    if csrf_missing(&req) {
        return Err(csrf_error());
    }
    Ok(next.run(req).await)
}

/// Either the person's session (cookie, plus the browser header on a change) or an admin API key
/// (`Authorization: Bearer askbar-admin-...`). If an `Authorization` header is present it alone is
/// judged: a wrong key is never rescued by a cookie. Failed keys count toward the same lockout as
/// wrong passwords.
pub async fn require_admin_auth(State(state): State<Arc<AdminState>>, req: Request, next: Next) -> Result<Response, ApiError> {
    use crate::api_key::{self, RouteClass};
    let Some(value) = req.headers().get(header::AUTHORIZATION) else {
        return require_session(State(state), req, next).await;
    };
    if let Some(secs) = state.auth.lockout_remaining() {
        return Err(too_many_attempts(secs));
    }
    let key = value.to_str().ok().and_then(|v| v.split_once(' ')).filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer")).map(|(_, k)| k.trim().to_string());
    let scope = match key {
        Some(k) => {
            let db = state.db.lock().await;
            api_key::authenticate(db.conn(), &k, crate::auth::now()).map_err(|_| ApiError::code(ErrorCode::Internal))?
        }
        None => None,
    };
    match scope {
        Some(scope) if scope.allows(RouteClass::Api) => Ok(next.run(req).await),
        _ => {
            state.auth.record_failure();
            Err(ApiError::new(ErrorCode::Unauthorized, "That API key is not valid. It may have been revoked.").no_challenge())
        }
    }
}

/// 429 with how long to wait.
fn too_many_attempts(secs: i64) -> ApiError {
    let secs = secs.max(1) as u64;
    let wait = ((secs + 59) / 60).max(1);
    ApiError::new(ErrorCode::TooManyAttempts, format!("Too many wrong passwords or keys. Try again in {wait} minute{}.", if wait == 1 { "" } else { "s" }))
        .retry_after(secs)
        .no_challenge()
}

// -------------------------------------------------------------------- handlers

#[derive(Deserialize)]
pub struct Login {
    password: String,
}

pub async fn login(State(state): State<Arc<AdminState>>, headers: HeaderMap, ApiJson(body): ApiJson<Login>) -> Response {
    if let Some(secs) = state.auth.lockout_remaining() {
        return too_many_attempts(secs).into_response();
    }
    let Some(stored) = stored_hash(&state).await else {
        // No password yet: the setup page has to create one first.
        return ApiError::new(ErrorCode::SetupRequired, "Set the admin password first.").no_challenge().into_response();
    };
    if !verify(stored, body.password).await {
        state.auth.record_failure();
        return ApiError::new(ErrorCode::Unauthorized, "That password is wrong.").no_challenge().into_response();
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
    let audit_key_set = state.db.lock().await.active_audit_key().ok().flatten().is_some();
    Json(serde_json::json!({ "password_set": stored_hash(&state).await.is_some(), "audit_key_set": audit_key_set }))
}

/// First run: the first visitor chooses the admin password and is signed in. Refused once one exists.
pub async fn setup_password(State(state): State<Arc<AdminState>>, headers: HeaderMap, ApiJson(body): ApiJson<Login>) -> Result<Response, Failure> {
    check_new_password(&body.password)?;
    let password = body.password;
    let hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not hash the password."))?;
    {
        let db = state.db.lock().await;
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
pub async fn change_password(State(state): State<Arc<AdminState>>, headers: HeaderMap, ApiJson(body): ApiJson<ChangePassword>) -> Result<StatusCode, Failure> {
    if state.auth.lockout_remaining().is_some() {
        return Err(ApiError::code(ErrorCode::TooManyAttempts).no_challenge());
    }
    let stored = stored_hash(&state).await.ok_or_else(|| err(StatusCode::CONFLICT, "No password is set."))?;
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
        let db = state.db.lock().await;
        db.conn()
            .execute("UPDATE admin_credentials SET password_hash = ?1, updated_at = ?2 WHERE id = 1", rusqlite::params![hash, crate::auth::now()])
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the password."))?;
    }
    state.auth.keep_only(session_cookie(&headers).as_deref());
    Ok(StatusCode::NO_CONTENT)
}
