//! Helpers for reading server-side configuration: the snapshot, and the provider keys stored beside it.

use crate::secrets::Secrets;
use crate::llm::{Provider, ProviderKind, DEFAULT_SYSTEM_PROMPT};
use crate::Db;
use rusqlite::OptionalExtension;

/// Load a provider by id: its settings from the current snapshot, its API key (decrypted)
/// from the database. The key never leaves this process.
pub fn load_provider(db: &Db, secrets: &Secrets, id: &str) -> rusqlite::Result<Option<Provider>> {
    let Some(p) = crate::snapshot::current(db)?.and_then(|s| s.providers.into_iter().find(|p| p.id == id)) else {
        return Ok(None);
    };
    let enc: Option<Vec<u8>> =
        db.conn().query_row("SELECT api_key_enc FROM providers WHERE id = ?1", [id], |r| r.get(0)).optional()?.flatten();
    Ok(Some(Provider {
        name: p.name,
        kind: if p.kind == "anthropic" { ProviderKind::Anthropic } else { ProviderKind::OpenAi },
        base_url: p.base_url,
        api_key: enc.and_then(|e| secrets.decrypt(&e).ok()).unwrap_or_default(),
        api_key_env: String::new(),
        model: p.model,
        max_tokens: p.max_tokens,
        temperature: p.temperature,
        headers: p.headers,
    }))
}

/// The system prompt chats use: the snapshot's, else the built-in one.
pub fn chat_system_prompt(db: &Db) -> rusqlite::Result<String> {
    Ok(crate::snapshot::current(db)?
        .and_then(|s| s.system_prompt)
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()))
}
