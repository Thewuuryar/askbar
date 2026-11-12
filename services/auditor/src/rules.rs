//! The alert rules (ADR 0051, ADR 0055): what the auditor says to the alerter because of what it sees. It decides when; the
//! alerter decides what is said and how. An event is a kind, a severity and typed fields, never a record's content.
//!
//! The engine is fed three ways: after each record it appends ([`Rules::fed`]), after each request on a link
//! ([`Rules::seen`]), and every 30 seconds ([`Rules::tick`], for what is about time and disk). Every method takes the time as a
//! number, so a test drives a day in a moment with a fake clock; nothing here reads a clock or a disk.

use askbar_proto::alerter::{Fields, Kind, Severity};
use askbar_service::alert_client::AlertClient;
use askbar_service::restart_loop::RestartLoop;
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
    /// A reply began (a `chat` turn span): something went through Caddy to the server a moment ago.
    TurnStarted,
    /// Caddy sent an access line.
    AccessLine,
}

/// The two limits on how long sealed content stays open, in minutes, as the last record of a change left them.
#[derive(Clone, Copy, PartialEq, Eq)]
struct UnsealLimits {
    idle: u32,
    total: u32,
}

/// Where the audit volume is, as far as the alerts for it have said.
#[derive(Default)]
struct DiskState {
    /// `audit_disk_low` was sent and the volume has not yet recovered.
    low: bool,
    /// `audit_disk_critical` was sent and the volume has not yet recovered.
    critical: bool,
}

/// Below this much free, `audit_disk_low`; it is armed again above [`LOW_REARM`], so a volume that hovers is one alert.
pub const LOW_BYTES: u64 = 1024 * 1024 * 1024;
pub const LOW_REARM: u64 = LOW_BYTES * 5 / 4;
/// Below this much free, `audit_disk_critical` (the auditor itself starts refusing new spans here: `Limits::min_free_bytes`).
pub const CRITICAL_BYTES: u64 = 256 * 1024 * 1024;
pub const CRITICAL_REARM: u64 = CRITICAL_BYTES * 5 / 4;

/// What the silence rules know about Caddy: a turn that went through it and no line about it.
#[derive(Default)]
struct CaddyState {
    /// The auditor takes Caddy's lines at all. Without that listener there is nothing to be silent.
    watched: bool,
    last_turn: Option<i64>,
    last_line: Option<i64>,
    /// `caddy_silent` was sent and no line has come since.
    reported: bool,
}

/// A link is silent when nothing has come over it for this long (ADR 0051). The services call `GET /status` every minute, so a
/// quiet but alive service is not silent.
pub const SILENT_AFTER_SECS: i64 = 5 * 60;
/// Caddy is silent when a reply began and no access line has come in this long since.
pub const CADDY_SILENT_AFTER_SECS: i64 = 15 * 60;

/// The links whose silence is an alert: the caller the auditor knows each by, and the link's name (the field of the event).
/// The admin's append link and the gateway's read link are left out because they are quiet by nature (the first speaks only
/// when someone changes something).
const WATCHED_LINKS: [(&str, &str); 4] = [
    ("server", "server-auditor"),
    ("model-gateway", "model-gateway-auditor"),
    ("alerter", "alerter-auditor"),
    ("admin-read", "admin-auditor-read"),
];

struct State {
    unseal: UnsealLimits,
    disk: DiskState,
    /// When this process began: a link that has said nothing since then is silent from then.
    started_at: i64,
    /// The callers `component_silent` was sent for and that have not spoken since.
    silent: std::collections::BTreeSet<&'static str>,
    caddy: CaddyState,
    /// The admin's boot IDs, from its hellos.
    admin_boots: RestartLoop,
}

impl State {
    fn new(now: i64) -> State {
        State {
            unseal: UnsealLimits {
                idle: askbar_proto::audit::DEFAULT_UNSEAL_IDLE_MINUTES,
                total: askbar_proto::audit::DEFAULT_UNSEAL_TOTAL_MINUTES,
            },
            disk: DiskState::default(),
            started_at: now,
            silent: Default::default(),
            caddy: CaddyState::default(),
            admin_boots: RestartLoop::default(),
        }
    }
}

pub struct Rules {
    alerts: Arc<dyn Alerts>,
    state: Mutex<State>,
}

impl Rules {
    /// `now` is when the process began: a link that has not spoken since is silent from then, not from the epoch.
    pub fn new(alerts: Arc<dyn Alerts>, now: i64) -> Rules {
        Rules { alerts, state: Mutex::new(State::new(now)) }
    }

    /// Caddy's access lines are taken here, so their absence means something.
    pub fn watch_caddy(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).caddy.watched = true;
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

    /// The retention was set to `days`, from `old`. Only a change is an alert: the admin pushes the setting every half minute.
    pub fn retention_changed(&self, old: u32, days: u32) {
        if old != days {
            self.say(Kind::RetentionChanged, Fields { count: Some(days), ..Fields::default() });
        }
    }

    /// A record was appended (`now` in unix seconds).
    pub fn fed(&self, fed: &Fed, now: i64) {
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
            // The admin's own restarts, from the boot ID in each hello. (The others are the admin's to count: it polls them.)
            Fed::Hello { service, boot_id } if service == "admin" => {
                let begun = self.state.lock().unwrap_or_else(|e| e.into_inner()).admin_boots.saw(boot_id, now);
                if let Some(count) = begun {
                    self.say(
                        Kind::RestartLoop,
                        Fields {
                            service: Some("admin".into()),
                            count: Some(count),
                            minutes: Some((askbar_service::restart_loop::WINDOW_SECS / 60) as u32),
                            ..Fields::default()
                        },
                    );
                }
            }
            Fed::Hello { .. } => {}
            Fed::TurnStarted => self.state.lock().unwrap_or_else(|e| e.into_inner()).caddy.last_turn = Some(now),
            Fed::AccessLine => {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                s.caddy.last_line = Some(now);
                s.caddy.reported = false;
            }
        }
    }

    /// A request passed the token check on the link of `caller`: it is not silent any more.
    pub fn seen(&self, caller: &str, _now: i64) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).silent.retain(|c| *c != caller);
    }

    /// Every 30 seconds: what is about time and disk. `free` is the bytes free where the store is, if it could be told;
    /// `last_requests` is when each link last called, by caller.
    pub fn tick(&self, now: i64, free: Option<u64>, last_requests: &std::collections::BTreeMap<&'static str, i64>) {
        if let Some(free) = free {
            self.disk(free);
        }
        self.silence(now, last_requests);
    }

    /// Links that have gone quiet, and Caddy. Each is said once and again only after it has spoken.
    fn silence(&self, now: i64, last_requests: &std::collections::BTreeMap<&'static str, i64>) {
        let mut to_say = Vec::new();
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            for (caller, link) in WATCHED_LINKS {
                let since = last_requests.get(caller).copied().unwrap_or(s.started_at).max(s.started_at);
                if now - since >= SILENT_AFTER_SECS {
                    if s.silent.insert(caller) {
                        to_say.push((
                            Kind::ComponentSilent,
                            Fields { link: Some(link.to_string()), minutes: Some(((now - since) / 60) as u32), ..Fields::default() },
                        ));
                    }
                } else {
                    s.silent.remove(caller);
                }
            }
            // Caddy logs a request when it ends, so a reply that went through it has a line behind it. A reply began and, fifteen
            // minutes later, no line has come since: the lines have stopped. (Comparing the last line with the clock instead
            // would raise this for every reply after a quiet hour.)
            let c = &mut s.caddy;
            let waiting_since = c.last_turn.filter(|t| c.last_line.is_none_or(|l| l < *t));
            if let Some(t) = waiting_since.filter(|t| c.watched && now - *t >= CADDY_SILENT_AFTER_SECS) {
                if !c.reported {
                    c.reported = true;
                    to_say.push((Kind::CaddySilent, Fields { minutes: Some(((now - t) / 60) as u32), ..Fields::default() }));
                }
            }
        }
        for (kind, fields) in to_say {
            self.say(kind, fields);
        }
    }

    /// The volume's free space. Critical below 256 MiB and low below 1 GiB, each said once, and said again only after the
    /// volume has been above its re-arm line (320 MiB and 1.25 GiB): a volume that hovers is not a stream of alerts. Going
    /// straight to critical says critical alone, because it already says everything low would.
    fn disk(&self, free: u64) {
        let (critical, low) = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let d = &mut s.disk;
            let mut critical = false;
            let mut low = false;
            if free < CRITICAL_BYTES && !d.critical {
                d.critical = true;
                d.low = true;
                critical = true;
            } else if d.critical && free >= CRITICAL_REARM {
                d.critical = false;
            }
            if free < LOW_BYTES && !d.low {
                d.low = true;
                low = true;
            } else if d.low && !d.critical && free >= LOW_REARM {
                d.low = false;
            }
            (critical, low)
        };
        let free_mb = Fields { free_mb: Some(free / (1024 * 1024)), ..Fields::default() };
        if critical {
            self.say(Kind::AuditDiskCritical, free_mb.clone());
        }
        if low {
            self.say(Kind::AuditDiskLow, free_mb);
        }
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
        // The server's replies come through `chat` turns; the admin's test of a provider does not go through Caddy.
        Record::Start(s) if s.kind == "turn" && s.name == "chat" => Some(Fed::TurnStarted),
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
        (Rules::new(c.clone(), 0), c)
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

    #[test]
    fn a_new_retention_is_an_alert_with_its_days_and_the_same_one_pushed_again_is_not() {
        let (r, c) = rules();
        r.retention_changed(30, 30);
        assert!(c.kinds().is_empty());
        r.retention_changed(30, 7);
        assert_eq!(c.kinds(), [Kind::RetentionChanged]);
        assert_eq!(c.0.lock().unwrap()[0].2, Fields { count: Some(7), ..Fields::default() });
    }

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn low_disk_is_said_once_and_again_only_after_the_volume_was_above_its_rearm_line() {
        let (r, c) = rules();
        let tick = |free: u64| r.tick(0, Some(free), &Default::default());
        tick(5 * 1024 * MIB);
        tick(1025 * MIB);
        assert!(c.kinds().is_empty(), "just over 1 GiB");
        tick(1000 * MIB);
        assert_eq!(c.kinds(), [Kind::AuditDiskLow]);
        assert_eq!(c.0.lock().unwrap()[0].2, Fields { free_mb: Some(1000), ..Fields::default() });
        tick(900 * MIB);
        tick(1100 * MIB);
        tick(1000 * MIB);
        tick(1200 * MIB);
        tick(990 * MIB);
        assert_eq!(c.kinds().len(), 1, "hovering between 1 GiB and 1.25 GiB is one alert");
        tick(1281 * MIB);
        tick(999 * MIB);
        assert_eq!(c.kinds().len(), 2, "above 1.25 GiB re-arms it");
    }

    #[test]
    fn critical_disk_is_critical_alone_when_it_falls_straight_there_and_has_its_own_hysteresis() {
        let (r, c) = rules();
        let tick = |free: u64| r.tick(0, Some(free), &Default::default());
        tick(10 * 1024 * MIB);
        tick(200 * MIB);
        assert_eq!(c.kinds(), [Kind::AuditDiskCritical], "straight to critical says critical only");
        assert_eq!(c.0.lock().unwrap()[0].1, Severity::Critical);
        tick(100 * MIB);
        tick(300 * MIB);
        tick(255 * MIB);
        assert_eq!(c.kinds().len(), 1, "300 MiB is not yet recovered");
        // Back over 320 MiB, still under 1 GiB: critical is armed again, low was already said.
        tick(400 * MIB);
        tick(200 * MIB);
        assert_eq!(c.kinds(), [Kind::AuditDiskCritical, Kind::AuditDiskCritical]);
        // A volume that fell from low into critical says both, low first of all.
        let (r, c) = rules();
        r.tick(0, Some(900 * MIB), &Default::default());
        r.tick(0, Some(100 * MIB), &Default::default());
        assert_eq!(c.kinds(), [Kind::AuditDiskLow, Kind::AuditDiskCritical]);
        // A volume whose free space cannot be told says nothing.
        let (r, c) = rules();
        r.tick(0, None, &Default::default());
        assert!(c.kinds().is_empty());
    }

    fn calls(pairs: &[(&'static str, i64)]) -> std::collections::BTreeMap<&'static str, i64> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn a_link_that_says_nothing_for_five_minutes_is_silent_once_and_again_only_after_it_spoke() {
        let (r, c) = rules();
        let all = |t: i64| calls(&[("server", t), ("model-gateway", t), ("alerter", t), ("admin-read", t)]);
        r.tick(60, None, &all(60));
        r.tick(299, None, &all(0));
        assert!(c.kinds().is_empty(), "four minutes and fifty-nine seconds is not five");
        // The gateway has not called since 100; the others have.
        let seen = calls(&[("server", 390), ("model-gateway", 100), ("alerter", 390), ("admin-read", 390)]);
        r.tick(400, None, &seen);
        assert_eq!(c.kinds(), [Kind::ComponentSilent]);
        let e = c.0.lock().unwrap()[0].clone();
        assert_eq!(
            (e.1, e.2),
            (Severity::Critical, Fields { link: Some("model-gateway-auditor".into()), minutes: Some(5), ..Fields::default() })
        );
        r.tick(430, None, &calls(&[("server", 420), ("model-gateway", 100), ("alerter", 420), ("admin-read", 420)]));
        r.tick(700, None, &calls(&[("server", 690), ("model-gateway", 100), ("alerter", 690), ("admin-read", 690)]));
        assert_eq!(c.kinds().len(), 1, "silence that goes on is said once");
        // It speaks, and goes quiet again: a new alert.
        r.seen("model-gateway", 800);
        r.tick(805, None, &calls(&[("server", 800), ("model-gateway", 800), ("alerter", 800), ("admin-read", 800)]));
        r.tick(1200, None, &calls(&[("server", 1190), ("model-gateway", 800), ("alerter", 1190), ("admin-read", 1190)]));
        assert_eq!(c.kinds(), [Kind::ComponentSilent, Kind::ComponentSilent]);
    }

    #[test]
    fn a_link_never_heard_from_is_silent_five_minutes_after_the_auditor_began_and_not_before() {
        let c = Arc::new(Collector::default());
        let r = Rules::new(c.clone(), 10_000);
        let none = calls(&[]);
        r.tick(10_200, None, &none);
        assert!(c.kinds().is_empty(), "the auditor has been up three minutes");
        r.tick(10_300, None, &none);
        assert_eq!(c.kinds(), [Kind::ComponentSilent; 4], "all four, once each");
        let mut links: Vec<_> = c.0.lock().unwrap().iter().map(|e| e.2.link.clone().unwrap()).collect();
        links.sort();
        assert_eq!(links, ["admin-auditor-read", "alerter-auditor", "model-gateway-auditor", "server-auditor"]);
    }

    #[test]
    fn caddy_is_silent_when_a_reply_began_and_fifteen_minutes_later_no_line_has_come_and_only_if_it_is_watched() {
        let quiet = |r: &Rules, t: i64| r.tick(t, None, &calls(&[("server", t), ("model-gateway", t), ("alerter", t), ("admin-read", t)]));
        // Not watched (no ingest listener): never.
        let (r, c) = rules();
        r.fed(&Fed::TurnStarted, 100);
        quiet(&r, 5_000);
        assert!(c.kinds().is_empty());
        // Watched.
        let (r, c) = rules();
        r.watch_caddy();
        quiet(&r, 50);
        quiet(&r, 5_000);
        assert!(c.kinds().is_empty(), "no reply, nothing to be silent about");
        r.fed(&Fed::TurnStarted, 5_000);
        quiet(&r, 5_030);
        quiet(&r, 5_000 + 899);
        assert!(c.kinds().is_empty(), "its line may still come");
        r.fed(&Fed::AccessLine, 5_100);
        quiet(&r, 5_000 + 900);
        assert!(c.kinds().is_empty(), "its line came: a quiet hour before it does not matter");
        // A second reply, and no line for fifteen minutes.
        r.fed(&Fed::TurnStarted, 9_000);
        quiet(&r, 9_000 + 899);
        quiet(&r, 9_000 + 900);
        assert_eq!(c.kinds(), [Kind::CaddySilent]);
        assert_eq!(c.0.lock().unwrap()[0].2, Fields { minutes: Some(15), ..Fields::default() });
        quiet(&r, 9_000 + 2_000);
        assert_eq!(c.kinds().len(), 1, "said once");
        // A line ends it; a later reply with no line is a new alert.
        r.fed(&Fed::AccessLine, 12_000);
        r.fed(&Fed::TurnStarted, 12_100);
        quiet(&r, 12_100 + 900);
        assert_eq!(c.kinds(), [Kind::CaddySilent, Kind::CaddySilent]);
    }

    #[test]
    fn the_admin_restarting_three_times_in_ten_minutes_is_a_restart_loop_from_its_hellos_alone() {
        let (r, c) = rules();
        let hello = |service: &str, boot: &str| Fed::Hello { service: service.into(), boot_id: boot.into() };
        for (i, boot) in ["a", "b", "c", "d"].iter().enumerate() {
            r.fed(&hello("admin", boot), i as i64 * 60);
        }
        assert_eq!(c.kinds(), [Kind::RestartLoop]);
        assert_eq!(
            c.0.lock().unwrap()[0].2,
            Fields { service: Some("admin".into()), count: Some(3), minutes: Some(10), ..Fields::default() }
        );
        // The server's and the gateway's restarts are the admin's to count: it polls them.
        let (r, c) = rules();
        for (i, boot) in ["a", "b", "c", "d", "e"].iter().enumerate() {
            r.fed(&hello("server", boot), i as i64);
            r.fed(&hello("model-gateway", boot), i as i64);
        }
        assert!(c.kinds().is_empty());
    }

    #[test]
    fn every_link_watched_for_silence_is_one_the_auditor_knows_by_that_caller() {
        for (caller, link) in WATCHED_LINKS {
            assert!(crate::LINKS.iter().any(|(name, c, _)| *name == link && *c == caller), "{caller} / {link}");
        }
        // And each is a name the alerter takes in a field.
        assert!(WATCHED_LINKS.iter().all(|(_, link)| askbar_proto::alerter::is_plain_name(link)));
    }
}
