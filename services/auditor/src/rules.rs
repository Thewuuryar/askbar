//! The alert rules (ADR 0051, ADR 0055): what the auditor says to the alerter because of what it sees. It decides when; the
//! alerter decides what is said and how. An event is a kind, a severity and typed fields, never a record's content.
//!
//! The engine is fed three ways: after each record it appends ([`Rules::fed`]), after each request on a link
//! ([`Rules::seen`]), and every 30 seconds ([`Rules::tick`], for what is about time and disk). Every method takes the time as a
//! number, so a test drives a day in a moment with a fake clock; nothing here reads a clock or a disk.

use askbar_proto::alerter::{Fields, Kind, Severity};
use askbar_service::alert_client::AlertClient;
use std::sync::{Arc, Mutex};

/// Where events go. The alerter's link in the service, a collector in a test.
pub trait Alerts: Send + Sync {
    fn send(&self, kind: Kind, severity: Severity, fields: Fields);
}

impl Alerts for AlertClient {
    fn send(&self, kind: Kind, severity: Severity, fields: Fields) {
        self.send_with(kind, severity, fields);
    }
}

/// What the engine learns from an append.
#[derive(Debug, Clone, PartialEq)]
pub enum Fed {
    /// An admin action record that ended `done`: its category and action, and the facts the admin added to it.
    AdminDone { category: String, action: String, detail: Option<serde_json::Value> },
    /// A service announced a boot ID.
    Hello { service: String, boot_id: String },
}

/// The two limits on how long sealed content stays open, in minutes, as the last record of a change left them.
#[derive(Clone, Copy, PartialEq, Eq)]
struct UnsealLimits {
    idle: u32,
    total: u32,
}

struct State {
    unseal: UnsealLimits,
}

impl Default for State {
    fn default() -> State {
        State {
            unseal: UnsealLimits {
                idle: askbar_proto::audit::DEFAULT_UNSEAL_IDLE_MINUTES,
                total: askbar_proto::audit::DEFAULT_UNSEAL_TOTAL_MINUTES,
            },
        }
    }
}

pub struct Rules {
    alerts: Arc<dyn Alerts>,
    state: Mutex<State>,
}

impl Rules {
    pub fn new(alerts: Arc<dyn Alerts>) -> Rules {
        Rules { alerts, state: Mutex::new(State::default()) }
    }

    /// The limits the admin last recorded, from before this process started (`None` for one never recorded: the default).
    /// Without this a restart would call the next save of the same limits a change.
    pub fn seed_unseal_limits(&self, idle: Option<u32>, total: Option<u32>) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.unseal.idle = idle.unwrap_or(s.unseal.idle);
        s.unseal.total = total.unwrap_or(s.unseal.total);
    }

    fn say(&self, kind: Kind, fields: Fields) {
        self.alerts.send(kind, kind.severity(), fields);
    }

    /// A record was appended (`now` in unix seconds).
    pub fn fed(&self, fed: &Fed, now: i64) {
        let _ = now;
        match fed {
            Fed::AdminDone { category, action, detail } => match (category.as_str(), action.as_str()) {
                // Someone opened sealed content: always worth knowing, because it is the one way to read what was asked.
                ("sealed_content", "unseal") => self.say(Kind::Unsealed, Fields::default()),
                // The limits on how long it stays open, when a save changes either (every save of the settings carries them).
                ("settings", "update") => {
                    let minutes = |key: &str| detail.as_ref().and_then(|d| d.get(key)).and_then(|v| v.as_u64()).map(|v| v as u32);
                    let changed = {
                        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        let next = UnsealLimits {
                            idle: minutes("unseal_idle_minutes").unwrap_or(s.unseal.idle),
                            total: minutes("unseal_total_minutes").unwrap_or(s.unseal.total),
                        };
                        let changed = next != s.unseal;
                        s.unseal = next;
                        changed
                    };
                    if changed {
                        self.say(Kind::UnsealTimeoutsChanged, Fields::default());
                    }
                }
                // The key that encrypts the backups (plan step 6.1): setting it or replacing it.
                ("secret", "backup_key_setup" | "backup_key_rotate") => self.say(Kind::BackupKeyChanged, Fields::default()),
                _ => {}
            },
            Fed::Hello { .. } => {}
        }
    }

    /// A request passed the token check on the link of `caller`.
    pub fn seen(&self, caller: &str, now: i64) {
        let _ = (caller, now);
    }

    /// Every 30 seconds: what is about time and disk. `free` is the bytes free where the store is, if it could be told;
    /// `last_requests` is when each link last called, by caller.
    pub fn tick(&self, now: i64, free: Option<u64>, last_requests: &std::collections::BTreeMap<&'static str, i64>) {
        let _ = (now, free, last_requests);
    }
}

/// What an append tells the rules, if anything.
pub fn fed_of(record: &askbar_audit::Record) -> Option<Fed> {
    use askbar_audit::Record;
    match record {
        Record::AdminAction(a) if a.outcome == "done" => Some(Fed::AdminDone {
            category: a.category.clone(),
            action: a.action.clone(),
            detail: a.detail_json.as_deref().and_then(|d| serde_json::from_str(d).ok()),
        }),
        Record::Hello { service, boot_id } => Some(Fed::Hello { service: service.clone(), boot_id: boot_id.clone() }),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    /// Collects what the rules send, for the tests.
    #[derive(Default)]
    pub struct Collector(pub Mutex<Vec<(Kind, Severity, Fields)>>);

    impl Alerts for Collector {
        fn send(&self, kind: Kind, severity: Severity, fields: Fields) {
            self.0.lock().unwrap().push((kind, severity, fields));
        }
    }

    impl Collector {
        pub fn kinds(&self) -> Vec<Kind> {
            self.0.lock().unwrap().iter().map(|e| e.0).collect()
        }
    }

    pub fn rules() -> (Rules, Arc<Collector>) {
        let c = Arc::new(Collector::default());
        (Rules::new(c.clone()), c)
    }

    fn done(category: &str, action: &str, detail: Option<serde_json::Value>) -> Fed {
        Fed::AdminDone { category: category.into(), action: action.into(), detail }
    }

    #[test]
    fn only_an_admin_action_that_is_done_and_a_hello_tell_the_rules_anything() {
        use askbar_audit::{AdminActionWrite, Record};
        let action = |outcome: &str| {
            Record::AdminAction(AdminActionWrite {
                id: "a".into(),
                at: 0,
                service: "admin".into(),
                category: "sealed_content".into(),
                action: "unseal".into(),
                outcome: outcome.into(),
                detail_json: Some(r#"{"status":200}"#.into()),
            })
        };
        assert_eq!(fed_of(&action("requested")), None);
        assert_eq!(fed_of(&action("refused")), None);
        assert_eq!(
            fed_of(&action("done")),
            Some(Fed::AdminDone {
                category: "sealed_content".into(),
                action: "unseal".into(),
                detail: Some(serde_json::json!({ "status": 200 }))
            })
        );
        assert_eq!(
            fed_of(&Record::Hello { service: "admin".into(), boot_id: "b".into() }),
            Some(Fed::Hello { service: "admin".into(), boot_id: "b".into() })
        );
        assert_eq!(fed_of(&Record::Setting { key: "retention_days".into(), value: "30".into() }), None);
    }

    #[test]
    fn an_unseal_is_an_alert_and_nothing_else_the_admin_does_with_sealed_content_is() {
        let (r, c) = rules();
        r.fed(&done("sealed_content", "unseal", None), 0);
        r.fed(&done("sealed_content", "seal", None), 0);
        r.fed(&done("sealed_content", "view", None), 0);
        assert_eq!(c.kinds(), [Kind::Unsealed]);
        assert_eq!(c.0.lock().unwrap()[0].1, Severity::Warning);
        assert_eq!(c.0.lock().unwrap()[0].2, Fields::default(), "no field: nothing about the passphrase or the key");
    }

    #[test]
    fn a_save_of_the_settings_is_an_alert_only_when_it_changes_how_long_content_stays_open() {
        let (r, c) = rules();
        let save = |idle: Option<u32>, total: Option<u32>| {
            let mut d = json!({ "audit_retention_days": 45 });
            if let Some(i) = idle {
                d["unseal_idle_minutes"] = json!(i);
            }
            if let Some(t) = total {
                d["unseal_total_minutes"] = json!(t);
            }
            done("settings", "update", Some(d))
        };
        // The defaults are 15 and 60: saving them again, or saving something else, says nothing.
        r.fed(&save(Some(15), Some(60)), 0);
        r.fed(&save(None, None), 0);
        r.fed(&done("settings", "update", None), 0);
        assert!(c.kinds().is_empty());
        r.fed(&save(Some(30), Some(60)), 0);
        assert_eq!(c.kinds(), [Kind::UnsealTimeoutsChanged]);
        r.fed(&save(Some(30), Some(60)), 0);
        assert_eq!(c.kinds().len(), 1, "the same limits again is not a change");
        r.fed(&save(None, Some(120)), 0);
        assert_eq!(c.kinds().len(), 2, "one limit alone");
        // A restart: the limits the admin last recorded are the baseline, not the defaults.
        let (r, c) = rules();
        r.seed_unseal_limits(Some(30), None);
        r.fed(&save(Some(30), Some(60)), 0);
        assert!(c.kinds().is_empty());
        r.fed(&save(Some(15), Some(60)), 0);
        assert_eq!(c.kinds(), [Kind::UnsealTimeoutsChanged]);
    }

    #[test]
    fn setting_or_replacing_the_backup_key_is_an_alert() {
        let (r, c) = rules();
        r.fed(&done("secret", "backup_key_setup", None), 0);
        r.fed(&done("secret", "backup_key_rotate", None), 0);
        r.fed(&done("secret", "audit_key_rotate", None), 0);
        r.fed(&done("secret", "api_key_create", None), 0);
        assert_eq!(c.kinds(), [Kind::BackupKeyChanged, Kind::BackupKeyChanged]);
    }
}
