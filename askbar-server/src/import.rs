//! `askbar-server import`: copy a desktop install's history and providers in.

use crate::secrets::Secrets;
use askbar_core::{Config, Db, ProviderKind};
use rusqlite::{params, Connection, OpenFlags};
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub conversations: usize,
    pub messages: usize,
    pub providers: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("{0}")]
    Config(#[from] askbar_core::ConfigError),
    #[error("{0}")]
    Db(#[from] askbar_core::DbError),
    #[error("could not read history database {path}: {source}")]
    Source { path: String, source: rusqlite::Error },
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not read {path}: {source}")]
    Io { path: String, source: std::io::Error },
}

pub fn import(db: &Db, secrets: &Secrets, config_path: &Path, history_path: &Path) -> Result<Report, ImportError> {
    let text = std::fs::read_to_string(config_path)
        .map_err(|e| ImportError::Io { path: config_path.display().to_string(), source: e })?;
    let cfg = Config::from_yaml(&text, &config_path.display().to_string())?;
    let src_err = |e| ImportError::Source { path: history_path.display().to_string(), source: e };
    let src = Connection::open_with_flags(history_path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(src_err)?;

    let user = db.seed_server()?;
    let conn = db.conn();
    let mut report = Report::default();
    let tx = conn.unchecked_transaction()?;
    let mut ids = ulid::Generator::new();
    let mut next_id = |_: ()| ids.generate().map(|u| u.to_string()).map_err(|_| rusqlite::Error::InvalidQuery);

    // Conversations and messages, oldest first so ULID order matches.
    let mut convs = src
        .prepare("SELECT id, title, created_at, updated_at FROM conversations ORDER BY id")
        .map_err(src_err)?;
    let mut msgs = src
        .prepare("SELECT role, content, created_at FROM messages WHERE conversation_id = ?1 ORDER BY id")
        .map_err(src_err)?;
    let rows: Vec<(i64, String, i64, i64)> = convs
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .map_err(src_err)?
        .collect::<Result<_, _>>()
        .map_err(src_err)?;
    for (old_id, title, created, updated) in rows {
        tx.execute(
            "INSERT INTO conversations (public_id, user_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![next_id(())?, user, title, created, updated],
        )?;
        let conv = tx.last_insert_rowid();
        let list: Vec<(String, String, i64)> = msgs
            .query_map([old_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(src_err)?
            .collect::<Result<_, _>>()
            .map_err(src_err)?;
        for (role, content, at) in list {
            tx.execute(
                "INSERT INTO messages (public_id, user_id, conversation_id, role, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![next_id(())?, user, conv, role, content, at],
            )?;
            report.messages += 1;
        }
        report.conversations += 1;
    }

    for p in &cfg.providers {
        let exists = tx
            .query_row("SELECT 1 FROM providers WHERE name = ?1", [&p.name], |_| Ok(()))
            .is_ok();
        if exists {
            report.warnings.push(format!("provider \"{}\" already exists on the server; skipped", p.name));
            continue;
        }
        let key = p.resolve_key();
        if key.is_none() && !p.api_key_env.trim().is_empty() {
            report.warnings.push(format!(
                "provider \"{}\": environment variable {} is not set, so no API key was imported",
                p.name,
                p.api_key_env.trim()
            ));
        }
        let enc = key.map(|k| secrets.encrypt(&k));
        let kind = match p.kind {
            ProviderKind::OpenAi => "openai",
            ProviderKind::Anthropic => "anthropic",
        };
        tx.execute(
            "INSERT INTO providers (id, name, type, base_url, model, api_key_enc, max_tokens, temperature, headers, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, strftime('%s','now'))",
            params![
                next_id(())?,
                p.name,
                kind,
                p.base_url,
                p.model,
                enc,
                p.max_tokens,
                p.temperature,
                serde_json::to_string(&p.headers).unwrap_or_else(|_| "{}".into()),
            ],
        )?;
        report.providers += 1;
    }
    tx.commit()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn fixture(dir: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        std::fs::create_dir_all(dir).unwrap();
        let hist = dir.join("history.db");
        {
            let d = Db::open(&hist).unwrap();
            let a = d.create_conversation("first").unwrap();
            d.add_message(a, "user", "q1").unwrap();
            d.add_message(a, "assistant", "a1").unwrap();
            d.add_message(a, "user", "q2").unwrap();
            let b = d.create_conversation("second").unwrap();
            d.add_message(b, "user", "héllo").unwrap();
        }
        let cfg = dir.join("config.yaml");
        std::fs::write(
            &cfg,
            "providers:
  - {name: inline, type: openai, base_url: 'http://x/v1', api_key: sk-inline, model: m1}
  - {name: fromenv, type: anthropic, api_key_env: ASKBAR_IMPORT_TEST_SET, model: m2}
  - {name: unset, type: openai, api_key_env: ASKBAR_IMPORT_TEST_UNSET, model: m3}
  - {name: local, type: openai, base_url: 'http://localhost:11434/v1', model: llama}
",
        )
        .unwrap();
        (cfg, hist)
    }

    #[test]
    fn imports_history_and_providers() {
        std::env::set_var("ASKBAR_IMPORT_TEST_SET", "sk-from-env");
        std::env::remove_var("ASKBAR_IMPORT_TEST_UNSET");
        let dir = std::env::temp_dir().join(format!("askbar-import-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (cfg, hist) = fixture(&dir);

        let server = Db::open(&dir.join("server").join("askbar.db")).unwrap();
        let secrets = Secrets::new(KEY).unwrap();
        let report = import(&server, &secrets, &cfg, &hist).unwrap();
        assert_eq!((report.conversations, report.messages, report.providers), (2, 4, 4));
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("ASKBAR_IMPORT_TEST_UNSET"));

        let c = server.conn();
        let count = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM conversations"), 2);
        assert_eq!(count("SELECT COUNT(*) FROM messages"), 4);
        assert_eq!(count("SELECT COUNT(*) FROM conversations WHERE public_id IS NULL OR user_id IS NULL"), 0);

        // Message order within and across conversations follows the ULIDs.
        let mut stmt = c
            .prepare("SELECT c.title, m.role, m.content FROM messages m JOIN conversations c ON c.id = m.conversation_id ORDER BY m.public_id")
            .unwrap();
        let got: Vec<(String, String, String)> =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
        let want: Vec<(String, String, String)> = [
            ("first", "user", "q1"),
            ("first", "assistant", "a1"),
            ("first", "user", "q2"),
            ("second", "user", "héllo"),
        ]
        .iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect();
        assert_eq!(got, want);

        // Keys: encrypted when present, empty when not, never plaintext.
        let key_of = |name: &str| -> Option<Vec<u8>> {
            c.query_row("SELECT api_key_enc FROM providers WHERE name = ?1", [name], |r| r.get(0)).unwrap()
        };
        assert_eq!(secrets.decrypt(&key_of("inline").unwrap()).unwrap(), "sk-inline");
        assert_eq!(secrets.decrypt(&key_of("fromenv").unwrap()).unwrap(), "sk-from-env");
        assert!(key_of("unset").is_none());
        assert!(key_of("local").is_none());
        let raw: Vec<u8> = c.query_row("SELECT api_key_enc FROM providers WHERE name='inline'", [], |r| r.get(0)).unwrap();
        assert!(!raw.windows(9).any(|w| w == b"sk-inline"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
