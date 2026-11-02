//! Read endpoints for a paired device. Every query is scoped to the token's user.

use crate::{auth::DeviceAuth, AppState};
use askbar_proto::{Conversation, ConversationDetail, DisplaySettings, Message, Prefs, ProviderInfo, SearchHit, Settings};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use rusqlite::{params, OptionalExtension};
use std::sync::Arc;

type St = State<Arc<AppState>>;
type Res<T> = Result<Json<T>, StatusCode>;

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

/// The providers clients may use, from the snapshot: the default first, then the fallbacks
/// in order, then the rest by name. A client with no choice of its own uses the first one.
pub async fn providers(State(state): St) -> Res<Vec<ProviderInfo>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = crate::snapshot::current(&db).map_err(internal)?;
    let list = snapshot.map(|s| crate::snapshot::in_client_order(&s)).unwrap_or_default();
    Ok(Json(list.into_iter().map(|p| ProviderInfo { id: p.id, name: p.name, kind: p.kind, model: p.model }).collect()))
}

pub async fn conversations(State(state): St, Extension(auth): Extension<DeviceAuth>) -> Res<Vec<Conversation>> {
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
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
) -> Res<ConversationDetail> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let c = db.get_conversation(&auth.user_id, &id).map_err(internal)?.ok_or(StatusCode::NOT_FOUND)?;
    let messages = db
        .get_messages(&auth.user_id, &id)
        .map_err(internal)?
        .into_iter()
        .map(|m| Message { id: m.id, role: m.role, content: m.content, created_at: m.created_at })
        .collect();
    Ok(Json(ConversationDetail {
        conversation: Conversation { id: c.id, title: c.title, updated_at: c.updated_at, message_count: c.message_count },
        messages,
    }))
}

pub async fn delete_conversation(
    State(state): St,
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
) -> StatusCode {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match db.delete_conversation(&auth.user_id, &id) {
        Ok(false) => StatusCode::NOT_FOUND,
        Ok(true) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn get_prefs(State(state): St, Extension(auth): Extension<DeviceAuth>) -> Res<Prefs> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let active: Option<Option<String>> = db
        .conn()
        .query_row("SELECT active_provider_id FROM user_prefs WHERE user_id = ?1", [&auth.user_id], |r| r.get(0))
        .optional()
        .map_err(internal)?;
    Ok(Json(Prefs { active_provider_id: active.flatten() }))
}

/// Replaces the caller's preferences. A provider id must name an existing provider.
pub async fn put_prefs(State(state): St, Extension(auth): Extension<DeviceAuth>, Json(p): Json<Prefs>) -> Res<Prefs> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(id) = &p.active_provider_id {
        let listed = crate::snapshot::current(&db).map_err(internal)?.is_some_and(|s| s.providers.iter().any(|p| &p.id == id));
        if !listed {
            return Err(StatusCode::NOT_FOUND);
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
pub async fn search(State(state): St, Extension(auth): Extension<DeviceAuth>, Query(q): Query<SearchQuery>) -> Res<Vec<SearchHit>> {
    if q.q.chars().count() > 200 {
        return Err(StatusCode::BAD_REQUEST);
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
pub async fn display_defaults(State(state): St, Extension(auth): Extension<DeviceAuth>) -> Res<DisplaySettings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(served_display(&db, Some(&auth.user_id)).map_err(internal)?))
}

pub async fn settings(State(state): St) -> Res<Settings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(Settings { display: served_display(&db, None).map_err(internal)? }))
}
