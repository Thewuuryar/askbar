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
/// How often a service calls the auditor's `GET /status`, so its link is never silent while it is alive.
pub const STATUS_EVERY: Duration = Duration::from_secs(60);

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
    Config(askbar_proto::audit::ConfigApplied),
    AlertDelivery(askbar_proto::audit::AlertDeliveryStart),
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
    /// Whether the last request reached the auditor and was not refused for its backlog.
    last_ok: AtomicBool,
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
            last_ok: AtomicBool::new(true),
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
        let sent = self.http.post(self.url(path)).bearer_auth(token).timeout(timeout).json(body).send().await;
        let status = sent.map(|r| r.status().as_u16()).map_err(|_| ());
        self.last_ok.store(matches!(status, Ok(s) if s != 503 && s != 429), Ordering::Relaxed);
        status
    }

    /// Whether this service may start a new piece of work that must be recorded: the auditor has its boot ID, the last
    /// request reached it and was not refused for its backlog, and the queue is not nearly full. A service that cannot
    /// record answers `503 audit_backlog` rather than work unrecorded (ADR 0010 rule 3).
    pub fn healthy(&self) -> bool {
        self.ready() && self.last_ok.load(Ordering::Relaxed) && self.queued() < QUEUE_LIMIT * 9 / 10
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

    /// Queue the record of the configuration version this service now runs (ADR 0028 rule 5). Behind the rest of the
    /// queue, and retried until the auditor takes it.
    pub fn config_applied(&self, version: i64) {
        self.push(Item::Config(askbar_proto::audit::ConfigApplied { id: ids::new_id(), at: now_parts().0, version }));
    }

    /// Queue the outcome of one alert (`alerter-auditor` only, ADR 0055): metadata, never the alert's sentence.
    pub fn alert_delivery(&self, kind: &str, channel: &str, outcome: &str, attempts: u32) {
        self.push(Item::AlertDelivery(askbar_proto::audit::AlertDeliveryStart {
            id: ids::new_id(),
            at: now_parts().0,
            kind: kind.to_string(),
            channel: channel.to_string(),
            outcome: outcome.to_string(),
            attempts,
        }));
    }

    /// Call the auditor's `GET /status` once a minute for as long as the process runs. The auditor notes the time of each
    /// request on a link, and a link is silent when nothing has come over it for five minutes (ADR 0051): a service that is
    /// alive but has nothing to record is not silent, because of this call. Nothing in the answer is used.
    pub async fn status_every_minute(self: Arc<Self>) {
        loop {
            self.ping().await;
            tokio::time::sleep(STATUS_EVERY).await;
        }
    }

    /// One `GET /status` to the auditor, for the time it notes. The result is of no use to the caller.
    pub async fn ping(&self) {
        let Some(token) = self.token() else { return };
        let _ = self.http.get(self.url("/status")).bearer_auth(token).timeout(SEND_TIMEOUT).send().await;
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
                Item::Config(c) => self.send("/config-applied", c, SEND_TIMEOUT).await,
                Item::AlertDelivery(d) => self.send("/alert-deliveries", d, SEND_TIMEOUT).await,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A stand-in auditor that counts what it is asked, and with which token.
    async fn auditor(seen: Arc<Mutex<Vec<(String, String, String)>>>, count: Arc<AtomicUsize>) -> String {
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let (seen, count) = (seen.clone(), count.clone());
            async move {
                let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                seen.lock().unwrap().push((req.method().to_string(), req.uri().path().to_string(), auth));
                count.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({ "interrupted": 0 }))
            }
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    }

    fn dir_with_token(link: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("askbar-auditclient-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(d.join("call")).unwrap();
        std::fs::write(d.join("call").join(format!("{link}.token")), "tok_secret\n").unwrap();
        d
    }

    #[tokio::test]
    async fn the_status_call_goes_to_the_auditors_status_route_with_the_links_own_token_and_does_not_need_a_boot_id() {
        let (seen, count) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
        let url = auditor(seen.clone(), count.clone()).await;
        let dir = dir_with_token("alerter-auditor");
        let client = AuditClient::new(&url, dir.clone(), "alerter-auditor");
        assert!(!client.ready(), "no hello yet");
        client.ping().await;
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            [("GET".to_string(), "/api/audit/v1/status".to_string(), "Bearer tok_secret".to_string())]
        );
        // It runs for as long as the process does: the first call comes at once and the next after the interval.
        let task = tokio::spawn(client.clone().status_every_minute());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(count.load(Ordering::SeqCst), 2, "the loop's first call is immediate");
        task.abort();
        assert_eq!(STATUS_EVERY, Duration::from_secs(60));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_missing_token_means_no_call_rather_than_an_unauthenticated_one() {
        let (seen, count) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
        let url = auditor(seen, count.clone()).await;
        let dir = std::env::temp_dir().join(format!("askbar-auditclient-none-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        AuditClient::new(&url, dir.clone(), "alerter-auditor").ping().await;
        assert_eq!(count.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_alert_delivery_is_queued_behind_the_rest_and_sent_to_its_own_route_as_metadata_only() {
        let (seen, count) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicUsize::new(0)));
        let url = auditor(seen.clone(), count.clone()).await;
        let dir = dir_with_token("alerter-auditor");
        let client = AuditClient::new(&url, dir.clone(), "alerter-auditor");
        tokio::spawn(client.clone().hello_until_done("boot".into()));
        tokio::spawn(client.clone().drain_forever());
        client.alert_delivery("unsealed", "ntfy", "sent", 1);
        for _ in 0..100 {
            if seen.lock().unwrap().iter().any(|(_, p, _)| p.ends_with("/alert-deliveries")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|(m, p, _)| format!("{m} {p}")).collect();
        assert_eq!(paths, ["POST /api/audit/v1/hello", "POST /api/audit/v1/alert-deliveries"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}
