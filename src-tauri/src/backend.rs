//! The seam between the Tauri commands and wherever conversations, providers
//! and settings live. `Local` wraps `askbar-core` on this machine.

use askbar_core::{
    config_path, http_client, make_title, stream_chat, ChatMessage, Config, ConversationSummary, Db, DbError,
    HttpClient, ProviderKind, StoredMessage, StreamOutcome,
};
use crate::client_config::Ui;
use askbar_proto::ProviderInfo;
use async_trait::async_trait;
use serde::Serialize;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use tokio::sync::oneshot;

/// What the window and the shell need to know about the active setup.
#[derive(Debug)]
pub struct Settings {
    pub ui: Ui,
    pub hotkey: String,
    pub provider: String,
    pub model: String,
}

impl Settings {
    pub fn builtin() -> Settings {
        Settings::from_config(&Config::builtin())
    }

    fn from_config(cfg: &Config) -> Settings {
        let (provider, model) = cfg
            .active()
            .map(|p| (p.name.clone(), p.model.clone()))
            .unwrap_or_default();
        Settings { ui: Ui::default(), hotkey: cfg.hotkey.clone(), provider, model }
    }
}

#[derive(Serialize, Clone)]
#[serde(tag = "event", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StreamEvent {
    Started { conversation_id: i64 },
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
    pub providers: Vec<String>,
}

impl ClientStatus {
    pub fn new(kind: &str, message: impl Into<String>) -> ClientStatus {
        ClientStatus { kind: kind.into(), message: message.into(), providers: Vec::new() }
    }
}

pub type Emit<'a> = &'a (dyn Fn(StreamEvent) + Send + Sync);

#[async_trait]
pub trait Backend: Send + Sync {
    /// Current settings, or a message explaining why they could not be read.
    async fn settings(&self) -> Result<Settings, String>;
    #[allow(dead_code)]
    async fn list_providers(&self) -> Result<Vec<ProviderInfo>, String>;
    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>, String>;
    async fn load_conversation(&self, id: i64) -> Result<Vec<StoredMessage>, String>;
    async fn delete_conversation(&self, id: i64) -> Result<(), String>;
    /// `None` when everything works. Checked when the window opens and after a failed send.
    async fn status(&self) -> Option<ClientStatus> {
        None
    }
    async fn set_active_provider(&self, _name: String) -> Result<(), String> {
        Err("This setup has no server to pick a provider from; edit `active_provider` in config.yaml.".into())
    }
    /// Send a message and stream the reply through `emit`. Failures are
    /// reported as `StreamEvent::Error`, not as a return value.
    async fn ask(&self, conversation_id: Option<i64>, text: String, cancel: oneshot::Receiver<()>, emit: Emit<'_>);
}

pub struct Local {
    db: Mutex<Db>,
    http: HttpClient,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn load_config() -> Result<Config, String> {
    Config::load_or_create(&config_path()).map(|(c, _)| c).map_err(|e| e.to_string())
}

impl Local {
    pub fn open(db_path: &Path) -> Result<Local, DbError> {
        Ok(Local { db: Mutex::new(Db::open(db_path)?), http: http_client() })
    }
}

#[async_trait]
impl Backend for Local {
    async fn settings(&self) -> Result<Settings, String> {
        load_config().map(|c| Settings::from_config(&c))
    }

    async fn list_providers(&self) -> Result<Vec<ProviderInfo>, String> {
        let cfg = load_config()?;
        Ok(cfg
            .providers
            .iter()
            .map(|p| ProviderInfo { name: p.name.clone(), kind: match p.kind {
                    ProviderKind::OpenAi => "openai",
                    ProviderKind::Anthropic => "anthropic",
                }
                .to_string(), model: p.model.clone() })
            .collect())
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>, String> {
        lock(&self.db).list_conversations().map_err(|e| e.to_string())
    }

    async fn load_conversation(&self, id: i64) -> Result<Vec<StoredMessage>, String> {
        lock(&self.db).get_messages(id).map_err(|e| e.to_string())
    }

    async fn delete_conversation(&self, id: i64) -> Result<(), String> {
        lock(&self.db).delete_conversation(id).map(|_| ()).map_err(|e| e.to_string())
    }

    async fn ask(&self, conversation_id: Option<i64>, text: String, cancel: oneshot::Receiver<()>, emit: Emit<'_>) {
        let fail = |message: String| emit(StreamEvent::Error { message });

        let cfg = match load_config() {
            Ok(c) => c,
            Err(e) => return fail(e),
        };
        let provider = match cfg.active() {
            Ok(p) => p.clone(),
            Err(e) => return fail(e.to_string()),
        };

        // Save the question first so it survives a failed request.
        let saved = {
            let db = lock(&self.db);
            (|| -> Result<(i64, Vec<StoredMessage>), DbError> {
                let id = match conversation_id {
                    Some(id) if db.conversation_exists(id)? => id,
                    _ => db.create_conversation(&make_title(&text))?,
                };
                db.add_message(id, "user", &text)?;
                Ok((id, db.get_messages(id)?))
            })()
        };
        let (conv_id, stored) = match saved {
            Ok(v) => v,
            Err(e) => return fail(e.to_string()),
        };
        emit(StreamEvent::Started { conversation_id: conv_id });

        let mut context: Vec<ChatMessage> = stored.iter().map(|m| ChatMessage::new(&m.role, &m.content)).collect();
        if let Some(n) = cfg.history.context_messages.filter(|&n| n > 0) {
            let start = context.len().saturating_sub(n);
            context.drain(..start);
        }

        let mut reply = String::new();
        let result = stream_chat(&self.http, &provider, &cfg.system_prompt, &context, cancel, |delta| {
            reply.push_str(delta);
            emit(StreamEvent::Delta { text: delta.to_string() });
        })
        .await;

        // Keep whatever arrived, even on cancel or error.
        if !reply.trim().is_empty() {
            if let Err(e) = lock(&self.db).add_message(conv_id, "assistant", &reply) {
                return fail(e.to_string());
            }
        }

        emit(match result {
            Ok(StreamOutcome::Completed) => StreamEvent::Done,
            Ok(StreamOutcome::Cancelled) => StreamEvent::Cancelled,
            Err(e) => StreamEvent::Error { message: e.to_string() },
        });
    }
}
