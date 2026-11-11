//! The hash chain over the audit store (ADR 0010 rule 7, ADR 0051, ADR 0052).
//!
//! Every record the store takes (a span's start, a span's finish, a sealed payload, an admin action, a prune) adds one
//! entry to the `chain` table in the same transaction: its kind, the record's id, the time retention goes by, a digest of
//! the record's fields as stored, and the hash of the entry before it. An entry's hash covers all of those, so changing
//! or removing an entry breaks every hash after it, and changing a record breaks its own digest. Entries are never
//! changed or deleted, not even by a prune: they hold digests, not content. A prune removes spans and payloads, and the
//! chain says so in its own prune entry, which is how a missing record is told from a deleted one.
//!
//! What the chain proves: that nothing in this file was edited, inserted or deleted without breaking it. What it does not
//! prove: that a caller told the truth, or that a file replaced by an older copy of itself is the newest one. Only a head
//! kept somewhere else shows that, which is why the admin keeps the latest head and checks it ([`head`], [`entry_hash`]).

use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// The hash before the first entry.
pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The newest entry: its number (0 when the chain is empty) and its hash.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Head {
    pub seq: i64,
    pub hash: String,
}

/// One thing a check found wrong.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Problem {
    /// The entry it concerns, when there is one (a record with no entry has none).
    pub seq: Option<i64>,
    pub kind: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub reason: String,
}

/// What a full check found. `problems` is cut at [`MAX_PROBLEMS`]; `more` says whether there were more.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub ok: bool,
    pub entries: i64,
    pub head: Head,
    pub problems: Vec<Problem>,
    pub more: bool,
}

pub const MAX_PROBLEMS: usize = 20;

enum Part<'a> {
    Text(&'a str),
    Int(i64),
    Blob(&'a [u8]),
    Null,
}

/// SHA-256 over each part with a tag and a length in front of it, so two different lists of parts never give the same
/// bytes.
fn digest(parts: &[Part]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        match p {
            Part::Text(s) => {
                h.update([1u8]);
                h.update((s.len() as u64).to_le_bytes());
                h.update(s.as_bytes());
            }
            Part::Int(n) => {
                h.update([2u8]);
                h.update(n.to_le_bytes());
            }
            Part::Blob(b) => {
                h.update([3u8]);
                h.update((b.len() as u64).to_le_bytes());
                h.update(b);
            }
            Part::Null => h.update([0u8]),
        }
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn text(v: &Option<String>) -> Part<'_> {
    v.as_deref().map_or(Part::Null, Part::Text)
}

fn int(v: Option<i64>) -> Part<'static> {
    v.map_or(Part::Null, Part::Int)
}

fn entry_hash_of(prev: &str, seq: i64, kind: &str, reference: &str, row_at: i64, record_digest: &str) -> String {
    digest(&[Part::Text(prev), Part::Int(seq), Part::Text(kind), Part::Text(reference), Part::Int(row_at), Part::Text(record_digest)])
}

/// A record's digest and the time retention goes by for it, as the record is stored now. `None` when it is not there.
/// A span's start covers what never changes after it; its finish covers what the finish set.
fn row_digest(conn: &Connection, kind: &str, reference: &str) -> rusqlite::Result<Option<(String, i64)>> {
    match kind {
        "span_start" => conn
            .query_row(
                "SELECT id, trace_id, parent_id, kind, name, started_at, user_id, client_id, service, boot_id FROM spans WHERE id = ?1",
                [reference],
                |r| {
                    let (id, trace, parent, k, name, at): (String, String, Option<String>, String, String, i64) =
                        (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
                    let (user, client, service, boot): (Option<String>, Option<String>, Option<String>, Option<String>) =
                        (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?);
                    let d = digest(&[
                        Part::Text(&id),
                        Part::Text(&trace),
                        text(&parent),
                        Part::Text(&k),
                        Part::Text(&name),
                        Part::Int(at),
                        text(&user),
                        text(&client),
                        text(&service),
                        text(&boot),
                    ]);
                    Ok((d, at))
                },
            )
            .optional(),
        "span_finish" => conn
            .query_row(
                "SELECT id, started_at, status, ended_at, provider_id, model, tokens_in, tokens_out, tokens_cached, ttft_ms, error_code, attrs_json
                 FROM spans WHERE id = ?1",
                [reference],
                |r| {
                    let (id, at, status): (String, i64, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
                    let ended: Option<i64> = r.get(3)?;
                    let (provider, model): (Option<String>, Option<String>) = (r.get(4)?, r.get(5)?);
                    let (tin, tout, tcached, ttft): (Option<i64>, Option<i64>, Option<i64>, Option<i64>) =
                        (r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?);
                    let (error, attrs): (Option<String>, Option<String>) = (r.get(10)?, r.get(11)?);
                    let d = digest(&[
                        Part::Text(&id),
                        Part::Text(&status),
                        int(ended),
                        text(&provider),
                        text(&model),
                        int(tin),
                        int(tout),
                        int(tcached),
                        int(ttft),
                        text(&error),
                        text(&attrs),
                    ]);
                    Ok((d, at))
                },
            )
            .optional(),
        "payload" => conn
            .query_row(
                "SELECT span_id, role, key_id, ciphertext, created_at FROM payloads WHERE rowid = CAST(?1 AS INTEGER)",
                [reference],
                |r| {
                    let (span, role, key): (String, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?);
                    let blob: Vec<u8> = r.get(3)?;
                    let at: i64 = r.get(4)?;
                    Ok((digest(&[Part::Text(&span), Part::Text(&role), Part::Text(&key), Part::Blob(&blob), Part::Int(at)]), at))
                },
            )
            .optional(),
        "admin_action" => conn
            .query_row("SELECT id, at, service, category, action, outcome, detail_json FROM admin_actions WHERE id = ?1", [reference], |r| {
                let (id, at, service, category, action, outcome): (String, i64, String, String, String, String) =
                    (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?);
                let detail: Option<String> = r.get(6)?;
                Ok((
                    digest(&[
                        Part::Text(&id),
                        Part::Int(at),
                        Part::Text(&service),
                        Part::Text(&category),
                        Part::Text(&action),
                        Part::Text(&outcome),
                        text(&detail),
                    ]),
                    at,
                ))
            })
            .optional(),
        "access_log" => conn
            .query_row(
                "SELECT at, method, host, path, status, bytes, duration_ms, remote_ip, proto, user_agent FROM access_logs WHERE id = CAST(?1 AS INTEGER)",
                [reference],
                |r| {
                    let at: i64 = r.get(0)?;
                    let (method, host, path): (String, String, String) = (r.get(1)?, r.get(2)?, r.get(3)?);
                    let (status, bytes, ms): (i64, i64, i64) = (r.get(4)?, r.get(5)?, r.get(6)?);
                    let (ip, proto): (String, String) = (r.get(7)?, r.get(8)?);
                    let agent: Option<String> = r.get(9)?;
                    Ok((
                        digest(&[
                            Part::Int(at),
                            Part::Text(&method),
                            Part::Text(&host),
                            Part::Text(&path),
                            Part::Int(status),
                            Part::Int(bytes),
                            Part::Int(ms),
                            Part::Text(&ip),
                            Part::Text(&proto),
                            text(&agent),
                        ]),
                        at,
                    ))
                },
            )
            .optional(),
        "prune" => conn
            .query_row("SELECT at, cutoff, spans, payloads FROM prunes WHERE id = CAST(?1 AS INTEGER)", [reference], |r| {
                let (at, cutoff, spans, payloads): (i64, i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
                Ok((digest(&[Part::Int(at), Part::Int(cutoff), Part::Int(spans), Part::Int(payloads)]), at))
            })
            .optional(),
        _ => Ok(None),
    }
}

/// The newest entry's number and hash (`0` and [`GENESIS`] for an empty chain).
pub fn head(conn: &Connection) -> rusqlite::Result<Head> {
    let row = conn
        .query_row("SELECT seq, hash FROM chain ORDER BY seq DESC LIMIT 1", [], |r| Ok(Head { seq: r.get(0)?, hash: r.get(1)? }))
        .optional()?;
    Ok(row.unwrap_or(Head { seq: 0, hash: GENESIS.to_string() }))
}

/// The hash of entry `seq`, if there is one.
pub fn entry_hash(conn: &Connection, seq: i64) -> rusqlite::Result<Option<String>> {
    conn.query_row("SELECT hash FROM chain WHERE seq = ?1", [seq], |r| r.get(0)).optional()
}

/// Adds the entry for a record that was just written, in the writer's own transaction. A record that is not there (a
/// write that changed nothing) adds nothing.
pub(crate) fn record(conn: &Connection, kind: &str, reference: &str) -> rusqlite::Result<()> {
    let Some((d, row_at)) = row_digest(conn, kind, reference)? else { return Ok(()) };
    add(conn, kind, reference, row_at, &d)
}

fn add(conn: &Connection, kind: &str, reference: &str, row_at: i64, record_digest: &str) -> rusqlite::Result<()> {
    let prev = head(conn)?;
    let seq = prev.seq + 1;
    let hash = entry_hash_of(&prev.hash, seq, kind, reference, row_at, record_digest);
    conn.execute(
        "INSERT INTO chain (seq, kind, ref, row_at, digest, prev_hash, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![seq, kind, reference, row_at, record_digest, prev.hash, hash],
    )?;
    Ok(())
}

/// Chains what the file held before the chain existed (the migration that adds it calls this once): every record, oldest
/// first. The chain proves nothing about those records from before that day.
pub(crate) fn backfill(conn: &Connection) -> rusqlite::Result<()> {
    // (time, order within the same second, kind, ref)
    let mut all: Vec<(i64, u8, &'static str, String)> = Vec::new();
    let mut collect = |sql: &str, order: u8, kind: &'static str| -> rusqlite::Result<()> {
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (at, id) = row?;
            all.push((at, order, kind, id));
        }
        Ok(())
    };
    collect("SELECT started_at, id FROM spans", 0, "span_start")?;
    collect("SELECT started_at, id FROM spans WHERE status <> 'running'", 1, "span_finish")?;
    collect("SELECT created_at, CAST(rowid AS TEXT) FROM payloads", 2, "payload")?;
    collect("SELECT at, id FROM admin_actions", 3, "admin_action")?;
    collect("SELECT at, CAST(id AS TEXT) FROM prunes", 4, "prune")?;
    all.sort();
    for (_, _, kind, reference) in all {
        record(conn, kind, &reference)?;
    }
    Ok(())
}

/// Tests only: chain what a hand-made file holds, as the migration does.
#[doc(hidden)]
pub fn backfill_for_tests(conn: &Connection) -> rusqlite::Result<()> {
    backfill(conn)
}

/// Tests only: add the entry for a record a test wrote by hand.
#[doc(hidden)]
pub fn record_for_tests(conn: &Connection, kind: &str, reference: &str) -> rusqlite::Result<()> {
    record(conn, kind, reference)
}

/// A full check: the links, every record against its digest, a record with no entry, and an entry with no record that
/// no prune explains.
pub fn verify(conn: &Connection) -> rusqlite::Result<Report> {
    let mut problems: Vec<Problem> = Vec::new();
    let mut more = false;
    let mut note = |p: Problem, problems: &mut Vec<Problem>| {
        if problems.len() < MAX_PROBLEMS {
            problems.push(p);
        } else {
            more = true;
        }
    };

    // The prunes: where each is in the chain and what it cut off.
    let prunes: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT chain.seq, prunes.cutoff FROM chain JOIN prunes ON chain.kind = 'prune' AND chain.ref = CAST(prunes.id AS TEXT)",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let (mut prev_seq, mut prev_hash, mut entries) = (0i64, GENESIS.to_string(), 0i64);
    let mut stmt = conn.prepare("SELECT seq, kind, ref, row_at, digest, prev_hash, hash FROM chain ORDER BY seq")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (seq, kind, reference, row_at, recorded, prev, hash): (i64, String, String, i64, String, String, String) =
            (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?);
        entries += 1;
        let bad = |reason: &str| Problem { seq: Some(seq), kind: kind.clone(), reference: reference.clone(), reason: reason.to_string() };
        if seq != prev_seq + 1 {
            note(bad("an entry before this one is missing"), &mut problems);
        }
        if prev != prev_hash {
            note(bad("it does not follow the entry before it"), &mut problems);
        }
        if entry_hash_of(&prev, seq, &kind, &reference, row_at, &recorded) != hash {
            note(bad("the entry itself was changed"), &mut problems);
        }
        match row_digest(conn, &kind, &reference)? {
            Some((d, _)) if d != recorded => note(bad("the record was changed after it was written"), &mut problems),
            Some(_) => {}
            None => {
                let pruned = matches!(kind.as_str(), "span_start" | "span_finish" | "payload" | "access_log")
                    && prunes.iter().any(|(ps, cutoff)| *ps > seq && *cutoff > row_at);
                if !pruned {
                    note(bad("the record is gone and no prune explains it"), &mut problems);
                }
            }
        }
        prev_seq = seq;
        prev_hash = hash;
    }
    drop(rows);
    drop(stmt);

    // Records nobody chained: put in by hand.
    for (kind, sql) in [
        ("span_start", "SELECT id FROM spans s WHERE NOT EXISTS (SELECT 1 FROM chain c WHERE c.kind = 'span_start' AND c.ref = s.id)"),
        ("span_finish", "SELECT id FROM spans s WHERE s.status <> 'running' AND NOT EXISTS (SELECT 1 FROM chain c WHERE c.kind = 'span_finish' AND c.ref = s.id)"),
        ("payload", "SELECT CAST(rowid AS TEXT) FROM payloads p WHERE NOT EXISTS (SELECT 1 FROM chain c WHERE c.kind = 'payload' AND c.ref = CAST(p.rowid AS TEXT))"),
        ("admin_action", "SELECT id FROM admin_actions a WHERE NOT EXISTS (SELECT 1 FROM chain c WHERE c.kind = 'admin_action' AND c.ref = a.id)"),
        ("access_log", "SELECT CAST(id AS TEXT) FROM access_logs a WHERE NOT EXISTS (SELECT 1 FROM chain c WHERE c.kind = 'access_log' AND c.ref = CAST(a.id AS TEXT))"),
        ("prune", "SELECT CAST(id AS TEXT) FROM prunes p WHERE NOT EXISTS (SELECT 1 FROM chain c WHERE c.kind = 'prune' AND c.ref = CAST(p.id AS TEXT))"),
    ] {
        let mut stmt = conn.prepare(sql)?;
        let ids: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for id in ids {
            note(Problem { seq: None, kind: kind.to_string(), reference: id, reason: "the record has no entry in the chain".to_string() }, &mut problems);
        }
    }

    let head = Head { seq: prev_seq, hash: prev_hash };
    Ok(Report { ok: problems.is_empty(), entries, head, problems, more })
}
