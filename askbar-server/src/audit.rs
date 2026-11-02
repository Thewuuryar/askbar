//! The audit store: a separate SQLite file (`audit.db`) holding a trace of spans for every
//! generation. It has its own append-only migrations and its own `PRAGMA user_version`,
//! independent of `askbar.db`. Phase 4 stores metadata only; message content is added in
//! Phase 5, sealed.

use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::{mpsc, oneshot};

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

    /// At start-up: no task from the previous run exists, so every span still `running` was cut
    /// off. Marking it `interrupted` is the one change the triggers allow to a running span.
    /// Returns how many there were.
    pub fn sweep_running(&self) -> Result<usize, AuditError> {
        Ok(self.conn.execute(
            "UPDATE spans SET status = 'interrupted', ended_at = ?1, error_code = COALESCE(error_code, 'interrupted') WHERE status = 'running'",
            [now_secs()],
        )?)
    }
}

// --------------------------------------------------------------------- recording

/// A span that has begun. It is written as `running` and finished later by `SpanFinish`.
#[derive(Debug, Clone, Default)]
pub struct SpanStart {
    pub id: String,
    pub trace_id: String,
    pub parent_id: Option<String>,
    /// turn, gate, attempt, tool or agent_step.
    pub kind: String,
    pub name: String,
    pub started_at: i64,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub user_id: Option<String>,
    pub device_id: Option<String>,
    pub attrs_json: Option<String>,
}

/// How a span ended. Fields left `None` keep what the start recorded.
#[derive(Debug, Clone, Default)]
pub struct SpanFinish {
    pub id: String,
    pub status: String,
    pub ended_at: i64,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub tokens_cached: Option<u64>,
    pub ttft_ms: Option<u64>,
    pub error_code: Option<String>,
    pub attrs_json: Option<String>,
}

/// What a span is. The set is fixed (the database checks it too).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A whole generation, from start to finish.
    Turn,
    /// A check before it runs, or time spent waiting.
    Gate,
    /// One request to one provider.
    Attempt,
    Tool,
    AgentStep,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Turn => "turn",
            Kind::Gate => "gate",
            Kind::Attempt => "attempt",
            Kind::Tool => "tool",
            Kind::AgentStep => "agent_step",
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl SpanStart {
    /// A span in trace `trace_id` (the generation id), starting now, with a fresh id.
    pub fn new(trace_id: &str, kind: Kind, name: &str) -> SpanStart {
        SpanStart {
            id: crate::ids::new_id(),
            trace_id: trace_id.to_string(),
            kind: kind.as_str().to_string(),
            name: name.to_string(),
            started_at: now_secs(),
            ..Default::default()
        }
    }

    pub fn parent(mut self, parent_id: &str) -> SpanStart {
        self.parent_id = Some(parent_id.to_string());
        self
    }

    pub fn provider(mut self, provider_id: &str, model: &str) -> SpanStart {
        self.provider_id = Some(provider_id.to_string());
        self.model = Some(model.to_string());
        self
    }

    pub fn who(mut self, user_id: &str, device_id: Option<&str>) -> SpanStart {
        self.user_id = Some(user_id.to_string());
        self.device_id = device_id.map(str::to_string);
        self
    }

    pub fn attrs(mut self, attrs: &serde_json::Value) -> SpanStart {
        self.attrs_json = Some(attrs.to_string());
        self
    }
}

impl SpanFinish {
    /// Ends span `id` now with `status`.
    pub fn new(id: &str, status: &str) -> SpanFinish {
        SpanFinish { id: id.to_string(), status: status.to_string(), ended_at: now_secs(), ..Default::default() }
    }

    pub fn error_code(mut self, code: &str) -> SpanFinish {
        self.error_code = Some(code.to_string());
        self
    }
}

enum Event {
    Start(SpanStart),
    Finish(SpanFinish),
    /// Answered once everything sent before it has been written.
    Flush(oneshot::Sender<()>),
    /// Delete spans and payloads that started before `cutoff`; answered with how many of each.
    Prune { cutoff: i64, done: oneshot::Sender<Result<(usize, usize), String>> },
}

/// Records are never kept for less than this many days, whatever the setting says.
pub const MIN_RETENTION_DAYS: u32 = 7;

/// The moment before which records are due to go: `retention_days` before `now`, but never
/// closer than 7 days to the real clock. The database enforces the same floor with a trigger;
/// clamping here keeps one young span from making the whole delete fail.
pub fn retention_cutoff(now: i64, retention_days: u32) -> i64 {
    let by_setting = now - i64::from(retention_days.max(MIN_RETENTION_DAYS)) * 86_400;
    by_setting.min(now_secs() - i64::from(MIN_RETENTION_DAYS) * 86_400)
}

/// Lets a test hold the writer still, to fill the queue.
#[derive(Default)]
struct Pause {
    paused: Mutex<bool>,
    changed: Condvar,
}

/// The handle the rest of the server records through. Cheap to clone. Recording never waits:
/// events go into a bounded queue, one writer thread owns the connection and drains it, and
/// when the queue is full the event is dropped and `healthy()` says so (new generations are
/// then refused, see 4.10).
#[derive(Clone)]
pub struct Recorder {
    tx: Option<mpsc::Sender<Event>>,
    pause: Arc<Pause>,
    dropped: Arc<AtomicU64>,
}

impl Recorder {
    /// Records nothing. The default, so a server without an audit store behaves as before.
    pub fn noop() -> Recorder {
        Recorder { tx: None, pause: Arc::default(), dropped: Arc::default() }
    }

    /// Takes ownership of `db` and writes to it from one thread. `capacity` is the queue length.
    pub fn spawn(db: AuditDb, capacity: usize) -> Recorder {
        let (tx, mut rx) = mpsc::channel::<Event>(capacity.max(1));
        let pause = Arc::new(Pause::default());
        let writer_pause = pause.clone();
        std::thread::Builder::new()
            .name("askbar-audit-writer".into())
            .spawn(move || {
                while let Some(event) = rx.blocking_recv() {
                    let mut paused = writer_pause.paused.lock().unwrap_or_else(|e| e.into_inner());
                    while *paused {
                        paused = writer_pause.changed.wait(paused).unwrap_or_else(|e| e.into_inner());
                    }
                    drop(paused);
                    match event {
                        Event::Start(s) => {
                            if let Err(e) = write_start(db.conn(), &s) {
                                tracing::warn!("audit: could not write a span: {e}");
                            }
                        }
                        Event::Finish(f) => {
                            if let Err(e) = write_finish(db.conn(), &f) {
                                tracing::warn!("audit: could not finish a span: {e}");
                            }
                        }
                        Event::Flush(done) => {
                            let _ = done.send(());
                        }
                        Event::Prune { cutoff, done } => {
                            let _ = done.send(prune(db.conn(), cutoff).map_err(|e| e.to_string()));
                        }
                    }
                }
            })
            .expect("audit writer thread");
        Recorder { tx: Some(tx), pause, dropped: Arc::default() }
    }

    /// False while the queue is full, i.e. the writer is not keeping up.
    pub fn healthy(&self) -> bool {
        self.tx.as_ref().is_none_or(|tx| tx.capacity() > 0)
    }

    /// How many events were dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn send(&self, event: Event) {
        let Some(tx) = &self.tx else { return };
        if tx.try_send(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records that a span has begun and returns its id, for its children and for `finish_span`.
    pub fn start_span(&self, span: SpanStart) -> String {
        let id = span.id.clone();
        self.send(Event::Start(span));
        id
    }

    /// Records how a span ended. Only the first finish of a span counts.
    pub fn finish_span(&self, span: SpanFinish) {
        self.send(Event::Finish(span));
    }

    /// Waits until everything recorded so far has been written. For shutdown and tests.
    pub async fn flush(&self) {
        let Some(tx) = &self.tx else { return };
        let (done, wait) = oneshot::channel();
        if tx.send(Event::Flush(done)).await.is_ok() {
            let _ = wait.await;
        }
    }

    /// Deletes what is older than `retention_days` before `now` (see `retention_cutoff`). `now`
    /// is passed in so tests can cross the boundary without waiting. Runs on the writer's
    /// connection, in order with everything else. Returns (spans, payloads) deleted.
    pub async fn prune(&self, retention_days: u32, now: i64) -> Result<(usize, usize), String> {
        let Some(tx) = &self.tx else { return Ok((0, 0)) };
        let (done, wait) = oneshot::channel();
        tx.send(Event::Prune { cutoff: retention_cutoff(now, retention_days), done }).await.map_err(|_| "the audit writer has stopped".to_string())?;
        wait.await.map_err(|_| "the audit writer has stopped".to_string())?
    }

    /// Tests only: hold the writer still (or let it go on), to fill the queue.
    #[doc(hidden)]
    pub fn pause_writer(&self, paused: bool) {
        *self.pause.paused.lock().unwrap_or_else(|e| e.into_inner()) = paused;
        self.pause.changed.notify_all();
    }
}

/// Payloads first (they belong to spans), then the spans. Only finished spans: one still running
/// is never old enough to matter, and the sweep handles the strays.
fn prune(conn: &Connection, cutoff: i64) -> rusqlite::Result<(usize, usize)> {
    let payloads = conn.execute("DELETE FROM payloads WHERE created_at < ?1", [cutoff])?;
    let spans = conn.execute("DELETE FROM spans WHERE started_at < ?1 AND status <> 'running'", [cutoff])?;
    Ok((spans, payloads))
}

fn write_start(conn: &Connection, s: &SpanStart) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO spans (id, trace_id, parent_id, kind, name, status, started_at, provider_id, model, user_id, device_id, attrs_json)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?8, ?9, ?10, ?11)",
        rusqlite::params![s.id, s.trace_id, s.parent_id, s.kind, s.name, s.started_at, s.provider_id, s.model, s.user_id, s.device_id, s.attrs_json],
    )?;
    Ok(())
}

fn write_finish(conn: &Connection, f: &SpanFinish) -> rusqlite::Result<()> {
    // `AND status = 'running'`: a span finishes once; a second finish changes nothing.
    conn.execute(
        "UPDATE spans SET status = ?2, ended_at = ?3,
             provider_id = COALESCE(?4, provider_id), model = COALESCE(?5, model),
             tokens_in = ?6, tokens_out = ?7, tokens_cached = ?8, ttft_ms = ?9,
             error_code = ?10, attrs_json = COALESCE(?11, attrs_json)
         WHERE id = ?1 AND status = 'running'",
        rusqlite::params![
            f.id,
            f.status,
            f.ended_at,
            f.provider_id,
            f.model,
            f.tokens_in.map(|n| n as i64),
            f.tokens_out.map(|n| n as i64),
            f.tokens_cached.map(|n| n as i64),
            f.ttft_ms.map(|n| n as i64),
            f.error_code,
            f.attrs_json
        ],
    )?;
    Ok(())
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
    fn the_sweep_interrupts_only_running_spans_and_only_once() {
        let db = AuditDb::open_in_memory().unwrap();
        span(&db, "a", "running", now());
        span(&db, "b", "running", now());
        span(&db, "c", "done", now());
        assert_eq!(db.sweep_running().unwrap(), 2);
        let status = |id: &str| -> (String, Option<i64>, Option<String>) {
            db.conn.query_row("SELECT status, ended_at, error_code FROM spans WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
        };
        let (s, ended, code) = status("a");
        assert_eq!((s.as_str(), code.as_deref()), ("interrupted", Some("interrupted")));
        assert!(ended.is_some());
        assert_eq!(status("c"), ("done".into(), None, None), "finished spans are untouched");
        assert_eq!(db.sweep_running().unwrap(), 0);
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

    fn temp_db() -> (std::path::PathBuf, AuditDb) {
        let dir = std::env::temp_dir().join(format!("askbar-audit-rec-{}", ulid::Ulid::new()));
        let db = AuditDb::open(&dir.join("audit.db")).unwrap();
        (dir, db)
    }

    fn start(id: &str, trace: &str) -> SpanStart {
        SpanStart { id: id.into(), trace_id: trace.into(), kind: "turn".into(), name: "chat".into(), started_at: 100, ..Default::default() }
    }

    #[tokio::test]
    async fn events_are_written_in_the_order_they_were_recorded() {
        let (dir, db) = temp_db();
        let rec = Recorder::spawn(db, 1024);
        for i in 0..200 {
            rec.start_span(start(&format!("s{i:03}"), "t"));
            if i % 2 == 0 {
                rec.finish_span(SpanFinish { id: format!("s{i:03}"), status: "done".into(), ended_at: 101, tokens_in: Some(i), ..Default::default() });
            }
        }
        rec.flush().await;
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        let ids: Vec<String> = conn.prepare("SELECT id FROM spans ORDER BY rowid").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(ids, (0..200).map(|i| format!("s{i:03}")).collect::<Vec<_>>());
        let done: i64 = conn.query_row("SELECT COUNT(*) FROM spans WHERE status = 'done' AND tokens_in IS NOT NULL", [], |r| r.get(0)).unwrap();
        assert_eq!(done, 100);
        let running: i64 = conn.query_row("SELECT COUNT(*) FROM spans WHERE status = 'running'", [], |r| r.get(0)).unwrap();
        assert_eq!(running, 100, "odd ones were never finished");
        assert_eq!(rec.dropped(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_full_queue_makes_it_unhealthy_and_it_recovers_when_the_writer_catches_up() {
        let (dir, db) = temp_db();
        let rec = Recorder::spawn(db, 4);
        assert!(rec.healthy());
        rec.pause_writer(true);
        // The writer takes one event and holds it; four more fill the queue.
        for i in 0..8 {
            rec.start_span(start(&format!("s{i}"), "t"));
            if i == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await; // let it pick the first up
            }
        }
        assert!(!rec.healthy(), "the queue is full");
        assert!(rec.dropped() >= 1, "what did not fit was dropped, and counted");
        rec.pause_writer(false);
        rec.flush().await;
        assert!(rec.healthy(), "drained");
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0)).unwrap();
        assert_eq!(n as u64 + rec.dropped(), 8, "every event was either written or counted as dropped");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn spans_link_to_their_parent_share_a_trace_and_a_second_finish_is_ignored() {
        let (dir, db) = temp_db();
        let rec = Recorder::spawn(db, 64);
        let turn = rec.start_span(SpanStart::new("gen-1", Kind::Turn, "chat").who("u1", Some("d1")));
        let attempt = rec.start_span(SpanStart::new("gen-1", Kind::Attempt, "openai").parent(&turn).provider("p1", "m1"));
        let gate = rec.start_span(SpanStart::new("gen-1", Kind::Gate, "queue").parent(&turn));
        assert!(turn < attempt && attempt < gate, "ids come from ids.rs and sort in creation order");
        rec.finish_span(SpanFinish { tokens_in: Some(5), ..SpanFinish::new(&attempt, "done") });
        rec.finish_span(SpanFinish::new(&attempt, "error").error_code("late")); // ignored
        rec.finish_span(SpanFinish::new(&gate, "done"));
        rec.finish_span(SpanFinish::new(&turn, "done"));
        rec.flush().await;

        let conn = Connection::open(dir.join("audit.db")).unwrap();
        let row = |id: &str| -> (String, Option<String>, String, String, Option<i64>, Option<String>) {
            conn.query_row("SELECT trace_id, parent_id, kind, status, tokens_in, error_code FROM spans WHERE id = ?1", [id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
            })
            .unwrap()
        };
        assert_eq!(row(&turn), ("gen-1".into(), None, "turn".into(), "done".into(), None, None));
        assert_eq!(row(&attempt), ("gen-1".into(), Some(turn.clone()), "attempt".into(), "done".into(), Some(5), None), "the first finish stands");
        assert_eq!(row(&gate).1.as_deref(), Some(turn.as_str()));
        let (user, device, provider, model): (String, String, String, String) = (
            conn.query_row("SELECT user_id FROM spans WHERE id = ?1", [&turn], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT device_id FROM spans WHERE id = ?1", [&turn], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT provider_id FROM spans WHERE id = ?1", [&attempt], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT model FROM spans WHERE id = ?1", [&attempt], |r| r.get(0)).unwrap(),
        );
        assert_eq!((user.as_str(), device.as_str(), provider.as_str(), model.as_str()), ("u1", "d1", "p1", "m1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_five_span_kinds_match_what_the_database_accepts() {
        let db = AuditDb::open_in_memory().unwrap();
        for k in [Kind::Turn, Kind::Gate, Kind::Attempt, Kind::Tool, Kind::AgentStep] {
            db.conn.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t',?2,'n','done',1)", rusqlite::params![k.as_str(), k.as_str()]).unwrap();
        }
    }

    #[test]
    fn the_cutoff_follows_the_setting_never_closer_than_a_week_to_the_real_clock() {
        let real = now_secs();
        assert_eq!(retention_cutoff(real, 30), real - 30 * 86_400);
        assert_eq!(retention_cutoff(real, 7), real - 7 * 86_400);
        assert_eq!(retention_cutoff(real, 1), real - 7 * 86_400, "below the floor, the floor applies");
        assert_eq!(retention_cutoff(real, 0), real - 7 * 86_400);
        // A clock far in the future would ask for more than the database allows: clamped.
        assert_eq!(retention_cutoff(real + 100 * 86_400, 7), real - 7 * 86_400);
        // A clock in the past is not clamped further.
        assert_eq!(retention_cutoff(real - 100 * 86_400, 30), real - 130 * 86_400);
    }

    #[tokio::test]
    async fn retention_deletes_across_the_boundary_and_the_seven_day_rule_still_holds() {
        let (dir, db) = temp_db();
        let rec = Recorder::spawn(db, 64);
        let real = now_secs();
        let day = 86_400;
        // Spans of ages 40, 29.5, 10, 3 and 0 days, each with a payload of the same age.
        let ages = [("a40", 40 * day), ("a29", 29 * day + day / 2), ("a10", 10 * day), ("a3", 3 * day), ("a0", 0)];
        {
            let conn = Connection::open(dir.join("audit.db")).unwrap();
            for (id, age) in ages {
                conn.execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at) VALUES (?1,'t','turn','c','done',?2,?2)", rusqlite::params![id, real - age]).unwrap();
                conn.execute("INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,'reply','k',x'00',?2)", rusqlite::params![id, real - age]).unwrap();
            }
        }
        let left = |dir: &std::path::Path| -> Vec<String> {
            let conn = Connection::open(dir.join("audit.db")).unwrap();
            let mut stmt = conn.prepare("SELECT id FROM spans ORDER BY started_at").unwrap();
            let ids = stmt.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
            ids
        };

        // 30 days of retention: only the 40-day-old goes (spans and payloads).
        assert_eq!(rec.prune(30, real).await.unwrap(), (1, 1));
        assert_eq!(left(&dir), ["a29", "a10", "a3", "a0"]);
        // Two days later by the injected clock, the 29.5-day-old is past 30 days too.
        assert_eq!(rec.prune(30, real + 2 * day).await.unwrap(), (1, 1));
        assert_eq!(left(&dir), ["a10", "a3", "a0"]);
        // The shortest setting is 7 days, even if asked for 1: the 3-day-old and the new one stay.
        assert_eq!(rec.prune(1, real).await.unwrap(), (1, 1));
        assert_eq!(left(&dir), ["a3", "a0"]);
        // And a clock jumping far ahead cannot make it delete what is under a week old by the real
        // clock: the database's own rule holds as well.
        assert_eq!(rec.prune(7, real + 400 * day).await.unwrap(), (0, 0));
        assert_eq!(left(&dir), ["a3", "a0"]);
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        assert!(conn.execute("DELETE FROM spans WHERE id = 'a3'", []).unwrap_err().to_string().contains("7 days"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_noop_recorder_is_always_healthy_and_records_nothing() {
        let rec = Recorder::noop();
        rec.start_span(start("s", "t"));
        rec.flush().await;
        assert!(rec.healthy());
        assert_eq!(rec.dropped(), 0);
    }
}
