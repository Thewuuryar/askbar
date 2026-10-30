//! The admin service's configuration snapshot, as the server stores and reads it.
//!
//! The admin service owns the configuration and pushes it whole with
//! `PUT /mgmt/v1/snapshot`. That handler is the only writer (`save` is private to this
//! module); everything else gets `current`, which only reads.

use crate::Db;
use askbar_proto::{DisplaySettings, Snapshot};
use rusqlite::{params, OptionalExtension};

/// Audit records are never kept for less than this many days.
pub const MIN_AUDIT_RETENTION_DAYS: u32 = 7;

fn check_display(d: &DisplaySettings, whose: &str) -> Result<(), String> {
    let in_range = |v: u32, lo: u32, hi: u32| (lo..=hi).contains(&v);
    if !matches!(d.theme.as_str(), "auto" | "light" | "dark")
        || !in_range(d.width, 300, 4000)
        || !in_range(d.max_height, 200, 4000)
        || !in_range(d.history_width, 120, 1000)
        || !(0.0..=0.8).contains(&d.top_percent)
    {
        return Err(format!("{whose}: display settings are out of range"));
    }
    Ok(())
}

/// Why a snapshot can't be accepted, in words for the admin service's log.
pub fn validate(s: &Snapshot) -> Result<(), String> {
    if s.audit_retention_days < MIN_AUDIT_RETENTION_DAYS {
        return Err(format!("audit_retention_days must be at least {MIN_AUDIT_RETENTION_DAYS}"));
    }
    let mut ids = std::collections::BTreeSet::new();
    for p in &s.providers {
        if p.id.trim().is_empty() || !ids.insert(p.id.as_str()) {
            return Err("provider ids must be present and unique".into());
        }
        if !matches!(p.kind.as_str(), "openai" | "anthropic") {
            return Err(format!("provider {}: type must be openai or anthropic", p.id));
        }
        if p.name.trim().is_empty() || p.model.trim().is_empty() {
            return Err(format!("provider {}: name and model are required", p.id));
        }
    }
    for id in s.default_provider_id.iter().chain(s.fallback_order.iter()) {
        if !ids.contains(id.as_str()) {
            return Err(format!("{id} is named as a default or fallback provider but is not in providers"));
        }
    }
    check_display(&s.display, "display")?;
    for (user, d) in &s.user_display_defaults {
        check_display(d, user)?;
    }
    Ok(())
}

/// The providers in the order clients should offer them: the default, then the fallbacks
/// in order, then the rest by name.
pub fn in_client_order(s: &Snapshot) -> Vec<askbar_proto::SnapshotProvider> {
    let mut rest: Vec<_> = s.providers.clone();
    rest.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = Vec::new();
    for id in s.default_provider_id.iter().chain(s.fallback_order.iter()) {
        if let Some(pos) = rest.iter().position(|p| &p.id == id) {
            out.push(rest.remove(pos));
        }
    }
    out.extend(rest);
    out
}

/// The stored snapshot, or `None' before the admin service has pushed one. Read-only.
pub fn current(db: &Db) -> rusqlite::Result<Option<Snapshot>> {
    let row: Option<(i64, String)> = db
        .conn()
        .query_row("SELECT version, body FROM config_snapshot WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(row.and_then(|(version, body)| {
        let mut s: Snapshot = serde_json::from_str(&body).ok()?;
        s.version = version as u64;
        Some(s)
    }))
}

/// Store `s` as the new snapshot and return its version. The caller is the management handler.
pub(crate) fn save(db: &Db, s: &Snapshot) -> rusqlite::Result<u64> {
    let tx = db.conn().unchecked_transaction()?;
    let previous: Option<i64> = tx.query_row("SELECT version FROM config_snapshot WHERE id = 1", [], |r| r.get(0)).optional()?;
    let version = previous.unwrap_or(0) + 1;
    let body = serde_json::to_string(&Snapshot { version: version as u64, ..s.clone() }).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    tx.execute(
        "INSERT INTO config_snapshot (id, version, body, updated_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET version = excluded.version, body = excluded.body, updated_at = excluded.updated_at",
        params![version, body, crate::auth::now()],
    )?;
    tx.commit()?;
    Ok(version as u64)
}
