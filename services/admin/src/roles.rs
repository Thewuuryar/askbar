//! The model pinned to each role (plan W4.2): `chat.manual`, `search.query` and `search.writer` each use one approved model.
//! The pins go to `model-gateway` in `PUT /config` (`role_pins`), which uses them for a request that names a role. A pin
//! whose model is no longer approved stays listed, marked not approved, and is not pushed. Each change is an audited admin
//! action (role and `provider/model`, no more), refused when the record cannot be appended.

use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::gateway::Role;
use askbar_proto::{CatalogEntry, ErrorCode};
use axum::{extract::Path, extract::State, http::StatusCode, Json};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Every role, in the order the page lists them.
const ROLES: [Role; 3] = [Role::ChatManual, Role::SearchQuery, Role::SearchWriter];

fn name(r: Role) -> String {
    serde_json::to_value(r).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

type St = State<Arc<AdminState>>;
fn internal<E>(_: E) -> Failure {
    err(StatusCode::INTERNAL_SERVER_ERROR, "The admin could not read or store the role pins.")
}

fn parse(role: &str) -> Result<Role, Failure> {
    serde_json::from_value(role.into()).map_err(|_| err(StatusCode::NOT_FOUND, "No such role."))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Serialize)]
pub struct RoleRow {
    pub role: String,
    pub pin: Option<Pin>,
    /// False when the pinned model is no longer approved: the gateway is not given the pin.
    pub approved: bool,
}

fn approved(conn: &Connection, p: &Pin) -> rusqlite::Result<bool> {
    Ok(crate::catalog::read(conn)?.models.contains(&CatalogEntry { provider_id: p.provider_id.clone(), model: p.model.clone() }))
}

fn saved(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Pin>> {
    conn.prepare("SELECT role, provider_id, model FROM role_pins")?
        .query_map([], |r| Ok((r.get(0)?, Pin { provider_id: r.get(1)?, model: r.get(2)? })))?
        .collect()
}

/// The pins the gateway is given, `provider/model` per role: only those whose model is still approved.
pub fn pushed(conn: &Connection) -> rusqlite::Result<BTreeMap<Role, String>> {
    let mut out = BTreeMap::new();
    for (role, pin) in saved(conn)? {
        if let (Ok(role), true) = (parse(&role), approved(conn, &pin)?) {
            out.insert(role, askbar_proto::model_id(&pin.provider_id, &pin.model));
        }
    }
    Ok(out)
}

/// `GET /api/admin/v1/roles`: the three roles, each with its pin or none.
pub async fn list(State(state): St) -> Result<Json<Vec<RoleRow>>, Failure> {
    let db = state.db.lock().await;
    let conn = db.conn();
    let mut pins = saved(conn).map_err(internal)?;
    let mut rows = Vec::new();
    for r in ROLES {
        let pin = pins.remove(&name(r));
        let approved = match &pin {
            Some(p) => approved(conn, p).map_err(internal)?,
            None => false,
        };
        rows.push(RoleRow { role: name(r), pin, approved });
    }
    Ok(Json(rows))
}

/// `PUT /api/admin/v1/roles/{role}`: pin an approved model to the role.
pub async fn put(
    State(state): St,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    Path(role): Path<String>,
    askbar_proto::http::ApiJson(pin): askbar_proto::http::ApiJson<Pin>,
) -> Result<Json<Pin>, Failure> {
    let r = parse(&role)?;
    if let Some(axum::Extension(n)) = &notes {
        n.add("role", name(r).into());
        n.add("model", askbar_proto::model_id(&pin.provider_id, &pin.model).into());
    }
    let db = state.db.lock().await;
    let conn = db.conn();
    if !approved(conn, &pin).map_err(internal)? {
        return Err(askbar_proto::http::ApiError::new(
            ErrorCode::ModelNotApproved,
            "That model is not approved. Approve it under Providers first.",
        ));
    }
    conn.execute(
        "INSERT INTO role_pins (role, provider_id, model) VALUES (?1, ?2, ?3)
         ON CONFLICT(role) DO UPDATE SET provider_id = excluded.provider_id, model = excluded.model",
        params![name(r), pin.provider_id, pin.model],
    )
    .map_err(internal)?;
    drop(db);
    if let Some(g) = state.gateway.get() {
        g.mark_dirty();
    }
    Ok(Json(pin))
}

/// `DELETE /api/admin/v1/roles/{role}`: the role has no pin again.
pub async fn delete(
    State(state): St,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    Path(role): Path<String>,
) -> Result<StatusCode, Failure> {
    let r = parse(&role)?;
    if let Some(axum::Extension(n)) = &notes {
        n.add("role", name(r).into());
    }
    let gone = state.db.lock().await.conn().execute("DELETE FROM role_pins WHERE role = ?1", [name(r)]).map_err(internal)?;
    if gone == 0 {
        return Err(err(StatusCode::NOT_FOUND, "That role has no pin."));
    }
    if let Some(g) = state.gateway.get() {
        g.mark_dirty();
    }
    Ok(StatusCode::NO_CONTENT)
}
