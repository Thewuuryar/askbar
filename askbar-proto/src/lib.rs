//! Wire types shared by the Askbar server and client.
//! IDs are server-generated ULIDs; timestamps are UTC unix seconds.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub api_version: u32,
    pub min_client: String,
}

/// A configured provider as seen by clients. Never carries key material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationDetail {
    #[serde(flatten)]
    pub conversation: Conversation,
    pub messages: Vec<Message>,
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

/// `GET /api/v1/settings`. The system prompt is deliberately not part of it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Settings {
    pub display: DisplaySettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    pub provider: String,
    pub text: String,
}

/// Server-Sent Event payloads for `POST /api/v1/chat`; the SSE event name is the `event` tag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatEvent {
    Meta { conversation_id: String },
    Delta { text: String },
    Done,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairRequest {
    pub code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairResponse {
    pub token: String,
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
        let p = ProviderInfo { name: "a".into(), kind: "openai".into(), model: "m".into() };
        assert_eq!(serde_json::to_string(&p).unwrap(), r#"{"name":"a","type":"openai","model":"m"}"#);
    }

    #[test]
    fn chat_event_round_trip() {
        for e in [
            ChatEvent::Meta { conversation_id: "x".into() },
            ChatEvent::Delta { text: "hi".into() },
            ChatEvent::Done,
            ChatEvent::Error { message: "bad".into() },
        ] {
            let s = serde_json::to_string(&e).unwrap();
            assert_eq!(serde_json::from_str::<ChatEvent>(&s).unwrap(), e);
        }
        assert_eq!(serde_json::to_string(&ChatEvent::Done).unwrap(), r#"{"event":"done"}"#);
    }

    #[test]
    fn chat_request_conversation_optional() {
        let r: ChatRequest = serde_json::from_str(r#"{"provider":"p","text":"t"}"#).unwrap();
        assert_eq!(r.conversation_id, None);
    }
}
