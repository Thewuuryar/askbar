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
    // v3 (additive): whether the server holds an API key for the provider. The key itself
    // is never stored here.
    "ALTER TABLE providers ADD COLUMN has_key INTEGER NOT NULL DEFAULT 0;",
    // v4: the audit keyring. Public data only: the public key, the salt and the Argon2 cost that
    // turn the passphrase into the private key. The passphrase and the private key are never
    // stored anywhere. At most one key is active (not retired); retired keys stay so content
    // sealed with them can still be opened.
    "CREATE TABLE audit_keys (
         key_id       TEXT PRIMARY KEY,
         public_key   BLOB    NOT NULL CHECK (length(public_key) = 32),
         salt         BLOB    NOT NULL CHECK (length(salt) = 16),
         argon_params TEXT    NOT NULL,
         created_at   INTEGER NOT NULL,
         retired_at   INTEGER
     );
     CREATE UNIQUE INDEX audit_keys_one_active ON audit_keys((1)) WHERE retired_at IS NULL;",
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

/// One row of the audit keyring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditKeyRow {
    pub key_id: String,
    pub public_key: [u8; 32],
    pub salt: [u8; 16],
    pub argon_params: String,
    pub created_at: i64,
    pub retired_at: Option<i64>,
}

pub struct AdminDb {
    conn: Connection,
}

/// Adds a key as the active one; fails if another is active (see `AdminDb::insert_audit_key`).
pub fn insert_audit_key(conn: &Connection, k: &AuditKeyRow) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO audit_keys (key_id, public_key, salt, argon_params, created_at, retired_at) VALUES (?1,?2,?3,?4,?5,?6)",
        rusqlite::params![k.key_id, k.public_key.as_slice(), k.salt.as_slice(), k.argon_params, k.created_at, k.retired_at],
    )?;
    Ok(())
}

fn read_key(r: &rusqlite::Row<'_>) -> rusqlite::Result<AuditKeyRow> {
    let public: Vec<u8> = r.get(1)?;
    let salt: Vec<u8> = r.get(2)?;
    Ok(AuditKeyRow {
        key_id: r.get(0)?,
        public_key: public.try_into().map_err(|_| rusqlite::Error::InvalidColumnType(1, "public_key".into(), rusqlite::types::Type::Blob))?,
        salt: salt.try_into().map_err(|_| rusqlite::Error::InvalidColumnType(2, "salt".into(), rusqlite::types::Type::Blob))?,
        argon_params: r.get(3)?,
        created_at: r.get(4)?,
        retired_at: r.get(5)?,
    })
}

/// The key the server seals with now, if one has been made.
pub fn active_audit_key(conn: &Connection) -> rusqlite::Result<Option<AuditKeyRow>> {
    use rusqlite::OptionalExtension;
    conn.query_row(
        "SELECT key_id, public_key, salt, argon_params, created_at, retired_at FROM audit_keys WHERE retired_at IS NULL",
        [],
        read_key,
    )
    .optional()
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

    /// Adds a key to the keyring as the active one. Fails if another key is still active:
    /// replacing one is `replace_active_audit_key`.
    pub fn insert_audit_key(&self, k: &AuditKeyRow) -> Result<(), DbError> {
        Ok(insert_audit_key(&self.conn, k)?)
    }

    /// Retires the active key (if any) and makes `new` the active one, in one transaction, so there
    /// is never a moment with two active keys or none.
    pub fn replace_active_audit_key(&mut self, new: &AuditKeyRow, now: i64) -> Result<(), DbError> {
        let tx = self.conn.transaction()?;
        tx.execute("UPDATE audit_keys SET retired_at = ?1 WHERE retired_at IS NULL", [now])?;
        tx.execute(
            "INSERT INTO audit_keys (key_id, public_key, salt, argon_params, created_at, retired_at) VALUES (?1,?2,?3,?4,?5,NULL)",
            rusqlite::params![new.key_id, new.public_key.as_slice(), new.salt.as_slice(), new.argon_params, new.created_at],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// A key by its id, retired or not.
    pub fn audit_key(&self, key_id: &str) -> Result<Option<AuditKeyRow>, DbError> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT key_id, public_key, salt, argon_params, created_at, retired_at FROM audit_keys WHERE key_id = ?1",
                [key_id],
                read_key,
            )
            .optional()?)
    }

    /// The key the server seals with now, if one has been made.
    pub fn active_audit_key(&self) -> Result<Option<AuditKeyRow>, DbError> {
        Ok(active_audit_key(&self.conn)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(id: &str, retired: Option<i64>) -> AuditKeyRow {
        AuditKeyRow { key_id: id.into(), public_key: [id.len() as u8; 32], salt: [3; 16], argon_params: "m=65536,t=3,p=1".into(), created_at: 100, retired_at: retired }
    }

    #[test]
    fn the_keyring_stores_public_data_and_finds_a_key_by_id() {
        let db = AdminDb::open_in_memory().unwrap();
        assert_eq!(db.active_audit_key().unwrap(), None, "no key until one is made");
        let k = key("aaaa", None);
        db.insert_audit_key(&k).unwrap();
        assert_eq!(db.audit_key("aaaa").unwrap(), Some(k.clone()));
        assert_eq!(db.active_audit_key().unwrap(), Some(k));
        assert_eq!(db.audit_key("nope").unwrap(), None);
        // Nothing in the table could hold a passphrase or a private key.
        let cols: Vec<String> = db.conn.prepare("SELECT name FROM pragma_table_info('audit_keys') ORDER BY cid").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(cols, ["key_id", "public_key", "salt", "argon_params", "created_at", "retired_at"]);
    }

    #[test]
    fn only_one_key_is_active_at_a_time_and_a_replaced_key_stays_findable() {
        let mut db = AdminDb::open_in_memory().unwrap();
        db.insert_audit_key(&key("aaaa", None)).unwrap();
        assert!(db.insert_audit_key(&key("bbbbb", None)).is_err(), "a second active key is refused");
        // An already-retired key can be added next to an active one (restoring history).
        db.insert_audit_key(&key("old", Some(50))).unwrap();

        db.replace_active_audit_key(&key("cccccc", None), 200).unwrap();
        let active = db.active_audit_key().unwrap().unwrap();
        assert_eq!(active.key_id, "cccccc");
        let retired = db.audit_key("aaaa").unwrap().unwrap();
        assert_eq!(retired.retired_at, Some(200), "the old one is retired, not deleted");
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM audit_keys WHERE retired_at IS NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        // A failed replacement (a duplicate id) leaves the active key as it was.
        assert!(db.replace_active_audit_key(&key("cccccc", None), 300).is_err());
        assert_eq!(db.active_audit_key().unwrap().unwrap().key_id, "cccccc");
        assert_eq!(db.active_audit_key().unwrap().unwrap().retired_at, None);
    }

    #[test]
    fn a_key_with_the_wrong_sized_public_key_or_salt_is_refused_by_the_table() {
        let db = AdminDb::open_in_memory().unwrap();
        for sql in [
            "INSERT INTO audit_keys VALUES ('x', x'00', x'00000000000000000000000000000000', 'p', 1, NULL)",
            "INSERT INTO audit_keys VALUES ('y', x'0000000000000000000000000000000000000000000000000000000000000000', x'00', 'p', 1, NULL)",
        ] {
            assert!(db.conn.execute(sql, []).is_err(), "{sql}");
        }
    }
}
