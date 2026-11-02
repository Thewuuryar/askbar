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
    /// Where the file is, so readers can open their own connection (None for in-memory).
    path: Option<std::path::PathBuf>,
}

impl AuditDb {
    pub fn open(path: &Path) -> Result<AuditDb, AuditError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AuditError::Io { path: dir.display().to_string(), source: e })?;
        }
        AuditDb::init(Connection::open(path)?, Some(path.to_path_buf()))
    }

    pub fn open_in_memory() -> Result<AuditDb, AuditError> {
        AuditDb::init(Connection::open_in_memory()?, None)
    }

    fn init(conn: Connection, path: Option<std::path::PathBuf>) -> Result<AuditDb, AuditError> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;")?;
        let mut db = AuditDb { conn, path };
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

// ------------------------------------------------------------------------ reading

/// What `list_traces` can filter on. Everything here is metadata: no message content is ever
/// stored in `spans`, and none of these functions reads `payloads`.
#[derive(Debug, Default, Clone)]
pub struct TraceFilter {
    pub status: Option<String>,
    pub provider: Option<String>,
    /// Only traces older than this trace id (ids sort by time): the cursor for "load more".
    pub before: Option<String>,
    pub limit: Option<usize>,
}

pub const DEFAULT_TRACE_LIMIT: usize = 50;
pub const MAX_TRACE_LIMIT: usize = 200;

/// One row per generation (its turn span), newest first, with the provider and timings of its
/// last attempt.
pub fn list_traces(conn: &Connection, f: &TraceFilter) -> rusqlite::Result<Vec<serde_json::Value>> {
    let limit = f.limit.unwrap_or(DEFAULT_TRACE_LIMIT).clamp(1, MAX_TRACE_LIMIT);
    let mut stmt = conn.prepare(
        "SELECT t.trace_id, t.id, t.status, t.started_at, t.ended_at, t.user_id, t.device_id,
                t.tokens_in, t.tokens_out, t.tokens_cached, t.error_code,
                a.provider_id, a.model, a.ttft_ms, json_extract(a.attrs_json, '$.latency_ms')
         FROM spans t
         LEFT JOIN spans a ON a.id = (SELECT id FROM spans WHERE trace_id = t.trace_id AND kind = 'attempt' ORDER BY id DESC LIMIT 1)
         WHERE t.kind = 'turn'
           AND (?1 IS NULL OR t.status = ?1)
           AND (?2 IS NULL OR a.provider_id = ?2)
           AND (?3 IS NULL OR t.trace_id < ?3)
         ORDER BY t.trace_id DESC LIMIT ?4",
    )?;
    let rows = stmt.query_map(rusqlite::params![f.status, f.provider, f.before, limit as i64], |r| {
        Ok(serde_json::json!({
            "trace_id": r.get::<_, String>(0)?,
            "span_id": r.get::<_, String>(1)?,
            "status": r.get::<_, String>(2)?,
            "started_at": r.get::<_, i64>(3)?,
            "ended_at": r.get::<_, Option<i64>>(4)?,
            "user_id": r.get::<_, Option<String>>(5)?,
            "device_id": r.get::<_, Option<String>>(6)?,
            "tokens_in": r.get::<_, Option<i64>>(7)?,
            "tokens_out": r.get::<_, Option<i64>>(8)?,
            "tokens_cached": r.get::<_, Option<i64>>(9)?,
            "error_code": r.get::<_, Option<String>>(10)?,
            "provider_id": r.get::<_, Option<String>>(11)?,
            "model": r.get::<_, Option<String>>(12)?,
            "ttft_ms": r.get::<_, Option<i64>>(13)?,
            "latency_ms": r.get::<_, Option<i64>>(14)?,
        }))
    })?;
    rows.collect()
}

/// Every span of one generation, in the order they began. Empty if there is no such trace.
pub fn trace_spans(conn: &Connection, trace_id: &str) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, parent_id, kind, name, status, started_at, ended_at, provider_id, model, tokens_in, tokens_out,
                tokens_cached, ttft_ms, error_code, user_id, device_id, attrs_json
         FROM spans WHERE trace_id = ?1 ORDER BY started_at, id",
    )?;
    let rows = stmt.query_map([trace_id], |r| {
        let attrs: Option<String> = r.get(16)?;
        Ok(serde_json::json!({
            "id": r.get::<_, String>(0)?,
            "parent_id": r.get::<_, Option<String>>(1)?,
            "kind": r.get::<_, String>(2)?,
            "name": r.get::<_, String>(3)?,
            "status": r.get::<_, String>(4)?,
            "started_at": r.get::<_, i64>(5)?,
            "ended_at": r.get::<_, Option<i64>>(6)?,
            "provider_id": r.get::<_, Option<String>>(7)?,
            "model": r.get::<_, Option<String>>(8)?,
            "tokens_in": r.get::<_, Option<i64>>(9)?,
            "tokens_out": r.get::<_, Option<i64>>(10)?,
            "tokens_cached": r.get::<_, Option<i64>>(11)?,
            "ttft_ms": r.get::<_, Option<i64>>(12)?,
            "error_code": r.get::<_, Option<String>>(13)?,
            "user_id": r.get::<_, Option<String>>(14)?,
            "device_id": r.get::<_, Option<String>>(15)?,
            "attrs": attrs.and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok()),
        }))
    })?;
    rows.collect()
}

/// The sealed payloads of one span, as stored (ciphertext in base64): `None` if there is no such
/// span, an empty list if it has none. Nothing here can turn them back into text.
pub fn span_payloads(conn: &Connection, span_id: &str) -> rusqlite::Result<Option<Vec<serde_json::Value>>> {
    use base64::Engine;
    let exists: bool = conn.query_row("SELECT EXISTS (SELECT 1 FROM spans WHERE id = ?1)", [span_id], |r| r.get(0))?;
    if !exists {
        return Ok(None);
    }
    let mut stmt = conn.prepare("SELECT role, key_id, ciphertext FROM payloads WHERE span_id = ?1 ORDER BY rowid")?;
    let rows = stmt.query_map([span_id], |r| {
        let ct: Vec<u8> = r.get(2)?;
        Ok(serde_json::json!({
            "role": r.get::<_, String>(0)?,
            "key_id": r.get::<_, String>(1)?,
            "ciphertext_b64": base64::engine::general_purpose::STANDARD.encode(ct),
        }))
    })?;
    Ok(Some(rows.collect::<Result<_, _>>()?))
}

/// The p-th percentile (0 to 100) of a sorted list, by nearest rank.
fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    Some(sorted[rank.min(sorted.len()) - 1])
}

/// Prices by provider id, as the snapshot has them now. `None` for a provider it does not know.
pub type PriceOf<'a> = &'a dyn Fn(&str) -> Option<crate::cost::Prices>;

/// The cost of a set of tokens reported by `provider_id`, or `None` when it cannot be known: the
/// provider is gone from the snapshot, or a price that matters is missing.
pub fn cost_of(price_of: PriceOf, provider_id: Option<&str>, input: Option<i64>, output: Option<i64>, cached: Option<i64>) -> Option<f64> {
    // No token counts at all (a request that failed before any reply): nothing to put a price on.
    if input.is_none() && output.is_none() {
        return None;
    }
    let prices = price_of(provider_id?)?;
    let n = |v: Option<i64>| v.unwrap_or(0).max(0) as u64;
    crate::cost::cost(crate::cost::Tokens { input: n(input), output: n(output), cached: n(cached) }, prices)
}

/// A sum of costs that is unknown if any part of it is.
#[derive(Default)]
struct CostSum {
    known: f64,
    unknown: usize,
}

impl CostSum {
    fn add(&mut self, cost: Option<f64>, has_tokens: bool) {
        match cost {
            Some(c) => self.known += c,
            None if has_tokens => self.unknown += 1,
            None => {}
        }
    }
    fn total(&self) -> Option<f64> {
        (self.unknown == 0).then_some(self.known)
    }
}

/// `YYYY-MM-DD` (UTC) of a unix time.
fn utc_day(t: i64) -> String {
    let z = t.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Counts, error rate, latency percentiles, token totals and cost for generations that began at or
/// after `since` (unix seconds), with a breakdown by provider and by day. Cost is computed here
/// from the prices `price_of` gives now (see `cost`): it is null when a needed price is missing.
pub fn stats(conn: &Connection, since: i64, price_of: PriceOf) -> rusqlite::Result<serde_json::Value> {
    let mut by_status = serde_json::Map::new();
    let mut requests = 0i64;
    {
        let mut stmt = conn.prepare("SELECT status, COUNT(*) FROM spans WHERE kind = 'turn' AND started_at >= ?1 GROUP BY status ORDER BY status")?;
        for row in stmt.query_map([since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (status, n) = row?;
            requests += n;
            by_status.insert(status, n.into());
        }
    }
    let errors: i64 = ["error", "timed_out"].iter().map(|k| by_status.get(*k).and_then(|v| v.as_i64()).unwrap_or(0)).sum();
    let (tokens_in, tokens_out, tokens_cached): (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(tokens_in),0), COALESCE(SUM(tokens_out),0), COALESCE(SUM(tokens_cached),0) FROM spans WHERE kind = 'turn' AND started_at >= ?1",
        [since],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let series = |sql: &str| -> rusqlite::Result<Vec<i64>> {
        let mut stmt = conn.prepare(sql)?;
        let mut v: Vec<i64> = stmt.query_map([since], |r| r.get::<_, i64>(0))?.collect::<Result<_, _>>()?;
        v.sort_unstable();
        Ok(v)
    };
    let latency = series("SELECT json_extract(attrs_json, '$.latency_ms') FROM spans WHERE kind = 'attempt' AND started_at >= ?1 AND json_extract(attrs_json, '$.latency_ms') IS NOT NULL")?;
    let ttft = series("SELECT ttft_ms FROM spans WHERE kind = 'attempt' AND started_at >= ?1 AND ttft_ms IS NOT NULL")?;
    // Cost, per provider and per day, from each generation's tokens and the provider that served it.
    struct Group {
        requests: i64,
        tokens: (i64, i64, i64),
        cost: CostSum,
    }
    let mut by_provider: std::collections::BTreeMap<String, Group> = Default::default();
    let mut by_day: std::collections::BTreeMap<String, Group> = Default::default();
    let mut total = CostSum::default();
    {
        let mut stmt = conn.prepare(
            "SELECT t.started_at, t.tokens_in, t.tokens_out, t.tokens_cached,
                    (SELECT provider_id FROM spans WHERE trace_id = t.trace_id AND kind = 'attempt' ORDER BY id DESC LIMIT 1)
             FROM spans t WHERE t.kind = 'turn' AND t.started_at >= ?1",
        )?;
        let rows = stmt.query_map([since], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<String>>(4)?))
        })?;
        for row in rows {
            let (started, tin, tout, tcached, provider) = row?;
            let has_tokens = tin.is_some() || tout.is_some();
            let cost = cost_of(price_of, provider.as_deref(), tin, tout, tcached);
            total.add(cost, has_tokens);
            for (map, key) in [(&mut by_provider, provider.clone().unwrap_or_default()), (&mut by_day, utc_day(started))] {
                let g = map.entry(key).or_insert_with(|| Group { requests: 0, tokens: (0, 0, 0), cost: CostSum::default() });
                g.requests += 1;
                g.tokens = (g.tokens.0 + tin.unwrap_or(0), g.tokens.1 + tout.unwrap_or(0), g.tokens.2 + tcached.unwrap_or(0));
                g.cost.add(cost, has_tokens);
            }
        }
    }
    let group_json = |key_name: &str, map: std::collections::BTreeMap<String, Group>| -> Vec<serde_json::Value> {
        map.into_iter()
            .map(|(key, g)| {
                serde_json::json!({
                    key_name: if key.is_empty() { serde_json::Value::Null } else { key.into() },
                    "requests": g.requests,
                    "tokens": { "in": g.tokens.0, "out": g.tokens.1, "cached": g.tokens.2 },
                    "cost": g.cost.total(),
                    "cost_unknown_requests": g.cost.unknown,
                })
            })
            .collect()
    };
    Ok(serde_json::json!({
        "since": since,
        "requests": requests,
        "cost": total.total(),
        "cost_unknown_requests": total.unknown,
        "by_provider": group_json("provider_id", by_provider),
        "by_day": group_json("day", by_day),
        "by_status": by_status,
        "error_rate": if requests > 0 { errors as f64 / requests as f64 } else { 0.0 },
        "latency_ms": { "p50": percentile(&latency, 50.0), "p95": percentile(&latency, 95.0) },
        "ttft_ms": { "p50": percentile(&ttft, 50.0), "p95": percentile(&ttft, 95.0) },
        "tokens": { "in": tokens_in, "out": tokens_out, "cached": tokens_cached },
    }))
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
    /// When it began, in unix milliseconds. The columns hold whole seconds; this keeps the finer
    /// time in `attrs.started_ms` so a trace can be drawn to scale.
    pub started_ms: i64,
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
    /// Merged into the span's attributes (a key given here replaces the same key from the start).
    pub attrs_json: Option<String>,
    /// When it ended, unix milliseconds (kept in `attrs.ended_ms`).
    pub ended_ms: i64,
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

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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
            started_ms: now_ms(),
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
        SpanFinish { id: id.to_string(), status: status.to_string(), ended_at: now_secs(), ended_ms: now_ms(), ..Default::default() }
    }

    pub fn error_code(mut self, code: &str) -> SpanFinish {
        self.error_code = Some(code.to_string());
        self
    }
}

/// A sealed payload (see `seal`) for a span: `role` is `request`, `reply` or `error`.
#[derive(Debug, Clone)]
pub struct PayloadWrite {
    pub span_id: String,
    pub role: String,
    pub key_id: String,
    pub ciphertext: Vec<u8>,
    pub created_at: i64,
}

enum Event {
    Start(SpanStart),
    Finish(SpanFinish),
    /// Answered once everything sent before it has been written.
    Flush(oneshot::Sender<()>),
    /// Sealed content attached to a span.
    Payload(PayloadWrite),
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
    path: Option<std::path::PathBuf>,
    pause: Arc<Pause>,
    dropped: Arc<AtomicU64>,
}

impl Recorder {
    /// Records nothing. The default, so a server without an audit store behaves as before.
    pub fn noop() -> Recorder {
        Recorder { tx: None, path: None, pause: Arc::default(), dropped: Arc::default() }
    }

    /// Takes ownership of `db` and writes to it from one thread. `capacity` is the queue length.
    pub fn spawn(db: AuditDb, capacity: usize) -> Recorder {
        let path = db.path.clone();
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
                        Event::Payload(p) => {
                            if let Err(e) = write_payload(db.conn(), &p) {
                                tracing::warn!("audit: could not write a payload: {e}");
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
        Recorder { tx: Some(tx), path, pause, dropped: Arc::default() }
    }

    /// False while the queue is full, i.e. the writer is not keeping up.
    pub fn healthy(&self) -> bool {
        self.tx.as_ref().is_none_or(|tx| tx.capacity() > 0)
    }

    /// A read-only connection to the audit file, for the management API. None when nothing is
    /// recorded to a file. Reading never blocks the writer (the file is in WAL mode).
    pub fn reader(&self) -> Option<Connection> {
        let path = self.path.as_ref()?;
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
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

    /// Records sealed content for a span. Only ciphertext ever gets here.
    pub fn payload(&self, span_id: &str, role: &str, sealed: crate::seal::Sealed) {
        self.send(Event::Payload(PayloadWrite {
            span_id: span_id.to_string(),
            role: role.to_string(),
            key_id: sealed.key_id,
            ciphertext: sealed.bytes,
            created_at: now_secs(),
        }));
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

fn write_payload(conn: &Connection, p: &PayloadWrite) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![p.span_id, p.role, p.key_id, p.ciphertext, p.created_at],
    )?;
    Ok(())
}

fn write_start(conn: &Connection, s: &SpanStart) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO spans (id, trace_id, parent_id, kind, name, status, started_at, provider_id, model, user_id, device_id, attrs_json)
         VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?8, ?9, ?10, json_patch(COALESCE(?11, '{}'), json_object('started_ms', ?12)))",
        rusqlite::params![s.id, s.trace_id, s.parent_id, s.kind, s.name, s.started_at, s.provider_id, s.model, s.user_id, s.device_id, s.attrs_json, s.started_ms],
    )?;
    Ok(())
}

fn write_finish(conn: &Connection, f: &SpanFinish) -> rusqlite::Result<()> {
    // `AND status = 'running'`: a span finishes once; a second finish changes nothing.
    conn.execute(
        "UPDATE spans SET status = ?2, ended_at = ?3,
             provider_id = COALESCE(?4, provider_id), model = COALESCE(?5, model),
             tokens_in = ?6, tokens_out = ?7, tokens_cached = ?8, ttft_ms = ?9,
             error_code = ?10,
             attrs_json = json_patch(json_patch(COALESCE(attrs_json, '{}'), COALESCE(?11, '{}')), json_object('ended_ms', ?12))
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
            f.attrs_json,
            f.ended_ms
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

    #[tokio::test]
    async fn spans_keep_millisecond_times_in_their_attributes_and_attributes_merge_on_finish() {
        let (dir, db) = temp_db();
        let rec = Recorder::spawn(db, 64);
        let id = rec.start_span(SpanStart::new("t", Kind::Attempt, "p").attrs(&serde_json::json!({ "retry_index": 1, "keep": "start" })));
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let mut done = SpanFinish::new(&id, "done");
        done.attrs_json = Some(serde_json::json!({ "latency_ms": 31, "keep": "finish" }).to_string());
        rec.finish_span(done);
        let bare = rec.start_span(SpanStart::new("t", Kind::Gate, "g")); // no attributes of its own
        rec.finish_span(SpanFinish::new(&bare, "done"));
        rec.flush().await;
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        let attrs = |id: &str| -> serde_json::Value {
            let a: String = conn.query_row("SELECT attrs_json FROM spans WHERE id = ?1", [id], |r| r.get(0)).unwrap();
            serde_json::from_str(&a).unwrap()
        };
        let a = attrs(&id);
        assert_eq!((a["retry_index"].as_i64(), a["latency_ms"].as_i64(), a["keep"].as_str()), (Some(1), Some(31), Some("finish")), "start attributes survive, finish ones are added or win");
        let (s, e) = (a["started_ms"].as_i64().unwrap(), a["ended_ms"].as_i64().unwrap());
        assert!(e - s >= 25 && e - s < 2000, "the span took about 30 ms: {}", e - s);
        let b = attrs(&bare);
        assert!(b["started_ms"].is_i64() && b["ended_ms"].is_i64(), "even a span with no attributes of its own has its times: {b}");
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
