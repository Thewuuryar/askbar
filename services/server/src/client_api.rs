//! Read endpoints for a paired client. Every query is scoped to the token's user.

use crate::error::{ApiError, ApiJson};
use crate::{auth::ClientAuth, AppState};
use askbar_proto::problem::ErrorCode;
use askbar_proto::{Conversation, ConversationDetail, DisplaySettings, Message, PendingGeneration, Prefs, ProviderInfo, SearchHit, Settings};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;

type St = State<Arc<AppState>>;
type Res<T> = Result<Json<T>, ApiError>;

fn internal<E>(_: E) -> ApiError {
    ApiError::code(ErrorCode::Internal)
}

/// The providers clients may use, from the snapshot: the default first, then the rest by name. A client with no choice of its own uses the first one.
/// `POST /me/pairing-keys`: a code to pair another client of this user (ADR 0008). The client builds the
/// pairing key from it with the server address and certificate pin it already has.
pub async fn new_pairing_key(
    State(state): St,
    Extension(auth): Extension<ClientAuth>,
    ApiJson(req): ApiJson<askbar_proto::PairingKeyRequest>,
) -> Result<(axum::http::StatusCode, Json<askbar_proto::PairingKeyResponse>), ApiError> {
    if req.name.chars().count() > 60 {
        return Err(ApiError::new(ErrorCode::BadRequest, "The client name is too long."));
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match crate::auth::new_client_pair_code(&db, &auth, &req.name).map_err(internal)? {
        Some(code) => Ok((axum::http::StatusCode::CREATED, Json(askbar_proto::PairingKeyResponse { code, expires_in: crate::auth::CLIENT_PAIR_CODE_TTL_SECS }))),
        None => Err(ApiError::new(
            ErrorCode::TooManyAttempts,
            format!("{} pairing keys are already waiting to be used. Use one, or wait 10 minutes.", crate::auth::MAX_OPEN_CLIENT_PAIR_CODES),
        )
        .retry_after(crate::auth::CLIENT_PAIR_CODE_TTL_SECS as u64)),
    }
}

pub async fn providers(State(state): St) -> Res<Vec<ProviderInfo>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = crate::snapshot::current(&db).map_err(internal)?;
    let list = snapshot.map(|s| crate::snapshot::in_client_order(&s)).unwrap_or_default();
    Ok(Json(list.into_iter().map(|p| ProviderInfo { id: p.id, name: p.name, kind: p.kind, model: p.model }).collect()))
}

pub async fn conversations(State(state): St, Extension(auth): Extension<ClientAuth>) -> Res<Vec<Conversation>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let rows = db
        .list_conversations(&auth.user_id)
        .map_err(internal)?
        .into_iter()
        .map(|c| Conversation { id: c.id, title: c.title, updated_at: c.updated_at, message_count: c.message_count })
        .collect();
    Ok(Json(rows))
}

pub async fn conversation(
    State(state): St,
    Extension(auth): Extension<ClientAuth>,
    Path(id): Path<String>,
) -> Res<ConversationDetail> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let c = db.get_conversation(&auth.user_id, &id).map_err(internal)?.ok_or_else(|| ApiError::new(ErrorCode::NotFound, "That conversation was not found."))?;
    let messages = db
        .get_messages(&auth.user_id, &id)
        .map_err(internal)?
        .into_iter()
        .map(|m| Message { id: m.id, role: m.role, content: m.content, created_at: m.created_at })
        .collect();
    let pending = db
        .list_active_generations(&auth.user_id)
        .map_err(internal)?
        .into_iter()
        .filter(|g| g.conversation_id == id)
        .map(pending_of)
        .collect();
    Ok(Json(ConversationDetail {
        conversation: Conversation { id: c.id, title: c.title, updated_at: c.updated_at, message_count: c.message_count },
        messages,
        pending,
    }))
}

fn pending_of(g: crate::Generation) -> PendingGeneration {
    PendingGeneration {
        generation_id: g.id,
        conversation_id: g.conversation_id,
        status: g.status,
        text: g.text,
        prompt: g.prompt,
        client_request_id: g.client_request_id,
    }
}

#[derive(serde::Deserialize)]
pub struct GenerationsQuery {
    status: Option<String>,
}

/// `GET /api/client/v1/generations?status=running`: the caller's running and queued generations, across
/// conversations, so a client that just started (or reconnected) can find what is still going.
pub async fn generations(
    State(state): St,
    Extension(auth): Extension<ClientAuth>,
    Query(q): Query<GenerationsQuery>,
) -> Res<Vec<PendingGeneration>> {
    if q.status.as_deref().is_some_and(|s| s != "running") {
        return Err(ApiError::new(ErrorCode::BadRequest, "Only status=running is supported."));
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(db.list_active_generations(&auth.user_id).map_err(internal)?.into_iter().map(pending_of).collect()))
}

pub async fn delete_conversation(
    State(state): St,
    Extension(auth): Extension<ClientAuth>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    match crate::generation::delete_conversation(&state, &auth.user_id, &id) {
        Ok(false) => Err(ApiError::new(ErrorCode::NotFound, "That conversation was not found.")),
        Ok(true) => Ok(StatusCode::NO_CONTENT),
        Err(_) => Err(ApiError::code(ErrorCode::Internal)),
    }
}

pub async fn get_prefs(State(state): St, Extension(auth): Extension<ClientAuth>) -> Res<Prefs> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let active: Option<Option<String>> = db
        .conn()
        .query_row("SELECT active_provider_id FROM user_prefs WHERE user_id = ?1", [&auth.user_id], |r| r.get(0))
        .optional()
        .map_err(internal)?;
    Ok(Json(Prefs { active_provider_id: active.flatten() }))
}

/// Replaces the caller's preferences. A provider id must name an existing provider.
pub async fn put_prefs(State(state): St, Extension(auth): Extension<ClientAuth>, ApiJson(p): ApiJson<Prefs>) -> Res<Prefs> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(id) = &p.active_provider_id {
        let listed = crate::snapshot::current(&db).map_err(internal)?.is_some_and(|s| s.providers.iter().any(|p| &p.id == id));
        if !listed {
            return Err(ApiError::new(ErrorCode::NotFound, "That provider does not exist."));
        }
    }
    db.conn()
        .execute(
            "INSERT INTO user_prefs (user_id, active_provider_id) VALUES (?1, ?2)
             ON CONFLICT(user_id) DO UPDATE SET active_provider_id = excluded.active_provider_id",
            params![auth.user_id, p.active_provider_id],
        )
        .map_err(internal)?;
    Ok(Json(p))
}

#[derive(serde::Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

/// Full-text search over the caller's own messages.
pub async fn search(State(state): St, Extension(auth): Extension<ClientAuth>, Query(q): Query<SearchQuery>) -> Res<Vec<SearchHit>> {
    if q.q.chars().count() > 200 {
        return Err(ApiError::new(ErrorCode::BadRequest, "A search can be at most 200 characters."));
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let hits = db
        .search_messages(&auth.user_id, &q.q, 20)
        .map_err(internal)?
        .into_iter()
        .map(|h| SearchHit { conversation_id: h.conversation_id, message_id: h.message_id, role: h.role, snippet: h.snippet, created_at: h.created_at })
        .collect();
    Ok(Json(hits))
}

/// What clients are served, from the snapshot: the user's own defaults if there are any,
/// else the server-wide ones, else the built-ins.
pub fn served_display(db: &crate::Db, user_id: Option<&str>) -> rusqlite::Result<DisplaySettings> {
    let Some(s) = crate::snapshot::current(db)? else { return Ok(DisplaySettings::default()) };
    Ok(user_id.and_then(|u| s.user_display_defaults.get(u).cloned()).unwrap_or(s.display))
}

/// Read-only: what this user's clients should look like unless the person overrides it locally.
pub async fn display_defaults(State(state): St, Extension(auth): Extension<ClientAuth>) -> Res<DisplaySettings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(served_display(&db, Some(&auth.user_id)).map_err(internal)?))
}

pub async fn settings(State(state): St) -> Res<Settings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(Settings { display: served_display(&db, None).map_err(internal)? }))
}
