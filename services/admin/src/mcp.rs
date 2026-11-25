//! Per-user permissions for the built-in MCP tools (ADR 0019, plan step P5A.17). Off by default: a user with no row holds
//! nothing. Each change is an audited admin action (user id and counts only), refused when the record cannot be appended.
//! [`permissions_config`] is the shape `mcp-gateway`'s `PUT /config` takes; the push itself is P5A.18.

use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::mcp::{MAX_PERMISSION_USERS, TOOLS};
use axum::{extract::Path, extract::State, http::StatusCode, Json};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserTools {
    #[serde(default)]
    pub allowed: Vec<String>,
    #[serde(default)]
    pub denied: Vec<String>,
}

/// User ID to the tools set for that user; a user with no row is absent.
pub fn permissions_config(conn: &Connection) -> rusqlite::Result<BTreeMap<String, UserTools>> {
    let mut out: BTreeMap<String, UserTools> = BTreeMap::new();
    let mut st = conn.prepare("SELECT user_id, tool, state FROM mcp_permissions ORDER BY user_id, tool")?;
    for row in st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        let (user, tool, state) = row?;
        let u = out.entry(user).or_default();
        if state == "allow" { &mut u.allowed } else { &mut u.denied }.push(tool);
    }
    Ok(out)
}

type St = State<Arc<AdminState>>;
fn internal(_: rusqlite::Error) -> Failure {
    err(StatusCode::INTERNAL_SERVER_ERROR, "The admin could not read or store the permissions.")
}

/// `GET /api/admin/v1/mcp/permissions`.
pub async fn list(State(state): St) -> Result<Json<BTreeMap<String, UserTools>>, Failure> {
    permissions_config(state.db.lock().await.conn()).map(Json).map_err(internal)
}

/// `PUT /api/admin/v1/mcp/permissions/{user}`: replace that user's lists (both empty is the same as none).
pub async fn put(
    State(state): St,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    Path(user): Path<String>,
    askbar_proto::http::ApiJson(mut t): askbar_proto::http::ApiJson<UserTools>,
) -> Result<Json<UserTools>, Failure> {
    for l in [&mut t.allowed, &mut t.denied] {
        l.sort();
        l.dedup();
    }
    if t.allowed.iter().chain(&t.denied).any(|x| !TOOLS.contains(&x.as_str())) {
        return Err(err(StatusCode::BAD_REQUEST, "There is no such tool."));
    }
    if t.allowed.iter().any(|x| t.denied.contains(x)) {
        return Err(err(StatusCode::BAD_REQUEST, "A tool cannot be both allowed and denied."));
    }
    if let Some(axum::Extension(n)) = &notes {
        n.add("user", user.clone().into());
        n.add("allowed", t.allowed.len().into());
        n.add("denied", t.denied.len().into());
    }
    let db = state.db.lock().await;
    let conn = db.conn();
    let known = conn.query_row("SELECT 1 FROM users WHERE id = ?1", [&user], |_| Ok(())).is_ok();
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such user."));
    }
    let tx = conn.unchecked_transaction().map_err(internal)?;
    tx.execute("DELETE FROM mcp_permissions WHERE user_id = ?1", [&user]).map_err(internal)?;
    for (list, s) in [(&t.allowed, "allow"), (&t.denied, "deny")] {
        for tool in list {
            tx.execute("INSERT INTO mcp_permissions (user_id, tool, state) VALUES (?1, ?2, ?3)", params![user, tool, s])
                .map_err(internal)?;
        }
    }
    let users: i64 = tx.query_row("SELECT COUNT(DISTINCT user_id) FROM mcp_permissions", [], |r| r.get(0)).map_err(internal)?;
    if users > MAX_PERMISSION_USERS as i64 {
        return Err(err(StatusCode::BAD_REQUEST, "Too many users have permissions."));
    }
    tx.commit().map_err(internal)?;
    Ok(Json(t))
}

/// `DELETE /api/admin/v1/mcp/permissions/{user}`: the user holds nothing again.
pub async fn delete(
    State(state): St,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    Path(user): Path<String>,
) -> Result<StatusCode, Failure> {
    if let Some(axum::Extension(n)) = &notes {
        n.add("user", user.clone().into());
    }
    let gone = state.db.lock().await.conn().execute("DELETE FROM mcp_permissions WHERE user_id = ?1", [&user]).map_err(internal)?;
    if gone == 0 {
        return Err(err(StatusCode::NOT_FOUND, "That user has no permissions set."));
    }
    Ok(StatusCode::NO_CONTENT)
}
