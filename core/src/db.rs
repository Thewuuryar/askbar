//! SQLite conversation history.
//!
//! Schema changes go in `MIGRATIONS` (append only); `PRAGMA user_version`
//! records how many have been applied.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const MIGRATIONS: &[&str] = &[
    // v1: conversations and messages
    "CREATE TABLE conversations (
         id         INTEGER PRIMARY KEY,
         title      TEXT    NOT NULL,
         created_at INTEGER NOT NULL,
         updated_at INTEGER NOT NULL
     );
     CREATE TABLE messages (
         id              INTEGER PRIMARY KEY,
         conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
         role            TEXT    NOT NULL CHECK (role IN ('user','assistant')),
         content         TEXT    NOT NULL,
         created_at      INTEGER NOT NULL
     );
     CREATE INDEX idx_messages_conversation ON messages(conversation_id, id);
     CREATE INDEX idx_conversations_updated ON conversations(updated_at DESC);",
];

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("history database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not create {path}: {source}")]
    Io { path: String, source: std::io::Error },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: i64,
    pub title: String,
    pub updated_at: i64,
    pub message_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StoredMessage {
    pub id: i64,
    pub role: String,
    pub content: String,
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
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        let mut db = Db { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), DbError> {
        let current: usize = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? as usize;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current) {
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute_batch(&format!("PRAGMA user_version = {}", i + 1))?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn create_conversation(&self, title: &str) -> Result<i64, DbError> {
        let t = now();
        self.conn.execute(
            "INSERT INTO conversations (title, created_at, updated_at) VALUES (?1, ?2, ?2)",
            params![title, t],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn add_message(&self, conversation_id: i64, role: &str, content: &str) -> Result<i64, DbError> {
        let t = now();
        self.conn.execute(
            "INSERT INTO messages (conversation_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![conversation_id, role, content, t],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn.execute(
            "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
            params![t, conversation_id],
        )?;
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
            Ok(ConversationSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                updated_at: r.get(2)?,
                message_count: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn get_messages(&self, conversation_id: i64) -> Result<Vec<StoredMessage>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, created_at FROM messages
             WHERE conversation_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([conversation_id], |r| {
            Ok(StoredMessage {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                created_at: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Returns true if a conversation was deleted. Its messages go with it.
    pub fn delete_conversation(&self, id: i64) -> Result<bool, DbError> {
        Ok(self.conn.execute("DELETE FROM conversations WHERE id = ?1", [id])? > 0)
    }

    pub fn conversation_exists(&self, id: i64) -> Result<bool, DbError> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM conversations WHERE id = ?1", [id], |_| Ok(()))
            .is_ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_and_load() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_conversation("first").unwrap();
        let b = db.create_conversation("second").unwrap();
        db.add_message(a, "user", "hi").unwrap();
        db.add_message(a, "assistant", "hello").unwrap();
        db.add_message(b, "user", "yo").unwrap();

        let list = db.list_conversations().unwrap();
        assert_eq!(list.len(), 2);
        let first = list.iter().find(|c| c.id == a).unwrap();
        assert_eq!(first.message_count, 2);
        assert_eq!(first.title, "first");

        let msgs = db.get_messages(a).unwrap();
        assert_eq!(
            msgs.iter().map(|m| (m.role.as_str(), m.content.as_str())).collect::<Vec<_>>(),
            vec![("user", "hi"), ("assistant", "hello")]
        );
    }

    #[test]
    fn delete_cascades_to_messages() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_conversation("a").unwrap();
        let b = db.create_conversation("b").unwrap();
        db.add_message(a, "user", "x").unwrap();
        db.add_message(b, "user", "y").unwrap();

        assert!(db.delete_conversation(a).unwrap());
        assert!(!db.delete_conversation(a).unwrap(), "second delete is a no-op");
        assert!(db.get_messages(a).unwrap().is_empty());
        assert_eq!(db.get_messages(b).unwrap().len(), 1);
        assert_eq!(db.list_conversations().unwrap().len(), 1);
        assert!(!db.conversation_exists(a).unwrap());
        assert!(db.conversation_exists(b).unwrap());
    }

    #[test]
    fn rejects_unknown_roles_and_orphans() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_conversation("a").unwrap();
        assert!(db.add_message(a, "system", "nope").is_err());
        assert!(db.add_message(9999, "user", "orphan").is_err());
    }

    #[test]
    fn adding_a_message_moves_conversation_to_top() {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_conversation("a").unwrap();
        let b = db.create_conversation("b").unwrap();
        // Same-second timestamps tie-break on id, so b is first...
        assert_eq!(db.list_conversations().unwrap()[0].id, b);
        // ...and touching a with a later timestamp must not push it below b.
        db.conn
            .execute("UPDATE conversations SET updated_at = updated_at - 100 WHERE id = ?1", [b])
            .unwrap();
        db.add_message(a, "user", "bump").unwrap();
        assert_eq!(db.list_conversations().unwrap()[0].id, a);
    }

    #[test]
    fn persists_and_migrates_once_on_disk() {
        let dir = std::env::temp_dir().join(format!("askbar-db-test-{}", std::process::id()));
        let path = dir.join("sub").join("h.db");
        let _ = std::fs::remove_dir_all(&dir);
        {
            let db = Db::open(&path).unwrap();
            let c = db.create_conversation("keep me").unwrap();
            db.add_message(c, "user", "persist").unwrap();
        }
        let db = Db::open(&path).unwrap();
        let list = db.list_conversations().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(db.get_messages(list[0].id).unwrap()[0].content, "persist");
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
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
