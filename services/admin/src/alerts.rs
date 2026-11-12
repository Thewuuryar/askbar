//! How alerts are delivered, from the admin site (ADR 0055): the ntfy server, topic and access token, a test alert, and the state
//! of delivery for the banner.
//!
//! The settings are write-only: they are stored encrypted with `admin.key`, no route returns one, and `GET /alerts` answers with
//! booleans. A change goes to the alerter first, which checks it; only if it takes the settings are they saved, and if it cannot be
//! reached nothing is changed (ADR 0028). Only a person's own session may call these routes.

use crate::alerter_link::{self, AlerterLink, Banner, PushError};
use crate::web::Failure;
use crate::AdminState;
use askbar_proto::alerter::{AlerterConfig, NtfyConfig, TestResult};
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
    /// `ok`, `not_set_up`, `failing` or `unreachable`.
    pub banner: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NtfyView {
    pub server: bool,
    pub topic: bool,
    pub token: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// The channel, or null to remove it.
    #[serde(default)]
    pub ntfy: Option<NtfyInput>,
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
    let banner = match state.alerter.get() {
        Some(link) => link.banner(ntfy.is_some(), crate::auth::now()),
        None => Banner::Unreachable,
    };
    View {
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

/// `PUT /api/admin/v1/alerts`: the alerter takes the settings first (it checks them), then they are saved.
pub async fn put_alerts(State(state): St, ApiJson(input): ApiJson<Input>) -> Result<Json<View>, Failure> {
    let link = alerter(&state)?.clone();
    let stored = {
        let db = state.db.lock().await;
        alerter_link::read_ntfy(db.conn(), &state.secrets)
    };
    let ntfy = match input.ntfy {
        None => None,
        Some(n) => {
            let (server, topic) = (n.server.trim().to_string(), n.topic.trim().to_string());
            if server.is_empty() || topic.is_empty() {
                return Err(bad("The ntfy server and the topic are both needed."));
            }
            if server.len() > 2048 || topic.len() > 64 {
                return Err(bad("The ntfy server or the topic is too long."));
            }
            let token = match n.token {
                None => stored.and_then(|s| s.token),
                Some(t) if t.trim().is_empty() => None,
                Some(t) => Some(t.trim().to_string()),
            };
            Some(NtfyConfig { server, topic, token })
        }
    };
    let mut config = AlerterConfig { version: 0, ntfy };
    config.version = alerter_link::version_of(&config);
    match link.put_config(&state, &config).await {
        Ok(()) => {}
        Err(PushError::Unreachable) => {
            return Err(ApiError::new(
                ErrorCode::ServerUnavailable,
                "The alerter did not answer. Check it on the Services page. Nothing was changed.",
            ))
        }
        // The alerter checks the server address, the topic and the token, and says only that it did not take them.
        Err(PushError::Refused(400)) => {
            return Err(bad(
                "The alerter did not accept those settings: check the ntfy server address, the topic and the token. Nothing was changed.",
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
            .map_err(|_| ApiError::new(ErrorCode::Internal, "The admin could not store the settings."))?;
    }
    link.mark_dirty();
    Ok(Json(view(&state).await))
}

/// `POST /api/admin/v1/alerts/test`: send a test alert through every channel that is set up, and say what each did.
pub async fn test_alerts(State(state): St) -> Result<Json<TestResult>, Failure> {
    Ok(Json(alerter(&state)?.test(&state).await?))
}
