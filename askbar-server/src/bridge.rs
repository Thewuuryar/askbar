//! TEMPORARY. Until the admin moves out of the server (R4.13), the old admin site still
//! keeps its configuration in the `providers`, `settings`, `display_defaults`, `pair_codes`
//! and `devices` tables. This rebuilds the snapshot from them, at start-up and after every
//! change made through the old admin, so that everything else reads only the snapshot.
//! It is the one writer of the snapshot besides `PUT /mgmt/v1/snapshot`, and goes away
//! with the old admin.

use crate::{auth::now, snapshot, AppState, Db};
use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode, SnapshotProvider};
use axum::{extract::{Request, State}, middleware::Next, response::Response};
use rusqlite::OptionalExtension;
use std::sync::Arc;

fn setting(db: &Db, key: &str) -> rusqlite::Result<Option<String>> {
    db.conn().query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
}

/// Build a snapshot from the old tables. What those tables have no place for (retention,
/// budget, kill switch, default provider) is kept from the current snapshot, if any.
pub fn build(db: &Db) -> rusqlite::Result<Snapshot> {
    let previous = snapshot::current(db)?;
    let mut providers = Vec::new();
    {
        let mut stmt = db.conn().prepare("SELECT id, name, type, base_url, model, max_tokens, temperature, headers FROM providers ORDER BY name")?;
        let rows = stmt.query_map([], |r| {
            Ok(SnapshotProvider {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                base_url: r.get(3)?,
                model: r.get(4)?,
                max_tokens: r.get(5)?,
                temperature: r.get(6)?,
                headers: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or_default(),
            })
        })?;
        for p in rows {
            providers.push(p?);
        }
    }
    let display: DisplaySettings = setting(db, "display")?.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut user_display_defaults = std::collections::BTreeMap::new();
    {
        let mut stmt = db.conn().prepare("SELECT user_id, value FROM display_defaults")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (user, value) = row?;
            if let Ok(d) = serde_json::from_str(&value) {
                user_display_defaults.insert(user, d);
            }
        }
    }
    let mut pair_codes = Vec::new();
    {
        let mut stmt = db.conn().prepare("SELECT hash, user_id, device_name, expires_at FROM pair_codes WHERE used = 0 AND expires_at > ?1")?;
        let rows = stmt.query_map([now()], |r| {
            Ok(SnapshotPairCode { hash: r.get(0)?, user_id: r.get(1)?, device_name: r.get(2)?, expires_at: r.get(3)? })
        })?;
        for c in rows {
            pair_codes.push(c?);
        }
    }
    let revoked_device_ids = {
        let mut stmt = db.conn().prepare("SELECT id FROM devices WHERE revoked_at IS NOT NULL ORDER BY id")?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        ids
    };
    let keep = previous.as_ref();
    let default_provider_id = keep.and_then(|p| p.default_provider_id.clone()).filter(|id| providers.iter().any(|p| &p.id == id));
    let fallback_order = keep.map(|p| p.fallback_order.iter().filter(|id| providers.iter().any(|p| &&p.id == id)).cloned().collect()).unwrap_or_default();
    Ok(Snapshot {
        version: 0,
        providers,
        default_provider_id,
        fallback_order,
        system_prompt: setting(db, "system_prompt")?,
        display,
        user_display_defaults,
        pair_codes,
        revoked_device_ids,
        budget: keep.map(|p| p.budget.clone()).unwrap_or_else(Budget::default),
        kill_switch: keep.map(|p| p.kill_switch).unwrap_or(false),
        audit_retention_days: keep.map(|p| p.audit_retention_days).unwrap_or(30),
    })
}

/// Rebuild and store the snapshot. Returns its new version.
pub fn rebuild(db: &Db) -> rusqlite::Result<u64> {
    let s = build(db)?;
    snapshot::save(db, &s)
}

/// Middleware for the old admin site: after a successful change, rebuild the snapshot.
pub async fn after_admin_change(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let changes = req.uri().path().starts_with("/admin/api/") && !matches!(req.method().as_str(), "GET" | "HEAD" | "OPTIONS");
    let resp = next.run(req).await;
    if changes && resp.status().is_success() {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(e) = rebuild(&db) {
            tracing::error!("could not rebuild the snapshot: {e}");
        }
    }
    resp
}
