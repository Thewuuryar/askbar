//! The record of what the admin does (ADR 0010, plan step 2.7): every change an administrator makes is appended to the
//! auditor, with the admin's own append token, **before** it is made. An action whose record cannot be appended is
//! refused with a sentence and nothing is changed. How it ended (`done`, `refused` or `failed`) is appended afterwards,
//! naming the request.
//!
//! One layer on the routes does it, so a new route cannot slip past: [`classify`] names the category and action of every
//! route that changes something, and a test fails for a route that has none. A record holds the route, who asked (the
//! session or an API key), the ids in the path and, if the handler adds them, a few facts about the change (never a body:
//! it could hold a secret). The caller recorded is the token's, never ours.
//!
//! The admin's records are claims. The services record what they applied themselves (`config` / `applied`), and a
//! mismatch alert comes later (ADR 0010).

use crate::web::err;
use crate::AdminState;
use askbar_proto::audit::AdminActionStart;
use axum::extract::{MatchedPath, RawPathParams, Request, State};
use axum::http::{header, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The link's name, as `bootstrap` names it.
pub const APPEND_LINK: &str = "admin-auditor-append";

pub fn token_file(token_dir: &Path) -> PathBuf {
    token_dir.join("call").join(format!("{APPEND_LINK}.token"))
}

/// The append token `bootstrap` made. A missing or empty file means `bootstrap` has not run.
pub fn append_token(token_dir: &Path) -> std::io::Result<String> {
    let path = token_file(token_dir);
    let missing = |why: &str| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} {why}; run `docker compose up -d`, which runs bootstrap first", path.display()),
        )
    };
    match std::fs::read_to_string(&path) {
        Ok(t) if !t.trim().is_empty() => Ok(t.trim().to_string()),
        Ok(_) => Err(missing("is empty")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(missing("is missing")),
        Err(e) => Err(e),
    }
}

/// Why a record could not be appended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotRecorded {
    /// The auditor did not answer.
    Unreachable,
    /// The auditor is behind or full (`503 audit_backlog`).
    Backlog,
    /// The auditor does not take this token, or this record.
    Rejected,
}

impl NotRecorded {
    /// What the person is told. Nothing was changed.
    pub fn sentence(self) -> &'static str {
        match self {
            NotRecorded::Unreachable => "The auditor cannot be reached, and every change is recorded first, so nothing was changed.",
            NotRecorded::Backlog => "The auditor is not taking records right now (it is full or behind), and every change is recorded first, so nothing was changed.",
            NotRecorded::Rejected => "The auditor does not accept this admin's record token, and every change is recorded first, so nothing was changed.",
        }
    }
}

/// The admin's way of appending its action records.
pub struct ActionLog {
    http: reqwest::Client,
    base: String,
    token: String,
    token_watch: crate::notify::TokenWatch,
}

impl ActionLog {
    pub fn new(base: &str, token: String) -> Arc<ActionLog> {
        Arc::new(ActionLog {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token,
            token_watch: crate::notify::TokenWatch::new(APPEND_LINK),
        })
    }

    /// Where an alert about this link's token goes (set once by `main`).
    pub fn set_notifier(&self, n: Arc<crate::notify::Notifier>) {
        self.token_watch.set_notifier(n);
    }

    /// Tell the auditor which process this is (`POST /hello`), until it takes it. The auditor counts the admin's restarts from
    /// these (three within ten minutes is a `restart_loop` alert, ADR 0051); the admin has no span to end, so it learns nothing.
    /// Runs in the background and gives up only when it succeeds.
    pub async fn hello_until_done(self: Arc<Self>, boot_id: String) {
        let url = format!("{}{}/hello", self.base, askbar_proto::paths::AUDIT_V1);
        let mut wait = Duration::from_millis(500);
        loop {
            let sent = self.http.post(&url).bearer_auth(&self.token).json(&json!({ "boot_id": boot_id })).send().await;
            if let Ok(resp) = sent {
                self.token_watch.saw(resp.status().as_u16());
                if resp.status().is_success() {
                    return;
                }
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(30));
        }
    }

    /// `POST /admin-actions`. `Ok` once the auditor has the record committed.
    pub async fn record(&self, r: &AdminActionStart) -> Result<(), NotRecorded> {
        let url = format!("{}{}/admin-actions", self.base, askbar_proto::paths::AUDIT_V1);
        let sent = self.http.post(url).bearer_auth(&self.token).json(r).send().await;
        if let Ok(resp) = &sent {
            self.token_watch.saw(resp.status().as_u16());
        }
        match sent {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) if resp.status() == StatusCode::SERVICE_UNAVAILABLE || resp.status() == StatusCode::TOO_MANY_REQUESTS => {
                Err(NotRecorded::Backlog)
            }
            Ok(_) => Err(NotRecorded::Rejected),
            Err(_) => Err(NotRecorded::Unreachable),
        }
    }

    /// An action the admin takes itself, with no request to hang it on (a configuration push, a reset): a `requested`
    /// record first, `f` only if it was appended, then `done` or `failed` (best effort). `Err` means nothing was done.
    /// `extra` adds to the end record what the result shows (the version a push was given, say).
    pub async fn around<T, E, F>(
        &self,
        category: &str,
        action: &str,
        detail: Value,
        f: F,
        extra: impl FnOnce(&Result<T, E>) -> Value,
    ) -> Result<Result<T, E>, NotRecorded>
    where
        F: std::future::Future<Output = Result<T, E>>,
    {
        let request = ulid::Ulid::new().to_string();
        let start = |id: &str, outcome: &str, detail: Value| AdminActionStart {
            id: id.to_string(),
            at: crate::auth::now(),
            category: category.into(),
            action: action.into(),
            outcome: outcome.into(),
            detail: Some(detail),
        };
        self.record(&start(&request, "requested", detail.clone())).await?;
        let result = f.await;
        let mut end = detail;
        if let Value::Object(m) = &mut end {
            m.insert("request".into(), json!(request));
            if let Value::Object(more) = extra(&result) {
                m.extend(more);
            }
        }
        let outcome = if result.is_ok() { "done" } else { "failed" };
        if let Err(why) = self.record(&start(&ulid::Ulid::new().to_string(), outcome, end)).await {
            tracing::warn!(category, action, ?why, "the end of an admin action could not be recorded");
        }
        Ok(result)
    }
}

/// Facts a handler adds to the record of its own change: the new retention, say. Never a secret or message content.
#[derive(Clone, Default)]
pub struct ActionNotes(Arc<Mutex<Map<String, Value>>>);

impl ActionNotes {
    pub fn add(&self, key: &str, value: Value) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(key.to_string(), value);
    }

    fn take(&self) -> Map<String, Value> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// The category and action of a route that changes something, or `None` for one that does not. Reading is not recorded,
/// except the backup (which hands over every record the admin holds) and the content of a sealed span (ADR 0011 rule 12:
/// each item viewed). Signing in and out are not changes, and the
/// first goes through its own route that this layer is never put on.
pub fn classify(method: &Method, template: &str) -> Option<(&'static str, &'static str)> {
    let v1 = askbar_proto::paths::ADMIN_V1;
    let rest = template.strip_prefix(v1)?;
    Some(match (method.as_str(), rest) {
        ("POST", "/providers") => ("providers", "create"),
        ("PUT", "/providers/{id}") => ("providers", "update"),
        ("DELETE", "/providers/{id}") => ("providers", "delete"),
        ("POST", "/providers/{id}/test") => ("provider_test", "test"),
        ("POST", "/providers/models") => ("provider_test", "list_models"),
        ("PUT", "/catalog") => ("providers", "catalog_update"),
        ("POST", "/clients/{id}/revoke") => ("clients", "revoke"),
        ("POST", "/users") => ("users", "create"),
        ("PUT", "/users/{id}/display-defaults") => ("users", "display_defaults"),
        ("POST", "/users/{id}/pairing-keys") => ("pairing_keys", "create"),
        ("DELETE", "/users/{id}") => ("users", "delete"),
        ("POST", "/users/{id}/pause") => ("users", "pause"),
        ("POST", "/users/{id}/resume") => ("users", "resume"),
        ("PUT", "/setup/audit-key") => ("secret", "audit_key_setup"),
        ("POST", "/audit/rotate-key") => ("secret", "audit_key_rotate"),
        ("POST", "/audit/session") => ("sealed_content", "unseal"),
        ("DELETE", "/audit/session") => ("sealed_content", "seal"),
        ("GET", "/audit/payloads/{span_id}") => ("sealed_content", "view"),
        ("PUT", "/settings") => ("settings", "update"),
        ("PUT", "/network") => ("network", "update"),
        ("POST", "/network/apply") => ("config_push", "network_apply"),
        ("PUT", "/password") => ("secret", "admin_password_change"),
        ("PUT", "/alerts") => ("alerts", "update"),
        ("POST", "/alerts/test") => ("alerts", "test"),
        ("POST", "/cpa/logins") => ("providers", "cpa_login_start"),
        ("POST", "/cpa/logins/{state}/callback") => ("providers", "cpa_login_finish"),
        ("DELETE", "/cpa/accounts/{name}") => ("providers", "cpa_account_remove"),
        ("POST", "/services/{name}/restart") => ("restart", "restart"),
        ("POST", "/keys") => ("secret", "api_key_create"),
        ("DELETE", "/keys/{id}") => ("secret", "api_key_revoke"),
        ("GET", "/backup") => ("backup", "download"),
        _ => return None,
    })
}

/// The path parameters worth recording: ids and names. Not `state`: an OAuth state is a secret for a few minutes.
const RECORDED_PARAMS: [&str; 3] = ["id", "name", "span_id"];

/// The layer: record the request, refuse if it cannot be recorded, run the handler, record how it ended.
pub async fn record_changes(
    State(state): State<Arc<AdminState>>,
    matched: MatchedPath,
    params: RawPathParams,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(log) = state.actions.get().cloned() else { return next.run(req).await };
    let Some((category, action)) = classify(req.method(), matched.as_str()) else { return next.run(req).await };
    let by = if req.headers().contains_key(header::AUTHORIZATION) { "api_key" } else { "session" };
    let mut ids = Map::new();
    for (k, v) in &params {
        if RECORDED_PARAMS.contains(&k) {
            ids.insert(k.to_string(), json!(v));
        }
    }
    let request_id = ulid::Ulid::new().to_string();
    let detail = json!({ "route": matched.as_str(), "method": req.method().as_str(), "by": by, "params": ids });
    let asked = AdminActionStart {
        id: request_id.clone(),
        at: crate::auth::now(),
        category: category.into(),
        action: action.into(),
        outcome: "requested".into(),
        detail: Some(detail),
    };
    if let Err(why) = log.record(&asked).await {
        tracing::warn!(category, action, ?why, "an admin action was refused: its record could not be appended");
        return err(StatusCode::SERVICE_UNAVAILABLE, why.sentence()).into_response();
    }
    let notes = ActionNotes::default();
    req.extensions_mut().insert(notes.clone());
    let resp = next.run(req).await;
    let status = resp.status();
    let outcome = if status.is_server_error() {
        "failed"
    } else if status.is_client_error() {
        "refused"
    } else {
        "done"
    };
    let mut end = notes.take();
    end.insert("request".into(), json!(request_id));
    end.insert("status".into(), json!(status.as_u16()));
    let ended = AdminActionStart {
        id: ulid::Ulid::new().to_string(),
        at: crate::auth::now(),
        category: category.into(),
        action: action.into(),
        outcome: outcome.into(),
        detail: Some(Value::Object(end)),
    };
    // The change is made: if the end cannot be recorded, the request is on record and says so by having no end.
    if let Err(why) = log.record(&ended).await {
        tracing::warn!(category, action, ?why, "the end of an admin action could not be recorded");
    }
    resp
}
