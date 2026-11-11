//! Repeat limiting (ADR 0055, ADR 0024): one delivery for each derived key in a window of 60 minutes, with the count of
//! suppressed repeats added to the next one, and no more than 20 deliveries an hour in all; when that cap is hit it sends
//! one summary and nothing else until the hour ends.
//!
//! The key is derived from the kind and the typed fields, never from anything a sender supplies as a key.

use askbar_proto::alerter::Event;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One delivery for each key in this many seconds.
pub const WINDOW_SECS: i64 = 60 * 60;
/// No more deliveries than this in an hour, in all.
pub const HOURLY_CAP: usize = 20;

/// The key of an event: its kind and its typed fields. Two events that say the same thing have the same key.
pub fn key(e: &Event) -> String {
    let f = &e.fields;
    let mut parts = vec![e.kind.name().to_string()];
    if let Some(v) = &f.link {
        parts.push(format!("link={v}"));
    }
    if let Some(v) = &f.service {
        parts.push(format!("service={v}"));
    }
    if let Some(v) = f.period {
        parts.push(format!("period={}", serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()));
    }
    if let Some(v) = f.percent {
        parts.push(format!("percent={v}"));
    }
    if let Some(v) = &f.version {
        parts.push(format!("version={v}"));
    }
    // A count, minutes or free space changes as a condition goes on: they are not part of what makes two alerts the same.
    parts.join("|")
}

/// What to do with an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admit {
    /// Deliver it. `suppressed` is how many repeats of this key were held back since the last delivery.
    Send { suppressed: u32 },
    /// Hold it back: the same key was delivered in the last hour. It is counted.
    Suppress,
    /// The hourly cap was just hit: deliver one summary, `held` events are being held back.
    Summary { held: u32 },
    /// The cap is hit and the summary is sent: nothing more this hour.
    Drop,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyState {
    /// Unix seconds of the last delivery of this key.
    pub last_sent: i64,
    /// Repeats held back since then.
    pub suppressed: u32,
}

/// The limiter's memory, which the ledger keeps on disk: keys, times and counts, and nothing else.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub keys: BTreeMap<String, KeyState>,
    /// When each delivery in the last hour was made.
    #[serde(default)]
    pub recent: Vec<i64>,
    /// Whether the summary for this hour's cap has been sent, and how many events it held back.
    #[serde(default)]
    pub summary_sent: bool,
    #[serde(default)]
    pub held: u32,
    /// Unix seconds of the last delivery that worked and the last that failed.
    #[serde(default)]
    pub last_ok: Option<i64>,
    #[serde(default)]
    pub last_failed: Option<i64>,
}

impl State {
    /// Decides for `event` at `now`. A `Send` or `Summary` is counted as a delivery.
    pub fn admit(&mut self, event: &Event, now: i64) -> Admit {
        self.recent.retain(|t| now - *t < WINDOW_SECS);
        self.keys.retain(|_, k| now - k.last_sent < WINDOW_SECS || k.suppressed > 0 && now - k.last_sent < 2 * WINDOW_SECS);
        let key = key(event);
        if let Some(k) = self.keys.get_mut(&key) {
            if now - k.last_sent < WINDOW_SECS {
                k.suppressed = k.suppressed.saturating_add(1);
                return Admit::Suppress;
            }
        }
        if self.recent.len() >= HOURLY_CAP {
            self.held = self.held.saturating_add(1);
            if self.summary_sent {
                return Admit::Drop;
            }
            self.summary_sent = true;
            return Admit::Summary { held: self.held };
        }
        // Back under the cap: the next time it is hit is a new hour's.
        self.summary_sent = false;
        self.held = 0;
        let k = self.keys.entry(key).or_default();
        let suppressed = std::mem::take(&mut k.suppressed);
        k.last_sent = now;
        self.recent.push(now);
        Admit::Send { suppressed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::alerter::{Fields, Kind, Severity};

    fn ev(kind: Kind, link: Option<&str>, minutes: Option<u32>) -> Event {
        Event { kind, severity: kind.severity(), at: 0, fields: Fields { link: link.map(Into::into), minutes, ..Fields::default() } }
    }

    #[test]
    fn the_key_comes_from_the_kind_and_what_names_the_condition_and_ignores_a_changing_number() {
        let a = key(&ev(Kind::ComponentSilent, Some("server-auditor"), Some(5)));
        let b = key(&ev(Kind::ComponentSilent, Some("server-auditor"), Some(40)));
        let c = key(&ev(Kind::ComponentSilent, Some("model-gateway-auditor"), Some(5)));
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, key(&ev(Kind::RejectedToken, Some("server-auditor"), None)));
        let warn = Event { severity: Severity::Warning, ..ev(Kind::Unsealed, None, None) };
        assert_eq!(key(&warn), "unsealed");
    }

    #[test]
    fn one_delivery_for_a_key_in_an_hour_then_the_count_comes_with_the_next() {
        let mut s = State::default();
        let e = ev(Kind::RejectedToken, Some("admin-server"), None);
        assert_eq!(s.admit(&e, 1_000), Admit::Send { suppressed: 0 });
        for t in [1_100, 1_200, 4_000] {
            assert_eq!(s.admit(&e, t), Admit::Suppress);
        }
        // Another key is not held back.
        assert_eq!(s.admit(&ev(Kind::RejectedToken, Some("admin-gateway"), None), 1_300), Admit::Send { suppressed: 0 });
        // After the hour the same key goes out with the three it held back.
        assert_eq!(s.admit(&e, 1_000 + WINDOW_SECS), Admit::Send { suppressed: 3 });
    }

    #[test]
    fn past_twenty_an_hour_one_summary_and_then_nothing_until_the_hour_ends() {
        let mut s = State::default();
        for i in 0..HOURLY_CAP {
            let e = ev(Kind::RejectedToken, Some(&format!("link-{i}")), None);
            assert_eq!(s.admit(&e, 100 + i as i64), Admit::Send { suppressed: 0 }, "{i}");
        }
        let next = |n: usize| ev(Kind::RejectedToken, Some(&format!("other-{n}")), None);
        assert_eq!(s.admit(&next(0), 200), Admit::Summary { held: 1 });
        for n in 1..5 {
            assert_eq!(s.admit(&next(n), 300 + n as i64), Admit::Drop);
        }
        // When the first deliveries have aged out, delivery resumes, and a later cap gets its own summary.
        assert_eq!(s.admit(&next(9), 100 + WINDOW_SECS + 25), Admit::Send { suppressed: 0 });
        assert!(!s.summary_sent);
    }

    #[test]
    fn the_state_round_trips_through_json_and_holds_only_keys_times_and_counts() {
        let mut s = State::default();
        s.admit(&ev(Kind::ComponentSilent, Some("server-auditor"), Some(5)), 10);
        s.last_ok = Some(11);
        let text = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<State>(&text).unwrap(), s);
        assert!(text.contains("component_silent|link=server-auditor"), "{text}");
    }
}
