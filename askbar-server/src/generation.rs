//! Generations: chat turns that run as server tasks and outlive the client that started them.
//!
//! This module holds the in-memory registry: each running generation has a `Live` that buffers
//! its deltas so a client can attach, drop and attach again without losing or repeating text.

use crate::llm::{stream_chat_report, ChatMessage, StreamOutcome, Usage};
use crate::auth::DeviceAuth;
use crate::chat::{sse_event, sse_event_with_id};
use crate::audit::{Kind, SpanFinish, SpanStart};
use crate::db::NewGeneration;
use crate::store;
use crate::AppState;
use askbar_proto::ChatEvent;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
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
    /// The audit span that times a queued message's wait, until it starts (or is removed).
    queue_span: Mutex<Option<String>>,
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

    pub fn set_queue_span(&self, id: String) {
        *self.queue_span.lock().unwrap_or_else(|e| e.into_inner()) = Some(id);
    }

    /// Ends the wait span, if there is one, with `status`.
    pub fn end_queue_span(&self, audit: &crate::audit::Recorder, status: &str) {
        if let Some(id) = self.queue_span.lock().unwrap_or_else(|e| e.into_inner()).take() {
            audit.finish_span(SpanFinish::new(&id, status));
        }
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

    /// Everything pushed so far, joined.
    pub fn text(&self) -> String {
        self.lock().deltas.concat()
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
    /// Set when the server is shutting down: nothing new starts.
    stopping: std::sync::atomic::AtomicBool,
}

impl Generations {
    pub fn is_stopping(&self) -> bool {
        self.stopping.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The generations that have not finished, by id.
    pub fn unfinished(&self) -> Vec<(String, Arc<Live>)> {
        self.map().iter().filter(|(_, l)| !l.is_finished()).map(|(k, l)| (k.clone(), l.clone())).collect()
    }

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

/// A chat send, as the handler received it.
pub struct Submit {
    pub user_id: String,
    pub device_id: String,
    pub conversation_id: Option<String>,
    pub provider_id: String,
    pub prompt: String,
    pub client_request_id: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SubmitError {
    /// The conversation or provider does not exist (or is someone else's).
    NotFound,
    /// The conversation already has `MAX_QUEUED` messages waiting.
    QueueFull,
    /// The audit writer is not keeping up: nothing new starts until it catches up.
    AuditBacklog,
    /// The administrator has not made an audit key yet, so nothing could be kept sealed.
    AuditKeyMissing,
    Internal,
}

/// How long a client is told to wait before sending again when the audit store is backed up.
pub const AUDIT_BACKLOG_RETRY_SECS: u64 = 5;

impl IntoResponse for SubmitError {
    fn into_response(self) -> axum::response::Response {
        match self {
            SubmitError::NotFound => StatusCode::NOT_FOUND.into_response(),
            SubmitError::QueueFull => StatusCode::CONFLICT.into_response(),
            SubmitError::Internal => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            SubmitError::AuditKeyMissing => (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(serde_json::json!({ "error": "audit_key_missing" })),
            )
                .into_response(),
            SubmitError::AuditBacklog => (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::RETRY_AFTER, AUDIT_BACKLOG_RETRY_SECS.to_string())],
                axum::Json(serde_json::json!({ "error": "audit_backlog" })),
            )
                .into_response(),
        }
    }
}

pub struct Submitted {
    pub generation_id: String,
    pub conversation_id: String,
    pub live: Arc<Live>,
    /// Waiting behind a reply that is still running; its question is not stored yet.
    pub queued: bool,
}

/// Messages that may wait behind a running reply in one conversation.
pub const MAX_QUEUED: i64 = 5;

/// How a generation's request ended, which decides the stored status.
enum Outcome {
    Done,
    Cancelled,
    /// Ran out of time; carries the message subscribers see.
    TimedOut(String),
    Error(String),
}

/// What an attempt span records beyond its columns: the whole latency, the HTTP status, whether
/// the counts were estimated, and why the provider says it stopped.
fn attempt_attrs(r: &crate::llm::StreamReport) -> serde_json::Value {
    serde_json::json!({
        "latency_ms": r.total.as_millis() as u64,
        "http_status": r.http_status,
        "usage_estimated": r.usage.map(|u| u.estimated),
        "finish_reason": r.finish_reason,
    })
}

/// A short stable code for why a request failed, for the audit trail.
fn error_code(e: &crate::llm::ChatError) -> String {
    use crate::llm::ChatError;
    match e {
        ChatError::Config(_) => "config".into(),
        ChatError::Connect { .. } => "connect".into(),
        ChatError::Api { status, .. } => format!("http_{status}"),
        ChatError::Stream(_) => "stream".into(),
    }
}

/// "Timed out after 5 minutes", or in seconds when the limit is not a whole number of minutes.
fn timed_out_message(timeout: Duration) -> String {
    let secs = timeout.as_secs() + u64::from(timeout.subsec_nanos() > 0);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{}", if n == 1 { "" } else { "s" });
    if secs >= 60 && secs % 60 == 0 {
        format!("Timed out after {}", plural(secs / 60, "minute"))
    } else {
        format!("Timed out after {}", plural(secs, "second"))
    }
}

/// Owned by the task. However the task ends (finished, failed, panicked), the reply is saved
/// once and subscribers are released.
struct Finisher {
    state: Arc<AppState>,
    live: Arc<Live>,
    id: String,
    user_id: String,
    device_id: Option<String>,
    provider_id: String,
    conversation_id: String,
    text: String,
    /// What the provider (or an estimate) says the reply used; recorded when the row ends.
    usage: Option<Usage>,
    /// The audit span for this generation as a whole, and why it failed if it did.
    turn_span: String,
    error_code: Option<String>,
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
        // Something else may have ended this generation already (shutdown, or the conversation
        // was deleted); then the reply has been dealt with and must not be written twice.
        let row_status = db.get_generation(&self.user_id, &self.id).ok().flatten().map(|g| g.status);
        if row_status.as_deref() != Some("running") {
            // Its row ended some other way; the audit trail says the same.
            self.finish_turn_span(row_status.as_deref().unwrap_or("cancelled"));
            return;
        }
        let message_id = if self.text.trim().is_empty() {
            None
        } else {
            db.add_message(&self.user_id, &self.conversation_id, "assistant", &self.text).ok()
        };
        let _ = db.finalize_generation(&self.id, status, &self.text, error, message_id.as_deref());
        self.finish_turn_span(status);
        // One usage row per generation that has numbers, whatever way it ended.
        if let Some(u) = self.usage {
            let _ = db.conn().execute(
                "INSERT INTO token_usage (id, user_id, device_id, provider_id, tokens_in, tokens_out, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    crate::ids::new_id(),
                    self.user_id,
                    self.device_id,
                    self.provider_id,
                    u.input.unwrap_or(0) as i64,
                    u.output.unwrap_or(0) as i64,
                    crate::auth::now()
                ],
            );
        }
    }

    fn finish_turn_span(&self, status: &str) {
        let mut f = SpanFinish::new(&self.turn_span, status);
        f.error_code = self.error_code.clone();
        if let Some(u) = self.usage {
            f.tokens_in = u.input;
            f.tokens_out = u.output;
            f.tokens_cached = u.cache_read;
        }
        self.state.audit.finish_span(f);
    }

    /// Saves the reply, then tells subscribers, so a client that has seen the end can read the message.
    fn finish(&mut self, outcome: Outcome) {
        match &outcome {
            Outcome::Done => self.save("done", None),
            Outcome::Cancelled => self.save("cancelled", None),
            Outcome::TimedOut(m) => self.save("timed_out", Some(m)),
            Outcome::Error(m) => self.save("error", Some(m)),
        }
        self.live.finish(match outcome {
            Outcome::Done | Outcome::Cancelled => Terminal::Done,
            Outcome::TimedOut(m) | Outcome::Error(m) => Terminal::Error(m),
        });
    }
}

impl Drop for Finisher {
    fn drop(&mut self) {
        self.save("error", Some("The reply stopped unexpectedly."));
        // A no-op if `finish` already ran; otherwise the task died and subscribers must not wait forever.
        self.live.finish(Terminal::Error("The reply stopped unexpectedly.".into()));
        // The row is final and the reply saved: the next message waiting in this conversation goes.
        start_next(&self.state, &self.conversation_id);
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

/// How long a generation that starts now may run: the test override if one is set, else the
/// current snapshot's setting, else the default (no snapshot yet).
fn timeout_now(state: &AppState, db: &crate::Db) -> Duration {
    if let Some(d) = state.generation_timeout_override() {
        return d;
    }
    let secs = crate::snapshot::current(db)
        .ok()
        .flatten()
        .map(|s| s.generation_timeout_secs)
        .unwrap_or(askbar_proto::DEFAULT_GENERATION_TIMEOUT_SECS);
    Duration::from_secs(secs as u64)
}

/// Writes the row's deadline: when it started, plus `timeout` (whole seconds, rounded up). Done
/// in SQL so `deadline_at - started_at` is exactly the timeout, whatever second it is.
fn stamp_deadline(db: &crate::Db, id: &str, timeout: Duration) {
    let secs = timeout.as_secs() + u64::from(timeout.subsec_nanos() > 0);
    let _ = db.conn().execute("UPDATE generations SET deadline_at = started_at + ?2 WHERE id = ?1", rusqlite::params![id, secs as i64]);
}

/// Starts a send: runs it now if its conversation is idle, else queues it. Everything that can
/// fail with a plain HTTP status happens here, before any stream starts.
pub fn submit(state: &Arc<AppState>, s: Submit) -> Result<Submitted, SubmitError> {
    // Generations already running finish; new ones wait until the audit store has caught up.
    if !state.audit.healthy() {
        return Err(SubmitError::AuditBacklog);
    }
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    fn internal<E>(_: E) -> SubmitError {
        SubmitError::Internal
    }
    store::load_provider(&db, &state.secrets(), &s.provider_id).map_err(internal)?.ok_or(SubmitError::NotFound)?;
    // Nothing runs unless what it says can be kept sealed: no audit key, no generation.
    if crate::snapshot::current(&db).map_err(internal)?.and_then(|snap| snap.audit_public_key).is_none() {
        return Err(SubmitError::AuditKeyMissing);
    }
    let (conversation_id, busy) = match &s.conversation_id {
        Some(id) => {
            let c = db.get_conversation(&s.user_id, id).map_err(internal)?.ok_or(SubmitError::NotFound)?;
            let busy = db.conversation_busy(&c.id).map_err(internal)?;
            (c.id, busy)
        }
        None => (db.create_conversation(&s.user_id, &crate::make_title(&s.prompt)).map_err(internal)?, false),
    };
    let new = |status, user_message_id| NewGeneration {
        user_id: &s.user_id,
        device_id: Some(&s.device_id),
        conversation_id: &conversation_id,
        user_message_id,
        client_request_id: s.client_request_id.as_deref(),
        provider_id: Some(&s.provider_id),
        status,
        prompt: &s.prompt,
        deadline_at: None,
    };
    if busy {
        if db.queued_count(&conversation_id).map_err(internal)? >= MAX_QUEUED {
            return Err(SubmitError::QueueFull);
        }
        // Queued: the question is stored when the message starts, not now.
        let id = db.create_generation(&new("queued", None)).map_err(internal)?;
        let live = Live::new();
        live.set_queue_span(state.audit.start_span(SpanStart::new(&id, Kind::Gate, "queue").who(&s.user_id, Some(&s.device_id))));
        state.generations.insert(&id, live.clone());
        return Ok(Submitted { generation_id: id, conversation_id, live, queued: true });
    }
    // Save the question first so it survives a failed request.
    let user_message_id = db.add_message(&s.user_id, &conversation_id, "user", &s.prompt).map_err(internal)?;
    let id = db.create_generation(&new("running", Some(&user_message_id))).map_err(internal)?;
    let timeout = timeout_now(state, &db);
    stamp_deadline(&db, &id, timeout);
    let live = Live::new();
    state.generations.insert(&id, live.clone());
    drop(db);
    spawn_run(state, &id, &s.user_id, Some(&s.device_id), &conversation_id, &s.provider_id, timeout, live.clone());
    Ok(Submitted { generation_id: id, conversation_id, live, queued: false })
}

/// Runs generation `id` as a task that does not depend on any client. The provider, system prompt
/// and history are read when it starts, so a message that waited sees the replies before it.
#[allow(clippy::too_many_arguments)]
fn spawn_run(
    state: &Arc<AppState>,
    id: &str,
    user_id: &str,
    device_id: Option<&str>,
    conversation_id: &str,
    provider_id: &str,
    timeout: Duration,
    live: Arc<Live>,
) {
    let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
    live.set_cancel(cancel_tx);
    let turn_span = state.audit.start_span(
        SpanStart::new(id, Kind::Turn, "chat").who(user_id, device_id).attrs(&serde_json::json!({ "conversation_id": conversation_id })),
    );
    let mut fin = Finisher {
        state: state.clone(),
        live,
        turn_span,
        error_code: None,
        id: id.to_string(),
        user_id: user_id.to_string(),
        device_id: device_id.map(str::to_string),
        provider_id: provider_id.to_string(),
        conversation_id: conversation_id.to_string(),
        text: String::new(),
        usage: None,
        saved: false,
        last_flush: Instant::now(),
        unflushed: 0,
    };
    let (state, provider_id) = (state.clone(), provider_id.to_string());
    tokio::spawn(async move {
        let prepared = {
            let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let provider = store::load_provider(&db, &state.secrets(), &provider_id).ok().flatten();
            let system = store::chat_system_prompt(&db).ok();
            let history: Option<Vec<ChatMessage>> = db
                .get_messages(&fin.user_id, &fin.conversation_id)
                .ok()
                .map(|ms| ms.iter().map(|m| ChatMessage::new(&m.role, &m.content)).collect());
            provider.zip(system).zip(history)
        };
        let Some(((provider, system), history)) = prepared else {
            fin.finish(Outcome::Error("The provider is no longer available.".into()));
            return;
        };
        // One attempt span for the request to the provider, under the turn.
        let attempt = state.audit.start_span(
            SpanStart::new(&fin.id, Kind::Attempt, &provider.name).parent(&fin.turn_span).provider(&provider_id, &provider.model),
        );
        // Race the request against the deadline. Dropping the request on expiry closes the
        // upstream connection; the text that had arrived is already in `fin`.
        let result = {
            let run = stream_chat_report(&state.http, &provider, &system, &history, cancel_rx, |delta| fin.push(delta));
            tokio::pin!(run);
            tokio::select! {
                r = &mut run => Some(r),
                _ = tokio::time::sleep(timeout) => None,
            }
        };
        let mut attempt_end = SpanFinish::new(&attempt, "done");
        match &result {
            Some(Ok(r)) => {
                attempt_end.status = match r.outcome {
                    StreamOutcome::Completed => "done",
                    StreamOutcome::Cancelled => "cancelled",
                }
                .into();
                attempt_end.ttft_ms = r.time_to_first_token.map(|d| d.as_millis() as u64);
                attempt_end.attrs_json = Some(attempt_attrs(r).to_string());
            }
            Some(Err(e)) => {
                attempt_end.status = "error".into();
                attempt_end.error_code = Some(error_code(&e.error));
                attempt_end.ttft_ms = e.report.time_to_first_token.map(|d| d.as_millis() as u64);
                attempt_end.attrs_json = Some(attempt_attrs(&e.report).to_string());
            }
            None => {
                attempt_end.status = "timed_out".into();
                attempt_end.error_code = Some("timeout".into());
            }
        }
        let used = match &result {
            Some(Ok(r)) => r.usage,
            Some(Err(e)) => e.report.usage,
            None => None,
        };
        if let Some(u) = used {
            attempt_end.tokens_in = u.input;
            attempt_end.tokens_out = u.output;
            attempt_end.tokens_cached = u.cache_read;
        }
        state.audit.finish_span(attempt_end);
        let (outcome, usage) = match result {
            Some(Ok(report)) => (
                match report.outcome {
                    StreamOutcome::Completed => Outcome::Done,
                    StreamOutcome::Cancelled => Outcome::Cancelled,
                },
                report.usage,
            ),
            Some(Err(e)) => {
                fin.error_code = Some(error_code(&e.error));
                (Outcome::Error(e.error.to_string()), e.report.usage)
            }
            None => {
                fin.error_code = Some("timeout".into());
                (Outcome::TimedOut(timed_out_message(timeout)), None)
            }
        };
        fin.usage = usage;
        fin.finish(outcome);
    });
}

/// Starts the oldest message waiting in `conversation_id`, if any: its question is stored now, so
/// the stored order is question, reply, question, reply, and its history includes the replies before it.
fn start_next(state: &Arc<AppState>, conversation_id: &str) {
    // Starting spawns a task, which only works inside the runtime.
    if tokio::runtime::Handle::try_current().is_err() || state.generations.is_stopping() {
        return;
    }
    let started = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(Some(next)) = db.next_queued(conversation_id) else { return };
        match db.add_message(&next.user_id, &next.conversation_id, "user", &next.prompt) {
            Ok(message_id) => {
                let _ = db.start_generation(&next.id, &message_id, None);
                // A queued message's clock starts now, not when it was queued.
                let timeout = timeout_now(state, &db);
                stamp_deadline(&db, &next.id, timeout);
                Some((next, timeout))
            }
            Err(_) => {
                // The conversation is gone or unwritable: the message cannot run.
                let _ = db.finalize_generation(&next.id, "error", "", Some("The conversation is no longer available."), None);
                None
            }
        }
    };
    match started {
        Some((next, timeout)) => {
            let live = state.generations.get(&next.id).unwrap_or_else(|| {
                let l = Live::new();
                state.generations.insert(&next.id, l.clone());
                l
            });
            live.end_queue_span(&state.audit, "done");
            spawn_run(state, &next.id, &next.user_id, next.device_id.as_deref(), &next.conversation_id, next.provider_id.as_deref().unwrap_or(""), timeout, live);
        }
        None => {
            // Try the one after it.
            start_next(state, conversation_id);
        }
    }
}

const INTERRUPTED: &str = "The server restarted before the reply finished.";
const NOT_RUN: &str = "Not run: the server restarted before this message started.";

/// At start-up: no task from a previous run exists, so any `running` row was cut off. Its partial
/// text becomes the assistant message; a `queued` row never ran and gets no message. Returns
/// how many of each it fixed.
pub fn sweep_on_start(db: &crate::Db) -> Result<(usize, usize), crate::DbError> {
    let rows: Vec<(String, String, String, String, String)> = {
        let mut stmt = db
            .conn()
            .prepare("SELECT id, user_id, conversation_id, status, text FROM generations WHERE status IN ('running','queued') ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?;
        rows.collect::<Result<_, _>>()?
    };
    let (mut running, mut queued) = (0, 0);
    for (id, user_id, conversation_id, status, text) in rows {
        if status == "queued" {
            db.finalize_generation(&id, "interrupted", "", Some(NOT_RUN), None)?;
            queued += 1;
        } else {
            let message_id = if text.trim().is_empty() {
                None
            } else {
                db.add_message(&user_id, &conversation_id, "assistant", &text).ok()
            };
            db.finalize_generation(&id, "interrupted", &text, Some(INTERRUPTED), message_id.as_deref())?;
            running += 1;
        }
    }
    Ok((running, queued))
}

/// On SIGTERM: save what each running reply has so far and mark it `interrupted`; waiting messages
/// never run. Upstream requests are stopped, and nothing new starts.
pub fn shutdown(state: &Arc<AppState>) {
    state.generations.stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    for (id, live) in state.generations.unfinished() {
        let text = live.text();
        {
            let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
            use rusqlite::OptionalExtension;
            let row: Option<(String, String, String)> = db
                .conn()
                .query_row("SELECT user_id, conversation_id, status FROM generations WHERE id = ?1", [&id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()
                .ok()
                .flatten();
            if let Some((user_id, conversation_id, status)) = row {
                if status == "running" {
                    let message_id = if text.trim().is_empty() {
                        None
                    } else {
                        db.add_message(&user_id, &conversation_id, "assistant", &text).ok()
                    };
                    let _ = db.finalize_generation(&id, "interrupted", &text, Some(INTERRUPTED), message_id.as_deref());
                } else {
                    let _ = db.finalize_generation(&id, "interrupted", "", Some(NOT_RUN), None);
                }
            }
        }
        live.end_queue_span(&state.audit, "interrupted");
        live.finish(Terminal::Error(INTERRUPTED.into()));
        live.cancel();
    }
}

/// Deletes a conversation for `user_id`. Its queue is cleared and its running reply is told to
/// stop first; their rows go with the conversation, and nothing is started in its place.
/// False if there is no such conversation.
pub fn delete_conversation(state: &Arc<AppState>, user_id: &str, conversation_id: &str) -> Result<bool, crate::DbError> {
    let (deleted, active) = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let active: Vec<_> =
            db.list_active_generations(user_id)?.into_iter().filter(|g| g.conversation_id == conversation_id).collect();
        // Queued messages never ran; ending them under this lock keeps the runner from starting one.
        for g in active.iter().filter(|g| g.status == "queued") {
            db.finalize_generation(&g.id, "cancelled", "", None, None)?;
        }
        (db.delete_conversation(user_id, conversation_id)?, active)
    };
    for g in active {
        let Some(live) = state.generations.get(&g.id) else { continue };
        if g.status == "queued" {
            live.end_queue_span(&state.audit, "cancelled");
            live.finish(Terminal::Done);
            state.generations.remove(&g.id);
        } else {
            live.cancel();
        }
    }
    Ok(deleted)
}

/// `POST /api/v1/generations/{id}/cancel`: stop a running reply, keeping the text so far.
/// 204 also when it has already finished; 404 for anyone else's.
pub async fn cancel(
    State(state): State<Arc<AppState>>,
    Extension(auth): Extension<DeviceAuth>,
    Path(id): Path<String>,
) -> StatusCode {
    // Under one lock, so a queued message cannot start between the look and the cancel.
    let queued = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        match db.get_generation(&auth.user_id, &id) {
            Ok(Some(g)) if g.status == "queued" => {
                // Never ran: nothing was stored for it, and nothing is written now.
                let _ = db.finalize_generation(&id, "cancelled", "", None, None);
                true
            }
            Ok(Some(_)) => false,
            Ok(None) => return StatusCode::NOT_FOUND,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
        }
    };
    if let Some(live) = state.generations.get(&id) {
        if queued {
            live.end_queue_span(&state.audit, "cancelled");
            live.finish(Terminal::Done);
            state.generations.remove(&id);
        } else {
            live.cancel();
        }
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

    #[test]
    fn the_timeout_message_is_in_minutes_when_whole_and_in_seconds_otherwise() {
        let m = |s: u64| timed_out_message(Duration::from_secs(s));
        assert_eq!(m(300), "Timed out after 5 minutes");
        assert_eq!(m(60), "Timed out after 1 minute");
        assert_eq!(m(3600), "Timed out after 60 minutes");
        assert_eq!(m(90), "Timed out after 90 seconds");
        assert_eq!(m(2), "Timed out after 2 seconds");
        assert_eq!(m(1), "Timed out after 1 second");
        assert_eq!(timed_out_message(Duration::from_millis(1500)), "Timed out after 2 seconds");
    }

    #[test]
    fn with_no_snapshot_the_timeout_is_300_seconds_and_the_override_wins() {
        let state = crate::testing::state_for_tests();
        let db = state.db.lock().unwrap();
        assert!(crate::snapshot::current(&db).unwrap().is_none());
        assert_eq!(timeout_now(&state, &db), Duration::from_secs(300));
        crate::testing::update_snapshot(&db, |s| s.generation_timeout_secs = 45);
        assert_eq!(timeout_now(&state, &db), Duration::from_secs(45));
        state.set_generation_timeout_override(Some(Duration::from_millis(1500)));
        assert_eq!(timeout_now(&state, &db), Duration::from_millis(1500));
    }

    #[test]
    fn the_start_up_sweep_interrupts_running_and_queued_rows() {
        let db = crate::Db::open_in_memory().unwrap();
        let u = db.seed_server().unwrap();
        let c = db.create_conversation(&u, "c").unwrap();
        let mk = |status: &str| {
            db.create_generation(&NewGeneration {
                user_id: &u,
                device_id: None,
                conversation_id: &c,
                user_message_id: None,
                client_request_id: None,
                provider_id: None,
                status,
                prompt: "q",
                deadline_at: None,
            })
            .unwrap()
        };
        let (with_text, empty, queued, done) = (mk("running"), mk("running"), mk("queued"), mk("running"));
        db.flush_generation_text(&with_text, "partial answer").unwrap();
        db.finalize_generation(&done, "done", "all of it", None, None).unwrap();

        assert_eq!(sweep_on_start(&db).unwrap(), (2, 1));
        let get = |id: &str| db.get_generation(&u, id).unwrap().unwrap();
        let g = get(&with_text);
        assert_eq!((g.status.as_str(), g.text.as_str()), ("interrupted", "partial answer"));
        assert!(g.assistant_message_id.is_some() && g.ended_at.is_some());
        assert!(g.error.unwrap().contains("restarted"));
        let g = get(&empty);
        assert_eq!((g.status.as_str(), g.assistant_message_id), ("interrupted", None), "no text, no message");
        let g = get(&queued);
        assert_eq!((g.status.as_str(), g.assistant_message_id), ("interrupted", None));
        assert!(g.error.unwrap().contains("Not run"));
        assert_eq!(get(&done).status, "done", "finished rows are left alone");
        let messages = db.get_messages(&u, &c).unwrap();
        assert_eq!(messages.len(), 1, "only the partial text became a message");
        assert_eq!((messages[0].role.as_str(), messages[0].content.as_str()), ("assistant", "partial answer"));
        assert_eq!(sweep_on_start(&db).unwrap(), (0, 0), "a second sweep finds nothing");
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
