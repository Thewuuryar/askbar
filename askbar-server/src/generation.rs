//! Generations: chat turns that run as server tasks and outlive the client that started them.
//!
//! This module holds the in-memory registry: each running generation has a `Live` that buffers
//! its deltas so a client can attach, drop and attach again without losing or repeating text.

use crate::llm::{stream_chat_report, ChatMessage, StreamOutcome, Usage};
use crate::auth::ClientAuth;
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
    /// The provider serving the reply; before any delta. Not counted in `seq`.
    Provider { id: String, name: String },
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
    /// The providers announced before the first delta, in order (more than one after a fallback).
    providers: Vec<(String, String)>,
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
    /// Set once Stop (or a delete, or a shutdown) has been asked for, so a generation between two
    /// attempts (waiting to retry) notices it as well as one with a request in flight.
    cancel_asked: std::sync::atomic::AtomicBool,
    cancel_notify: Notify,
    /// Why it was cancelled when that was not the person pressing Stop (e.g. `kill_switch`).
    cancel_reason: Mutex<Option<String>>,
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

    /// Announces the provider now serving the reply. Only before the first delta (after text has
    /// been sent the provider cannot change), and not twice in a row for the same provider.
    pub fn set_provider(&self, id: &str, name: &str) {
        {
            let mut g = self.lock();
            if g.terminal.is_some() || !g.deltas.is_empty() || g.providers.last().is_some_and(|(last, _)| last == id) {
                return;
            }
            g.providers.push((id.to_string(), name.to_string()));
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
        self.cancel_asked.store(true, std::sync::atomic::Ordering::SeqCst);
        self.cancel_notify.notify_waiters();
        if let Some(tx) = self.cancel.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = tx.send(());
        }
        true
    }

    /// Cancels it and records why (the first reason stands).
    pub fn cancel_because(&self, reason: &str) -> bool {
        self.cancel_reason.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(|| reason.to_string());
        self.cancel()
    }

    pub fn cancel_reason(&self) -> Option<String> {
        self.cancel_reason.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn cancel_was_asked(&self) -> bool {
        self.cancel_asked.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resolves when a cancel has been asked for.
    pub async fn cancel_asked(&self) {
        loop {
            let notified = self.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancel_was_asked() {
                return;
            }
            notified.await;
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
            /// How many provider announcements have been passed on. A subscriber that already has
            /// deltas has seen them and starts past them.
            providers_sent: usize,
        }
        let providers_sent = if after == 0 { 0 } else { usize::MAX };
        futures_util::stream::unfold(State { live: self.clone(), next: after, ended: false, providers_sent }, |mut s| async move {
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
                    if s.next == 0 && s.providers_sent < g.providers.len() {
                        let (id, name) = g.providers[s.providers_sent].clone();
                        s.providers_sent += 1;
                        Some(GenEvent::Provider { id, name })
                    } else if (s.next as usize) < g.deltas.len() {
                        Some(GenEvent::Delta { seq: s.next + 1, text: g.deltas[s.next as usize].clone() })
                    } else {
                        g.terminal.as_ref().map(|t| match t {
                            Terminal::Done => GenEvent::Done,
                            Terminal::Error(m) => GenEvent::Error { message: m.clone() },
                        })
                    }
                };
                match step {
                    Some(ev @ GenEvent::Provider { .. }) => return Some((ev, s)),
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
    pub client_id: String,
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
    /// The administrator has switched generations off.
    KillSwitch,
    /// A token budget has been used up for its period (`"daily"` or `"monthly"`).
    BudgetExceeded(&'static str),
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
            SubmitError::BudgetExceeded(period) => (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(serde_json::json!({ "error": "budget_exceeded", "period": period })),
            )
                .into_response(),
            SubmitError::KillSwitch => {
                (StatusCode::SERVICE_UNAVAILABLE, axum::Json(serde_json::json!({ "error": "kill_switch" }))).into_response()
            }
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
    /// Stopped because the administrator switched generations off.
    Killed,
    /// Ran out of time; carries the message subscribers see.
    TimedOut(String),
    Error(String),
}

/// What an attempt span records beyond its columns: which try it was (and which provider it stood
/// in for), the whole latency, the HTTP status, whether the counts were estimated, why the provider
/// says it stopped, and whether the request was cut. `report` is `None` for an attempt that never
/// finished (a timeout).
fn attempt_attrs(r: Option<&crate::llm::StreamReport>, info: &AttemptInfo) -> serde_json::Value {
    let mut attrs = serde_json::json!({ "retry_index": info.retry_index, "fallback_from": info.fallback_from });
    if let Some(r) = r {
        attrs["latency_ms"] = (r.total.as_millis() as u64).into();
        attrs["http_status"] = serde_json::json!(r.http_status);
        attrs["usage_estimated"] = serde_json::json!(r.usage.map(|u| u.estimated));
        attrs["finish_reason"] = serde_json::json!(r.finish_reason);
    }
    if info.request_truncated {
        attrs["truncated"] = true.into();
    }
    attrs
}

/// The public bytes of the key content is sealed to now, from the current snapshot.
fn current_audit_key(state: &AppState) -> Option<Vec<u8>> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let key = crate::snapshot::current(&db).ok().flatten()?.audit_public_key?;
    crate::snapshot::parse_hex(&key.public_key)
}

/// Sealed payloads (request, reply) are cut at this many bytes; the span is marked `truncated`.
pub const MAX_PAYLOAD_BYTES: usize = 256 * 1024;

/// Provider error text kept with an attempt is cut here.
pub const MAX_ERROR_PAYLOAD_BYTES: usize = 4096;

/// `text` cut to at most `max` bytes at a character boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
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
    client_id: Option<String>,
    provider_id: String,
    conversation_id: String,
    text: String,
    /// What the provider (or an estimate) says the reply used; recorded when the row ends.
    usage: Option<Usage>,
    /// The audit span for this generation as a whole, and why it failed if it did.
    turn_span: String,
    /// The public key content is sealed to, as it was when this generation started (None: the
    /// snapshot had none, which `submit` normally prevents).
    audit_key: Option<Vec<u8>>,
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
                "INSERT INTO token_usage (id, user_id, client_id, provider_id, tokens_in, tokens_out, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    crate::ids::new_id(),
                    self.user_id,
                    self.client_id,
                    self.provider_id,
                    u.input.unwrap_or(0) as i64,
                    u.output.unwrap_or(0) as i64,
                    crate::auth::now()
                ],
            );
        }
    }

    fn finish_turn_span(&self, status: &str) {
        // The reply, sealed, whatever way this ended (an empty reply has nothing to keep).
        let mut truncated = false;
        if let (Some(key), false) = (&self.audit_key, self.text.is_empty()) {
            let kept = cut(&self.text, MAX_PAYLOAD_BYTES);
            truncated = kept.len() < self.text.len();
            if let Ok(sealed) = crate::seal::seal(key, kept.as_bytes()) {
                self.state.audit.payload(&self.turn_span, "reply", sealed);
            }
        }
        let mut f = SpanFinish::new(&self.turn_span, status);
        if truncated {
            // Replaces the start attributes, so they are repeated.
            f.attrs_json = Some(serde_json::json!({ "conversation_id": self.conversation_id, "truncated": true }).to_string());
        }
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
            Outcome::Killed => self.save("cancelled", Some(KILLED)),
            Outcome::TimedOut(m) => self.save("timed_out", Some(m)),
            Outcome::Error(m) => self.save("error", Some(m)),
        }
        self.live.finish(match outcome {
            Outcome::Done | Outcome::Cancelled => Terminal::Done,
            Outcome::Killed => Terminal::Error(KILLED.into()),
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

/// Records that the kill switch was off for this generation.
fn pass_kill_switch_gate(state: &AppState, generation_id: &str, s: &Submit) {
    let span = state.audit.start_span(
        SpanStart::new(generation_id, Kind::Gate, "kill_switch").who(&s.user_id, Some(&s.client_id)).attrs(&serde_json::json!({ "kill_switch": false })),
    );
    state.audit.finish_span(SpanFinish::new(&span, "done"));
}

/// Records that the budget had room for this generation, with the numbers it saw.
fn pass_budget_gate(state: &AppState, generation_id: &str, s: &Submit, note: &serde_json::Value) {
    if note.is_null() {
        return;
    }
    let span = state.audit.start_span(SpanStart::new(generation_id, Kind::Gate, "budget").who(&s.user_id, Some(&s.client_id)).attrs(note));
    state.audit.finish_span(SpanFinish::new(&span, "done"));
}

/// Midnight UTC at the start of the day containing `t` (unix seconds).
pub fn day_start(t: i64) -> i64 {
    t - t.rem_euclid(86_400)
}

/// Midnight UTC on the first of the month containing `t` (unix seconds).
pub fn month_start(t: i64) -> i64 {
    // Civil date from days since 1970-01-01 and back (Howard Hinnant's algorithms).
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    // Days since 1970-01-01 of day 1 of that month.
    let y = year - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * 86_400
}

/// The token budget gate. Counts the tokens used since the start of the day and of the month by
/// providers that are paid per token (a subscription provider costs nothing, so its tokens do not
/// count, and a request to one is never refused for budget). `Err((period, numbers))` once a limit
/// that is set has been reached; `Ok(numbers)` otherwise, or `Null` when no budget is set.
fn budget_gate(db: &crate::Db, snap: &askbar_proto::Snapshot, requested: &str, now: i64) -> Result<serde_json::Value, (&'static str, serde_json::Value)> {
    let budget = &snap.budget;
    if budget.daily_tokens.is_none() && budget.monthly_tokens.is_none() {
        return Ok(serde_json::Value::Null);
    }
    // A request to a subscription provider spends no tokens that count.
    if snap.providers.iter().any(|p| p.id == requested && p.billing == askbar_proto::Billing::Subscription) {
        return Ok(serde_json::Value::Null);
    }
    let used_since = |since: i64| -> u64 {
        let mut stmt = match db.conn().prepare("SELECT provider_id, COALESCE(SUM(tokens_in + tokens_out), 0) FROM token_usage WHERE created_at >= ?1 GROUP BY provider_id") {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let rows = stmt.query_map([since], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i64>(1)?))).map(|it| it.flatten().collect::<Vec<_>>()).unwrap_or_default();
        rows.into_iter()
            .filter(|(pid, _)| !snap.providers.iter().any(|p| Some(&p.id) == pid.as_ref() && p.billing == askbar_proto::Billing::Subscription))
            .map(|(_, n)| n.max(0) as u64)
            .sum()
    };
    let (daily, monthly) = (used_since(day_start(now)), used_since(month_start(now)));
    let note = serde_json::json!({
        "daily_used": daily, "daily_limit": budget.daily_tokens,
        "monthly_used": monthly, "monthly_limit": budget.monthly_tokens,
    });
    if budget.daily_tokens.is_some_and(|limit| daily >= limit) {
        return Err(("daily", note));
    }
    if budget.monthly_tokens.is_some_and(|limit| monthly >= limit) {
        return Err(("monthly", note));
    }
    Ok(note)
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
    let snapshot = crate::snapshot::current(&db).map_err(internal)?;
    // Nothing runs unless what it says can be kept sealed: no audit key, no generation.
    if snapshot.as_ref().and_then(|snap| snap.audit_public_key.as_ref()).is_none() {
        return Err(SubmitError::AuditKeyMissing);
    }
    // The kill switch gate. It leaves a span either way: a refusal has no generation, so it gets a
    // trace of its own; a pass goes into the trace of the generation it lets through (below).
    let kill_switch = snapshot.as_ref().is_some_and(|snap| snap.kill_switch);
    let mut budget_note = serde_json::Value::Null;
    if let Some(snap) = snapshot.as_ref().filter(|_| !kill_switch) {
        match budget_gate(&db, snap, &s.provider_id, crate::auth::now()) {
            Ok(note) => budget_note = note,
            Err((period, note)) => {
                let trace = crate::ids::new_id();
                let span = state.audit.start_span(SpanStart::new(&trace, Kind::Gate, "budget").who(&s.user_id, Some(&s.client_id)).attrs(&note));
                state.audit.finish_span(SpanFinish::new(&span, "blocked").error_code("budget_exceeded"));
                return Err(SubmitError::BudgetExceeded(period));
            }
        }
    }
    if kill_switch {
        let trace = crate::ids::new_id();
        let span = state.audit.start_span(
            SpanStart::new(&trace, Kind::Gate, "kill_switch").who(&s.user_id, Some(&s.client_id)).attrs(&serde_json::json!({ "kill_switch": true })),
        );
        state.audit.finish_span(SpanFinish::new(&span, "blocked").error_code("kill_switch"));
        return Err(SubmitError::KillSwitch);
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
        client_id: Some(&s.client_id),
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
        pass_kill_switch_gate(state, &id, &s);
        pass_budget_gate(state, &id, &s, &budget_note);
        let live = Live::new();
        live.set_queue_span(state.audit.start_span(SpanStart::new(&id, Kind::Gate, "queue").who(&s.user_id, Some(&s.client_id))));
        state.generations.insert(&id, live.clone());
        return Ok(Submitted { generation_id: id, conversation_id, live, queued: true });
    }
    // Save the question first so it survives a failed request.
    let user_message_id = db.add_message(&s.user_id, &conversation_id, "user", &s.prompt).map_err(internal)?;
    let id = db.create_generation(&new("running", Some(&user_message_id))).map_err(internal)?;
    let timeout = timeout_now(state, &db);
    stamp_deadline(&db, &id, timeout);
    pass_kill_switch_gate(state, &id, &s);
    pass_budget_gate(state, &id, &s, &budget_note);
    let live = Live::new();
    state.generations.insert(&id, live.clone());
    drop(db);
    spawn_run(state, &id, &s.user_id, Some(&s.client_id), &conversation_id, &s.provider_id, timeout, live.clone());
    Ok(Submitted { generation_id: id, conversation_id, live, queued: false })
}

/// Runs generation `id` as a task that does not depend on any client. The provider, system prompt
/// and history are read when it starts, so a message that waited sees the replies before it.
#[allow(clippy::too_many_arguments)]
fn spawn_run(
    state: &Arc<AppState>,
    id: &str,
    user_id: &str,
    client_id: Option<&str>,
    conversation_id: &str,
    provider_id: &str,
    timeout: Duration,
    live: Arc<Live>,
) {
    let turn_span = state.audit.start_span(
        SpanStart::new(id, Kind::Turn, "chat").who(user_id, client_id).attrs(&serde_json::json!({ "conversation_id": conversation_id })),
    );
    let audit_key = current_audit_key(state);
    let mut fin = Finisher {
        state: state.clone(),
        live,
        turn_span,
        audit_key,
        error_code: None,
        id: id.to_string(),
        user_id: user_id.to_string(),
        client_id: client_id.map(str::to_string),
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
        // The providers to try, in order: the one asked for, then the fallbacks the administrator
        // allowed (see router::plan), each loaded with its key.
        let prepared = {
            let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let plan = crate::snapshot::current(&db).ok().flatten().map(|s| crate::router::plan(&s, &provider_id)).unwrap_or_default();
            let candidates: Vec<Candidate> = plan
                .into_iter()
                .filter_map(|r| {
                    let provider = store::load_provider(&db, &state.secrets(), &r.id).ok().flatten()?;
                    Some(Candidate { id: r.id, provider, fallback_from: r.fallback_from })
                })
                .collect();
            let system = store::chat_system_prompt(&db).ok();
            let history: Option<Vec<ChatMessage>> = db
                .get_messages(&fin.user_id, &fin.conversation_id)
                .ok()
                .map(|ms| ms.iter().map(|m| ChatMessage::new(&m.role, &m.content)).collect());
            (!candidates.is_empty()).then_some(candidates).zip(system).zip(history)
        };
        let Some(((candidates, system), history)) = prepared else {
            fin.finish(Outcome::Error("The provider is no longer available.".into()));
            return;
        };

        // Race the whole sequence of attempts against the deadline. Dropping it on expiry closes
        // the request in flight; the text that had arrived is already in `fin`.
        let started = Instant::now();
        let open_attempt: Mutex<Option<OpenAttempt>> = Mutex::new(None);
        let ended = {
            let run = attempts(&state, &mut fin, &candidates, &system, &history, started, timeout, &open_attempt);
            tokio::pin!(run);
            tokio::select! {
                r = &mut run => Some(r),
                _ = tokio::time::sleep(timeout) => None,
            }
        };
        let (outcome, usage) = match ended {
            Some(Ended::Done(report)) => (Outcome::Done, report.usage),
            Some(Ended::Cancelled(report)) => {
                let usage = report.and_then(|r| r.usage);
                if fin.live.cancel_reason().as_deref() == Some("kill_switch") {
                    fin.error_code = Some("kill_switch".into());
                    (Outcome::Killed, usage)
                } else {
                    (Outcome::Cancelled, usage)
                }
            }
            Some(Ended::Failed(e)) => {
                fin.error_code = Some(error_code(&e.error));
                (Outcome::Error(e.error.to_string()), e.report.usage)
            }
            None => {
                // The attempt in flight when time ran out never got to say how it ended.
                if let Some(open) = open_attempt.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let mut end = SpanFinish::new(&open.span, "timed_out");
                    end.error_code = Some("timeout".into());
                    end.attrs_json = Some(attempt_attrs(None, &open.info).to_string());
                    state.audit.finish_span(end);
                }
                fin.error_code = Some("timeout".into());
                (Outcome::TimedOut(timed_out_message(timeout)), None)
            }
        };
        fin.usage = usage;
        fin.finish(outcome);
    });
}

/// A provider to try, with the provider it stands in for (if it is not the one asked for).
struct Candidate {
    id: String,
    provider: crate::llm::Provider,
    fallback_from: Option<String>,
}

/// Retries per provider, and the wait before each (when the provider names none).
const MAX_RETRIES: u32 = 2;
const RETRY_BACKOFF: [Duration; 2] = [Duration::from_millis(250), Duration::from_secs(1)];

/// How a run of attempts ended.
enum Ended {
    Done(crate::llm::StreamReport),
    Cancelled(Option<crate::llm::StreamReport>),
    Failed(crate::llm::ReportedError),
}

/// What identifies an attempt in the audit trail.
#[derive(Clone)]
struct AttemptInfo {
    retry_index: u32,
    fallback_from: Option<String>,
    request_truncated: bool,
}

/// The attempt span in flight, so a timeout can close it.
struct OpenAttempt {
    span: String,
    info: AttemptInfo,
}

/// Tries each candidate in order, and each up to `MAX_RETRIES` more times when the failure is
/// worth it, until one serves the reply. Nothing is retried or switched once a delta has been
/// sent: the person has seen text, so the failure ends the generation. A cancel ends it at once,
/// also while it waits to retry.
#[allow(clippy::too_many_arguments)]
async fn attempts(
    state: &Arc<AppState>,
    fin: &mut Finisher,
    candidates: &[Candidate],
    system: &str,
    history: &[ChatMessage],
    started: Instant,
    timeout: Duration,
    open_attempt: &Mutex<Option<OpenAttempt>>,
) -> Ended {
    use crate::router::{classify, Failure};
    let live = fin.live.clone();
    let mut last_error = None;
    for cand in candidates {
        for retry_index in 0..=MAX_RETRIES {
            let (cancel_tx, cancel_rx) = oneshot::channel::<()>();
            live.set_cancel(cancel_tx);
            if live.cancel_was_asked() {
                return Ended::Cancelled(None);
            }
            if fin.provider_id != cand.id || retry_index == 0 {
                // The row says which provider is serving it (finally: the one that did).
                fin.provider_id = cand.id.clone();
                let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
                let _ = db.conn().execute("UPDATE generations SET provider_id = ?2 WHERE id = ?1", rusqlite::params![fin.id, cand.id]);
            }
            live.set_provider(&cand.id, &cand.provider.name);
            let span = state.audit.start_span(
                SpanStart::new(&fin.id, Kind::Attempt, &cand.provider.name)
                    .parent(&fin.turn_span)
                    .provider(&cand.id, &cand.provider.model)
                    .attrs(&serde_json::json!({ "retry_index": retry_index, "fallback_from": cand.fallback_from })),
            );
            let mut info = AttemptInfo { retry_index, fallback_from: cand.fallback_from.clone(), request_truncated: false };
            // What was sent, sealed: the system prompt and the history, as the provider will see them.
            if let Some(key) = &fin.audit_key {
                let request = serde_json::json!({
                    "system": system,
                    "messages": history.iter().map(|m| serde_json::json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
                })
                .to_string();
                let kept = cut(&request, MAX_PAYLOAD_BYTES);
                info.request_truncated = kept.len() < request.len();
                if let Ok(sealed) = crate::seal::seal(key, kept.as_bytes()) {
                    state.audit.payload(&span, "request", sealed);
                }
            }
            *open_attempt.lock().unwrap_or_else(|e| e.into_inner()) = Some(OpenAttempt { span: span.clone(), info: info.clone() });

            let result = stream_chat_report(&state.http, &cand.provider, system, history, cancel_rx, |delta| fin.push(delta)).await;

            // The attempt is over; say how it went.
            open_attempt.lock().unwrap_or_else(|e| e.into_inner()).take();
            let mut end = SpanFinish::new(&span, "done");
            match &result {
                Ok(r) => {
                    end.status = match r.outcome {
                        StreamOutcome::Completed => "done",
                        StreamOutcome::Cancelled => "cancelled",
                    }
                    .into();
                    if r.outcome == StreamOutcome::Cancelled {
                        end.error_code = live.cancel_reason();
                    }
                    end.ttft_ms = r.time_to_first_token.map(|d| d.as_millis() as u64);
                    end.attrs_json = Some(attempt_attrs(Some(r), &info).to_string());
                    if let Some(u) = r.usage {
                        (end.tokens_in, end.tokens_out, end.tokens_cached) = (u.input, u.output, u.cache_read);
                    }
                }
                Err(e) => {
                    // The provider's own words about the failure, sealed, cut at 4 KB.
                    if let Some(key) = &fin.audit_key {
                        let message = e.error.to_string();
                        if let Ok(sealed) = crate::seal::seal(key, cut(&message, MAX_ERROR_PAYLOAD_BYTES).as_bytes()) {
                            state.audit.payload(&span, "error", sealed);
                        }
                    }
                    end.status = "error".into();
                    end.error_code = Some(error_code(&e.error));
                    end.ttft_ms = e.report.time_to_first_token.map(|d| d.as_millis() as u64);
                    end.attrs_json = Some(attempt_attrs(Some(&e.report), &info).to_string());
                }
            }
            state.audit.finish_span(end);

            let e = match result {
                Ok(r) => {
                    // It worked, so whatever was wrong with its credentials no longer is.
                    if r.outcome == StreamOutcome::Completed {
                        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
                        let _ = db.clear_provider_health(&cand.id);
                    }
                    return match r.outcome {
                        StreamOutcome::Completed => Ended::Done(r),
                        StreamOutcome::Cancelled => Ended::Cancelled(Some(r)),
                    }
                }
                Err(e) => e,
            };
            let class = classify(&e.error);
            // Text already sent, or a request that no provider would accept: the failure stands.
            if !fin.text.is_empty() || class == Failure::Fatal {
                return Ended::Failed(e);
            }
            match class {
                // Bad credentials: another try here cannot help; the next provider may. The
                // administrator is told through the provider's health flag.
                Failure::AuthFailed => {
                    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
                    let _ = db.flag_provider_auth_failed(&cand.id, &error_code(&e.error));
                    drop(db);
                    last_error = Some(e);
                    break;
                }
                _ if retry_index >= MAX_RETRIES => {
                    last_error = Some(e);
                    break;
                }
                _ => {
                    // The provider's Retry-After replaces our backoff when it fits in the time left.
                    let remaining = timeout.saturating_sub(started.elapsed());
                    let wait = match e.report.retry_after {
                        Some(ra) if ra < remaining => ra,
                        _ => RETRY_BACKOFF[retry_index as usize],
                    };
                    last_error = Some(e);
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = live.cancel_asked() => return Ended::Cancelled(None),
                    }
                }
            }
        }
    }
    Ended::Failed(last_error.expect("at least one candidate was tried"))
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
            spawn_run(state, &next.id, &next.user_id, next.client_id.as_deref(), &next.conversation_id, next.provider_id.as_deref().unwrap_or(""), timeout, live);
        }
        None => {
            // Try the one after it.
            start_next(state, conversation_id);
        }
    }
}

const KILLED: &str = "Generations were switched off by the administrator.";
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

/// The administrator turned the kill switch on. Queued messages never run: they are cleared
/// whatever the setting. Replies already running are stopped (ending `cancelled`, error code
/// `kill_switch`) when `aborts_running`, and left to finish when not. Nothing new starts either
/// way (`submit` refuses). Safe to call again while the switch stays on.
pub fn apply_kill_switch(state: &Arc<AppState>, aborts_running: bool) {
    let queued: Vec<String> = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<String> = db
            .conn()
            .prepare("SELECT id FROM generations WHERE status = 'queued'")
            .and_then(|mut stmt| stmt.query_map([], |r| r.get(0))?.collect::<Result<_, _>>())
            .unwrap_or_default();
        for id in &ids {
            let _ = db.conn().execute(
                "UPDATE generations SET status = 'cancelled', error = ?2, ended_at = ?3 WHERE id = ?1 AND status = 'queued'",
                rusqlite::params![id, KILLED, crate::auth::now()],
            );
        }
        ids
    };
    for id in queued {
        if let Some(live) = state.generations.get(&id) {
            live.end_queue_span(&state.audit, "cancelled");
            live.finish(Terminal::Error(KILLED.into()));
            state.generations.remove(&id);
        }
    }
    if aborts_running {
        for (_, live) in state.generations.unfinished() {
            live.cancel_because("kill_switch");
        }
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
    Extension(auth): Extension<ClientAuth>,
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
        GenEvent::Provider { id, name } => sse_event(&ChatEvent::Provider { id, name }),
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
    Extension(auth): Extension<ClientAuth>,
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
    fn day_and_month_start_at_midnight_utc() {
        // (instant, start of its day, start of its month), checked against an independent calendar.
        for (t, day, month) in [
            (1_793_631_845, 1_793_577_600, 1_793_491_200), // 2026-11-02 15:04:05
            (1_801_439_999, 1_801_353_600, 1_798_761_600), // 2027-01-31 23:59:59
            (1_709_164_800, 1_709_164_800, 1_706_745_600), // 2024-02-29 00:00:00, a leap day
            (1_709_424_000, 1_709_424_000, 1_709_251_200), // 2024-03-03
            (1_704_067_205, 1_704_067_200, 1_704_067_200), // 2024-01-01 00:00:05
        ] {
            assert_eq!(day_start(t), day, "day of {t}");
            assert_eq!(month_start(t), month, "month of {t}");
        }
    }

    #[test]
    fn error_text_is_cut_at_a_character_boundary_within_4_kb() {
        assert_eq!(cut("short", 4096), "short");
        let long = "é".repeat(3000); // 6000 bytes
        let c = cut(&long, MAX_ERROR_PAYLOAD_BYTES);
        assert!(c.len() <= 4096 && c.len() >= 4094, "{}", c.len());
        assert!(c.chars().all(|ch| ch == 'é'), "never half a character");
        assert_eq!(cut("abc", 0), "");
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
                client_id: None,
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
