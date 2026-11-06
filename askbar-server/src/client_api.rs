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

/// The providers clients may use, from the snapshot: the default first, then the fallbacks
/// in order, then the rest by name. A client with no choice of its own uses the first one.
pub async fn providers(State(state): St) -> Res<Vec<ProviderInfo>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = crate::snapshot::current(&db).map_err(internal)?;
    let list = snapshot.map(|s| crate::snapshot::in_client_order(&s)).unwrap_or_default();
    Ok(Json(list.into_iter().map(|p| ProviderInfo { id: p.id, name: p.name, kind: p.kind, model: p.model }).collect()))
}

/// The models users may choose on one provider.
pub async fn provider_models(State(state): St, Path(id): Path<String>) -> Res<askbar_proto::ProviderModels> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = crate::snapshot::current(&db).map_err(internal)?;
    let Some(p) = snapshot.and_then(|s| s.providers.into_iter().find(|p| p.id == id)) else {
        return Err(ApiError::new(ErrorCode::NotFound, "That provider does not exist."));
    };
    Ok(Json(askbar_proto::ProviderModels { default: p.model.clone(), models: p.choices() }))
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
    let (provider_id, model) = db.conversation_model(&auth.user_id, &id).map_err(internal)?.unwrap_or_default();
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
        provider_id,
        model,
        pending,
    }))
}

/// Sets the provider and model one conversation answers with from its next message on.
pub async fn put_conversation_model(
    State(state): St,
    Extension(auth): Extension<ClientAuth>,
    Path(id): Path<String>,
    ApiJson(m): ApiJson<askbar_proto::ConversationModel>,
) -> Res<askbar_proto::ConversationModel> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = crate::snapshot::current(&db).map_err(internal)?;
    let Some(provider) = snapshot.as_ref().and_then(|s| s.providers.iter().find(|p| p.id == m.provider_id)) else {
        return Err(ApiError::new(ErrorCode::NotFound, "That provider does not exist."));
    };
    if m.model.as_ref().is_some_and(|wanted| !provider.choices().contains(wanted)) {
        return Err(ApiError::new(ErrorCode::NotFound, "That provider does not offer that model."));
    }
    if !db.set_conversation_model(&auth.user_id, &id, &m.provider_id, m.model.as_deref()).map_err(internal)? {
        return Err(ApiError::new(ErrorCode::NotFound, "That conversation was not found."));
    }
    Ok(Json(m))
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
    let row: Option<(Option<String>, Option<String>)> = db
        .conn()
        .query_row("SELECT active_provider_id, active_model FROM user_prefs WHERE user_id = ?1", [&auth.user_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(internal)?;
    let (active_provider_id, active_model) = row.unwrap_or_default();
    Ok(Json(Prefs { active_provider_id, active_model }))
}

/// Replaces the caller's preferences. A provider id must name an existing provider.
pub async fn put_prefs(State(state): St, Extension(auth): Extension<ClientAuth>, ApiJson(p): ApiJson<Prefs>) -> Res<Prefs> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if p.active_model.is_some() && p.active_provider_id.is_none() {
        return Err(ApiError::new(ErrorCode::BadRequest, "Choose a provider to choose one of its models."));
    }
    if let Some(id) = &p.active_provider_id {
        let snapshot = crate::snapshot::current(&db).map_err(internal)?;
        let Some(provider) = snapshot.as_ref().and_then(|s| s.providers.iter().find(|p| &p.id == id)) else {
            return Err(ApiError::new(ErrorCode::NotFound, "That provider does not exist."));
        };
        if let Some(m) = &p.active_model {
            if !provider.choices().contains(m) {
                return Err(ApiError::new(ErrorCode::NotFound, "That provider does not offer that model."));
            }
        }
    }
    db.conn()
        .execute(
            "INSERT INTO user_prefs (user_id, active_provider_id, active_model) VALUES (?1, ?2, ?3)
             ON CONFLICT(user_id) DO UPDATE SET active_provider_id = excluded.active_provider_id, active_model = excluded.active_model",
            params![auth.user_id, p.active_provider_id, p.active_model],
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
