//! Read endpoints for a paired device. Every query is scoped to the token's user.

use crate::{auth::DeviceAuth, AppState};
use askbar_proto::{Conversation, ConversationDetail, DisplaySettings, Message, ProviderInfo, SearchHit, Settings};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use rusqlite::OptionalExtension;
use std::sync::Arc;

type St = State<Arc<AppState>>;
type Res<T> = Result<Json<T>, StatusCode>;

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

pub async fn providers(State(state): St) -> Res<Vec<ProviderInfo>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let mut stmt = db.conn().prepare("SELECT name, type, model FROM providers ORDER BY name").map_err(internal)?;
    let rows = stmt
        .query_map([], |r| Ok(ProviderInfo { name: r.get(0)?, kind: r.get(1)?, model: r.get(2)? }))
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(rows))
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

/// Display defaults: the stored `display` setting (a JSON object that may
/// set only some fields) laid over the built-in defaults.
pub fn load_display(db: &crate::Db) -> rusqlite::Result<DisplaySettings> {
    let stored: Option<String> =
        db.conn().query_row("SELECT value FROM settings WHERE key = 'display'", [], |r| r.get(0)).optional()?;
    Ok(stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

pub async fn settings(State(state): St) -> Res<Settings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(Settings { display: load_display(&db).map_err(internal)? }))
}
