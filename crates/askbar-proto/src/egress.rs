//! Config of the egress proxy (ADR 0056): what each source may reach, and the limits the proxy works within.
//!
//! The host and address checks ([`normalize_host`], [`parse_authority`], [`classify`]) are here too, so the proxy and the MCP
//! front share one copy. They are pure: no I/O, no clock, no panic on any input.
//!
//! The admin derives it and pushes it with `PUT /config`; the proxy holds it in memory. Every type refuses a field it does
//! not know and fills a missing one with its default, so an older push still parses.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The whole config: the allowlist per source (a source is the network a request arrives on) and the limits.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EgressConfig {
    pub sources: BTreeMap<String, SourceRules>,
    pub limits: Limits,
}

/// What one source may reach. Empty means nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SourceRules {
    pub entries: Vec<Entry>,
}

/// One destination: a whole host name (no wildcard, no suffix) or the exact IP literal, and one port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// A lower-case ASCII name (an IDN as `xn--` punycode), a canonical dotted IPv4 or a bracketed IPv6. Ignored with `any_public`.
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Also allow absolute-form `http://` requests, not only `CONNECT`.
    #[serde(default)]
    pub plain_http: bool,
    /// Also reach loopback, private and other non-public addresses (a LAN ntfy, the dev mock).
    #[serde(default)]
    pub allow_private: bool,
    /// The source's relay port tunnels to this entry (SMTP).
    #[serde(default)]
    pub relay: bool,
    /// The one wildcard (the MCP front's `fetch`): any name that passes the host check, on this port. IP literals and
    /// non-public addresses stay refused, whatever `allow_private` says.
    #[serde(default)]
    pub any_public: bool,
}

fn default_port() -> u16 {
    443
}

/// The proxy's limits. The defaults are in force until the first push; a push is checked with [`Limits::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    pub head_bytes: u32,
    pub header_lines: u32,
    pub connect_secs: u32,
    pub head_read_secs: u32,
    pub idle_secs: u32,
    pub max_tunnels: u32,
    pub max_tunnels_per_source: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            head_bytes: 8 * 1024,
            header_lines: 64,
            connect_secs: 10,
            head_read_secs: 10,
            idle_secs: 300,
            max_tunnels: 256,
            max_tunnels_per_source: 128,
        }
    }
}

impl Limits {
    pub const HEAD_BYTES: (u32, u32) = (1024, 64 * 1024);
    pub const HEADER_LINES: (u32, u32) = (8, 256);
    pub const CONNECT_SECS: (u32, u32) = (1, 60);
    pub const HEAD_READ_SECS: (u32, u32) = (1, 60);
    pub const IDLE_SECS: (u32, u32) = (10, 3600);
    pub const TUNNELS: (u32, u32) = (1, 4096);

    /// The name of the first field out of its bounds. A source may not be allowed more tunnels than there are in all.
    pub fn check(&self) -> Result<(), &'static str> {
        let ok = |v: u32, (lo, hi): (u32, u32)| (lo..=hi).contains(&v);
        let fields = [
            ("head_bytes", ok(self.head_bytes, Self::HEAD_BYTES)),
            ("header_lines", ok(self.header_lines, Self::HEADER_LINES)),
            ("connect_secs", ok(self.connect_secs, Self::CONNECT_SECS)),
            ("head_read_secs", ok(self.head_read_secs, Self::HEAD_READ_SECS)),
            ("idle_secs", ok(self.idle_secs, Self::IDLE_SECS)),
            ("max_tunnels", ok(self.max_tunnels, Self::TUNNELS)),
            ("max_tunnels_per_source", ok(self.max_tunnels_per_source, Self::TUNNELS) && self.max_tunnels_per_source <= self.max_tunnels),
        ];
        match fields.iter().find(|f| !f.1) {
            Some(f) => Err(f.0),
            None => Ok(()),
        }
    }
}

impl EgressConfig {
    /// A hash of the config (FNV-1a over its JSON, whose keys are sorted), so the admin tells whether a proxy runs the config
    /// it would push without a counter, and a restarted proxy (version null) is seen at once.
    pub fn version(&self) -> u64 {
        let json = serde_json::to_vec(self).unwrap_or_default();
        json.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
    }
}

/// The most `PUT /config` body the proxy reads.
pub const MAX_CONFIG_BYTES: usize = 256 * 1024;
/// How many denied attempts `GET /status` keeps, newest first.
pub const RECENT_DENIED: usize = 50;
/// How long the proxy reads and drops what a client still sends after a refusal, so the answer is not lost to a reset.
pub const DRAIN_MILLIS: u64 = 500;
/// The header that carries a denial's [`Reason::code`] on a `403`.
pub const REASON_HEADER: &str = "X-Askbar-Egress-Reason";

/// `GET /status`. No path, header or content of any request is in it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressStatus {
    pub configured: bool,
    /// [`EgressConfig::version`] of the config in force; null until the first push.
    pub version: Option<u64>,
    pub boot_id: String,
    pub open_tunnels: u32,
    pub sources: BTreeMap<String, SourceStats>,
    pub recent_denied: Vec<Denied>,
}

/// What one source's listener has done since the process started.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStats {
    pub allowed: u64,
    /// Denied attempts by reason code.
    pub denied: BTreeMap<String, u64>,
}

/// One denied attempt: the host as validated (or `invalid`), never a URL or a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Denied {
    pub source: String,
    pub host: String,
    pub port: Option<u16>,
    pub reason: String,
    /// Unix seconds.
    pub at: i64,
}

/// A host that passed [`normalize_host`]: a lower-case ASCII name, or an IP literal in its one accepted form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Host {
    Name(String),
    Ip(IpAddr),
}

/// Why a request is refused. The code is the fixed `X-Askbar-Egress-Reason` value and the log field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NotListed,
    BadHost,
    IpLiteral,
    Port,
    PrivateAddress,
    PlainHttp,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::NotListed => "not_listed",
            Reason::BadHost => "bad_host",
            Reason::IpLiteral => "ip_literal",
            Reason::Port => "port",
            Reason::PrivateAddress => "private_address",
            Reason::PlainHttp => "plain_http",
        }
    }
}

/// Why an address is not public. The first matching range names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Unspecified,
    ThisNetwork,
    Loopback,
    Private,
    Cgnat,
    LinkLocal,
    Metadata,
    SiteLocal,
    UniqueLocal,
    Multicast,
    Broadcast,
    Documentation,
    Benchmarking,
    Teredo,
    Discard,
    Reserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Public,
    NonPublic(Kind),
}

const fn v4(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_be_bytes([a, b, c, d])
}

/// Non-public IPv4 ranges, most specific first. Everything else is public.
const V4: &[(u32, u32, Kind)] = &[
    (v4(169, 254, 169, 254), 32, Kind::Metadata),   // cloud metadata (inside link-local, RFC 3927)
    (v4(0, 0, 0, 0), 32, Kind::Unspecified),        // RFC 1122
    (v4(0, 0, 0, 0), 8, Kind::ThisNetwork),         // RFC 1122 3.2.1.3
    (v4(10, 0, 0, 0), 8, Kind::Private),            // RFC 1918
    (v4(100, 64, 0, 0), 10, Kind::Cgnat),           // RFC 6598
    (v4(127, 0, 0, 0), 8, Kind::Loopback),          // RFC 1122
    (v4(169, 254, 0, 0), 16, Kind::LinkLocal),      // RFC 3927
    (v4(172, 16, 0, 0), 12, Kind::Private),         // RFC 1918
    (v4(192, 0, 0, 0), 24, Kind::Reserved),         // IETF protocol assignments, RFC 6890
    (v4(192, 0, 2, 0), 24, Kind::Documentation),    // TEST-NET-1, RFC 5737
    (v4(192, 88, 99, 0), 24, Kind::Reserved),       // 6to4 relay anycast, RFC 7526
    (v4(192, 168, 0, 0), 16, Kind::Private),        // RFC 1918
    (v4(198, 18, 0, 0), 15, Kind::Benchmarking),    // RFC 2544
    (v4(198, 51, 100, 0), 24, Kind::Documentation), // TEST-NET-2, RFC 5737
    (v4(203, 0, 113, 0), 24, Kind::Documentation),  // TEST-NET-3, RFC 5737
    (v4(224, 0, 0, 0), 4, Kind::Multicast),         // RFC 5771
    (v4(255, 255, 255, 255), 32, Kind::Broadcast),  // RFC 919
    (v4(240, 0, 0, 0), 4, Kind::Reserved),          // class E, RFC 1112
];

const fn v6(a: u16, b: u16, c: u16) -> u128 {
    ((a as u128) << 112) | ((b as u128) << 96) | ((c as u128) << 80)
}

/// Non-public IPv6 ranges, most specific first. IPv4-mapped, NAT64 and 6to4 are judged by the IPv4 inside them, before this.
const V6: &[(u128, u32, Kind)] = &[
    (1, 128, Kind::Loopback),                         // ::1, RFC 4291
    (0, 128, Kind::Unspecified),                      // ::, RFC 4291
    (0, 96, Kind::Reserved),                          // IPv4-compatible ::a.b.c.d, deprecated, RFC 4291
    (v6(0xfc00, 0, 0), 7, Kind::UniqueLocal),         // ULA, RFC 4193
    (v6(0xfe80, 0, 0), 10, Kind::LinkLocal),          // RFC 4291
    (v6(0xfec0, 0, 0), 10, Kind::SiteLocal),          // deprecated, RFC 3879
    (v6(0xff00, 0, 0), 8, Kind::Multicast),           // RFC 4291
    (v6(0x2001, 0x0db8, 0), 32, Kind::Documentation), // RFC 3849
    (v6(0x3fff, 0, 0), 20, Kind::Documentation),      // RFC 9637
    (v6(0x2001, 0x0002, 0), 48, Kind::Benchmarking),  // RFC 5180
    (v6(0x2001, 0, 0), 32, Kind::Teredo),             // RFC 4380: hides a client's IPv4, so none is judged
    (v6(0x2001, 0, 0), 23, Kind::Reserved),           // IETF protocol assignments, RFC 2928
    (v6(0x0100, 0, 0), 64, Kind::Discard),            // RFC 6666
    (v6(0x0064, 0xff9b, 1), 48, Kind::Reserved),      // local-use NAT64, RFC 8215
];

fn classify_v4(a: Ipv4Addr) -> Class {
    let n = u32::from(a);
    match V4.iter().find(|(base, bits, _)| n >> (32 - bits) == base >> (32 - bits)) {
        Some(&(_, _, k)) => Class::NonPublic(k),
        None => Class::Public,
    }
}

/// Is this address one the proxy may connect to without `allow_private`?
pub fn classify(ip: IpAddr) -> Class {
    match ip {
        IpAddr::V4(a) => classify_v4(a),
        IpAddr::V6(a) => {
            let s = a.segments();
            let embedded = |hi: u16, lo: u16| classify_v4(Ipv4Addr::from(((hi as u32) << 16) | lo as u32));
            if let Some(m) = a.to_ipv4_mapped() {
                classify_v4(m) // ::ffff:0:0/96, RFC 4291
            } else if s[..2] == [0x64, 0xff9b] && s[2..6] == [0; 4] {
                embedded(s[6], s[7]) // NAT64 64:ff9b::/96, RFC 6052
            } else if s[0] == 0x2002 {
                embedded(s[1], s[2]) // 6to4 2002::/16, RFC 3056
            } else {
                let n = u128::from(a);
                match V6.iter().find(|(base, bits, _)| n >> (128 - bits) == base >> (128 - bits)) {
                    Some(&(_, _, k)) => Class::NonPublic(k),
                    None => Class::Public,
                }
            }
        }
    }
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// The one accepted IPv4 spelling: four decimal numbers, 0 to 255, no leading zero.
fn canonical_v4(s: &str) -> Option<Ipv4Addr> {
    let mut out = [0u8; 4];
    let mut parts = s.split('.');
    for o in &mut out {
        let p = parts.next()?;
        if !is_digits(p) || p.len() > 3 || (p.len() > 1 && p.starts_with('0')) {
            return None;
        }
        *o = p.parse().ok()?;
    }
    parts.next().is_none().then_some(Ipv4Addr::from(out))
}

/// Check and normalize a host with no port. Lower-case ASCII; one trailing dot is dropped; no userinfo; labels of 1 to 63
/// of `a-z0-9-` not starting or ending with `-`; at most 253 bytes. A name whose last label is all digits or starts `0x` is
/// an IP literal: only the canonical dotted quad (and `[ipv6]`) is accepted, as `Host::Ip`; the odd forms are `IpLiteral`.
pub fn normalize_host(s: &str) -> Result<Host, Reason> {
    if !s.is_ascii() || s.is_empty() || s.len() > 255 {
        return Err(Reason::BadHost);
    }
    if let Some(inner) = s.strip_prefix('[') {
        let v6: Ipv6Addr = inner.strip_suffix(']').and_then(|i| i.parse().ok()).ok_or(Reason::BadHost)?;
        return Ok(Host::Ip(v6.into()));
    }
    let h = s.strip_suffix('.').unwrap_or(s).to_ascii_lowercase();
    if h.is_empty() || h.len() > 253 {
        return Err(Reason::BadHost);
    }
    for label in h.split('.') {
        let ok = (1..=63).contains(&label.len())
            && label.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-');
        if !ok {
            return Err(Reason::BadHost);
        }
    }
    let last = h.rsplit('.').next().unwrap_or("");
    if is_digits(last) || last.starts_with("0x") {
        return canonical_v4(&h).map(|a| Host::Ip(a.into())).ok_or(Reason::IpLiteral);
    }
    Ok(Host::Name(h))
}

fn parse_port(p: &str) -> Result<u16, Reason> {
    if !is_digits(p) || p.len() > 5 {
        return Err(Reason::Port);
    }
    p.parse::<u16>().ok().filter(|&n| n != 0).ok_or(Reason::Port)
}

/// Split `host[:port]` (a `CONNECT` target or an absolute-form authority) and check both. The host is checked first.
pub fn parse_authority(s: &str, default_port: u16) -> Result<(Host, u16), Reason> {
    if !s.is_ascii() || s.len() > 300 {
        return Err(Reason::BadHost);
    }
    let (host, port) =
        if s.starts_with('[') { s.split_at(s.find(']').ok_or(Reason::BadHost)? + 1) } else { s.split_at(s.find(':').unwrap_or(s.len())) };
    let host = normalize_host(host)?;
    match port.strip_prefix(':') {
        Some(p) => Ok((host, parse_port(p)?)),
        None if port.is_empty() => Ok((host, default_port)),
        None => Err(Reason::BadHost),
    }
}

#[cfg(test)]
mod tests {
    use super::Kind::*;
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }
    fn name(s: &str) -> Host {
        Host::Name(s.into())
    }

    #[test]
    fn hosts_table() {
        let long_label = "a".repeat(64);
        let long_name = format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(62)); // 254 bytes
        let max_name = format!("{}.{}.{}.{}", "a".repeat(63), "b".repeat(63), "c".repeat(63), "d".repeat(61)); // 253
        let cases: Vec<(&str, Result<Host, Reason>)> = vec![
            ("api.example.com", Ok(name("api.example.com"))),
            ("API.Example.COM", Ok(name("api.example.com"))),
            ("api.example.com.", Ok(name("api.example.com"))),
            ("api.example.com..", Err(Reason::BadHost)),
            (".", Err(Reason::BadHost)),
            ("", Err(Reason::BadHost)),
            (".example.com", Err(Reason::BadHost)),
            ("a..b", Err(Reason::BadHost)),
            ("xn--bcher-kva.example", Ok(name("xn--bcher-kva.example"))),
            ("-a.example", Err(Reason::BadHost)),
            ("a-.example", Err(Reason::BadHost)),
            ("a_b.example", Err(Reason::BadHost)),
            ("a b.example", Err(Reason::BadHost)),
            ("a\0b", Err(Reason::BadHost)),
            ("a/b", Err(Reason::BadHost)),
            ("a%2eb", Err(Reason::BadHost)),
            ("*.example.com", Err(Reason::BadHost)),
            ("a@evil", Err(Reason::BadHost)),
            ("evil@a", Err(Reason::BadHost)),
            ("user:pw@a.example", Err(Reason::BadHost)),
            ("bücher.example", Err(Reason::BadHost)),
            ("例え.jp", Err(Reason::BadHost)),
            ("ａ.example", Err(Reason::BadHost)),
            (long_label.as_str(), Err(Reason::BadHost)),
            (long_name.as_str(), Err(Reason::BadHost)),
            (max_name.as_str(), Ok(name(&max_name))),
            ("1.2.3.4", Ok(Host::Ip(ip("1.2.3.4")))),
            ("127.0.0.1", Ok(Host::Ip(ip("127.0.0.1")))),
            ("2130706433", Err(Reason::IpLiteral)),
            ("0177.0.0.1", Err(Reason::IpLiteral)),
            ("0x7f.1", Err(Reason::IpLiteral)),
            ("0x7f.0.0.1", Err(Reason::IpLiteral)),
            ("0X7F000001", Err(Reason::IpLiteral)),
            ("127.1", Err(Reason::IpLiteral)),
            ("127.0.1", Err(Reason::IpLiteral)),
            ("127.0.0.0x1", Err(Reason::IpLiteral)),
            ("1.2.3.4.5", Err(Reason::IpLiteral)),
            ("1.2.3.256", Err(Reason::IpLiteral)),
            ("01.2.3.4", Err(Reason::IpLiteral)),
            ("0x1", Err(Reason::IpLiteral)),
            ("[::1]", Ok(Host::Ip(ip("::1")))),
            ("[0:0:0:0:0:0:0:1]", Ok(Host::Ip(ip("::1")))),
            ("[2001:DB8::1]", Ok(Host::Ip(ip("2001:db8::1")))),
            ("[::ffff:127.0.0.1]", Ok(Host::Ip(ip("::ffff:127.0.0.1")))),
            ("[::1", Err(Reason::BadHost)),
            ("::1", Err(Reason::BadHost)),
            ("[1.2.3.4]", Err(Reason::BadHost)),
            ("[fe80::1%eth0]", Err(Reason::BadHost)),
            ("[]", Err(Reason::BadHost)),
            ("api.example.com.evil.net", Ok(name("api.example.com.evil.net"))),
        ];
        for (input, want) in cases {
            assert_eq!(normalize_host(input), want, "{input:?}");
        }
    }

    #[test]
    fn authorities_table() {
        type Parsed = Result<(Host, u16), Reason>;
        let cases: Vec<(&str, Parsed)> = vec![
            ("a.example", Ok((name("a.example"), 443))),
            ("a.example:8443", Ok((name("a.example"), 8443))),
            ("A.EXAMPLE.:80", Ok((name("a.example"), 80))),
            ("a.example:1", Ok((name("a.example"), 1))),
            ("a.example:65535", Ok((name("a.example"), 65535))),
            ("a.example:0", Err(Reason::Port)),
            ("a.example:65536", Err(Reason::Port)),
            ("a.example:443:80", Err(Reason::Port)),
            ("a.example:", Err(Reason::Port)),
            ("a.example:+80", Err(Reason::Port)),
            ("a.example:-1", Err(Reason::Port)),
            ("a.example:80a", Err(Reason::Port)),
            ("a.example:000080", Err(Reason::Port)),
            ("a.example: 80", Err(Reason::Port)),
            (":443", Err(Reason::BadHost)),
            ("a@evil:443", Err(Reason::BadHost)),
            ("evil@a:0", Err(Reason::BadHost)),
            ("[::1]:8080", Ok((Host::Ip(ip("::1")), 8080))),
            ("[::1]", Ok((Host::Ip(ip("::1")), 443))),
            ("[::1]:0", Err(Reason::Port)),
            ("[::1]8080", Err(Reason::BadHost)),
            ("[::1]]:80", Err(Reason::BadHost)),
            ("[::1", Err(Reason::BadHost)),
            ("127.1:80", Err(Reason::IpLiteral)),
            ("1.2.3.4:80", Ok((Host::Ip(ip("1.2.3.4")), 80))),
            ("bücher.example:80", Err(Reason::BadHost)),
        ];
        for (input, want) in cases {
            assert_eq!(parse_authority(input, 443), want, "{input:?}");
        }
    }

    /// Each range's neighbours, its first and last address, stated by hand: the list is independent of the tables under test.
    #[test]
    fn classify_range_boundaries() {
        let t: &[(&str, Option<Kind>)] = &[
            // 0.0.0.0/8, 0.0.0.0/32
            ("0.0.0.0", Some(Unspecified)),
            ("0.0.0.1", Some(ThisNetwork)),
            ("0.255.255.255", Some(ThisNetwork)),
            ("1.0.0.0", None),
            // 10/8
            ("9.255.255.255", None),
            ("10.0.0.0", Some(Private)),
            ("10.255.255.255", Some(Private)),
            ("11.0.0.0", None),
            // 100.64/10
            ("100.63.255.255", None),
            ("100.64.0.0", Some(Cgnat)),
            ("100.127.255.255", Some(Cgnat)),
            ("100.128.0.0", None),
            // 127/8
            ("126.255.255.255", None),
            ("127.0.0.0", Some(Loopback)),
            ("127.0.0.1", Some(Loopback)),
            ("127.255.255.255", Some(Loopback)),
            ("128.0.0.0", None),
            // 169.254/16 and the metadata address
            ("169.253.255.255", None),
            ("169.254.0.0", Some(LinkLocal)),
            ("169.254.169.253", Some(LinkLocal)),
            ("169.254.169.254", Some(Metadata)),
            ("169.254.169.255", Some(LinkLocal)),
            ("169.254.255.255", Some(LinkLocal)),
            ("169.255.0.0", None),
            // 172.16/12
            ("172.15.255.255", None),
            ("172.16.0.0", Some(Private)),
            ("172.31.255.255", Some(Private)),
            ("172.32.0.0", None),
            // 192.0.0/24, 192.0.2/24
            ("191.255.255.255", None),
            ("192.0.0.0", Some(Reserved)),
            ("192.0.0.255", Some(Reserved)),
            ("192.0.1.0", None),
            ("192.0.1.255", None),
            ("192.0.2.0", Some(Documentation)),
            ("192.0.2.255", Some(Documentation)),
            ("192.0.3.0", None),
            // 192.88.99/24
            ("192.88.98.255", None),
            ("192.88.99.0", Some(Reserved)),
            ("192.88.99.255", Some(Reserved)),
            ("192.88.100.0", None),
            // 192.168/16
            ("192.167.255.255", None),
            ("192.168.0.0", Some(Private)),
            ("192.168.255.255", Some(Private)),
            ("192.169.0.0", None),
            // 198.18/15, 198.51.100/24, 203.0.113/24
            ("198.17.255.255", None),
            ("198.18.0.0", Some(Benchmarking)),
            ("198.19.255.255", Some(Benchmarking)),
            ("198.20.0.0", None),
            ("198.51.99.255", None),
            ("198.51.100.0", Some(Documentation)),
            ("198.51.100.255", Some(Documentation)),
            ("198.51.101.0", None),
            ("203.0.112.255", None),
            ("203.0.113.0", Some(Documentation)),
            ("203.0.113.255", Some(Documentation)),
            ("203.0.114.0", None),
            // 224/4, 240/4, broadcast
            ("223.255.255.255", None),
            ("224.0.0.0", Some(Multicast)),
            ("239.255.255.255", Some(Multicast)),
            ("240.0.0.0", Some(Reserved)),
            ("255.255.255.254", Some(Reserved)),
            ("255.255.255.255", Some(Broadcast)),
            // ordinary public addresses
            ("8.8.8.8", None),
            ("1.1.1.1", None),
            ("93.184.216.34", None),
            // IPv6: ::, ::1, ::/96
            ("::", Some(Unspecified)),
            ("::1", Some(Loopback)),
            ("::2", Some(Reserved)),
            ("::ffff:ffff", Some(Reserved)),
            ("::1:0:0", None),
            // IPv4-mapped ::ffff:0:0/96: judged by the IPv4 inside
            ("::ffff:127.0.0.1", Some(Loopback)),
            ("::ffff:10.1.2.3", Some(Private)),
            ("::ffff:169.254.169.254", Some(Metadata)),
            ("::ffff:0.0.0.0", Some(Unspecified)),
            ("::ffff:255.255.255.255", Some(Broadcast)),
            ("::ffff:8.8.8.8", None),
            ("::fffe:7f00:1", None),
            ("::1:ffff:7f00:1", None),
            // NAT64 64:ff9b::/96: judged by the IPv4 inside
            ("64:ff9b::7f00:1", Some(Loopback)),
            ("64:ff9b::a00:1", Some(Private)),
            ("64:ff9b::a9fe:a9fe", Some(Metadata)),
            ("64:ff9b::808:808", None),
            ("64:ff9b:0:0:0:1:808:808", None),
            ("64:ff9a:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("64:ff9c::", None),
            // local-use NAT64 64:ff9b:1::/48
            ("64:ff9b:0:ffff:ffff:ffff:ffff:ffff", None),
            ("64:ff9b:1::", Some(Reserved)),
            ("64:ff9b:1:ffff:ffff:ffff:ffff:ffff", Some(Reserved)),
            ("64:ff9b:2::", None),
            // 6to4 2002::/16: judged by the IPv4 inside
            ("2002:7f00:1::", Some(Loopback)),
            ("2002:a00:1::1", Some(Private)),
            ("2002:c0a8:101::", Some(Private)),
            ("2002:808:808::", None),
            ("2001:ffff:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("2003::", None),
            // Teredo 2001::/32, benchmarking 2001:2::/48, IETF 2001::/23, documentation 2001:db8::/32
            ("2000:ffff:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("2001::", Some(Teredo)),
            ("2001:0:ffff:ffff:ffff:ffff:ffff:ffff", Some(Teredo)),
            ("2001:1::", Some(Reserved)),
            ("2001:1:ffff:ffff:ffff:ffff:ffff:ffff", Some(Reserved)),
            ("2001:2::", Some(Benchmarking)),
            ("2001:2:0:ffff:ffff:ffff:ffff:ffff", Some(Benchmarking)),
            ("2001:2:1::", Some(Reserved)),
            ("2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff", Some(Reserved)),
            ("2001:200::", None),
            ("2001:db7:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("2001:db8::", Some(Documentation)),
            ("2001:db8:ffff:ffff:ffff:ffff:ffff:ffff", Some(Documentation)),
            ("2001:db9::", None),
            // 3fff::/20
            ("3ffe:ffff:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("3fff::", Some(Documentation)),
            ("3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff", Some(Documentation)),
            ("3fff:1000::", None),
            // discard 100::/64
            ("ff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("100::", Some(Discard)),
            ("100::ffff:ffff:ffff:ffff", Some(Discard)),
            ("100:0:0:1::", None),
            // ULA fc00::/7
            ("fbff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("fc00::", Some(UniqueLocal)),
            ("fd00:ec2::254", Some(UniqueLocal)),
            ("fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Some(UniqueLocal)),
            ("fe00::", None),
            // link-local fe80::/10, site-local fec0::/10
            ("fe7f:ffff:ffff:ffff:ffff:ffff:ffff:ffff", None),
            ("fe80::", Some(LinkLocal)),
            ("febf:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Some(LinkLocal)),
            ("fec0::", Some(SiteLocal)),
            ("feff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Some(SiteLocal)),
            ("ff00::", Some(Multicast)),
            ("ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", Some(Multicast)),
            // ordinary public
            ("2606:4700:4700::1111", None),
            ("2a00:1450:4001:81b::200e", None),
        ];
        for (addr, want) in t {
            let want = want.map_or(Class::Public, Class::NonPublic);
            assert_eq!(classify(ip(addr)), want, "{addr}");
        }
    }

    #[test]
    fn no_character_outside_the_set_is_accepted() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        let pool: Vec<char> = "abcXYZ019-.[]:".chars().collect();
        let bad: Vec<char> = " \t\r\n\0@/\\?#%_*!~;,'\"<>é中\u{7f}\u{1}".chars().collect();
        for _ in 0..3000 {
            let len = next() % 24;
            let mut s: String = (0..len).map(|_| pool[next() % pool.len()]).collect();
            // What is accepted is a name of lower-case letters, digits, dots and hyphens, or a literal.
            if let Ok(Host::Name(n)) = normalize_host(&s) {
                assert!(n.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.'), "{s:?}");
            }
            let at = next() % (s.len() + 1);
            s.insert(at, bad[next() % bad.len()]);
            assert!(normalize_host(&s).is_err(), "{s:?}");
            assert!(parse_authority(&s, 443).is_err(), "{s:?}");
        }
    }

    #[test]
    fn reason_codes_are_distinct() {
        let all = [Reason::NotListed, Reason::BadHost, Reason::IpLiteral, Reason::Port, Reason::PrivateAddress, Reason::PlainHttp];
        let codes: std::collections::BTreeSet<_> = all.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), all.len());
    }

    #[test]
    fn defaults_fill_in_and_are_in_bounds() {
        let c: EgressConfig = serde_json::from_str(r#"{"sources":{"a":{"entries":[{"host":"x.example"}]}}}"#).unwrap();
        let e = &c.sources["a"].entries[0];
        assert_eq!((e.port, e.plain_http, e.allow_private, e.relay), (443, false, false, false));
        assert_eq!(c.limits, Limits::default());
        assert_eq!(c.limits.check(), Ok(()));
        assert_eq!(serde_json::from_str::<EgressConfig>("{}").unwrap(), EgressConfig::default());
    }

    #[test]
    fn unknown_fields_are_refused() {
        assert!(serde_json::from_str::<EgressConfig>(r#"{"x":1}"#).is_err());
        assert!(serde_json::from_str::<Entry>(r#"{"host":"a","wildcard":true}"#).is_err());
    }

    #[test]
    fn limits_out_of_bounds_name_the_field() {
        let d = Limits::default();
        assert_eq!(Limits { head_bytes: 10, ..d }.check(), Err("head_bytes"));
        assert_eq!(Limits { idle_secs: 0, ..d }.check(), Err("idle_secs"));
        assert_eq!(Limits { max_tunnels: 4097, ..d }.check(), Err("max_tunnels"));
        assert_eq!(Limits { max_tunnels_per_source: 257, ..d }.check(), Err("max_tunnels_per_source"));
    }
}
