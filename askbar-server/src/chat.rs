//! `POST /api/v1/chat`: stream a reply as our own SSE events.
//!
//! Provider output is never passed through; it is decoded by `askbar-core`
//! and re-emitted as `meta`, `delta`, `done` or `error`.

use crate::{auth::DeviceAuth, store, AppState};
use askbar_core::{stream_chat, ChatMessage};
use askbar_proto::{ChatEvent, ChatRequest};
use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Extension, Json,
};
use rusqlite::{params, OptionalExtension};
use std::{convert::Infallible, sync::Arc};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::UnboundedReceiverStream, StreamExt};

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

fn sse_event(e: &ChatEvent) -> Event {
    let name = match e {
        ChatEvent::Meta { .. } => "meta",
        ChatEvent::Delta { .. } => "delta",
        ChatEvent::Done => "done",
        ChatEvent::Error { .. } => "error",
    };
    Event::default().event(name).json_data(e).expect("ChatEvent serializes")
}

/// Holds the reply as it arrives. Dropping it, however the task ends
/// (finished, error, client gone, panic), saves whatever text there is.
struct ReplyGuard {
    state: Arc<AppState>,
    conversation: i64,
    user_id: String,
    text: String,
}

impl Drop for ReplyGuard {
    fn drop(&mut self) {
        if self.text.trim().is_empty() {
            return;
        }
        let db = self.state.db.lock().unwrap_or_else(|e| e.into_inner());
        let _ = save_message(&db, self.conversation, &self.user_id, "assistant", &self.text);
    }
}

fn save_message(db: &askbar_core::Db, conversation: i64, user_id: &str, role: &str, text: &str) -> Result<(), askbar_core::DbError> {
    let id = db.add_message(conversation, role, text)?;
    db.conn().execute(
        "UPDATE messages SET public_id = ?1, user_id = ?2 WHERE id = ?3",
        params![ulid::Ulid::new().to_string(), user_id, id],
    )?;
    Ok(())
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<DeviceAuth>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Everything that can fail with a plain HTTP status happens before the stream starts.
    let secrets = state.secrets().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let (provider, system, conv_rowid, conv_public, history) = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let provider = store::load_provider(&db, &secrets, &req.provider)
            .map_err(internal)?
            .ok_or(StatusCode::NOT_FOUND)?;
        let system = store::system_prompt(&db).map_err(internal)?;
        let (rowid, public) = match &req.conversation_id {
            Some(pid) => {
                let found: Option<i64> = db
                    .conn()
                    .query_row(
                        "SELECT id FROM conversations WHERE public_id = ?1 AND user_id = ?2",
                        params![pid, auth.user_id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(internal)?;
                (found.ok_or(StatusCode::NOT_FOUND)?, pid.clone())
            }
            None => {
                let id = db.create_conversation(&askbar_core::make_title(&text)).map_err(internal)?;
                let public = ulid::Ulid::new().to_string();
                db.conn()
                    .execute(
                        "UPDATE conversations SET public_id = ?1, user_id = ?2 WHERE id = ?3",
                        params![public, auth.user_id, id],
                    )
                    .map_err(internal)?;
                (id, public)
            }
        };
        // Save the question first so it survives a failed request.
        save_message(&db, rowid, &auth.user_id, "user", &text).map_err(internal)?;
        let history: Vec<ChatMessage> = db
            .get_messages(rowid)
            .map_err(internal)?
            .iter()
            .map(|m| ChatMessage::new(&m.role, &m.content))
            .collect();
        (provider, system, rowid, public, history)
    };

    let (tx, rx) = mpsc::unbounded_channel::<ChatEvent>();
    let _ = tx.send(ChatEvent::Meta { conversation_id: conv_public });

    tokio::spawn(async move {
        let mut guard = ReplyGuard { state: state.clone(), conversation: conv_rowid, user_id: auth.user_id, text: String::new() };
        // Never fires; `stream_chat` wants a cancel channel and we cancel by dropping the future.
        let (_keep, cancel_rx) = oneshot::channel::<()>();
        let run = stream_chat(&state.http, &provider, &system, &history, cancel_rx, |delta| {
            guard.text.push_str(delta);
            let _ = tx.send(ChatEvent::Delta { text: delta.to_string() });
        });
        let outcome = tokio::select! {
            r = run => Some(r),
            // The client went away: dropping `run` closes the upstream request.
            _ = tx.closed() => None,
        };
        if let Some(r) = outcome {
            let _ = tx.send(match r {
                Ok(_) => ChatEvent::Done,
                Err(e) => ChatEvent::Error { message: e.to_string() },
            });
        }
        // `guard` drops here and saves the reply (partial or complete).
    });

    let stream = UnboundedReceiverStream::new(rx).map(|e| Ok(sse_event(&e)));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
