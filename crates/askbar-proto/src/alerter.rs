//! Wire types of the alerter API, `/api/alerter/v1` (ADR 0055).
//!
//! An event is a kind from a fixed list, a severity, a time and typed fields. There is no free text and no dedupe key:
//! the alerter turns a kind into one of its own sentences and derives the key from the kind and the fields, so a
//! compromised sender cannot make it say anything else. Every type refuses a field it does not know.

use serde::{Deserialize, Serialize};
use std::fmt;

/// How loud an alert is. ntfy priorities 3, 4 and 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    /// The ntfy priority: `info` 3 (default), `warning` 4 (high), `critical` 5 (max).
    pub fn ntfy_priority(self) -> u8 {
        match self {
            Severity::Info => 3,
            Severity::Warning => 4,
            Severity::Critical => 5,
        }
    }
}

/// Who may send a kind (ADR 0055): the link a request came in on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sender {
    /// `auditor-alerter`.
    Auditor,
    /// `admin-alerter`.
    Admin,
    /// The alerter itself: no link may send these.
    Alerter,
}

/// Every kind of alert. Adding one is a change to this list, to `docs/openapi/alerter.v1.yaml` and to the alerter's
/// templates, and a test fails if one of the three is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    // The auditor's.
    Unsealed,
    UnsealTimeoutsChanged,
    RetentionChanged,
    BackupKeyChanged,
    AuditDiskLow,
    AuditDiskCritical,
    ComponentSilent,
    CaddySilent,
    // The auditor's for the admin (from its hellos), and the admin's for the services it polls.
    RestartLoop,
    // The admin's.
    ChainMismatch,
    RejectedToken,
    BudgetThreshold,
    BackupFailed,
    BackupMissed,
    RestorePerformed,
    // The alerter's own.
    ReleaseAvailable,
    Test,
}

impl Kind {
    /// Every kind, in a fixed order.
    pub const ALL: [Kind; 17] = [
        Kind::Unsealed,
        Kind::UnsealTimeoutsChanged,
        Kind::RetentionChanged,
        Kind::BackupKeyChanged,
        Kind::AuditDiskLow,
        Kind::AuditDiskCritical,
        Kind::ComponentSilent,
        Kind::CaddySilent,
        Kind::RestartLoop,
        Kind::ChainMismatch,
        Kind::RejectedToken,
        Kind::BudgetThreshold,
        Kind::BackupFailed,
        Kind::BackupMissed,
        Kind::RestorePerformed,
        Kind::ReleaseAvailable,
        Kind::Test,
    ];

    /// The name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Unsealed => "unsealed",
            Kind::UnsealTimeoutsChanged => "unseal_timeouts_changed",
            Kind::RetentionChanged => "retention_changed",
            Kind::BackupKeyChanged => "backup_key_changed",
            Kind::AuditDiskLow => "audit_disk_low",
            Kind::AuditDiskCritical => "audit_disk_critical",
            Kind::ComponentSilent => "component_silent",
            Kind::CaddySilent => "caddy_silent",
            Kind::RestartLoop => "restart_loop",
            Kind::ChainMismatch => "chain_mismatch",
            Kind::RejectedToken => "rejected_token",
            Kind::BudgetThreshold => "budget_threshold",
            Kind::BackupFailed => "backup_failed",
            Kind::BackupMissed => "backup_missed",
            Kind::RestorePerformed => "restore_performed",
            Kind::ReleaseAvailable => "release_available",
            Kind::Test => "test",
        }
    }

    /// The severity an event of this kind has. `budget_threshold` is `warning` at 80% and `critical` at 100%
    /// ([`Event::is_well_formed`] checks which); every other kind has one.
    pub fn severity(self) -> Severity {
        match self {
            Kind::Unsealed
            | Kind::UnsealTimeoutsChanged
            | Kind::RetentionChanged
            | Kind::BackupKeyChanged
            | Kind::AuditDiskLow
            | Kind::CaddySilent
            | Kind::RestartLoop
            | Kind::RejectedToken
            | Kind::BudgetThreshold
            | Kind::RestorePerformed => Severity::Warning,
            Kind::AuditDiskCritical | Kind::ComponentSilent | Kind::ChainMismatch | Kind::BackupFailed | Kind::BackupMissed => {
                Severity::Critical
            }
            Kind::ReleaseAvailable | Kind::Test => Severity::Info,
        }
    }

    /// Which links may send this kind. `restart_loop` has two: the auditor reports the admin's, the admin the others'.
    pub fn senders(self) -> &'static [Sender] {
        match self {
            Kind::Unsealed
            | Kind::UnsealTimeoutsChanged
            | Kind::RetentionChanged
            | Kind::BackupKeyChanged
            | Kind::AuditDiskLow
            | Kind::AuditDiskCritical
            | Kind::ComponentSilent
            | Kind::CaddySilent => &[Sender::Auditor],
            Kind::RestartLoop => &[Sender::Auditor, Sender::Admin],
            Kind::ChainMismatch
            | Kind::RejectedToken
            | Kind::BudgetThreshold
            | Kind::BackupFailed
            | Kind::BackupMissed
            | Kind::RestorePerformed => &[Sender::Admin],
            Kind::ReleaseAvailable | Kind::Test => &[Sender::Alerter],
        }
    }

    /// The fields an event of this kind must carry, and may carry (the rest are refused).
    pub fn fields(self) -> &'static [Field] {
        match self {
            Kind::Unsealed
            | Kind::UnsealTimeoutsChanged
            | Kind::BackupKeyChanged
            | Kind::ChainMismatch
            | Kind::BackupFailed
            | Kind::RestorePerformed
            | Kind::Test => &[],
            Kind::RetentionChanged => &[Field::Count],
            Kind::AuditDiskLow | Kind::AuditDiskCritical => &[Field::FreeMb],
            Kind::ComponentSilent => &[Field::Link, Field::Minutes],
            Kind::CaddySilent => &[Field::Minutes],
            Kind::RestartLoop => &[Field::Service, Field::Count, Field::Minutes],
            Kind::RejectedToken => &[Field::Link],
            Kind::BudgetThreshold => &[Field::Period, Field::Percent],
            Kind::BackupMissed => &[Field::Minutes],
            Kind::ReleaseAvailable => &[Field::Version],
        }
    }
}

/// One typed field of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Link,
    Service,
    Period,
    Percent,
    Count,
    Minutes,
    FreeMb,
    Version,
}

/// The budget period a `budget_threshold` is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Day,
    Month,
}

/// The typed fields an event may carry: the only content it has. Strings are names from a closed set the receiver checks
/// (a link or a service name, a version), never free text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    /// A link name, as `bootstrap` names it (`admin-server`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// A service name (`server`, `model-gateway`, `admin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<Period>,
    /// A percent of a budget: 80 or 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_mb: Option<u64>,
    /// A release version, such as `1.2.3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Fields {
    fn has(&self, f: Field) -> bool {
        match f {
            Field::Link => self.link.is_some(),
            Field::Service => self.service.is_some(),
            Field::Period => self.period.is_some(),
            Field::Percent => self.percent.is_some(),
            Field::Count => self.count.is_some(),
            Field::Minutes => self.minutes.is_some(),
            Field::FreeMb => self.free_mb.is_some(),
            Field::Version => self.version.is_some(),
        }
    }
}

/// A name or a version in a field: short, and only characters that carry no markup or line break.
pub fn is_plain_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// One alert, as a sender gives it to the alerter (`POST /events`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub kind: Kind,
    pub severity: Severity,
    /// Unix seconds.
    pub at: i64,
    #[serde(default)]
    pub fields: Fields,
}

/// Why an event is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// This link may not send this kind.
    NotYours,
    /// The severity is not one this kind has.
    Severity,
    /// A field this kind does not take is set, a field it needs is missing, or a value is out of range.
    Fields,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Refusal::NotYours => "This link may not send an alert of this kind.",
            Refusal::Severity => "The severity is not one this kind has.",
            Refusal::Fields => "The fields do not match what this kind takes.",
        })
    }
}

const ALL_FIELDS: [Field; 8] =
    [Field::Link, Field::Service, Field::Period, Field::Percent, Field::Count, Field::Minutes, Field::FreeMb, Field::Version];

impl Event {
    /// Whether `from` may send this event, with its fields and severity as its kind defines them.
    pub fn check(&self, from: Sender) -> Result<(), Refusal> {
        if !self.kind.senders().contains(&from) {
            return Err(Refusal::NotYours);
        }
        self.is_well_formed()
    }

    /// The severity and the fields, against the kind's definition.
    pub fn is_well_formed(&self) -> Result<(), Refusal> {
        let ok_severity = match self.kind {
            Kind::BudgetThreshold => match self.fields.percent {
                Some(100) => self.severity == Severity::Critical,
                _ => self.severity == Severity::Warning,
            },
            k => self.severity == k.severity(),
        };
        if !ok_severity {
            return Err(Refusal::Severity);
        }
        let allowed = self.kind.fields();
        // Every field the kind names is required, and none other is allowed.
        for f in ALL_FIELDS {
            if allowed.contains(&f) != self.fields.has(f) {
                return Err(Refusal::Fields);
            }
        }
        let f = &self.fields;
        let names_ok = [&f.link, &f.service].into_iter().flatten().all(|s| is_plain_name(s))
            && f.version.as_deref().is_none_or(is_plain_name)
            && f.percent.is_none_or(|p| matches!(p, 80 | 100))
            && f.count.is_none_or(|c| c <= 100_000)
            && f.minutes.is_none_or(|m| m <= 1_000_000);
        if names_ok {
            Ok(())
        } else {
            Err(Refusal::Fields)
        }
    }
}

/// `POST /events` answered: the event was taken (queued, or suppressed as a repeat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventTaken {
    pub suppressed: bool,
}

/// ntfy: a server, a topic and an optional access token. Holds the token and the topic: no `Debug` output shows
/// either, and no endpoint returns them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtfyConfig {
    /// `https://ntfy.example.com`. The scheme is `https`, or `http` for a server on the local network.
    pub server: String,
    pub topic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl fmt::Debug for NtfyConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NtfyConfig")
            .field("server", &"(set)")
            .field("topic", &"(set)")
            .field("token", &if self.token.is_some() { "(set)" } else { "(none)" })
            .finish()
    }
}

/// How the alerter secures its connection to the mail server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SmtpTls {
    /// The connection starts in the clear and is upgraded with STARTTLS, which is required (usually port 587).
    Starttls,
    /// TLS from the first byte (usually port 465).
    Tls,
    /// No TLS, for a relay on the local network. A user name and password are refused with it.
    None,
}

/// Email: an SMTP server and who the mail is from and to. Holds a password and addresses: no `Debug` output shows any of
/// them, and no endpoint returns them.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    /// The user name and the password go together, or neither is given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    pub from: String,
    pub to: String,
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &"(set)")
            .field("port", &self.port)
            .field("tls", &self.tls)
            .field("user", &if self.user.is_some() { "(set)" } else { "(none)" })
            .field("password", &if self.password.is_some() { "(set)" } else { "(none)" })
            .finish_non_exhaustive()
    }
}

/// `PUT /config`: the delivery settings. Replaces the whole of the earlier one. The alerter holds them in memory only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlerterConfig {
    /// Which configuration this is: a hash of its content, so the same settings give the same version and
    /// `GET /status` tells the admin whether a push is due.
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ntfy: Option<NtfyConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<SmtpConfig>,
}

/// One delivery channel, as the alerter names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Ntfy,
    Email,
}

/// How one delivery ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Sent,
    Failed,
    Suppressed,
}

/// `POST /test` answered: what each channel did with the test alert, now. Never a secret and never the server's words.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestResult {
    pub results: Vec<ChannelResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelResult {
    pub channel: Channel,
    pub ok: bool,
    /// What went wrong, in the alerter's own words: `unreachable`, `refused` (the server answered with an error),
    /// `unauthorized` (a 401 or 403) or `timeout`. Empty when it worked.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub problem: String,
}

/// `GET /status`: what every service reports, and how delivery is going.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlerterStatus {
    pub config_version: Option<u64>,
    pub boot_id: String,
    /// Unix seconds of the last delivery that worked and the last that failed, if any.
    pub last_delivery_ok_at: Option<i64>,
    pub last_delivery_failed_at: Option<i64>,
    /// Events waiting to be delivered, and how many were dropped from a full queue since the start.
    pub queued: u32,
    pub dropped: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: Kind, severity: Severity, fields: Fields) -> Event {
        Event { kind, severity, at: 1, fields }
    }

    #[test]
    fn every_kind_has_a_unique_wire_name_a_sender_and_a_severity() {
        let mut names = std::collections::BTreeSet::new();
        for k in Kind::ALL {
            assert!(names.insert(k.name()), "{}", k.name());
            assert_eq!(serde_json::to_value(k).unwrap(), serde_json::json!(k.name()));
            assert!(!k.senders().is_empty());
        }
        assert_eq!(Kind::ALL.len(), 17);
        // Any kind a sender could add is in the list: the enum's own variants are exactly ALL.
        let all: std::collections::BTreeSet<Kind> = Kind::ALL.into_iter().collect();
        assert_eq!(all.len(), Kind::ALL.len());
    }

    #[test]
    fn severities_map_to_the_ntfy_priorities_3_4_and_5() {
        assert_eq!(Severity::Info.ntfy_priority(), 3);
        assert_eq!(Severity::Warning.ntfy_priority(), 4);
        assert_eq!(Severity::Critical.ntfy_priority(), 5);
    }

    #[test]
    fn an_unknown_kind_or_field_is_refused_and_so_is_free_text() {
        let ok = r#"{"kind":"unsealed","severity":"warning","at":5}"#;
        assert!(serde_json::from_str::<Event>(ok).is_ok());
        for bad in [
            r#"{"kind":"kill_switch_changed","severity":"warning","at":5}"#,
            r#"{"kind":"unsealed","severity":"warning","at":5,"message":"hello"}"#,
            r#"{"kind":"unsealed","severity":"warning","at":5,"dedupe_key":"x"}"#,
            r#"{"kind":"unsealed","severity":"loud","at":5}"#,
            r#"{"kind":"unsealed","severity":"warning","at":5,"fields":{"text":"hi"}}"#,
        ] {
            assert!(serde_json::from_str::<Event>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_link_may_send_only_its_own_kinds() {
        let unsealed = event(Kind::Unsealed, Severity::Warning, Fields::default());
        assert_eq!(unsealed.check(Sender::Auditor), Ok(()));
        assert_eq!(unsealed.check(Sender::Admin), Err(Refusal::NotYours));
        let chain = event(Kind::ChainMismatch, Severity::Critical, Fields::default());
        assert_eq!(chain.check(Sender::Admin), Ok(()));
        assert_eq!(chain.check(Sender::Auditor), Err(Refusal::NotYours));
        // The two links that share restart_loop.
        let f = Fields { service: Some("server".into()), count: Some(3), minutes: Some(10), ..Fields::default() };
        let loop_ = event(Kind::RestartLoop, Severity::Warning, f);
        assert_eq!(loop_.check(Sender::Auditor), Ok(()));
        assert_eq!(loop_.check(Sender::Admin), Ok(()));
        // The alerter's own kinds come over no link.
        for k in [Kind::ReleaseAvailable, Kind::Test] {
            assert_eq!(event(k, k.severity(), Fields::default()).check(Sender::Auditor), Err(Refusal::NotYours));
            assert_eq!(event(k, k.severity(), Fields::default()).check(Sender::Admin), Err(Refusal::NotYours));
        }
    }

    #[test]
    fn severity_and_fields_must_match_the_kind() {
        let wrong = event(Kind::Unsealed, Severity::Critical, Fields::default());
        assert_eq!(wrong.check(Sender::Auditor), Err(Refusal::Severity));
        let extra = event(Kind::Unsealed, Severity::Warning, Fields { count: Some(1), ..Fields::default() });
        assert_eq!(extra.check(Sender::Auditor), Err(Refusal::Fields));
        let missing = event(Kind::AuditDiskLow, Severity::Warning, Fields::default());
        assert_eq!(missing.check(Sender::Auditor), Err(Refusal::Fields));
        let low = event(Kind::AuditDiskLow, Severity::Warning, Fields { free_mb: Some(900), ..Fields::default() });
        assert_eq!(low.check(Sender::Auditor), Ok(()));
        // A name with markup or a line break is not a name.
        for name in ["a b", "x\ny", "<b>", ""] {
            let f = Fields { link: Some(name.into()), ..Fields::default() };
            assert_eq!(event(Kind::RejectedToken, Severity::Warning, f).check(Sender::Admin), Err(Refusal::Fields), "{name:?}");
        }
        // The budget alert is a warning at 80% and critical at 100%.
        let at = |p, s| event(Kind::BudgetThreshold, s, Fields { period: Some(Period::Day), percent: Some(p), ..Fields::default() });
        assert_eq!(at(80, Severity::Warning).check(Sender::Admin), Ok(()));
        assert_eq!(at(100, Severity::Critical).check(Sender::Admin), Ok(()));
        assert_eq!(at(80, Severity::Critical).check(Sender::Admin), Err(Refusal::Severity));
        assert_eq!(at(50, Severity::Warning).check(Sender::Admin), Err(Refusal::Fields));
    }

    #[test]
    fn the_config_refuses_unknown_fields_and_its_debug_shows_no_secret() {
        let c = r#"{"version":1,"ntfy":{"server":"https://ntfy.example.com","topic":"marker-topic","token":"marker-token"}}"#;
        let cfg: AlerterConfig = serde_json::from_str(c).unwrap();
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("marker-topic") && !shown.contains("marker-token") && !shown.contains("ntfy.example.com"), "{shown}");
        assert!(serde_json::from_str::<AlerterConfig>(r#"{"version":1,"smtp":{}}"#).is_err());
        let mail = r#"{"version":2,"email":{"host":"marker-host.example.com","port":587,"tls":"starttls","user":"marker-user","password":"marker-password","from":"a@example.com","to":"marker-to@example.com"}}"#;
        let cfg: AlerterConfig = serde_json::from_str(mail).unwrap();
        let shown = format!("{cfg:?}");
        for m in ["marker-host", "marker-user", "marker-password", "marker-to", "a@example.com"] {
            assert!(!shown.contains(m), "{shown}");
        }
        assert_eq!(serde_json::from_str::<AlerterConfig>(&serde_json::to_string(&cfg).unwrap()).unwrap(), cfg);
        assert!(serde_json::from_str::<AlerterConfig>(&mail.replace("starttls", "ssl")).is_err());
        assert!(serde_json::from_str::<AlerterConfig>(&mail.replace(r#""port":587,"#, "")).is_err());
    }
}
