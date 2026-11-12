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
         device_name TEXT    NOT NULL DEFAULT '', -- names-ok
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
    // v2 (additive): devices the admin has revoked; they go to the server in the snapshot. names-ok
    "CREATE TABLE revoked_devices ( -- names-ok
         device_id  TEXT PRIMARY KEY, -- names-ok
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
    // v5: who opened what. Every time the administrator opens sealed content (or fails to), a row
    // is written first, so reading the trail leaves a trail. No content and no passphrase, only
    // which span, which key, when, and whether the passphrase was right.
    "CREATE TABLE audit_access (
         id        INTEGER PRIMARY KEY AUTOINCREMENT,
         span_id   TEXT    NOT NULL,
         key_id    TEXT    NOT NULL,
         opened_at INTEGER NOT NULL,
         ok        INTEGER NOT NULL CHECK (ok IN (0, 1))
     );",
    // v6: how each provider is paid for, and what its tokens cost (per million). Existing
    // providers are metered with no prices, which is what the server assumes when told nothing.
    "ALTER TABLE providers ADD COLUMN billing TEXT NOT NULL DEFAULT 'metered';
     ALTER TABLE providers ADD COLUMN allow_as_fallback INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE providers ADD COLUMN price_in REAL;
     ALTER TABLE providers ADD COLUMN price_out REAL;
     ALTER TABLE providers ADD COLUMN price_cached_in REAL;",
    // v7: one word for the paired app: clients (they were called devices). Rows are kept. names-ok
    "ALTER TABLE revoked_devices RENAME TO revoked_clients; -- names-ok
     ALTER TABLE revoked_clients RENAME COLUMN device_id TO client_id; -- names-ok
     ALTER TABLE pair_codes RENAME COLUMN device_name TO client_name; -- names-ok",
    // v8: the admin API key, for scripts. Only the SHA-256 of the key is kept, with a name, a scope
    // (`admin` is the only one so far) and when it was made and last used. At most one key is active
    // (not revoked) at a time; a revoked row stays as a record.
    "CREATE TABLE admin_api_keys (
         id           TEXT PRIMARY KEY,
         name         TEXT    NOT NULL,
         hash         TEXT    NOT NULL UNIQUE,
         scope        TEXT    NOT NULL DEFAULT 'admin',
         created_at   INTEGER NOT NULL,
         last_used_at INTEGER,
         revoked_at   INTEGER
     );
     CREATE UNIQUE INDEX admin_api_keys_one_active ON admin_api_keys((1)) WHERE revoked_at IS NULL;",
    // v9: who started, stopped or restarted a service, and when. Written before the controller is asked.
    "CREATE TABLE service_actions (
         id      INTEGER PRIMARY KEY AUTOINCREMENT,
         service TEXT    NOT NULL,
         action  TEXT    NOT NULL,
         at      INTEGER NOT NULL,
         ok      INTEGER NOT NULL DEFAULT 0 CHECK (ok IN (0, 1))
     );",
    // v10: a provider the admin made and keeps in step itself (the bundled CLIProxyAPI). Its name, type,
    // address and key are not editable, and it cannot be deleted.
    "ALTER TABLE providers ADD COLUMN managed INTEGER NOT NULL DEFAULT 0;",
    // v11: the published first-run password must be changed before anything else (ADR 0007).
    // Existing installs chose their own password, so they start with nothing to change.
    "ALTER TABLE admin_credentials ADD COLUMN must_change INTEGER NOT NULL DEFAULT 0 CHECK (must_change IN (0, 1));",
    // v12: the users the admin makes (ADR 0008); pushed to the server, which keeps them. An install from
    // before adopts the server's users the first time the list is read (see admin_api::list_users).
    "CREATE TABLE users (
         id         TEXT PRIMARY KEY,
         name       TEXT    NOT NULL UNIQUE,
         created_at INTEGER NOT NULL
     );",
    // v13: the provider keys the admin keeps so it can hand them to `model-gateway` (ADR 0054), each encrypted
    // with the admin's own key (`admin.key`). Write-only like the rest: no endpoint returns one. A key the server
    // held before this version is not here, because the admin could never read it; it is entered again.
    "CREATE TABLE provider_keys (
         provider_id TEXT PRIMARY KEY,
         key_enc     BLOB    NOT NULL,
         updated_at  INTEGER NOT NULL
     );",
    // The newest entry of the auditor's hash chain the admin has seen and checked (ADR 0010 rule 7): the one place the
    // chain's head is kept apart from the auditor, which is what lets a rollback be noticed.
    "CREATE TABLE audit_chain_head (
         id         INTEGER PRIMARY KEY CHECK (id = 1),
         seq        INTEGER NOT NULL,
         hash       TEXT    NOT NULL,
         checked_at INTEGER NOT NULL
     );",
    // The emergency stop for one person (plan step 3.2, ADR 0014): a paused user's replies are refused, and the gateway
    // stops the ones running.
    "ALTER TABLE users ADD COLUMN paused INTEGER NOT NULL DEFAULT 0 CHECK (paused IN (0, 1));",
    // The approved model catalog (plan step 3.3, ADR 0014): the models of each provider people may use, in the order the
    // administrator saved them. The default is the `default_model` setting. An install starts with none approved.
    "CREATE TABLE catalog_models (
         provider_id TEXT    NOT NULL REFERENCES providers(id) ON DELETE CASCADE,
         model       TEXT    NOT NULL,
         position    INTEGER NOT NULL,
         PRIMARY KEY (provider_id, model)
     );",
    // How alerts are delivered (plan step 4.1, ADR 0055): the ntfy server, topic and access token, each encrypted with the
    // admin's own key (`admin.key`), in one row. Write-only like the provider keys: no endpoint returns one, and the API
    // answers with booleans. The alerter holds them in memory only; the admin pushes them again whenever it starts. No row
    // means no channel is set up.
    "CREATE TABLE alert_settings (
         id             INTEGER PRIMARY KEY CHECK (id = 1),
         ntfy_server_enc BLOB    NOT NULL,
         ntfy_topic_enc  BLOB    NOT NULL,
         ntfy_token_enc  BLOB,
         updated_at      INTEGER NOT NULL
     );",
    // Email as a second way to deliver alerts (plan step 4.2b, ADR 0055): the mail server, who the mail is from and to, and the
    // login, each value that names or unlocks something encrypted with `admin.key` like the ntfy settings. The port and the TLS
    // mode are not secrets. One row; no row means email is not set up.
    "CREATE TABLE alert_email_settings (
         id           INTEGER PRIMARY KEY CHECK (id = 1),
         host_enc     BLOB    NOT NULL,
         port         INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
         tls          TEXT    NOT NULL CHECK (tls IN ('starttls', 'tls', 'none')),
         user_enc     BLOB,
         password_enc BLOB,
         from_enc     BLOB    NOT NULL,
         to_enc       BLOB    NOT NULL,
         updated_at   INTEGER NOT NULL
     );",
    // How Caddy proves it owns the host name to Let's Encrypt (plan step 5.3, ADR 0015, D19): `dns` (the Cloudflare DNS challenge, which
    // needs the stored token and works for a private address) or `http` (HTTP-01, which needs public port 80 and no token). Every
    // install that had a certificate set up used the DNS challenge, so that is the default.
    "ALTER TABLE network_settings ADD COLUMN acme_challenge TEXT NOT NULL DEFAULT 'dns' CHECK (acme_challenge IN ('dns', 'http'));",
    // The backup key (plan step 6.1, ADR 0013, D22): an age key pair. `recipient` is the public key (`age1...`) that every component
    // encrypts its export to; `protected` is the private key encrypted with the owner's passphrase (the file `backup-key.age` that
    // every backup carries), which only that passphrase opens. The passphrase is never stored. Rotating makes a new pair and retires
    // the old one, which stays listed (older backups carry their own protected key). At most one key is active.
    "CREATE TABLE backup_keys (
         id          INTEGER PRIMARY KEY AUTOINCREMENT,
         recipient   TEXT    NOT NULL UNIQUE CHECK (recipient LIKE 'age1%'),
         fingerprint TEXT    NOT NULL,
         protected   BLOB    NOT NULL CHECK (length(protected) > 0),
         created_at  INTEGER NOT NULL,
         retired_at  INTEGER
     );
     CREATE UNIQUE INDEX backup_keys_one_active ON backup_keys((1)) WHERE retired_at IS NULL;",
];

/// The schema steps, in order (for the upgrade tests).
#[doc(hidden)]
pub fn migrations() -> &'static [&'static str] {
    MIGRATIONS
}

#[doc(hidden)]
pub fn application_id() -> i64 {
    APPLICATION_ID
}

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

/// One row of the backup keyring (ADR 0013).
#[derive(Clone, PartialEq, Eq)]
pub struct BackupKeyRow {
    pub recipient: String,
    pub fingerprint: String,
    /// The private key encrypted with the passphrase: `backup-key.age`.
    pub protected: Vec<u8>,
    pub created_at: i64,
    pub retired_at: Option<i64>,
}

impl std::fmt::Debug for BackupKeyRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackupKeyRow").field("fingerprint", &self.fingerprint).finish_non_exhaustive()
    }
}

/// The active backup key, if the owner has set one.
pub fn active_backup_key(conn: &Connection) -> rusqlite::Result<Option<BackupKeyRow>> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT recipient, fingerprint, protected, created_at, retired_at FROM backup_keys WHERE retired_at IS NULL", [], |r| {
        Ok(BackupKeyRow {
            recipient: r.get(0)?,
            fingerprint: r.get(1)?,
            protected: r.get(2)?,
            created_at: r.get(3)?,
            retired_at: r.get(4)?,
        })
    })
    .optional()
}

/// How many backup keys there have been, the active one included.
pub fn backup_key_count(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM backup_keys", [], |r| r.get(0))
}

/// Retires the active backup key, if any, and makes `new` the active one, in the caller's transaction. The unique index settles two
/// calls at once: one wins.
pub fn replace_active_backup_key_in(conn: &Connection, new: &BackupKeyRow, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE backup_keys SET retired_at = ?1 WHERE retired_at IS NULL", [now])?;
    conn.execute(
        "INSERT INTO backup_keys (recipient, fingerprint, protected, created_at, retired_at) VALUES (?1, ?2, ?3, ?4, NULL)",
        rusqlite::params![new.recipient, new.fingerprint, new.protected, new.created_at],
    )?;
    Ok(())
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
        public_key: public
            .try_into()
            .map_err(|_| rusqlite::Error::InvalidColumnType(1, "public_key".into(), rusqlite::types::Type::Blob))?,
        salt: salt.try_into().map_err(|_| rusqlite::Error::InvalidColumnType(2, "salt".into(), rusqlite::types::Type::Blob))?,
        argon_params: r.get(3)?,
        created_at: r.get(4)?,
        retired_at: r.get(5)?,
    })
}

/// Retires the active key (if any) and makes `new` the active one, using the caller's transaction.
pub fn replace_active_audit_key_in(conn: &Connection, new: &AuditKeyRow, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE audit_keys SET retired_at = ?1 WHERE retired_at IS NULL", [now])?;
    conn.execute(
        "INSERT INTO audit_keys (key_id, public_key, salt, argon_params, created_at, retired_at) VALUES (?1,?2,?3,?4,?5,NULL)",
        rusqlite::params![new.key_id, new.public_key.as_slice(), new.salt.as_slice(), new.argon_params, new.created_at],
    )?;
    Ok(())
}

/// Records an attempt to open the content of `span_id` (sealed to `key_id`).
pub fn record_audit_access(conn: &Connection, span_id: &str, key_id: &str, opened_at: i64, ok: bool) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO audit_access (span_id, key_id, opened_at, ok) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![span_id, key_id, opened_at, ok as i64],
    )?;
    Ok(())
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
    /// Tests only: open a connection that already holds an older schema.
    #[doc(hidden)]
    pub fn init_for_test(conn: Connection) -> Result<AdminDb, DbError> {
        AdminDb::init(conn)
    }

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
        conn.execute_batch(
            "PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;",
        )?;
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
        replace_active_audit_key_in(&tx, new, now)?;
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

    /// Every key ever made, oldest first: the registry the sealed-content session opens with one passphrase.
    pub fn audit_keys(&self) -> Result<Vec<AuditKeyRow>, DbError> {
        let mut stmt = self
            .conn
            .prepare("SELECT key_id, public_key, salt, argon_params, created_at, retired_at FROM audit_keys ORDER BY created_at, key_id")?;
        let rows = stmt.query_map([], read_key)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// The key the server seals with now, if one has been made.
    pub fn active_audit_key(&self) -> Result<Option<AuditKeyRow>, DbError> {
        Ok(active_audit_key(&self.conn)?)
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_database_from_before_the_clients_rename_keeps_its_revoked_clients_and_pairing_codes() {
        // A 0.4.0 database: schema version 6, with a revoked device (as it was called) and a pairing code. names-ok
        let conn = Connection::open_in_memory().unwrap();
        for (i, sql) in MIGRATIONS.iter().take(6).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 6").unwrap();
        conn.execute_batch(
            "INSERT INTO revoked_devices (device_id, revoked_at) VALUES ('d1', 5); -- names-ok
             INSERT INTO pair_codes (hash, user_id, device_name, expires_at, created_at) VALUES ('h', 'u', 'Work laptop', 9, 1);", // names-ok
        )
        .unwrap();
        let db = AdminDb::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let revoked: (String, i64) =
            db.conn.query_row("SELECT client_id, revoked_at FROM revoked_clients", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(revoked, ("d1".to_string(), 5));
        let name: String = db.conn.query_row("SELECT client_name FROM pair_codes WHERE hash = 'h'", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Work laptop");
    }
    use super::*;

    #[test]
    fn a_database_from_before_email_alerts_keeps_its_ntfy_settings_and_gains_an_empty_email_table() {
        let conn = Connection::open_in_memory().unwrap();
        // The database as it was just before the step that adds the email table (found by its text, so later steps do not move it).
        let before = MIGRATIONS.iter().position(|m| m.contains("CREATE TABLE alert_email_settings")).expect("the email step");
        for (i, sql) in MIGRATIONS.iter().take(before).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch(&format!("PRAGMA user_version = {before}")).unwrap();
        conn.execute_batch("INSERT INTO alert_settings (id, ntfy_server_enc, ntfy_topic_enc, updated_at) VALUES (1, x'01', x'02', 7)")
            .unwrap();
        assert!(conn.prepare("SELECT 1 FROM alert_email_settings").is_err(), "the table is not there before the step");
        let db = AdminDb::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let kept: i64 = db.conn.query_row("SELECT updated_at FROM alert_settings WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, 7);
        let rows: i64 = db.conn.query_row("SELECT COUNT(*) FROM alert_email_settings", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0);
        // The table refuses a TLS mode it does not know and a port out of range.
        for (port, tls) in [(0, "tls"), (70000, "tls"), (587, "ssl")] {
            let r = db.conn.execute(
                "INSERT INTO alert_email_settings (id, host_enc, port, tls, from_enc, to_enc, updated_at) VALUES (1, x'01', ?1, ?2, x'02', x'03', 1)",
                rusqlite::params![port, tls],
            );
            assert!(r.is_err(), "{port} {tls}");
        }
    }

    #[test]
    fn a_database_from_before_the_challenge_setting_keeps_its_network_settings_and_stays_on_dns() {
        let conn = Connection::open_in_memory().unwrap();
        let before = MIGRATIONS.iter().position(|m| m.contains("ADD COLUMN acme_challenge")).expect("the challenge step");
        for (i, sql) in MIGRATIONS.iter().take(before).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch(&format!("PRAGMA user_version = {before}")).unwrap();
        conn.execute_batch(
            "UPDATE network_settings SET hostname = 'askbar.example.org', tls_enabled = 1, staging_ca = 1, cloudflare_token = x'0102'",
        )
        .unwrap();
        assert!(conn.prepare("SELECT acme_challenge FROM network_settings").is_err(), "the column is not there before the step");
        let db = AdminDb::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let row: (String, i64, i64, String, Vec<u8>) = db
            .conn
            .query_row(
                "SELECT hostname, tls_enabled, staging_ca, acme_challenge, cloudflare_token FROM network_settings WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            ("askbar.example.org".to_string(), 1, 1, "dns".to_string(), vec![1, 2]),
            "nothing changed, and it is still the DNS challenge"
        );
        db.conn.execute("UPDATE network_settings SET acme_challenge = 'http'", []).unwrap();
        for bad in ["tls-alpn", "", "HTTP"] {
            assert!(db.conn.execute("UPDATE network_settings SET acme_challenge = ?1", [bad]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_database_from_before_the_backup_key_gains_an_empty_keyring_that_allows_one_active_key() {
        let conn = Connection::open_in_memory().unwrap();
        let before = MIGRATIONS.iter().position(|m| m.contains("CREATE TABLE backup_keys")).expect("the backup key step");
        for (i, sql) in MIGRATIONS.iter().take(before).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {APPLICATION_ID}")).unwrap();
            }
        }
        conn.execute_batch(&format!("PRAGMA user_version = {before}")).unwrap();
        conn.execute_batch("UPDATE network_settings SET hostname = 'askbar.example.org'").unwrap();
        assert!(conn.prepare("SELECT 1 FROM backup_keys").is_err(), "the table is not there before the step");
        let db = AdminDb::init(conn).unwrap();
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let host: String = db.conn.query_row("SELECT hostname FROM network_settings", [], |r| r.get(0)).unwrap();
        assert_eq!(host, "askbar.example.org", "nothing else changed");
        assert_eq!(backup_key_count(&db.conn).unwrap(), 0);
        assert!(active_backup_key(&db.conn).unwrap().is_none());
        let row = |n: &str, at: i64| BackupKeyRow {
            recipient: format!("age1{n}"),
            fingerprint: "aaaa-bbbb-cccc-dddd".into(),
            protected: vec![1, 2, 3],
            created_at: at,
            retired_at: None,
        };
        replace_active_backup_key_in(&db.conn, &row("one", 1), 1).unwrap();
        replace_active_backup_key_in(&db.conn, &row("two", 2), 2).unwrap();
        assert_eq!(backup_key_count(&db.conn).unwrap(), 2);
        assert_eq!(active_backup_key(&db.conn).unwrap().unwrap().recipient, "age1two", "the newer one is active, the older retired");
        // Two active keys at once, an empty protected key and a recipient that is not an age key are refused by the table.
        assert!(db
            .conn
            .execute("INSERT INTO backup_keys (recipient, fingerprint, protected, created_at) VALUES ('age1three', 'f', x'01', 3)", [])
            .is_err());
        db.conn.execute("UPDATE backup_keys SET retired_at = 9", []).unwrap();
        assert!(db
            .conn
            .execute("INSERT INTO backup_keys (recipient, fingerprint, protected, created_at) VALUES ('age1four', 'f', x'', 4)", [])
            .is_err());
        assert!(db
            .conn
            .execute("INSERT INTO backup_keys (recipient, fingerprint, protected, created_at) VALUES ('notage', 'f', x'01', 4)", [])
            .is_err());
        // A key's Debug output shows its fingerprint and nothing else.
        let shown = format!("{:?}", row("secret", 1));
        assert!(shown.contains("aaaa-bbbb") && !shown.contains("age1secret"), "{shown}");
    }

    fn key(id: &str, retired: Option<i64>) -> AuditKeyRow {
        AuditKeyRow {
            key_id: id.into(),
            public_key: [id.len() as u8; 32],
            salt: [3; 16],
            argon_params: "m=65536,t=3,p=1".into(),
            created_at: 100,
            retired_at: retired,
        }
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
        let cols: Vec<String> = db
            .conn
            .prepare("SELECT name FROM pragma_table_info('audit_keys') ORDER BY cid")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
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
