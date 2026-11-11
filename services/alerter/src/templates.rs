//! One fixed sentence for each kind of alert, built from the event's typed fields only (ADR 0055).
//!
//! The text is the alerter's own: a sender chooses a kind and fills typed fields, so a compromised sender cannot make
//! the alerter say anything else. No sentence ever holds a token, a secret, a prompt or a reply.

use askbar_proto::alerter::{Event, Kind, Period};

/// What is sent: a short title and the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    pub title: String,
    pub body: String,
}

fn period(p: Option<Period>) -> &'static str {
    match p {
        Some(Period::Day) => "daily",
        Some(Period::Month) => "monthly",
        None => "",
    }
}

/// The alert for `event`. Fields a kind does not take are never read (and `Event::check` has refused them).
pub fn render(e: &Event) -> Rendered {
    let f = &e.fields;
    let name = |v: &Option<String>| v.clone().unwrap_or_default();
    let (title, body) = match e.kind {
        Kind::Unsealed => (
            "Audit content opened",
            "The administrator unsealed audit content. If that was not you, change the passphrase and look at the audit records."
                .to_string(),
        ),
        Kind::UnsealTimeoutsChanged => {
            ("Audit unseal timeouts changed", "The time limits of an unsealed audit session were changed in the admin.".to_string())
        }
        Kind::RetentionChanged => ("Audit retention changed", format!("The audit retention was set to {} days.", f.count.unwrap_or(0))),
        Kind::BackupKeyChanged => (
            "Backup key changed",
            "The backup key was set or replaced. Older backups need the passphrase that was current when they were made.".to_string(),
        ),
        Kind::AuditDiskLow => (
            "Audit disk space is low",
            format!("The audit store has {} MB free. New model calls stop below 256 MB.", f.free_mb.unwrap_or(0)),
        ),
        Kind::AuditDiskCritical => (
            "Audit disk space is critical",
            format!("The audit store has {} MB free. New model calls are refused until space is freed.", f.free_mb.unwrap_or(0)),
        ),
        Kind::ComponentSilent => {
            ("A component has gone quiet", format!("Nothing has come from {} for {} minutes.", name(&f.link), f.minutes.unwrap_or(0)))
        }
        Kind::CaddySilent => (
            "Caddy has gone quiet",
            format!("Replies were recorded, but Caddy has sent no access line for {} minutes.", f.minutes.unwrap_or(0)),
        ),
        Kind::RestartLoop => (
            "A service keeps restarting",
            format!("{} restarted {} times in {} minutes.", name(&f.service), f.count.unwrap_or(0), f.minutes.unwrap_or(0)),
        ),
        Kind::ChainMismatch => (
            "The audit chain does not match",
            "The audit chain the admin stored no longer matches the auditor's. Records may have been rolled back or changed.".to_string(),
        ),
        Kind::RejectedToken => (
            "A link's token was rejected",
            format!("The token for {} was refused. Rotate it from the host if this was not planned.", name(&f.link)),
        ),
        Kind::BudgetThreshold => {
            ("A token budget is near its limit", format!("The {} token budget has reached {}%.", period(f.period), f.percent.unwrap_or(0)))
        }
        Kind::BackupFailed => ("A backup failed", "The last backup did not finish. Look at the Backup page.".to_string()),
        Kind::BackupMissed => ("A backup is overdue", format!("No backup has finished for {} minutes.", f.minutes.unwrap_or(0))),
        Kind::RestorePerformed => ("A restore was performed", "A backup was restored into this install.".to_string()),
        Kind::ReleaseAvailable => ("A new Askbar release", format!("Askbar {} is available.", name(&f.version))),
        Kind::Test => ("Askbar test alert", "This is a test alert. If you can read it, delivery works.".to_string()),
    };
    Rendered { title: title.to_string(), body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::alerter::{Fields, Sender};

    /// A well-formed event of `kind`, with a marker in every string field.
    pub(crate) fn sample(kind: Kind) -> Event {
        let mut f = Fields::default();
        for field in kind.fields() {
            use askbar_proto::alerter::Field::*;
            match field {
                Link => f.link = Some("admin-server".into()),
                Service => f.service = Some("model-gateway".into()),
                Period => f.period = Some(askbar_proto::alerter::Period::Month),
                Percent => f.percent = Some(80),
                Count => f.count = Some(3),
                Minutes => f.minutes = Some(7),
                FreeMb => f.free_mb = Some(512),
                Version => f.version = Some("1.2.3".into()),
            }
        }
        let severity = if kind == Kind::BudgetThreshold { askbar_proto::alerter::Severity::Warning } else { kind.severity() };
        Event { kind, severity, at: 1, fields: f }
    }

    #[test]
    fn every_kind_has_a_sentence_built_from_its_own_fields() {
        for k in Kind::ALL {
            let e = sample(k);
            e.is_well_formed().unwrap_or_else(|r| panic!("{}: {r}", k.name()));
            let r = render(&e);
            assert!(!r.title.is_empty() && !r.body.is_empty(), "{}", k.name());
            assert!(!r.body.contains("{}") && !r.body.contains("None"), "{}: {}", k.name(), r.body);
        }
        // Each sender has at least one kind (the list is not empty for any link).
        assert!(Kind::ALL.iter().any(|k| k.senders().contains(&Sender::Auditor)));
    }

    #[test]
    fn the_sentence_names_the_fields_and_nothing_a_sender_could_add() {
        let r = render(&sample(Kind::ComponentSilent));
        assert_eq!(r.body, "Nothing has come from admin-server for 7 minutes.");
        let r = render(&sample(Kind::BudgetThreshold));
        assert_eq!(r.body, "The monthly token budget has reached 80%.");
        // Two events of one kind differ only in their typed fields.
        let mut a = sample(Kind::Unsealed);
        let b = sample(Kind::Unsealed);
        a.at = 999;
        assert_eq!(render(&a), render(&b));
    }
}
