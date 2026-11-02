//! Generations: chat turns that run as server tasks and outlive the client that started them.
//!
//! This module holds the in-memory registry: each running generation has a `Live` that buffers
//! its deltas so a client can attach, drop and attach again without losing or repeating text.

use crate::llm::{stream_chat, ChatMessage, Provider, StreamOutcome};
use crate::auth::DeviceAuth;
use crate::chat::{sse_event, sse_event_with_id};
use crate::db::{DbError, NewGeneration};
use crate::AppState;
use askbar_proto::ChatEvent;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Extension,
};
use futures_util::StreamExt;
use std::convert::Infallible;
use futures_util::Stream;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};

/// What a subscriber sees. `seq` counts deltas from 1, and is what SSE `id:` carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenEvent {
    Delta { seq: u64, text: String },
    Done,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    Done,
    Error(String),
}

#[derive(Default)]
struct Inner {
    deltas: Vec<String>,
    terminal: Option<Terminal>,
}

/// One running generation's buffered output.
#[derive(Default)]
pub struct Live {
    inner: Mutex<Inner>,
    notify: Notify,
    /// Fires the running request's cancel channel; taken on first use.
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl Live {
    pub fn new() -> Arc<Live> {
        Arc::new(Live::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Appends a delta. Ignored once the generation has finished.
    pub fn push(&self, text: &str) {
        {
            let mut g = self.lock();
            if g.terminal.is_some() {
                return;
            }
            g.deltas.push(text.to_string());
        }
        self.notify.notify_waiters();
    }

    /// Ends the generation; the first call wins.
    pub fn finish(&self, terminal: Terminal) {
        {
            let mut g = self.lock();
            if g.terminal.is_some() {
                return;
            }
            g.terminal = Some(terminal);
        }
        self.notify.notify_waiters();
    }

    pub fn set_cancel(&self, tx: oneshot::Sender<()>) {
        *self.cancel.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    }

    /// Asks the generation to stop. True if it was still running and has been told.
    pub fn cancel(&self) -> bool {
        if self.is_finished() {
            return false;
        }
        match self.cancel.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(tx) => tx.send(()).is_ok(),
            None => false,
        }
    }

    pub fn delta_count(&self) -> u64 {
        self.lock().deltas.len() as u64
    }

    pub fn is_finished(&self) -> bool {
        self.lock().terminal.is_some()
    }

    /// Deltas after the first `after` of them, then live ones, then the end. Starting after
    /// the end of a finished generation yields just the terminal event.
    pub fn subscribe(self: &Arc<Self>, after: u64) -> impl Stream<Item = GenEvent> + Send + 'static {
        struct State {
            live: Arc<Live>,
            next: u64,
            ended: bool,
        }
        futures_util::stream::unfold(State { live: self.clone(), next: after, ended: false }, |mut s| async move {
            if s.ended {
                return None;
            }
            let live = s.live.clone();
            loop {
                // Register for wake-ups before looking, so a push between the look and the wait is not missed.
                let notified = live.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let step = {
                    let g = live.lock();
                    if (s.next as usize) < g.deltas.len() {
                        Some(GenEvent::Delta { seq: s.next + 1, text: g.deltas[s.next as usize].clone() })
                    } else {
                        g.terminal.as_ref().map(|t| match t {
                            Terminal::Done => GenEvent::Done,
                            Terminal::Error(m) => GenEvent::Error { message: m.clone() },
                        })
                    }
                };
                match step {
                    Some(ev @ GenEvent::Delta { .. }) => {
                        s.next += 1;
                        return Some((ev, s));
                    }
                    Some(ev) => {
                        s.ended = true;
                        return Some((ev, s));
                    }
                    None => notified.await,
                }
            }
        })
    }
}

/// The running generations, by id.
#[derive(Default)]
pub struct Generations {
    live: Mutex<HashMap<String, Arc<Live>>>,
}

impl Generations {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Live>>> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn insert(&self, id: &str, live: Arc<Live>) {
        self.map().insert(id.to_string(), live);
    }

    pub fn get(&self, id: &str) -> Option<Arc<Live>> {
        self.map().get(id).cloned()
    }

    pub fn remove(&self, id: &str) {
        self.map().remove(id);
    }

    pub fn len(&self) -> usize {
        self.map().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Everything a generation needs, gathered by the handler before it starts.
pub struct Start {
    pub user_id: String,
    pub device_id: String,
    pub conversation_id: String,
    pub user_message_id: Option<String>,
    pub prompt: String,
    pub provider_id: String,
    pub client_request_id: Option<String>,
    pub provider: Provider,
    pub system: String,
    pub history: Vec<ChatMessage>,
}

/// How a generation's request ended, which decides the stored status.
enum Outcome {
    Done,
    Cancelled,
    Error(String),
}

/// Owned by the task. However the task ends (finished, failed, panicked), the reply is saved
/// once and subscribers are released.
struct Finisher {
    state: Arc<AppState>,
    live: Arc<Live>,
    id: String,
    user_id: String,
    conversation_id: String,
    text: String,
    saved: bool,
    /// Progress is flushed to the row about once a second, or every 2 KB.
    last_flush: Instant,
    unflushed: usize,
}

/// How long a finished generation's deltas stay replayable from memory.
const FINISHED_RETENTION: Duration = Duration::from_secs(300);
const FLUSH_EVERY: Duration = Duration::from_secs(1);
const FLUSH_BYTES: usize = 2048;

impl Finisher {
    fn push(&mut self, delta: &str) {
        self.text.push_str(delta);
        self.live.push(delta);
        self.unflushed += delta.len();
        if self.unflushed >= FLUSH_BYTES || self.last_flush.elapsed() >= FLUSH_EVERY {
            let db = self.state.db.lock().unwrap_or_else(|e| e.into_inner());
            let _ = db.flush_generation_text(&self.id, &self.text);
            self.last_flush = Instant::now();
            self.unflushed = 0;
        }
    }

    /// Writes the assistant message (if there is any text) and ends the row, once.
    fn save(&mut self, status: &str, error: Option<&str>) {
        if self.saved {
            return;
        }
        self.saved = true;
        let db = self.state.db.lock().unwrap_or_else(|e| e.into_inner());
        let message_id = if self.text.trim().is_empty() {
            None
        } else {
            db.add_message(&self.user_id, &self.conversation_id, "assistant", &self.text).ok()
        };
        let _ = db.finalize_generation(&self.id, status, &self.text, error, message_id.as_deref());
    }

    /// Saves the reply, then tells subscribers, so a client that has seen the end can read the message.
    fn finish(&mut self, outcome: Outcome) {
        match &outcome {
            Outcome::Done => self.save("done", None),
            Outcome::Cancelled => self.save("cancelled", None),
            Outcome::Error(m) => self.save("error", Some(m)),
        }
        self.live.finish(match outcome {
            Outcome::Done | Outcome::Cancelled => Terminal::Done,
            Outcome::Error(m) => Terminal::Error(m),
        });
    }
}

impl Drop for Finisher {
    fn drop(&mut self) {
        self.save("error", Some("The reply stopped unexpectedly."));
        // A no-op if `finish` already ran; otherwise the task died and subscribers must not wait forever.
        self.live.finish(Terminal::Error("The reply stopped unexpectedly.".into()));
        // Keep the buffer a while so a client that dropped can still resume exactly where it was.
        let (state, id) = (self.state.clone(), self.id.clone());
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move {
                    tokio::time::sleep(FINISHED_RETENTION).await;
                    state.generations.remove(&id);
                });
            }
            Err(_) => state.generations.remove(&id),
        }
    }
}

/// Runs a generation as a task that does not depend on any client. Returns its id and its buffer.
pub fn start(state: &Arc<AppState>, s: Start) -> Result<(String, Arc<Live>), DbError> {
    let id = state.db.lock().unwrap_or_else(|e| e.into_inner()).create_generation(&NewGeneration {
        user_id: &s.user_id,
        device_id: Some(&s.device_id),
        conversation_id: &s.conversation_id,
        user_message_id: s.user_message_id.as_deref(),
        client_request_id: s.client_request_id.as_deref(),
        provider_id: Some(&s.provider_id),
        status: "running",
        prompt: &s.prompt,
        deadline_at: None,
    })?;
    let live = Live::new();
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    live.set_cancel(cancel_tx);
    state.generations.insert(&id, live.clone());
    let mut fin = Finisher {
        state: state.clone(),
        live: live.clone(),
        id: id.clone(),
        user_id: s.user_id,
        conversation_id: s.conversation_id,
        text: String::new(),
        saved: false,
        last_flush: Instant::now(),
        unflushed: 0,
    };
    let state = state.clone();
    tokio::spawn(async move {
        let result = stream_chat(&state.http, &s.provider, &s.system, &s.history, cancel_rx, |delta| fin.push(delta)).await;
        fin.finish(match result {
            Ok(StreamOutcome::Completed) => Outcome::Done,
            Ok(StreamOutcome::Cancelled) => Outcome::Cancelled,
            Err(e) => Outcome::Error(e.to_string()),
        });
    });
    Ok((id, live))
}

/// `POST /api/v1/generations/{id}/cancel`: stop a running reply, keeping the text so far.
/// 204 also when it has already finished; 404 for anyone else's.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
) -> StatusCode {
    {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db.get_generation(&auth.user_id, &id) {
            Ok(Some(_)) => {}
            Ok(None) => return StatusCode::NOT_FOUND,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    if let Some(live) = state.generations.get(&id) {
        live.cancel();
    }
    StatusCode::NO_CONTENT
}

#[derive(serde::Deserialize)]
pub struct StreamQuery {
    after: Option<u64>,
}

pub(crate) type EventStream = std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>;

fn to_sse(e: GenEvent) -> Result<Event, Infallible> {
    Ok(match e {
        GenEvent::Delta { seq, text } => sse_event_with_id(&ChatEvent::Delta { text }, Some(seq)),
        GenEvent::Done => sse_event(&ChatEvent::Done),
        GenEvent::Error { message } => sse_event(&ChatEvent::Error { message }),
    })
}

/// How a finished generation ends a stream: `done` unless it failed, was cut short or timed out.
fn terminal_of(status: &str, error: Option<&str>) -> GenEvent {
    match status {
        "done" | "cancelled" => GenEvent::Done,
        "interrupted" => GenEvent::Error { message: error.unwrap_or("The server restarted before the reply finished.").into() },
        _ => GenEvent::Error { message: error.unwrap_or("The reply failed.").into() },
    }
}

/// The events of `row` after the first `after` deltas: from memory if it is there, else from the row.
pub(crate) fn events_for(state: &AppState, row: &crate::Generation, after: u64) -> EventStream {
    match state.generations.get(&row.id) {
        Some(live) => Box::pin(live.subscribe(after).map(to_sse)),
        None => {
            let mut v = Vec::new();
            if after == 0 && !row.text.is_empty() {
                v.push(GenEvent::Delta { seq: 1, text: row.text.clone() });
            }
            v.push(terminal_of(&row.status, row.error.as_deref()));
            Box::pin(futures_util::stream::iter(v.into_iter().map(to_sse)))
        }
    }
}

/// `GET /api/v1/generations/{id}/stream?after=N`: the deltas after the first N, then the rest live.
///
/// While the generation runs (and for a few minutes after) this replays from memory, exactly.
/// Once only the database remembers it, delta boundaries are gone: a client that has seen
/// nothing gets the whole text as one delta, and one that has seen some gets just the ending,
/// so it should reload the conversation to read the saved message.
pub async fn stream(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
    Query(q): Query<StreamQuery>,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    let after = q.after.unwrap_or(0);
    let row = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        db.get_generation(&auth.user_id, &id).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.ok_or(StatusCode::NOT_FOUND)?
    };
    let events = events_for(&state, &row, after);
    Ok(Sse::new(events).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use std::time::Duration;

    fn d(seq: u64, t: &str) -> GenEvent {
        GenEvent::Delta { seq, text: t.into() }
    }

    async fn collect(s: impl Stream<Item = GenEvent>) -> Vec<GenEvent> {
        tokio::time::timeout(Duration::from_secs(5), s.collect::<Vec<_>>()).await.expect("stream should end")
    }

    #[tokio::test]
    async fn replays_buffered_deltas_then_ends() {
        let live = Live::new();
        live.push("a");
        live.push("b");
        live.finish(Terminal::Done);
        assert_eq!(collect(live.subscribe(0)).await, [d(1, "a"), d(2, "b"), GenEvent::Done]);
        assert_eq!(collect(live.subscribe(1)).await, [d(2, "b"), GenEvent::Done], "after=1 skips the first");
    }

    #[tokio::test]
    async fn follows_live_deltas_until_the_end() {
        let live = Live::new();
        live.push("a");
        let sub = tokio::spawn(collect(live.subscribe(0)));
        tokio::time::sleep(Duration::from_millis(30)).await;
        live.push("b");
        tokio::time::sleep(Duration::from_millis(30)).await;
        live.push("c");
        live.finish(Terminal::Error("boom".into()));
        assert_eq!(sub.await.unwrap(), [d(1, "a"), d(2, "b"), d(3, "c"), GenEvent::Error { message: "boom".into() }]);
    }

    #[tokio::test]
    async fn a_late_subscriber_after_finish_gets_everything_once() {
        let live = Live::new();
        for t in ["x", "y", "z"] {
            live.push(t);
        }
        live.finish(Terminal::Done);
        live.push("ignored"); // nothing is added after the end
        live.finish(Terminal::Error("ignored".into())); // and the first ending wins
        assert!(live.is_finished());
        assert_eq!(live.delta_count(), 3);
        assert_eq!(collect(live.subscribe(0)).await, [d(1, "x"), d(2, "y"), d(3, "z"), GenEvent::Done]);
        assert_eq!(collect(live.subscribe(3)).await, [GenEvent::Done], "nothing left but the end");
    }

    #[tokio::test]
    async fn two_subscribers_each_see_every_delta() {
        let live = Live::new();
        let a = tokio::spawn(collect(live.subscribe(0)));
        live.push("1");
        let b = tokio::spawn(collect(live.subscribe(0)));
        tokio::time::sleep(Duration::from_millis(20)).await;
        live.push("2");
        live.finish(Terminal::Done);
        let want = [d(1, "1"), d(2, "2"), GenEvent::Done];
        assert_eq!(a.await.unwrap(), want);
        assert_eq!(b.await.unwrap(), want);
    }

    #[tokio::test]
    async fn registry_inserts_gets_and_removes() {
        let reg = Generations::default();
        assert!(reg.get("g1").is_none() && reg.is_empty());
        let live = Live::new();
        reg.insert("g1", live.clone());
        assert!(Arc::ptr_eq(&reg.get("g1").unwrap(), &live));
        assert_eq!(reg.len(), 1);
        reg.remove("g1");
        assert!(reg.get("g1").is_none());
    }
}
