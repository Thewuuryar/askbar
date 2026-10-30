//! `admin.db`: everything the admin service owns. Nothing in it is shared with the server's
//! database; the server only ever sees what the admin pushes it.
//!
//! Schema changes go in `MIGRATIONS` (append only). A database this build did not create,
//! or one newer than it, is refused.

use rusqlite::Connection;
use std::path::Path;

/// "ASKA", so the server's database (application id "ASKB") is never mistaken for this one.
const APPLICATION_ID: i64 = 0x4153_4B41;

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE admin_credentials (
         id            INTEGER PRIMARY KEY CHECK (id = 1),
         password_hash TEXT    NOT NULL,
         updated_at    INTEGER NOT NULL
     );
     CREATE TABLE providers (
         id          TEXT PRIMARY KEY,
         name        TEXT    NOT NULL UNIQUE,
         type        TEXT    NOT NULL CHECK (type IN ('openai','anthropic')),
         base_url    TEXT    NOT NULL DEFAULT '',
         model       TEXT    NOT NULL,
         max_tokens  INTEGER,
         temperature REAL,
         headers     TEXT    NOT NULL DEFAULT '{}',
         created_at  INTEGER NOT NULL
     );
     CREATE TABLE settings (
         key   TEXT PRIMARY KEY,
         value TEXT NOT NULL
     );
     CREATE TABLE display_defaults (
         user_id TEXT PRIMARY KEY,
         value   TEXT NOT NULL
     );
     CREATE TABLE pair_codes (
         hash        TEXT PRIMARY KEY,
         user_id     TEXT    NOT NULL,
         device_name TEXT    NOT NULL DEFAULT '',
         expires_at  INTEGER NOT NULL,
         created_at  INTEGER NOT NULL
     );
     CREATE TABLE budgets (
         id             INTEGER PRIMARY KEY CHECK (id = 1),
         daily_tokens   INTEGER,
         monthly_tokens INTEGER,
         kill_switch    INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE network_settings (
         id                INTEGER PRIMARY KEY CHECK (id = 1),
         hostname          TEXT    NOT NULL DEFAULT '',
         tls_enabled       INTEGER NOT NULL DEFAULT 0,
         staging_ca        INTEGER NOT NULL DEFAULT 0,
         cloudflare_token  BLOB,
         -- Which addresses may open the admin site. Off unless switched on.
         allowlist_enabled INTEGER NOT NULL DEFAULT 0,
         allowlist         TEXT    NOT NULL DEFAULT ''
     );
     INSERT INTO budgets (id) VALUES (1);
     INSERT INTO network_settings (id) VALUES (1);",
    // v2 (additive): devices the admin has revoked; they go to the server in the snapshot.
    "CREATE TABLE revoked_devices (
         device_id  TEXT PRIMARY KEY,
         revoked_at INTEGER NOT NULL
     );",
];

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("admin database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not create {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error("unexpected admin database version: {found} (this build writes versions up to {expected}); refusing to touch it")]
    UnexpectedVersion { found: i64, expected: usize },
}

pub struct AdminDb {
    conn: Connection,
}

impl AdminDb {
    pub fn open(path: &Path) -> Result<AdminDb, DbError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| DbError::Io { path: dir.display().to_string(), source: e })?;
        }
        AdminDb::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<AdminDb, DbError> {
        AdminDb::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<AdminDb, DbError> {
        conn.execute_batch("PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")?;
        let mut db = AdminDb { conn };
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

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}
