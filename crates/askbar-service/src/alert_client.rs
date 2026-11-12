//! A service's link to the alerter, for sending it events (ADR 0055), shared by the admin and, later, the auditor.
//!
//! An event is a kind, a severity, a time and typed fields. This client queues it in memory and a background task sends it
//! to `POST /events` with the link's own token, in order. The queue is short and events go stale: an alert that could not be
//! sent for an hour is not worth sending, because the alerter's own rules (a repeat window, an hourly cap) assume it is
//! about now. The alerter decides everything else; a `503` (it holds no settings yet) is waited out, and any other refusal
//! is dropped with a log line that names the kind and the status, never the fields.

use askbar_proto::alerter::{Event, Fields, Kind, Severity};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

/// How many events wait; past it the oldest is dropped.
pub const QUEUE_LIMIT: usize = 200;
/// An event older than this is dropped instead of sent.
pub const STALE_AFTER_SECS: i64 = 60 * 60;
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

pub struct AlertClient {
    http: reqwest::Client,
    base: String,
    token_dir: PathBuf,
    /// The link, as `bootstrap` names it (`admin-alerter`): its token is `call/<link>.token`.
    link: &'static str,
    queue: Mutex<VecDeque<Event>>,
    wake: Notify,
    sent: AtomicU64,
}

impl AlertClient {
    pub fn new(base: &str, token_dir: PathBuf, link: &'static str) -> Arc<AlertClient> {
        Arc::new(AlertClient {
            http: reqwest::Client::builder().connect_timeout(Duration::from_secs(2)).build().expect("reqwest client"),
            base: base.trim_end_matches('/').to_string(),
            token_dir,
            link,
            queue: Mutex::new(VecDeque::new()),
            wake: Notify::new(),
            sent: AtomicU64::new(0),
        })
    }

    /// The token `bootstrap` made for this link, read each time: a rotated token works without a restart.
    fn token(&self) -> Option<String> {
        let t = std::fs::read_to_string(self.token_dir.join("call").join(format!("{}.token", self.link))).ok()?;
        Some(t.trim().to_string()).filter(|t| !t.is_empty())
    }

    /// Queue an event of `kind` with `fields`, at the severity the kind has. `budget_threshold` is the one kind with two: use
    /// [`AlertClient::send_with`].
    pub fn send(&self, kind: Kind, fields: Fields) {
        self.send_with(kind, kind.severity(), fields);
    }

    pub fn send_with(&self, kind: Kind, severity: Severity, fields: Fields) {
        self.push(Event { kind, severity, at: now(), fields });
    }

    fn push(&self, e: Event) {
        {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.len() >= QUEUE_LIMIT {
                q.pop_front();
            }
            q.push_back(e);
        }
        self.wake.notify_one();
    }

    /// How many events wait to be sent.
    pub fn queued(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// How many the alerter has taken.
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// What the alerter answered to the oldest event, or `None` if nothing reached it (or there is no token).
    async fn post(&self, e: &Event) -> Option<u16> {
        let token = self.token()?;
        let url = format!("{}{}/events", self.base, askbar_proto::paths::ALERTER_V1);
        let sent = self.http.post(url).bearer_auth(token).timeout(SEND_TIMEOUT).json(e).send().await;
        sent.ok().map(|r| r.status().as_u16())
    }

    /// Send the queue in order for as long as the process runs.
    pub async fn drain_forever(self: Arc<Self>) {
        let mut idle = Duration::from_millis(250);
        loop {
            let next = self.queue.lock().unwrap_or_else(|e| e.into_inner()).front().cloned();
            let Some(event) = next else {
                let _ = tokio::time::timeout(Duration::from_secs(1), self.wake.notified()).await;
                continue;
            };
            if now() - event.at > STALE_AFTER_SECS {
                tracing::warn!(kind = event.kind.name(), "an alert could not be sent for an hour; dropped it");
                self.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                continue;
            }
            match self.post(&event).await {
                Some(200 | 202) => {
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                    self.sent.fetch_add(1, Ordering::Relaxed);
                    idle = Duration::from_millis(250);
                }
                // Not now: the alerter does not answer, has no settings yet or is busy. Keep it and try again.
                Some(503 | 429) | None => {
                    tokio::time::sleep(idle).await;
                    idle = (idle * 2).min(Duration::from_secs(10));
                }
                Some(other) => {
                    tracing::warn!(kind = event.kind.name(), status = other, "the alerter refused an alert for good; dropped it");
                    self.queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                }
            }
        }
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    type Seen = Arc<Mutex<Vec<(String, String, serde_json::Value)>>>;

    /// A stand-in alerter: answers `status` and notes the path, the token and the body of each request.
    async fn alerter(seen: Seen, status: Arc<Mutex<StatusCode>>) -> String {
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let (seen, status) = (seen.clone(), status.clone());
            async move {
                let auth = req.headers().get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                let path = req.uri().path().to_string();
                let body = axum::body::to_bytes(req.into_body(), 1 << 16).await.unwrap();
                seen.lock().unwrap().push((path, auth, serde_json::from_slice(&body).unwrap_or_default()));
                let s = *status.lock().unwrap();
                (s, axum::Json(serde_json::json!({ "suppressed": false })))
            }
        });
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    }

    fn dir_with_token(link: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("askbar-alertclient-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(d.join("call")).unwrap();
        std::fs::write(d.join("call").join(format!("{link}.token")), "tok_secret\n").unwrap();
        d
    }

    async fn until(what: &str, mut ok: impl FnMut() -> bool) {
        for _ in 0..200 {
            if ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("timed out waiting for {what}");
    }

    #[tokio::test]
    async fn an_event_goes_to_the_events_route_with_the_links_own_token_in_order_and_with_typed_fields_only() {
        let (seen, status) = (Seen::default(), Arc::new(Mutex::new(StatusCode::ACCEPTED)));
        let url = alerter(seen.clone(), status).await;
        let dir = dir_with_token("admin-alerter");
        let client = AlertClient::new(&url, dir.clone(), "admin-alerter");
        client.send(Kind::ChainMismatch, Fields::default());
        client.send_with(
            Kind::BudgetThreshold,
            Severity::Critical,
            Fields { period: Some(askbar_proto::alerter::Period::Day), percent: Some(100), ..Fields::default() },
        );
        tokio::spawn(client.clone().drain_forever());
        until("both events", || client.sent() == 2).await;
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.iter().map(|s| (s.0.as_str(), s.1.as_str())).collect::<Vec<_>>(),
            [("/api/alerter/v1/events", "Bearer tok_secret"); 2]
        );
        assert_eq!(seen[0].2["kind"], "chain_mismatch");
        assert_eq!(seen[0].2["severity"], "critical");
        assert_eq!(seen[1].2["fields"], serde_json::json!({ "period": "day", "percent": 100 }));
        // What was sent is an event the alerter's own check takes.
        let e: Event = serde_json::from_value(seen[1].2.clone()).unwrap();
        assert!(e.check(askbar_proto::alerter::Sender::Admin).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_alerter_that_has_no_settings_yet_is_waited_for_and_a_refusal_is_dropped() {
        let (seen, status) = (Seen::default(), Arc::new(Mutex::new(StatusCode::SERVICE_UNAVAILABLE)));
        let url = alerter(seen.clone(), status.clone()).await;
        let dir = dir_with_token("admin-alerter");
        let client = AlertClient::new(&url, dir.clone(), "admin-alerter");
        client.send(Kind::ChainMismatch, Fields::default());
        tokio::spawn(client.clone().drain_forever());
        until("a first try", || !seen.lock().unwrap().is_empty()).await;
        assert_eq!((client.queued(), client.sent()), (1, 0), "kept while the alerter is not ready");
        *status.lock().unwrap() = StatusCode::ACCEPTED;
        until("the retry", || client.sent() == 1).await;
        assert_eq!(client.queued(), 0);
        // A refusal for good (not this link's kind, say) is dropped, not retried for ever.
        *status.lock().unwrap() = StatusCode::FORBIDDEN;
        client.send(Kind::RejectedToken, Fields { link: Some("admin-server".into()), ..Fields::default() });
        until("the drop", || client.queued() == 0).await;
        assert_eq!(client.sent(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_queue_is_short_old_events_are_dropped_and_no_token_means_no_call() {
        let (seen, status) = (Seen::default(), Arc::new(Mutex::new(StatusCode::ACCEPTED)));
        let url = alerter(seen.clone(), status).await;
        let none = std::env::temp_dir().join(format!("askbar-alertclient-none-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&none).unwrap();
        let client = AlertClient::new(&url, none.clone(), "admin-alerter");
        for _ in 0..QUEUE_LIMIT + 5 {
            client.send(Kind::ChainMismatch, Fields::default());
        }
        assert_eq!(client.queued(), QUEUE_LIMIT, "the oldest are dropped");
        assert_eq!(
            client.post(&Event { kind: Kind::ChainMismatch, severity: Severity::Critical, at: 0, fields: Fields::default() }).await,
            None
        );
        assert!(seen.lock().unwrap().is_empty(), "no token, no request");
        // An event that waited too long is dropped without a request.
        client.queue.lock().unwrap().iter_mut().for_each(|e| e.at -= STALE_AFTER_SECS + 1);
        tokio::spawn(client.clone().drain_forever());
        until("the stale events to go", || client.queued() == 0).await;
        assert!(seen.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(none);
    }
}
