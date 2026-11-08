//! Helpers for tests, here and in the client crate, that need the server in a known state.
//! They go through the same code the admin service's pushes do (`snapshot::apply`), so a test
//! sees what a real deployment would. Not part of the server's interface.

use crate::{auth, ids, snapshot, Db};
use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode, SnapshotProvider};
use sha2::{Digest, Sha256};

pub const TEST_MASTER_KEY: &str = "0123456789abcdef0123456789abcdef";

/// The secret half of the test audit key (the tests that read what was sealed use it).
pub const TEST_AUDIT_SECRET: [u8; 32] = [9; 32];

/// The public key of `TEST_AUDIT_SECRET`, as a snapshot carries it. `empty_snapshot` includes it,
/// so tests that start generations through the helpers have a key unless they remove it.
pub fn test_audit_key() -> askbar_proto::AuditKey {
    let public = *crypto_box::SecretKey::from(TEST_AUDIT_SECRET).public_key().as_bytes();
    askbar_proto::AuditKey { key_id: crate::seal::key_id(&public), public_key: public.iter().map(|b| format!("{b:02x}")).collect() }
}

/// A snapshot with nothing in it, apart from the test audit key.
pub fn empty_snapshot() -> Snapshot {
    Snapshot {
        version: 0,
        providers: vec![],
        default_provider_id: None,
        system_prompt: None,
        display: DisplaySettings::default(),
        user_display_defaults: Default::default(),
        users: vec![],
        pair_codes: vec![],
        revoked_client_ids: vec![],
        budget: Budget::default(),
        kill_switch: false,
        kill_switch_aborts_running: true,
        audit_retention_days: 30,
        generation_timeout_secs: 300,
        audit_public_key: Some(test_audit_key()),
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
        billing: Default::default(),
        price_in: None,
        price_out: None,
        price_cached_in: None,
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

/// Store an API key for a provider the way the server API would.
pub fn store_key(db: &Db, secrets: &crate::secrets::Secrets, provider_id: &str, key: &str) {
    db.conn()
        .execute(
            "INSERT INTO providers (id, api_key_enc) VALUES (?1, ?2) ON CONFLICT(id) DO UPDATE SET api_key_enc = excluded.api_key_enc",
            rusqlite::params![provider_id, secrets.encrypt(key)],
        )
        .unwrap();
}

/// A one-time pairing code for `user_id`, listed in the snapshot (valid for ten minutes).
pub fn add_pair_code(db: &Db, user_id: &str, client_name: &str) -> String {
    add_pair_code_expiring(db, user_id, client_name, 600)
}

/// Like `add_pair_code`, valid for `ttl_secs` from now (negative: already expired).
pub fn add_pair_code_expiring(db: &Db, user_id: &str, client_name: &str, ttl_secs: i64) -> String {
    let code = ids::new_id() + &ids::new_id();
    let hash: String = Sha256::digest(code.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
    update_snapshot(db, |s| {
        s.pair_codes.push(SnapshotPairCode {
            hash,
            user_id: user_id.into(),
            client_name: client_name.into(),
            expires_at: auth::now() + ttl_secs,
        })
    });
    code
}

/// An `AppState` over an empty in-memory database, for unit tests.
pub fn state_for_tests() -> std::sync::Arc<crate::AppState> {
    crate::AppState::new(Db::open_in_memory().unwrap(), crate::secrets::Secrets::new(TEST_MASTER_KEY).unwrap(), crate::MIN_CLIENT)
}

/// A recorder writing `audit.db` in a fresh temp directory, which is returned too. For tests.
pub fn audit_recorder() -> (crate::audit::Recorder, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("askbar-audit-test-{}", ids::new_id()));
    let db = crate::audit::AuditDb::open(&dir.join("audit.db")).expect("audit.db opens");
    (crate::audit::Recorder::spawn(db, 1024), dir)
}

/// An `AppState` over an in-memory database with a provider "p1" pointing at `base_url` (its key
/// stored), a user, and a recorder writing a temp `audit.db`; returns the state and that
/// directory. For tests.
pub fn state_with_audit(base_url: &str) -> (std::sync::Arc<crate::AppState>, std::path::PathBuf) {
    let db = Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let secrets = crate::secrets::Secrets::new(TEST_MASTER_KEY).unwrap();
    add_provider(&db, provider("p1", "mock", base_url, "m"));
    store_key(&db, &secrets, "p1", "sk-test");
    let (recorder, dir) = audit_recorder();
    (crate::AppState::new(db, secrets, crate::MIN_CLIENT).with_audit(recorder), dir)
}

/// `user_id`'s personal space, made if need be, as `GET /spaces` would list it. For tests that call
/// `generation::submit` or read a space's store directly.
pub fn personal_space(state: &crate::AppState, user_id: &str) -> std::sync::Arc<crate::Space> {
    let db = state.db.lock().unwrap();
    let id = db.ensure_personal_space(user_id).unwrap();
    state.spaces.for_member(&db, user_id, &id).unwrap().expect("the owner is a member")
}

/// Make generations started from now on time out after `d` instead of the snapshot's value.
pub fn override_generation_timeout(state: &crate::AppState, d: std::time::Duration) {
    state.set_generation_timeout_override(Some(d));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn new_records_nothing_and_with_audit_gives_a_state_its_recorder() {
        let plain = state_for_tests();
        assert!(plain.audit.healthy() && plain.audit.dropped() == 0, "the default recorder is a healthy no-op");
        let (state, dir) = state_with_audit("http://127.0.0.1:1/v1");
        assert!(dir.join("audit.db").exists(), "the audit store is opened beside the state");
        assert!(state.audit.healthy());
        assert!(crate::store::load_provider(&state.db.lock().unwrap(), &state.secrets(), "p1").unwrap().is_some(), "with its provider");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_timeout_override_can_be_set_read_back_and_cleared() {
        let state =
            crate::AppState::new(Db::open_in_memory().unwrap(), crate::secrets::Secrets::new(TEST_MASTER_KEY).unwrap(), crate::MIN_CLIENT);
        assert_eq!(state.generation_timeout_override(), None);
        override_generation_timeout(&state, Duration::from_secs(2));
        assert_eq!(state.generation_timeout_override(), Some(Duration::from_secs(2)));
        state.set_generation_timeout_override(None);
        assert_eq!(state.generation_timeout_override(), None);
    }
}
