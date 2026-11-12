//! What the admin does when something happens that a person should hear about (ADR 0055): a log line, an event queued for
//! the alerter, and a note on every admin page for a day. The three go together so that one cannot be forgotten.
//!
//! The admin sends only the events it alone can know: `chain_mismatch`, `rejected_token`, `restart_loop` for the services it
//! polls, and `budget_threshold` (later also the backup ones). Each carries typed fields only; the sentence on the page and
//! in the log is made here from the same fields, never from what a record or a service said.

use crate::sealed::Alert;
use askbar_proto::alerter::{Fields, Kind, Period, Severity};
use askbar_service::alert_client::AlertClient;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// How long a note stays on the pages.
const NOTE_SECS: i64 = 24 * 60 * 60;
/// How many notes are kept.
const MAX_NOTES: usize = 20;

#[derive(Default)]
pub struct Notifier {
    /// The link to the alerter; set by `main`. Without one (tests, a stack with no alerter) an alert is a log line and a note.
    client: OnceLock<Arc<AlertClient>>,
    notes: Mutex<VecDeque<Alert>>,
}

impl Notifier {
    pub fn set_client(&self, client: Arc<AlertClient>) {
        let _ = self.client.set(client);
    }

    /// The client, for the task that sends what is queued.
    pub fn alert_client(&self) -> Option<Arc<AlertClient>> {
        self.client.get().cloned()
    }

    /// A log line, a queued event and a note for a day. `severity` is the kind's own except for `budget_threshold`.
    pub fn notify(&self, kind: Kind, severity: Severity, fields: Fields) {
        let text = sentence(kind, &fields);
        tracing::warn!(kind = kind.name(), "ALERT {text}");
        {
            let mut n = self.notes.lock().unwrap_or_else(|e| e.into_inner());
            n.push_back(Alert { kind: kind.name(), at: crate::auth::now(), text });
            while n.len() > MAX_NOTES {
                n.pop_front();
            }
        }
        if let Some(c) = self.client.get() {
            c.send_with(kind, severity, fields);
        }
    }

    /// The notes of the last day, oldest first.
    pub fn notes(&self) -> Vec<Alert> {
        let now = crate::auth::now();
        self.notes.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|a| a.at > now - NOTE_SECS).cloned().collect()
    }
}

/// The sentence for an event: a person's words made from the kind and its fields. Names and numbers only.
pub fn sentence(kind: Kind, f: &Fields) -> String {
    let link = f.link.as_deref().unwrap_or("a link");
    let service = f.service.as_deref().unwrap_or("a service");
    match kind {
        Kind::ChainMismatch => {
            "The audit trail does not match the chain this admin stored (see the red banner for what it found).".to_string()
        }
        Kind::RejectedToken => format!(
            "The admin's token on the link {link} was refused. Rotate it with `docker compose run --rm bootstrap rotate {link}`, then restart the services at both ends."
        ),
        Kind::RestartLoop => format!(
            "{service} restarted {} times in {} minutes.",
            f.count.unwrap_or(0),
            f.minutes.unwrap_or(0)
        ),
        Kind::BudgetThreshold => format!(
            "The {} token budget is {}% used.",
            match f.period {
                Some(Period::Month) => "monthly",
                _ => "daily",
            },
            f.percent.unwrap_or(0)
        ),
        other => format!("Alert: {}.", other.name()),
    }
}

/// Whether the other end accepts this admin's token on one link, from the status of each answer. A `401` is the only answer
/// that means the token was not accepted (any other status, an error included, came after the check). The first `401` of an
/// episode is an alert (`rejected_token`); the next answer that is not a `401` ends the episode, so a token that is
/// rotated and then refused again is a new alert.
pub struct TokenWatch {
    link: &'static str,
    rejected: AtomicBool,
    notifier: OnceLock<Arc<Notifier>>,
}

impl TokenWatch {
    pub fn new(link: &'static str) -> TokenWatch {
        TokenWatch { link, rejected: AtomicBool::new(false), notifier: OnceLock::new() }
    }

    pub fn set_notifier(&self, n: Arc<Notifier>) {
        let _ = self.notifier.set(n);
    }

    /// The status of an answer on this link.
    pub fn saw(&self, status: u16) {
        if status != 401 {
            self.rejected.store(false, Ordering::Relaxed);
            return;
        }
        if !self.rejected.swap(true, Ordering::Relaxed) {
            if let Some(n) = self.notifier.get() {
                n.notify(Kind::RejectedToken, Severity::Warning, Fields { link: Some(self.link.to_string()), ..Fields::default() });
            }
        }
    }

    /// Whether the last answer was a `401`.
    pub fn rejected(&self) -> bool {
        self.rejected.load(Ordering::Relaxed)
    }
}

/// Three restarts within ten minutes are a restart loop (ADR 0051).
pub const LOOP_CHANGES: usize = 3;
pub const LOOP_WINDOW_SECS: i64 = 10 * 60;

#[derive(Default)]
struct Boots {
    last: Option<String>,
    /// When the boot ID changed, within the window.
    changes: VecDeque<i64>,
    alerted_at: Option<i64>,
}

/// Watches the boot ID a polled service reports: each change is a restart. Three within ten minutes is a `restart_loop`
/// alert, and one is sent for a loop however long it goes on (another only after a quiet window). The first boot ID seen
/// is where the count starts, not a restart. Only services the admin polls are watched; the auditor reports the admin's.
pub struct RestartWatch {
    service: &'static str,
    boots: Mutex<Boots>,
    notifier: OnceLock<Arc<Notifier>>,
}

impl RestartWatch {
    pub fn new(service: &'static str) -> RestartWatch {
        RestartWatch { service, boots: Mutex::default(), notifier: OnceLock::new() }
    }

    pub fn set_notifier(&self, n: Arc<Notifier>) {
        let _ = self.notifier.set(n);
    }

    /// The boot ID the service reported just now.
    pub fn saw(&self, boot_id: &str) {
        self.saw_at(boot_id, crate::auth::now());
    }

    pub fn saw_at(&self, boot_id: &str, now: i64) {
        if boot_id.is_empty() {
            return;
        }
        let count = {
            let mut b = self.boots.lock().unwrap_or_else(|e| e.into_inner());
            let changed = b.last.as_deref().is_some_and(|last| last != boot_id);
            b.last = Some(boot_id.to_string());
            if !changed {
                return;
            }
            b.changes.push_back(now);
            while b.changes.front().is_some_and(|t| *t <= now - LOOP_WINDOW_SECS) {
                b.changes.pop_front();
            }
            let quiet = b.alerted_at.is_none_or(|t| now - t >= LOOP_WINDOW_SECS);
            if b.changes.len() < LOOP_CHANGES || !quiet {
                return;
            }
            b.alerted_at = Some(now);
            b.changes.len() as u32
        };
        if let Some(n) = self.notifier.get() {
            n.notify(
                Kind::RestartLoop,
                Severity::Warning,
                Fields {
                    service: Some(self.service.to_string()),
                    count: Some(count),
                    minutes: Some((LOOP_WINDOW_SECS / 60) as u32),
                    ..Fields::default()
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_kept_for_a_day_and_the_list_is_short() {
        let n = Notifier::default();
        for _ in 0..30 {
            n.notify(Kind::ChainMismatch, Severity::Critical, Fields::default());
        }
        assert_eq!(n.notes().len(), MAX_NOTES);
        n.notes.lock().unwrap().iter_mut().for_each(|a| a.at -= NOTE_SECS + 1);
        assert!(n.notes().is_empty());
    }

    #[test]
    fn three_restarts_in_ten_minutes_are_a_loop_and_one_alert_until_it_goes_quiet() {
        let n = Arc::new(Notifier::default());
        let w = RestartWatch::new("server");
        w.set_notifier(n.clone());
        let loops = || n.notes().iter().filter(|a| a.kind == "restart_loop").count();
        w.saw_at("a", 0);
        w.saw_at("a", 30);
        w.saw_at("b", 60);
        w.saw_at("c", 120);
        assert_eq!(loops(), 0, "the first boot ID is a start, and two restarts are not a loop");
        w.saw_at("d", 180);
        assert_eq!(loops(), 1, "the third change within ten minutes");
        w.saw_at("e", 240);
        w.saw_at("f", 300);
        assert_eq!(loops(), 1, "a loop that goes on is one alert");
        assert!(n.notes()[0].text.contains("server restarted 3 times in 10 minutes"));
        // Slow restarts, one every five minutes, are never three within ten.
        let slow = RestartWatch::new("model-gateway");
        slow.set_notifier(n.clone());
        for (i, id) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            slow.saw_at(id, 10_000 + i as i64 * 301);
        }
        assert_eq!(loops(), 1);
        // After a quiet window a new loop is a new alert.
        for (i, id) in ["g", "h", "i", "j"].iter().enumerate() {
            w.saw_at(id, 5_000 + i as i64);
        }
        assert_eq!(loops(), 2);
    }

    #[test]
    fn a_refused_token_is_one_alert_for_each_episode() {
        let n = Arc::new(Notifier::default());
        let w = TokenWatch::new("admin-server");
        w.set_notifier(n.clone());
        for _ in 0..5 {
            w.saw(401);
        }
        assert_eq!((n.notes().len(), w.rejected()), (1, true), "five refusals in a row are one alert");
        w.saw(500);
        assert!(!w.rejected(), "any answer that is not a 401 ends it");
        w.saw(401);
        assert_eq!(n.notes().len(), 2, "refused again is a new alert");
        assert!(n.notes()[0].text.contains("admin-server") && n.notes()[0].text.contains("bootstrap rotate"));
    }
}
