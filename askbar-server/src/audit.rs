//! The audit store: a separate SQLite file (`audit.db`) holding a trace of spans for every
//! generation. It has its own append-only migrations and its own `PRAGMA user_version`,
//! independent of `askbar.db`. Phase 4 stores metadata only; message content is added in
//! Phase 5, sealed.

use rusqlite::Connection;
use std::path::Path;

/// "ADIT": marks the file as an Askbar audit database, so no other SQLite file is taken for one.
const APPLICATION_ID: i64 = 0x4144_4954;

/// Append only; `PRAGMA user_version` counts the entries applied.
const MIGRATIONS: &[&str] = &[
    // v1: spans and (later sealed) payloads.
    //
    // A span is one step of a generation: `turn` (the whole reply), `gate` (a check before it
    // runs, or time spent waiting), `attempt` (one request to a provider), `tool`, `agent_step`.
    // `trace_id` is the generation id. Times are unix seconds; finer timing is in `attrs_json`.
    "CREATE TABLE spans (
         id            TEXT PRIMARY KEY,
         trace_id      TEXT    NOT NULL,
         parent_id     TEXT,
         kind          TEXT    NOT NULL CHECK (kind IN ('turn','gate','attempt','tool','agent_step')),
         name          TEXT    NOT NULL,
         status        TEXT    NOT NULL,
         started_at    INTEGER NOT NULL,
         ended_at      INTEGER,
         provider_id   TEXT,
         model         TEXT,
         tokens_in     INTEGER,
         tokens_out    INTEGER,
         tokens_cached INTEGER,
         ttft_ms       INTEGER,
         error_code    TEXT,
         user_id       TEXT,
         device_id     TEXT,
         attrs_json    TEXT
     );
     CREATE INDEX idx_spans_trace   ON spans(trace_id, started_at, id);
     CREATE INDEX idx_spans_started ON spans(started_at DESC, id DESC);
     CREATE TABLE payloads (
         span_id    TEXT    NOT NULL,
         role       TEXT    NOT NULL,
         key_id     TEXT    NOT NULL,
         ciphertext BLOB    NOT NULL,
         created_at INTEGER NOT NULL
     );
     CREATE INDEX idx_payloads_span ON payloads(span_id);",
    // v2: the store is append-only by rule, enforced by the database itself. Nothing younger than
    // 7 days is ever deleted (so not even a mistaken retention setting can remove a week of
    // evidence), and a span can only change while it is still `running`: once it has finished (or
    // been marked interrupted) it is fixed.
    "CREATE TRIGGER spans_keep_a_week BEFORE DELETE ON spans
     WHEN old.started_at > CAST(strftime('%s','now') AS INTEGER) - 7 * 86400
     BEGIN SELECT RAISE(ABORT, 'spans younger than 7 days cannot be deleted'); END;
     CREATE TRIGGER payloads_keep_a_week BEFORE DELETE ON payloads
     WHEN old.created_at > CAST(strftime('%s','now') AS INTEGER) - 7 * 86400
     BEGIN SELECT RAISE(ABORT, 'payloads younger than 7 days cannot be deleted'); END;
     CREATE TRIGGER spans_change_only_while_running BEFORE UPDATE ON spans
     WHEN old.status <> 'running'
     BEGIN SELECT RAISE(ABORT, 'a span that is no longer running cannot be changed'); END;",
];

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("audit database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("could not create {path}: {source}")]
    Io { path: String, source: std::io::Error },
    #[error(
        "unexpected audit database version: {found}, but this server writes versions up to {expected}. \
         It was made by a different Askbar version; refusing to touch it."
    )]
    UnexpectedVersion { found: i64, expected: usize },
}

pub struct AuditDb {
    conn: Connection,
}

impl AuditDb {
    pub fn open(path: &Path) -> Result<AuditDb, AuditError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AuditError::Io { path: dir.display().to_string(), source: e })?;
        }
        AuditDb::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<AuditDb, AuditError> {
        AuditDb::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<AuditDb, AuditError> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")?;
        let mut db = AuditDb { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&mut self) -> Result<(), AuditError> {
        let current: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if current != 0 {
            let app: i64 = self.conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
            if current < 0 || current as usize > MIGRATIONS.len() || app != APPLICATION_ID {
                return Err(AuditError::UnexpectedVersion { found: current, expected: MIGRATIONS.len() });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(db: &AuditDb) -> Vec<String> {
        let mut stmt = db.conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    fn columns(db: &AuditDb, table: &str) -> Vec<String> {
        let mut stmt = db.conn.prepare(&format!("SELECT name FROM pragma_table_info('{table}') ORDER BY cid")).unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn open_creates_the_schema() {
        let db = AuditDb::open_in_memory().unwrap();
        assert_eq!(tables(&db), ["payloads", "spans"]);
        assert_eq!(
            columns(&db, "spans"),
            [
                "id", "trace_id", "parent_id", "kind", "name", "status", "started_at", "ended_at", "provider_id", "model",
                "tokens_in", "tokens_out", "tokens_cached", "ttft_ms", "error_code", "user_id", "device_id", "attrs_json"
            ]
        );
        assert_eq!(columns(&db, "payloads"), ["span_id", "role", "key_id", "ciphertext", "created_at"]);
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        let app: i64 = db.conn.query_row("PRAGMA application_id", [], |r| r.get(0)).unwrap();
        assert_eq!(app, APPLICATION_ID);
    }

    #[test]
    fn reopening_is_idempotent_and_keeps_the_rows() {
        let dir = std::env::temp_dir().join(format!("askbar-audit-open-{}", ulid::Ulid::new()));
        let path = dir.join("nested").join("audit.db");
        {
            let db = AuditDb::open(&path).unwrap();
            db.conn.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('s','t','turn','chat','done',1)", []).unwrap();
        }
        let db = AuditDb::open(&path).unwrap();
        let n: i64 = db.conn.query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
        let v: i64 = db.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len(), "migrations ran once");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
    }

    fn span(db: &AuditDb, id: &str, status: &str, started_at: i64) {
        db.conn
            .execute(
                "INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t','turn','chat',?2,?3)",
                rusqlite::params![id, status, started_at],
            )
            .unwrap();
    }

    #[test]
    fn spans_and_payloads_younger_than_seven_days_cannot_be_deleted() {
        let db = AuditDb::open_in_memory().unwrap();
        let n = now();
        span(&db, "young", "done", n - 6 * 86400);
        span(&db, "old", "done", n - 8 * 86400);
        let e = db.conn.execute("DELETE FROM spans WHERE id = 'young'", []).unwrap_err();
        assert!(e.to_string().contains("7 days"), "{e}");
        assert!(db.conn.execute("DELETE FROM spans", []).is_err(), "a bulk delete that includes a young span fails whole");
        assert_eq!(db.conn.execute("DELETE FROM spans WHERE id = 'old'", []).unwrap(), 1);

        for (id, created) in [("young", n), ("old", n - 9 * 86400)] {
            db.conn.execute("INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,'request','k',x'00',?2)", rusqlite::params![id, created]).unwrap();
        }
        assert!(db.conn.execute("DELETE FROM payloads WHERE span_id = 'young'", []).unwrap_err().to_string().contains("7 days"));
        assert_eq!(db.conn.execute("DELETE FROM payloads WHERE span_id = 'old'", []).unwrap(), 1);
        let left: i64 = db.conn.query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn a_span_can_change_only_while_it_is_running() {
        let db = AuditDb::open_in_memory().unwrap();
        span(&db, "s", "running", now());
        // Allowed: a running span is finished...
        assert_eq!(db.conn.execute("UPDATE spans SET status = 'done', ended_at = 5 WHERE id = 's'", []).unwrap(), 1);
        // ...and then it is fixed, whatever the change.
        for sql in [
            "UPDATE spans SET status = 'error' WHERE id = 's'",
            "UPDATE spans SET tokens_in = 9 WHERE id = 's'",
            "UPDATE spans SET status = 'running' WHERE id = 's'",
        ] {
            let e = db.conn.execute(sql, []).unwrap_err();
            assert!(e.to_string().contains("no longer running"), "{sql}: {e}");
        }
        // Marking a leftover running span interrupted is the same allowed path.
        span(&db, "left-over", "running", now());
        assert_eq!(db.conn.execute("UPDATE spans SET status = 'interrupted' WHERE status = 'running'", []).unwrap(), 1);
        let status: String = db.conn.query_row("SELECT status FROM spans WHERE id = 's'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "done");
    }

    #[test]
    fn a_span_kind_outside_the_fixed_set_is_refused() {
        let db = AuditDb::open_in_memory().unwrap();
        for kind in ["turn", "gate", "attempt", "tool", "agent_step"] {
            db.conn.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t',?1,'n','done',1)", [kind]).unwrap();
        }
        assert!(db.conn.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('x','t','banana','n','done',1)", []).is_err());
    }

    #[test]
    fn another_programs_database_or_a_newer_one_is_refused() {
        let dir = std::env::temp_dir().join(format!("askbar-audit-ver-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        let newer = dir.join("newer.db");
        {
            let db = AuditDb::open(&newer).unwrap();
            db.conn.execute_batch("PRAGMA user_version = 99").unwrap();
        }
        assert!(matches!(AuditDb::open(&newer), Err(AuditError::UnexpectedVersion { found: 99, .. })));
        let foreign = dir.join("foreign.db");
        Connection::open(&foreign).unwrap().execute_batch("CREATE TABLE t (x); PRAGMA user_version = 1;").unwrap();
        assert!(matches!(AuditDb::open(&foreign), Err(AuditError::UnexpectedVersion { found: 1, .. })));
        let _ = std::fs::remove_dir_all(dir);
    }
}
