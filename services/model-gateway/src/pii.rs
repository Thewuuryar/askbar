//! The gateway's side of PII handling (plan PII-P1.2 to P2): placeholders for what `askbar_service::pii::detect` finds, the reply
//! filter that restores them, and the `redact` step's attributes. A value found is never logged, put in an error or an
//! attribute: only counts leave here. What is detected, and how, is in `askbar_service::pii`.

use askbar_proto::gateway::ChatMessage;
use askbar_proto::pii::{PiiAction, PiiConfig, PiiMode, CARD, CUSTOM, EMAIL, IBAN, PHONE};
use askbar_service::pii::detect;

/// Said once to the model when a placeholder went in. It names no value.
pub const PLACEHOLDER_NOTE: &str = "Some private values in this conversation are replaced by placeholders like \u{27e6}EMAIL_1\u{27e7}. They stand for the real values: reuse them exactly as written.";

/// A request as the provider gets it, with what the reply filter needs. Built once per request, kept in memory only:
/// `reverse` holds real values and must never be logged, recorded or returned (plan Decisions).
pub struct Redacted {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    /// (placeholder, first original text), in numbering order. For the reply filter (PII-P2.2).
    pub reverse: Vec<(String, String)>,
    /// The `redact` step's attributes: counts by type, what the configuration does with each type, the mode, and in
    /// `enforce` the number of values replaced. `None` when nothing was found, so a clean request adds nothing to the
    /// trace. No value, snippet or offset.
    pub step: Option<serde_json::Value>,
    /// In `enforce`, the count of values whose action is `Block`, by type (PII-P2.3). Types and counts only.
    pub blocked: std::collections::BTreeMap<&'static str, u64>,
}

/// What makes two values the same value: email and custom terms ignore case, a phone is its digits (and a leading `+`),
/// a card or IBAN its letters and digits in capitals. Secrets and the rest are exact.
fn normalise(ty: &str, v: &str) -> String {
    match ty {
        EMAIL | CUSTOM => v.to_lowercase(),
        PHONE => v.chars().filter(|c| c.is_ascii_digit() || (*c == '+' && v.starts_with('+'))).collect(),
        CARD | IBAN => v.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_uppercase()).collect(),
        _ => v.to_string(),
    }
}

/// Detects over the system text, then the messages in order. In `enforce` each value whose action is `Pseudonymize`
/// becomes `⟦TYPE_n⟧`, `n` counting the distinct values of that type by first occurrence. `Block` and `Allow` values stay
/// (a blocked value is refused at the gate, PII-P2.3). Any other mode changes nothing. Deterministic: no state outside
/// the arguments.
pub fn redact(cfg: &PiiConfig, system: &str, messages: &[ChatMessage]) -> Redacted {
    let enforce = cfg.enabled && cfg.mode == PiiMode::Enforce;
    let mut counts = std::collections::BTreeMap::<&str, u64>::new();
    let mut seen: Vec<(&str, String)> = Vec::new(); // (type, normalised value); the index in `reverse` is the order
    let mut reverse: Vec<(String, String)> = Vec::new();
    let mut replaced = 0u64;
    let mut blocked = std::collections::BTreeMap::<&'static str, u64>::new();
    let mut one = |text: &str| -> String {
        let spans = detect(text, cfg);
        let (mut out, mut at) = (String::new(), 0);
        for s in &spans {
            *counts.entry(s.ty).or_default() += 1;
            if enforce && cfg.action_for(s.ty) == PiiAction::Block {
                *blocked.entry(s.ty).or_default() += 1;
            }
            if !enforce || cfg.action_for(s.ty) != PiiAction::Pseudonymize {
                continue;
            }
            let (orig, norm) = (&text[s.start..s.end], normalise(s.ty, &text[s.start..s.end]));
            let n = match seen.iter().filter(|(t, _)| *t == s.ty).position(|(_, v)| *v == norm) {
                Some(i) => i + 1,
                None => {
                    seen.push((s.ty, norm));
                    let n = seen.iter().filter(|(t, _)| *t == s.ty).count();
                    reverse.push((format!("\u{27e6}{}_{n}\u{27e7}", s.ty), orig.to_string()));
                    n
                }
            };
            out.push_str(&text[at..s.start]);
            out.push_str(&format!("\u{27e6}{}_{n}\u{27e7}", s.ty));
            at = s.end;
            replaced += 1;
        }
        out.push_str(&text[at..]);
        out
    };
    let mut system = one(system);
    let messages: Vec<ChatMessage> = messages.iter().map(|m| ChatMessage { role: m.role.clone(), content: one(&m.content) }).collect();
    if replaced > 0 {
        system = if system.is_empty() { PLACEHOLDER_NOTE.into() } else { format!("{system}\n\n{PLACEHOLDER_NOTE}") };
    }
    let step = (!counts.is_empty()).then(|| {
        let actions: std::collections::BTreeMap<&str, _> = counts.keys().map(|ty| (*ty, cfg.action_for(ty))).collect();
        let mut pii = serde_json::json!({ "counts": counts, "actions": actions, "mode": cfg.mode });
        if enforce {
            pii["replaced"] = replaced.into();
        }
        serde_json::json!({ "pii": pii })
    });
    Redacted { system, messages, reverse, step, blocked }
}

/// Puts the original values back into the reply as it streams (PII-P2.2). The model may write a placeholder as given
/// (`⟦EMAIL_1⟧`) or without its brackets (`EMAIL_1`), and it can arrive split at any point between two deltas, so text
/// that could still become a placeholder is held back: from a `⟦` until its `⟧` or until 24 characters are in hand, and
/// for the bare form only a tail that is still a prefix of a known token. A placeholder with no entry passes through
/// unchanged; a value is never invented. Holds real values: never log or record it.
pub struct ReplyFilter {
    map: Vec<(String, String)>,
    held: String,
    /// The last character already consumed, for the word boundary in front of a bare token.
    prev: char,
    restored: u64,
    /// The canary markers to strip (`canary`), and whether each came back. Empty when no canary was planted.
    markers: Vec<String>,
    echoed: [bool; 2],
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl ReplyFilter {
    pub fn new(reverse: &[(String, String)]) -> Self {
        Self { map: reverse.to_vec(), held: String::new(), prev: ' ', restored: 0, markers: Vec::new(), echoed: [false; 2] }
    }

    /// Also strips these two canary markers from the reply, holding back a tail that could still become one.
    pub fn with_markers(mut self, markers: &[String; 2]) -> Self {
        self.markers = markers.to_vec();
        self
    }

    /// Whether marker A and marker B came back in the reply so far.
    pub fn echoed(&self) -> [bool; 2] {
        self.echoed
    }

    /// How many placeholders were put back so far.
    pub fn restored(&self) -> u64 {
        self.restored
    }

    /// The text of this delta that can go out now.
    pub fn push(&mut self, delta: &str) -> String {
        self.held.push_str(delta);
        self.drain(false)
    }

    /// Everything still held, substituted if it is a whole placeholder. Call it when the stream ends, however it ends.
    pub fn finish(&mut self) -> String {
        let out = self.drain(true);
        self.prev = ' ';
        out
    }

    fn drain(&mut self, end: bool) -> String {
        let buf = std::mem::take(&mut self.held);
        let (mut out, mut i) = (String::new(), 0);
        'scan: while i < buf.len() {
            let rest = &buf[i..];
            let c = rest.chars().next().unwrap_or(' ');
            for (k, m) in self.markers.iter().enumerate() {
                if rest.starts_with(m.as_str()) {
                    self.echoed[k] = true;
                    (i, self.prev) = (i + m.len(), ' ');
                    continue 'scan;
                }
                if !end && m.starts_with(rest) {
                    break 'scan;
                }
                // A reply cut inside a marker: drop the part rather than deliver it. Short tails are ordinary text.
                if end && rest.len() >= 4 && !is_word(self.prev) && m.starts_with(rest) {
                    i = buf.len();
                    break 'scan;
                }
            }
            if c == '\u{27e6}' {
                if let Some((j, close)) = rest.char_indices().take(24).find(|(_, c)| *c == '\u{27e7}') {
                    let tok = &rest[..j + close.len_utf8()];
                    match self.map.iter().find(|(p, _)| p == tok) {
                        Some((_, orig)) => {
                            out.push_str(orig);
                            self.restored += 1;
                        }
                        None => out.push_str(tok),
                    }
                    (i, self.prev) = (i + tok.len(), '\u{27e7}');
                    continue;
                }
                if !end && rest.chars().nth(23).is_none() {
                    break;
                }
            } else if !is_word(self.prev) {
                for (p, orig) in &self.map {
                    let tok = p.trim_matches(['\u{27e6}', '\u{27e7}']);
                    if let Some(after) = rest.strip_prefix(tok) {
                        match after.chars().next() {
                            Some(n) if is_word(n) => {}
                            None if !end => break 'scan,
                            _ => {
                                out.push_str(orig);
                                self.restored += 1;
                                (i, self.prev) = (i + tok.len(), '\u{27e7}');
                                continue 'scan;
                            }
                        }
                    } else if !end && tok.starts_with(rest) {
                        break 'scan;
                    }
                }
            }
            out.push(c);
            (i, self.prev) = (i + c.len_utf8(), c);
        }
        self.held = buf[i..].to_string();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::pii::API_KEY;

    fn on() -> PiiConfig {
        PiiConfig { enabled: true, ..Default::default() }
    }

    fn step_attrs<'a>(cfg: &PiiConfig, texts: impl IntoIterator<Item = &'a str>) -> Option<serde_json::Value> {
        let msgs: Vec<ChatMessage> = texts.into_iter().map(|t| ChatMessage::new("user", t)).collect();
        redact(cfg, "", &msgs).step
    }

    #[test]
    fn off_means_nothing() {
        assert!(step_attrs(&PiiConfig::default(), ["a@b.co"]).is_none());
    }

    #[test]
    fn step_attrs_are_counts_actions_and_mode_only() {
        let a = step_attrs(&on(), ["a@b.co and c@d.co", "ip 1.2.3.4 key sk-ant-api03-AbCdEfGhIjKlMnOpQr"]).unwrap();
        assert_eq!(
            a,
            serde_json::json!({ "pii": {
                "counts": { "EMAIL": 2, "IP": 1, "API_KEY": 1 },
                "actions": { "EMAIL": "pseudonymize", "IP": "allow", "API_KEY": "block" },
                "mode": "detect_only" } })
        );
        assert!(step_attrs(&on(), ["nothing here"]).is_none());
    }

    fn enforce() -> PiiConfig {
        PiiConfig { enabled: true, mode: PiiMode::Enforce, ..Default::default() }
    }

    fn run(cfg: &PiiConfig, system: &str, msgs: &[&str]) -> Redacted {
        let m: Vec<ChatMessage> = msgs.iter().map(|t| ChatMessage::new("user", t)).collect();
        redact(cfg, system, &m)
    }

    #[test]
    fn same_value_same_placeholder_and_types_number_apart() {
        let r =
            run(&enforce(), "ask a@b.co", &["a@b.co, A@B.CO, c@d.co and +1 555 123 4567 / +1-555-123-4567", "c@d.co again, 555-123-4567"]);
        assert!(r.system.starts_with("ask \u{27e6}EMAIL_1\u{27e7}\n\n"), "{}", r.system);
        assert!(r.system.ends_with(PLACEHOLDER_NOTE));
        assert_eq!(
            r.messages[0].content,
            "\u{27e6}EMAIL_1\u{27e7}, \u{27e6}EMAIL_1\u{27e7}, \u{27e6}EMAIL_2\u{27e7} and \u{27e6}PHONE_1\u{27e7} / \u{27e6}PHONE_1\u{27e7}"
        );
        // A phone without its +1 is another digit string: it gets its own number.
        assert_eq!(r.messages[1].content, "\u{27e6}EMAIL_2\u{27e7} again, \u{27e6}PHONE_2\u{27e7}");
        assert_eq!(r.step.unwrap()["pii"]["replaced"], 8);
    }

    #[test]
    fn numbering_is_stable_across_turns_and_requests() {
        let a = run(&enforce(), "", &["x@y.co and z@y.co"]);
        let b = run(&enforce(), "", &["x@y.co and z@y.co"]);
        assert_eq!((&a.messages, &a.reverse, &a.system), (&b.messages, &b.reverse, &b.system));
        let c = run(&enforce(), "", &["x@y.co and z@y.co", "w@y.co then x@y.co"]);
        assert_eq!(c.messages[0], a.messages[0]);
        assert_eq!(c.messages[1].content, "\u{27e6}EMAIL_3\u{27e7} then \u{27e6}EMAIL_1\u{27e7}");
    }

    #[test]
    fn allow_block_and_off_leave_the_text() {
        let key = "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv";
        let t = format!("ip 10.1.2.3 key {key}\n```\nk={key} a@b.co\n```");
        let r = run(&enforce(), "", &[&t]);
        // The key is Block (left for the gate) and the email is inside a fence: nothing is replaced.
        assert_eq!((r.messages[0].content.as_str(), r.system.as_str(), r.reverse.len()), (t.as_str(), "", 0));
        let mut allow = enforce();
        allow.allow_terms = vec!["a@b.co".into()];
        assert_eq!(run(&allow, "s", &[&t]).messages[0].content, t);
        let mut pseudo = enforce();
        pseudo.entities.insert(API_KEY.into(), PiiAction::Pseudonymize);
        let r = run(&pseudo, "", &[&t]);
        assert!(!r.messages[0].content.contains("AbCdEf") && r.messages[0].content.contains("\u{27e6}API_KEY_1\u{27e7}"));
        for cfg in [PiiConfig { enabled: true, ..Default::default() }, PiiConfig { mode: PiiMode::Enforce, ..Default::default() }] {
            let r = run(&cfg, "s", &[&t, "a@b.co"]);
            assert_eq!((r.system.as_str(), r.messages[0].content.as_str(), r.messages[1].content.as_str()), ("s", t.as_str(), "a@b.co"));
            assert!(r.reverse.is_empty());
        }
    }

    #[test]
    fn deny_terms_become_custom_placeholders() {
        let mut cfg = enforce();
        cfg.deny_terms = vec!["Project Falcon".into()];
        let r = run(&cfg, "", &["about project falcon and PROJECT FALCON"]);
        assert_eq!(r.messages[0].content, "about \u{27e6}CUSTOM_1\u{27e7} and \u{27e6}CUSTOM_1\u{27e7}");
    }

    #[test]
    fn the_reverse_map_restores_exactly() {
        let t = "see https://x.example/u?e=a@b.co&f=a@b.co, call 555-123-4567, mail c@d.co";
        let r = run(&enforce(), "", &[t]);
        assert!(!r.messages[0].content.contains("@") && !r.messages[0].content.contains("555-123"));
        let mut back = r.messages[0].content.clone();
        for (ph, orig) in &r.reverse {
            back = back.replace(ph, orig);
        }
        assert_eq!(back, t);
    }
    fn fixture() -> Vec<(String, String)> {
        [("EMAIL_1", "ann@x.org"), ("PHONE_1", "+44 20"), ("EMAIL_2", "bo@y.org")]
            .iter()
            .map(|(p, o)| (format!("\u{27e6}{p}\u{27e7}"), o.to_string()))
            .collect()
    }

    fn run_filter(parts: &[&str]) -> String {
        let mut f = ReplyFilter::new(&fixture());
        let mut out: String = parts.iter().map(|p| f.push(p)).collect();
        out.push_str(&f.finish());
        out
    }

    const REPLY: &str = "\u{27e6}EMAIL_1\u{27e7}\u{27e6}PHONE_1\u{27e7} hi EMAIL_2, PHONE_1. no \u{27e6}EMAIL_9\u{27e7} EMAIL_9 MY_EMAIL_1x EMAIL_1x \u{27e6}EMAIL_1\u{27e7}";
    const RESTORED: &str = "ann@x.org+44 20 hi bo@y.org, +44 20. no \u{27e6}EMAIL_9\u{27e7} EMAIL_9 MY_EMAIL_1x EMAIL_1x ann@x.org";

    #[test]
    fn the_filter_restores_whole_text_and_leaves_the_rest() {
        assert_eq!(run_filter(&[REPLY]), RESTORED);
        assert_eq!(run_filter(&["EMAIL_1"]), "ann@x.org");
        assert_eq!(run_filter(&["EMAIL_1x EMAIL_10 xEMAIL_1 EMAIL_1"]), "EMAIL_1x EMAIL_10 xEMAIL_1 ann@x.org");
        assert_eq!(run_filter(&["a", "b"]), "ab");
        let mut f = ReplyFilter::new(&fixture());
        f.push(REPLY);
        f.finish();
        assert_eq!(f.restored(), 5);
    }

    #[test]
    fn the_filter_gives_the_same_text_for_every_split() {
        let chars: Vec<usize> = REPLY.char_indices().map(|(i, _)| i).chain([REPLY.len()]).collect();
        for &at in &chars {
            assert_eq!(run_filter(&[&REPLY[..at], &REPLY[at..]]), RESTORED, "split at {at}");
        }
        // Fixed-seed multi-splits: no dependency for a generator.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..2000 {
            let mut cuts: Vec<usize> = (0..1 + seed as usize % 6)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    chars[seed as usize % chars.len()]
                })
                .collect();
            cuts.sort();
            let (mut parts, mut from) = (Vec::new(), 0);
            for c in cuts.into_iter().chain([REPLY.len()]) {
                parts.push(&REPLY[from..c]);
                from = c;
            }
            assert_eq!(run_filter(&parts), RESTORED, "{parts:?}");
        }
    }

    const MARKERS: [&str; 2] = ["1a2b3c4d", "5e6f7a8b"];

    fn run_marked(parts: &[&str]) -> (String, [bool; 2]) {
        let mut f = ReplyFilter::new(&fixture()).with_markers(&MARKERS.map(String::from));
        let mut out: String = parts.iter().map(|p| f.push(p)).collect();
        out.push_str(&f.finish());
        (out, f.echoed())
    }

    #[test]
    fn the_filter_strips_split_markers_and_still_restores_placeholders() {
        let reply = "see EMAIL_1 and \u{27e6}PHONE_1\u{27e7} \u{e9} deadbeef\n\n1a2b3c4d 5e6f7a8b";
        let want = "see ann@x.org and +44 20 \u{e9} deadbeef\n\n ";
        for at in (0..=reply.len()).filter(|i| reply.is_char_boundary(*i)) {
            let (out, echoed) = run_marked(&[&reply[..at], &reply[at..]]);
            assert_eq!((out.as_str(), echoed), (want, [true, true]), "split at {at}");
        }
        // Every piece one character long: no marker, whole or partial, is ever delivered.
        let pieces: Vec<String> = reply.chars().map(String::from).collect();
        let mut f = ReplyFilter::new(&fixture()).with_markers(&MARKERS.map(String::from));
        for p in &pieces {
            assert!(!MARKERS.iter().any(|m| f.push(p).contains(&m[..4])));
        }
        assert_eq!(run_marked(&["only 5e6f7a8b"]), ("only ".into(), [false, true]));
        assert_eq!(run_marked(&["none"]), ("none".into(), [false, false]));
    }

    #[test]
    fn a_reply_cut_inside_a_marker_does_not_deliver_the_part() {
        assert_eq!(run_marked(&["end 1a2b3c"]).0, "end ");
        assert_eq!(run_marked(&["end 1a2"]).0, "end 1a2", "a short tail is ordinary text");
        assert_eq!(run_marked(&["Banana"]).0, "Banana");
    }

    #[test]
    fn a_lone_open_bracket_is_held_for_24_characters_and_flushed_by_finish() {
        let mut f = ReplyFilter::new(&fixture());
        assert_eq!(f.push(&format!("x \u{27e6}{}", "a".repeat(22))), "x ");
        assert_eq!(f.push("a"), format!("\u{27e6}{}", "a".repeat(23)));
        let mut f = ReplyFilter::new(&fixture());
        assert_eq!(f.push("ok \u{27e6}EMA"), "ok ");
        assert_eq!(f.finish(), "\u{27e6}EMA");
        assert_eq!(f.finish(), "");
        // Held at the end of a delta, then a whole token when the stream ends.
        let mut f = ReplyFilter::new(&fixture());
        assert_eq!(f.push("see EMAIL_1"), "see ");
        assert_eq!(f.finish(), "ann@x.org");
    }
}
