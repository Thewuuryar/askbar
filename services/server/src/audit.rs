//! The server's records in the audit trail (ADR 0010, ADR 0054): the gates it passes before a generation, the
//! sealed request and reply it adds to the turn span the gateway opened, and how that turn ended.
//!
//! The records go to the auditor over HTTP, through a bounded in-memory queue that a background task empties in order
//! (`askbar_service::audit_client`). Only `model-gateway` opens model-call spans; the server's link opens `gate`
//! spans and finishes and adds payloads to spans. A server that cannot reach the auditor answers `503 audit_backlog`
//! to a new generation instead of working unrecorded.

use askbar_proto::audit as wire;
use askbar_service::audit_client::AuditClient;
use std::sync::Arc;

pub use crate::seal::Sealed;

/// What a span is. The set is fixed (the auditor checks it too).
pub use wire::SpanKind as Kind;

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// A span that has begun, built up with the methods below and handed to `Recorder::start_span`.
#[derive(Debug, Clone)]
pub struct SpanStart {
    inner: wire::SpanStart,
}

impl SpanStart {
    /// A span in trace `trace_id` (the generation id), starting now, with a fresh id.
    pub fn new(trace_id: &str, kind: Kind, name: &str) -> SpanStart {
        SpanStart {
            inner: wire::SpanStart {
                id: askbar_service::ids::new_id(),
                trace_id: trace_id.to_string(),
                parent_id: None,
                kind,
                name: name.to_string(),
                started_at: now_secs(),
                started_ms: now_ms(),
                provider_id: None,
                model: None,
                user_id: None,
                client_id: None,
                attrs: None,
            },
        }
    }

    pub fn who(mut self, user_id: &str, client_id: Option<&str>) -> SpanStart {
        self.inner.user_id = Some(user_id.to_string());
        self.inner.client_id = client_id.map(str::to_string);
        self
    }

    pub fn attrs(mut self, attrs: &serde_json::Value) -> SpanStart {
        self.inner.attrs = Some(attrs.clone());
        self
    }

    pub fn id(&self) -> &str {
        &self.inner.id
    }
}

/// How a span ended. Fields left `None` keep what the start recorded.
#[derive(Debug, Clone, Default)]
pub struct SpanFinish {
    pub id: String,
    pub status: String,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub tokens_cached: Option<u64>,
    pub error_code: Option<String>,
    /// Merged into the span's attributes (a key given here replaces the same key from the start).
    pub attrs_json: Option<String>,
}

impl SpanFinish {
    /// Ends span `id` now with `status`.
    pub fn new(id: &str, status: &str) -> SpanFinish {
        SpanFinish { id: id.to_string(), status: status.to_string(), ..Default::default() }
    }

    pub fn error_code(mut self, code: &str) -> SpanFinish {
        self.error_code = Some(code.to_string());
        self
    }

    fn into_wire(self) -> (String, wire::SpanFinish) {
        let attrs = self.attrs_json.and_then(|a| serde_json::from_str(&a).ok());
        (
            self.id,
            wire::SpanFinish {
                status: self.status,
                ended_at: now_secs(),
                ended_ms: now_ms(),
                provider_id: None,
                model: None,
                tokens_in: self.tokens_in,
                tokens_out: self.tokens_out,
                tokens_cached: self.tokens_cached,
                ttft_ms: None,
                error_code: self.error_code,
                attrs,
            },
        )
    }
}

/// Where the server's records go. `noop` records nothing (a state nothing sends records from); `new` sends them to the
/// auditor through `client`.
#[derive(Clone, Default)]
pub struct Recorder {
    client: Option<Arc<AuditClient>>,
    /// Other clients whose queues `flush` waits for too (tests only: the gateway's, in the rig).
    peers: Vec<Arc<AuditClient>>,
}

impl Recorder {
    pub fn noop() -> Recorder {
        Recorder { client: None, peers: Vec::new() }
    }

    pub fn new(client: Arc<AuditClient>) -> Recorder {
        Recorder { client: Some(client), peers: Vec::new() }
    }

    /// Tests only: `flush` waits for `peer`'s queue to empty as well as this recorder's.
    #[doc(hidden)]
    pub fn also_flush(mut self, peer: Arc<AuditClient>) -> Recorder {
        self.peers.push(peer);
        self
    }

    /// Waits (up to ten seconds) until every record queued so far has been taken by the auditor. For tests, which read
    /// the store right after a reply and need what it left to have arrived.
    #[doc(hidden)]
    pub async fn flush(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let queued = || self.client.iter().chain(self.peers.iter()).map(|c| c.queued()).sum::<usize>();
        while queued() > 0 && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Queue a span's start and return its id. The finish record follows it in the queue.
    pub fn start_span(&self, s: SpanStart) -> String {
        let id = s.inner.id.clone();
        if let Some(c) = &self.client {
            c.start(s.inner);
        }
        id
    }

    pub fn finish_span(&self, f: SpanFinish) {
        if let Some(c) = &self.client {
            let (id, wire) = f.into_wire();
            c.finish(&id, wire);
        }
    }

    /// Queue a sealed payload for a span (`request`, `reply`).
    pub fn payload(&self, span_id: &str, role: &str, sealed: Sealed) {
        if let Some(c) = &self.client {
            c.payload(span_id, role, &sealed.key_id, &sealed.bytes);
        }
    }

    /// Whether a new generation may start: the auditor can take what it will record. A no-op recorder always can.
    pub fn healthy(&self) -> bool {
        self.client.as_ref().is_none_or(|c| c.healthy())
    }

    /// Record the configuration version this server now runs, for the auditor (ADR 0028 rule 5).
    pub fn config_applied(&self, version: i64) {
        if let Some(c) = &self.client {
            c.config_applied(version);
        }
    }

    /// The client, for the tasks that run it (hello, draining the queue).
    pub fn client(&self) -> Option<&Arc<AuditClient>> {
        self.client.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_noop_recorder_is_healthy_and_hands_back_ids() {
        let r = Recorder::noop();
        assert!(r.healthy());
        let id = r.start_span(SpanStart::new("t", Kind::Gate, "budget").who("u", Some("c")));
        r.finish_span(SpanFinish::new(&id, "done").error_code("x"));
        assert_eq!(id.len(), 26, "a ULID");
    }

    #[test]
    fn a_finish_keeps_its_attributes_as_json_and_its_error_code() {
        let mut f = SpanFinish::new("s1", "error").error_code("http_500");
        f.attrs_json = Some(serde_json::json!({ "truncated": true }).to_string());
        f.tokens_in = Some(3);
        let (id, w) = f.into_wire();
        assert_eq!((id.as_str(), w.status.as_str(), w.error_code.as_deref(), w.tokens_in), ("s1", "error", Some("http_500"), Some(3)));
        assert_eq!(w.attrs.unwrap()["truncated"], true);
    }
}
