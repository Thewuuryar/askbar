//! The verdict on a request (ADR 0056). Pure: no I/O, no clock, no panic on any input.
//!
//! The proxy runs these in a fixed order: `askbar_proto::egress::parse_authority` on what the request names, [`decide`]
//! against the source's entries, then, for a name, [`check_addrs`] on every address the one resolution returned.

use askbar_proto::egress::{classify, normalize_host, Class, Entry, Host, Reason, SourceRules};
use std::net::IpAddr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict<'a> {
    Allow(&'a Entry),
    Deny(Reason),
}

/// May this source reach `host:port`? `plain` is an absolute-form `http://` request rather than a `CONNECT`. An IP literal
/// needs an entry naming that same address, and is judged here; a name's addresses are judged by [`check_addrs`].
/// An `any_public` entry matches every name (never an IP literal); an entry naming the host exactly is tried first.
pub fn decide<'a>(rules: &'a SourceRules, host: &Host, port: u16, plain: bool) -> Verdict<'a> {
    let is_name = matches!(host, Host::Name(_));
    let mut named: Vec<&Entry> = rules
        .entries
        .iter()
        .filter(|e| (e.any_public && is_name) || (!e.any_public && normalize_host(&e.host).as_ref() == Ok(host)))
        .collect();
    named.sort_by_key(|e| e.any_public);
    if named.is_empty() {
        return Verdict::Deny(if is_name { Reason::NotListed } else { Reason::IpLiteral });
    }
    let Some(entry) = named.into_iter().find(|e| e.port == port) else {
        return Verdict::Deny(Reason::Port);
    };
    if plain && !entry.plain_http {
        return Verdict::Deny(Reason::PlainHttp);
    }
    match host {
        Host::Ip(ip) if !entry.allow_private && classify(*ip) != Class::Public => Verdict::Deny(Reason::PrivateAddress),
        _ => Verdict::Allow(entry),
    }
}

/// The rebinding defence: every address a name resolved to must be public, unless the entry has `allow_private`
/// (which `any_public` never honours). An empty answer is refused too.
pub fn check_addrs(entry: &Entry, addrs: &[IpAddr]) -> Result<(), Reason> {
    let lax = entry.allow_private && !entry.any_public;
    if addrs.is_empty() || (!lax && addrs.iter().any(|a| classify(*a) != Class::Public)) {
        return Err(Reason::PrivateAddress);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use askbar_proto::egress::parse_authority;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn entry(host: &str, port: u16) -> Entry {
        Entry { host: host.into(), port, plain_http: false, allow_private: false, relay: false, any_public: false }
    }
    fn rules(entries: Vec<Entry>) -> SourceRules {
        SourceRules { entries }
    }
    fn allowed(r: &SourceRules, authority: &str, plain: bool) -> Result<bool, Reason> {
        let (h, p) = parse_authority(authority, 443)?;
        Ok(matches!(decide(r, &h, p, plain), Verdict::Allow(_)))
    }
    fn denied(r: &SourceRules, authority: &str, plain: bool) -> Reason {
        let (h, p) = parse_authority(authority, 443).unwrap();
        match decide(r, &h, p, plain) {
            Verdict::Deny(why) => why,
            Verdict::Allow(_) => panic!("{authority} was allowed"),
        }
    }

    #[test]
    fn decide_table() {
        let mut plain = entry("ntfy.lan", 80);
        plain.plain_http = true;
        let r = rules(vec![
            entry("api.example.com", 443),
            entry("api.example.com", 8443),
            plain,
            entry("[::1]", 443),
            entry("93.184.216.34", 443),
        ]);
        assert_eq!(allowed(&r, "api.example.com", false), Ok(true));
        assert_eq!(allowed(&r, "API.EXAMPLE.COM.:8443", false), Ok(true));
        assert_eq!(denied(&r, "api.example.com:80", false), Reason::Port);
        assert_eq!(denied(&r, "api.example.com", true), Reason::PlainHttp);
        assert_eq!(denied(&r, "other.example.com", false), Reason::NotListed);
        assert_eq!(denied(&r, "api.example.com.evil.net", false), Reason::NotListed);
        assert_eq!(denied(&r, "evil.api.example.com", false), Reason::NotListed);
        assert_eq!(denied(&r, "example.com", false), Reason::NotListed);
        assert_eq!(allowed(&r, "ntfy.lan:80", true), Ok(true));
        assert_eq!(allowed(&r, "ntfy.lan:80", false), Ok(true));
        assert_eq!(allowed(&r, "93.184.216.34", false), Ok(true));
        // An IP literal only if an entry names that address; any other is refused for being one.
        assert_eq!(denied(&r, "93.184.216.35", false), Reason::IpLiteral);
        assert_eq!(denied(&r, "[0:0:0:0:0:0:0:1]", false), Reason::PrivateAddress);
        assert_eq!(denied(&r, "[::1]", false), Reason::PrivateAddress);
        assert_eq!(allowed(&r, "2130706433", false), Err(Reason::IpLiteral));
        assert_eq!(allowed(&r, "a@evil", false), Err(Reason::BadHost));
        // Nothing listed, nothing allowed.
        assert_eq!(denied(&SourceRules::default(), "api.example.com", false), Reason::NotListed);
    }

    #[test]
    fn allow_private_with_and_without() {
        let mut lan = entry("192.168.1.10", 80);
        let mut lo6 = entry("[::1]", 443);
        let without = rules(vec![lan.clone(), lo6.clone(), entry("mock.lan", 80)]);
        assert_eq!(denied(&without, "192.168.1.10:80", false), Reason::PrivateAddress);
        assert_eq!(denied(&without, "[::1]", false), Reason::PrivateAddress);
        lan.allow_private = true;
        lo6.allow_private = true;
        let with = rules(vec![lan, lo6]);
        assert_eq!(allowed(&with, "192.168.1.10:80", false), Ok(true));
        assert_eq!(allowed(&with, "[::1]", false), Ok(true));
        // A name's addresses are judged after resolution: one non-public answer among public ones refuses.
        let pub1 = ip("93.184.216.34");
        let mixed = [pub1, ip("10.0.0.1")];
        let strict = entry("mock.lan", 80);
        let mut open = strict.clone();
        open.allow_private = true;
        assert_eq!(check_addrs(&strict, &[pub1]), Ok(()));
        assert_eq!(check_addrs(&strict, &mixed), Err(Reason::PrivateAddress));
        assert_eq!(check_addrs(&strict, &[ip("::ffff:10.0.0.1")]), Err(Reason::PrivateAddress));
        assert_eq!(check_addrs(&strict, &[]), Err(Reason::PrivateAddress));
        assert_eq!(check_addrs(&open, &mixed), Ok(()));
    }

    /// Each range's neighbours, its first and last address, stated by hand: the list is independent of the tables under test.

    #[test]
    fn any_public_passes_names_on_its_port_only() {
        let mut wild = entry("", 443);
        wild.any_public = true;
        wild.allow_private = true; // never honoured
        let mut wild80 = wild.clone();
        wild80.port = 80;
        wild80.plain_http = true;
        let r = rules(vec![wild.clone(), wild80]);
        assert_eq!(allowed(&r, "anything.example.org", false), Ok(true));
        assert_eq!(allowed(&r, "Any.Example.ORG.:443", false), Ok(true));
        assert_eq!(allowed(&r, "a.example:80", true), Ok(true));
        assert_eq!(denied(&r, "a.example", true), Reason::PlainHttp);
        assert_eq!(denied(&r, "a.example:8443", false), Reason::Port);
        assert_eq!(denied(&r, "93.184.216.34", false), Reason::IpLiteral);
        assert_eq!(denied(&r, "[2606:4700::1]", false), Reason::IpLiteral);
        assert_eq!(denied(&r, "127.0.0.1", false), Reason::IpLiteral);
        assert_eq!(allowed(&r, "2130706433", false), Err(Reason::IpLiteral));
        assert_eq!(allowed(&r, "a@evil", false), Err(Reason::BadHost));
        // A name that resolves to a private address is refused, even though the entry says allow_private.
        let Verdict::Allow(e) = decide(&r, &Host::Name("a.example".into()), 443, false) else { panic!() };
        assert_eq!(check_addrs(e, &[ip("93.184.216.34")]), Ok(()));
        assert_eq!(check_addrs(e, &[ip("10.0.0.1")]), Err(Reason::PrivateAddress));
        assert_eq!(check_addrs(e, &[ip("93.184.216.34"), ip("::1")]), Err(Reason::PrivateAddress));
        // An exact entry is tried before the wildcard, and an IP literal is allowed only when an entry names it.
        let mut lan = entry("ntfy.lan", 443);
        lan.allow_private = true;
        let mut ten = entry("10.0.0.5", 443);
        ten.allow_private = true;
        let r = rules(vec![wild, lan, ten]);
        let Verdict::Allow(e) = decide(&r, &Host::Name("ntfy.lan".into()), 443, false) else { panic!() };
        assert_eq!(check_addrs(e, &[ip("192.168.1.2")]), Ok(()));
        assert_eq!(allowed(&r, "10.0.0.5", false), Ok(true));
    }

    #[test]
    fn arbitrary_input_never_panics() {
        let mut seed = 42_u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as u8
        };
        let r = rules(vec![entry("a.example", 443)]);
        for i in 0..600 {
            let bytes: Vec<u8> = (0..i % 400).map(|_| next()).collect();
            let s = String::from_utf8_lossy(&bytes);
            let _ = normalize_host(&s);
            if let Ok((h, p)) = parse_authority(&s, 443) {
                let _ = decide(&r, &h, p, i % 2 == 0);
            }
        }
        for s in ["[", "]", "[]", "[:", ":", "::", ".", "..", "0x", "[::", "a:b:c", "\u{0}", "é", "[é]", "99999999999999999999"] {
            let _ = parse_authority(s, 443);
        }
        // Every address, sampled, classifies.
        for i in 0..2000u128 {
            let _ = classify(IpAddr::V6(Ipv6Addr::from(i.wrapping_mul(0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c835))));
            let _ = classify(IpAddr::V4(Ipv4Addr::from((i as u32).wrapping_mul(2654435761))));
        }
    }
}
