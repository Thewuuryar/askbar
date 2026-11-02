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
use std::sync::Arc;
use tokio_stream::StreamExt;

fn internal<E>(_: E) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}

pub(crate) fn sse_event(e: &ChatEvent) -> Event {
    sse_event_with_id(e, None)
}

/// Deltas carry `id: <seq>` so a client can resume with `?after=<last id>`.
pub(crate) fn sse_event_with_id(e: &ChatEvent, seq: Option<u64>) -> Event {
    let name = match e {
        ChatEvent::Meta { .. } => "meta",
        ChatEvent::Delta { .. } => "delta",
        ChatEvent::Done => "done",
        ChatEvent::Error { .. } => "error",
    };
    let ev = Event::default().event(name).json_data(e).expect("ChatEvent serializes");
    match seq {
        Some(n) => ev.id(n.to_string()),
        None => ev,
    }
}

pub async fn chat(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<DeviceAuth>,
    Json(req): Json<ChatRequest>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // A repeat of a send this device already made attaches to that generation and starts nothing.
    if let Some(rid) = req.client_request_id.as_deref() {
        let existing = state
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .find_by_request_id(&auth.device_id, rid)
            .map_err(internal)?;
        if let Some(g) = existing {
            let meta = ChatEvent::Meta { conversation_id: g.conversation_id.clone(), generation_id: Some(g.id.clone()) };
            let events = futures_util::stream::once(async move { Ok(sse_event(&meta)) }).chain(generation::events_for(&state, &g, 0));
            return Ok(Sse::new(Box::pin(events) as generation::EventStream).keep_alive(KeepAlive::default()));
        }
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

    let (generation_id, live) = generation::start(
        &state,
        generation::Start {
            user_id: auth.user_id,
            device_id: auth.device_id,
            conversation_id: conversation.clone(),
            user_message_id: Some(user_message_id),
            prompt: text,
            provider_id: req.provider,
            client_request_id: req.client_request_id,
            provider,
            system,
            history,
        },
    )
    .map_err(internal)?;
    let meta = futures_util::stream::once(async move {
        Ok(sse_event(&ChatEvent::Meta { conversation_id: conversation, generation_id: Some(generation_id) }))
    });
    let events = live.subscribe(0).map(|e| {
        Ok(match e {
            GenEvent::Delta { seq, text } => sse_event_with_id(&ChatEvent::Delta { text }, Some(seq)),
            GenEvent::Done => sse_event(&ChatEvent::Done),
            GenEvent::Error { message } => sse_event(&ChatEvent::Error { message }),
        })
    });
    Ok(Sse::new(Box::pin(meta.chain(events)) as generation::EventStream).keep_alive(KeepAlive::default()))
}
