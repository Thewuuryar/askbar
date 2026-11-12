//! How alerts are delivered, from the admin site (ADR 0055): the ntfy server, topic and access token, the email settings, a test
//! alert, and the state of delivery for the banner.
//!
//! The settings are write-only: they are stored encrypted with `admin.key`, no route returns one, and `GET /alerts` answers with
//! booleans. A change goes to the alerter first, which checks it; only if it takes the settings are they saved, and if it cannot be
//! reached nothing is changed (ADR 0028). Only a person's own session may call these routes.

use crate::alerter_link::{self, AlerterLink, Banner, PushError};
use crate::web::Failure;
use crate::AdminState;
use askbar_proto::alerter::{AlerterConfig, NtfyConfig, SmtpConfig, SmtpTls, TestResult};
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::ErrorCode;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

type St = State<Arc<AdminState>>;

fn alerter(state: &AdminState) -> Result<&Arc<AlerterLink>, Failure> {
    state.alerter.get().ok_or_else(|| ApiError::new(ErrorCode::ServerUnavailable, "No link to the alerter is set up."))
}

/// What `GET /alerts` says: which settings are held, never their values, and what the banner says.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct View {
    pub ntfy: NtfyView,
    pub email: EmailView,
    /// `ok`, `not_set_up`, `failing` or `unreachable`.
    pub banner: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NtfyView {
    pub server: bool,
    pub topic: bool,
    pub token: bool,
}

/// What `GET /alerts` says about email: which secrets are held, and the two settings that are not secret.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct EmailView {
    pub host: bool,
    pub port: Option<u16>,
    pub tls: Option<SmtpTls>,
    pub user: bool,
    pub password: bool,
    pub from: bool,
    pub to: bool,
}

/// Tells a field that is absent (keep what is stored) from one that is null (remove it).
fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Absent keeps the channel as it is, null removes it, an object replaces it.
    #[serde(default, deserialize_with = "present")]
    pub ntfy: Option<Option<NtfyInput>>,
    /// The same for email.
    #[serde(default, deserialize_with = "present")]
    pub email: Option<Option<EmailInput>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmailInput {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// Absent or null keeps the user name already stored, an empty string removes it, anything else replaces it.
    #[serde(default)]
    pub user: Option<String>,
    /// The same for the password.
    #[serde(default)]
    pub password: Option<String>,
    pub from: String,
    pub to: String,
}

impl std::fmt::Debug for EmailInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailInput").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtfyInput {
    pub server: String,
    pub topic: String,
    /// Absent or null keeps the token already stored, an empty string removes it, anything else replaces it.
    #[serde(default)]
    pub token: Option<String>,
}

impl std::fmt::Debug for NtfyInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NtfyInput").finish_non_exhaustive()
    }
}

/// The view, from `admin.db` and the link.
pub async fn view(state: &AdminState) -> View {
    let ntfy = {
        let db = state.db.lock().await;
        alerter_link::read_ntfy(db.conn(), &state.secrets)
    };
    let email = {
        let db = state.db.lock().await;
        alerter_link::read_email(db.conn(), &state.secrets)
    };
    let banner = match state.alerter.get() {
        Some(link) => link.banner(ntfy.is_some() || email.is_some(), crate::auth::now()),
        None => Banner::Unreachable,
    };
    View {
        email: EmailView {
            host: email.is_some(),
            port: email.as_ref().map(|m| m.port),
            tls: email.as_ref().map(|m| m.tls),
            user: email.as_ref().is_some_and(|m| m.user.is_some()),
            password: email.as_ref().is_some_and(|m| m.password.is_some()),
            from: email.is_some(),
            to: email.is_some(),
        },
        ntfy: NtfyView { server: ntfy.is_some(), topic: ntfy.is_some(), token: ntfy.as_ref().is_some_and(|n| n.token.is_some()) },
        banner: banner.as_str(),
    }
}

/// `GET /api/admin/v1/alerts`.
pub async fn get_alerts(State(state): St) -> Json<View> {
    Json(view(&state).await)
}

fn bad(detail: &str) -> Failure {
    ApiError::new(ErrorCode::BadRequest, detail)
}

/// A kept-or-replaced optional secret: absent or null keeps `stored`, an empty string removes it.
fn merge(given: Option<String>, stored: Option<String>) -> Option<String> {
    match given {
        None => stored,
        Some(v) if v.trim().is_empty() => None,
        Some(v) => Some(v.trim().to_string()),
    }
}

fn ntfy_from(input: NtfyInput, stored: Option<&NtfyConfig>) -> Result<NtfyConfig, Failure> {
    let (server, topic) = (input.server.trim().to_string(), input.topic.trim().to_string());
    if server.is_empty() || topic.is_empty() {
        return Err(bad("The ntfy server and the topic are both needed."));
    }
    if server.len() > 2048 || topic.len() > 64 {
        return Err(bad("The ntfy server or the topic is too long."));
    }
    Ok(NtfyConfig { server, topic, token: merge(input.token, stored.and_then(|s| s.token.clone())) })
}

fn email_from(input: EmailInput, stored: Option<&SmtpConfig>) -> Result<SmtpConfig, Failure> {
    let (host, from, to) = (input.host.trim().to_string(), input.from.trim().to_string(), input.to.trim().to_string());
    if host.is_empty() || from.is_empty() || to.is_empty() {
        return Err(bad("The mail server, the from address and the to address are all needed."));
    }
    if host.len() > 253 || from.len() > 254 || to.len() > 254 {
        return Err(bad("The mail server or an address is too long."));
    }
    Ok(SmtpConfig {
        host,
        port: input.port,
        tls: input.tls,
        user: merge(input.user, stored.and_then(|s| s.user.clone())),
        password: merge(input.password, stored.and_then(|s| s.password.clone())),
        from,
        to,
    })
}

/// `PUT /api/admin/v1/alerts`: the alerter takes the settings first (it checks them), then they are saved. A channel that is not
/// in the body stays as it is.
pub async fn put_alerts(State(state): St, ApiJson(input): ApiJson<Input>) -> Result<Json<View>, Failure> {
    let link = alerter(&state)?.clone();
    let (stored_ntfy, stored_email) = {
        let db = state.db.lock().await;
        (alerter_link::read_ntfy(db.conn(), &state.secrets), alerter_link::read_email(db.conn(), &state.secrets))
    };
    let ntfy = match input.ntfy {
        None => stored_ntfy,
        Some(None) => None,
        Some(Some(n)) => Some(ntfy_from(n, stored_ntfy.as_ref())?),
    };
    let email = match input.email {
        None => stored_email,
        Some(None) => None,
        Some(Some(m)) => Some(email_from(m, stored_email.as_ref())?),
    };
    let backup_public_key = {
        let db = state.db.lock().await;
        crate::db::active_backup_key(db.conn()).ok().flatten().map(|k| k.recipient)
    };
    let mut config = AlerterConfig { version: 0, ntfy, email, backup_public_key };
    config.version = alerter_link::version_of(&config);
    match link.put_config(&state, &config).await {
        Ok(()) => {}
        Err(PushError::Unreachable) => {
            return Err(ApiError::new(
                ErrorCode::ServerUnavailable,
                "The alerter did not answer. Check it on the Services page. Nothing was changed.",
            ))
        }
        // The alerter checks every value, and says only that it did not take them.
        Err(PushError::Refused(400)) => {
            return Err(bad(
                "The alerter did not accept those settings: check the ntfy server address, topic and token, or the mail server, port, TLS mode, login and addresses. Nothing was changed.",
            ))
        }
        Err(PushError::Refused(_)) => {
            return Err(ApiError::new(ErrorCode::ServerUnavailable, "The alerter did not take the settings. Nothing was changed."))
        }
        Err(PushError::NotRecorded(why)) => return Err(ApiError::new(ErrorCode::ServerUnavailable, why)),
    }
    {
        let db = state.db.lock().await;
        alerter_link::write_ntfy(db.conn(), &state.secrets, config.ntfy.as_ref())
            .and_then(|()| alerter_link::write_email(db.conn(), &state.secrets, config.email.as_ref()))
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The admin could not store the settings."))?;
    }
    link.mark_dirty();
    Ok(Json(view(&state).await))
}

/// `POST /api/admin/v1/alerts/test`: send a test alert through every channel that is set up, and say what each did.
pub async fn test_alerts(State(state): St) -> Result<Json<TestResult>, Failure> {
    Ok(Json(alerter(&state)?.test(&state).await?))
}
