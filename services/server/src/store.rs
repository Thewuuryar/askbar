//! Helpers for reading server-side configuration from the snapshot. The server holds no provider keys (ADR 0054).

use crate::llm::DEFAULT_SYSTEM_PROMPT;
use crate::Db;

/// The system prompt chats use: the snapshot's, else the built-in one.
pub fn chat_system_prompt(db: &Db) -> rusqlite::Result<String> {
    Ok(crate::snapshot::current(db)?.and_then(|s| s.system_prompt).unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()))
}

/// A provider by id, as the current snapshot describes it. It holds no key: the gateway does (ADR 0054).
pub fn provider_meta(db: &Db, id: &str) -> rusqlite::Result<Option<askbar_proto::SnapshotProvider>> {
    Ok(crate::snapshot::current(db)?.and_then(|s| s.providers.into_iter().find(|p| p.id == id)))
}
