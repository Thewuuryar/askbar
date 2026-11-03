//! The admin API key: how a script (rather than a person in a browser) uses the admin API.
//!
//! One key is active at a time. It is shown once, when it is made; only its SHA-256 is kept, with a
//! name, a scope and when it was last used. Revoking it keeps the row as a record and lets a new
//! key be made. A key can do everything its scope allows except manage keys, change the password or
//! open a session (those need the person's own session), so a leaked key cannot mint more keys or
//! lock the administrator out.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// What every key starts with, so one can be recognised (and a secret scanner can find it).
pub const PREFIX: &str = "askbar-admin-";
pub const MAX_NAME_CHARS: usize = 60;
/// `last_used_at` is written at most this often.
pub const TOUCH_EVERY_SECS: i64 = 60;

/// What a key may do. Only `Admin` exists; the check goes through `allows`, so a narrower scope
/// (for example audit logs only) is a new variant and a new arm there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Admin,
}

/// What kind of route is being asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteClass {
    /// Any other API route.
    Api,
    /// Routes that need the person's own session: the keys, the password, the session itself.
    SessionOnly,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Admin => "admin",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        match s {
            "admin" => Some(Scope::Admin),
            _ => None,
        }
    }

    pub fn allows(self, class: RouteClass) -> bool {
        match (self, class) {
            (Scope::Admin, RouteClass::Api) => true,
            (Scope::Admin, RouteClass::SessionOnly) => false,
        }
    }
}

/// A key as it is listed afterwards: never the key, never its hash.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct KeyRow {
    pub id: String,
    pub name: String,
    pub scope: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CreateError {
    /// An active key already exists; revoke it first.
    Exists,
    /// The name is empty or too long.
    BadName,
    Db(String),
}

pub fn generate() -> String {
    let mut b = [0u8; 32];
    OsRng.fill_bytes(&mut b);
    format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(b))
}

pub fn hash(key: &str) -> String {
    Sha256::digest(key.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Makes the key. Returns its row and the key itself, which the caller shows once and forgets.
pub fn create(conn: &Connection, name: &str, now: i64) -> Result<(KeyRow, String), CreateError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(CreateError::BadName);
    }
    let (id, key) = (ulid::Ulid::new().to_string(), generate());
    match conn.execute(
        "INSERT INTO admin_api_keys (id, name, hash, scope, created_at) VALUES (?1, ?2, ?3, 'admin', ?4)",
        params![id, name, hash(&key), now],
    ) {
        Ok(_) => Ok((KeyRow { id, name: name.into(), scope: "admin".into(), created_at: now, last_used_at: None }, key)),
        Err(rusqlite::Error::SqliteFailure(f, _)) if f.code == rusqlite::ErrorCode::ConstraintViolation => Err(CreateError::Exists),
        Err(e) => Err(CreateError::Db(e.to_string())),
    }
}

/// The keys that can still be used (at most one).
pub fn list(conn: &Connection) -> rusqlite::Result<Vec<KeyRow>> {
    let mut stmt = conn.prepare("SELECT id, name, scope, created_at, last_used_at FROM admin_api_keys WHERE revoked_at IS NULL ORDER BY created_at, id")?;
    let rows = stmt.query_map([], |r| Ok(KeyRow { id: r.get(0)?, name: r.get(1)?, scope: r.get(2)?, created_at: r.get(3)?, last_used_at: r.get(4)? }))?;
    rows.collect()
}

/// Revokes the key `id`. False if there is no such usable key.
pub fn revoke(conn: &Connection, id: &str, now: i64) -> rusqlite::Result<bool> {
    Ok(conn.execute("UPDATE admin_api_keys SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL", params![id, now])? == 1)
}

/// The scope of `key` if it is an active key, and note that it was just used (at most once a minute).
pub fn authenticate(conn: &Connection, key: &str, now: i64) -> rusqlite::Result<Option<Scope>> {
    if !key.starts_with(PREFIX) {
        return Ok(None);
    }
    let found: Option<(String, String, Option<i64>)> = conn
        .query_row("SELECT id, scope, last_used_at FROM admin_api_keys WHERE hash = ?1 AND revoked_at IS NULL", [hash(key)], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((id, scope, last)) = found else { return Ok(None) };
    if last.is_none_or(|t| now - t >= TOUCH_EVERY_SECS) {
        conn.execute("UPDATE admin_api_keys SET last_used_at = ?2 WHERE id = ?1", params![id, now])?;
    }
    Ok(Scope::parse(&scope))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AdminDb;

    fn db() -> AdminDb {
        AdminDb::open_in_memory().unwrap()
    }

    #[test]
    fn a_key_has_the_prefix_enough_randomness_and_is_stored_only_as_a_hash() {
        let a = generate();
        assert!(a.starts_with(PREFIX) && a.len() >= PREFIX.len() + 43, "{a}");
        assert_ne!(a, generate());
        assert_eq!(hash(&a).len(), 64);
        assert_ne!(hash(&a), a);
        let d = db();
        let (row, key) = create(d.conn(), "  build server ", 100).unwrap();
        assert_eq!((row.name.as_str(), row.scope.as_str(), row.created_at, row.last_used_at), ("build server", "admin", 100, None));
        // Nothing in the table holds the key.
        let (stored_hash, dump): (String, String) = d
            .conn()
            .query_row("SELECT hash, id || name || hash || scope FROM admin_api_keys", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(stored_hash, hash(&key));
        assert!(!dump.contains(&key));
    }

    #[test]
    fn the_listing_shows_a_name_and_times_never_the_key() {
        let d = db();
        let (row, key) = create(d.conn(), "ci", 5).unwrap();
        let listed = list(d.conn()).unwrap();
        assert_eq!(listed, [row]);
        let json = serde_json::to_string(&listed).unwrap();
        assert!(!json.contains(&key) && !json.contains(&hash(&key)) && !json.contains("hash"), "{json}");
    }

    #[test]
    fn only_one_key_is_active_until_it_is_revoked() {
        let d = db();
        let (first, _) = create(d.conn(), "one", 1).unwrap();
        assert_eq!(create(d.conn(), "two", 2).unwrap_err(), CreateError::Exists);
        assert!(!revoke(d.conn(), "no-such-id", 3).unwrap());
        assert!(revoke(d.conn(), &first.id, 3).unwrap());
        assert!(!revoke(d.conn(), &first.id, 4).unwrap(), "revoking twice finds nothing");
        assert!(list(d.conn()).unwrap().is_empty());
        let (second, _) = create(d.conn(), "two", 5).unwrap();
        assert_eq!(list(d.conn()).unwrap(), [second]);
        let n: i64 = d.conn().query_row("SELECT COUNT(*) FROM admin_api_keys", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2, "the revoked row stays as a record");
    }

    #[test]
    fn names_must_be_one_to_sixty_characters() {
        let d = db();
        for bad in ["", "   ", &"x".repeat(61)] {
            assert_eq!(create(d.conn(), bad, 1).unwrap_err(), CreateError::BadName, "{bad:?}");
        }
        assert!(create(d.conn(), &"é".repeat(60), 1).is_ok());
    }

    #[test]
    fn only_an_active_key_authenticates_and_use_is_noted_once_a_minute() {
        let d = db();
        let (row, key) = create(d.conn(), "ci", 1000).unwrap();
        assert_eq!(authenticate(d.conn(), &key, 1000).unwrap(), Some(Scope::Admin));
        assert_eq!(authenticate(d.conn(), "askbar-admin-nope", 1000).unwrap(), None);
        assert_eq!(authenticate(d.conn(), &hash(&key), 1000).unwrap(), None, "the hash is not a credential");
        assert_eq!(authenticate(d.conn(), "something else", 1000).unwrap(), None);
        let used = |d: &AdminDb| d.conn().query_row("SELECT last_used_at FROM admin_api_keys", [], |r| r.get::<_, Option<i64>>(0)).unwrap();
        assert_eq!(used(&d), Some(1000));
        authenticate(d.conn(), &key, 1030).unwrap();
        assert_eq!(used(&d), Some(1000), "not again within a minute");
        authenticate(d.conn(), &key, 1061).unwrap();
        assert_eq!(used(&d), Some(1061));
        revoke(d.conn(), &row.id, 1100).unwrap();
        assert_eq!(authenticate(d.conn(), &key, 1200).unwrap(), None, "a revoked key opens nothing");
    }

    #[test]
    fn the_admin_scope_cannot_manage_keys_or_sessions() {
        assert!(Scope::Admin.allows(RouteClass::Api));
        assert!(!Scope::Admin.allows(RouteClass::SessionOnly));
        assert_eq!(Scope::parse(Scope::Admin.as_str()), Some(Scope::Admin));
        assert_eq!(Scope::parse("audit:read"), None, "an unknown scope is no scope");
    }

    #[test]
    fn a_database_from_before_the_keys_upgrades_and_keeps_its_rows() {
        let conn = Connection::open_in_memory().unwrap();
        for (i, sql) in crate::db::migrations().iter().take(7).enumerate() {
            conn.execute_batch(sql).unwrap();
            if i == 0 {
                conn.execute_batch(&format!("PRAGMA application_id = {}", crate::db::application_id())).unwrap();
            }
        }
        conn.execute_batch("PRAGMA user_version = 7").unwrap();
        conn.execute_batch("INSERT INTO revoked_clients (client_id, revoked_at) VALUES ('d1', 5);").unwrap();
        let db = AdminDb::init_for_test(conn).unwrap();
        let v: i64 = db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, crate::db::migrations().len());
        assert_eq!(db.conn().query_row("SELECT COUNT(*) FROM revoked_clients", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        create(db.conn(), "ci", 1).unwrap();
    }
}
