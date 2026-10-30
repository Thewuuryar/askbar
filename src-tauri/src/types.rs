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

#[derive(Serialize, Clone)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    Started { conversation_id: String },
    Delta { text: String },
    Done,
    Cancelled,
    Error { message: String },
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
