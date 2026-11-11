//! The audit store: a separate SQLite file (`audit.db`) holding a trace of spans for every
//! generation. It has its own append-only migrations and its own `PRAGMA user_version`,
//! independent of `control.db` and the space files. Phase 4 stores metadata only; message content is added in
//! Phase 5, sealed.

pub mod chain;

pub use rusqlite;
use rusqlite::Connection;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
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
         client_id     TEXT,
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
    // v3: the auditor (ADR 0051). A span records which service appended it and in which process (its boot
    // ID), so a service that starts again can have its earlier unfinished spans ended as interrupted
    // (ADR 0010 rule 6). Admin action records, a record of each prune and the retention setting are new.
    // Admin action records and prune records are never changed or deleted: retention removes spans and
    // payloads only.
    "ALTER TABLE spans ADD COLUMN service TEXT;
     ALTER TABLE spans ADD COLUMN boot_id TEXT;
     CREATE INDEX idx_spans_service_status ON spans(service, status);
     CREATE TABLE admin_actions (
         id          TEXT PRIMARY KEY,
         at          INTEGER NOT NULL,
         service     TEXT    NOT NULL,
         category    TEXT    NOT NULL,
         action      TEXT    NOT NULL,
         outcome     TEXT    NOT NULL,
         detail_json TEXT
     );
     CREATE INDEX idx_admin_actions_at ON admin_actions(at DESC, id DESC);
     CREATE TRIGGER admin_actions_are_never_changed BEFORE UPDATE ON admin_actions
     BEGIN SELECT RAISE(ABORT, 'admin action records cannot be changed'); END;
     CREATE TRIGGER admin_actions_are_never_deleted BEFORE DELETE ON admin_actions
     BEGIN SELECT RAISE(ABORT, 'admin action records cannot be deleted'); END;
     CREATE TABLE prunes (
         id       INTEGER PRIMARY KEY AUTOINCREMENT,
         at       INTEGER NOT NULL,
         cutoff   INTEGER NOT NULL,
         spans    INTEGER NOT NULL,
         payloads INTEGER NOT NULL
     );
     CREATE TRIGGER prunes_are_never_changed BEFORE UPDATE ON prunes
     BEGIN SELECT RAISE(ABORT, 'prune records cannot be changed'); END;
     CREATE TRIGGER prunes_are_never_deleted BEFORE DELETE ON prunes
     BEGIN SELECT RAISE(ABORT, 'prune records cannot be deleted'); END;
     CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // v4: the hash chain (ADR 0010 rule 7, ADR 0052). One entry for each record taken: a span's start, a span's finish, a
    // payload, an admin action, a prune. Each holds a digest of its record, the hash of the entry before it and its own
    // hash over all of that. Entries are never changed or deleted, a prune included: they hold digests, not content.
    // What the file held before is chained by `chain::backfill` as this migration is applied.
    "CREATE TABLE chain (
         seq       INTEGER PRIMARY KEY,
         kind      TEXT    NOT NULL CHECK (kind IN ('span_start','span_finish','payload','admin_action','prune')),
         ref       TEXT    NOT NULL,
         row_at    INTEGER NOT NULL,
         digest    TEXT    NOT NULL,
         prev_hash TEXT    NOT NULL,
         hash      TEXT    NOT NULL
     );
     CREATE INDEX idx_chain_ref ON chain(kind, ref);
     CREATE TRIGGER chain_is_never_changed BEFORE UPDATE ON chain
     BEGIN SELECT RAISE(ABORT, 'chain entries cannot be changed'); END;
     CREATE TRIGGER chain_is_never_deleted BEFORE DELETE ON chain
     BEGIN SELECT RAISE(ABORT, 'chain entries cannot be deleted'); END;",
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
            if i == 3 {
                chain::backfill(&tx)?;
            }
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
        let tx = self.conn.unchecked_transaction()?;
        let n = interrupt_running(&tx, "", [])?;
        tx.commit()?;
        Ok(n)
    }

    /// The newest entry of the hash chain.
    pub fn chain_head(&self) -> Result<chain::Head, AuditError> {
        Ok(chain::head(&self.conn)?)
    }
}

/// Marks the spans still `running` that match `condition` (SQL starting with ` AND `, or empty) as interrupted, and
/// chains each one's finish. Returns how many.
fn interrupt_running<P: rusqlite::Params + Clone>(conn: &Connection, condition: &str, params: P) -> rusqlite::Result<usize> {
    let ids: Vec<String> = {
        let mut stmt = conn.prepare(&format!("SELECT id FROM spans WHERE status = 'running'{condition}"))?;
        let rows = stmt.query_map(params.clone(), |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    conn.execute(
        &format!(
            "UPDATE spans SET status = 'interrupted', ended_at = {}, error_code = COALESCE(error_code, 'interrupted') WHERE status = 'running'{condition}",
            now_secs()
        ),
        params,
    )?;
    for id in &ids {
        chain::record(conn, "span_finish", id)?;
    }
    Ok(ids.len())
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
        "SELECT t.trace_id, t.id, t.status, t.started_at, t.ended_at, t.user_id, t.client_id,
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
            "client_id": r.get::<_, Option<String>>(6)?,
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
                tokens_cached, ttft_ms, error_code, user_id, client_id, attrs_json,
                (SELECT COUNT(*) FROM payloads WHERE span_id = spans.id)
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
            "client_id": r.get::<_, Option<String>>(15)?,
            "attrs": attrs.and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok()),
            // How many sealed payloads the span has (their content is not here).
            "sealed": r.get::<_, i64>(17)?,
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

/// Admin action records, newest first. `before` is the id of the last one already seen (ids sort by time).
pub fn list_admin_actions(conn: &Connection, before: Option<&str>, limit: Option<usize>) -> rusqlite::Result<Vec<serde_json::Value>> {
    let limit = limit.unwrap_or(DEFAULT_TRACE_LIMIT).clamp(1, MAX_TRACE_LIMIT);
    let mut stmt = conn.prepare(
        "SELECT id, at, service, category, action, outcome, detail_json FROM admin_actions
         WHERE (?1 IS NULL OR id < ?1) ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![before, limit as i64], |r| {
        let detail: Option<String> = r.get(6)?;
        Ok(serde_json::json!({
            "id": r.get::<_, String>(0)?,
            "at": r.get::<_, i64>(1)?,
            "service": r.get::<_, String>(2)?,
            "category": r.get::<_, String>(3)?,
            "action": r.get::<_, String>(4)?,
            "outcome": r.get::<_, String>(5)?,
            "detail": detail.and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok()),
        }))
    })?;
    rows.collect()
}

/// What was used since `since` (unix seconds), by provider and model of each generation's last attempt. Token
/// counts only: prices are the admin's.
pub fn token_counts(conn: &Connection, since: i64) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT a.provider_id, a.model, COUNT(*), COALESCE(SUM(t.tokens_in), 0), COALESCE(SUM(t.tokens_out), 0),
                COALESCE(SUM(t.tokens_cached), 0)
         FROM spans t
         LEFT JOIN spans a ON a.id = (SELECT id FROM spans WHERE trace_id = t.trace_id AND kind = 'attempt' ORDER BY id DESC LIMIT 1)
         WHERE t.kind = 'turn' AND t.started_at >= ?1
         GROUP BY a.provider_id, a.model ORDER BY a.provider_id, a.model",
    )?;
    let rows = stmt.query_map([since], |r| {
        Ok(serde_json::json!({
            "provider_id": r.get::<_, Option<String>>(0)?,
            "model": r.get::<_, Option<String>>(1)?,
            "requests": r.get::<_, i64>(2)?,
            "tokens_in": r.get::<_, i64>(3)?,
            "tokens_out": r.get::<_, i64>(4)?,
            "tokens_cached": r.get::<_, i64>(5)?,
        }))
    })?;
    rows.collect()
}

/// A setting of the store itself (see `Record::Setting`).
pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    use rusqlite::OptionalExtension;
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
}

/// Every prune so far, oldest first: when, the cutoff, and how many spans and payloads it deleted.
pub fn list_prunes(conn: &Connection) -> rusqlite::Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare("SELECT at, cutoff, spans, payloads FROM prunes ORDER BY id")?;
    let rows = stmt.query_map([], |r| {
        Ok(serde_json::json!({
            "at": r.get::<_, i64>(0)?, "cutoff": r.get::<_, i64>(1)?, "spans": r.get::<_, i64>(2)?, "payloads": r.get::<_, i64>(3)?,
        }))
    })?;
    rows.collect()
}

/// The p-th percentile (0 to 100) of a sorted list, by nearest rank.
fn percentile(sorted: &[i64], p: f64) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    Some(sorted[rank.min(sorted.len()) - 1])
}

/// Works out the cost of a set of tokens: `(provider_id, input, output, cached)` to a cost, or
/// `None` when it cannot be known. The audit store holds only token counts; the caller owns prices.
pub type CostFn<'a> = &'a dyn Fn(Option<&str>, Option<i64>, Option<i64>, Option<i64>) -> Option<f64>;

/// A sealed payload and the id of the key it was sealed to, so the right key can be chosen to
/// read it even after the key has been rotated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub key_id: String,
    pub bytes: Vec<u8>,
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
/// from the prices `cost_of` applies now: it is null when a needed price is missing.
pub fn stats(conn: &Connection, since: i64, cost_of: CostFn) -> rusqlite::Result<serde_json::Value> {
    let mut by_status = serde_json::Map::new();
    let mut requests = 0i64;
    {
        let mut stmt =
            conn.prepare("SELECT status, COUNT(*) FROM spans WHERE kind = 'turn' AND started_at >= ?1 GROUP BY status ORDER BY status")?;
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
        /// How many of them have token counts (the rest cost nothing to price, and nothing to know).
        with_tokens: i64,
        tokens: (i64, i64, i64),
        cost: CostSum,
    }
    let mut by_provider: std::collections::BTreeMap<String, Group> = Default::default();
    let mut by_day: std::collections::BTreeMap<String, Group> = Default::default();
    // Per day and provider, with the tokens and no cost, so whoever holds the prices can price each day exactly.
    let mut by_day_provider: std::collections::BTreeMap<(String, String), Group> = Default::default();
    let mut total = CostSum::default();
    {
        let mut stmt = conn.prepare(
            "SELECT t.started_at, t.tokens_in, t.tokens_out, t.tokens_cached,
                    (SELECT provider_id FROM spans WHERE trace_id = t.trace_id AND kind = 'attempt' ORDER BY id DESC LIMIT 1)
             FROM spans t WHERE t.kind = 'turn' AND t.started_at >= ?1",
        )?;
        let rows = stmt.query_map([since], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        for row in rows {
            let (started, tin, tout, tcached, provider) = row?;
            let has_tokens = tin.is_some() || tout.is_some();
            let cost = cost_of(provider.as_deref(), tin, tout, tcached);
            total.add(cost, has_tokens);
            for (map, key) in [(&mut by_provider, provider.clone().unwrap_or_default()), (&mut by_day, utc_day(started))] {
                let g =
                    map.entry(key).or_insert_with(|| Group { requests: 0, with_tokens: 0, tokens: (0, 0, 0), cost: CostSum::default() });
                g.requests += 1;
                g.with_tokens += i64::from(has_tokens);
                g.tokens = (g.tokens.0 + tin.unwrap_or(0), g.tokens.1 + tout.unwrap_or(0), g.tokens.2 + tcached.unwrap_or(0));
                g.cost.add(cost, has_tokens);
            }
            let g = by_day_provider.entry((utc_day(started), provider.clone().unwrap_or_default())).or_insert_with(|| Group {
                requests: 0,
                with_tokens: 0,
                tokens: (0, 0, 0),
                cost: CostSum::default(),
            });
            g.requests += 1;
            g.with_tokens += i64::from(has_tokens);
            g.tokens = (g.tokens.0 + tin.unwrap_or(0), g.tokens.1 + tout.unwrap_or(0), g.tokens.2 + tcached.unwrap_or(0));
        }
    }
    // How the router treated each provider: every try (attempt span) it served or failed, the retries
    // among them (retry_index above 0), and the failures.
    let mut routing: std::collections::BTreeMap<String, (i64, i64, i64)> = Default::default();
    {
        let mut stmt = conn.prepare(
            "SELECT COALESCE(provider_id, ''), COUNT(*),
                    COALESCE(SUM(CASE WHEN json_extract(attrs_json, '$.retry_index') > 0 THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(CASE WHEN status = 'error' THEN 1 ELSE 0 END), 0)
             FROM spans WHERE kind = 'attempt' AND started_at >= ?1 GROUP BY provider_id",
        )?;
        for row in
            stmt.query_map([since], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?))))?
        {
            let (k, v) = row?;
            routing.insert(k, v);
        }
    }
    // A provider that only ever failed (no request ended on it) still has a row, so its failures show.
    for k in routing.keys() {
        by_provider.entry(k.clone()).or_insert_with(|| Group { requests: 0, with_tokens: 0, tokens: (0, 0, 0), cost: CostSum::default() });
    }
    let group_json = |key_name: &str, map: std::collections::BTreeMap<String, Group>| -> Vec<serde_json::Value> {
        map.into_iter()
            .map(|(key, g)| {
                let mut v = serde_json::json!({
                    key_name: if key.is_empty() { serde_json::Value::Null } else { key.clone().into() },
                    "requests": g.requests,
                    "requests_with_tokens": g.with_tokens,
                    "tokens": { "in": g.tokens.0, "out": g.tokens.1, "cached": g.tokens.2 },
                    "cost": g.cost.total(),
                    "cost_unknown_requests": g.cost.unknown,
                });
                if key_name == "provider_id" {
                    let (attempts, retries, failed) = routing.get(&key).copied().unwrap_or_default();
                    v["attempts"] = attempts.into();
                    v["retries"] = retries.into();
                    v["failed_attempts"] = failed.into();
                }
                v
            })
            .collect()
    };
    let by_day_provider: Vec<serde_json::Value> = by_day_provider
        .into_iter()
        .map(|((day, provider), g)| {
            serde_json::json!({
                "day": day,
                "provider_id": if provider.is_empty() { serde_json::Value::Null } else { provider.into() },
                "requests": g.requests,
                "requests_with_tokens": g.with_tokens,
                "tokens": { "in": g.tokens.0, "out": g.tokens.1, "cached": g.tokens.2 },
            })
        })
        .collect();
    Ok(serde_json::json!({
        "since": since,
        "requests": requests,
        "cost": total.total(),
        "cost_unknown_requests": total.unknown,
        "by_provider": group_json("provider_id", by_provider),
        "by_day": group_json("day", by_day),
        "by_day_provider": by_day_provider,
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
    pub client_id: Option<String>,
    pub attrs_json: Option<String>,
    /// When it began, in unix milliseconds. The columns hold whole seconds; this keeps the finer
    /// time in `attrs.started_ms` so a trace can be drawn to scale.
    pub started_ms: i64,
    /// The service that appended it and its process, set by the auditor from the caller's token (a
    /// server recording to its own file leaves them empty).
    pub service: Option<String>,
    pub boot_id: Option<String>,
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
            id: askbar_service::ids::new_id(),
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

    pub fn who(mut self, user_id: &str, client_id: Option<&str>) -> SpanStart {
        self.user_id = Some(user_id.to_string());
        self.client_id = client_id.map(str::to_string);
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

/// An admin action record (ADR 0010), as stored.
#[derive(Debug, Clone)]
pub struct AdminActionWrite {
    pub id: String,
    pub at: i64,
    /// The caller, from its token.
    pub service: String,
    pub category: String,
    pub action: String,
    pub outcome: String,
    pub detail_json: Option<String>,
}

/// What can be appended and acknowledged (see [`Recorder::append`]). The acknowledgment comes after the
/// record is committed, and appending the same record twice records it once.
#[derive(Debug, Clone)]
pub enum Record {
    Start(SpanStart),
    Finish(SpanFinish),
    Payload(PayloadWrite),
    AdminAction(AdminActionWrite),
    /// A service's process announces itself: that service's spans from other processes that are still
    /// running end as interrupted. Acknowledged with how many.
    Hello {
        service: String,
        boot_id: String,
    },
    /// A setting of the store itself (the retention days). Not an audit record.
    Setting {
        key: String,
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppendError {
    /// The queue is full: nothing was recorded.
    #[error("the audit queue is full")]
    Full,
    /// A finish or a payload for a span that is not there.
    #[error("no such span")]
    NoSuchSpan,
    #[error("the audit writer has stopped")]
    Stopped,
    #[error("{0}")]
    Db(String),
}

type Ack = oneshot::Sender<Result<u64, AppendError>>;

enum Event {
    Start(SpanStart),
    Finish(SpanFinish),
    /// Answered once everything sent before it has been written.
    Flush(oneshot::Sender<()>),
    /// Sealed content attached to a span.
    Payload(PayloadWrite),
    /// Delete spans and payloads that started before `cutoff`; answered with how many of each.
    Prune {
        cutoff: i64,
        done: oneshot::Sender<Result<(usize, usize), String>>,
    },
    /// A record whose caller waits for it to be committed.
    Acked(Record, Ack),
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

/// How far behind the writer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Backlog {
    /// Records queued or being written.
    pub queued: usize,
    /// How long the oldest of them has waited.
    pub oldest: Duration,
}

/// The most records one transaction holds (group commit).
const MAX_BATCH: usize = 128;

/// The handle the rest of the server records through. Cheap to clone. Recording never waits:
/// events go into a bounded queue, one writer thread owns the connection and drains it, and
/// when the queue is full the event is dropped and `healthy()` says so (new generations are
/// then refused, see 4.10). The writer commits what is queued in groups. A caller that must know its
/// record is stored uses [`Recorder::append`], which answers after the commit.
#[derive(Clone)]
pub struct Recorder {
    tx: Option<mpsc::Sender<Event>>,
    path: Option<std::path::PathBuf>,
    pause: Arc<Pause>,
    dropped: Arc<AtomicU64>,
    /// When each queued record was queued, oldest first. Pushed under the same lock that sends it and
    /// popped by the writer when it takes it, so the two never disagree.
    queued: Arc<Mutex<VecDeque<Instant>>>,
}

impl Recorder {
    /// Records nothing. The default, so a server without an audit store behaves as before.
    pub fn noop() -> Recorder {
        Recorder { tx: None, path: None, pause: Arc::default(), dropped: Arc::default(), queued: Arc::default() }
    }

    /// Takes ownership of `db` and writes to it from one thread. `capacity` is the queue length.
    pub fn spawn(db: AuditDb, capacity: usize) -> Recorder {
        let path = db.path.clone();
        let (tx, mut rx) = mpsc::channel::<Event>(capacity.max(1));
        let pause = Arc::new(Pause::default());
        let writer_pause = pause.clone();
        let queued: Arc<Mutex<VecDeque<Instant>>> = Arc::default();
        let writer_queued = queued.clone();
        std::thread::Builder::new()
            .name("askbar-audit-writer".into())
            .spawn(move || {
                while let Some(first) = rx.blocking_recv() {
                    let mut paused = writer_pause.paused.lock().unwrap_or_else(|e| e.into_inner());
                    while *paused {
                        paused = writer_pause.changed.wait(paused).unwrap_or_else(|e| e.into_inner());
                    }
                    drop(paused);
                    // Whatever else is already queued goes in the same transaction.
                    let mut batch = vec![first];
                    while batch.len() < MAX_BATCH {
                        match rx.try_recv() {
                            Ok(event) => batch.push(event),
                            Err(_) => break,
                        }
                    }
                    {
                        let mut q = writer_queued.lock().unwrap_or_else(|e| e.into_inner());
                        for _ in 0..batch.len() {
                            q.pop_front();
                        }
                    }
                    run_batch(db.conn(), batch);
                }
            })
            .expect("audit writer thread");
        Recorder { tx: Some(tx), path, pause, dropped: Arc::default(), queued }
    }

    /// Queues `event` if there is room. The queue time is noted under the lock that sends it.
    fn try_enqueue(&self, event: Event) -> Result<(), AppendError> {
        let Some(tx) = &self.tx else { return Err(AppendError::Stopped) };
        let mut queued = self.queued.lock().unwrap_or_else(|e| e.into_inner());
        match tx.try_reserve() {
            Ok(permit) => {
                permit.send(event);
                queued.push_back(Instant::now());
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => Err(AppendError::Full),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(AppendError::Stopped),
        }
    }

    /// Queues `event`, waiting for room.
    async fn enqueue(&self, event: Event) -> Result<(), AppendError> {
        let Some(tx) = &self.tx else { return Err(AppendError::Stopped) };
        let permit = tx.reserve().await.map_err(|_| AppendError::Stopped)?;
        let mut queued = self.queued.lock().unwrap_or_else(|e| e.into_inner());
        permit.send(event);
        queued.push_back(Instant::now());
        Ok(())
    }

    /// Appends `record` and answers once it is committed: how many spans a `Hello` interrupted, else 0.
    /// Never waits for room: a full queue is `Full`, and nothing was recorded.
    pub async fn append(&self, record: Record) -> Result<u64, AppendError> {
        let (ack, wait) = oneshot::channel();
        self.try_enqueue(Event::Acked(record, ack))?;
        wait.await.map_err(|_| AppendError::Stopped)?
    }

    /// How far behind the writer is.
    pub fn backlog(&self) -> Backlog {
        let q = self.queued.lock().unwrap_or_else(|e| e.into_inner());
        Backlog { queued: q.len(), oldest: q.front().map(Instant::elapsed).unwrap_or_default() }
    }

    /// False while the queue is full, i.e. the writer is not keeping up.
    pub fn healthy(&self) -> bool {
        self.tx.as_ref().is_none_or(|tx| tx.capacity() > 0)
    }

    /// A read-only connection to the audit file, for the server API. None when nothing is
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
        if self.tx.is_none() {
            return;
        }
        if self.try_enqueue(event).is_err() {
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
    pub fn payload(&self, span_id: &str, role: &str, sealed: Sealed) {
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
        if self.tx.is_none() {
            return;
        }
        let (done, wait) = oneshot::channel();
        if self.enqueue(Event::Flush(done)).await.is_ok() {
            let _ = wait.await;
        }
    }

    /// Deletes what is older than `retention_days` before `now` (see `retention_cutoff`). `now`
    /// is passed in so tests can cross the boundary without waiting. Runs on the writer's
    /// connection, in order with everything else. Returns (spans, payloads) deleted.
    pub async fn prune(&self, retention_days: u32, now: i64) -> Result<(usize, usize), String> {
        if self.tx.is_none() {
            return Ok((0, 0));
        }
        let (done, wait) = oneshot::channel();
        self.enqueue(Event::Prune { cutoff: retention_cutoff(now, retention_days), done })
            .await
            .map_err(|_| "the audit writer has stopped".to_string())?;
        wait.await.map_err(|_| "the audit writer has stopped".to_string())?
    }

    /// Tests only: hold the writer still (or let it go on), to fill the queue.
    #[doc(hidden)]
    pub fn pause_writer(&self, paused: bool) {
        *self.pause.paused.lock().unwrap_or_else(|e| e.into_inner()) = paused;
        self.pause.changed.notify_all();
    }

    /// Tests only: hold the writer still with an event in its hands, and return once it is. A paused writer
    /// still takes one event off the queue and parks with it, so with `pause_writer` alone the queue looks
    /// full until the writer's thread first runs, and then has room for one more (a test that checks
    /// `healthy()` in between sees it flip). Here the writer has taken its event before this returns, so
    /// the queue holds exactly its capacity from then on and stays unhealthy until the writer is let go.
    #[doc(hidden)]
    pub async fn stall_writer(&self) {
        self.pause_writer(true);
        let Some(tx) = &self.tx else { return };
        // An event that writes nothing: the writer answers it when it is let go, and nobody is listening.
        let (done, _) = oneshot::channel();
        if self.enqueue(Event::Flush(done)).await.is_err() {
            return;
        }
        // A writer that never takes it (its thread has died) fails the test here instead of hanging it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while tx.capacity() < tx.max_capacity() {
            assert!(std::time::Instant::now() < deadline, "the audit writer did not take its event within 5 s");
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }
}

/// Payloads first (they belong to spans), then the spans, and a record of the prune, in one transaction: a
/// delete without its record cannot happen. Only finished spans: one still running is never old enough to
/// matter, and the sweep handles the strays.
fn prune(conn: &Connection, cutoff: i64) -> rusqlite::Result<(usize, usize)> {
    let tx = conn.unchecked_transaction()?;
    let payloads = tx.execute("DELETE FROM payloads WHERE created_at < ?1", [cutoff])?;
    let spans = tx.execute("DELETE FROM spans WHERE started_at < ?1 AND status <> 'running'", [cutoff])?;
    tx.execute(
        "INSERT INTO prunes (at, cutoff, spans, payloads) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![now_secs(), cutoff, spans as i64, payloads as i64],
    )?;
    chain::record(&tx, "prune", &tx.last_insert_rowid().to_string())?;
    tx.commit()?;
    Ok((spans, payloads))
}

type Answers = Vec<(Ack, Result<u64, AppendError>)>;

/// Writes one batch in one transaction, then answers the callers that wait for a commit. A prune is its own
/// transaction (it is written with its record), so the batch is committed before it and a new one begins.
fn run_batch(conn: &Connection, batch: Vec<Event>) {
    let mut in_txn = conn.execute_batch("BEGIN IMMEDIATE").is_ok();
    let mut acks: Answers = Vec::new();
    let mut dones: Vec<oneshot::Sender<()>> = Vec::new();
    let commit = |in_txn: &mut bool, acks: &mut Answers, dones: &mut Vec<oneshot::Sender<()>>| {
        let committed = if *in_txn { conn.execute_batch("COMMIT").map_err(|e| AppendError::Db(e.to_string())) } else { Ok(()) };
        *in_txn = false;
        for (ack, result) in acks.drain(..) {
            let _ = ack.send(match &committed {
                Ok(()) => result,
                Err(e) => Err(e.clone()),
            });
        }
        for done in dones.drain(..) {
            let _ = done.send(());
        }
    };
    for event in batch {
        match event {
            Event::Start(s) => {
                if let Err(e) = write_start(conn, &s, false) {
                    tracing::warn!("audit: could not write a span: {e}");
                }
            }
            Event::Payload(p) => {
                if let Err(e) = write_payload(conn, &p) {
                    tracing::warn!("audit: could not write a payload: {e}");
                }
            }
            Event::Finish(f) => {
                if let Err(e) = write_finish(conn, &f) {
                    tracing::warn!("audit: could not finish a span: {e}");
                }
            }
            Event::Flush(done) => dones.push(done),
            Event::Acked(record, ack) => {
                let result = apply(conn, &record);
                acks.push((ack, result));
            }
            Event::Prune { cutoff, done } => {
                commit(&mut in_txn, &mut acks, &mut dones);
                let _ = done.send(prune(conn, cutoff).map_err(|e| e.to_string()));
                in_txn = conn.execute_batch("BEGIN IMMEDIATE").is_ok();
            }
        }
    }
    commit(&mut in_txn, &mut acks, &mut dones);
}

fn db_err(e: rusqlite::Error) -> AppendError {
    AppendError::Db(e.to_string())
}

fn span_exists(conn: &Connection, id: &str) -> Result<bool, AppendError> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM spans WHERE id = ?1)", [id], |r| r.get(0)).map_err(db_err)
}

/// One acknowledged record. Idempotent: a retry of a record that was stored changes nothing.
fn apply(conn: &Connection, record: &Record) -> Result<u64, AppendError> {
    match record {
        Record::Start(s) => {
            write_start(conn, s, true).map_err(db_err)?;
            Ok(0)
        }
        Record::Finish(f) => {
            if !span_exists(conn, &f.id)? {
                return Err(AppendError::NoSuchSpan);
            }
            write_finish(conn, f).map_err(db_err)?;
            Ok(0)
        }
        Record::Payload(p) => {
            if !span_exists(conn, &p.span_id)? {
                return Err(AppendError::NoSuchSpan);
            }
            let n = conn
                .execute(
                    "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at)
                     SELECT ?1, ?2, ?3, ?4, ?5
                     WHERE NOT EXISTS (SELECT 1 FROM payloads WHERE span_id = ?1 AND role = ?2 AND ciphertext = ?4)",
                    rusqlite::params![p.span_id, p.role, p.key_id, p.ciphertext, p.created_at],
                )
                .map_err(db_err)?;
            if n > 0 {
                chain::record(conn, "payload", &conn.last_insert_rowid().to_string()).map_err(db_err)?;
            }
            Ok(0)
        }
        Record::AdminAction(a) => {
            let n = conn
                .execute(
                    "INSERT OR IGNORE INTO admin_actions (id, at, service, category, action, outcome, detail_json) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    rusqlite::params![a.id, a.at, a.service, a.category, a.action, a.outcome, a.detail_json],
                )
                .map_err(db_err)?;
            if n > 0 {
                chain::record(conn, "admin_action", &a.id).map_err(db_err)?;
            }
            Ok(0)
        }
        Record::Hello { service, boot_id } => {
            let n = interrupt_running(conn, " AND service = ?1 AND COALESCE(boot_id, '') <> ?2", rusqlite::params![service, boot_id])
                .map_err(db_err)?;
            Ok(n as u64)
        }
        Record::Setting { key, value } => {
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map_err(db_err)?;
            Ok(0)
        }
    }
}

fn write_payload(conn: &Connection, p: &PayloadWrite) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,?2,?3,?4,?5)",
        rusqlite::params![p.span_id, p.role, p.key_id, p.ciphertext, p.created_at],
    )?;
    chain::record(conn, "payload", &conn.last_insert_rowid().to_string())
}

fn write_start(conn: &Connection, s: &SpanStart, ignore_duplicate: bool) -> rusqlite::Result<usize> {
    let n = conn.execute(
        &format!(
            "INSERT {} INTO spans (id, trace_id, parent_id, kind, name, status, started_at, provider_id, model, user_id, client_id, attrs_json, service, boot_id)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?8, ?9, ?10, json_patch(COALESCE(?11, '{{}}'), json_object('started_ms', ?12)), ?13, ?14)",
            if ignore_duplicate { "OR IGNORE" } else { "" }
        ),
        rusqlite::params![
            s.id,
            s.trace_id,
            s.parent_id,
            s.kind,
            s.name,
            s.started_at,
            s.provider_id,
            s.model,
            s.user_id,
            s.client_id,
            s.attrs_json,
            s.started_ms,
            s.service,
            s.boot_id
        ],
    )?;
    if n > 0 {
        chain::record(conn, "span_start", &s.id)?;
    }
    Ok(n)
}

fn write_finish(conn: &Connection, f: &SpanFinish) -> rusqlite::Result<()> {
    // `AND status = 'running'`: a span finishes once; a second finish changes nothing.
    let n = conn.execute(
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
    if n > 0 {
        chain::record(conn, "span_finish", &f.id)?;
    }
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
        assert_eq!(tables(&db), ["admin_actions", "chain", "payloads", "prunes", "settings", "spans", "sqlite_sequence"]);
        assert_eq!(
            columns(&db, "spans"),
            [
                "id",
                "trace_id",
                "parent_id",
                "kind",
                "name",
                "status",
                "started_at",
                "ended_at",
                "provider_id",
                "model",
                "tokens_in",
                "tokens_out",
                "tokens_cached",
                "ttft_ms",
                "error_code",
                "user_id",
                "client_id",
                "attrs_json",
                "service",
                "boot_id"
            ]
        );
        assert_eq!(columns(&db, "admin_actions"), ["id", "at", "service", "category", "action", "outcome", "detail_json"]);
        assert_eq!(columns(&db, "prunes"), ["id", "at", "cutoff", "spans", "payloads"]);
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
            db.conn
                .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('s','t','turn','chat','done',1)", [])
                .unwrap();
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
            db.conn
                .execute(
                    "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,'request','k',x'00',?2)",
                    rusqlite::params![id, created],
                )
                .unwrap();
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
            db.conn
                .query_row("SELECT status, ended_at, error_code FROM spans WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
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
            db.conn
                .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t',?1,'n','done',1)", [kind])
                .unwrap();
        }
        assert!(db
            .conn
            .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('x','t','banana','n','done',1)", [])
            .is_err());
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
                rec.finish_span(SpanFinish {
                    id: format!("s{i:03}"),
                    status: "done".into(),
                    ended_at: 101,
                    tokens_in: Some(i),
                    ..Default::default()
                });
            }
        }
        rec.flush().await;
        let conn = Connection::open(dir.join("audit.db")).unwrap();
        let ids: Vec<String> =
            conn.prepare("SELECT id FROM spans ORDER BY rowid").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(ids, (0..200).map(|i| format!("s{i:03}")).collect::<Vec<_>>());
        let done: i64 =
            conn.query_row("SELECT COUNT(*) FROM spans WHERE status = 'done' AND tokens_in IS NOT NULL", [], |r| r.get(0)).unwrap();
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
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                // let it pick the first up
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
        assert_eq!(
            row(&attempt),
            ("gen-1".into(), Some(turn.clone()), "attempt".into(), "done".into(), Some(5), None),
            "the first finish stands"
        );
        assert_eq!(row(&gate).1.as_deref(), Some(turn.as_str()));
        let (user, client, provider, model): (String, String, String, String) = (
            conn.query_row("SELECT user_id FROM spans WHERE id = ?1", [&turn], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT client_id FROM spans WHERE id = ?1", [&turn], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT provider_id FROM spans WHERE id = ?1", [&attempt], |r| r.get(0)).unwrap(),
            conn.query_row("SELECT model FROM spans WHERE id = ?1", [&attempt], |r| r.get(0)).unwrap(),
        );
        assert_eq!((user.as_str(), client.as_str(), provider.as_str(), model.as_str()), ("u1", "d1", "p1", "m1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stats_count_attempts_and_retries_per_provider() {
        let db = AuditDb::open_in_memory().unwrap();
        let now = now_secs();
        let span = |id: &str, trace: &str, kind: &str, provider: Option<&str>, status: &str, attrs: &str| {
            db.conn
                .execute(
                    "INSERT INTO spans (id, trace_id, kind, name, status, started_at, provider_id, attrs_json) VALUES (?1,?2,?3,'n',?4,?5,?6,?7)",
                    rusqlite::params![id, trace, kind, status, now, provider, attrs],
                )
                .unwrap();
        };
        // One request on p1: two tries failed, the third (a second retry) served it. A second
        // request on p2 failed once and gave up.
        span("t1", "T", "turn", None, "done", "{}");
        span("a1", "T", "attempt", Some("p1"), "error", r#"{"retry_index":0}"#);
        span("a2", "T", "attempt", Some("p1"), "error", r#"{"retry_index":1}"#);
        span("a3", "T", "attempt", Some("p1"), "done", r#"{"retry_index":2}"#);
        span("t2", "U", "turn", None, "error", "{}");
        span("b1", "U", "attempt", Some("p2"), "error", r#"{"retry_index":0}"#);
        let v = stats(&db.conn, now - 10, &|_, _, _, _| None).unwrap();
        let by = |p: &str| {
            v["by_provider"].as_array().unwrap().iter().find(|x| x["provider_id"] == p).cloned().unwrap_or_else(|| panic!("no row for {p}"))
        };
        let nums = |row: &serde_json::Value| ["attempts", "retries", "failed_attempts"].map(|k| row[k].as_i64().unwrap());
        assert_eq!(nums(&by("p1")), [3, 2, 2], "p1 had three tries, two of them retries, two failed");
        assert_eq!(nums(&by("p2")), [1, 0, 1], "p2 had one try, and it failed");
        assert!(by("p1").get("fallbacks").is_none(), "there is no fallback to count");
        // Outside the window nothing counts.
        let later = stats(&db.conn, now + 100, &|_, _, _, _| None).unwrap();
        assert!(later["by_provider"].as_array().unwrap().is_empty());
    }

    #[test]
    fn stats_give_tokens_by_day_and_provider_so_whoever_has_the_prices_can_price_each_day() {
        let db = AuditDb::open_in_memory().unwrap();
        let now = now_secs();
        let turn = |id: &str, trace: &str, tin: Option<i64>, tout: Option<i64>| {
            db.conn
                .execute(
                    "INSERT INTO spans (id, trace_id, kind, name, status, started_at, tokens_in, tokens_out) VALUES (?1,?2,'turn','n','done',?3,?4,?5)",
                    rusqlite::params![id, trace, now, tin, tout],
                )
                .unwrap();
            db.conn
                .execute(
                    "INSERT INTO spans (id, trace_id, kind, name, status, started_at, provider_id) VALUES (?1,?2,'attempt','n','done',?3,?4)",
                    rusqlite::params![format!("a-{id}"), trace, now, if trace == "T3" { "p2" } else { "p1" }],
                )
                .unwrap();
        };
        turn("t1", "T1", Some(100), Some(10));
        turn("t2", "T2", Some(50), Some(5));
        turn("t3", "T3", None, None); // no counts: nothing to price
        let v = stats(&db.conn, now - 10, &|_, _, _, _| None).unwrap();
        let rows = v["by_day_provider"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "{rows:?}");
        let p1 = rows.iter().find(|r| r["provider_id"] == "p1").unwrap();
        assert_eq!((p1["requests"].as_i64(), p1["requests_with_tokens"].as_i64()), (Some(2), Some(2)));
        assert_eq!((p1["tokens"]["in"].as_i64(), p1["tokens"]["out"].as_i64()), (Some(150), Some(15)));
        assert_eq!(p1["day"].as_str().unwrap().len(), 10, "YYYY-MM-DD");
        let p2 = rows.iter().find(|r| r["provider_id"] == "p2").unwrap();
        assert_eq!((p2["requests"].as_i64(), p2["requests_with_tokens"].as_i64()), (Some(1), Some(0)));
        // The existing groups say how many of their requests had counts too.
        assert_eq!(v["by_provider"].as_array().unwrap().iter().map(|r| r["requests_with_tokens"].as_i64().unwrap()).sum::<i64>(), 2);
        assert_eq!(v["by_day"][0]["requests_with_tokens"], 2);
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
        assert_eq!(
            (a["retry_index"].as_i64(), a["latency_ms"].as_i64(), a["keep"].as_str()),
            (Some(1), Some(31), Some("finish")),
            "start attributes survive, finish ones are added or win"
        );
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
            db.conn
                .execute(
                    "INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES (?1,'t',?2,'n','done',1)",
                    rusqlite::params![k.as_str(), k.as_str()],
                )
                .unwrap();
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
                conn.execute(
                    "INSERT INTO spans (id, trace_id, kind, name, status, started_at, ended_at) VALUES (?1,'t','turn','c','done',?2,?2)",
                    rusqlite::params![id, real - age],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES (?1,'reply','k',x'00',?2)",
                    rusqlite::params![id, real - age],
                )
                .unwrap();
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

    // ------------------------------------------------------------ the auditor's side (ADR 0051)

    fn temp_recorder(capacity: usize) -> (Recorder, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("askbar-audit-ack-{}", ulid::Ulid::new()));
        let db = AuditDb::open(&dir.join("audit.db")).unwrap();
        (Recorder::spawn(db, capacity), dir)
    }

    fn started(id: &str, trace: &str, kind: &str, service: &str, boot: &str) -> SpanStart {
        SpanStart {
            id: id.into(),
            trace_id: trace.into(),
            kind: kind.into(),
            name: "n".into(),
            started_at: now_secs(),
            started_ms: now_ms(),
            service: Some(service.into()),
            boot_id: Some(boot.into()),
            ..Default::default()
        }
    }

    fn count(rec: &Recorder, sql: &str) -> i64 {
        rec.reader().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[tokio::test]
    async fn an_acknowledged_append_is_committed_when_it_answers_and_a_retry_records_it_once() {
        let (rec, dir) = temp_recorder(64);
        let start = started("s1", "t1", "turn", "server", "b1");
        assert_eq!(rec.append(Record::Start(start.clone())).await, Ok(0));
        // Committed already: a separate reader sees it, with no flush in between.
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 's1' AND service = 'server' AND boot_id = 'b1'"), 1);
        assert_eq!(rec.append(Record::Start(start)).await, Ok(0), "a retry of a stored record is fine");
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 1);

        let finish = SpanFinish { tokens_in: Some(5), ..SpanFinish::new("s1", "done") };
        assert_eq!(rec.append(Record::Finish(finish.clone())).await, Ok(0));
        assert_eq!(rec.append(Record::Finish(finish)).await, Ok(0));
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE status = 'done' AND tokens_in = 5"), 1);

        let payload = PayloadWrite {
            span_id: "s1".into(),
            role: "reply".into(),
            key_id: "k".into(),
            ciphertext: vec![1, 2, 3],
            created_at: now_secs(),
        };
        assert_eq!(rec.append(Record::Payload(payload.clone())).await, Ok(0));
        assert_eq!(rec.append(Record::Payload(payload)).await, Ok(0));
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM payloads"), 1, "the same sealed bytes twice are one payload");

        assert_eq!(rec.append(Record::Finish(SpanFinish::new("nope", "done"))).await, Err(AppendError::NoSuchSpan));
        let orphan =
            PayloadWrite { span_id: "nope".into(), role: "reply".into(), key_id: "k".into(), ciphertext: vec![9], created_at: now_secs() };
        assert_eq!(rec.append(Record::Payload(orphan)).await, Err(AppendError::NoSuchSpan));
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM payloads"), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn hello_ends_that_services_earlier_running_spans_and_nobody_elses() {
        let (rec, dir) = temp_recorder(64);
        for (id, svc, boot) in [("a1", "server", "b1"), ("a2", "server", "b1"), ("a3", "server", "b2"), ("g1", "model-gateway", "b1")] {
            rec.append(Record::Start(started(id, "t", "gate", svc, boot))).await.unwrap();
        }
        rec.append(Record::Finish(SpanFinish::new("a2", "done"))).await.unwrap();
        let hello = |svc: &str, boot: &str| Record::Hello { service: svc.into(), boot_id: boot.into() };
        assert_eq!(rec.append(hello("server", "b2")).await, Ok(1), "only a1: a2 is finished, a3 is this process's");
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 'a1' AND status = 'interrupted' AND error_code = 'interrupted'"), 1);
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE status = 'running'"), 2, "a3 and the gateway's g1");
        assert_eq!(rec.append(hello("server", "b2")).await, Ok(0), "a second hello ends nothing");
        assert_eq!(rec.append(hello("model-gateway", "b9")).await, Ok(1));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn what_is_queued_together_is_committed_together_and_every_caller_is_answered() {
        let (rec, dir) = temp_recorder(64);
        rec.stall_writer().await;
        let tasks: Vec<_> = (0..20)
            .map(|i| {
                let rec = rec.clone();
                tokio::spawn(async move { rec.append(Record::Start(started(&format!("s{i:02}"), "t", "gate", "server", "b"))).await })
            })
            .collect();
        // Not committed while the writer is held.
        for _ in 0..50 {
            if rec.backlog().queued >= 21 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 0);
        rec.pause_writer(false);
        for t in tasks {
            assert_eq!(t.await.unwrap(), Ok(0));
        }
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 20);
        assert_eq!(rec.backlog().queued, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_backlog_counts_queued_records_and_how_long_the_oldest_has_waited() {
        let (rec, dir) = temp_recorder(8);
        assert_eq!(rec.backlog(), Backlog { queued: 0, oldest: Duration::ZERO });
        rec.stall_writer().await;
        let held = rec.clone();
        let task = tokio::spawn(async move { held.append(Record::Start(started("s1", "t", "gate", "server", "b"))).await });
        for _ in 0..100 {
            if rec.backlog().queued >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
        let b = rec.backlog();
        assert_eq!(b.queued, 2, "the event the writer holds and the one behind it");
        assert!(b.oldest >= Duration::from_millis(100), "{b:?}");
        rec.pause_writer(false);
        assert_eq!(task.await.unwrap(), Ok(0));
        assert_eq!(rec.backlog().queued, 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_full_queue_refuses_an_append_and_records_nothing() {
        let (rec, dir) = temp_recorder(2);
        rec.stall_writer().await;
        let tasks: Vec<_> = (0..2)
            .map(|i| {
                let rec = rec.clone();
                tokio::spawn(async move { rec.append(Record::Start(started(&format!("s{i}"), "t", "gate", "server", "b"))).await })
            })
            .collect();
        for _ in 0..100 {
            if !rec.healthy() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(rec.append(Record::Start(started("s9", "t", "gate", "server", "b"))).await, Err(AppendError::Full));
        rec.pause_writer(false);
        for t in tasks {
            assert_eq!(t.await.unwrap(), Ok(0));
        }
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans WHERE id = 's9'"), 0);
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn an_admin_action_is_stored_once_with_its_caller_and_can_never_be_changed_or_deleted() {
        let (rec, dir) = temp_recorder(16);
        let action = AdminActionWrite {
            id: "01A".into(),
            at: now_secs(),
            service: "admin".into(),
            category: "settings".into(),
            action: "kill switch on".into(),
            outcome: "done".into(),
            detail_json: Some(r#"{"kill_switch":true}"#.into()),
        };
        assert_eq!(rec.append(Record::AdminAction(action.clone())).await, Ok(0));
        assert_eq!(rec.append(Record::AdminAction(action)).await, Ok(0));
        let conn = rec.reader().unwrap();
        let rows = list_admin_actions(&conn, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0]["service"].clone(), rows[0]["detail"]["kill_switch"].clone()), ("admin".into(), true.into()));
        // The store itself refuses a change or a delete, for any connection.
        let db = AuditDb::open(&dir.join("audit.db")).unwrap();
        assert!(db.conn().execute("UPDATE admin_actions SET outcome = 'done' WHERE id = '01A'", []).is_err());
        assert!(db.conn().execute("DELETE FROM admin_actions", []).is_err());
        rec.prune(30, now_secs()).await.unwrap();
        assert!(db.conn().execute("UPDATE prunes SET spans = 99", []).is_err(), "a prune record cannot be changed");
        assert!(db.conn().execute("DELETE FROM prunes", []).is_err(), "nor deleted");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_prune_writes_its_record_with_the_delete_and_the_record_stays() {
        let dir = std::env::temp_dir().join(format!("askbar-audit-prune-{}", ulid::Ulid::new()));
        let db = AuditDb::open(&dir.join("audit.db")).unwrap();
        let old = now_secs() - 40 * 86_400;
        db.conn
            .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('old','t','turn','n','done',?1)", [old])
            .unwrap();
        db.conn
            .execute("INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES ('old','reply','k',x'01',?1)", [old])
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('young','t','turn','n','done',?1)",
                [now_secs()],
            )
            .unwrap();
        let rec = Recorder::spawn(db, 16);
        assert_eq!(rec.prune(30, now_secs()).await, Ok((1, 1)));
        assert_eq!(count(&rec, "SELECT COUNT(*) FROM spans"), 1, "only the young one is left");
        let prunes = list_prunes(&rec.reader().unwrap()).unwrap();
        assert_eq!(prunes.len(), 1);
        assert_eq!((prunes[0]["spans"].clone(), prunes[0]["payloads"].clone()), (1.into(), 1.into()));
        assert!(prunes[0]["cutoff"].as_i64().unwrap() <= now_secs() - 29 * 86_400);
        // Nothing to delete is still a prune, and still has its record.
        assert_eq!(rec.prune(30, now_secs()).await, Ok((0, 0)));
        assert_eq!(list_prunes(&rec.reader().unwrap()).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn token_counts_group_by_the_provider_and_model_of_the_last_attempt_and_hold_no_prices() {
        let (rec, dir) = temp_recorder(64);
        let mut n = 0;
        for (prov, model, tin, tout) in [("p1", "m1", 10, 20), ("p1", "m1", 5, 5), ("p2", "m9", 1, 2)] {
            n += 1;
            let (turn, attempt) = (format!("t{n}"), format!("a{n}"));
            rec.append(Record::Start(started(&turn, &turn, "turn", "model-gateway", "b"))).await.unwrap();
            let mut a = started(&attempt, &turn, "attempt", "model-gateway", "b");
            a.provider_id = Some(prov.into());
            a.model = Some(model.into());
            rec.append(Record::Start(a)).await.unwrap();
            rec.append(Record::Finish(SpanFinish {
                tokens_in: Some(tin),
                tokens_out: Some(tout),
                tokens_cached: Some(0),
                ..SpanFinish::new(&turn, "done")
            }))
            .await
            .unwrap();
        }
        let rows = token_counts(&rec.reader().unwrap(), 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0],
            serde_json::json!({ "provider_id": "p1", "model": "m1", "requests": 2, "tokens_in": 15, "tokens_out": 25, "tokens_cached": 0 })
        );
        assert_eq!(rows[1]["provider_id"], "p2");
        assert!(rows.iter().all(|r| r.get("cost").is_none()), "the auditor holds no prices");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn a_setting_is_stored_through_the_writer_and_replaced() {
        let (rec, dir) = temp_recorder(8);
        assert_eq!(get_setting(&rec.reader().unwrap(), "retention_days").unwrap(), None);
        rec.append(Record::Setting { key: "retention_days".into(), value: "45".into() }).await.unwrap();
        rec.append(Record::Setting { key: "retention_days".into(), value: "60".into() }).await.unwrap();
        assert_eq!(get_setting(&rec.reader().unwrap(), "retention_days").unwrap().as_deref(), Some("60"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
