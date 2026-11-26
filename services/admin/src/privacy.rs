//! The PII settings (plan PII-P3.1): one JSON row in `settings` (key `pii`), the default while absent. The allow and deny terms
//! are personal data: they stay in `admin.db` and go only to the gateway and the front, never into a record, log or error.
//! Audited as `privacy/set` with counts only, refused when the record cannot be appended.

use crate::web::Failure;
use crate::AdminState;
use askbar_proto::http::{ApiError, ApiJson};
use askbar_proto::pii::{PiiAction, PiiConfig, CUSTOM, PII_TYPES};
use askbar_proto::ErrorCode;
use axum::{extract::State, Json};
use rusqlite::Connection;
use serde::Serialize;
use serde_json::json;
use std::sync::Arc;

const KEY: &str = "pii";

/// The stored settings, or the default when none are stored or the row does not read.
pub fn read(conn: &Connection) -> PiiConfig {
    let stored = conn.query_row("SELECT value FROM settings WHERE key = ?1", [KEY], |r| r.get::<_, String>(0)).ok();
    stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[derive(Serialize)]
pub struct TypeRow {
    name: &'static str,
    default: PiiAction,
}

#[derive(Serialize)]
pub struct PrivacyView {
    #[serde(flatten)]
    config: PiiConfig,
    /// Each configurable type with the action it takes when `entities` leaves it out.
    types: Vec<TypeRow>,
}

fn view(conn: &Connection) -> PrivacyView {
    let d = PiiConfig::default();
    let types = PII_TYPES.iter().filter(|t| **t != CUSTOM).map(|t| TypeRow { name: t, default: d.action_for(t) }).collect();
    PrivacyView { config: read(conn), types }
}

/// `GET /api/admin/v1/privacy`.
pub async fn get(State(state): State<Arc<AdminState>>) -> Json<PrivacyView> {
    Json(view(state.db.lock().await.conn()))
}

/// `PUT /api/admin/v1/privacy`: the whole settings. A refusal names the problem, never a term.
pub async fn put(
    State(state): State<Arc<AdminState>>,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    ApiJson(body): ApiJson<PiiConfig>,
) -> Result<Json<PrivacyView>, Failure> {
    body.validate().map_err(|why| ApiError::new(ErrorCode::BadRequest, why))?;
    if let Some(axum::Extension(n)) = &notes {
        n.add("enabled", json!(body.enabled));
        n.add("mode", json!(body.mode));
        n.add("entity_overrides", json!(body.entities.len()));
        n.add("allow_terms", json!(body.allow_terms.len()));
        n.add("deny_terms", json!(body.deny_terms.len()));
    }
    let db = state.db.lock().await;
    let json = serde_json::to_string(&body).map_err(|_| internal())?;
    db.conn()
        .execute("INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [KEY, &json])
        .map_err(|_| internal())?;
    if let Some(g) = state.gateway.get() {
        g.mark_dirty();
    }
    if let Some(m) = state.mcp.get() {
        m.mark_dirty();
    }
    Ok(Json(view(db.conn())))
}

fn internal() -> Failure {
    crate::web::err(axum::http::StatusCode::INTERNAL_SERVER_ERROR, "The admin could not store the privacy settings.")
}
