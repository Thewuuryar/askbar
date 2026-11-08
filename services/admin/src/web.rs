//! Admin site authentication: a password, an HttpOnly session cookie, and a required
//! header on anything that changes state. Only the cookie counts; `Authorization` is
//! ignored, so no token of any kind opens the admin.

use crate::{auth::SESSION_TTL_SECS, AdminState};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::problem::ErrorCode;
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

/// The session cookie (ADR 0029): `__Secure-` makes a browser accept it only with `Secure` from a
/// secure origin, and `Path=/api/admin` sends it to the admin API and nowhere else on the host.
pub const COOKIE_NAME: &str = "__Secure-askbar_admin";
pub const CSRF_HEADER: &str = "x-askbar-admin";
pub const MIN_PASSWORD_CHARS: usize = 10;
/// The one admin account's name (ADR 0007: v1 has one admin account).
pub const ADMIN_USERNAME: &str = "admin";
/// The published first-run password (ADR 0007). It works until it is changed, and nothing but the
/// change is allowed while it is in use. Also what `askbar-admin reset` restores.
pub const DEFAULT_PASSWORD: &str = "admin";
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

/// First start: store the published password, marked as needing a change. Does nothing once any
/// password exists. Returns true if it stored one.
pub fn ensure_default_credentials(db: &crate::AdminDb) -> rusqlite::Result<bool> {
    let hash = hash_password(DEFAULT_PASSWORD).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let n = db.conn().execute(
        "INSERT OR IGNORE INTO admin_credentials (id, password_hash, updated_at, must_change) VALUES (1, ?1, ?2, 1)",
        rusqlite::params![hash, crate::auth::now()],
    )?;
    Ok(n == 1)
}

/// `askbar-admin reset` (a host command, ADR 0007): put the published password back, marked as needing a
/// change, and record the reset in the admin's action log. Sessions of a running admin live in its memory;
/// restart the admin afterwards to end them.
pub fn reset_to_default(db: &crate::AdminDb) -> rusqlite::Result<()> {
    let hash = hash_password(DEFAULT_PASSWORD).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let now = crate::auth::now();
    db.conn().execute(
        "INSERT INTO admin_credentials (id, password_hash, updated_at, must_change) VALUES (1, ?1, ?2, 1)
         ON CONFLICT(id) DO UPDATE SET password_hash = excluded.password_hash, updated_at = excluded.updated_at, must_change = 1",
        rusqlite::params![hash, now],
    )?;
    db.conn().execute("INSERT INTO service_actions (service, action, at, ok) VALUES ('admin', 'reset', ?1, 1)", [now])?;
    Ok(())
}

/// True while the published password is still the admin password.
pub async fn password_change_required(state: &AdminState) -> bool {
    let db = state.db.lock().await;
    db.conn().query_row("SELECT must_change FROM admin_credentials WHERE id = 1", [], |r| r.get::<_, i64>(0)).optional().ok().flatten()
        == Some(1)
}

/// While the published password is in use, only these signed-in routes answer: signing out, checking
/// the session, and changing the password.
fn allowed_before_change(req: &Request) -> bool {
    let path = req.uri().path();
    let (session, password) = (format!("{}/session", askbar_proto::paths::ADMIN_V1), format!("{}/password", askbar_proto::paths::ADMIN_V1));
    (path == session && matches!(*req.method(), Method::GET | Method::DELETE)) || (path == password && *req.method() == Method::PUT)
}

fn change_first() -> ApiError {
    ApiError::code(ErrorCode::PasswordChangeRequired).no_challenge()
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').find_map(|kv| {
        let (k, v) = kv.trim().split_once('=')?;
        (k == COOKIE_NAME && !v.is_empty()).then(|| v.to_string())
    })
}

fn set_cookie(value: &str, max_age: i64) -> HeaderValue {
    // Always `Secure`: the site is served over HTTPS from the first start (ADR 0034), and a browser
    // also treats http://localhost and 127.0.0.1 as secure, which is what the tests use.
    let c = format!("{COOKIE_NAME}={value}; Secure; HttpOnly; SameSite=Strict; Path={}; Max-Age={max_age}", askbar_proto::paths::ADMIN_API);
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

/// A change made from a browser must come from the admin site itself (ADR 0029):
/// - `Sec-Fetch-Site`, when the browser sends it, must be `same-origin` (or `none`, typed by the
///   person). A `cross-site` or `same-site` change is refused, whatever else it carries.
/// - It must carry `X-Askbar-Admin: 1`. A cross-site form or simple request can't add custom
///   headers, and cross-origin scripts need a CORS preflight we never approve.
///
/// Reads need neither. (An API key is not a cookie, so a request made with one skips this: see
/// `require_admin_auth`.)
fn browser_change_refused(req: &Request) -> Option<ApiError> {
    if matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS) {
        return None;
    }
    let site = req.headers().get("sec-fetch-site").map(|v| v.to_str().unwrap_or(""));
    if site.is_some_and(|s| !matches!(s, "same-origin" | "none")) {
        return Some(ApiError::new(ErrorCode::Forbidden, "Changes are accepted only from the admin site itself.").no_challenge());
    }
    if req.headers().get(CSRF_HEADER).and_then(|v| v.to_str().ok()) != Some("1") {
        return Some(ApiError::new(ErrorCode::Forbidden, "A change needs the header `X-Askbar-Admin: 1`.").no_challenge());
    }
    None
}

/// For the open routes that change things (signing in, first-run setup).
pub async fn require_admin_header(req: Request, next: Next) -> Result<Response, ApiError> {
    if let Some(e) = browser_change_refused(&req) {
        return Err(e);
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
    // Pages and the API are never stored. A static file sets its own `no-cache` with an `ETag` (pages.rs).
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
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
    if let Some(e) = browser_change_refused(&req) {
        return Err(e);
    }
    if !allowed_before_change(&req) && password_change_required(&state).await {
        return Err(change_first());
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
    let key = value
        .to_str()
        .ok()
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, k)| k.trim().to_string());
    let scope = match key {
        Some(k) => {
            let db = state.db.lock().await;
            api_key::authenticate(db.conn(), &k, crate::auth::now()).map_err(|_| ApiError::code(ErrorCode::Internal))?
        }
        None => None,
    };
    match scope {
        // (A key can only be made after the password was changed, and a reset puts the published one back.)
        Some(_) if password_change_required(&state).await => Err(change_first()),
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
    let wait = secs.div_ceil(60).max(1);
    ApiError::new(
        ErrorCode::TooManyAttempts,
        format!("Too many wrong passwords or keys. Try again in {wait} minute{}.", if wait == 1 { "" } else { "s" }),
    )
    .retry_after(secs)
    .no_challenge()
}

// -------------------------------------------------------------------- handlers

#[derive(Deserialize)]
pub struct Login {
    /// Optional; when given it must be `admin`. A wrong name and a wrong password get the same answer.
    #[serde(default)]
    username: Option<String>,
    password: String,
}

pub async fn login(State(state): State<Arc<AdminState>>, ApiJson(body): ApiJson<Login>) -> Response {
    if let Some(secs) = state.auth.lockout_remaining() {
        return too_many_attempts(secs).into_response();
    }
    let Some(stored) = stored_hash(&state).await else {
        // No password yet: the setup page has to create one first.
        return ApiError::new(ErrorCode::SetupRequired, "Set the admin password first.").no_challenge().into_response();
    };
    let name_ok = body.username.as_deref().is_none_or(|u| u.trim() == ADMIN_USERNAME);
    // The hash is checked whatever the name, so a wrong name takes as long as a wrong password.
    let password_ok = verify(stored, body.password).await;
    if !(name_ok && password_ok) {
        state.auth.record_failure();
        return ApiError::new(ErrorCode::Unauthorized, "That username or password is wrong.").no_challenge().into_response();
    }
    state.auth.clear_failures();
    let token = state.auth.new_session();
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie(&token, SESSION_TTL_SECS));
    r
}

pub async fn logout(State(state): State<Arc<AdminState>>, headers: HeaderMap) -> Response {
    if let Some(t) = session_cookie(&headers) {
        state.auth.end_session(&t);
    }
    let mut r = StatusCode::NO_CONTENT.into_response();
    r.headers_mut().insert(header::SET_COOKIE, set_cookie("", 0));
    r
}

/// What the setup page needs to decide which screen to show. Booleans only.
pub async fn setup_status(State(state): State<Arc<AdminState>>) -> Json<serde_json::Value> {
    let audit_key_set = state.db.lock().await.active_audit_key().ok().flatten().is_some();
    Json(serde_json::json!({ "password_change_required": password_change_required(&state).await, "audit_key_set": audit_key_set }))
}

#[derive(Deserialize)]
pub struct ChangePassword {
    current: String,
    new: String,
}

/// Change the password. The current one is required; every other session is signed out.
pub async fn change_password(
    State(state): State<Arc<AdminState>>,
    headers: HeaderMap,
    ApiJson(body): ApiJson<ChangePassword>,
) -> Result<StatusCode, Failure> {
    if state.auth.lockout_remaining().is_some() {
        return Err(ApiError::code(ErrorCode::TooManyAttempts).no_challenge());
    }
    let stored = stored_hash(&state).await.ok_or_else(|| err(StatusCode::CONFLICT, "No password is set."))?;
    if !verify(stored, body.current.clone()).await {
        state.auth.record_failure();
        // 403, not 401: the page treats 401 as "signed out".
        return Err(err(StatusCode::FORBIDDEN, "The current password is wrong."));
    }
    state.auth.clear_failures();
    check_new_password(&body.new)?;
    if body.new == DEFAULT_PASSWORD || body.new.trim().eq_ignore_ascii_case(ADMIN_USERNAME) || body.new == body.current {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "Choose a new password: not the published one, not the username, and not the current one.",
        ));
    }
    let new = body.new;
    let hash = tokio::task::spawn_blocking(move || hash_password(&new))
        .await
        .ok()
        .and_then(Result::ok)
        .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not hash the password."))?;
    {
        let db = state.db.lock().await;
        db.conn()
            .execute(
                "UPDATE admin_credentials SET password_hash = ?1, updated_at = ?2, must_change = 0 WHERE id = 1",
                rusqlite::params![hash, crate::auth::now()],
            )
            .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the password."))?;
    }
    state.auth.keep_only(session_cookie(&headers).as_deref());
    Ok(StatusCode::NO_CONTENT)
}
