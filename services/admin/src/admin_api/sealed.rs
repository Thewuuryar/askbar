use super::*;

// ------------------------------------------------------- opening sealed content

#[derive(Deserialize)]
pub struct OpenRequest {
    pub(crate) passphrase: String,
}

/// What the page and the banner need to know about the sealed-content session, for the browser that asks: whether one is
/// open and for how long, and the key registry with which keys it holds.
async fn session_status(state: &AdminState, headers: &axum::http::HeaderMap) -> serde_json::Value {
    use crate::sealed::{limits, FRESH_SIGNIN_SECS};
    let token = crate::web::session_cookie(headers);
    let browser = token.as_deref().map(crate::auth::AdminAuth::hash_token);
    let view = browser.as_deref().and_then(|b| state.sealed.view(b));
    let (idle, total) = limits(state.db.lock().await.conn());
    let keys = state.db.lock().await.audit_keys().unwrap_or_default();
    let held: Vec<String> = view.as_ref().map(|v| v.key_ids.clone()).unwrap_or_default();
    let fresh_left = token.as_deref().and_then(|t| state.auth.session_age_secs(t)).map(|age| (FRESH_SIGNIN_SECS - age).max(0));
    serde_json::json!({
        "open": view.is_some(),
        "idle_left_secs": view.as_ref().map(|v| v.idle_left_secs),
        "total_left_secs": view.as_ref().map(|v| v.total_left_secs),
        "idle_limit_secs": view.as_ref().map_or(idle.as_secs(), |v| v.idle_limit_secs),
        "total_limit_secs": view.as_ref().map_or(total.as_secs(), |v| v.total_limit_secs),
        "fresh_signin_secs_left": fresh_left,
        "keys": keys.iter().map(|k| serde_json::json!({
            "key_id": k.key_id, "created_at": k.created_at, "retired_at": k.retired_at, "open": held.contains(&k.key_id),
        })).collect::<Vec<_>>(),
    })
}

/// The banner's part of the status: whether this browser has a session and how long it has left, and the alerts of the
/// last day. Nothing here names a key or content.
pub(crate) fn banner_status(state: &AdminState, headers: &axum::http::HeaderMap) -> serde_json::Value {
    let browser = crate::web::session_cookie(headers).map(|t| crate::auth::AdminAuth::hash_token(&t));
    let view = browser.as_deref().and_then(|b| state.sealed.view(b));
    // What the sealed-content session raised and what the admin's own alerts raised, oldest first.
    let mut all = state.sealed.alerts();
    all.extend(state.notifier.notes());
    all.sort_by_key(|a| a.at);
    let alerts: Vec<_> = all.iter().map(|a| serde_json::json!({ "kind": a.kind, "at": a.at, "text": a.text })).collect();
    serde_json::json!({
        "open": view.is_some(),
        "idle_left_secs": view.as_ref().map(|v| v.idle_left_secs),
        "total_left_secs": view.as_ref().map(|v| v.total_left_secs),
        "alerts": alerts,
    })
}

/// `GET /api/admin/v1/audit/session`: is sealed content open for this browser, and for how long.
pub async fn get_unseal_session(State(state): St, headers: axum::http::HeaderMap) -> Json<serde_json::Value> {
    // A session that is due ends now, and says so in the record, before anyone is told it is open.
    crate::sealed::sweep(&state).await;
    Json(session_status(&state, &headers).await)
}

/// `POST /api/admin/v1/audit/session`: the passphrase, entered after a fresh sign-in, opens sealed content for a
/// while (ADR 0011 rule 8). The keys it derives live in this process's memory only. The request is recorded before it
/// is handled, so an unseal that cannot be recorded does not happen.
pub async fn start_unseal_session(
    State(state): St,
    headers: axum::http::HeaderMap,
    ApiJson(b): ApiJson<OpenRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), Failure> {
    use crate::audit_key::{derive, KdfParams};
    use crate::sealed::{is_that_key, limits, FRESH_SIGNIN_SECS};
    let now = crate::auth::now();
    let token = crate::web::session_cookie(&headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Sign in again."))?;
    if state.auth.session_age_secs(&token).is_none_or(|age| age > FRESH_SIGNIN_SECS) {
        return Err(err(
            StatusCode::FORBIDDEN,
            &format!(
                "Sealed content needs a fresh sign-in: sign out, sign in again and enter the passphrase within {} minutes.",
                FRESH_SIGNIN_SECS / 60
            ),
        ));
    }
    if let Some(wait) = state.audit_opens.locked_for(now) {
        return Err(err(StatusCode::TOO_MANY_REQUESTS, &format!("Too many wrong passphrases. Try again in {} minutes.", (wait + 59) / 60))
            .retry_after(wait.max(1) as u64));
    }
    let rows = state.db.lock().await.audit_keys().map_err(internal)?;
    if rows.is_empty() {
        return Err(err(StatusCode::CONFLICT, "There is no audit key yet."));
    }
    // One key for each key in the registry that this passphrase makes; each is derived once and kept only in the session.
    let mut keys = std::collections::BTreeMap::new();
    for row in &rows {
        let (pass, salt, params) = (b.passphrase.clone(), row.salt, KdfParams::parse(&row.argon_params).unwrap_or(KdfParams::CURRENT));
        let kp = tokio::task::spawn_blocking(move || derive(&pass, &salt, params)).await.map_err(internal)?.map_err(internal)?;
        if is_that_key(&row.key_id, &kp) {
            keys.insert(row.key_id.clone(), kp);
        }
    }
    // Written before anything is opened; if it cannot be written, nothing is.
    {
        let db = state.db.lock().await;
        let named = keys.keys().next().cloned().or_else(|| rows.last().map(|r| r.key_id.clone())).unwrap_or_default();
        crate::db::record_audit_access(db.conn(), "(session)", &named, now, !keys.is_empty()).map_err(internal)?;
    }
    if keys.is_empty() {
        state.audit_opens.record_wrong(now);
        // 403, not 401: the page treats 401 as "signed out".
        return Err(err(StatusCode::FORBIDDEN, "That passphrase does not open any audit key."));
    }
    state.audit_opens.record_right();
    let (idle, total) = limits(state.db.lock().await.conn());
    let browser = crate::auth::AdminAuth::hash_token(&token);
    let count = keys.len();
    let fresh = state.sealed.start(&browser, keys, idle, total);
    state.sealed.alert(
        "unsealed",
        format!(
            "Sealed audit content was {} ({} key{}); it is sealed again after {} minutes without use or {} in all.",
            if fresh { "unsealed" } else { "unsealed with another passphrase" },
            count,
            if count == 1 { "" } else { "s" },
            idle.as_secs() / 60,
            total.as_secs() / 60
        ),
    );
    let status = session_status(&state, &headers).await;
    Ok((if fresh { StatusCode::CREATED } else { StatusCode::OK }, Json(status)))
}

/// `DELETE /api/admin/v1/audit/session`: Seal. Drops the keys. The request is its record.
pub async fn end_unseal_session(State(state): St) -> StatusCode {
    state.end_sealed(crate::sealed::Ended::Seal, false).await;
    StatusCode::NO_CONTENT
}

/// `GET /api/admin/v1/audit/payloads/{span_id}`: the content of one span, opened with the session's keys. Each view is
/// recorded before it is made (so one that cannot be recorded is refused), and counts as use. Without a session there is
/// nothing to open with: `403`.
pub async fn view_audit_payloads(
    State(state): St,
    headers: axum::http::HeaderMap,
    Path(span_id): Path<String>,
) -> Result<Json<serde_json::Value>, Failure> {
    use base64::Engine;
    if span_id.is_empty() || span_id.len() > 64 || !span_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(err(StatusCode::NOT_FOUND, "No such span."));
    }
    let token = crate::web::session_cookie(&headers).ok_or_else(|| err(StatusCode::UNAUTHORIZED, "Sign in again."))?;
    let browser = crate::auth::AdminAuth::hash_token(&token);
    crate::sealed::sweep(&state).await;
    if !state.sealed.touch(&browser) {
        return Err(err(StatusCode::FORBIDDEN, "Sealed content is sealed. Enter the audit passphrase to open it."));
    }
    let body = match auditor(&state)?.get(&format!("/spans/{span_id}/payloads")).await {
        Some((200, body)) => body,
        Some((404, _)) => return Err(err(StatusCode::NOT_FOUND, "No such span.")),
        Some(_) | None => {
            return Err(err(StatusCode::SERVICE_UNAVAILABLE, "The auditor cannot be reached, so the sealed content cannot be read now."))
        }
    };
    let list: Vec<serde_json::Value> = serde_json::from_value(body).map_err(internal)?;
    if list.is_empty() {
        return Ok(Json(serde_json::json!({ "span_id": span_id, "payloads": [] })));
    }
    let registry = state.db.lock().await.audit_keys().map_err(internal)?;
    let mut out = Vec::new();
    let mut any = false;
    for p in &list {
        let kid = p["key_id"].as_str().unwrap_or("");
        let ct = base64::engine::general_purpose::STANDARD.decode(p["ciphertext_b64"].as_str().unwrap_or("")).map_err(internal)?;
        let text = state.sealed.open_one(&browser, kid, &ct).map(|b| String::from_utf8_lossy(&b).into_owned());
        any |= text.is_some();
        // A payload the session has no key for names the key, so the person knows which passphrase it needs.
        let named = registry.iter().find(|k| k.key_id == kid);
        out.push(serde_json::json!({
            "role": p["role"], "key_id": kid, "text": text,
            "key_created_at": named.map(|k| k.created_at), "key_retired_at": named.and_then(|k| k.retired_at),
        }));
    }
    {
        let first = list[0]["key_id"].as_str().unwrap_or("").to_string();
        let db = state.db.lock().await;
        crate::db::record_audit_access(db.conn(), &span_id, &first, crate::auth::now(), any).map_err(internal)?;
    }
    Ok(Json(serde_json::json!({ "span_id": span_id, "payloads": out })))
}
