//! The ADR 0019 URL rules of `fetch`, as pure functions: a URL is accepted or refused with a stable reason (the `reason`
//! member of `fetch_refused`) before any network call. The front is advisory: the egress proxy resolves the name and is
//! authoritative for private ranges. The URL is parsed here by hand, strictly, over the raw text; the `url` crate's
//! lenient parsing (IDN, percent-decoded hosts, `\` as `/`, decimal IPs) is never what decides.

use crate::constants::MAX_URL_CHARS;
use askbar_proto::egress::{parse_authority, Host, Reason};
use askbar_proto::mcp::FETCH_PORTS;

/// A URL that passed: the text to request (no fragment) and what the redirect rules compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub host: String,
    pub https: bool,
}

/// `raw` is an `http` or `https` URL with a plain public-looking name on port 80 or 443, no userinfo and no IP literal, and
/// a query string only if `query_data`. The reason of the first rule it breaks otherwise.
pub fn check(raw: &str, query_data: bool) -> Result<Target, &'static str> {
    if raw.len() > MAX_URL_CHARS {
        return Err("bad_url");
    }
    let (scheme, rest) = raw.split_once("://").ok_or("bad_url")?;
    let https = match scheme.to_ascii_lowercase().as_str() {
        "http" => false,
        "https" => true,
        _ => return Err("scheme"),
    };
    let (auth, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    if auth.contains('@') {
        return Err("userinfo");
    }
    let (host, port) = parse_authority(auth, if https { 443 } else { 80 }).map_err(|r| match r {
        Reason::IpLiteral => "ip_literal",
        Reason::Port => "port",
        _ => "bad_host",
    })?;
    let Host::Name(name) = host else { return Err("ip_literal") };
    if !FETCH_PORTS.contains(&port) {
        return Err("port");
    }
    // A name with no dot is never a public site (`localhost`, a service name); `.local` and the like are never public either.
    if !name.contains('.') || [".localhost", ".local", ".internal", ".localdomain"].iter().any(|s| name.ends_with(s)) {
        return Err("local_name");
    }
    let tail = tail.split('#').next().unwrap_or(""); // the fragment is never sent
    if tail.bytes().any(|b| !(0x21..=0x7e).contains(&b) || b == b'\\') {
        return Err("bad_url");
    }
    if !query_data && tail.contains('?') {
        return Err("query_string");
    }
    Ok(Target { url: format!("{}://{auth}{tail}", if https { "https" } else { "http" }), host: name, https })
}

/// The URL a `Location` header points to, from the page it came from: a full URL or `//host/..` is returned as written (so
/// [`check`] judges the raw text); a relative one is resolved by the `url` crate against a base that already passed, so
/// its host is the base's.
pub fn resolve(base: &Target, location: &str) -> Result<String, &'static str> {
    if location.split(['/', '?', '#']).next().is_some_and(|first| first.contains(':')) {
        return Ok(location.to_string());
    }
    if let Some(rest) = location.strip_prefix("//") {
        return Ok(format!("{}://{rest}", if base.https { "https" } else { "http" }));
    }
    if location.bytes().any(|b| !(0x21..=0x7e).contains(&b) || b == b'\\') {
        return Err("bad_url");
    }
    let base = reqwest::Url::parse(&base.url).map_err(|_| "bad_url")?;
    base.join(location).map(String::from).map_err(|_| "bad_url")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_rule_refuses_with_its_own_reason() {
        let cases = [
            ("ftp://a.example/", "scheme"),
            ("file:///etc/passwd", "scheme"),
            ("gopher://a.example/", "scheme"),
            ("javascript:alert(1)", "bad_url"),
            ("//a.example/x", "bad_url"),
            ("http://a.example:22/", "port"),
            ("http://a.example:8080/", "port"),
            ("https://a.example:0/", "port"),
            ("http://user@a.example/", "userinfo"),
            ("http://user:pw@a.example/", "userinfo"),
            ("http://evil.com\\@good.com/", "userinfo"),
            ("http://evil.com\\good.com/", "bad_host"),
            ("http://b\u{fc}cher.example/", "bad_host"),
            ("http://xn--bcher-kva.example%2f@x/", "userinfo"),
            ("http://a%2eexample/", "bad_host"),
            ("http://a%00.example/", "bad_host"),
            ("http://127.0.0.1/", "ip_literal"),
            ("http://2130706433/", "ip_literal"),
            ("http://0x7f.1/", "ip_literal"),
            ("http://017700000001/", "ip_literal"),
            ("http://0177.0.0.1/", "ip_literal"),
            ("http://[::1]/", "ip_literal"),
            ("http://[::ffff:127.0.0.1]/", "ip_literal"),
            ("http://[::1]:8080/", "ip_literal"),
            ("http://10.0.0.1/", "ip_literal"),
            ("http://localhost/", "local_name"),
            ("http://localhost./", "local_name"),
            ("http://a.localhost/", "local_name"),
            ("http://db.internal/", "local_name"),
            ("http://printer.local/", "local_name"),
            ("http://x.localdomain/", "local_name"),
            ("http://server/", "local_name"),
            ("https://a.example/p?q=1", "query_string"),
            ("https://a.example?q=1", "query_string"),
            ("https://a.example/p?", "query_string"),
            ("https://a.example/a b", "bad_url"),
            ("https://a.example/a\nb", "bad_url"),
            ("https://a.example/a\\b", "bad_url"),
            ("https://a.example/\u{e9}", "bad_url"),
        ];
        for (url, reason) in cases {
            assert_eq!(check(url, false).unwrap_err(), reason, "{url}");
        }
    }

    #[test]
    fn a_plain_public_url_passes_and_the_fragment_is_dropped() {
        for (url, want) in [
            ("http://a.example/x", "http://a.example/x"),
            ("HTTPS://A.Example:443/x#top", "https://A.Example:443/x"),
            ("http://a.example:80", "http://a.example:80"),
            ("https://a.example:80/", "https://a.example:80/"),
            ("https://a.example/#a?b", "https://a.example/"),
            ("https://a.example/%7Euser/", "https://a.example/%7Euser/"),
        ] {
            assert_eq!(check(url, false).map(|t| t.url), Ok(want.to_string()), "{url}");
        }
        assert_eq!(check("https://a.example/p?q=1", true).map(|t| t.url), Ok("https://a.example/p?q=1".to_string()));
        // `query_data` lifts the query rule only.
        assert_eq!(check("https://127.0.0.1/p?q=1", true).unwrap_err(), "ip_literal");
    }

    #[test]
    fn a_location_is_resolved_and_judged_as_written() {
        let base = check("http://a.example/dir/page", false).unwrap();
        let go = |l: &str| resolve(&base, l).and_then(|u| check(&u, false)).map(|t| t.url);
        assert_eq!(go("other"), Ok("http://a.example/dir/other".into()));
        assert_eq!(go("/root"), Ok("http://a.example/root".into()));
        assert_eq!(go("https://b.example/x"), Ok("https://b.example/x".into()));
        assert_eq!(go("//b.example/x"), Ok("http://b.example/x".into()));
        for (l, reason) in [
            ("/x?y=1", "query_string"),
            ("http://127.0.0.1/", "ip_literal"),
            ("http://[::1]/", "ip_literal"),
            ("http://a.example:8080/", "port"),
            ("//169.254.169.254/", "ip_literal"),
            ("ftp://a.example/", "scheme"),
            ("http:evil.example", "bad_url"),
            ("javascript:alert(1)", "bad_url"),
            ("/a\\b", "bad_url"),
            ("/a b", "bad_url"),
        ] {
            assert_eq!(go(l).unwrap_err(), reason, "{l}");
        }
    }
}
