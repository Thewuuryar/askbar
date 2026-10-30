//! Helpers for tests, here and in the client crate, that need the server in a known state.
//! They go through the same code the admin service's pushes do (`snapshot::apply`), so a test
//! sees what a real deployment would. Not part of the server's interface.

use crate::{auth, ids, snapshot, Db};
use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode, SnapshotProvider};
use sha2::{Digest, Sha256};


pub const TEST_MASTER_KEY: &str = "0123456789abcdef0123456789abcdef";

/// A snapshot with nothing in it.
pub fn empty_snapshot() -> Snapshot {
    Snapshot {
        version: 0,
        providers: vec![],
        default_provider_id: None,
        fallback_order: vec![],
        system_prompt: None,
        display: DisplaySettings::default(),
        user_display_defaults: Default::default(),
        pair_codes: vec![],
        revoked_device_ids: vec![],
        budget: Budget::default(),
        kill_switch: false,
        audit_retention_days: 30,
    }
}

/// An OpenAI-style provider.
pub fn provider(id: &str, name: &str, base_url: &str, model: &str) -> SnapshotProvider {
    SnapshotProvider {
        id: id.into(),
        name: name.into(),
        kind: "openai".into(),
        base_url: base_url.into(),
        model: model.into(),
        max_tokens: None,
        temperature: None,
        headers: Default::default(),
    }
}

/// Change the current snapshot (or an empty one) with `f` and store the result, as a push would.
pub fn update_snapshot(db: &Db, f: impl FnOnce(&mut Snapshot)) -> u64 {
    let mut s = snapshot::current(db).unwrap().unwrap_or_else(empty_snapshot);
    f(&mut s);
    snapshot::apply(db, &s).expect("the test snapshot must be valid")
}

/// Add `p` to the snapshot's providers (replacing one with the same id).
pub fn add_provider(db: &Db, p: SnapshotProvider) {
    update_snapshot(db, |s| {
        s.providers.retain(|x| x.id != p.id);
        s.providers.push(p);
    });
}

/// Store an API key for a provider the way the management API would.
pub fn store_key(db: &Db, secrets: &crate::secrets::Secrets, provider_id: &str, key: &str) {
    db.conn()
        .execute(
            "INSERT INTO providers (id, api_key_enc) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET api_key_enc = excluded.api_key_enc",
            rusqlite::params![provider_id, secrets.encrypt(key)],
        )
        .unwrap();
}

/// A one-time pairing code for `user_id`, listed in the snapshot (valid for ten minutes).
pub fn add_pair_code(db: &Db, user_id: &str, device_name: &str) -> String {
    add_pair_code_expiring(db, user_id, device_name, 600)
}

/// Like `add_pair_code`, valid for `ttl_secs` from now (negative: already expired).
pub fn add_pair_code_expiring(db: &Db, user_id: &str, device_name: &str, ttl_secs: i64) -> String {
    let code = ids::new_id() + &ids::new_id();
    let hash: String = Sha256::digest(code.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    update_snapshot(db, |s| {
        s.pair_codes.push(SnapshotPairCode { hash, user_id: user_id.into(), device_name: device_name.into(), expires_at: auth::now() + ttl_secs })
    });
    code
}
