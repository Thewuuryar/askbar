//! Sealed-content sessions (ADR 0011 rules 8 to 13, plan step 2.8).
//!
//! Opening sealed audit content is a session, not a prompt for each item. A passphrase typed within ten minutes of
//! signing in opens a session: the keys it derives are held in this process's memory and nowhere else (never on disk,
//! in a log, in a backup or in a record), and content stays readable without more prompts until the session ends: after
//! 15 minutes idle or an hour in all (the administrator may set both, up to 12 hours), on **Seal**, on sign-out, or when
//! the admin restarts. Only the person's own browser session may start or use one: an admin API key cannot.
//!
//! What is recorded (through [`crate::actions`], so a record that cannot be written refuses the step): the start, each
//! item viewed and a Seal as changes of their own; the end by a timeout or a sign-out here. Every unseal and every
//! change of the timeouts is an alert: a log line and a banner until the alerter exists (phase 4).

use crate::audit_key::{key_id, AuditKeypair};
use crate::AdminState;
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_IDLE_MINUTES: u32 = 15;
pub const DEFAULT_TOTAL_MINUTES: u32 = 60;
pub const MAX_MINUTES: u32 = 12 * 60;
/// A sign-in older than this does not start a session.
pub const FRESH_SIGNIN_SECS: i64 = 10 * 60;
/// How long an alert stays in the banner.
const ALERT_SECS: i64 = 24 * 60 * 60;
const MAX_ALERTS: usize = 20;

/// Why a session ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The person pressed Seal.
    Seal,
    /// Nothing was viewed for the idle time.
    Idle,
    /// The total time ran out.
    Total,
    /// The browser session that started it ended.
    SignOut,
}

impl Ended {
    pub fn as_str(self) -> &'static str {
        match self {
            Ended::Seal => "seal",
            Ended::Idle => "idle_timeout",
            Ended::Total => "total_timeout",
            Ended::SignOut => "sign_out",
        }
    }
}

/// A fact the person should see, kept 24 hours in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub kind: &'static str,
    pub at: i64,
    pub text: String,
}

struct Open {
    id: String,
    /// The browser session that started it (a hash: the token is never kept).
    browser: String,
    keys: BTreeMap<String, AuditKeypair>,
    started: Instant,
    started_at: i64,
    last_used: Instant,
    idle: Duration,
    total: Duration,
}

impl Open {
    fn due(&self, now: Instant) -> Option<Ended> {
        if now.duration_since(self.started) >= self.total {
            Some(Ended::Total)
        } else if now.duration_since(self.last_used) >= self.idle {
            Some(Ended::Idle)
        } else {
            None
        }
    }
}

/// What a browser may know of its session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub id: String,
    pub started_at: i64,
    pub idle_left_secs: u64,
    pub total_left_secs: u64,
    pub idle_limit_secs: u64,
    pub total_limit_secs: u64,
    pub key_ids: Vec<String>,
}

/// What ended: for the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndedSession {
    pub id: String,
    pub seconds: u64,
    pub why: Ended,
}

#[derive(Default)]
pub struct Sealed {
    open: Mutex<Option<Open>>,
    alerts: Mutex<VecDeque<Alert>>,
}

impl Sealed {
    fn open(&self) -> std::sync::MutexGuard<'_, Option<Open>> {
        self.open.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The session of this browser, if it has one that is not due.
    pub fn view(&self, browser: &str) -> Option<View> {
        self.view_at(browser, Instant::now())
    }

    fn view_at(&self, browser: &str, now: Instant) -> Option<View> {
        let g = self.open();
        let o = g.as_ref().filter(|o| o.browser == browser && o.due(now).is_none())?;
        Some(View {
            id: o.id.clone(),
            started_at: o.started_at,
            idle_left_secs: o.idle.saturating_sub(now.duration_since(o.last_used)).as_secs(),
            total_left_secs: o.total.saturating_sub(now.duration_since(o.started)).as_secs(),
            idle_limit_secs: o.idle.as_secs(),
            total_limit_secs: o.total.as_secs(),
            key_ids: o.keys.keys().cloned().collect(),
        })
    }

    /// Starts a session for this browser with `keys`, or adds them to the one it has (the time limits of that one do not
    /// restart). Returns whether it is a new session. Another browser's session is replaced: there is one at a time.
    pub fn start(&self, browser: &str, keys: BTreeMap<String, AuditKeypair>, idle: Duration, total: Duration) -> bool {
        let now = Instant::now();
        let mut g = self.open();
        if let Some(o) = g.as_mut().filter(|o| o.browser == browser && o.due(now).is_none()) {
            o.keys.extend(keys);
            o.last_used = now;
            return false;
        }
        *g = Some(Open {
            id: ulid::Ulid::new().to_string(),
            browser: browser.to_string(),
            keys,
            started: now,
            started_at: crate::auth::now(),
            last_used: now,
            idle,
            total,
        });
        true
    }

    /// Counts a use (the idle time starts again) if this browser's session is open and not due.
    pub fn touch(&self, browser: &str) -> bool {
        let now = Instant::now();
        let mut g = self.open();
        match g.as_mut().filter(|o| o.browser == browser && o.due(now).is_none()) {
            Some(o) => {
                o.last_used = now;
                true
            }
            None => false,
        }
    }

    /// Opens one sealed payload with the session's key of that id. `None` when the session has no such key or the box does
    /// not open.
    pub fn open_one(&self, browser: &str, key: &str, sealed: &[u8]) -> Option<Vec<u8>> {
        let g = self.open();
        let o = g.as_ref().filter(|o| o.browser == browser)?;
        crate::audit_key::open_sealed(&o.keys.get(key)?.secret, sealed)
    }

    /// The new limits apply to the session that is open too.
    pub fn set_limits(&self, idle: Duration, total: Duration) {
        if let Some(o) = self.open().as_mut() {
            o.idle = idle;
            o.total = total;
        }
    }

    /// Ends the session (its keys are dropped with it), whoever's it is.
    pub fn end(&self, why: Ended) -> Option<EndedSession> {
        let o = self.open().take()?;
        Some(EndedSession { id: o.id, seconds: o.started.elapsed().as_secs(), why })
    }

    /// If the session is due, or its browser session is gone, the reason it should end.
    pub fn due(&self, browser_valid: impl Fn(&str) -> bool) -> Option<Ended> {
        self.due_at(Instant::now(), browser_valid)
    }

    fn due_at(&self, now: Instant, browser_valid: impl Fn(&str) -> bool) -> Option<Ended> {
        let g = self.open();
        let o = g.as_ref()?;
        if !browser_valid(&o.browser) {
            return Some(Ended::SignOut);
        }
        o.due(now)
    }

    /// Whether `browser` is the one that has the session (it may be due and not yet ended).
    pub fn belongs_to(&self, browser: &str) -> bool {
        self.open().as_ref().is_some_and(|o| o.browser == browser)
    }

    /// Tests only: make the session look as if it had run, and sat idle, for `by` longer.
    #[doc(hidden)]
    pub fn rewind(&self, by: Duration) {
        if let Some(o) = self.open().as_mut() {
            o.started = o.started.checked_sub(by).unwrap_or(o.started);
            o.last_used = o.last_used.checked_sub(by).unwrap_or(o.last_used);
        }
    }

    // ------------------------------------------------------------------ alerts

    /// An alert: a log line now, and a banner for a day.
    pub fn alert(&self, kind: &'static str, text: String) {
        tracing::warn!(kind, "ALERT sealed content: {text}");
        let mut a = self.alerts.lock().unwrap_or_else(|e| e.into_inner());
        a.push_back(Alert { kind, at: crate::auth::now(), text });
        while a.len() > MAX_ALERTS {
            a.pop_front();
        }
    }

    /// The alerts of the last day, oldest first.
    pub fn alerts(&self) -> Vec<Alert> {
        let now = crate::auth::now();
        self.alerts.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|a| a.at > now - ALERT_SECS).cloned().collect()
    }
}

/// What the session limits are now, from the settings (minutes, defaults 15 and 60).
pub fn limits(conn: &rusqlite::Connection) -> (Duration, Duration) {
    let get = |key: &str, default: u32| {
        crate::admin_api::setting(conn, key)
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|n| (1..=MAX_MINUTES).contains(n))
            .unwrap_or(default)
    };
    (
        Duration::from_secs(u64::from(get("unseal_idle_minutes", DEFAULT_IDLE_MINUTES)) * 60),
        Duration::from_secs(u64::from(get("unseal_total_minutes", DEFAULT_TOTAL_MINUTES)) * 60),
    )
}

impl AdminState {
    /// Ends the sealed-content session, if there is one. `record` appends the end to the auditor (best effort: it is
    /// already over); a Seal that came as a request is recorded by the request, so it passes `false`.
    pub async fn end_sealed(&self, why: Ended, record: bool) {
        let Some(ended) = self.sealed.end(why) else { return };
        tracing::info!(reason = why.as_str(), seconds = ended.seconds, "sealed content was sealed again");
        if !record {
            return;
        }
        let Some(log) = self.actions.get() else { return };
        let detail = serde_json::json!({ "reason": why.as_str(), "session": ended.id, "seconds": ended.seconds });
        let r = askbar_proto::audit::AdminActionStart {
            id: ulid::Ulid::new().to_string(),
            at: crate::auth::now(),
            category: "sealed_content".into(),
            action: "session_end".into(),
            outcome: "done".into(),
            detail: Some(detail),
        };
        if let Err(why) = log.record(&r).await {
            tracing::warn!(?why, "the end of a sealed-content session could not be recorded");
        }
    }
}

/// Ends the session when it is due or its sign-in is gone, for as long as the process runs.
pub async fn run(state: std::sync::Arc<AdminState>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        sweep(&state).await;
    }
}

/// One look: end the session if it is due.
pub async fn sweep(state: &AdminState) {
    if let Some(why) = state.sealed.due(|h| state.auth.valid_hash(h)) {
        state.end_sealed(why, true).await;
    }
}

/// Whether `pair` is the key its id names (the passphrase was right for it).
pub fn is_that_key(row_key_id: &str, pair: &AuditKeypair) -> bool {
    key_id(&pair.public_bytes()) == row_key_id
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit_key::{derive, KdfParams};

    const FAST: KdfParams = KdfParams { memory_kib: 64, passes: 1, lanes: 1 };

    fn keys(pass: &str) -> BTreeMap<String, AuditKeypair> {
        let kp = derive(pass, &[3u8; 16], FAST).unwrap();
        BTreeMap::from([(kp.key_id(), kp)])
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_session_is_one_browsers_and_ends_after_its_idle_or_total_time_whichever_comes_first() {
        let s = Sealed::default();
        assert!(s.view("a").is_none());
        assert!(s.start("a", keys("one passphrase of enough length"), secs(900), secs(3600)));
        let v = s.view("a").unwrap();
        assert!(v.idle_left_secs <= 900 && v.total_left_secs <= 3600 && v.key_ids.len() == 1);
        assert!(s.view("b").is_none(), "another browser has no session");
        let t0 = Instant::now();
        assert_eq!(s.due_at(t0 + secs(899), |_| true), None);
        assert_eq!(s.due_at(t0 + secs(901), |_| true), Some(Ended::Idle));
        // Using it starts the idle time again, but never the total.
        s.rewind(secs(800));
        assert!(s.touch("a"));
        assert_eq!(s.due_at(Instant::now() + secs(800), |_| true), None);
        assert_eq!(
            s.due_at(Instant::now() + secs(1000), |_| true),
            Some(Ended::Idle),
            "800 s of use were not enough to keep it for 1000 more"
        );
        s.rewind(secs(3000));
        assert_eq!(s.due_at(Instant::now() + secs(700), |_| true), Some(Ended::Total), "an hour in all, however it is used");
    }

    #[test]
    fn a_due_session_cannot_be_used_and_the_browser_session_ending_ends_it() {
        let s = Sealed::default();
        s.start("a", keys("one passphrase of enough length"), secs(60), secs(3600));
        assert_eq!(s.due(|_| false), Some(Ended::SignOut), "its sign-in is gone");
        s.rewind(secs(61));
        assert!(!s.touch("a") && s.view("a").is_none(), "idle: no view and no use");
        assert_eq!(s.due(|_| true), Some(Ended::Idle));
        let ended = s.end(Ended::Idle).unwrap();
        assert_eq!(ended.why, Ended::Idle);
        assert!(s.end(Ended::Idle).is_none() && s.view("a").is_none());
    }

    #[test]
    fn only_the_sessions_own_keys_open_content_and_adding_a_key_does_not_restart_the_clock() {
        let s = Sealed::default();
        let one = keys("one passphrase of enough length");
        let sealed = {
            let (_, kp) = one.iter().next().unwrap();
            kp.public.seal(&mut crypto_box::aead::OsRng, b"text").unwrap()
        };
        let one_id = one.keys().next().unwrap().clone();
        s.start("a", one, secs(60), secs(120));
        assert_eq!(s.open_one("a", &one_id, &sealed).as_deref(), Some(&b"text"[..]));
        assert_eq!(s.open_one("a", "0000000000000000", &sealed), None, "a key the session does not hold");
        assert_eq!(s.open_one("b", &one_id, &sealed), None, "another browser");
        s.rewind(secs(30));
        assert!(!s.start("a", keys("another passphrase of enough length"), secs(900), secs(9000)), "added, not restarted");
        let v = s.view("a").unwrap();
        assert_eq!(v.key_ids.len(), 2);
        assert!(v.total_left_secs <= 90, "the clock kept running: {v:?}");
    }

    #[test]
    fn new_limits_apply_to_the_session_that_is_open() {
        let s = Sealed::default();
        s.start("a", keys("one passphrase of enough length"), secs(900), secs(3600));
        s.set_limits(secs(30), secs(60));
        s.rewind(secs(31));
        assert_eq!(s.due(|_| true), Some(Ended::Idle));
    }

    #[test]
    fn alerts_are_kept_for_a_day_and_the_list_is_short() {
        let s = Sealed::default();
        for i in 0..30 {
            s.alert("unsealed", format!("n{i}"));
        }
        let a = s.alerts();
        assert_eq!(a.len(), MAX_ALERTS);
        assert_eq!(a.last().unwrap().text, "n29");
        s.alerts.lock().unwrap().iter_mut().for_each(|a| a.at -= ALERT_SECS + 1);
        assert!(s.alerts().is_empty());
    }

    #[test]
    fn the_limits_come_from_the_settings_with_defaults_and_bounds() {
        let db = crate::AdminDb::open_in_memory().unwrap();
        assert_eq!(limits(db.conn()), (secs(15 * 60), secs(60 * 60)));
        for (k, v) in [("unseal_idle_minutes", "5"), ("unseal_total_minutes", "720")] {
            db.conn().execute("INSERT INTO settings (key, value) VALUES (?1, ?2)", [k, v]).unwrap();
        }
        assert_eq!(limits(db.conn()), (secs(300), secs(720 * 60)));
        db.conn().execute("UPDATE settings SET value = '9999' WHERE key = 'unseal_idle_minutes'", []).unwrap();
        assert_eq!(limits(db.conn()).0, secs(15 * 60), "out of bounds falls back to the default");
    }
}
