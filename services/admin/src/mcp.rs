//! Per-user permissions for the built-in MCP tools (ADR 0019, plan step P5A.17). Off by default: a user with no row holds
//! nothing. Each change is an audited admin action (user id and counts only), refused when the record cannot be appended.
//! [`permissions_config`] is the shape `mcp-gateway`'s `PUT /config` takes; pushed by `mcp_link`.

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
/// The front gets the new permissions now, not at its next check.
fn mark_dirty(state: &AdminState) {
    if let Some(l) = state.mcp.get() {
        l.mark_dirty();
    }
}
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
    mark_dirty(&state);
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
    mark_dirty(&state);
    Ok(StatusCode::NO_CONTENT)
}

/// What the page says of `searxng`, from the front's last search (the admin cannot reach `searxng`): `no_search_yet`, `ok`, `down`
/// (the last search failed) or `unknown` (the front does not answer, so its last word may be stale). One poll after a search fails.
fn searxng(online: bool, last: Option<crate::mcp_link::LastSearch>) -> serde_json::Value {
    let state = match &last {
        _ if !online => "unknown",
        None => "no_search_yet",
        Some(l) if l.outcome == "ok" => "ok",
        Some(_) => "down",
    };
    serde_json::json!({ "state": state, "last_at": last.as_ref().map(|l| l.at), "outcome": last.map(|l| l.outcome) })
}

/// `GET /api/admin/v1/mcp/status`: what the link to the front keeps, and the approval key's fingerprint and version. Counts and
/// versions only; no key, user or URL.
pub async fn status(State(state): St) -> Json<serde_json::Value> {
    let (config, key) = {
        let db = state.db.lock().await;
        (crate::mcp_link::build_config(db.conn()), crate::approval_key::public_key(db.conn()))
    };
    let key = key.map(|(version, k)| serde_json::json!({ "fingerprint": crate::approval_key::fingerprint(&k), "version": version }));
    Json(match state.mcp.get() {
        None => serde_json::json!({
            "online": false, "error": "No link to the MCP gateway is set up.", "configured": false,
            "pushed_version": config.version(), "reported_version": null, "calls": {}, "approval_key": key,
            "searxng": searxng(false, None),
        }),
        Some(l) => {
            let (reported, calls) = l.reported();
            serde_json::json!({
                "online": l.online(), "error": l.last_error(), "configured": reported.is_some(),
                "pushed_version": config.version(), "reported_version": reported, "calls": calls, "approval_key": key,
                "searxng": searxng(l.online(), l.last_search()),
            })
        }
    })
}

/// `POST /api/admin/v1/mcp/approval-key/rotate`: a new key under the next version, a session signed in within 10 minutes. The record
/// of it is made by the audit layer first (version only); the front and `model-gateway` get the new keys at once.
pub async fn rotate_key(
    State(state): St,
    headers: axum::http::HeaderMap,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
) -> Result<Json<serde_json::Value>, Failure> {
    crate::web::require_fresh_signin(&state, &headers)?;
    let key = {
        let db = state.db.lock().await;
        crate::approval_key::rotate(db.conn(), &state.secrets)
            .ok()
            .and_then(|()| crate::approval_key::public_key(db.conn()))
            .ok_or_else(|| err(StatusCode::INTERNAL_SERVER_ERROR, "The admin could not make the approval key."))?
    };
    if let Some(axum::Extension(n)) = &notes {
        n.add("version", key.0.into());
    }
    mark_dirty(&state);
    if let Some(g) = state.gateway.get() {
        g.mark_dirty();
    }
    Ok(Json(serde_json::json!({ "fingerprint": crate::approval_key::fingerprint(&key.1), "version": key.0 })))
}
