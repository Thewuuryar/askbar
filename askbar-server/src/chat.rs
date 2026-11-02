//! `POST /api/v1/chat`: start a generation and stream its reply as our own SSE events.
//!
//! Provider output is never passed through; it is decoded by `llm`
//! and re-emitted as `meta`, `delta`, `done` or `error`. The generation runs as its own task
//! (see `generation`), so this stream going away does not stop it.

use crate::generation::{self, GenEvent};
use crate::llm::ChatMessage;
use crate::{auth::DeviceAuth, store, AppState};
use askbar_proto::{ChatEvent, ChatRequest};
use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Extension, Json,
};
use std::{convert::Infallible, sync::Arc};
use tokio_stream::StreamExt;

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
    let (provider, system, conversation, history, user_message_id) = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let secrets = state.secrets();
        let provider = store::load_provider(&db, &secrets, &req.provider)
            .map_err(internal)?
            .ok_or(StatusCode::NOT_FOUND)?;
        let system = store::chat_system_prompt(&db).map_err(internal)?;
        let conversation = match &req.conversation_id {
            Some(id) => db.get_conversation(&auth.user_id, id).map_err(internal)?.ok_or(StatusCode::NOT_FOUND)?.id,
            None => db.create_conversation(&auth.user_id, &crate::make_title(&text)).map_err(internal)?,
        };
        // Save the question first so it survives a failed request.
        let user_message_id = db.add_message(&auth.user_id, &conversation, "user", &text).map_err(internal)?;
        let history: Vec<ChatMessage> = db
            .get_messages(&auth.user_id, &conversation)
            .map_err(internal)?
            .iter()
            .map(|m| ChatMessage::new(&m.role, &m.content))
            .collect();
        (provider, system, conversation, history, user_message_id)
    };

    let (_id, live) = generation::start(
        &state,
        generation::Start {
            user_id: auth.user_id,
            device_id: auth.device_id,
            conversation_id: conversation.clone(),
            user_message_id: Some(user_message_id),
            prompt: text,
            provider_id: req.provider,
            provider,
            system,
            history,
        },
    )
    .map_err(internal)?;
    let meta = futures_util::stream::once(async move { Ok(sse_event(&ChatEvent::Meta { conversation_id: conversation })) });
    let events = live.subscribe(0).map(|e| {
        Ok(sse_event(&match e {
            GenEvent::Delta { text, .. } => ChatEvent::Delta { text },
            GenEvent::Done => ChatEvent::Done,
            GenEvent::Error { message } => ChatEvent::Error { message },
        }))
    });
    Ok(Sse::new(meta.chain(events)).keep_alive(KeepAlive::default()))
}
