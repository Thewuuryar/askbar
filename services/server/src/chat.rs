//! `POST /api/client/v1/spaces/{space_id}/chat`: start a generation in a space and stream its reply
//! as our own SSE events.
//!
//! Provider output is never passed through; it is decoded by `llm`
//! and re-emitted as `meta`, `delta`, `done` or `error`. The generation runs as its own task
//! (see `generation`), so this stream going away does not stop it.

use crate::error::{ApiError, ApiJson};
use crate::generation::{self, GenEvent};
use crate::{auth::ClientAuth, client_api::MemberSpace, AppState};
use askbar_proto::problem::ErrorCode;
use askbar_proto::{ChatEvent, ChatRequest};
use axum::{
    extract::State,
    response::sse::{Event, KeepAlive, Sse},
    response::IntoResponse,
    Extension,
};
use std::sync::Arc;
use tokio_stream::StreamExt;

fn internal<E>(_: E) -> ApiError {
    ApiError::code(ErrorCode::Internal)
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
    Extension(auth): Extension<ClientAuth>,
    MemberSpace(space): MemberSpace,
    ApiJson(req): ApiJson<ChatRequest>,
) -> Result<impl axum::response::IntoResponse, axum::response::Response> {
    let text = req.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::new(ErrorCode::BadRequest, "The message is empty.").into_response());
    }

    // A repeat of a send this client already made in this space attaches to that generation and starts nothing.
    if let Some(rid) = req.client_request_id.as_deref() {
        let existing = space.lock().find_by_request_id(&auth.client_id, rid).map_err(|e| internal(e).into_response())?;
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
            space,
            user_id: auth.user_id,
            client_id: auth.client_id,
            conversation_id: req.conversation_id,
            provider_id: req.provider,
            prompt: text,
            client_request_id: req.client_request_id,
        },
    )
    .map_err(IntoResponse::into_response)?;
    let meta = ChatEvent::Meta { conversation_id: sub.conversation_id, generation_id: Some(sub.generation_id), queued: sub.queued };
    let meta = futures_util::stream::once(async move { Ok(sse_event(&meta)) });
    let events = sub.live.subscribe(0).map(|e| {
        Ok(match e {
            GenEvent::Provider { id, name } => sse_event(&ChatEvent::Provider { id, name }),
            GenEvent::Delta { seq, text } => sse_event_with_id(&ChatEvent::Delta { text }, Some(seq)),
            GenEvent::Done => sse_event(&ChatEvent::Done),
            GenEvent::Error { message, code } => sse_event(&ChatEvent::Error { message, code: Some(code.into()) }),
        })
    });
    Ok(Sse::new(Box::pin(meta.chain(events)) as generation::EventStream).keep_alive(KeepAlive::default()))
}
