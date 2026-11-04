//! Start, stop and restart the server, Caddy and CLIProxyAPI, through the controller. Only a person's own
//! session may do this (a script's API key may not), every attempt is written to `service_actions` before the
//! controller is asked, and a few actions per minute is the most that is allowed.

use crate::control_link::{ACTIONS, SERVICES};
use crate::AdminState;
use askbar_proto::http::ApiError;
use askbar_proto::problem::ErrorCode;
use axum::{
    extract::{Path, State},
    http::header,
    response::Response,
    Json,
};
use std::sync::Arc;

type St = State<Arc<AdminState>>;
type Failure = ApiError;

/// At most this many actions in `WINDOW_SECS`.
const MAX_ACTIONS: usize = 6;
const WINDOW_SECS: i64 = 60;

fn unreachable(msg: &str) -> Failure {
    ApiError::new(ErrorCode::ServerUnavailable, msg)
}

fn control(state: &AdminState) -> Result<Arc<crate::control_link::ControlLink>, Failure> {
    state
        .control
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .ok_or_else(|| unreachable("The controller is not set up, so services cannot be controlled from here."))
}

fn passthrough(status: u16, body: Vec<u8>) -> Result<Response, Failure> {
    if status != 200 {
        // The controller speaks problems too; pass its sentence on without its status.
        let detail = serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v["detail"].as_str().map(String::from))
            .unwrap_or_else(|| format!("The controller answered {status}."));
        return Err(if status == 404 { ApiError::new(ErrorCode::NotFound, detail) } else { unreachable(&detail) });
    }
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body))
        .map_err(|_| ApiError::code(ErrorCode::Internal))
}

fn recent(db: &crate::AdminDb) -> serde_json::Value {
    let mut out = Vec::new();
    if let Ok(mut stmt) = db.conn().prepare("SELECT service, action, at, ok FROM service_actions ORDER BY id DESC LIMIT 10") {
        if let Ok(rows) = stmt.query_map([], |r| Ok(serde_json::json!({ "service": r.get::<_, String>(0)?, "action": r.get::<_, String>(1)?, "at": r.get::<_, i64>(2)?, "ok": r.get::<_, i64>(3)? != 0 }))) {
            out.extend(rows.flatten());
        }
    }
    serde_json::Value::Array(out)
}

/// `GET /admin/api/v1/services`: how each service is doing, and the last ten actions.
pub async fn list(State(state): St) -> Result<Json<serde_json::Value>, Failure> {
    let link = control(&state)?;
    let (status, body) = link.services().await.map_err(|e| unreachable(&e))?;
    if status != 200 {
        passthrough(status, body)?;
        return Err(unreachable("The controller gave no list."));
    }
    let mut v: serde_json::Value = serde_json::from_slice(&body).map_err(|_| unreachable("The controller's answer was not understood."))?;
    v["recent"] = recent(&*state.db.lock().await);
    Ok(Json(v))
}

/// `POST /admin/api/v1/services/{name}/{action}`.
pub async fn act(State(state): St, Path((name, action)): Path<(String, String)>) -> Result<Response, Failure> {
    if !SERVICES.contains(&name.as_str()) {
        return Err(ApiError::new(ErrorCode::NotFound, "No such service."));
    }
    if !ACTIONS.contains(&action.as_str()) {
        return Err(ApiError::new(ErrorCode::NotFound, "No such action. Use start, stop or restart."));
    }
    let link = control(&state)?;
    let now = crate::auth::now();
    {
        let mut q = state.service_actions.lock().unwrap_or_else(|e| e.into_inner());
        while q.front().is_some_and(|t| now - t >= WINDOW_SECS) {
            q.pop_front();
        }
        if q.len() >= MAX_ACTIONS {
            return Err(ApiError::new(ErrorCode::TooManyAttempts, "Too many service actions in a minute. Wait a little and try again.")
                .retry_after(WINDOW_SECS as u64));
        }
        q.push_back(now);
    }
    let row = {
        let db = state.db.lock().await;
        db.conn()
            .execute("INSERT INTO service_actions (service, action, at) VALUES (?1, ?2, ?3)", rusqlite::params![name, action, now])
            .map_err(|_| ApiError::code(ErrorCode::Internal))?;
        db.conn().last_insert_rowid()
    };
    let (status, body) = link.act(&name, &action).await.map_err(|e| unreachable(&e))?;
    if status == 200 {
        let db = state.db.lock().await;
        let _ = db.conn().execute("UPDATE service_actions SET ok = 1 WHERE id = ?1", [row]);
    }
    passthrough(status, body)
}
