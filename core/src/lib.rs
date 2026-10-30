//! Askbar core: everything except the window.
//!
//! Kept free of GUI dependencies so it builds and tests anywhere.

pub mod config;
pub mod db;
pub mod provider;

pub use config::{config_path, db_path, Config, ConfigError, Provider, ProviderKind, DEFAULT_SYSTEM_PROMPT};
pub use db::{make_title, ConversationSummary, Db, DbError, StoredMessage};
pub use provider::{http_client, stream_chat, ChatError, ChatMessage, StreamOutcome};
pub use reqwest::Client as HttpClient;
