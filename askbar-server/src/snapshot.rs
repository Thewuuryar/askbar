//! The admin service's configuration snapshot, as the server stores and reads it.
//!
//! The admin service owns the configuration and pushes it whole with
//! `PUT /api/server/v1/snapshot`. That handler is the only writer (`save` is private to this
//! module); everything else gets `current`, which only reads.

use crate::Db;
use askbar_proto::{DisplaySettings, Snapshot};
use rusqlite::{params, OptionalExtension};

/// Audit records are never kept for less than this many days.
pub const MIN_AUDIT_RETENTION_DAYS: u32 = 7;

/// A generation may be given between 30 seconds and an hour.
pub const MIN_GENERATION_TIMEOUT_SECS: u32 = 30;
pub const MAX_GENERATION_TIMEOUT_SECS: u32 = 3600;

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

/// The public key must be 32 bytes in hex, and its id must be the one derived from it, so a
/// payload's `key_id` can always be trusted to name the key that sealed it.
fn check_audit_key(k: &askbar_proto::AuditKey) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let bytes = parse_hex(&k.public_key).filter(|b| b.len() == 32).ok_or("audit_public_key.public_key must be 32 bytes in hex")?;
    let id: String = Sha256::digest(&bytes)[..8].iter().map(|b| format!("{b:02x}")).collect();
    if id != k.key_id {
        return Err("audit_public_key.key_id does not match the key".into());
    }
    Ok(())
}

/// Hex to bytes; `None` for odd length or a non-hex character.
pub fn parse_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.is_ascii() {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect()
}

/// Why a snapshot can't be accepted, in words for the admin service's log.
pub fn validate(s: &Snapshot) -> Result<(), String> {
    if s.audit_retention_days < MIN_AUDIT_RETENTION_DAYS {
        return Err(format!("audit_retention_days must be at least {MIN_AUDIT_RETENTION_DAYS}"));
    }
    if !(MIN_GENERATION_TIMEOUT_SECS..=MAX_GENERATION_TIMEOUT_SECS).contains(&s.generation_timeout_secs) {
        return Err(format!("generation_timeout_secs must be between {MIN_GENERATION_TIMEOUT_SECS} and {MAX_GENERATION_TIMEOUT_SECS}"));
    }
    if let Some(k) = &s.audit_public_key {
        check_audit_key(k)?;
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
        for (label, price) in [("price_in", p.price_in), ("price_out", p.price_out), ("price_cached_in", p.price_cached_in)] {
            if price.is_some_and(|v| !v.is_finite() || v < 0.0) {
                return Err(format!("provider {}: {label} must be zero or more", p.id));
            }
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
    let row: Option<(i64, String)> =
        db.conn().query_row("SELECT version, body FROM config_snapshot WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    Ok(row.and_then(|(version, body)| {
        let mut s: Snapshot = serde_json::from_str(&body).ok()?;
        s.version = version as u64;
        Some(s)
    }))
}

/// Store `s` as the new snapshot and return its version. The caller is the server handler.
fn save(db: &Db, s: &Snapshot) -> rusqlite::Result<u64> {
    let tx = db.conn().unchecked_transaction()?;
    let previous: Option<i64> = tx.query_row("SELECT version FROM config_snapshot WHERE id = 1", [], |r| r.get(0)).optional()?;
    let version = previous.unwrap_or(0) + 1;
    let body = serde_json::to_string(&Snapshot { version: version as u64, ..s.clone() })
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    tx.execute(
        "INSERT INTO config_snapshot (id, version, body, updated_at) VALUES (1, ?1, ?2, ?3)
         ON CONFLICT(id) DO UPDATE SET version = excluded.version, body = excluded.body, updated_at = excluded.updated_at",
        params![version, body, crate::auth::now()],
    )?;
    tx.commit()?;
    Ok(version as u64)
}

#[derive(Debug)]
pub enum ApplyError {
    /// The snapshot is not acceptable; the text says why. Nothing was stored.
    Invalid(String),
    Store(rusqlite::Error),
}

/// Validate and store `s` as the new snapshot, and drop the stored keys of providers it no
/// longer lists. Returns the version the server assigned. This is what `PUT /api/server/v1/snapshot`
/// does; tests use it to put the server in a known state.
pub fn apply(db: &Db, s: &Snapshot) -> Result<u64, ApplyError> {
    validate(s).map_err(ApplyError::Invalid)?;
    let version = save(db, s).map_err(ApplyError::Store)?;
    let keep: std::collections::BTreeSet<&str> = s.providers.iter().map(|p| p.id.as_str()).collect();
    let stored: Vec<String> =
        db.conn().prepare("SELECT id FROM providers").and_then(|mut st| st.query_map([], |r| r.get(0))?.collect()).unwrap_or_default();
    for id in stored.iter().filter(|id| !keep.contains(id.as_str())) {
        let _ = db.conn().execute("DELETE FROM providers WHERE id = ?1", [id]);
    }
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_prices_must_not_be_negative_and_billing_fields_pass_through() {
        let mut s = crate::testing::empty_snapshot();
        s.providers.push(crate::testing::provider("p1", "one", "http://x/v1", "m"));
        assert_eq!(validate(&s), Ok(()), "no prices at all is fine: cost is then unknown");
        s.providers[0].price_in = Some(0.0);
        s.providers[0].price_out = Some(15.5);
        s.providers[0].price_cached_in = Some(0.3);
        s.providers[0].billing = askbar_proto::Billing::Subscription;
        s.providers[0].allow_as_fallback = true;
        assert_eq!(validate(&s), Ok(()), "zero is a price (free)");
        for set in [
            (|p: &mut askbar_proto::SnapshotProvider| p.price_in = Some(-0.01)) as fn(&mut askbar_proto::SnapshotProvider),
            |p| p.price_out = Some(-1.0),
            |p| p.price_cached_in = Some(-5.0),
            |p| p.price_in = Some(f64::INFINITY),
            |p| p.price_out = Some(f64::NAN),
        ] {
            let mut bad = s.clone();
            set(&mut bad.providers[0]);
            let e = validate(&bad).unwrap_err();
            assert!(e.contains("zero or more") && e.contains("p1"), "{e}");
        }
    }

    #[test]
    fn the_audit_public_key_must_be_32_hex_bytes_with_its_own_id() {
        use sha2::{Digest, Sha256};
        let public = [5u8; 32];
        let key = |public_hex: &str, id: &str| askbar_proto::AuditKey { key_id: id.into(), public_key: public_hex.into() };
        let hex: String = public.iter().map(|b| format!("{b:02x}")).collect();
        let id: String = Sha256::digest(public)[..8].iter().map(|b| format!("{b:02x}")).collect();
        let mut s = crate::testing::empty_snapshot();
        assert_eq!(validate(&s), Ok(()), "no key is fine; the server just refuses to start replies");
        s.audit_public_key = Some(key(&hex, &id));
        assert_eq!(validate(&s), Ok(()));
        for bad in [key(&hex, "0000000000000000"), key(&hex[..62], &id), key("zz".repeat(32).as_str(), &id), key("", &id)] {
            s.audit_public_key = Some(bad.clone());
            assert!(validate(&s).is_err(), "{bad:?}");
        }
        assert_eq!(parse_hex("00ff"), Some(vec![0, 255]));
        assert_eq!(parse_hex("0"), None);
    }

    #[test]
    fn the_generation_timeout_must_be_between_30_and_3600_seconds() {
        let mut s = crate::testing::empty_snapshot();
        for ok in [30, 300, 3600] {
            s.generation_timeout_secs = ok;
            assert_eq!(validate(&s), Ok(()), "{ok} is allowed");
        }
        for bad in [0, 29, 3601, 86_400] {
            s.generation_timeout_secs = bad;
            let e = validate(&s).unwrap_err();
            assert!(e.contains("generation_timeout_secs") && e.contains("30") && e.contains("3600"), "{bad}: {e}");
        }
    }
}
