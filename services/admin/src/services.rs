//! The Services tab: how the server, the admin, Caddy and CLIProxyAPI are doing, and restarting one.
//! There is no controller and no Docker socket (ADR 0005), so each service is asked in its own way:
//!
//! | Service | State from | Restart |
//! | --- | --- | --- |
//! | `server` | `GET /api/server/v1/status` | `POST /api/server/v1/restart`, then wait for a new boot ID |
//! | `model-gateway` | `GET /api/model-gateway/v1/status` | `POST /api/model-gateway/v1/restart`, wait for a new boot ID, push the configuration again |
//! | `alerter` | `GET /api/alerter/v1/status` | `POST /api/alerter/v1/restart`, wait for a new boot ID, push the settings again |
//! | `admin` | this process | exit after answering; Docker starts it again |
//! | `caddy` | its admin socket | push its configuration again (a live reload) |
//! | `cpa` | its management API (accounts) | ask its supervisor through `restart-request`, wait for `restart-done` and the API |
//!
//! CLIProxyAPI reads its config only at start, so it runs under a supervisor in the `askbar-cpa` image
//! (ADR 0037) that restarts it when the admin writes `restart-request` (or its config changes).
//! Only a person's own session may restart (a script's API key may not), every attempt is written to
//! `service_actions` first, and a few actions per minute is the most that is allowed. Each service the
//! project builds also refuses a restart in its first 30 seconds (askbar_service::restart).

use crate::AdminState;
use askbar_proto::http::ApiError;
use askbar_proto::problem::ErrorCode;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

type St = State<Arc<AdminState>>;
type Failure = ApiError;

/// The services the Services tab shows and may restart.
pub const SERVICES: [&str; 6] = ["server", "model-gateway", "alerter", "admin", "caddy", "cpa"];
/// At most this many actions in `WINDOW_SECS`.
const MAX_ACTIONS: usize = 6;
const WINDOW_SECS: i64 = 60;
/// How long a server restart may take before the admin gives up waiting (ADR 0002: under a minute).
const SERVER_BACK_WITHIN: Duration = Duration::from_secs(60);

fn unreachable(msg: &str) -> Failure {
    ApiError::new(ErrorCode::ServerUnavailable, msg)
}

fn card(name: &str, up: bool, status: impl Into<String>) -> Value {
    json!({ "name": name, "state": if up { "running" } else { "unreachable" }, "status": status.into(), "health": null })
}

async fn server_state(state: &Arc<AdminState>) -> Value {
    let Some(link) = state.server.clone() else { return card("server", false, "No link to the server is set up.") };
    match link.status_now().await {
        Some((Some(v), _)) => card("server", true, format!("Answers; configuration version {v}.")),
        Some((None, _)) => card("server", true, "Answers, but has not received its configuration yet."),
        None => card("server", false, "Does not answer."),
    }
}

async fn gateway_state(state: &Arc<AdminState>) -> Value {
    let Some(link) = state.gateway.get().cloned() else { return card("model-gateway", false, "No link to the gateway is set up.") };
    match link.status_now().await {
        Some((Some(_), _)) => card("model-gateway", true, "Answers and runs the configuration it was given."),
        Some((None, _)) => card(
            "model-gateway",
            true,
            link.last_error()
                .unwrap_or_else(|| "Answers, but has not received its configuration yet. The admin gives it one within seconds.".into()),
        ),
        None => card("model-gateway", false, link.last_error().unwrap_or_else(|| "Does not answer.".into())),
    }
}

async fn alerter_state(state: &Arc<AdminState>) -> Value {
    let Some(link) = state.alerter.get().cloned() else { return card("alerter", false, "No link to the alerter is set up.") };
    match link.status_now().await {
        Some(st) if st.config_version.is_some() => {
            let delivery = match (st.last_delivery_ok_at, st.last_delivery_failed_at) {
                (_, Some(failed)) if st.last_delivery_ok_at.is_none_or(|ok| failed > ok) => " Its last delivery failed.",
                _ => "",
            };
            let release = match link.releases().await {
                Some(r) if r.newer => format!(" Askbar {} is available; this install runs {}.", r.latest.unwrap_or_default(), r.current),
                Some(r) if r.latest.is_some() => format!(" Askbar is up to date ({}).", r.current),
                Some(r) if r.failed_at.is_some() => " The last check for a new release did not work.".to_string(),
                _ => String::new(),
            };
            card("alerter", true, format!("Answers and runs the settings it was given.{delivery}{release}"))
        }
        Some(_) => card(
            "alerter",
            true,
            link.last_error()
                .unwrap_or_else(|| "Answers, but has not received its settings yet. The admin gives it them within seconds.".into()),
        ),
        None => card("alerter", false, link.last_error().unwrap_or_else(|| "Does not answer.".into())),
    }
}

async fn caddy_state(state: &Arc<AdminState>) -> Value {
    if crate::caddy::reachable(state).await {
        card("caddy", true, "Its admin socket answers.")
    } else {
        card("caddy", false, "Its admin socket does not answer.")
    }
}

async fn cpa_state(state: &Arc<AdminState>) -> Value {
    let link = state.cpa.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match link {
        None => card("cpa", false, "Not set up from here (its config could not be made)."),
        Some(l) => match l.accounts().await {
            Ok(a) => card("cpa", true, format!("Answers; {} account(s) connected.", a.len())),
            Err(_) => card("cpa", false, "Does not answer."),
        },
    }
}

fn recent(db: &crate::AdminDb) -> Value {
    let mut out = Vec::new();
    if let Ok(mut stmt) = db.conn().prepare("SELECT service, action, at, ok FROM service_actions ORDER BY id DESC LIMIT 10") {
        if let Ok(rows) = stmt.query_map([], |r| Ok(json!({ "service": r.get::<_, String>(0)?, "action": r.get::<_, String>(1)?, "at": r.get::<_, i64>(2)?, "ok": r.get::<_, i64>(3)? != 0 }))) {
            out.extend(rows.flatten());
        }
    }
    Value::Array(out)
}

/// `GET /api/admin/v1/services`: how each service is doing, and the last ten actions.
pub async fn list(State(state): St) -> Result<Json<Value>, Failure> {
    let (server, gateway, alerter, caddy, cpa) =
        tokio::join!(server_state(&state), gateway_state(&state), alerter_state(&state), caddy_state(&state), cpa_state(&state));
    let admin = card("admin", true, "This page is served by it.");
    let recent = recent(&*state.db.lock().await);
    Ok(Json(json!({ "services": [server, gateway, alerter, admin, caddy, cpa], "recent": recent })))
}

/// `POST /api/admin/v1/services/{name}/restart`.
pub async fn restart(State(state): St, Path(name): Path<String>) -> Result<(StatusCode, Json<Value>), Failure> {
    if !SERVICES.contains(&name.as_str()) {
        return Err(ApiError::new(ErrorCode::NotFound, "No such service."));
    }
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
    // Recorded before anything is asked, so a restart that takes the admin down is still on record.
    let row = {
        let db = state.db.lock().await;
        db.conn()
            .execute("INSERT INTO service_actions (service, action, at) VALUES (?1, 'restart', ?2)", rusqlite::params![name, now])
            .map_err(|_| ApiError::code(ErrorCode::Internal))?;
        db.conn().last_insert_rowid()
    };
    tracing::info!(service = %name, "restart asked from the admin site");
    let result = match name.as_str() {
        "server" => restart_server(&state).await,
        "model-gateway" => restart_gateway(&state).await,
        "alerter" => restart_alerter(&state).await,
        "admin" => restart_admin(&state).await,
        "caddy" => restart_caddy(&state).await,
        _ => restart_cpa(&state).await,
    };
    if result.is_ok() {
        let db = state.db.lock().await;
        let _ = db.conn().execute("UPDATE service_actions SET ok = 1 WHERE id = ?1", [row]);
    }
    result
}

/// Ask the server to restart, then wait until a new process answers (a new boot ID).
async fn restart_server(state: &Arc<AdminState>) -> Result<(StatusCode, Json<Value>), Failure> {
    let link = state.server.clone().ok_or_else(|| unreachable("No link to the server is set up."))?;
    let path = askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, askbar_service::routes::RESTART);
    let (status, body) = link
        .request(reqwest::Method::POST, &path, None)
        .await
        .map_err(|_| unreachable("The server did not answer, so it could not be asked to restart."))?;
    if status != 202 {
        let p: Option<askbar_proto::Problem> = serde_json::from_slice(&body).ok();
        let detail = p.as_ref().map(|p| p.detail.clone()).unwrap_or_else(|| format!("The server answered {status}."));
        return Err(match p.as_ref().and_then(|p| p.code()) {
            // The server's cooldown (its first 30 seconds): pass the refusal on.
            Some(ErrorCode::TooManyAttempts) => {
                ApiError::new(ErrorCode::TooManyAttempts, detail).retry_after(askbar_service::restart::COOLDOWN.as_secs())
            }
            _ => unreachable(&detail),
        });
    }
    let old = serde_json::from_slice::<Value>(&body).ok().and_then(|v| v["boot_id"].as_str().map(String::from)).unwrap_or_default();
    let started = Instant::now();
    while started.elapsed() < SERVER_BACK_WITHIN {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some((_, boot)) = link.status_now().await {
            if boot != old {
                // The status check that follows sees the new boot ID and pushes the configuration.
                link.check().await;
                let secs = started.elapsed().as_secs();
                return Ok((StatusCode::OK, Json(card("server", true, format!("Restarted; back after {secs}s.")))));
            }
        }
    }
    Err(unreachable("The server was asked to restart but has not come back within a minute. Check it from the host."))
}

/// Ask the gateway to restart, wait for a new process, then give it its configuration again: its keys were in the
/// memory that just went.
async fn restart_gateway(state: &Arc<AdminState>) -> Result<(StatusCode, Json<Value>), Failure> {
    let link = state.gateway.get().cloned().ok_or_else(|| unreachable("No link to the gateway is set up."))?;
    let old = link.restart().await?;
    let started = Instant::now();
    while started.elapsed() < SERVER_BACK_WITHIN {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some((_, boot)) = link.status_now().await {
            if boot != old {
                let pushed = link.sync(state).await;
                let secs = started.elapsed().as_secs();
                let said = match pushed {
                    crate::gateway_link::Sync::Pushed(_) | crate::gateway_link::Sync::Current => {
                        format!("Restarted; back after {secs}s, with its keys.")
                    }
                    _ => format!("Restarted; back after {secs}s. It has no configuration yet; the admin keeps trying."),
                };
                return Ok((StatusCode::OK, Json(card("model-gateway", true, said))));
            }
        }
    }
    Err(unreachable("The gateway was asked to restart but has not come back within a minute. Check it from the host."))
}

/// Ask the alerter to restart, wait for a new process, then give it its settings again: they were in the memory that just went.
async fn restart_alerter(state: &Arc<AdminState>) -> Result<(StatusCode, Json<Value>), Failure> {
    let link = state.alerter.get().cloned().ok_or_else(|| unreachable("No link to the alerter is set up."))?;
    let old = link.restart().await?;
    let started = Instant::now();
    while started.elapsed() < SERVER_BACK_WITHIN {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Some(st) = link.status_now().await {
            if st.boot_id != old {
                let pushed = link.sync(state).await;
                let secs = started.elapsed().as_secs();
                let said = match pushed {
                    crate::alerter_link::Sync::Pushed(_) | crate::alerter_link::Sync::Current => {
                        format!("Restarted; back after {secs}s, with its settings.")
                    }
                    _ => format!("Restarted; back after {secs}s. It has no settings yet; the admin keeps trying."),
                };
                return Ok((StatusCode::OK, Json(card("alerter", true, said))));
            }
        }
    }
    Err(unreachable("The alerter was asked to restart but has not come back within a minute. Check it from the host."))
}

/// Answer, then exit; Docker starts the admin again. This page reconnects when it is back.
async fn restart_admin(state: &Arc<AdminState>) -> Result<(StatusCode, Json<Value>), Failure> {
    let r = state.restarter.get().cloned().ok_or_else(|| ApiError::new(ErrorCode::Internal, "Restart is not set up in this process."))?;
    let _accepted = askbar_service::restart::handler(State(r)).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(
            json!({ "name": "admin", "state": "restarting", "status": "Restarting; this page reconnects in a few seconds.", "health": null }),
        ),
    ))
}

/// Caddy has no restart endpoint (ADR 0005): push its configuration again, which it applies live.
async fn restart_caddy(state: &Arc<AdminState>) -> Result<(StatusCode, Json<Value>), Failure> {
    let st = crate::caddy::apply(state).await;
    if st.ok {
        Ok((StatusCode::OK, Json(card("caddy", true, "Configuration reloaded."))))
    } else {
        Err(unreachable(&st.message))
    }
}

/// Ask CLIProxyAPI's supervisor (ADR 0037): write a new value to `restart-request`, then wait for the
/// supervisor to echo it in `restart-done` and for CLIProxyAPI's API to answer, at most a minute.
async fn restart_cpa(state: &Arc<AdminState>) -> Result<(StatusCode, Json<Value>), Failure> {
    let dir = crate::cpa::config_dir();
    let request = crate::cpa::request_restart(&dir).map_err(|_| unreachable("CLIProxyAPI's restart request could not be written."))?;
    let started = Instant::now();
    let link = state.cpa.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut restarted = false;
    while started.elapsed() < SERVER_BACK_WITHIN {
        tokio::time::sleep(Duration::from_millis(500)).await;
        restarted = restarted || std::fs::read_to_string(dir.join(crate::cpa::RESTART_DONE)).is_ok_and(|d| d == request);
        if restarted {
            match &link {
                Some(l) if l.accounts().await.is_err() => continue,
                _ => {
                    let secs = started.elapsed().as_secs();
                    return Ok((StatusCode::OK, Json(card("cpa", true, format!("Restarted; back after {secs}s.")))));
                }
            }
        }
    }
    Err(unreachable(if restarted {
        "CLIProxyAPI was restarted but does not answer. Check it from the host."
    } else {
        "CLIProxyAPI's supervisor did not act on the restart request within a minute. Check it from the host."
    }))
}
