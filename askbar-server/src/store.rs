//! Helpers for reading server-side configuration out of the database.

use crate::secrets::Secrets;
use crate::llm::{Provider, ProviderKind, DEFAULT_SYSTEM_PROMPT};
use crate::Db;
use rusqlite::OptionalExtension;

/// Load a provider by id with its API key decrypted. The key never leaves this process.
pub fn load_provider(db: &Db, secrets: &Secrets, id: &str) -> rusqlite::Result<Option<Provider>> {
    let row = db
        .conn()
        .query_row(
            "SELECT name, type, base_url, model, api_key_enc, max_tokens, temperature, headers
             FROM providers WHERE id = ?1",
            [id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<Vec<u8>>>(4)?,
                    r.get::<_, Option<u32>>(5)?,
                    r.get::<_, Option<f32>>(6)?,
                    r.get::<_, String>(7)?,
                ))
            },
        )
        .optional()?;
    Ok(row.map(|(name, kind, base_url, model, enc, max_tokens, temperature, headers)| Provider {
        name,
        kind: if kind == "anthropic" { ProviderKind::Anthropic } else { ProviderKind::OpenAi },
        base_url,
        api_key: enc.and_then(|e| secrets.decrypt(&e).ok()).unwrap_or_default(),
        api_key_env: String::new(),
        model,
        max_tokens,
        temperature,
        headers: serde_json::from_str(&headers).unwrap_or_default(),
    }))
}

pub fn system_prompt(db: &Db) -> rusqlite::Result<String> {
    let stored: Option<String> =
        db.conn().query_row("SELECT value FROM settings WHERE key = 'system_prompt'", [], |r| r.get(0)).optional()?;
    Ok(stored.unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()))
}
