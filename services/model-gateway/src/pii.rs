//! Finds personal data and secrets in text (plan PII-P1.2). Patterns with checksums, no model: names and addresses
//! are not found at all. A value found is never logged, put in an error or an attribute: only counts leave here.
//!
//! Covered, and nothing else:
//! - `EMAIL`: ASCII local part and domain with a dot and a TLD of 2 or more letters.
//! - `PHONE`: `+` and 8 to 15 digits with separators; US style `(555) 123-4567`, `555-123-4567`, `555 123 4567`,
//!   `555.123.4567`, optionally after `1-`. Bare digit runs are not phones. Other local formats are later work.
//! - `CARD`: 13 to 19 digits (single space or dash between digits), first digit 2 to 6, Luhn valid, not all one digit.
//! - `IBAN`: 2 letters, 2 digits, 11 to 30 alphanumerics, optional spaces, mod-97 equal to 1.
//! - `NATIONAL_ID`: US SSN `123-45-6789` with its structural exclusions, and UK NINO `AB 12 34 56 C`. Other
//!   countries' IDs are later work.
//! - `IP`: IPv4 (four octets, not part of a longer dotted number) and IPv6 (full and `::` forms, with a digit in it so
//!   that `Add::add` is not one).
//! - `API_KEY`: `sk-ant-`, `sk-proj-`, `sk-` (with a digit), `AKIA`/`ASIA` + 16, `AIza` + 35, `xox[abposr]-`,
//!   `sk_live_`/`rk_live_`, `SG.a.b`, `glpat-`, `npm_` + 36.
//! - `GITHUB_TOKEN`: `ghp_ gho_ ghu_ ghs_ ghr_` and `github_pat_`.
//! - `JWT`: three base64url segments starting `eyJ`.
//! - `PRIVATE_KEY`: a PEM `PRIVATE KEY` block, or with no end marker the header and its base64 lines (4 KiB at most).
//! - `CUSTOM`: the admin's `deny_terms`, plain ASCII-case-insensitive substrings.
//!
//! Inside a fenced code block (``` or ~~~) only secrets (and `CUSTOM` terms, which the admin chose) are kept: hashes,
//! UUIDs and identifiers are the false positives there. `allow_terms` drop a span whose text equals one.

use askbar_proto::gateway::ChatMessage;
use askbar_proto::pii::{
    PiiAction, PiiConfig, PiiMode, API_KEY, CARD, CUSTOM, EMAIL, GITHUB_TOKEN, IBAN, IP, JWT, NATIONAL_ID, PHONE, PRIVATE_KEY,
};
use regex::Regex;
use std::net::Ipv6Addr;
use std::sync::OnceLock;

/// A value found: its type and byte range (on char boundaries) in the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub ty: &'static str,
    pub start: usize,
    pub end: usize,
}

const SECRETS: [&str; 4] = [API_KEY, GITHUB_TOKEN, JWT, PRIVATE_KEY];
const PEM_BODY_MAX: usize = 4096;

struct Patterns {
    email: Regex,
    phone_intl: Regex,
    phone_us: Regex,
    card: Regex,
    iban: Regex,
    ssn: Regex,
    nino: Regex,
    ipv4: Regex,
    ipv6: Regex,
    api_key: Regex,
    github: Regex,
    jwt: Regex,
    pem: Regex,
    pem_end: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        // `(?-u)`: ASCII classes, so the crate needs none of its Unicode tables.
        let r = |p: &str| Regex::new(&format!("(?-u){p}")).expect("a fixed pattern compiles");
        Patterns {
            email: r(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}"),
            phone_intl: r(r"\+\d(?:[ .()-]{0,2}\d){7,14}"),
            phone_us: r(r"(?:1[ .-])?(?:\(\d{3}\)[ .-]?|\d{3}[ .-])\d{3}[ .-]\d{4}"),
            card: r(r"\d(?:[ -]?\d){12,18}"),
            iban: r(r"[A-Z]{2}\d{2}(?: ?[A-Z0-9]){11,30}"),
            ssn: r(r"\d{3}-\d{2}-\d{4}"),
            nino: r(r"[A-CEGHJ-PR-TW-Z][A-CEGHJ-NPR-TW-Z] ?\d{2} ?\d{2} ?\d{2} ?[A-D]"),
            ipv4: r(r"\d{1,3}(?:\.\d{1,3}){3}"),
            ipv6: r(r"(?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}"),
            api_key: r(concat!(
                r"sk-ant-[A-Za-z0-9_-]{13,}|sk-[A-Za-z0-9_-]{20,}|(?:AKIA|ASIA)[A-Z0-9]{16}|AIza[A-Za-z0-9_-]{35}",
                r"|xox[abposr]-[A-Za-z0-9-]{10,}|[sr]k_live_[A-Za-z0-9]{16,}|SG\.[A-Za-z0-9_-]{16,}\.[A-Za-z0-9_-]{16,}",
                r"|glpat-[A-Za-z0-9_-]{20,}|npm_[A-Za-z0-9]{36}"
            )),
            github: r(r"gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}"),
            jwt: r(r"eyJ[A-Za-z0-9_-]{5,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}"),
            pem: r(r"-----BEGIN [A-Z ]*PRIVATE KEY-----"),
            pem_end: r(r"-----END [A-Z ]*PRIVATE KEY-----"),
        }
    })
}

fn before(t: &str, i: usize) -> Option<char> {
    t[..i].chars().next_back()
}

fn after(t: &str, i: usize) -> Option<char> {
    t[i..].chars().next()
}

/// A number or code that stands alone: no letter or digit against it, and not a piece of a longer dotted or dashed
/// number (`1.2.3.4.5`, `2026-11-26-3`).
fn alone(t: &str, s: usize, e: usize) -> bool {
    let (p, n) = (before(t, s), after(t, e));
    if p.is_some_and(|c| c.is_alphanumeric() || c == '_') || n.is_some_and(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    let glued = |c: Option<char>, beyond: Option<char>| matches!(c, Some('-' | '.')) && beyond.is_some_and(|d| d.is_ascii_digit());
    !(glued(p, t[..s].chars().rev().nth(1)) || glued(n, t[e..].chars().nth(1)))
}

/// A token not part of a longer one: no token character (letter, digit, `_` or `-`) against either end.
fn token_alone(t: &str, s: usize, e: usize) -> bool {
    let tok = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    !before(t, s).is_some_and(tok) && !after(t, e).is_some_and(tok)
}

fn luhn(d: &[u8]) -> bool {
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            let x = u32::from(x);
            if !i.is_multiple_of(2) {
                let x = x * 2;
                x - if x > 9 { 9 } else { 0 }
            } else {
                x
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn mod97(s: &[u8]) -> u32 {
    // The first four characters go to the end; letters count 10 to 35.
    s[4..].iter().chain(&s[..4]).fold(0u32, |r, &c| {
        let v = if c.is_ascii_digit() { u32::from(c - b'0') } else { u32::from(c - b'A') + 10 };
        if v > 9 {
            (r * 100 + v) % 97
        } else {
            (r * 10 + v) % 97
        }
    })
}

fn all_same(b: &[u8]) -> bool {
    b.windows(2).all(|w| w[0] == w[1])
}

fn find_all(re: &Regex, t: &str, ty: &'static str, ok: impl Fn(usize, usize) -> bool, out: &mut Vec<Span>) {
    out.extend(re.find_iter(t).filter(|m| ok(m.start(), m.end())).map(|m| Span { ty, start: m.start(), end: m.end() }));
}

fn ssn_ok(m: &str) -> bool {
    let (area, group, serial) = (&m[..3], &m[4..6], &m[7..]);
    !(area == "000" || area == "666" || area.starts_with('9') || group == "00" || serial == "0000")
}

fn nino_ok(m: &str) -> bool {
    let c: String = m.chars().filter(|c| !c.is_ascii_whitespace()).take(2).collect();
    !["BG", "GB", "NK", "KN", "TN", "NT", "ZZ"].contains(&c.as_str())
}

/// Every candidate span, before the fence, term and overlap rules.
fn candidates(t: &str) -> Vec<Span> {
    let p = patterns();
    let mut out = Vec::new();
    if t.contains('@') {
        find_all(&p.email, t, EMAIL, |_, _| true, &mut out);
    }
    if t.contains('+') {
        find_all(&p.phone_intl, t, PHONE, |s, e| alone(t, s, e), &mut out);
    }
    find_all(&p.phone_us, t, PHONE, |s, e| alone(t, s, e), &mut out);
    for m in p.card.find_iter(t).filter(|m| alone(t, m.start(), m.end())) {
        let d: Vec<u8> = m.as_str().bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
        if (13..=19).contains(&d.len()) && (2..=6).contains(&d[0]) && !all_same(&d) && luhn(&d) {
            out.push(Span { ty: CARD, start: m.start(), end: m.end() });
        }
    }
    for m in p.iban.find_iter(t).filter(|m| !before(t, m.start()).is_some_and(|c| c.is_alphanumeric() || c == '_')) {
        // Longest first: a word after the IBAN (`... 00 OK`) may have been swallowed by the match.
        let ends: Vec<usize> = m.as_str().char_indices().filter(|(_, c)| *c != ' ').map(|(i, _)| m.start() + i + 1).collect();
        let chars: Vec<u8> = m.as_str().bytes().filter(|b| *b != b' ').collect();
        for n in (15..=chars.len()).rev() {
            if mod97(&chars[..n]) == 1 && !all_same(&chars[4..n]) && alone(t, m.start(), ends[n - 1]) {
                out.push(Span { ty: IBAN, start: m.start(), end: ends[n - 1] });
                break;
            }
        }
    }
    find_all(&p.ssn, t, NATIONAL_ID, |s, e| alone(t, s, e) && ssn_ok(&t[s..e]), &mut out);
    find_all(&p.nino, t, NATIONAL_ID, |s, e| alone(t, s, e) && nino_ok(&t[s..e]), &mut out);
    find_all(&p.ipv4, t, IP, |s, e| alone(t, s, e) && t[s..e].split('.').all(|o| o.parse::<u8>().is_ok()), &mut out);
    if t.contains(':') {
        let v6 = |s: usize, e: usize| {
            let m = &t[s..e];
            alone(t, s, e)
                && m.bytes().any(|b| b.is_ascii_digit())
                && !after(t, e).is_some_and(|c| c == ':')
                && m.parse::<Ipv6Addr>().is_ok()
        };
        find_all(&p.ipv6, t, IP, v6, &mut out);
    }
    let key = |s: usize, e: usize| {
        let m = &t[s..e];
        token_alone(t, s, e)
            && (!m.starts_with("sk-") || m.starts_with("sk-ant-") || m.starts_with("sk-proj-") || m.bytes().any(|b| b.is_ascii_digit()))
    };
    find_all(&p.api_key, t, API_KEY, key, &mut out);
    find_all(&p.github, t, GITHUB_TOKEN, |s, e| token_alone(t, s, e), &mut out);
    find_all(&p.jwt, t, JWT, |s, e| token_alone(t, s, e), &mut out);
    for h in p.pem.find_iter(t) {
        let rest = &t[h.end()..];
        let end = match p.pem_end.find(rest) {
            Some(m) => h.end() + m.end(),
            None => {
                // No end marker: the base64 lines after the header, 4 KiB at most.
                let (mut end, mut pos) = (h.end(), rest.find('\n').map_or(rest.len(), |i| i + 1));
                while pos < rest.len() {
                    let line = rest[pos..].split('\n').next().unwrap_or("");
                    let body = line.trim_end();
                    if body.is_empty()
                        || pos + line.len() > PEM_BODY_MAX
                        || !body.bytes().all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
                    {
                        break;
                    }
                    end = h.end() + pos + body.len();
                    pos += line.len() + 1;
                }
                end
            }
        };
        out.push(Span { ty: PRIVATE_KEY, start: h.start(), end });
    }
    out
}

/// Byte ranges of fenced code blocks: from the opening fence line to the closing one, or to the end of the text.
fn fences(t: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut open: Option<(usize, char, usize)> = None;
    let mut pos = 0;
    for line in t.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let c = trimmed.chars().next().filter(|c| *c == '`' || *c == '~');
        let run = c.map_or(0, |c| trimmed.chars().take_while(|x| *x == c).count());
        match (open, c) {
            (None, Some(c)) if run >= 3 => open = Some((pos, c, run)),
            (Some((s, oc, on)), Some(c)) if c == oc && run >= on && trimmed[run..].trim().is_empty() => {
                out.push((s, pos + line.len()));
                open = None;
            }
            _ => {}
        }
        pos += line.len();
    }
    out.extend(open.map(|(s, _, _)| (s, t.len())));
    out
}

/// ASCII-case-insensitive substring search on char boundaries.
fn find_term(t: &str, term: &str, out: &mut Vec<Span>) {
    let (h, n) = (t.as_bytes(), term.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return;
    }
    for i in 0..=h.len() - n.len() {
        if h[i].eq_ignore_ascii_case(&n[0])
            && h[i..i + n.len()].eq_ignore_ascii_case(n)
            && t.is_char_boundary(i)
            && t.is_char_boundary(i + n.len())
        {
            out.push(Span { ty: CUSTOM, start: i, end: i + n.len() });
        }
    }
}

fn rank(s: &Span) -> u8 {
    match s.ty {
        CARD | IBAN => 2,
        ty if SECRETS.contains(&ty) => 1,
        _ => 0,
    }
}

/// Every span found, whatever its action (counting needs the allowed ones too), with none overlapping. Empty when
/// detection is off.
pub fn detect(text: &str, cfg: &PiiConfig) -> Vec<Span> {
    if !cfg.enabled {
        return vec![];
    }
    let mut spans = candidates(text);
    let fenced = fences(text);
    spans.retain(|s| !fenced.iter().any(|&(a, b)| (a..b).contains(&s.start)) || SECRETS.contains(&s.ty));
    for term in &cfg.deny_terms {
        find_term(text, term, &mut spans);
    }
    let allow: Vec<String> = cfg.allow_terms.iter().map(|t| t.to_lowercase()).collect();
    spans.retain(|s| !allow.contains(&text[s.start..s.end].to_lowercase()));
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<Span> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(last) if s.start < last.end => {
                // A checksum type first, then a secret, then the longer one.
                if (rank(&s), s.end - s.start) > (rank(last), last.end - last.start) {
                    *last = s;
                }
            }
            _ => out.push(s),
        }
    }
    out
}

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
    let mut one = |text: &str| -> String {
        let spans = detect(text, cfg);
        let (mut out, mut at) = (String::new(), 0);
        for s in &spans {
            *counts.entry(s.ty).or_default() += 1;
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
    Redacted { system, messages, reverse, step }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on() -> PiiConfig {
        PiiConfig { enabled: true, ..Default::default() }
    }

    /// (type, text) of each span.
    fn found(t: &str) -> Vec<(&'static str, String)> {
        detect(t, &on()).into_iter().map(|s| (s.ty, t[s.start..s.end].to_string())).collect()
    }

    fn step_attrs<'a>(cfg: &PiiConfig, texts: impl IntoIterator<Item = &'a str>) -> Option<serde_json::Value> {
        let msgs: Vec<ChatMessage> = texts.into_iter().map(|t| ChatMessage::new("user", t)).collect();
        redact(cfg, "", &msgs).step
    }

    fn only(t: &str, ty: &'static str) -> bool {
        found(t).iter().map(|(a, _)| *a).collect::<Vec<_>>() == vec![ty]
    }

    #[test]
    fn positives() {
        let cases: &[(&str, &str, &str)] = &[
            (EMAIL, "write to jane.doe+tag@mail.example.co.uk now", "jane.doe+tag@mail.example.co.uk"),
            (EMAIL, "<a_b-c@x-y.org>", "a_b-c@x-y.org"),
            (PHONE, "call +44 20 7946 0958 today", "+44 20 7946 0958"),
            (PHONE, "tel: +1 (555) 123-4567.", "+1 (555) 123-4567"),
            (PHONE, "tel +4915123456789", "+4915123456789"),
            (PHONE, "at (555) 123-4567 please", "(555) 123-4567"),
            (PHONE, "at 555-123-4567", "555-123-4567"),
            (PHONE, "at 555 123 4567", "555 123 4567"),
            (PHONE, "at 1-800-555-0199", "1-800-555-0199"),
            (CARD, "card 4111 1111 1111 1111 ok", "4111 1111 1111 1111"),
            (CARD, "card 4111-1111-1111-1111", "4111-1111-1111-1111"),
            (CARD, "card 5500000000000004!", "5500000000000004"),
            (CARD, "amex 378282246310005", "378282246310005"),
            (IBAN, "iban DE89 3704 0044 0532 0130 00 OK", "DE89 3704 0044 0532 0130 00"),
            (IBAN, "GB82WEST12345698765432", "GB82WEST12345698765432"),
            (IBAN, "FR14 2004 1010 0505 0001 3M02 606", "FR14 2004 1010 0505 0001 3M02 606"),
            (NATIONAL_ID, "ssn 123-45-6789.", "123-45-6789"),
            (NATIONAL_ID, "nino AB 12 34 56 C", "AB 12 34 56 C"),
            (NATIONAL_ID, "nino JG103759A", "JG103759A"),
            (IP, "host 192.168.1.10:8080", "192.168.1.10"),
            (IP, "ip 1.2.3.4.", "1.2.3.4"),
            (IP, "v6 2001:db8:85a3:0:0:8a2e:370:7334 up", "2001:db8:85a3:0:0:8a2e:370:7334"),
            (IP, "v6 2001:db8::ff00:42:8329 up", "2001:db8::ff00:42:8329"),
            (IP, "loopback ::1 here", "::1"),
            (API_KEY, "key sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv", "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUv"),
            (API_KEY, "key=sk-proj-AbCdEfGhIjKlMnOpQrSt_12", "sk-proj-AbCdEfGhIjKlMnOpQrSt_12"),
            (API_KEY, "key sk-AbCdEfGhIjKlMnOpQrSt12", "sk-AbCdEfGhIjKlMnOpQrSt12"),
            (API_KEY, "AKIAIOSFODNN7EXAMPLE", "AKIAIOSFODNN7EXAMPLE"),
            (API_KEY, "AIzaSyA1234567890abcdefghijklmnopqrstuv", "AIzaSyA1234567890abcdefghijklmnopqrstuv"),
            (API_KEY, "xoxb-1234567890-abcdefghij", "xoxb-1234567890-abcdefghij"),
            (API_KEY, "sk_live_abcdefghijklmnop1234", "sk_live_abcdefghijklmnop1234"),
            (API_KEY, "SG.abcdefghijklmnop1234.abcdefghijklmnop1234", "SG.abcdefghijklmnop1234.abcdefghijklmnop1234"),
            (API_KEY, "glpat-abcdefghij1234567890", "glpat-abcdefghij1234567890"),
            (API_KEY, "npm_abcdefghijklmnopqrstuvwxyz0123456789", "npm_abcdefghijklmnopqrstuvwxyz0123456789"),
            (GITHUB_TOKEN, "ghp_abcdefghijklmnopqrstuvwxyz0123456789", "ghp_abcdefghijklmnopqrstuvwxyz0123456789"),
            (GITHUB_TOKEN, "github_pat_11ABCDEFG0abcdefghijkl_abcdefghijklmnop", "github_pat_11ABCDEFG0abcdefghijkl_abcdefghijklmnop"),
            (
                JWT,
                "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghij-_KLMN",
                "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghij-_KLMN",
            ),
        ];
        for (ty, text, want) in cases {
            assert_eq!(found(text), vec![(*ty, want.to_string())], "{text}");
        }
    }

    #[test]
    fn private_key_with_and_without_an_end_marker() {
        let t = "k:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\nIBAAKC\n-----END RSA PRIVATE KEY-----\nafter";
        assert!(found(t)[0].1.ends_with("-----END RSA PRIVATE KEY-----"));
        assert_eq!(found(t).len(), 1);
        let cut = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkq\nhkiG9w0BAQEFAASC\n\nnot base64 here";
        let f = found(cut);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].1, "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkq\nhkiG9w0BAQEFAASC");
        let long = format!("-----BEGIN PRIVATE KEY-----\n{}", "QUJDREVGR0hJSktMTU5PUFFSU1RVVldY\n".repeat(400));
        let f = detect(&long, &on());
        assert!(f.len() == 1 && f[0].end - 27 <= PEM_BODY_MAX, "{f:?}");
    }

    #[test]
    fn false_positives() {
        let none = [
            "550e8400-e29b-41d4-a716-446655440000",
            "da39a3ee5e6b4b0d3255bfef95601890afd80709",
            "commit 3f2a9c1 fixed it",
            "version 1.2.3 and 10.0.0.1.2 and 1.2.3.4.5",
            "v2.0.0-beta.1+build.5 and 1.0.0-rc.1",
            "time 2026-11-26T10:20:30Z on 2026-11-26 at 10:20:30",
            "epoch 1700000000 and 1700000000000",
            "order 123456789012 and 20261126123456 and 9876543210",
            "see /usr/local/lib/python3.11/site-packages/foo.py:123",
            "C:\\Users\\bob\\file-123-456.txt",
            "SGVsbG8gd29ybGQhIFRoaXMgaXMgYSBiYXNlNjQgYmxvYg== and aGVsbG8=",
            "colour #ff00aa and #123456 and 0xdeadbeef",
            "Will you call Rose and Mark?",
            "use std::vec::Vec and Add::add and Cell::new",
            "12:34:56 and aa:bb:cc:dd:ee:ff",
            "task-force-assignment-document-number and sk-learn",
            "the email is user@host and foo@1.2.3 and @decorator",
            "MSG.abcdefghijklmnop1234.abcdefghijklmnop1234",
            "eyJhbGciOiJIUzI1NiJ9 alone and a.b.c",
            "IBAN-like AB12 CDEF GHIJ KLMN OP and GB00 0000 0000 0000 0000 00",
            "ssn 000-12-3456 666-12-3456 900-12-3456 123-00-4567 123-45-0000",
            "nino BG 12 34 56 A DF 12 34 56 A AB 12 34 56 E",
            "ip 256.1.1.1 and 1.2.3 and 999.999.999.999",
        ];
        for t in none {
            assert_eq!(found(t), vec![], "{t}");
        }
    }

    #[test]
    fn card_and_iban_need_their_checksum_and_a_clean_edge() {
        assert!(found("4111 1111 1111 1112").is_empty(), "fails Luhn");
        assert!(found("4242424242424243").is_empty());
        assert!(found("0000000000000000 and 4444444444444444").is_empty(), "same digit");
        assert!(found("x 44111111111111119999 y").is_empty(), "inside a longer run");
        assert!(found("DE89 3704 0044 0532 0130 01").is_empty(), "fails mod-97");
        assert!(only("4111111111111111", CARD));
    }

    #[test]
    fn a_card_is_not_also_a_phone_and_overlaps_leave_one_span() {
        let t = "4111 1111 1111 1111 and 555 123 4567";
        let f = found(t);
        assert_eq!(f, vec![(CARD, "4111 1111 1111 1111".into()), (PHONE, "555 123 4567".into())]);
        // A key that holds a run that would be a phone: the secret stays whole.
        let k = "sk-ant-api03-555-123-4567-abcdefgh";
        assert_eq!(found(k), vec![(API_KEY, k.into())]);
        let spans = detect("a@b.co 192.168.0.1 +1 555 123 4567 4111111111111111", &on());
        assert!(spans.windows(2).all(|w| w[0].end <= w[1].start));
    }

    #[test]
    fn only_secrets_are_kept_inside_fenced_code() {
        let t = "mail a@b.co\n```\nmail c@d.co ip 10.0.0.1\nkey sk-ant-api03-AbCdEfGhIjKlMnOpQr\n```\nback e@f.co";
        let f = found(t);
        assert_eq!(f.iter().map(|(a, _)| *a).collect::<Vec<_>>(), vec![EMAIL, API_KEY, EMAIL]);
        let tilde = "~~~\nx@y.co\n~~~\nz@w.co";
        assert_eq!(found(tilde), vec![(EMAIL, "z@w.co".into())]);
        let open = "```\nx@y.co and ghp_abcdefghijklmnopqrstuvwxyz0123456789";
        assert_eq!(found(open).iter().map(|(a, _)| *a).collect::<Vec<_>>(), vec![GITHUB_TOKEN], "an open fence runs to the end");
    }

    #[test]
    fn terms() {
        let cfg = PiiConfig {
            enabled: true,
            deny_terms: vec!["Project.X (secret)".into(), "ünï".into()],
            allow_terms: vec!["Help@Example.com".into()],
            ..Default::default()
        };
        let t = "ask help@example.com and a@b.co about PROJECT.x (SECRET) and project-x, ünï";
        let f: Vec<_> = detect(t, &cfg).iter().map(|s| (s.ty, &t[s.start..s.end])).collect();
        assert_eq!(f, vec![(EMAIL, "a@b.co"), (CUSTOM, "PROJECT.x (SECRET)"), (CUSTOM, "ünï")]);
    }

    #[test]
    fn off_means_nothing() {
        assert!(detect("a@b.co sk-ant-api03-AbCdEfGhIjKlMnOpQr", &PiiConfig::default()).is_empty());
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

    #[test]
    fn benign_corpus_has_no_findings_and_big_text_is_fast() {
        let block = "Notes for the team: we shipped v1.4.2 on 2026-11-26 (build 20261126.3). Will you call Rose and Mark?\n\
            The order number is 123456789012 and the ticket is ABC-1234; see https://example.com/docs/page?id=42#top.\n\
            Config lives in /etc/askbar/config.toml and src/lib.rs:128. Colour #1a2b3c, uuid 550e8400-e29b-41d4-a716-446655440000,\n\
            commit 3f2a9c1 (full da39a3ee5e6b4b0d3255bfef95601890afd80709), epoch 1700000000, 1700000000000, ratio 3.14159.\n\
            ```rust\nfn main() { let v: Vec<u8> = std::vec::Vec::new(); println!(\"{:?}\", v); // 10.0.0.1.2\n}\n```\n\
            Blob: SGVsbG8gd29ybGQhIFRoaXMgaXMgYSBiYXNlNjQgYmxvYg== and the time 12:34:56 -- version 1.2.3-beta.1.\n";
        assert!(found(block).is_empty(), "{:?}", found(block));
        let big = block.repeat(100_000 / block.len() + 1);
        let t = std::time::Instant::now();
        let spans = detect(&big, &on());
        let ms = t.elapsed().as_millis();
        assert!(spans.is_empty() && big.len() >= 100_000);
        assert!(ms < 100, "{ms} ms for {} bytes", big.len());
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
}
