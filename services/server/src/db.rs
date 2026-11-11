//! The control database (`control.db`): users, clients, pairing hashes, spaces, memberships, the
//! snapshot the admin pushed (with its budget copy), token usage and provider health. Each space's
//! history is in its own file (`space`, ADR 0009).
//!
//! Schema changes go in `MIGRATIONS` (append only); `PRAGMA user_version`
//! records how many have been applied. A database that this build did not
//! create, or that is newer than it, is refused rather than touched.

use crate::ids::new_id;
use rusqlite::{params, Connection, OptionalExtension};
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
    // v8: spaces (ADR 0009). Conversations, messages, their index, memory and generations move to
    // one file per space (`space::SPACE_MIGRATIONS`); here they are dropped, with the unused
    // `projects`. Who may use a space is kept here. Token usage records the space it was spent in.
    "DROP TABLE generations;
     DROP TRIGGER messages_fts_insert;
     DROP TRIGGER messages_fts_delete;
     DROP TRIGGER messages_fts_update;
     DROP TABLE messages_fts;
     DROP TABLE messages;
     DROP TABLE conversations;
     DROP TABLE projects;
     DROP TABLE memory;
     ALTER TABLE token_usage ADD COLUMN space_id TEXT;
     CREATE TABLE spaces (
         id            TEXT PRIMARY KEY,
         kind          TEXT    NOT NULL CHECK (kind IN ('personal','shared')),
         name          TEXT    NOT NULL DEFAULT '',
         owner_user_id TEXT    REFERENCES users(id) ON DELETE CASCADE,
         created_at    INTEGER NOT NULL,
         CHECK ((kind = 'personal') = (owner_user_id IS NOT NULL))
     );
     CREATE UNIQUE INDEX idx_spaces_personal ON spaces(owner_user_id) WHERE kind = 'personal';
     CREATE TABLE space_members (
         space_id TEXT    NOT NULL REFERENCES spaces(id) ON DELETE CASCADE,
         user_id  TEXT    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
         added_at INTEGER NOT NULL,
         PRIMARY KEY (space_id, user_id)
     );
     CREATE INDEX idx_space_members_user ON space_members(user_id);",
    // v9: a user's chosen model, `provider/model` from the catalog (ADR 0014), in place of the provider they used to
    // choose. `active_provider_id` stays in the table, unused.
    "ALTER TABLE user_prefs ADD COLUMN active_model TEXT;",
];

/// A provider whose credentials were refused: its id, status, since when (unix seconds) and error code.
pub type ProviderHealth = (String, String, i64, Option<String>);

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

pub struct Db {
    conn: Connection,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A space as the control database lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceRow {
    pub id: String,
    pub kind: String,
    pub name: String,
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
        db.migrate(MIGRATIONS.len())?;
        Ok(db)
    }

    /// Applies the migrations after the file's version, up to `upto` (all of them, except in tests
    /// that check an upgrade to an earlier version).
    fn migrate(&mut self, upto: usize) -> Result<(), DbError> {
        let current: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current != 0 {
            let app: i64 = self.conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
            if current < 0 || current as usize > MIGRATIONS.len() || app != APPLICATION_ID {
                return Err(DbError::UnexpectedVersion { found: current, expected: MIGRATIONS.len() });
            }
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().take(upto).skip(current as usize) {
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

    /// Tests: make sure one user exists, with its personal space. Returns the user's id.
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
        .and_then(|u| self.ensure_personal_space(&u).map(|_| u))
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
    pub fn provider_health(&self) -> Result<Vec<ProviderHealth>, DbError> {
        let mut stmt = self.conn.prepare("SELECT provider_id, status, since, error_code FROM provider_health ORDER BY provider_id")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The user's personal space, made (with its membership) the first time. Returns its id.
    pub fn ensure_personal_space(&self, user_id: &str) -> Result<String, DbError> {
        let tx = self.conn.unchecked_transaction()?;
        let existing: Option<String> =
            tx.query_row("SELECT id FROM spaces WHERE kind = 'personal' AND owner_user_id = ?1", [user_id], |r| r.get(0)).optional()?;
        let id = match existing {
            Some(id) => id,
            None => {
                let (id, t) = (new_id(), now());
                tx.execute(
                    "INSERT INTO spaces (id, kind, name, owner_user_id, created_at) VALUES (?1, 'personal', '', ?2, ?3)",
                    params![id, user_id, t],
                )?;
                id
            }
        };
        tx.execute(
            "INSERT INTO space_members (space_id, user_id, added_at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
            params![id, user_id, now()],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// The spaces `user_id` is a member of: the personal one first, then shared ones by name.
    pub fn spaces_for(&self, user_id: &str) -> Result<Vec<SpaceRow>, DbError> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.kind, s.name FROM spaces s JOIN space_members m ON m.space_id = s.id
             WHERE m.user_id = ?1 ORDER BY s.kind = 'personal' DESC, s.name, s.id",
        )?;
        let rows = stmt.query_map([user_id], |r| Ok(SpaceRow { id: r.get(0)?, kind: r.get(1)?, name: r.get(2)? }))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Whether `user_id` is a member of `space_id`. This is the membership check every space route makes.
    pub fn is_member(&self, space_id: &str, user_id: &str) -> Result<bool, DbError> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM space_members WHERE space_id = ?1 AND user_id = ?2)",
            params![space_id, user_id],
            |r| r.get(0),
        )?)
    }

    /// Every space's id, oldest first.
    pub fn space_ids(&self) -> Result<Vec<String>, DbError> {
        let mut stmt = self.conn.prepare("SELECT id FROM spaces ORDER BY id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database that has the first `upto` migrations only, for tests of an upgrade to that version.
    fn init_to(conn: Connection, upto: usize) -> Db {
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        let mut db = Db { conn };
        db.migrate(upto).unwrap();
        db
    }

    #[test]
    fn persists_and_migrates_once_on_disk() {
        let dir = std::env::temp_dir().join(format!("askbar-db-test-{}", new_id()));
        let path = dir.join("sub").join("control.db");
        let (user, space) = {
            let db = Db::open(&path).unwrap();
            let u = db.seed_server().unwrap();
            let s = db.ensure_personal_space(&u).unwrap();
            (u, s)
        };
        let db = Db::open(&path).unwrap();
        assert_eq!(db.seed_server().unwrap(), user, "seeding is idempotent");
        assert_eq!(db.ensure_personal_space(&user).unwrap(), space, "one personal space per user");
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
            "user_prefs",
            "token_usage",
            "providers", // only the encrypted API keys, by provider id
            "config_snapshot",
            "redeemed_codes",    // what the admin service pushes, and codes already used
            "client_pair_codes", // codes a client asked for, hash only
            "provider_health",   // providers whose credentials were refused
            "spaces",
            "space_members", // who may use which space file (ADR 0009)
        ] {
            assert!(t.iter().any(|x| x == name), "missing table {name}");
        }
        for gone in ["settings", "pair_codes", "display_defaults"] {
            assert!(!t.iter().any(|x| x == gone), "{gone} belongs to the admin service now");
        }
        for moved in ["conversations", "messages", "messages_fts", "memory", "generations"] {
            assert!(!t.iter().any(|x| x == moved), "{moved} belongs in a space file now");
        }
        assert!(!t.iter().any(|x| x == "projects"), "projects was never used and is dropped");
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
        let db = init_to(conn, 7);
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 7);
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
        let db = init_to(conn, 7);
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 7);
        let t = tables(&db);
        assert!(t.iter().any(|x| x == "clients") && !t.iter().any(|x| x == "devices"), "{t:?}"); // names-ok
        let name: String = db.conn.query_row("SELECT name FROM clients WHERE hash = 'h1'", [], |r| r.get(0)).unwrap();
        assert_eq!(name, "Work laptop", "the paired client and its token hash survive");
        let g: String = db
            .conn
            .query_row("SELECT id FROM generations WHERE client_id = 'd1' AND client_request_id = 'r1'", [], |r| r.get(0))
            .expect("the generation is found by its client id");
        assert_eq!(g, "g1");
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
        let db = init_to(conn, 7);
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, 7);
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

    #[test]
    fn v8_moves_the_history_out_and_keeps_users_clients_and_keys() {
        let conn = Connection::open_in_memory().unwrap();
        let db = init_to(conn, 7);
        db.conn
            .execute_batch(
                "INSERT INTO users (id, name, created_at) VALUES ('u1','owner',0);
                 INSERT INTO clients (id, user_id, name, hash, created_at) VALUES ('c1','u1','Laptop','h1',0);
                 INSERT INTO providers (id, api_key_enc) VALUES ('p1', x'01');
                 INSERT INTO conversations (id, user_id, title, created_at, updated_at) VALUES ('k1','u1','t',0,0);
                 INSERT INTO messages (id, conversation_id, user_id, role, content, created_at) VALUES ('m1','k1','u1','user','hi',0);
                 INSERT INTO token_usage (id, user_id, client_id, provider_id, tokens_in, tokens_out, created_at)
                     VALUES ('t1','u1','c1','p1',3,4,0);",
            )
            .unwrap();
        let mut db = db;
        db.migrate(MIGRATIONS.len()).unwrap();
        let t = tables(&db);
        assert!(["conversations", "messages", "generations", "projects", "memory"].iter().all(|x| !t.iter().any(|y| y == x)), "{t:?}");
        let n = |sql: &str| -> i64 { db.conn.query_row(sql, [], |r| r.get(0)).unwrap() };
        assert_eq!(n("SELECT COUNT(*) FROM users") + n("SELECT COUNT(*) FROM clients") + n("SELECT COUNT(*) FROM providers"), 3);
        assert_eq!(n("SELECT COUNT(*) FROM token_usage WHERE space_id IS NULL"), 1, "usage keeps its numbers, with no space");
        assert_eq!(n("SELECT COUNT(*) FROM spaces"), 0, "spaces are made as users arrive, not by the migration");
    }

    #[test]
    fn a_personal_space_has_its_owner_as_its_only_member() {
        let db = Db::open_in_memory().unwrap();
        let a = db.seed_server().unwrap();
        db.conn.execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
        let sa = db.ensure_personal_space(&a).unwrap();
        let sb = db.ensure_personal_space("B").unwrap();
        assert_ne!(sa, sb);
        assert!(crate::space::is_space_id(&sa), "a space id is a ULID: {sa}");
        assert!(db.is_member(&sa, &a).unwrap() && db.is_member(&sb, "B").unwrap());
        assert!(!db.is_member(&sa, "B").unwrap() && !db.is_member(&sb, &a).unwrap(), "nobody is in another's personal space");
        assert!(!db.is_member("no such space", &a).unwrap());
        assert_eq!(db.spaces_for(&a).unwrap(), [SpaceRow { id: sa.clone(), kind: "personal".into(), name: String::new() }]);
        assert_eq!(db.space_ids().unwrap().len(), 2);
        // A second personal space for one user is refused by the schema.
        let dup =
            db.conn.execute("INSERT INTO spaces (id, kind, name, owner_user_id, created_at) VALUES ('X', 'personal', '', ?1, 0)", [&a]);
        assert!(dup.is_err());
        assert!(
            db.conn.execute("INSERT INTO spaces (id, kind, created_at) VALUES ('Y', 'personal', 0)", []).is_err(),
            "a personal space has an owner"
        );
    }

    #[test]
    fn provider_health_flags_clears_and_keeps_the_first_time() {
        let db = Db::open_in_memory().unwrap();
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
