//! SQLite storage: users, devices, conversations and messages.
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
    // Live: users, devices, conversations, messages, messages_fts, user_prefs.
    // Reserved, empty: memory (append-only), projects, token_usage.
    // Interim until migration 3: providers (full rows), settings, pair_codes, display_defaults.
    "CREATE TABLE users (
         id         TEXT PRIMARY KEY,
         name       TEXT    NOT NULL,
         created_at INTEGER NOT NULL
     );
     CREATE TABLE devices (
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
         device_id   TEXT,
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
         device_name TEXT    NOT NULL DEFAULT '',
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
         device_id   TEXT,
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

pub struct Db {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// First non-empty line of `text`, collapsed and cut to 60 characters.
pub fn make_title(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("New conversation");
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
            std::fs::create_dir_all(dir).map_err(|e| DbError::Io {
                path: dir.display().to_string(),
                source: e,
            })?;
        }
        Db::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Db, DbError> {
        Db::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Db, DbError> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")?;
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
        let existing: Option<String> = self
            .conn
            .query_row("SELECT id FROM users ORDER BY created_at, id LIMIT 1", [], |r| r.get(0))
            .optional()?;
        match existing {
            Some(u) => Ok(u),
            None => {
                let id = new_id();
                self.conn.execute(
                    "INSERT INTO users (id, name, created_at) VALUES (?1, 'owner', ?2)",
                    params![id, now()],
                )?;
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
            Ok(ConversationSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                updated_at: r.get(2)?,
                message_count: r.get(3)?,
            })
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
                |r| {
                    Ok(ConversationSummary {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        updated_at: r.get(2)?,
                        message_count: r.get(3)?,
                    })
                },
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
            Ok(StoredMessage {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Full-text search over `user_id`'s own messages, best match first. Every word
    /// must appear; punctuation in `query` is treated as plain text, never as search syntax.
    pub fn search_messages(&self, user_id: &str, query: &str, limit: usize) -> Result<Vec<SearchHit>, DbError> {
        let terms: Vec<String> = query
            .split_whitespace()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect();
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
            Ok(SearchHit {
                conversation_id: r.get(0)?,
                message_id: r.get(1)?,
                role: r.get(2)?,
                snippet: r.get(3)?,
                created_at: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Returns true if a conversation was deleted. Its messages go with it.
    pub fn delete_conversation(&self, user_id: &str, id: &str) -> Result<bool, DbError> {
        Ok(self
            .conn
            .execute("DELETE FROM conversations WHERE id = ?1 AND user_id = ?2", params![id, user_id])?
            > 0)
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
        assert_eq!((hits[0].conversation_id.as_str(), hits[0].message_id.as_str(), hits[0].role.as_str()), (mine.as_str(), m.as_str(), "user"));
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
        db.conn
            .execute("UPDATE conversations SET updated_at = updated_at - 100 WHERE id = ?1", [&b])
            .unwrap();
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
            "users", "devices", "conversations", "messages", "messages_fts", "user_prefs", // live
            "memory", "projects", "token_usage", // reserved
            "providers", // only the encrypted API keys, by provider id
            "config_snapshot", "redeemed_codes", // what the admin service pushes, and codes already used
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
        assert!(!t.iter().any(|x| x == "device_tokens"), "devices replaces device_tokens");
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let app: i64 = db.conn.query_row("PRAGMA application_id", [], |r| r.get(0)).unwrap();
        assert_eq!(app, APPLICATION_ID);
        // Foreign keys hold together: nothing dangles in a fresh database.
        let bad: i64 = db.conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(bad, 0);

        let u = db.seed_server().unwrap();
        let c = db.create_conversation(&u, "c").unwrap();
        let project: Option<String> =
            db.conn.query_row("SELECT project_id FROM conversations WHERE id = ?1", [&c], |r| r.get(0)).unwrap();
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
        Connection::open(&old)
            .unwrap()
            .execute_batch("CREATE TABLE device_tokens (id TEXT); PRAGMA user_version = 1;")
            .unwrap();
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
