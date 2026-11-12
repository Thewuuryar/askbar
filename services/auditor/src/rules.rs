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

#[derive(Default)]
struct State {}

pub struct Rules {
    alerts: Arc<dyn Alerts>,
    state: Mutex<State>,
}

impl Rules {
    pub fn new(alerts: Arc<dyn Alerts>) -> Rules {
        Rules { alerts, state: Mutex::new(State::default()) }
    }

    /// A record was appended (`now` in unix seconds).
    pub fn fed(&self, fed: &Fed, now: i64) {
        let _ = (fed, now, &self.state, &self.alerts);
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
}
