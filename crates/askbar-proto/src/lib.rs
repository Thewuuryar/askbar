//! Wire types shared by the Askbar server and client.
//! IDs are server-generated ULIDs; timestamps are UTC unix seconds.

/// What the model is told when no system prompt has been set.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You are a fast, concise assistant in a quick-question popup. \
Answer directly. Use Markdown when it helps. You cannot access the user's computer, files, or the internet.";

mod sse;
pub use sse::{SseEvent, SseParser};

pub mod audit;
pub mod gateway;
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

/// A space the caller is a member of (`GET /api/client/v1/spaces`, ADR 0009). Each user has one
/// `personal` space, listed first; `shared` spaces come later (plan step 1.12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceInfo {
    pub id: String,
    /// `personal` or `shared`.
    pub kind: String,
    /// Empty for a personal space.
    pub name: String,
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
    /// For a reply: the provider and model that wrote it, as they were then. Left out for questions
    /// and for replies stored before servers recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ReplyProvider>,
    /// For a question: why its turn failed (it got no reply, or only part of one). Left out when it
    /// did not fail, and for questions stored before servers recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed: Option<MessageFailure>,
}

/// Why a question's turn failed. `code` is the same stable reason as the stream's `error` event
/// (`timed_out`, `interrupted`, `kill_switch`, `provider_error`, `internal`); `message` is a sentence
/// for a person, always the server's own words, never a provider's error text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFailure {
    pub code: String,
    pub message: String,
}

/// Which provider wrote a reply, and with which model. A copy taken when the reply was stored, so a
/// provider renamed or deleted later does not change it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyProvider {
    pub id: String,
    pub name: String,
    pub model: String,
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
    /// How the provider is paid for. A `subscription` costs nothing per token; a `metered` one does.
    /// (`allow_as_fallback`, sent by earlier versions, is ignored: v1 has no provider fallback, ADR 0014.)
    #[serde(default)]
    pub billing: Billing,
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

/// A user, as the admin pushes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotUser {
    pub id: String,
    pub name: String,
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
    // `fallback_order`, in snapshots stored by earlier versions, is ignored: there is no provider
    // fallback in v1 (ADR 0014). A failed provider is an error, not another provider's answer.
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Server-wide display defaults.
    #[serde(default)]
    pub display: DisplaySettings,
    /// Per-user display defaults, by user id.
    #[serde(default)]
    pub user_display_defaults: std::collections::BTreeMap<String, DisplaySettings>,
    /// The users the admin made (ADR 0008). The server keeps them; a pairing key names one of them.
    #[serde(default)]
    pub users: Vec<SnapshotUser>,
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
    /// How many earlier questions, each with its reply, are sent with a new question (0 to
    /// 1000). 0 sends the whole conversation, which is also what a snapshot without it means.
    #[serde(default)]
    pub history_turns: u32,
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
    /// Show a conversation newest first, just under the input bar (each question still above its
    /// reply). False, the default, is oldest first.
    pub newest_first: bool,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self {
            theme: "auto".into(),
            width: 720,
            max_height: 640,
            history_width: 260,
            top_percent: 0.22,
            hide_on_blur: true,
            newest_first: false,
        }
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

/// Server-Sent Event payloads for `POST /api/client/v1/spaces/{space_id}/chat`; the SSE event name is the `event` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatEvent {
    Meta {
        conversation_id: String,
        /// The generation running this reply; resume with `GET /api/client/v1/spaces/{space_id}/generations/{id}/stream`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        generation_id: Option<String>,
        /// True when another reply in this conversation is still running: this message waits its
        /// turn, and its question is not in the conversation yet.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        queued: bool,
    },
    /// Which provider is serving this reply, sent once before the first delta. It is always the one
    /// asked for (there is no provider fallback). Not a delta: it has no SSE `id`. Clients that do
    /// not know it ignore it.
    Provider {
        id: String,
        name: String,
        /// The model the provider is set to use. Left out by servers from before it was sent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
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

/// `POST /api/client/v1/me/pairing-keys`: a paired client asks for a code to pair another client of
/// the same user (ADR 0008). The client builds the pairing key from the code and the server address
/// and certificate pin it already holds; the server knows neither.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingKeyRequest {
    /// What the new client will be called; empty means "client".
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingKeyResponse {
    /// The one-time code, as in [`Pairing::code`].
    pub code: String,
    /// Seconds until it expires.
    pub expires_in: i64,
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

/// What a pairing key carries: `askbar-pair:<base64url(json)>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pairing {
    /// The server's address, `https://...` (or `http://` for a server on the same machine).
    pub url: String,
    /// The one-time pairing code.
    pub code: String,
    /// SHA-256 (lowercase hex) of the server's certificate, while the server uses its own certificate
    /// instead of a publicly trusted one. A client trusts exactly that certificate for this server.
    /// Absent when the server has a real certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_sha256: Option<String>,
}

impl Pairing {
    pub fn encode(&self) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let json = serde_json::to_vec(self).expect("plain struct serializes");
        format!("{PAIR_PREFIX}{}", URL_SAFE_NO_PAD.encode(json))
    }

    /// `None` if `s` is not a pairing key, or names a certificate in a form that is not a SHA-256.
    pub fn decode(s: &str) -> Option<Pairing> {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let body = s.trim().strip_prefix(PAIR_PREFIX)?;
        let mut p: Pairing = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(body).ok()?).ok()?;
        if let Some(fp) = &mut p.cert_sha256 {
            *fp = fp.to_ascii_lowercase();
            if fp.len() != 64 || !fp.chars().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
        }
        (!p.url.is_empty() && !p.code.is_empty()).then_some(p)
    }
}

/// A pairing key with no certificate pin.
pub fn encode_pairing(url: &str, code: &str) -> String {
    Pairing { url: url.into(), code: code.into(), cert_sha256: None }.encode()
}

/// Returns `(url, code)`, or `None` if the string is not a pairing key.
pub fn decode_pairing(s: &str) -> Option<(String, String)> {
    Pairing::decode(s).map(|p| (p.url, p.code))
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
    fn a_pairing_string_can_carry_the_certificate_pin() {
        let fp = "AB".repeat(32);
        let s = Pairing { url: "https://192.168.1.20".into(), code: "c".into(), cert_sha256: Some(fp.clone()) }.encode();
        assert_eq!(Pairing::decode(&s).unwrap().cert_sha256, Some(fp.to_lowercase()), "kept, in lower case");
        // Without a pin the field is left out, so a string looks as it did before pins existed.
        let plain = encode_pairing("https://askbar.example.com", "c");
        assert!(!String::from_utf8(
            base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &plain[PAIR_PREFIX.len()..]).unwrap()
        )
        .unwrap()
        .contains("cert_sha256"));
        assert_eq!(Pairing::decode(&plain).unwrap().cert_sha256, None);
        let bad = Pairing { url: "https://x".into(), code: "c".into(), cert_sha256: Some("not-a-hash".into()) }.encode();
        assert_eq!(Pairing::decode(&bad), None, "a malformed pin is refused, not ignored");
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
            ChatEvent::Provider { id: "p1".into(), name: "OpenAI".into(), model: None },
            ChatEvent::Provider { id: "p1".into(), name: "OpenAI".into(), model: Some("gpt-x".into()) },
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
    fn a_reply_names_its_provider_and_older_messages_still_read() {
        let reply = Message {
            id: "m".into(),
            role: "assistant".into(),
            content: "hi".into(),
            created_at: 1,
            provider: Some(ReplyProvider { id: "p1".into(), name: "OpenAI".into(), model: "gpt-x".into() }),
            failed: None,
        };
        let s = serde_json::to_string(&reply).unwrap();
        assert!(s.contains(r#""provider":{"id":"p1","name":"OpenAI","model":"gpt-x"}"#), "{s}");
        assert_eq!(serde_json::from_str::<Message>(&s).unwrap(), reply);
        // A question, or a reply from before replies were labelled: no `provider` key, and one without it reads.
        let question = Message { provider: None, role: "user".into(), ..reply };
        assert!(!serde_json::to_string(&question).unwrap().contains("provider"));
        let old: Message = serde_json::from_str(r#"{"id":"m","role":"assistant","content":"hi","created_at":1}"#).unwrap();
        assert_eq!(old.provider, None);
        // A provider event from an older server, without `model`, still reads.
        let ev: ChatEvent = serde_json::from_str(r#"{"event":"provider","id":"p1","name":"OpenAI"}"#).unwrap();
        assert_eq!(ev, ChatEvent::Provider { id: "p1".into(), name: "OpenAI".into(), model: None });
    }

    #[test]
    fn a_failed_question_says_why_and_older_messages_still_read() {
        let q = Message {
            id: "m".into(),
            role: "user".into(),
            content: "q".into(),
            created_at: 1,
            provider: None,
            failed: Some(MessageFailure { code: "timed_out".into(), message: "Timed out after 5 minutes".into() }),
        };
        let s = serde_json::to_string(&q).unwrap();
        assert!(s.contains(r#""failed":{"code":"timed_out","message":"Timed out after 5 minutes"}"#), "{s}");
        assert_eq!(serde_json::from_str::<Message>(&s).unwrap(), q);
        // One that did not fail has no `failed` key, and a message without it reads.
        assert!(!serde_json::to_string(&Message { failed: None, ..q }).unwrap().contains("failed"));
        let old: Message = serde_json::from_str(r#"{"id":"m","role":"user","content":"q","created_at":1}"#).unwrap();
        assert_eq!(old.failed, None);
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
        assert_eq!(p.billing, Billing::Metered);
        assert_eq!((p.price_in, p.price_out, p.price_cached_in), (None, None, None));
        let p: SnapshotProvider = serde_json::from_str(
            r#"{"id":"p","name":"n","type":"openai","model":"m","billing":"subscription","allow_as_fallback":true,"price_in":2.5,"price_out":10,"price_cached_in":0.25}"#,
        )
        .unwrap();
        // `allow_as_fallback` from an earlier version is read and dropped.
        assert_eq!((p.billing, p.price_in, p.price_out, p.price_cached_in), (Billing::Subscription, Some(2.5), Some(10.0), Some(0.25)));
        assert!(!serde_json::to_string(&p).unwrap().contains("fallback"));
        assert!(
            serde_json::from_str::<SnapshotProvider>(r#"{"id":"p","name":"n","type":"openai","model":"m","billing":"free"}"#).is_err(),
            "only metered or subscription"
        );
        // And it serializes back to the same words.
        assert!(serde_json::to_string(&p).unwrap().contains(r#""billing":"subscription""#));
    }

    #[test]
    fn a_snapshot_stored_with_a_fallback_order_still_parses_and_drops_it() {
        let s: Snapshot =
            serde_json::from_str(r#"{"version":3,"audit_retention_days":30,"default_provider_id":"p1","fallback_order":["p2"]}"#).unwrap();
        assert_eq!((s.version, s.default_provider_id.as_deref()), (3, Some("p1")));
        assert!(!serde_json::to_string(&s).unwrap().contains("fallback"));
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
    fn a_snapshot_without_a_history_limit_sends_the_whole_conversation() {
        let s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30}"#).unwrap();
        assert_eq!(s.history_turns, 0);
        let s: Snapshot = serde_json::from_str(r#"{"audit_retention_days": 30, "history_turns": 5}"#).unwrap();
        assert_eq!(s.history_turns, 5);
    }

    #[test]
    fn display_settings_without_the_order_are_oldest_first() {
        let d: DisplaySettings = serde_json::from_str(
            r#"{"theme":"dark","width":900,"max_height":700,"history_width":300,"top_percent":0.3,"hide_on_blur":false}"#,
        )
        .unwrap();
        assert!(!d.newest_first);
        assert_eq!(d.width, 900, "the other fields still read");
        let d: DisplaySettings = serde_json::from_str(r#"{"newest_first":true}"#).unwrap();
        assert!(d.newest_first);
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
