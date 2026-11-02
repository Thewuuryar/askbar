//! Types the Tauri commands and the server client (`remote`) share.

use crate::client_config::Ui;
use serde::Serialize;

/// What the window and the shell need to know about the active setup.
#[derive(Debug)]
pub struct Settings {
    pub ui: Ui,
    pub hotkey: String,
    pub provider: String,
    pub model: String,
}

impl Settings {
    /// Used when the real settings could not be read, so the window can still open.
    pub fn builtin() -> Settings {
        Settings { ui: Ui::default(), hotkey: crate::client_config::ClientConfig::default().hotkey, provider: String::new(), model: String::new() }
    }
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    /// `generation_id` names the server-side reply (to resume or cancel it); `queued` means
    /// another reply in this conversation is still running and this one waits its turn.
    Started { conversation_id: String, generation_id: Option<String>, queued: bool },
    Delta { text: String },
    Done,
    Cancelled,
    Error { message: String },
}

/// A reply that has not finished: running, or a message waiting its turn behind one.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PendingReply {
    pub generation_id: String,
    /// "running" or "queued".
    pub status: String,
    /// The reply so far, as last saved by the server.
    pub text: String,
    /// The question; for a queued message it is not in the conversation yet.
    pub prompt: String,
}

impl From<askbar_proto::PendingGeneration> for PendingReply {
    fn from(g: askbar_proto::PendingGeneration) -> PendingReply {
        PendingReply { generation_id: g.generation_id, status: g.status, text: g.text, prompt: g.prompt }
    }
}

/// A message this device sent and the server has not taken yet.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UnsentMessage {
    pub client_request_id: String,
    pub conversation_id: Option<String>,
    pub text: String,
    pub created_at: i64,
}

impl From<crate::outbox::OutboxEntry> for UnsentMessage {
    fn from(e: crate::outbox::OutboxEntry) -> UnsentMessage {
        UnsentMessage { client_request_id: e.client_request_id, conversation_id: e.conversation_id, text: e.text, created_at: e.created_at }
    }
}

/// Something wrong between this client and its backend, with a way forward in the UI.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientStatus {
    /// "offline", "rejected", "not_paired", "update_needed", "provider_missing" or "problem".
    pub kind: String,
    pub message: String,
    /// For "provider_missing": the providers the user can pick instead.
    pub providers: Vec<ProviderChoice>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ProviderChoice {
    pub id: String,
    pub name: String,
}

impl ClientStatus {
    pub fn new(kind: &str, message: impl Into<String>) -> ClientStatus {
        ClientStatus { kind: kind.into(), message: message.into(), providers: Vec::new() }
    }
}

pub type Emit<'a> = &'a (dyn Fn(StreamEvent) + Send + Sync);
