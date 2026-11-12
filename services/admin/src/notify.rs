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
