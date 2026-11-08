//! SQLite storage: users, clients, conversations and messages.
//!
//! Schema changes go in `MIGRATIONS` (append only); `PRAGMA user_version`
//! records how many have been applied. A database that this build did not
//! create, or that is newer than it, is refused rather than touched.

use crate::ids::new_id;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Stamped into `PRAGMA application_id` by the first migration ("ASKB"), so a
/// database from an older Askbar version is never mistaken for this schema.
const APPLICATION_ID: i64 = 0x4153_4B42;

const MIGRATIONS: &[&str] = &[
    // v1: the final chat schema.
    //
    // Live: users, devices, conversations, messages, messages_fts, user_prefs. names-ok
    // Reserved, empty: memory (append-only), projects, token_usage.
    // Interim until migration 3: providers (full rows), settings, pair_codes, display_defaults.
    "CREATE TABLE users (
         id         TEXT PRIMARY KEY,
         name       TEXT    NOT NULL,
         created_at INTEGER NOT NULL
     );
     CREATE TABLE devices ( -- names-ok
         id         TEXT PRIMARY KEY,
         user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         name       TEXT    NOT NULL,
         hash       TEXT    NOT NULL UNIQUE,
         created_at INTEGER NOT NULL,
         last_seen  INTEGER,
         revoked_at INTEGER
     );
     CREATE TABLE projects (
         id         TEXT PRIMARY KEY,
         user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         name       TEXT    NOT NULL,
         created_at INTEGER NOT NULL
     );
     CREATE TABLE conversations (
         id         TEXT PRIMARY KEY,
         user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         project_id TEXT    REFERENCES projects(id) ON DELETE SET NULL,
         title      TEXT    NOT NULL,
         created_at INTEGER NOT NULL,
         updated_at INTEGER NOT NULL
     );
     CREATE INDEX idx_conversations_user ON conversations(user_id, updated_at DESC, id DESC);
     CREATE TABLE messages (
         id              TEXT PRIMARY KEY,
         conversation_id TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
         user_id         TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         role            TEXT    NOT NULL CHECK (role IN ('user','assistant')),
         content         TEXT    NOT NULL,
         created_at      INTEGER NOT NULL
     );
     CREATE INDEX idx_messages_conversation ON messages(conversation_id, id);
     CREATE VIRTUAL TABLE messages_fts USING fts5(content, content='messages', content_rowid='rowid');
     CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
         INSERT INTO messages_fts (rowid, content) VALUES (new.rowid, new.content);
     END;
     CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
         INSERT INTO messages_fts (messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
     END;
     CREATE TRIGGER messages_fts_update AFTER UPDATE OF content ON messages BEGIN
         INSERT INTO messages_fts (messages_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
         INSERT INTO messages_fts (rowid, content) VALUES (new.rowid, new.content);
     END;
     CREATE TABLE user_prefs (
         user_id            TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
         active_provider_id TEXT
     );
     CREATE TABLE memory (
         id         TEXT PRIMARY KEY,
         user_id    TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         content    TEXT    NOT NULL,
         created_at INTEGER NOT NULL
     );
     CREATE TRIGGER memory_append_only_update BEFORE UPDATE ON memory BEGIN
         SELECT RAISE(ABORT, 'memory is append-only');
     END;
     CREATE TRIGGER memory_append_only_delete BEFORE DELETE ON memory BEGIN
         SELECT RAISE(ABORT, 'memory is append-only');
     END;
     CREATE TABLE token_usage (
         id          TEXT PRIMARY KEY,
         user_id     TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         device_id   TEXT, -- names-ok
         provider_id TEXT,
         tokens_in   INTEGER NOT NULL DEFAULT 0,
         tokens_out  INTEGER NOT NULL DEFAULT 0,
         created_at  INTEGER NOT NULL
     );
     CREATE TABLE providers (
         id          TEXT PRIMARY KEY,
         name        TEXT    NOT NULL UNIQUE,
         type        TEXT    NOT NULL CHECK (type IN ('openai','anthropic')),
         base_url    TEXT    NOT NULL DEFAULT '',
         model       TEXT    NOT NULL,
         api_key_enc BLOB,
         max_tokens  INTEGER,
         temperature REAL,
         headers     TEXT    NOT NULL DEFAULT '{}',
         created_at  INTEGER NOT NULL
     );
     CREATE TABLE settings (
         key   TEXT PRIMARY KEY,
         value TEXT NOT NULL
     );
     CREATE TABLE pair_codes (
         hash        TEXT PRIMARY KEY,
         user_id     TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         device_name TEXT    NOT NULL DEFAULT '', -- names-ok
         expires_at  INTEGER NOT NULL,
         used        INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE display_defaults (
         user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
         value   TEXT NOT NULL
     );",
    // v2 (additive): the admin service's snapshot and the pair codes already redeemed.
    "CREATE TABLE config_snapshot (
         id         INTEGER PRIMARY KEY CHECK (id = 1),
         version    INTEGER NOT NULL,
         body       TEXT    NOT NULL,
         updated_at INTEGER NOT NULL
     );
     CREATE TABLE redeemed_codes (
         hash        TEXT PRIMARY KEY,
         device_id   TEXT, -- names-ok
         redeemed_at INTEGER NOT NULL
     );",
    // v3: the admin site has left the server. Its tables go, and a provider keeps only what the
    // server alone must hold: its id and its encrypted key. Everything else about providers,
    // and settings, display defaults and pairing codes, arrive in the snapshot.
    "DROP TABLE settings;
     DROP TABLE pair_codes;
     DROP TABLE display_defaults;
     CREATE TABLE providers_new (
         id          TEXT PRIMARY KEY,
         api_key_enc BLOB
     );
     INSERT INTO providers_new (id, api_key_enc) SELECT id, api_key_enc FROM providers;
     DROP TABLE providers;
     ALTER TABLE providers_new RENAME TO providers;",
    // v4: server-side generations. A chat turn is a row here that outlives its client.
    // Statuses: queued, running, done, error, cancelled, timed_out, interrupted. A queued row
    // holds the prompt; its user message is written to `messages` when it starts.
    "CREATE TABLE generations (
         id                   TEXT PRIMARY KEY,
         user_id              TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         device_id            TEXT, -- names-ok
         conversation_id      TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
         user_message_id      TEXT,
         assistant_message_id TEXT,
         client_request_id    TEXT,
         provider_id          TEXT,
         status               TEXT    NOT NULL CHECK (status IN
                                  ('queued','running','done','error','cancelled','timed_out','interrupted')),
         prompt               TEXT    NOT NULL DEFAULT '',
         text                 TEXT    NOT NULL DEFAULT '',
         error                TEXT,
         queued_at            INTEGER NOT NULL,
         started_at           INTEGER,
         deadline_at          INTEGER,
         ended_at             INTEGER
     );
     CREATE UNIQUE INDEX idx_generations_request ON generations(device_id, client_request_id) -- names-ok
         WHERE client_request_id IS NOT NULL;
     CREATE INDEX idx_generations_conversation ON generations(conversation_id, status);
     CREATE INDEX idx_generations_user_status ON generations(user_id, status);",
    // v5: what the router has learned about providers. A provider whose credentials were refused is
    // flagged `auth_failed` (with the error code, never the provider's message, which may echo
    // part of a key) until a try succeeds or a new key is saved.
    "CREATE TABLE provider_health (
         provider_id TEXT PRIMARY KEY,
         status      TEXT    NOT NULL CHECK (status IN ('auth_failed')),
         since       INTEGER NOT NULL,
         error_code  TEXT
     );",
    // v6: one word for the paired app: clients (they were called devices). Tokens and rows are kept. names-ok
    "ALTER TABLE devices RENAME TO clients; -- names-ok
     ALTER TABLE generations RENAME COLUMN device_id TO client_id; -- names-ok
     ALTER TABLE token_usage RENAME COLUMN device_id TO client_id; -- names-ok
     ALTER TABLE redeemed_codes RENAME COLUMN device_id TO client_id; -- names-ok",
    // v7: codes a paired client asked for, to pair another client of the same user (ADR 0008). The
    // server makes these itself (it never calls the admin); redeemed like the admin's, once.
    "CREATE TABLE client_pair_codes (
         hash        TEXT PRIMARY KEY,
         user_id     TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         client_name TEXT    NOT NULL DEFAULT '',
         created_by  TEXT    NOT NULL REFERENCES clients(id) ON DELETE CASCADE,
         expires_at  INTEGER NOT NULL,
         created_at  INTEGER NOT NULL
     );",
];

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("history database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not create {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error(
        "unexpected database version: {found}, but this server writes versions up to {expected}. \
         It was made by a different Askbar version; refusing to touch it."
    )]
    UnexpectedVersion { found: i64, expected: usize },
    #[error("not found")]
    NotFound,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StoredMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchHit {
    pub conversation_id: String,
    pub message_id: String,
    pub role: String,
    pub snippet: String,
    pub created_at: i64,
}

/// One chat turn run by the server. See migration v4 for the statuses.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Generation {
    pub id: String,
    pub user_id: String,
    pub client_id: Option<String>,
    pub conversation_id: String,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub client_request_id: Option<String>,
    pub provider_id: Option<String>,
    pub status: String,
    pub prompt: String,
    pub text: String,
    pub error: Option<String>,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub deadline_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// What `create_generation` needs. `status` is `running` (the row also gets `started_at`) or `queued`.
#[derive(Debug, Clone)]
pub struct NewGeneration<'a> {
    pub user_id: &'a str,
    pub client_id: Option<&'a str>,
    pub conversation_id: &'a str,
    pub user_message_id: Option<&'a str>,
    pub client_request_id: Option<&'a str>,
    pub provider_id: Option<&'a str>,
    pub status: &'a str,
    pub prompt: &'a str,
    pub deadline_at: Option<i64>,
}

const GENERATION_COLUMNS: &str = "id, user_id, client_id, conversation_id, user_message_id, assistant_message_id, \
     client_request_id, provider_id, status, prompt, text, error, queued_at, started_at, deadline_at, ended_at";

fn read_generation(r: &rusqlite::Row<'_>) -> rusqlite::Result<Generation> {
    Ok(Generation {
        id: r.get(0)?,
        user_id: r.get(1)?,
        client_id: r.get(2)?,
        conversation_id: r.get(3)?,
        user_message_id: r.get(4)?,
        assistant_message_id: r.get(5)?,
        client_request_id: r.get(6)?,
        provider_id: r.get(7)?,
        status: r.get(8)?,
        prompt: r.get(9)?,
        text: r.get(10)?,
        error: r.get(11)?,
        queued_at: r.get(12)?,
        started_at: r.get(13)?,
        deadline_at: r.get(14)?,
        ended_at: r.get(15)?,
    })
}

pub struct Db {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// First non-empty line of `text`, collapsed and cut to 60 characters.
pub fn make_title(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("New conversation");
    let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX: usize = 60;
    if collapsed.chars().count() <= MAX {
        collapsed
    } else {
        let cut: String = collapsed.chars().take(MAX - 1).collect();
        format!("{}…", cut.trim_end())
    }
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, DbError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DbError::Io { path: dir.display().to_string(), source: e })?;
        }
        Db::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Db, DbError> {
        Db::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Db, DbError> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), DbError> {
        let current: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current != 0 {
            let app: i64 = self.conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
            if current < 0 || current as usize > MIGRATIONS.len() || app != APPLICATION_ID {
                return Err(DbError::UnexpectedVersion { found: current, expected: MIGRATIONS.len() });
            }
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            if i == 0 {
                tx.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}"))?;
            }
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Raw connection, for the server's own queries.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Server start-up: make sure one user exists. Returns the user's id.
    pub fn seed_server(&self) -> Result<String, DbError> {
        let existing: Option<String> =
            self.conn.query_row("SELECT id FROM users ORDER BY created_at, id LIMIT 1", [], |r| r.get(0)).optional()?;
        match existing {
            Some(u) => Ok(u),
            None => {
                let id = new_id();
                self.conn.execute("INSERT INTO users (id, name, created_at) VALUES (?1, 'owner', ?2)", params![id, now()])?;
                Ok(id)
            }
        }
    }

    /// Returns the new conversation's id.
    pub fn create_conversation(&self, user_id: &str, title: &str) -> Result<String, DbError> {
        let (id, t) = (new_id(), now());
        self.conn.execute(
            "INSERT INTO conversations (id, user_id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, user_id, title, t],
        )?;
        Ok(id)
    }

    /// Adds a message to one of `user_id`'s conversations; `NotFound` for anyone else's.
    pub fn add_message(&self, user_id: &str, conversation_id: &str, role: &str, content: &str) -> Result<String, DbError> {
        let (id, t) = (new_id(), now());
        let tx = self.conn.unchecked_transaction()?;
        let added = tx.execute(
            "INSERT INTO messages (id, conversation_id, user_id, role, content, created_at)
             SELECT ?1, id, user_id, ?4, ?5, ?6 FROM conversations WHERE id = ?2 AND user_id = ?3",
            params![id, conversation_id, user_id, role, content, t],
        )?;
        if added == 0 {
            return Err(DbError::NotFound);
        }
        tx.execute("UPDATE conversations SET updated_at = ?1 WHERE id = ?2", params![t, conversation_id])?;
        tx.commit()?;
        Ok(id)
    }

    pub fn list_conversations(&self, user_id: &str) -> Result<Vec<ConversationSummary>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id)
             FROM conversations c WHERE c.user_id = ?1
             ORDER BY c.updated_at DESC, c.id DESC",
        )?;
        let rows = stmt.query_map([user_id], |r| {
            Ok(ConversationSummary { id: r.get(0)?, title: r.get(1)?, updated_at: r.get(2)?, message_count: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// One of `user_id`'s conversations, or `None` if it doesn't exist or is someone else's.
    pub fn get_conversation(&self, user_id: &str, id: &str) -> Result<Option<ConversationSummary>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT c.id, c.title, c.updated_at,
                        (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id)
                 FROM conversations c WHERE c.id = ?1 AND c.user_id = ?2",
                params![id, user_id],
                |r| Ok(ConversationSummary { id: r.get(0)?, title: r.get(1)?, updated_at: r.get(2)?, message_count: r.get(3)? }),
            )
            .optional()?)
    }

    /// Oldest first. Empty if the conversation doesn't exist or is someone else's.
    pub fn get_messages(&self, user_id: &str, conversation_id: &str) -> Result<Vec<StoredMessage>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, created_at FROM messages
             WHERE conversation_id = ?1 AND user_id = ?2 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id, user_id], |r| {
            Ok(StoredMessage { id: r.get(0)?, role: r.get(1)?, content: r.get(2)?, created_at: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Full-text search over `user_id`'s own messages, best match first. Every word
    /// must appear; punctuation in `query` is treated as plain text, never as search syntax.
    pub fn search_messages(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<SearchHit>, DbError> {
        let terms: Vec<String> = query.split_whitespace().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT m.conversation_id, m.id, m.role, snippet(messages_fts, 0, '', '', '…', 16), m.created_at
             FROM messages_fts JOIN messages m ON m.rowid = messages_fts.rowid
             WHERE messages_fts MATCH ?1 AND m.user_id = ?2
             ORDER BY messages_fts.rank, m.id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![terms.join(" "), user_id, limit as i64], |r| {
            Ok(SearchHit { conversation_id: r.get(0)?, message_id: r.get(1)?, role: r.get(2)?, snippet: r.get(3)?, created_at: r.get(4)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Returns true if a conversation was deleted. Its messages go with it.
    pub fn delete_conversation(&self, user_id: &str, id: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute("DELETE FROM conversations WHERE id = ?1 AND user_id = ?2", params![id, user_id])? > 0)
    }

    /// Deletes finished generations that ended before `older_than` (unix seconds). Running and
    /// queued rows are never touched. Returns how many went.
    pub fn prune_generations(&self, older_than: i64) -> Result<usize, DbError> {
        Ok(self.conn.execute("DELETE FROM generations WHERE status NOT IN ('queued','running') AND ended_at < ?1", [older_than])?)
    }

    /// Flags a provider as having had its credentials refused. Keeps the original time if it was
    /// already flagged.
    pub fn flag_provider_auth_failed(&self, provider_id: &str, error_code: &str) -> Result<(), DbError> {
        self.conn.execute(
            "INSERT INTO provider_health (provider_id, status, since, error_code) VALUES (?1, 'auth_failed', ?2, ?3)
             ON CONFLICT(provider_id) DO UPDATE SET error_code = excluded.error_code",
            params![provider_id, now(), error_code],
        )?;
        Ok(())
    }

    /// Clears a provider's flag (a try succeeded, or a new key was saved). True if there was one.
    pub fn clear_provider_health(&self, provider_id: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute("DELETE FROM provider_health WHERE provider_id = ?1", [provider_id])? > 0)
    }

    /// Every flagged provider: (provider id, status, since, error code).
    pub fn provider_health(&self) -> Result<Vec<(String, String, i64, Option<String>)>, DbError> {
        let mut stmt = self.conn.prepare("SELECT provider_id, status, since, error_code FROM provider_health ORDER BY provider_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Returns the new generation's id.
    pub fn create_generation(&self, g: &NewGeneration) -> Result<String, DbError> {
        let (id, t) = (new_id(), now());
        let started = (g.status == "running").then_some(t);
        self.conn.execute(
            "INSERT INTO generations (id, user_id, client_id, conversation_id, user_message_id, client_request_id,
                                      provider_id, status, prompt, queued_at, started_at, deadline_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                id,
                g.user_id,
                g.client_id,
                g.conversation_id,
                g.user_message_id,
                g.client_request_id,
                g.provider_id,
                g.status,
                g.prompt,
                t,
                started,
                g.deadline_at
            ],
        )?;
        Ok(id)
    }

    /// One of `user_id`'s generations; `None` for anyone else's.
    pub fn get_generation(&self, user_id: &str, id: &str) -> Result<Option<Generation>, DbError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {GENERATION_COLUMNS} FROM generations WHERE id = ?1 AND user_id = ?2"),
                params![id, user_id],
                read_generation,
            )
            .optional()?)
    }

    /// Moves a queued generation to `running` once its user message is written.
    pub fn start_generation(&self, id: &str, user_message_id: &str, deadline_at: Option<i64>) -> Result<(), DbError> {
        self.conn.execute(
            "UPDATE generations SET status = 'running', user_message_id = ?2, started_at = ?3, deadline_at = ?4
             WHERE id = ?1 AND status = 'queued'",
            params![id, user_message_id, now(), deadline_at],
        )?;
        Ok(())
    }

    /// Saves the reply so far while a generation runs. Ignored once it has finished.
    pub fn flush_generation_text(&self, id: &str, text: &str) -> Result<(), DbError> {
        self.conn.execute("UPDATE generations SET text = ?2 WHERE id = ?1 AND status = 'running'", params![id, text])?;
        Ok(())
    }

    /// Ends a generation with its final status. Returns false if it had already ended.
    pub fn finalize_generation(
        &self,
        id: &str,
        status: &str,
        text: &str,
        error: Option<&str>,
        assistant_message_id: Option<&str>,
    ) -> Result<bool, DbError> {
        Ok(self.conn.execute(
            "UPDATE generations SET status = ?2, text = ?3, error = ?4, assistant_message_id = ?5, ended_at = ?6
             WHERE id = ?1 AND status IN ('queued','running')",
            params![id, status, text, error, assistant_message_id, now()],
        )? > 0)
    }

    /// The caller's running and queued generations, oldest first.
    pub fn list_active_generations(&self, user_id: &str) -> Result<Vec<Generation>, DbError> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {GENERATION_COLUMNS} FROM generations
             WHERE user_id = ?1 AND status IN ('running','queued') ORDER BY id"
        ))?;
        let rows = stmt.query_map([user_id], read_generation)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn queued_count(&self, conversation_id: &str) -> Result<i64, DbError> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM generations WHERE conversation_id = ?1 AND status = 'queued'",
            [conversation_id],
            |r| r.get(0),
        )?)
    }

    /// Whether the conversation has a running or queued generation.
    pub fn conversation_busy(&self, conversation_id: &str) -> Result<bool, DbError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM generations WHERE conversation_id = ?1 AND status IN ('running','queued'))",
            [conversation_id],
            |r| r.get(0),
        )?)
    }

    /// The oldest queued generation in the conversation.
    pub fn next_queued(&self, conversation_id: &str) -> Result<Option<Generation>, DbError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {GENERATION_COLUMNS} FROM generations
                     WHERE conversation_id = ?1 AND status = 'queued' ORDER BY id LIMIT 1"
                ),
                [conversation_id],
                read_generation,
            )
            .optional()?)
    }

    /// A generation this client already started with `client_request_id`.
    pub fn find_by_request_id(&self, client_id: &str, client_request_id: &str) -> Result<Option<Generation>, DbError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {GENERATION_COLUMNS} FROM generations WHERE client_id = ?1 AND client_request_id = ?2"),
                params![client_id, client_request_id],
                read_generation,
            )
            .optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_user() -> (Db, String) {
        let db = Db::open_in_memory().unwrap();
        let user = db.seed_server().unwrap();
        (db, user)
    }

    #[test]
    fn create_list_and_load() {
        let (db, u) = db_with_user();
        let a = db.create_conversation(&u, "first").unwrap();
        let b = db.create_conversation(&u, "second").unwrap();
        db.add_message(&u, &a, "user", "hi").unwrap();
        db.add_message(&u, &a, "assistant", "hello").unwrap();
        db.add_message(&u, &b, "user", "yo").unwrap();

        let list = db.list_conversations(&u).unwrap();
        assert_eq!(list.len(), 2);
        let first = list.iter().find(|c| c.id == a).unwrap();
        assert_eq!(first.message_count, 2);
        assert_eq!(first.title, "first");

        let msgs = db.get_messages(&u, &a).unwrap();
        assert_eq!(
            msgs.iter().map(|m| (m.role.as_str(), m.content.as_str())).collect::<Vec<_>>(),
            vec![("user", "hi"), ("assistant", "hello")]
        );
    }

    #[test]
    fn delete_cascades_to_messages() {
        let (db, u) = db_with_user();
        let a = db.create_conversation(&u, "a").unwrap();
        let b = db.create_conversation(&u, "b").unwrap();
        db.add_message(&u, &a, "user", "x").unwrap();
        db.add_message(&u, &b, "user", "y").unwrap();

        assert!(db.delete_conversation(&u, &a).unwrap());
        assert!(!db.delete_conversation(&u, &a).unwrap(), "second delete is a no-op");
        assert!(db.get_messages(&u, &a).unwrap().is_empty());
        assert_eq!(db.get_messages(&u, &b).unwrap().len(), 1);
        assert_eq!(db.list_conversations(&u).unwrap().len(), 1);
        assert!(db.get_conversation(&u, &a).unwrap().is_none());
        assert!(db.get_conversation(&u, &b).unwrap().is_some());
    }

    #[test]
    fn user_b_cannot_read_or_change_user_as_conversations() {
        let (db, a) = db_with_user();
        db.conn.execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
        let conv = db.create_conversation(&a, "private").unwrap();
        db.add_message(&a, &conv, "user", "secret question").unwrap();

        assert!(db.list_conversations("B").unwrap().is_empty());
        assert!(db.get_conversation("B", &conv).unwrap().is_none());
        assert!(db.get_messages("B", &conv).unwrap().is_empty());
        assert!(matches!(db.add_message("B", &conv, "user", "intrusion"), Err(DbError::NotFound)));
        assert!(!db.delete_conversation("B", &conv).unwrap());

        // Untouched for its owner.
        assert_eq!(db.list_conversations(&a).unwrap().len(), 1);
        assert_eq!(db.get_messages(&a, &conv).unwrap().len(), 1);
    }

    #[test]
    fn search_finds_a_message_and_never_returns_another_users() {
        let (db, a) = db_with_user();
        db.conn.execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
        let mine = db.create_conversation(&a, "mine").unwrap();
        let theirs = db.create_conversation("B", "theirs").unwrap();
        let m = db.add_message(&a, &mine, "user", "How do I tame a zebra?").unwrap();
        db.add_message(&a, &mine, "assistant", "Slowly.").unwrap();
        db.add_message("B", &theirs, "user", "My zebra is private").unwrap();

        let hits = db.search_messages(&a, "zebra", 20).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(
            (hits[0].conversation_id.as_str(), hits[0].message_id.as_str(), hits[0].role.as_str()),
            (mine.as_str(), m.as_str(), "user")
        );
        assert!(hits[0].snippet.contains("zebra"));

        let theirs_only = db.search_messages("B", "zebra", 20).unwrap();
        assert_eq!(theirs_only.len(), 1);
        assert_eq!(theirs_only[0].conversation_id, theirs);
        assert!(db.search_messages("B", "tame", 20).unwrap().is_empty(), "B cannot find A's words");

        // Several words must all match; syntax characters are plain text.
        assert_eq!(db.search_messages(&a, "tame zebra", 20).unwrap().len(), 1);
        assert!(db.search_messages(&a, "tame lion", 20).unwrap().is_empty());
        for odd in ["\"", "zebra OR", "NEAR(", "*", "  ", "a:b", "-zebra"] {
            assert!(db.search_messages(&a, odd, 20).is_ok(), "{odd:?} must not be a syntax error");
        }

        // Edited and deleted messages leave the index.
        db.conn.execute("UPDATE messages SET content = 'giraffe' WHERE id = ?1", [&m]).unwrap();
        assert!(db.search_messages(&a, "zebra", 20).unwrap().is_empty());
        assert_eq!(db.search_messages(&a, "giraffe", 20).unwrap().len(), 1);
        db.delete_conversation(&a, &mine).unwrap();
        assert!(db.search_messages(&a, "giraffe", 20).unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_roles_and_orphans() {
        let (db, u) = db_with_user();
        let a = db.create_conversation(&u, "a").unwrap();
        assert!(db.add_message(&u, &a, "system", "nope").is_err());
        assert!(matches!(db.add_message(&u, "no-such-conversation", "user", "orphan"), Err(DbError::NotFound)));
    }

    #[test]
    fn adding_a_message_moves_conversation_to_top() {
        let (db, u) = db_with_user();
        let a = db.create_conversation(&u, "a").unwrap();
        let b = db.create_conversation(&u, "b").unwrap();
        // Same-second timestamps tie-break on id, so b is first...
        assert_eq!(db.list_conversations(&u).unwrap()[0].id, b);
        // ...and touching a with a later timestamp must not push it below b.
        db.conn.execute("UPDATE conversations SET updated_at = updated_at - 100 WHERE id = ?1", [&b]).unwrap();
        db.add_message(&u, &a, "user", "bump").unwrap();
        assert_eq!(db.list_conversations(&u).unwrap()[0].id, a);
    }

    #[test]
    fn persists_and_migrates_once_on_disk() {
        let dir = std::env::temp_dir().join(format!("askbar-db-test-{}", std::process::id()));
        let path = dir.join("sub").join("h.db");
        let _ = std::fs::remove_dir_all(&dir);
        let user = {
            let db = Db::open(&path).unwrap();
            let u = db.seed_server().unwrap();
            let c = db.create_conversation(&u, "keep me").unwrap();
            db.add_message(&u, &c, "user", "persist").unwrap();
            u
        };
        let db = Db::open(&path).unwrap();
        assert_eq!(db.seed_server().unwrap(), user, "seeding is idempotent");
        let list = db.list_conversations(&user).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(db.get_messages(&user, &list[0].id).unwrap()[0].content, "persist");
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let wal: String = db.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(wal, "wal");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tables(db: &Db) -> Vec<String> {
        let mut stmt = db.conn.prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view')").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn the_schema_creates_cleanly() {
        let db = Db::open_in_memory().unwrap();
        let t = tables(&db);
        for name in [
            "users",
            "clients",
            "conversations",
            "messages",
            "messages_fts",
            "user_prefs", // live
            "memory",
            "projects",
            "token_usage", // reserved
            "providers",   // only the encrypted API keys, by provider id
            "config_snapshot",
            "redeemed_codes",  // what the admin service pushes, and codes already used
            "generations",     // chat turns that outlive their client
            "provider_health", // providers whose credentials were refused
        ] {
            assert!(t.iter().any(|x| x == name), "missing table {name}");
        }
        for gone in ["settings", "pair_codes", "display_defaults"] {
            assert!(!t.iter().any(|x| x == gone), "{gone} belongs to the admin service now");
        }
        let cols: Vec<String> = {
            let mut stmt = db.conn.prepare("SELECT name FROM pragma_table_info('providers') ORDER BY cid").unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(cols, ["id", "api_key_enc"], "a provider keeps only its id and its encrypted key");
        assert!(!t.iter().any(|x| x == "client_tokens"), "clients replaces client_tokens");
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let app: i64 = db.conn.query_row("PRAGMA application_id", [], |r| r.get(0)).unwrap();
        assert_eq!(app, APPLICATION_ID);
        // Foreign keys hold together: nothing dangles in a fresh database.
        let bad: i64 = db.conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(bad, 0);

        let u = db.seed_server().unwrap();
        let c = db.create_conversation(&u, "c").unwrap();
        let project: Option<String> = db.conn.query_row("SELECT project_id FROM conversations WHERE id = ?1", [&c], |r| r.get(0)).unwrap();
        assert_eq!(project, None);
        for reserved in ["memory", "projects", "token_usage"] {
            let n: i64 = db.conn.query_row(&format!("SELECT COUNT(*) FROM {reserved}"), [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "{reserved} is reserved and stays empty");
        }
    }

    #[test]
    fn messages_are_indexed_by_triggers() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        db.add_message(&u, &c, "user", "the quick brown fox").unwrap();
        let hits = |q: &str| -> i64 {
            db.conn.query_row("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH ?1", [q], |r| r.get(0)).unwrap()
        };
        assert_eq!(hits("fox"), 1);
        db.delete_conversation(&u, &c).unwrap();
        assert_eq!(hits("fox"), 0, "deleting the conversation removes its messages from the index");
    }

    #[test]
    fn migrating_from_the_version_with_the_admin_tables_keeps_the_provider_keys() {
        // A database as the previous step of this refactor left it: version 2, with the interim tables.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (i, sql) in MIGRATIONS.iter().take(2).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 2").unwrap();
        conn.execute_batch(
            "INSERT INTO providers (id, name, type, model, api_key_enc, created_at) VALUES ('p1','one','openai','m',x'0102',0), ('p2','two','openai','m',NULL,0);
             INSERT INTO settings VALUES ('system_prompt', 'old');",
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let keys: Vec<(String, Option<Vec<u8>>)> = {
            let mut stmt = db.conn.prepare("SELECT id, api_key_enc FROM providers ORDER BY id").unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(keys, [("p1".to_string(), Some(vec![1, 2])), ("p2".to_string(), None)], "ids and keys survive");
        let t = tables(&db);
        assert!(["settings", "pair_codes", "display_defaults"].iter().all(|gone| !t.iter().any(|x| x == gone)));
    }

    #[test]
    fn a_database_from_before_the_clients_rename_keeps_its_paired_clients_and_generations() {
        // A 0.4.0 database: schema version 5, with a paired device (as it was called) and a generation it started. names-ok
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (i, sql) in MIGRATIONS.iter().take(5).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 5").unwrap();
        conn.execute_batch(
            "INSERT INTO users (id, name, created_at) VALUES ('u1','owner',0);
             INSERT INTO devices (id, user_id, name, hash, created_at) VALUES ('d1','u1','Work laptop','h1',0); -- names-ok
             INSERT INTO conversations (id, user_id, title, created_at, updated_at) VALUES ('c1','u1','t',0,0);
             INSERT INTO generations (id, user_id, device_id, conversation_id, client_request_id, status, queued_at) -- names-ok
                 VALUES ('g1','u1','d1','c1','r1','done',0);
             INSERT INTO redeemed_codes (hash, device_id, redeemed_at) VALUES ('code','d1',0);", // names-ok
        )
        .unwrap();
        let db = Db::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let t = tables(&db);
        assert!(t.iter().any(|x| x == "clients") && !t.iter().any(|x| x == "devices"), "{t:?}"); // names-ok
        let name: String = db.conn.query_row("SELECT name FROM clients WHERE hash = 'h1'", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Work laptop", "the paired client and its token hash survive");
        let g = db.find_by_request_id("d1", "r1").unwrap().expect("the generation is found by its client id");
        assert_eq!(g.id, "g1");
        let used: String = db.conn.query_row("SELECT client_id FROM redeemed_codes", [], |r| r.get(0)).unwrap();
        assert_eq!(used, "d1");
        assert!(crate::auth::authenticate(&db, "no such token").unwrap().is_none());
    }

    #[test]
    fn a_v3_database_upgrades_to_v4_and_keeps_its_data() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        for (i, sql) in MIGRATIONS.iter().take(3).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 3").unwrap();
        conn.execute_batch(
            "INSERT INTO users (id, name, created_at) VALUES ('u1','owner',0);
             INSERT INTO conversations (id, user_id, title, created_at, updated_at) VALUES ('c1','u1','t',0,0);
             INSERT INTO messages (id, conversation_id, user_id, role, content, created_at) VALUES ('m1','c1','u1','user','hi',0);",
        )
        .unwrap();
        assert!(!conn.prepare("SELECT 1 FROM sqlite_master WHERE name = 'generations'").unwrap().exists([]).unwrap());
        let db = Db::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        assert!(v >= 4);
        assert!(tables(&db).iter().any(|x| x == "generations"));
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1, "existing history survives");
        // The unique index covers (client, client_request_id) only when the latter is set.
        let ins = |id: &str, rid: Option<&str>| {
            db.conn.execute(
                "INSERT INTO generations (id, user_id, conversation_id, client_id, client_request_id, status, queued_at)
                 VALUES (?1, 'u1', 'c1', 'd1', ?2, 'running', 0)",
                params![id, rid],
            )
        };
        ins("g1", Some("r1")).unwrap();
        assert!(ins("g2", Some("r1")).is_err(), "a repeated request id from one client is refused");
        ins("g3", None).unwrap();
        ins("g4", None).unwrap();
    }

    fn new_gen<'a>(user: &'a str, conv: &'a str, status: &'a str) -> NewGeneration<'a> {
        NewGeneration {
            user_id: user,
            client_id: Some("dev1"),
            conversation_id: conv,
            user_message_id: None,
            client_request_id: None,
            provider_id: Some("p"),
            status,
            prompt: "hello",
            deadline_at: None,
        }
    }

    fn other_user(db: &Db) -> String {
        db.conn.execute("INSERT INTO users (id, name, created_at) VALUES ('u2','other',0)", []).unwrap();
        "u2".into()
    }

    #[test]
    fn generations_are_created_read_and_finalized() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let id = db.create_generation(&new_gen(&u, &c, "running")).unwrap();
        let g = db.get_generation(&u, &id).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.prompt.as_str(), g.text.as_str()), ("running", "hello", ""));
        assert!(g.started_at.is_some() && g.ended_at.is_none());

        db.flush_generation_text(&id, "partial").unwrap();
        assert_eq!(db.get_generation(&u, &id).unwrap().unwrap().text, "partial");

        assert!(db.finalize_generation(&id, "done", "full text", None, Some("m1")).unwrap());
        let g = db.get_generation(&u, &id).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.text.as_str(), g.assistant_message_id.as_deref()), ("done", "full text", Some("m1")));
        assert!(g.ended_at.is_some());

        // A finished generation is final: a second finalize and a late flush change nothing.
        assert!(!db.finalize_generation(&id, "error", "x", Some("boom"), None).unwrap());
        db.flush_generation_text(&id, "late").unwrap();
        let g = db.get_generation(&u, &id).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.text.as_str()), ("done", "full text"));
    }

    #[test]
    fn another_users_generation_is_invisible() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let id = db.create_generation(&new_gen(&u, &c, "running")).unwrap();
        let v = other_user(&db);
        assert_eq!(db.get_generation(&v, &id).unwrap(), None);
        assert!(db.list_active_generations(&v).unwrap().is_empty());
        assert_eq!(db.list_active_generations(&u).unwrap().len(), 1);
    }

    #[test]
    fn active_list_holds_running_and_queued_only() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let done = db.create_generation(&new_gen(&u, &c, "running")).unwrap();
        db.finalize_generation(&done, "done", "t", None, None).unwrap();
        let running = db.create_generation(&new_gen(&u, &c, "running")).unwrap();
        let queued = db.create_generation(&new_gen(&u, &c, "queued")).unwrap();
        let ids: Vec<String> = db.list_active_generations(&u).unwrap().into_iter().map(|g| g.id).collect();
        assert_eq!(ids, [running, queued]);
    }

    #[test]
    fn queue_counts_and_order() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let c2 = db.create_conversation(&u, "c2").unwrap();
        assert!(!db.conversation_busy(&c).unwrap());
        assert_eq!(db.next_queued(&c).unwrap(), None);
        db.create_generation(&new_gen(&u, &c, "running")).unwrap();
        assert!(db.conversation_busy(&c).unwrap());
        assert_eq!(db.queued_count(&c).unwrap(), 0);
        let q1 = db.create_generation(&new_gen(&u, &c, "queued")).unwrap();
        let q2 = db.create_generation(&new_gen(&u, &c, "queued")).unwrap();
        db.create_generation(&new_gen(&u, &c2, "queued")).unwrap();
        assert_eq!(db.queued_count(&c).unwrap(), 2, "other conversations don't count");
        assert_eq!(db.next_queued(&c).unwrap().unwrap().id, q1, "oldest first");

        db.start_generation(&q1, "um1", Some(99)).unwrap();
        let g = db.get_generation(&u, &q1).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.user_message_id.as_deref(), g.deadline_at), ("running", Some("um1"), Some(99)));
        assert!(g.started_at.is_some());
        assert_eq!(db.next_queued(&c).unwrap().unwrap().id, q2);
        db.finalize_generation(&q2, "cancelled", "", None, None).unwrap();
        assert_eq!(db.queued_count(&c).unwrap(), 0);
    }

    #[test]
    fn prune_removes_only_finished_rows_older_than_the_cutoff() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let end = |status: &str, ended_at: i64| {
            let id = db.create_generation(&new_gen(&u, &c, "running")).unwrap();
            db.finalize_generation(&id, status, "t", None, None).unwrap();
            db.conn.execute("UPDATE generations SET ended_at = ?2 WHERE id = ?1", params![id, ended_at]).unwrap();
            id
        };
        let old: Vec<String> = ["done", "error", "cancelled", "timed_out", "interrupted"].iter().map(|s| end(s, 100)).collect();
        let fresh = end("done", 1000);
        let running = db.create_generation(&new_gen(&u, &c, "running")).unwrap();
        let queued = db.create_generation(&new_gen(&u, &c, "queued")).unwrap();

        assert_eq!(db.prune_generations(500).unwrap(), 5);
        for id in &old {
            assert_eq!(db.get_generation(&u, id).unwrap(), None);
        }
        for id in [&fresh, &running, &queued] {
            assert!(db.get_generation(&u, id).unwrap().is_some(), "{id} stays");
        }
        assert_eq!(db.prune_generations(500).unwrap(), 0);
        assert_eq!(db.prune_generations(2000).unwrap(), 1, "a later cutoff takes the fresh one too");
    }

    #[test]
    fn find_by_request_id_is_per_client() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let mut n = new_gen(&u, &c, "running");
        n.client_request_id = Some("req-1");
        let id = db.create_generation(&n).unwrap();
        assert_eq!(db.find_by_request_id("dev1", "req-1").unwrap().unwrap().id, id);
        assert_eq!(db.find_by_request_id("dev2", "req-1").unwrap(), None);
        assert_eq!(db.find_by_request_id("dev1", "req-2").unwrap(), None);
    }

    #[test]
    fn deleting_a_conversation_removes_its_generations() {
        let (db, u) = db_with_user();
        let c = db.create_conversation(&u, "c").unwrap();
        let id = db.create_generation(&new_gen(&u, &c, "queued")).unwrap();
        db.delete_conversation(&u, &c).unwrap();
        assert_eq!(db.get_generation(&u, &id).unwrap(), None);
    }

    #[test]
    fn provider_health_flags_clears_and_keeps_the_first_time() {
        let (db, _) = db_with_user();
        assert!(db.provider_health().unwrap().is_empty());
        db.flag_provider_auth_failed("p1", "http_401").unwrap();
        db.conn.execute("UPDATE provider_health SET since = 100", []).unwrap();
        db.flag_provider_auth_failed("p1", "http_403").unwrap(); // flagged again: same since, newer code
        db.flag_provider_auth_failed("p2", "config").unwrap();
        let h = db.provider_health().unwrap();
        assert_eq!(
            h,
            [
                ("p1".into(), "auth_failed".into(), 100, Some("http_403".into())),
                ("p2".into(), "auth_failed".into(), h[1].2, Some("config".into()))
            ]
        );
        assert!(db.clear_provider_health("p1").unwrap());
        assert!(!db.clear_provider_health("p1").unwrap(), "already clear");
        assert_eq!(db.provider_health().unwrap().len(), 1);
        assert!(db.conn.execute("INSERT INTO provider_health VALUES ('x', 'broken', 0, NULL)", []).is_err(), "only the known status");
    }

    #[test]
    fn memory_is_append_only() {
        let (db, u) = db_with_user();
        db.conn.execute("INSERT INTO memory (id, user_id, content, created_at) VALUES ('m1', ?1, 'x', 0)", [&u]).unwrap();
        assert!(db.conn.execute("UPDATE memory SET content = 'y'", []).is_err());
        assert!(db.conn.execute("DELETE FROM memory", []).is_err());
    }

    #[test]
    fn an_unexpected_user_version_is_refused() {
        let dir = std::env::temp_dir().join(format!("askbar-ver-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let refused = |path: &Path| matches!(Db::open(path), Err(DbError::UnexpectedVersion { .. }));

        // Newer than this build knows.
        let newer = dir.join("newer.db");
        drop(Db::open(&newer).unwrap());
        Connection::open(&newer).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(refused(&newer));

        // A v0.2.0 database has user_version 2 and no Askbar application id.
        let old = dir.join("old.db");
        Connection::open(&old).unwrap().execute_batch("CREATE TABLE client_tokens (id TEXT); PRAGMA user_version = 1;").unwrap();
        assert!(refused(&old));
        let err = Db::open(&old).err().unwrap().to_string();
        assert!(err.contains("unexpected database version"), "{err}");

        // The refused file is left as it was.
        let v: i64 = Connection::open(&old).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn titles() {
        assert_eq!(make_title("  hello   world \nsecond line"), "hello world");
        assert_eq!(make_title("\n\n  \nthird"), "third");
        assert_eq!(make_title(""), "New conversation");
        let long = "x".repeat(200);
        let t = make_title(&long);
        assert_eq!(t.chars().count(), 60);
        assert!(t.ends_with('…'));
        // multi-byte characters must not be split
        let t = make_title(&"é".repeat(100));
        assert_eq!(t.chars().count(), 60);
    }
}
