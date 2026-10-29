//! Read endpoints for a paired device. Every query is scoped to the token's user.

use crate::{auth::DeviceAuth, AppState};
use askbar_proto::{Conversation, ConversationDetail, DisplaySettings, Message, ProviderInfo, Settings};
use axum::{
    extract::{Path, State},
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
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT c.public_id, c.title, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id)
             FROM conversations c WHERE c.user_id = ?1
             ORDER BY c.updated_at DESC, c.id DESC",
        )
        .map_err(internal)?;
    let rows = stmt
        .query_map([&auth.user_id], |r| {
            Ok(Conversation { id: r.get(0)?, title: r.get(1)?, updated_at: r.get(2)?, message_count: r.get(3)? })
        })
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(rows))
}

pub async fn conversation(
    State(state): St,
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
) -> Res<ConversationDetail> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let found: Option<(i64, String, i64)> = db
        .conn()
        .query_row(
            "SELECT id, title, updated_at FROM conversations WHERE public_id = ?1 AND user_id = ?2",
            params![id, auth.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(internal)?;
    let (rowid, title, updated_at) = found.ok_or(StatusCode::NOT_FOUND)?;
    let mut stmt = db
        .conn()
        .prepare("SELECT public_id, role, content, created_at FROM messages WHERE conversation_id = ?1 ORDER BY id")
        .map_err(internal)?;
    let messages = stmt
        .query_map([rowid], |r| Ok(Message { id: r.get(0)?, role: r.get(1)?, content: r.get(2)?, created_at: r.get(3)? }))
        .map_err(internal)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(internal)?;
    Ok(Json(ConversationDetail {
        conversation: Conversation { id, title, updated_at, message_count: messages.len() as i64 },
        messages,
    }))
}

pub async fn delete_conversation(
    State(state): St,
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
) -> StatusCode {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    match db
        .conn()
        .execute("DELETE FROM conversations WHERE public_id = ?1 AND user_id = ?2", params![id, auth.user_id])
    {
        Ok(0) => StatusCode::NOT_FOUND,
        Ok(_) => StatusCode::NO_CONTENT,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Display defaults: the stored `display` setting (a JSON object that may
/// set only some fields) laid over the built-in defaults.
pub fn load_display(db: &askbar_core::Db) -> rusqlite::Result<DisplaySettings> {
    let stored: Option<String> =
        db.conn().query_row("SELECT value FROM settings WHERE key = 'display'", [], |r| r.get(0)).optional()?;
    Ok(stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

pub async fn settings(State(state): St) -> Res<Settings> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    Ok(Json(Settings { display: load_display(&db).map_err(internal)? }))
}
