//! Wire types shared by the Askbar server and client.
//! IDs are server-generated ULIDs; timestamps are UTC unix seconds.

/// What the model is told when no system prompt has been set.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a fast, concise assistant in a quick-question popup. \
Answer directly. Use Markdown when it helps. You cannot access the user's computer, files, or the internet.";

mod sse;
pub use sse::{SseEvent, SseParser};

pub mod paths;
pub mod problem;
pub use problem::{ErrorCode, Problem};

#[cfg(feature = "axum")]
pub mod http;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub api_version: u32,
    pub min_client: String,
}

/// A configured provider as seen by clients. Never carries key material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    /// Stable id; chats and preferences refer to a provider by this, never by name.
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub message_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

/// Provider metadata in a snapshot. Never carries a key: those go through their own endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotProvider {
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
    pub headers: std::collections::BTreeMap<String, String>,
    /// How the provider is paid for. A `subscription` costs nothing per token; a `metered` one
    /// does, so it is not used as a fallback unless `allow_as_fallback` says so.
    #[serde(default)]
    pub billing: Billing,
    /// A metered provider may serve as a fallback for another only when this is true.
    #[serde(default)]
    pub allow_as_fallback: bool,
    /// Price per million tokens, in whatever currency the administrator thinks in. Missing means
    /// unknown (cost is then reported as unknown, never as 0).
    #[serde(default)]
    pub price_in: Option<f64>,
    #[serde(default)]
    pub price_out: Option<f64>,
    #[serde(default)]
    pub price_cached_in: Option<f64>,
}

/// How a provider is paid for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Billing {
    /// Per token; the safe assumption when nothing is said.
    #[default]
    Metered,
    /// A flat plan; tokens cost nothing extra.
    Subscription,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotPairCode {
    /// SHA-256 (hex) of the code; the code itself is never sent.
    pub hash: String,
    pub user_id: String,
    // `device_name` is the old key, still read from a snapshot stored by an earlier version. names-ok
    #[serde(default, alias = "device_name")] // names-ok
    pub client_name: String,
    pub expires_at: i64,
}

/// Spending limits. `None` means no limit.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub daily_tokens: Option<u64>,
    #[serde(default)]
    pub monthly_tokens: Option<u64>,
}

/// Everything the server needs from the admin service to run, as one document.
/// The admin service owns it and pushes the whole thing; the server stores it as-is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Set by the server: 1 for the first snapshot it accepts, then up by one each time.
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub providers: Vec<SnapshotProvider>,
    #[serde(default)]
    pub default_provider_id: Option<String>,
    /// Provider ids to try, in order, when the first choice fails.
    #[serde(default)]
    pub fallback_order: Vec<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Server-wide display defaults.
    #[serde(default)]
    pub display: DisplaySettings,
    /// Per-user display defaults, by user id.
    #[serde(default)]
    pub user_display_defaults: std::collections::BTreeMap<String, DisplaySettings>,
    #[serde(default)]
    pub pair_codes: Vec<SnapshotPairCode>,
    // `revoked_device_ids` is the old key, still read from a snapshot stored by an earlier version. names-ok
    #[serde(default, alias = "revoked_device_ids")] // names-ok
    pub revoked_client_ids: Vec<String>,
    #[serde(default)]
    pub budget: Budget,
    /// When true the server refuses new chats.
    #[serde(default)]
    pub kill_switch: bool,
    /// When the kill switch is turned on: true stops the replies already running (they end
    /// `cancelled`, with error code `kill_switch`), false lets them finish. Queued messages are
    /// cleared either way, and nothing new starts.
    #[serde(default = "default_true")]
    pub kill_switch_aborts_running: bool,
    /// How long audit records are kept. At least 7.
    pub audit_retention_days: u32,
    /// Seconds a generation may run before it is stopped (30 to 3600). Read when it starts.
    #[serde(default = "default_generation_timeout_secs")]
    pub generation_timeout_secs: u32,
    /// The public key the server seals audit content to. Only ever the public half: the private
    /// key does not exist anywhere outside the administrator's passphrase. `None` until the admin
    /// has made one; the server then refuses to start replies.
    #[serde(default)]
    pub audit_public_key: Option<AuditKey>,
}

/// An audit public key and its id (the first 8 bytes of the SHA-256 of the key, in hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditKey {
    pub key_id: String,
    /// The 32-byte X25519 public key, as 64 hex characters.
    pub public_key: String,
}

fn default_true() -> bool {
    true
}

/// Five minutes.
pub const DEFAULT_GENERATION_TIMEOUT_SECS: u32 = 300;

fn default_generation_timeout_secs() -> u32 {
    DEFAULT_GENERATION_TIMEOUT_SECS
}

/// A user's preferences, shared by all of their clients.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// Id of the provider new chats use; `None` means "the server's default".
    pub active_provider_id: Option<String>,
}

/// One message that matched a history search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub conversation_id: String,
    pub message_id: String,
    pub role: String,
    /// A short excerpt around the match.
    pub snippet: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationDetail {
    #[serde(flatten)]
    pub conversation: Conversation,
    pub messages: Vec<Message>,
    /// Replies still running or waiting their turn; left out when there are none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending: Vec<PendingGeneration>,
}

/// A generation that has not finished: `running`, or `queued` behind another reply.
/// `prompt` is the question; for a queued one it is not in the messages yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingGeneration {
    pub generation_id: String,
    pub conversation_id: String,
    pub status: String,
    /// The reply so far, as last saved by the server (it can trail the live stream by a second).
    pub text: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

/// Display defaults served to clients; each client may override fields locally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub theme: String,
    pub width: u32,
    pub max_height: u32,
    pub history_width: u32,
    pub top_percent: f64,
    pub hide_on_blur: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self { theme: "auto".into(), width: 720, max_height: 640, history_width: 260, top_percent: 0.22, hide_on_blur: true }
    }
}

/// `GET /api/client/v1/settings`. The system prompt is deliberately not part of it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Settings {
    pub display: DisplaySettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// Id of the provider to answer with.
    pub provider: String,
    pub text: String,
    /// Chosen by the client for each send. A repeat from the same client attaches to the
    /// generation it started instead of starting another, so a retry never doubles a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
}

/// Server-Sent Event payloads for `POST /api/client/v1/chat`; the SSE event name is the `event` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatEvent {
    Meta {
        conversation_id: String,
        /// The generation running this reply; resume with `GET /api/client/v1/generations/{id}/stream`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation_id: Option<String>,
        /// True when another reply in this conversation is still running: this message waits its
        /// turn, and its question is not in the conversation yet.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        queued: bool,
    },
    /// Which provider is serving this reply, sent before the first delta (and again if a fallback
    /// changes it before any text). Not a delta: it has no SSE `id`. Clients that do not know it
    /// ignore it.
    Provider {
        id: String,
        name: String,
    },
    Delta {
        text: String,
    },
    Done,
    /// `code` is the stable reason (`timed_out`, `interrupted`, `kill_switch`, `provider_error`, `internal`); clients
    /// branch on it and show `message`. Absent from servers that predate it.
    Error {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
}

/// Header every client sends on API calls other than health.
pub const CLIENT_VERSION_HEADER: &str = "x-askbar-client-version";

/// Parse "1.2.3" (a leading "v" and any "-suffix" are ignored; missing parts are 0).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let mut next = || -> Option<u64> { parts.next().map_or(Some(0), |p| p.parse().ok()) };
    let out = (next()?, next()?, next()?);
    parts.next().is_none().then_some(out)
}

pub const PAIR_PREFIX: &str = "askbar-pair:";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PairPayload {
    url: String,
    code: String,
}

/// `askbar-pair:<base64url(json{url, code})>`
pub fn encode_pairing(url: &str, code: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let json = serde_json::to_vec(&PairPayload { url: url.into(), code: code.into() }).expect("plain struct serializes");
    format!("{PAIR_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
}

/// Returns `(url, code)`, or `None` if the string is not a pairing string.
pub fn decode_pairing(s: &str) -> Option<(String, String)> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let body = s.trim().strip_prefix(PAIR_PREFIX)?;
    let p: PairPayload = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
    (!p.url.is_empty() && !p.code.is_empty()).then_some((p.url, p.code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("v1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("1.2.3-beta+4"), Some((1, 2, 3)));
        assert!(parse_version("0.9.0") < parse_version("0.10.0"));
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn pairing_string_round_trip() {
        let s = encode_pairing("https://askbar.example.com:8443", "abc_-123");
        assert!(s.starts_with("askbar-pair:"));
        assert_eq!(decode_pairing(&format!("  {s}\n")), Some(("https://askbar.example.com:8443".into(), "abc_-123".into())));
        assert_eq!(decode_pairing("askbar-pair:!!!"), None);
        assert_eq!(decode_pairing("hello"), None);
        assert_eq!(decode_pairing(&encode_pairing("", "x")), None);
    }

    #[test]
    fn provider_info_uses_type_key() {
        let p = ProviderInfo { id: "p1".into(), name: "a".into(), kind: "openai".into(), model: "m".into() };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"id":"p1","name":"a","type":"openai","model":"m"}"#);
    }

    #[test]
    fn chat_event_round_trip() {
        for e in [
            ChatEvent::Meta { conversation_id: "x".into(), generation_id: None, queued: false },
            ChatEvent::Meta { conversation_id: "x".into(), generation_id: Some("g".into()), queued: true },
            ChatEvent::Provider { id: "p1".into(), name: "OpenAI".into() },
            ChatEvent::Delta { text: "hi".into() },
            ChatEvent::Done,
            ChatEvent::Error { message: "bad".into(), code: None },
            ChatEvent::Error { message: "slow".into(), code: Some("timed_out".into()) },
        ] {
            let s = serde_json::to_string(&e).unwrap();
            assert_eq!(serde_json::from_str::<ChatEvent>(&s).unwrap(), e);
        }
        assert_eq!(serde_json::to_string(&ChatEvent::Done).unwrap(), r#"{"event":"done"}"#);
    }

    #[test]
    fn pending_is_left_out_when_empty_and_optional_to_read() {
        let d = ConversationDetail {
            conversation: Conversation { id: "c".into(), title: "t".into(), updated_at: 1, message_count: 0 },
            messages: vec![],
            pending: vec![],
        };
        let s = serde_json::to_string(&d).unwrap();
        assert!(!s.contains("pending"), "{s}");
        assert_eq!(serde_json::from_str::<ConversationDetail>(&s).unwrap(), d);
    }

    /// Every object key anywhere in `v`, lower-cased.
    fn all_keys(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                for (k, v) in m {
                    out.push(k.to_lowercase());
                    all_keys(v, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|v| all_keys(v, out)),
            _ => {}
        }
    }

    #[test]
    fn an_older_provider_without_billing_or_prices_parses_as_metered_with_unknown_prices() {
        let p: SnapshotProvider = serde_json::from_str(r#"{"id":"p","name":"n","type":"openai","model":"m"}"#).unwrap();
        assert_eq!((p.billing, p.allow_as_fallback), (Billing::Metered, false));
        assert_eq!((p.price_in, p.price_out, p.price_cached_in), (None, None, None));
        let p: SnapshotProvider = serde_json::from_str(
            r#"{"id":"p","name":"n","type":"openai","model":"m","billing":"subscription","allow_as_fallback":true,"price_in":2.5,"price_out":10,"price_cached_in":0.25}"#,
        )
        .unwrap();
        assert_eq!(
            (p.billing, p.allow_as_fallback, p.price_in, p.price_out, p.price_cached_in),
            (Billing::Subscription, true, Some(2.5), Some(10.0), Some(0.25))
        );
        assert!(
            serde_json::from_str::<SnapshotProvider>(r#"{"id":"p","name":"n","type":"openai","model":"m","billing":"free"}"#).is_err(),
            "only metered or subscription"
        );
        // And it serializes back to the same words.
        assert!(serde_json::to_string(&p).unwrap().contains(r#""billing":"subscription""#));
    }

    #[test]
    fn the_snapshot_carries_a_public_key_and_nothing_that_could_be_a_secret() {
        // A snapshot with every optional part filled in, so every field name shows up.
        let mut s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30}"#).unwrap();
        s.audit_public_key = Some(AuditKey { key_id: "0011223344556677".into(), public_key: "ab".repeat(32) });
        s.default_provider_id = Some("p".into());
        s.system_prompt = Some("x".into());
        s.providers.push(SnapshotProvider {
            id: "p".into(),
            name: "n".into(),
            kind: "openai".into(),
            base_url: String::new(),
            model: "m".into(),
            max_tokens: Some(1),
            temperature: Some(0.5),
            headers: Default::default(),
            billing: Billing::Subscription,
            allow_as_fallback: true,
            price_in: Some(1.0),
            price_out: Some(2.0),
            price_cached_in: Some(0.5),
        });
        let mut keys = Vec::new();
        all_keys(&serde_json::to_value(&s).unwrap(), &mut keys);
        for k in &keys {
            for banned in ["secret", "private", "passphrase", "password", "salt", "argon", "api_key"] {
                assert!(!k.contains(banned), "the snapshot has a field named {k:?}");
            }
            // (`daily_tokens` and `monthly_tokens` are budget counts, not credentials.)
            assert!(!matches!(k.as_str(), "token" | "bearer" | "auth_token" | "access_token"), "the snapshot has a field named {k:?}");
        }
        assert!(keys.contains(&"audit_public_key".to_string()));
        // The key itself is 32 bytes: a public key, not a seed or a derived secret of another size.
        assert_eq!(s.audit_public_key.as_ref().unwrap().public_key.len(), 64);
    }

    #[test]
    fn the_audit_public_key_round_trips_and_an_older_snapshot_without_it_still_parses() {
        let mut s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30}"#).unwrap();
        assert_eq!(s.audit_public_key, None);
        s.audit_public_key = Some(AuditKey { key_id: "0011223344556677".into(), public_key: "ab".repeat(32) });
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), s);
        assert!(json.contains("audit_public_key") && !json.contains("secret") && !json.contains("passphrase"));
    }

    #[test]
    fn a_snapshot_without_the_abort_setting_aborts_running_replies() {
        let s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30}"#).unwrap();
        assert!(s.kill_switch_aborts_running, "the safer default: the switch really stops things");
        let s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30, "kill_switch_aborts_running": false}"#).unwrap();
        assert!(!s.kill_switch_aborts_running);
    }

    #[test]
    fn a_snapshot_without_a_generation_timeout_loads_as_300() {
        let s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30}"#).unwrap();
        assert_eq!(s.generation_timeout_secs, 300);
        assert_eq!(s.generation_timeout_secs, DEFAULT_GENERATION_TIMEOUT_SECS);
        let s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30, "generation_timeout_secs": 90}"#).unwrap();
        assert_eq!(s.generation_timeout_secs, 90);
    }

    #[test]
    fn meta_without_a_generation_id_still_parses() {
        let e: ChatEvent = serde_json::from_str(r#"{"event":"meta","conversation_id":"c"}"#).unwrap();
        assert_eq!(e, ChatEvent::Meta { conversation_id: "c".into(), generation_id: None, queued: false });
    }

    #[test]
    fn chat_request_conversation_optional() {
        let r: ChatRequest = serde_json::from_str(r#"{"provider":"p","text":"t"}"#).unwrap();
        assert_eq!(r.conversation_id, None);
        assert_eq!(r.client_request_id, None);
    }
}
