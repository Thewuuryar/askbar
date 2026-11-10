//! A service's link to the auditor (ADR 0054, ADR 0010 rules 1 to 3), shared by `model-gateway` and the server.
//!
//! - A span that must be on record before a provider is called is appended and **acknowledged**
//!   ([`AuditClient::open`]): one second at most, and no answer in time is a refusal.
//! - Everything else (finish records, sealed payloads, the records of refusals) goes through a bounded queue
//!   in memory that a background task empties, in order, whenever the auditor answers. A record is sent again
//!   after a lost answer, which is safe because the auditor takes each id once.
//!
//! Content never passes through here in the clear: payloads arrive sealed.

use crate::ids;
use askbar_proto::audit::{Hello, SealedPayload, SpanFinish, SpanKind, SpanStart};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// How long a span start waits for its acknowledgment (ADR 0051's table).
pub const ACK_TIMEOUT: Duration = Duration::from_secs(1);
/// How many records the queue keeps; past it the oldest is dropped, and the drops are counted and recorded.
pub const QUEUE_LIMIT: usize = 1000;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a span could not be opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The auditor did not answer, or not in time, or is not known to be ours yet (no hello).
    Unreachable,
    /// The auditor refused: its backlog or its disk (`503 audit_backlog`).
    Backlog,
    /// The auditor does not take this gateway's token, or this record.
    Rejected,
}

#[derive(Debug, Clone)]
enum Item {
    Start(SpanStart),
    Finish(String, SpanFinish),
    Payload(String, SealedPayload),
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Item>,
    /// Records dropped to keep the queue at its limit, not yet recorded.
    dropped: u64,
}

pub struct AuditClient {
    http: reqwest::Client,
    base: String,
    token_dir: PathBuf,
    /// The link, as `bootstrap` names it (`model-gateway-auditor`, `server-auditor`): its token is `call/<link>.token`.
    link: &'static str,
    queue: Mutex<Queue>,
    wake: Notify,
    /// The auditor has our boot ID: spans opened from now on belong to this process.
    hello_done: AtomicBool,
    /// Records sent after waiting in the queue, for the tests and the status.
    sent: AtomicU64,
}

impl AuditClient {
    pub fn new(base: &str, token_dir: PathBuf, link: &'static str) -> Arc<AuditClient> {
        Arc::new(AuditClient {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(2)).build().expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token_dir,
            link,
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
            hello_done: AtomicBool::new(false),
            sent: AtomicU64::new(0),
        })
    }

    /// The token `bootstrap` made for this link, read each time: a rotated token works without a restart.
    fn token(&self) -> Option<String> {
        let t = std::fs::read_to_string(self.token_dir.join("call").join(format!("{}.token", self.link))).ok()?;
        Some(t.trim().to_string()).filter(|t| !t.is_empty())
    }

    fn url(&self, rest: &str) -> String {
        format!("{}{}{rest}", self.base, askbar_proto::paths::AUDIT_V1)
    }

    /// One request; what the auditor answered with, or `None` if nothing reached it.
    async fn send<T: Serialize>(&self, path: &str, body: &T, timeout: Duration) -> Result<u16, ()> {
        let token = self.token().ok_or(())?;
        let r = self.http.post(self.url(path)).bearer_auth(token).timeout(timeout).json(body).send().await.map_err(|_| ())?;
        Ok(r.status().as_u16())
    }

    /// Whether the auditor has been told this process's boot ID.
    pub fn ready(&self) -> bool {
        self.hello_done.load(Ordering::Relaxed)
    }

    /// Say which process this is, until the auditor takes it: it then ends the spans an earlier process left
    /// running (ADR 0010 rule 6). Runs in the background; no span is opened before it succeeds.
    pub async fn hello_until_done(self: Arc<Self>, boot_id: String) {
        let mut wait = Duration::from_millis(200);
        loop {
            if let Ok(200) = self.send("/hello", &Hello { boot_id: boot_id.clone() }, SEND_TIMEOUT).await {
                self.hello_done.store(true, Ordering::Relaxed);
                self.wake.notify_one();
                return;
            }
            tokio::time::sleep(wait).await;
            wait = (wait * 2).min(Duration::from_secs(5));
        }
    }

    /// Append a span start and wait for the acknowledgment. Nothing is queued: the caller refuses the request.
    pub async fn open(&self, start: &SpanStart) -> Result<(), Refused> {
        if !self.ready() {
            return Err(Refused::Unreachable);
        }
        match self.send("/spans", start, ACK_TIMEOUT).await {
            Ok(200 | 201) => Ok(()),
            Ok(503) => Err(Refused::Backlog),
            Ok(401 | 403) => Err(Refused::Rejected),
            Ok(_) => Err(Refused::Rejected),
            Err(()) => Err(Refused::Unreachable),
        }
    }

    fn push(&self, item: Item) {
        {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.items.len() >= QUEUE_LIMIT {
                q.items.pop_front();
                q.dropped += 1;
            }
            q.items.push_back(item);
        }
        self.wake.notify_one();
    }

    /// Queue how a span ended.
    pub fn finish(&self, id: &str, f: SpanFinish) {
        self.push(Item::Finish(id.to_string(), f));
    }

    /// Queue a span start that needs no acknowledgment: the server's own gate spans (ADR 0010 rule 2 lets its link open
    /// those and nothing else). Behind the queue, so it reaches the auditor before the span's finish record.
    pub fn start(&self, start: SpanStart) {
        self.push(Item::Start(start));
    }

    /// Queue a sealed payload for a span: the id of the key it was sealed to, and the ciphertext.
    pub fn payload(&self, id: &str, role: &str, key_id: &str, ciphertext: &[u8]) {
        self.push(Item::Payload(
            id.to_string(),
            SealedPayload { role: role.to_string(), key_id: key_id.to_string(), ciphertext_b64: STANDARD.encode(ciphertext) },
        ));
    }

    /// Queue the record of a request this gateway refused: a span that began and ended at once.
    pub fn refusal(&self, start: SpanStart, finish: SpanFinish) {
        let id = start.id.clone();
        self.push(Item::Start(start));
        self.push(Item::Finish(id, finish));
    }

    /// How many records wait to be sent.
    pub fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).items.len()
    }

    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Empty the queue in order, for as long as the process runs. A record the auditor refuses for good (a 4xx
    /// other than a full backlog) is dropped with a log line that names the status and never the record.
    pub async fn drain_forever(self: Arc<Self>) {
        let mut idle = Duration::from_millis(250);
        loop {
            if !self.ready() {
                let _ = tokio::time::timeout(Duration::from_millis(500), self.wake.notified()).await;
                continue;
            }
            self.record_drops();
            let next = self.queue.lock().unwrap_or_else(|e| e.into_inner()).items.front().cloned();
            let Some(item) = next else {
                let _ = tokio::time::timeout(Duration::from_secs(1), self.wake.notified()).await;
                continue;
            };
            let status = match &item {
                Item::Start(s) => self.send("/spans", s, SEND_TIMEOUT).await,
                Item::Finish(id, f) => self.send(&format!("/spans/{id}/finish"), f, SEND_TIMEOUT).await,
                Item::Payload(id, p) => self.send(&format!("/spans/{id}/payloads"), p, SEND_TIMEOUT).await,
            };
            match status {
                Ok(200 | 201) => {
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).items.pop_front();
                    self.sent.fetch_add(1, Ordering::Relaxed);
                    idle = Duration::from_millis(250);
                }
                // Not now: the auditor is busy or out of reach. Keep the record and try again.
                Ok(503 | 429) | Err(()) => {
                    tokio::time::sleep(idle).await;
                    idle = (idle * 2).min(Duration::from_secs(5));
                }
                Ok(other) => {
                    tracing::warn!(status = other, "the auditor refused a queued record for good; dropped it");
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).items.pop_front();
                }
            }
        }
    }

    /// Records that were dropped to keep the queue at its limit are themselves recorded: one span saying how
    /// many (ADR 0054). Queued behind the rest, so it is sent when the auditor is back.
    fn record_drops(&self) {
        let dropped = {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::take(&mut q.dropped)
        };
        if dropped == 0 {
            return;
        }
        let (now, ms) = now_parts();
        let id = ids::new_id();
        let start = SpanStart {
            id: id.clone(),
            trace_id: id.clone(),
            parent_id: None,
            kind: SpanKind::Gate,
            name: "records_dropped".into(),
            started_at: now,
            started_ms: ms,
            provider_id: None,
            model: None,
            user_id: None,
            client_id: None,
            attrs: Some(serde_json::json!({ "dropped": dropped })),
        };
        let finish = SpanFinish {
            status: "done".into(),
            ended_at: now,
            ended_ms: ms,
            provider_id: None,
            model: None,
            tokens_in: None,
            tokens_out: None,
            tokens_cached: None,
            ttft_ms: None,
            error_code: None,
            attrs: None,
        };
        // Pushed straight in, not through `push`, so recording the drops cannot drop anything.
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.items.push_back(Item::Start(start));
        q.items.push_back(Item::Finish(id, finish));
    }
}

/// Unix seconds and milliseconds now.
pub fn now_parts() -> (i64, i64) {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    (d.as_secs() as i64, d.as_millis() as i64)
}
