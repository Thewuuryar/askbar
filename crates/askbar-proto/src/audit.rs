//! Wire types of the audit API, `/api/audit/v1` (ADR 0051, ADR 0010). What a service sends the auditor to
//! record, and what the admin reads back.
//!
//! Sealed content is only ever ciphertext here: nothing in this file can hold a prompt or a reply as text.
//! The caller of a request is the one its token belongs to; none of these bodies names one.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a span is. The set is fixed; the auditor's store checks it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// A whole generation, from start to finish.
    Turn,
    /// A check before it runs, or time spent waiting.
    Gate,
    /// One request to one provider.
    Attempt,
    Tool,
    AgentStep,
}

impl SpanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SpanKind::Turn => "turn",
            SpanKind::Gate => "gate",
            SpanKind::Attempt => "attempt",
            SpanKind::Tool => "tool",
            SpanKind::AgentStep => "agent_step",
        }
    }
}

/// `POST /spans`: a span has begun. Its id is made by the caller, so a retry of an append whose answer was
/// lost records it once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanStart {
    pub id: String,
    /// The generation id: every span of one reply shares it.
    pub trace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub kind: SpanKind,
    pub name: String,
    /// Unix seconds, and milliseconds for drawing a trace to scale.
    pub started_at: i64,
    pub started_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// Small metadata only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Value>,
}

/// `POST /spans/{id}/finish`: how a span ended. A field left out keeps what the start recorded. Only the
/// first finish of a span counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpanFinish {
    pub status: String,
    pub ended_at: i64,
    pub ended_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_cached: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Merged into the span's attributes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs: Option<Value>,
}

/// `POST /spans/{id}/payloads`: sealed content for a span (ADR 0011). Ciphertext only, in base64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SealedPayload {
    /// What it holds: `request`, `reply`, `error`, and so on.
    pub role: String,
    /// The id of the public key it was sealed to.
    pub key_id: String,
    pub ciphertext_b64: String,
}

/// `POST /admin-actions`: something the admin did, or refused to do (ADR 0010). A claim: the services that
/// applied a change record it themselves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminActionStart {
    pub id: String,
    pub at: i64,
    /// What kind of change: `settings`, `restart`, `config_push`, `secret`, `retention`, `backup`, ...
    pub category: String,
    pub action: String,
    /// `done` or `refused`.
    pub outcome: String,
    /// Metadata about the change, never a secret or message content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// `POST /hello`: a service announces the boot ID of the process that is now talking. The auditor ends that
/// service's earlier unfinished spans as interrupted (ADR 0010 rule 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub boot_id: String,
}

/// The answer to an append: the id it was stored under, once it is committed. An append is acknowledged only
/// after that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appended {
    pub id: String,
}

/// What `hello` ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    /// How many unfinished spans of this service's earlier processes were ended as interrupted.
    pub interrupted: u64,
}

/// `GET /retention` and `PUT /retention`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retention {
    pub days: u32,
    /// The least it can be set to. A request below it is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor_days: Option<u32>,
}

/// One row of `GET /traces`: a generation, newest first, with the provider and timings of its last attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceSummary {
    pub trace_id: String,
    pub span_id: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub user_id: Option<String>,
    pub client_id: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub tokens_cached: Option<i64>,
    pub error_code: Option<String>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub ttft_ms: Option<i64>,
    pub latency_ms: Option<i64>,
}

/// One span of `GET /traces/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Span {
    pub id: String,
    pub parent_id: Option<String>,
    pub kind: SpanKind,
    pub name: String,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub tokens_cached: Option<i64>,
    pub ttft_ms: Option<i64>,
    pub error_code: Option<String>,
    pub user_id: Option<String>,
    pub client_id: Option<String>,
    pub attrs: Option<Value>,
    /// How many sealed payloads it has. Their content is read with `GET /spans/{id}/payloads`.
    pub sealed: i64,
}

/// One sealed payload of `GET /spans/{id}/payloads`, as stored. Only the holder of the audit passphrase can
/// turn it into text, and not here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredPayload {
    pub role: String,
    pub key_id: String,
    pub ciphertext_b64: String,
}

/// One row of `GET /admin-actions`, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdminActionRecord {
    pub id: String,
    pub at: i64,
    /// Which token appended it: the caller, from the token, never from the body.
    pub service: String,
    pub category: String,
    pub action: String,
    pub outcome: String,
    pub detail: Option<Value>,
}

/// One row of `GET /prunes`, oldest first: retention deleted records. Each prune is recorded in the same
/// transaction as its delete, so a deletion that is not retention shows as a gap with no record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PruneRecord {
    pub at: i64,
    /// Records that started before this were due to go.
    pub cutoff: i64,
    pub spans: i64,
    pub payloads: i64,
}

/// One row of `GET /token-counts`: what was used, by provider and model. The auditor holds no prices; the
/// admin applies them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount {
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub requests: i64,
    pub tokens_in: i64,
    pub tokens_out: i64,
    pub tokens_cached: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) -> Value {
        let wire = serde_json::to_value(v).unwrap();
        let back: T = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(&back, v, "{wire}");
        wire
    }

    #[test]
    fn every_wire_type_survives_a_round_trip_through_json() {
        let start = SpanStart {
            id: "01H".into(),
            trace_id: "01T".into(),
            parent_id: Some("01P".into()),
            kind: SpanKind::AgentStep,
            name: "attempt 1".into(),
            started_at: 1_700_000_000,
            started_ms: 1_700_000_000_123,
            provider_id: Some("p1".into()),
            model: Some("m".into()),
            user_id: Some("u".into()),
            client_id: None,
            attrs: Some(json!({ "retry": 1 })),
        };
        let wire = round_trip(&start);
        assert_eq!(wire["kind"], "agent_step", "kinds are snake_case on the wire");
        assert!(wire.get("client_id").is_none(), "an empty option is left out");
        round_trip(&SpanFinish {
            status: "done".into(),
            ended_at: 1,
            ended_ms: 1000,
            provider_id: None,
            model: Some("m".into()),
            tokens_in: Some(10),
            tokens_out: Some(20),
            tokens_cached: Some(0),
            ttft_ms: Some(90),
            error_code: None,
            attrs: Some(json!({ "latency_ms": 400 })),
        });
        round_trip(&SealedPayload { role: "request".into(), key_id: "abcd".into(), ciphertext_b64: "AAEC".into() });
        round_trip(&AdminActionStart {
            id: "01A".into(),
            at: 5,
            category: "settings".into(),
            action: "set kill switch".into(),
            outcome: "done".into(),
            detail: Some(json!({ "kill_switch": true })),
        });
        round_trip(&PruneRecord { at: 9, cutoff: 5, spans: 2, payloads: 3 });
        round_trip(&Hello { boot_id: "01B".into() });
        round_trip(&Appended { id: "01H".into() });
        round_trip(&HelloResult { interrupted: 2 });
        round_trip(&Retention { days: 30, floor_days: Some(7) });
        round_trip(&TokenCount { provider_id: Some("p".into()), model: None, requests: 3, tokens_in: 4, tokens_out: 5, tokens_cached: 6 });
        round_trip(&AdminActionRecord {
            id: "01A".into(),
            at: 5,
            service: "admin".into(),
            category: "settings".into(),
            action: "x".into(),
            outcome: "refused".into(),
            detail: None,
        });
        round_trip(&StoredPayload { role: "reply".into(), key_id: "k".into(), ciphertext_b64: "AA==".into() });
    }

    #[test]
    fn a_trace_summary_and_a_span_read_what_the_store_writes_and_a_kind_outside_the_set_is_refused() {
        let summary: TraceSummary = serde_json::from_value(json!({
            "trace_id": "t", "span_id": "s", "status": "done", "started_at": 1, "ended_at": null, "user_id": null,
            "client_id": null, "tokens_in": null, "tokens_out": null, "tokens_cached": null, "error_code": null,
            "provider_id": null, "model": null, "ttft_ms": null, "latency_ms": null
        }))
        .unwrap();
        assert_eq!(summary.ended_at, None);
        let bad = json!({ "id": "x", "trace_id": "t", "kind": "admin_action", "name": "n", "started_at": 1, "started_ms": 1 });
        assert!(serde_json::from_value::<SpanStart>(bad).is_err(), "a span is one of the five kinds");
        for k in [SpanKind::Turn, SpanKind::Gate, SpanKind::Attempt, SpanKind::Tool, SpanKind::AgentStep] {
            assert_eq!(serde_json::to_value(k).unwrap(), k.as_str());
        }
    }
}
