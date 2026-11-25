//! The in-memory store of used one-time IDs. One lock covers check and insert, so two calls racing with one approval
//! cannot both pass. A restart empties it, which is safe only while the approval is shorter-lived than a restart can
//! hide: an approval lives `DEFAULT_APPROVAL_TTL_SECS`, and a replay after a restart must still beat that expiry.

use crate::constants::MAX_ONE_TIME_IDS;
use askbar_service::approval::OneTimeIds;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub struct OneTimeStore(Mutex<HashMap<String, i64>>);

/// The store as `verify` sees it: at `now`, keeping an ID for `window` seconds (never less than its approval lives).
pub struct At<'a> {
    pub store: &'a OneTimeStore,
    pub now: i64,
    pub window: i64,
}

impl OneTimeIds for At<'_> {
    fn consume(&self, id: &str, expires_at: i64) -> bool {
        let mut m = self.store.0.lock().unwrap_or_else(|e| e.into_inner());
        if m.get(id).is_some_and(|until| *until > self.now) {
            return false;
        }
        if m.len() >= MAX_ONE_TIME_IDS {
            m.retain(|_, until| *until > self.now);
        }
        if m.len() >= MAX_ONE_TIME_IDS {
            return false;
        }
        m.insert(id.to_string(), expires_at.max(self.now + self.window));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_new_once_and_a_purge_frees_the_expired() {
        let s = OneTimeStore::default();
        let at = |now| At { store: &s, now, window: 10 };
        assert!(at(0).consume("a", 5));
        assert!(!at(9).consume("a", 5), "still held inside the window");
        for i in 0..MAX_ONE_TIME_IDS - 1 {
            assert!(at(0).consume(&format!("x{i}"), 5));
        }
        assert!(!at(9).consume("new", 5), "full of live ids: refused");
        assert!(at(10).consume("new", 15), "expired ids are purged when full");
    }
}
