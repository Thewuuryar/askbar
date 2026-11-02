//! Admin sessions and failed-login limiting. Both live in memory only: a restart signs
//! everyone out and clears any lockout.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chacha20poly1305::aead::{rand_core::RngCore, OsRng};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const SESSION_TTL_SECS: i64 = 12 * 60 * 60;
pub const MAX_FAILURES: usize = 5;
pub const FAILURE_WINDOW_SECS: i64 = 15 * 60;
pub const LOCKOUT_SECS: i64 = 15 * 60;

pub fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn sha(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Default)]
pub struct AdminAuth {
    /// sha256(session token) -> expiry. The token itself is never kept.
    sessions: Mutex<HashMap<String, i64>>,
    failures: Mutex<Vec<i64>>,
    locked_until: Mutex<i64>,
}

impl AdminAuth {
    /// Start a session and return its token (shown once, to the browser).
    pub fn new_session(&self) -> String {
        let mut b = [0u8; 32];
        OsRng.fill_bytes(&mut b);
        let token = URL_SAFE_NO_PAD.encode(b);
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).insert(sha(&token), now() + SESSION_TTL_SECS);
        token
    }

    pub fn valid_session(&self, token: &str) -> bool {
        let mut s = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        let t = now();
        s.retain(|_, exp| *exp > t);
        s.contains_key(&sha(token))
    }

    pub fn end_session(&self, token: &str) {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(&sha(token));
    }

    /// Sign out every session except `keep`.
    pub fn keep_only(&self, keep: Option<&str>) {
        let keep = keep.map(sha);
        self.sessions.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| Some(k) == keep.as_ref());
    }

    /// Seconds left on a lockout, if one is active.
    pub fn lockout_remaining(&self) -> Option<i64> {
        let until = *self.locked_until.lock().unwrap_or_else(|e| e.into_inner());
        (until > now()).then(|| until - now())
    }

    /// Count a failed login; `MAX_FAILURES` within the window locks logins out for a while.
    pub fn record_failure(&self) {
        self.record_failure_at(now());
    }

    pub fn record_failure_at(&self, t: i64) {
        let mut f = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        f.retain(|x| *x > t - FAILURE_WINDOW_SECS);
        f.push(t);
        if f.len() >= MAX_FAILURES {
            *self.locked_until.lock().unwrap_or_else(|e| e.into_inner()) = t + LOCKOUT_SECS;
            f.clear();
        }
    }

    pub fn clear_failures(&self) {
        self.failures.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}
