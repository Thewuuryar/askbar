//! Wire types of the model-gateway API, `/api/model-gateway/v1` (ADR 0054, ADR 0014).
//!
//! What the admin pushes (`PUT /config`, with the provider keys) and what the server sends and gets back
//! (`POST /chat`, a Server-Sent Events stream). A provider's error text is not here: the server gets a
//! code and a class, and the text goes to the auditor sealed.

use crate::{AuditKey, Billing, Budget, CatalogEntry};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

/// One provider as the gateway runs it. Holds the key: no `Debug` output, log line or error shows it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewayProvider {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The provider's API key. Empty for a provider that needs none (a local server). Never returned by
    /// any endpoint.
    #[serde(default)]
    pub api_key: String,
    /// How it is paid for: a subscription's tokens do not count toward the budget (plan step 3.2).
    #[serde(default)]
    pub billing: Billing,
}

impl fmt::Debug for GatewayProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayProvider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("type", &self.kind)
            .field("model", &self.model)
            .field("api_key", &if self.api_key.is_empty() { "(none)" } else { "(set)" })
            .finish_non_exhaustive()
    }
}

/// The approval signing key (plan step P5.3): the PKCS#8 Ed25519 private key, base64url without padding, and the version
/// of the pair, which changes on rotation. Held in memory only; no `Debug` output shows the key.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalSigningKey {
    pub version: u64,
    pub pkcs8: String,
}

impl fmt::Debug for ApprovalSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApprovalSigningKey").field("version", &self.version).field("pkcs8", &"(set)").finish()
    }
}

/// `PUT /config`: everything the gateway needs to run. Replaces the whole of the earlier one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GatewayConfig {
    /// Which configuration this is; `GET /status` reports it back, so the admin sees whether a push is due.
    pub version: u64,
    pub providers: Vec<GatewayProvider>,
    /// The key provider errors are sealed to. Without one the gateway refuses to call a provider: it could not
    /// record what the provider said.
    pub audit_public_key: Option<AuditKey>,
    /// How long a generation may run, so a turn the server never finishes is ended 60 seconds after it.
    #[serde(default = "default_timeout")]
    pub generation_timeout_secs: u32,
    /// The token budget, authoritative here (ADR 0014): the server holds a copy to refuse early and to show usage.
    #[serde(default)]
    pub budget: Budget,
    /// While on, the gateway starts no reply.
    #[serde(default)]
    pub kill_switch: bool,
    /// When the switch goes on, also stop the replies already running (the default).
    #[serde(default = "yes")]
    pub kill_switch_aborts_running: bool,
    /// Users whose replies are stopped, whatever else is allowed: the emergency stop for one person. Their running
    /// replies are stopped too.
    #[serde(default)]
    pub paused_users: Vec<String>,
    /// The approved models (ADR 0014): a reply may use one of these and nothing else, so an empty catalog approves
    /// nothing. The gateway serves it to the server (`GET /catalog`).
    #[serde(default)]
    pub catalog: Vec<CatalogEntry>,
    /// The catalog's default, `provider/model`: what a send that names no model uses. Present exactly when the catalog
    /// is not empty.
    #[serde(default)]
    pub default_model: Option<String>,
    /// The key approvals are signed with. Absent until the admin has one to push; the gateway then signs nothing.
    #[serde(default)]
    pub approval_key: Option<ApprovalSigningKey>,
    /// How long a signed approval lasts (plan step P5.4).
    #[serde(default = "default_approval_ttl")]
    pub approval_ttl_secs: u32,
}

fn default_approval_ttl() -> u32 {
    crate::DEFAULT_APPROVAL_TTL_SECS
}

fn yes() -> bool {
    true
}

fn default_timeout() -> u32 {
    crate::DEFAULT_GENERATION_TIMEOUT_SECS
}

/// One message of a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: &str) -> Self {
        Self { role: role.into(), content: content.into() }
    }
}

/// Drop empty messages and leading assistant turns, and never send two messages with the same role
/// in a row (some APIs reject that).
///
/// Two user messages in a row mean the earlier one got no reply: its turn failed, or was stopped,
/// timed out or interrupted before any text. Only the last of them is kept. Sending the unanswered
/// ones again would repeat whatever made them fail (a question the provider refuses would stick the
/// conversation for good) and mix old questions into the new one; the person retries a question on
/// purpose instead. Consecutive assistant messages are joined.
pub fn normalize(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for m in messages {
        if m.content.trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == m.role && m.role == "user" => *last = m.clone(),
            Some(last) if last.role == m.role => {
                last.content.push_str("\n\n");
                last.content.push_str(&m.content);
            }
            None if m.role != "user" => continue,
            _ => out.push(m.clone()),
        }
    }
    out
}

/// Of a `normalize`d history (question, reply, ..., the new question), the new question and the
/// `turns` question-and-reply pairs before it. 0 keeps everything.
pub fn last_turns(messages: Vec<ChatMessage>, turns: u32) -> Vec<ChatMessage> {
    let keep = (turns as usize).saturating_mul(2).saturating_add(1);
    if turns == 0 || messages.len() <= keep {
        return messages;
    }
    messages[messages.len() - keep..].to_vec()
}

/// `POST /chat`: one generation. The ids are the caller's own; the gateway records them with the turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub provider_id: String,
    /// The model to call, one the catalog approves for this provider.
    pub model: String,
    pub user_id: String,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub space_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// The generation id: the trace id of every span of this reply.
    pub generation_id: String,
    pub system: String,
    pub messages: Vec<ChatMessage>,
}

/// What a failed try means (the router's three classes, ADR 0014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Worth another try: the network, a rate limit, the provider's own trouble.
    Retryable,
    /// The provider does not accept the key (or there is none). The administrator should be told.
    AuthFailed,
    /// The provider refuses the request itself.
    Fatal,
}

/// The events of the `POST /chat` stream, in the order they come: `span` once, then any number of `delta` and
/// `usage`, then one `done` or one `error`. The `event:` line carries the name and `data:` the JSON of the
/// matching variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatEvent {
    /// The turn span the gateway opened, acknowledged by the auditor. The server adds its payloads and its
    /// finish record to it.
    Span { span_id: String, trace_id: String },
    /// A piece of the reply, and which try it belongs to.
    Delta { text: String },
    /// Tokens as the provider has counted them so far.
    Usage { input: Option<u64>, output: Option<u64>, cache_read: Option<u64>, cache_write: Option<u64>, estimated: bool },
    /// The reply is complete.
    Done { finish_reason: Option<String>, ttft_ms: Option<u64>, attempts: u32 },
    /// The generation failed. A code, never the provider's words.
    Error { code: String, class: FailureClass, retry_after_secs: Option<u64>, attempts: u32 },
}

impl ChatEvent {
    /// The `event:` name on the wire.
    pub fn name(&self) -> &'static str {
        match self {
            ChatEvent::Span { .. } => "span",
            ChatEvent::Delta { .. } => "delta",
            ChatEvent::Usage { .. } => "usage",
            ChatEvent::Done { .. } => "done",
            ChatEvent::Error { .. } => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str, c: &str) -> ChatMessage {
        ChatMessage::new(role, c)
    }

    #[test]
    fn last_turns_keeps_the_new_question_and_the_pairs_before_it() {
        let conv = || vec![m("user", "q1"), m("assistant", "a1"), m("user", "q2"), m("assistant", "a2"), m("user", "q3")];
        assert_eq!(last_turns(conv(), 0), conv(), "0 sends everything");
        assert_eq!(last_turns(conv(), 1), vec![m("user", "q2"), m("assistant", "a2"), m("user", "q3")]);
        assert_eq!(last_turns(conv(), 2), conv());
        assert_eq!(last_turns(conv(), 1000), conv(), "more than there is sends all");
        assert_eq!(last_turns(vec![m("user", "only")], 1), vec![m("user", "only")], "the new question is always sent");
        // After normalize, a failed question takes no place in the count.
        let h = normalize(&[
            m("user", "q1"),
            m("assistant", "a1"),
            m("user", "failed"),
            m("user", "q2"),
            m("assistant", "a2"),
            m("user", "q3"),
        ]);
        assert_eq!(last_turns(h, 1), vec![m("user", "q2"), m("assistant", "a2"), m("user", "q3")]);
    }

    #[test]
    fn normalize_keeps_the_last_unanswered_question_and_trims() {
        let out = normalize(&[
            m("assistant", "stray"),
            m("user", "a"),
            m("user", "b"),
            m("assistant", "  "),
            m("assistant", "c"),
            m("user", "d"),
        ]);
        assert_eq!(out, vec![m("user", "b"), m("assistant", "c"), m("user", "d")], "a got no reply, so only b is sent");
        assert!(normalize(&[m("assistant", "only")]).is_empty());
        // A failed question is not carried into the next turn.
        let out = normalize(&[m("user", "hello"), m("assistant", "hi"), m("user", "fail"), m("user", "429"), m("user", "hello again")]);
        assert_eq!(out, vec![m("user", "hello"), m("assistant", "hi"), m("user", "hello again")]);
        // Two assistant messages in a row are still joined.
        assert_eq!(normalize(&[m("user", "q"), m("assistant", "x"), m("assistant", "y")]), vec![m("user", "q"), m("assistant", "x\n\ny")]);
    }

    #[test]
    fn a_provider_never_shows_its_key_in_debug_output() {
        let p = GatewayProvider {
            id: "p".into(),
            name: "n".into(),
            kind: "openai".into(),
            base_url: String::new(),
            model: "m".into(),
            max_tokens: None,
            temperature: None,
            headers: Default::default(),
            api_key: "sk-marker-secret".into(),
            billing: Billing::Metered,
        };
        let cfg = GatewayConfig {
            version: 1,
            providers: vec![p.clone()],
            audit_public_key: None,
            generation_timeout_secs: 300,
            budget: Budget::default(),
            kill_switch: false,
            kill_switch_aborts_running: true,
            paused_users: vec![],
            catalog: vec![],
            default_model: None,
            approval_key: Some(ApprovalSigningKey { version: 1, pkcs8: "marker-approval-key".into() }),
            approval_ttl_secs: 60,
        };
        for shown in [format!("{p:?}"), format!("{p:#?}"), format!("{cfg:?}")] {
            assert!(!shown.contains("sk-marker-secret"), "{shown}");
            assert!(!shown.contains("marker-approval-key"), "{shown}");
            assert!(shown.contains("(set)"));
        }
    }

    #[test]
    fn a_configuration_from_before_the_budget_has_no_limits_no_stop_and_aborts_running_ones() {
        let cfg: GatewayConfig = serde_json::from_str(r#"{"version":1,"providers":[],"audit_public_key":null}"#).unwrap();
        assert_eq!(
            (cfg.budget, cfg.kill_switch, cfg.kill_switch_aborts_running, cfg.paused_users),
            (Budget::default(), false, true, vec![])
        );
        assert_eq!((cfg.catalog, cfg.default_model), (vec![], None), "no catalog approves no model");
        let p: GatewayProvider = serde_json::from_str(r#"{"id":"p","name":"n","type":"openai","model":"m"}"#).unwrap();
        assert_eq!(p.billing, Billing::Metered, "metered unless it says it is a subscription");
    }

    #[test]
    fn events_round_trip_and_each_names_itself() {
        let events = [
            ChatEvent::Span { span_id: "s".into(), trace_id: "t".into() },
            ChatEvent::Delta { text: "hi".into() },
            ChatEvent::Usage { input: Some(1), output: None, cache_read: None, cache_write: None, estimated: false },
            ChatEvent::Done { finish_reason: Some("stop".into()), ttft_ms: Some(5), attempts: 1 },
            ChatEvent::Error { code: "http_401".into(), class: FailureClass::AuthFailed, retry_after_secs: None, attempts: 1 },
        ];
        for e in events {
            let wire = serde_json::to_value(&e).unwrap();
            assert_eq!(wire["event"], e.name(), "the tag is the event name");
            assert_eq!(serde_json::from_value::<ChatEvent>(wire).unwrap(), e);
        }
    }
}
