//! Spaces (ADR 0009): each space's conversations, messages, search index, memory and generations
//! live in a SQLite file of their own, `<data>/spaces/<space id>.db`. Who may use a space is kept
//! in the control database (`spaces`, `space_members`); a handler gets a space's store only through
//! `Spaces::for_member`, after that check, so a query bug in one space cannot reach another's rows.
//!
//! Schema changes go in `SPACE_MIGRATIONS` (append only) and run on every space file when it is
//! opened. A file this build did not create, or that is newer than it, is refused.

use crate::db::DbError;
use crate::ids::new_id;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Stamped into `PRAGMA application_id` of every space file ("ASKS"), so the control database and
/// a space file can never be opened as each other.
const SPACE_APPLICATION_ID: i64 = 0x4153_4B53;

const SPACE_MIGRATIONS: &[&str] = &[
    // v1: one space's history. Who wrote a row is kept (`created_by`, `user_id`) for shared spaces,
    // but no query filters on it: the file is the boundary, and membership is checked before it opens.
    "CREATE TABLE conversations (
         id         TEXT PRIMARY KEY,
         created_by TEXT    NOT NULL,
         title      TEXT    NOT NULL,
         created_at INTEGER NOT NULL,
         updated_at INTEGER NOT NULL
     );
     CREATE INDEX idx_conversations_updated ON conversations(updated_at DESC, id DESC);
     CREATE TABLE messages (
         id              TEXT PRIMARY KEY,
         conversation_id TEXT    NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
         user_id         TEXT    NOT NULL,
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
     -- Reserved, empty until memory is built. Each entry records who or what created it (ADR 0009).
     CREATE TABLE memory (
         id         TEXT PRIMARY KEY,
         created_by TEXT    NOT NULL,
         content    TEXT    NOT NULL,
         created_at INTEGER NOT NULL
     );
     CREATE TRIGGER memory_append_only_update BEFORE UPDATE ON memory BEGIN
         SELECT RAISE(ABORT, 'memory is append-only');
     END;
     CREATE TRIGGER memory_append_only_delete BEFORE DELETE ON memory BEGIN
         SELECT RAISE(ABORT, 'memory is append-only');
     END;
     -- Chat turns run by the server; they hold the prompt and the reply, so they live with the history.
     CREATE TABLE generations (
         id                   TEXT PRIMARY KEY,
         user_id              TEXT    NOT NULL,
         client_id            TEXT,
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
     CREATE UNIQUE INDEX idx_generations_request ON generations(client_id, client_request_id)
         WHERE client_request_id IS NOT NULL;
     CREATE INDEX idx_generations_conversation ON generations(conversation_id, status);
     CREATE INDEX idx_generations_status ON generations(status);",
];

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

/// One chat turn run by the server. See `SPACE_MIGRATIONS` for the statuses.
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

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// One space's file.
pub struct SpaceStore {
    conn: Connection,
}

impl SpaceStore {
    pub fn open(path: &Path) -> Result<SpaceStore, DbError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DbError::Io { path: dir.display().to_string(), source: e })?;
        }
        SpaceStore::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<SpaceStore, DbError> {
        SpaceStore::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<SpaceStore, DbError> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )?;
        let mut store = SpaceStore { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> Result<(), DbError> {
        let current: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current != 0 {
            let app: i64 = self.conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
            if current < 0 || current as usize > SPACE_MIGRATIONS.len() || app != SPACE_APPLICATION_ID {
                return Err(DbError::UnexpectedVersion { found: current, expected: SPACE_MIGRATIONS.len() });
            }
        }
        for (i, sql) in SPACE_MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            if i == 0 {
                tx.execute_batch(&format!("PRAGMA application_id = {SPACE_APPLICATION_ID}"))?;
            }
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Raw connection, for the server's own queries on this space's file.
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Returns the new conversation's id. `created_by` is the user who started it.
    pub fn create_conversation(&self, created_by: &str, title: &str) -> Result<String, DbError> {
        let (id, t) = (new_id(), now());
        self.conn.execute(
            "INSERT INTO conversations (id, created_by, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, created_by, title, t],
        )?;
        Ok(id)
    }

    /// Adds a message by `user_id` to a conversation in this space; `NotFound` if there is no such conversation.
    pub fn add_message(&self, user_id: &str, conversation_id: &str, role: &str, content: &str) -> Result<String, DbError> {
        let (id, t) = (new_id(), now());
        let tx = self.conn.unchecked_transaction()?;
        let added = tx.execute(
            "INSERT INTO messages (id, conversation_id, user_id, role, content, created_at)
             SELECT ?1, id, ?3, ?4, ?5, ?6 FROM conversations WHERE id = ?2",
            params![id, conversation_id, user_id, role, content, t],
        )?;
        if added == 0 {
            return Err(DbError::NotFound);
        }
        tx.execute("UPDATE conversations SET updated_at = ?1 WHERE id = ?2", params![t, conversation_id])?;
        tx.commit()?;
        Ok(id)
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.updated_at,
                    (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id)
             FROM conversations c
             ORDER BY c.updated_at DESC, c.id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ConversationSummary { id: r.get(0)?, title: r.get(1)?, updated_at: r.get(2)?, message_count: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// A conversation in this space, or `None` if there is no such conversation.
    pub fn get_conversation(&self, id: &str) -> Result<Option<ConversationSummary>, DbError> {
        Ok(self
            .conn
            .query_row(
                "SELECT c.id, c.title, c.updated_at,
                        (SELECT COUNT(*) FROM messages m WHERE m.conversation_id = c.id)
                 FROM conversations c WHERE c.id = ?1",
                params![id],
                |r| Ok(ConversationSummary { id: r.get(0)?, title: r.get(1)?, updated_at: r.get(2)?, message_count: r.get(3)? }),
            )
            .optional()?)
    }

    /// Oldest first. Empty if there is no such conversation.
    pub fn get_messages(&self, conversation_id: &str) -> Result<Vec<StoredMessage>, DbError> {
        let mut stmt =
            self.conn.prepare("SELECT id, role, content, created_at FROM messages WHERE conversation_id = ?1 ORDER BY id ASC")?;
        let rows = stmt.query_map(params![conversation_id], |r| {
            Ok(StoredMessage { id: r.get(0)?, role: r.get(1)?, content: r.get(2)?, created_at: r.get(3)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Full-text search over this space's messages, best match first. Every word must appear;
    /// punctuation in `query` is treated as plain text, never as search syntax.
    pub fn search_messages(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, DbError> {
        let terms: Vec<String> = query.split_whitespace().map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT m.conversation_id, m.id, m.role, snippet(messages_fts, 0, '', '', '…', 16), m.created_at
             FROM messages_fts JOIN messages m ON m.rowid = messages_fts.rowid
             WHERE messages_fts MATCH ?1
             ORDER BY messages_fts.rank, m.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![terms.join(" "), limit as i64], |r| {
            Ok(SearchHit { conversation_id: r.get(0)?, message_id: r.get(1)?, role: r.get(2)?, snippet: r.get(3)?, created_at: r.get(4)? })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Returns true if a conversation was deleted. Its messages and generations go with it.
    pub fn delete_conversation(&self, id: &str) -> Result<bool, DbError> {
        Ok(self.conn.execute("DELETE FROM conversations WHERE id = ?1", params![id])? > 0)
    }

    /// Deletes finished generations that ended before `older_than` (unix seconds). Running and
    /// queued rows are never touched. Returns how many went.
    pub fn prune_generations(&self, older_than: i64) -> Result<usize, DbError> {
        Ok(self.conn.execute("DELETE FROM generations WHERE status NOT IN ('queued','running') AND ended_at < ?1", [older_than])?)
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

    /// A generation in this space; `None` if there is no such generation.
    pub fn get_generation(&self, id: &str) -> Result<Option<Generation>, DbError> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {GENERATION_COLUMNS} FROM generations WHERE id = ?1"), params![id], read_generation)
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

    /// This space's running and queued generations, oldest first.
    pub fn list_active_generations(&self) -> Result<Vec<Generation>, DbError> {
        let mut stmt =
            self.conn.prepare(&format!("SELECT {GENERATION_COLUMNS} FROM generations WHERE status IN ('running','queued') ORDER BY id"))?;
        let rows = stmt.query_map([], read_generation)?;
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

    /// A generation this client already started in this space with `client_request_id`.
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

/// One open space: its id and its store.
pub struct Space {
    pub id: String,
    store: Mutex<SpaceStore>,
}

impl Space {
    /// The space's store. Lock order: the control database first, then a space, never the reverse.
    pub fn lock(&self) -> MutexGuard<'_, SpaceStore> {
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Whether `id` can name a space file: a ULID in its canonical form (26 characters, upper case).
/// Ids come from the control database, but the check keeps any other text out of a file path.
pub fn is_space_id(id: &str) -> bool {
    ulid::Ulid::from_string(id).is_ok_and(|u| u.to_string() == id)
}

/// The space files, opened on first use and kept open. With no directory (tests), each space is an
/// in-memory database that lives as long as this value.
pub struct Spaces {
    dir: Option<PathBuf>,
    open: Mutex<HashMap<String, Arc<Space>>>,
}

impl Spaces {
    /// Space files under `dir` (`<dir>/<space id>.db`).
    pub fn on_disk(dir: PathBuf) -> Spaces {
        Spaces { dir: Some(dir), open: Mutex::new(HashMap::new()) }
    }

    pub fn in_memory() -> Spaces {
        Spaces { dir: None, open: Mutex::new(HashMap::new()) }
    }

    /// The file a space is kept in, or `None` in memory.
    pub fn path_of(&self, id: &str) -> Option<PathBuf> {
        self.dir.as_ref().filter(|_| is_space_id(id)).map(|d| d.join(format!("{id}.db")))
    }

    /// Opens a space's store (creating and migrating its file the first time). Crate-private: a
    /// handler reaches a space only through `for_member`.
    pub(crate) fn open(&self, id: &str) -> Result<Arc<Space>, DbError> {
        if !is_space_id(id) {
            return Err(DbError::NotFound);
        }
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = open.get(id) {
            return Ok(s.clone());
        }
        let store = match &self.dir {
            Some(dir) => SpaceStore::open(&dir.join(format!("{id}.db")))?,
            None => SpaceStore::open_in_memory()?,
        };
        let space = Arc::new(Space { id: id.to_string(), store: Mutex::new(store) });
        open.insert(id.to_string(), space.clone());
        Ok(space)
    }

    /// The space `space_id`, if `user_id` is a member of it (the control database says so);
    /// `None` for a space that does not exist or that the user is not in.
    pub fn for_member(&self, control: &crate::Db, user_id: &str, space_id: &str) -> Result<Option<Arc<Space>>, DbError> {
        if !control.is_member(space_id, user_id)? {
            return Ok(None);
        }
        self.open(space_id).map(Some)
    }

    /// Every space the control database lists, opened. At start-up and after a configuration push,
    /// so each space's file exists and has every migration.
    pub fn open_all(&self, control: &crate::Db) -> Result<Vec<Arc<Space>>, DbError> {
        control.space_ids()?.iter().map(|id| self.open(id)).collect()
    }

    /// The spaces open now (for work across spaces that only concerns what is running).
    pub fn opened(&self) -> Vec<Arc<Space>> {
        let mut v: Vec<_> = self.open.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SpaceStore {
        SpaceStore::open_in_memory().unwrap()
    }

    const U: &str = "u1";

    #[test]
    fn create_list_and_load() {
        let db = store();
        let a = db.create_conversation(U, "first").unwrap();
        let b = db.create_conversation(U, "second").unwrap();
        db.add_message(U, &a, "user", "hi").unwrap();
        db.add_message(U, &a, "assistant", "hello").unwrap();
        db.add_message(U, &b, "user", "yo").unwrap();

        let list = db.list_conversations().unwrap();
        assert_eq!(list.len(), 2);
        let first = list.iter().find(|c| c.id == a).unwrap();
        assert_eq!(first.message_count, 2);
        assert_eq!(first.title, "first");

        let msgs = db.get_messages(&a).unwrap();
        assert_eq!(
            msgs.iter().map(|m| (m.role.as_str(), m.content.as_str())).collect::<Vec<_>>(),
            vec![("user", "hi"), ("assistant", "hello")]
        );
    }

    #[test]
    fn delete_cascades_to_messages() {
        let db = store();
        let a = db.create_conversation(U, "a").unwrap();
        let b = db.create_conversation(U, "b").unwrap();
        db.add_message(U, &a, "user", "x").unwrap();
        db.add_message(U, &b, "user", "y").unwrap();

        assert!(db.delete_conversation(&a).unwrap());
        assert!(!db.delete_conversation(&a).unwrap(), "second delete is a no-op");
        assert!(db.get_messages(&a).unwrap().is_empty());
        assert_eq!(db.get_messages(&b).unwrap().len(), 1);
        assert_eq!(db.list_conversations().unwrap().len(), 1);
        assert!(db.get_conversation(&a).unwrap().is_none());
        assert!(db.get_conversation(&b).unwrap().is_some());
    }

    /// ADR 0009: a query bug in one space cannot reach another's rows. Two space files, and on one
    /// of them every query a buggy handler could write with no filter at all: only that file's rows.
    #[test]
    fn a_query_with_no_filter_on_one_space_returns_only_that_spaces_rows() {
        let spaces = Spaces::in_memory();
        let (a_id, b_id) = (new_id(), new_id());
        let (a, b) = (spaces.open(&a_id).unwrap(), spaces.open(&b_id).unwrap());
        let mine = a.lock().create_conversation("ua", "mine").unwrap();
        a.lock().add_message("ua", &mine, "user", "my zebra question").unwrap();
        let theirs = b.lock().create_conversation("ub", "theirs").unwrap();
        b.lock().add_message("ub", &theirs, "user", "their zebra secret").unwrap();
        b.lock().create_generation(&new_gen("ub", &theirs, "running")).unwrap();

        let db = a.lock();
        let all = |sql: &str| -> Vec<String> {
            let mut stmt = db.conn().prepare(sql).unwrap();
            stmt.query_map([], |r| r.get::<_, String>(0)).unwrap().map(Result::unwrap).collect()
        };
        assert_eq!(all("SELECT id FROM conversations"), std::slice::from_ref(&mine));
        assert_eq!(all("SELECT content FROM messages"), ["my zebra question"]);
        assert!(all("SELECT id FROM generations").is_empty());
        assert_eq!(all("SELECT content FROM messages_fts WHERE messages_fts MATCH 'zebra'"), ["my zebra question"]);
        // The store's own reads, which filter on nothing but the space.
        assert_eq!(db.list_conversations().unwrap().len(), 1);
        assert_eq!(db.search_messages("zebra", 20).unwrap().len(), 1);
        assert!(db.search_messages("secret", 20).unwrap().is_empty());
        assert!(db.get_conversation(&theirs).unwrap().is_none());
        assert!(db.get_messages(&theirs).unwrap().is_empty());
        assert!(db.list_active_generations().unwrap().is_empty());
        assert!(matches!(db.add_message("ua", &theirs, "user", "intrusion"), Err(DbError::NotFound)));
        assert!(!db.delete_conversation(&theirs).unwrap());
        drop(db);
        assert_eq!(b.lock().get_messages(&theirs).unwrap().len(), 1, "the other space is untouched");
    }

    #[test]
    fn search_finds_a_message_and_treats_syntax_as_text() {
        let db = store();
        let mine = db.create_conversation(U, "mine").unwrap();
        let m = db.add_message(U, &mine, "user", "How do I tame a zebra?").unwrap();
        db.add_message(U, &mine, "assistant", "Slowly.").unwrap();

        let hits = db.search_messages("zebra", 20).unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(
            (hits[0].conversation_id.as_str(), hits[0].message_id.as_str(), hits[0].role.as_str()),
            (mine.as_str(), m.as_str(), "user")
        );
        assert!(hits[0].snippet.contains("zebra"));

        // Several words must all match; syntax characters are plain text.
        assert_eq!(db.search_messages("tame zebra", 20).unwrap().len(), 1);
        assert!(db.search_messages("tame lion", 20).unwrap().is_empty());
        for odd in ["\"", "zebra OR", "NEAR(", "*", "  ", "a:b", "-zebra"] {
            assert!(db.search_messages(odd, 20).is_ok(), "{odd:?} must not be a syntax error");
        }

        // Edited and deleted messages leave the index.
        db.conn.execute("UPDATE messages SET content = 'giraffe' WHERE id = ?1", [&m]).unwrap();
        assert!(db.search_messages("zebra", 20).unwrap().is_empty());
        assert_eq!(db.search_messages("giraffe", 20).unwrap().len(), 1);
        db.delete_conversation(&mine).unwrap();
        assert!(db.search_messages("giraffe", 20).unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_roles_and_orphans() {
        let db = store();
        let a = db.create_conversation(U, "a").unwrap();
        assert!(db.add_message(U, &a, "system", "nope").is_err());
        assert!(matches!(db.add_message(U, "no-such-conversation", "user", "orphan"), Err(DbError::NotFound)));
    }

    #[test]
    fn adding_a_message_moves_conversation_to_top() {
        let db = store();
        let a = db.create_conversation(U, "a").unwrap();
        let b = db.create_conversation(U, "b").unwrap();
        // Same-second timestamps tie-break on id, so b is first...
        assert_eq!(db.list_conversations().unwrap()[0].id, b);
        // ...and touching a with a later timestamp must not push it below b.
        db.conn.execute("UPDATE conversations SET updated_at = updated_at - 100 WHERE id = ?1", [&b]).unwrap();
        db.add_message(U, &a, "user", "bump").unwrap();
        assert_eq!(db.list_conversations().unwrap()[0].id, a);
    }

    #[test]
    fn a_space_file_persists_and_migrates_once_on_disk() {
        let dir = std::env::temp_dir().join(format!("askbar-space-test-{}", new_id()));
        let id = new_id();
        {
            let spaces = Spaces::on_disk(dir.clone());
            let s = spaces.open(&id).unwrap();
            let c = s.lock().create_conversation(U, "keep me").unwrap();
            s.lock().add_message(U, &c, "user", "persist").unwrap();
        }
        assert!(dir.join(format!("{id}.db")).exists(), "one file per space, named by its id");
        let spaces = Spaces::on_disk(dir.clone());
        let s = spaces.open(&id).unwrap();
        let db = s.lock();
        let list = db.list_conversations().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(db.get_messages(&list[0].id).unwrap()[0].content, "persist");
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, SPACE_MIGRATIONS.len());
        let app: i64 = db.conn.query_row("PRAGMA application_id", [], |r| r.get(0)).unwrap();
        assert_eq!(app, SPACE_APPLICATION_ID);
        let wal: String = db.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(wal, "wal");
        drop(db);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_canonical_space_id_names_a_file() {
        let spaces = Spaces::on_disk(std::env::temp_dir().join("askbar-space-ids"));
        let id = new_id();
        assert!(is_space_id(&id));
        for bad in ["", "personal", "../control", "01ARZ3NDEKTSV4RRFFQ69G5FA/", &id.to_lowercase(), &format!("{id}.db")] {
            assert!(!is_space_id(bad), "{bad:?}");
            assert!(spaces.path_of(bad).is_none(), "{bad:?}");
            assert!(matches!(spaces.open(bad), Err(DbError::NotFound)), "{bad:?}");
        }
    }

    #[test]
    fn a_control_database_is_not_opened_as_a_space() {
        let dir = std::env::temp_dir().join(format!("askbar-space-app-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("control.db");
        drop(crate::Db::open(&path).unwrap());
        assert!(matches!(SpaceStore::open(&path), Err(DbError::UnexpectedVersion { .. })));
        let space = dir.join("s.db");
        drop(SpaceStore::open(&space).unwrap());
        assert!(matches!(crate::Db::open(&space), Err(DbError::UnexpectedVersion { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_schema_creates_cleanly() {
        let db = store();
        let mut stmt = db.conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap();
        let t: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for name in ["conversations", "messages", "messages_fts", "memory", "generations"] {
            assert!(t.iter().any(|x| x == name), "missing table {name}");
        }
        for control in ["users", "clients", "providers", "spaces", "space_members", "token_usage", "projects"] {
            assert!(!t.iter().any(|x| x == control), "{control} does not belong in a space file");
        }
        let bad: i64 = db.conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0)).unwrap();
        assert_eq!(bad, 0);
    }

    #[test]
    fn messages_are_indexed_by_triggers() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        db.add_message(U, &c, "user", "the quick brown fox").unwrap();
        let hits = |q: &str| -> i64 {
            db.conn.query_row("SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH ?1", [q], |r| r.get(0)).unwrap()
        };
        assert_eq!(hits("fox"), 1);
        db.delete_conversation(&c).unwrap();
        assert_eq!(hits("fox"), 0, "deleting the conversation removes its messages from the index");
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

    #[test]
    fn generations_are_created_read_and_finalized() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        let id = db.create_generation(&new_gen(U, &c, "running")).unwrap();
        let g = db.get_generation(&id).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.prompt.as_str(), g.text.as_str()), ("running", "hello", ""));
        assert!(g.started_at.is_some() && g.ended_at.is_none());

        db.flush_generation_text(&id, "partial").unwrap();
        assert_eq!(db.get_generation(&id).unwrap().unwrap().text, "partial");

        assert!(db.finalize_generation(&id, "done", "full text", None, Some("m1")).unwrap());
        let g = db.get_generation(&id).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.text.as_str(), g.assistant_message_id.as_deref()), ("done", "full text", Some("m1")));
        assert!(g.ended_at.is_some());

        // A finished generation is final: a second finalize and a late flush change nothing.
        assert!(!db.finalize_generation(&id, "error", "x", Some("boom"), None).unwrap());
        db.flush_generation_text(&id, "late").unwrap();
        let g = db.get_generation(&id).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.text.as_str()), ("done", "full text"));
    }

    #[test]
    fn active_list_holds_running_and_queued_only() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        let done = db.create_generation(&new_gen(U, &c, "running")).unwrap();
        db.finalize_generation(&done, "done", "t", None, None).unwrap();
        let running = db.create_generation(&new_gen(U, &c, "running")).unwrap();
        let queued = db.create_generation(&new_gen(U, &c, "queued")).unwrap();
        let ids: Vec<String> = db.list_active_generations().unwrap().into_iter().map(|g| g.id).collect();
        assert_eq!(ids, [running, queued]);
    }

    #[test]
    fn queue_counts_and_order() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        let c2 = db.create_conversation(U, "c2").unwrap();
        assert!(!db.conversation_busy(&c).unwrap());
        assert_eq!(db.next_queued(&c).unwrap(), None);
        db.create_generation(&new_gen(U, &c, "running")).unwrap();
        assert!(db.conversation_busy(&c).unwrap());
        assert_eq!(db.queued_count(&c).unwrap(), 0);
        let q1 = db.create_generation(&new_gen(U, &c, "queued")).unwrap();
        let q2 = db.create_generation(&new_gen(U, &c, "queued")).unwrap();
        db.create_generation(&new_gen(U, &c2, "queued")).unwrap();
        assert_eq!(db.queued_count(&c).unwrap(), 2, "other conversations don't count");
        assert_eq!(db.next_queued(&c).unwrap().unwrap().id, q1, "oldest first");

        db.start_generation(&q1, "um1", Some(99)).unwrap();
        let g = db.get_generation(&q1).unwrap().unwrap();
        assert_eq!((g.status.as_str(), g.user_message_id.as_deref(), g.deadline_at), ("running", Some("um1"), Some(99)));
        assert!(g.started_at.is_some());
        assert_eq!(db.next_queued(&c).unwrap().unwrap().id, q2);
        db.finalize_generation(&q2, "cancelled", "", None, None).unwrap();
        assert_eq!(db.queued_count(&c).unwrap(), 0);
    }

    #[test]
    fn prune_removes_only_finished_rows_older_than_the_cutoff() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        let end = |status: &str, ended_at: i64| {
            let id = db.create_generation(&new_gen(U, &c, "running")).unwrap();
            db.finalize_generation(&id, status, "t", None, None).unwrap();
            db.conn.execute("UPDATE generations SET ended_at = ?2 WHERE id = ?1", params![id, ended_at]).unwrap();
            id
        };
        let old: Vec<String> = ["done", "error", "cancelled", "timed_out", "interrupted"].iter().map(|s| end(s, 100)).collect();
        let fresh = end("done", 1000);
        let running = db.create_generation(&new_gen(U, &c, "running")).unwrap();
        let queued = db.create_generation(&new_gen(U, &c, "queued")).unwrap();

        assert_eq!(db.prune_generations(500).unwrap(), 5);
        for id in &old {
            assert_eq!(db.get_generation(id).unwrap(), None);
        }
        for id in [&fresh, &running, &queued] {
            assert!(db.get_generation(id).unwrap().is_some(), "{id} stays");
        }
        assert_eq!(db.prune_generations(500).unwrap(), 0);
        assert_eq!(db.prune_generations(2000).unwrap(), 1, "a later cutoff takes the fresh one too");
    }

    #[test]
    fn find_by_request_id_is_per_client() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        let mut n = new_gen(U, &c, "running");
        n.client_request_id = Some("req-1");
        let id = db.create_generation(&n).unwrap();
        assert_eq!(db.find_by_request_id("dev1", "req-1").unwrap().unwrap().id, id);
        assert_eq!(db.find_by_request_id("dev2", "req-1").unwrap(), None);
        assert_eq!(db.find_by_request_id("dev1", "req-2").unwrap(), None);
        // The unique index covers (client, client_request_id) only when the latter is set.
        assert!(db.create_generation(&n).is_err(), "a repeated request id from one client is refused");
        n.client_request_id = None;
        db.create_generation(&n).unwrap();
        db.create_generation(&n).unwrap();
    }

    #[test]
    fn deleting_a_conversation_removes_its_generations() {
        let db = store();
        let c = db.create_conversation(U, "c").unwrap();
        let id = db.create_generation(&new_gen(U, &c, "queued")).unwrap();
        db.delete_conversation(&c).unwrap();
        assert_eq!(db.get_generation(&id).unwrap(), None);
    }

    #[test]
    fn memory_is_append_only() {
        let db = store();
        db.conn.execute("INSERT INTO memory (id, created_by, content, created_at) VALUES ('m1', ?1, 'x', 0)", [U]).unwrap();
        assert!(db.conn.execute("UPDATE memory SET content = 'y'", []).is_err());
        assert!(db.conn.execute("DELETE FROM memory", []).is_err());
    }

    #[test]
    fn an_unexpected_space_version_is_refused() {
        let dir = std::env::temp_dir().join(format!("askbar-space-ver-{}", new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let newer = dir.join("newer.db");
        drop(SpaceStore::open(&newer).unwrap());
        Connection::open(&newer).unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(matches!(SpaceStore::open(&newer), Err(DbError::UnexpectedVersion { .. })));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
