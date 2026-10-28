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

#[cfg(test)]
mod tests {
    use super::*;

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
