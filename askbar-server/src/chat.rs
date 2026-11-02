//! `POST /api/v1/chat`: start a generation and stream its reply as our own SSE events.
//!
//! Provider output is never passed through; it is decoded by `llm`
//! and re-emitted as `meta`, `delta`, `done` or `error`. The generation runs as its own task
//! (see `generation`), so this stream going away does not stop it.

use crate::generation::{self, GenEvent};
use crate::{auth::DeviceAuth, AppState};
use askbar_proto::{ChatEvent, ChatRequest};
use axum::{
    response::IntoResponse,
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
        ChatEvent::Provider { .. } => "provider",
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
) -> Result<impl axum::response::IntoResponse, axum::response::Response> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }

    // A repeat of a send this device already made attaches to that generation and starts nothing.
    if let Some(rid) = req.client_request_id.as_deref() {
        let existing = state
            .db
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .find_by_request_id(&auth.device_id, rid)
            .map_err(|e| internal(e).into_response())?;
        if let Some(g) = existing {
            let meta = ChatEvent::Meta {
                conversation_id: g.conversation_id.clone(),
                generation_id: Some(g.id.clone()),
                queued: g.status == "queued",
            };
            let events = futures_util::stream::once(async move { Ok(sse_event(&meta)) }).chain(generation::events_for(&state, &g, 0));
            return Ok(Sse::new(Box::pin(events) as generation::EventStream).keep_alive(KeepAlive::default()));
        }
    }

    let sub = generation::submit(
        &state,
        generation::Submit {
            user_id: auth.user_id,
            device_id: auth.device_id,
            conversation_id: req.conversation_id,
            provider_id: req.provider,
            prompt: text,
            client_request_id: req.client_request_id,
        },
    )
    .map_err(IntoResponse::into_response)?;
    let meta = ChatEvent::Meta {
        conversation_id: sub.conversation_id,
        generation_id: Some(sub.generation_id),
        queued: sub.queued,
    };
    let meta = futures_util::stream::once(async move { Ok(sse_event(&meta)) });
    let events = sub.live.subscribe(0).map(|e| {
        Ok(match e {
            GenEvent::Provider { id, name } => sse_event(&ChatEvent::Provider { id, name }),
            GenEvent::Delta { seq, text } => sse_event_with_id(&ChatEvent::Delta { text }, Some(seq)),
            GenEvent::Done => sse_event(&ChatEvent::Done),
            GenEvent::Error { message } => sse_event(&ChatEvent::Error { message }),
        })
    });
    Ok(Sse::new(Box::pin(meta.chain(events)) as generation::EventStream).keep_alive(KeepAlive::default()))
}
