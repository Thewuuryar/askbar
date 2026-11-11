//! ntfy: an HTTPS `POST` to `<server>/<topic>`, with the priority from the severity and the access token, if there is one,
//! as a bearer token (ADR 0055). The topic and the token are secrets: neither is logged, and neither is in an error.

use super::Problem;
use crate::templates::Rendered;
use askbar_proto::alerter::{NtfyConfig, Severity};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `c` can be used: an `http` or `https` server address with a host, a topic of letters, digits, `-` and `_`, and a
/// token without spaces or control characters. The reason is the alerter's own words and names no value.
pub fn validate(c: &NtfyConfig) -> Result<(), &'static str> {
    let url = reqwest::Url::parse(c.server.trim()).map_err(|_| "The ntfy server is not a web address.")?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("The ntfy server must be an http or https address.");
    }
    if url.query().is_some() || url.fragment().is_some() || !url.username().is_empty() {
        return Err("The ntfy server address must be the server only.");
    }
    if c.topic.is_empty() || c.topic.len() > 64 || !c.topic.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err("The ntfy topic is 1 to 64 letters, digits, - or _.");
    }
    if let Some(t) = &c.token {
        if t.is_empty() || t.len() > 4096 || t.bytes().any(|b| b <= b' ' || b == 0x7f) {
            return Err("The ntfy token has a space or a control character, or is too long.");
        }
    }
    Ok(())
}

/// The URL alerts go to.
fn url(c: &NtfyConfig) -> String {
    format!("{}/{}", c.server.trim().trim_end_matches('/'), c.topic)
}

/// Sends one alert. The title and the body are the alerter's own text.
pub async fn send(http: &reqwest::Client, c: &NtfyConfig, severity: Severity, r: &Rendered) -> Result<(), Problem> {
    let mut req = http
        .post(url(c))
        .timeout(TIMEOUT)
        .header("Title", &r.title)
        .header("Priority", severity.ntfy_priority().to_string())
        .body(r.body.clone());
    req = match severity {
        Severity::Info => req,
        Severity::Warning => req.header("Tags", "warning"),
        Severity::Critical => req.header("Tags", "rotating_light"),
    };
    if let Some(t) = &c.token {
        req = req.bearer_auth(t);
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) if matches!(resp.status().as_u16(), 401 | 403) => Err(Problem::Unauthorized),
        Ok(_) => Err(Problem::Refused),
        Err(e) if e.is_timeout() => Err(Problem::Timeout),
        Err(_) => Err(Problem::Unreachable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(server: &str, topic: &str, token: Option<&str>) -> NtfyConfig {
        NtfyConfig { server: server.into(), topic: topic.into(), token: token.map(Into::into) }
    }

    #[test]
    fn a_config_is_usable_only_if_it_names_a_server_a_plain_topic_and_a_clean_token() {
        assert_eq!(validate(&cfg("https://ntfy.example.com", "askbar-alerts_1", Some("tk_abc"))), Ok(()));
        assert_eq!(validate(&cfg("http://192.168.1.5:8080/", "t", None)), Ok(()));
        for bad in [
            cfg("ntfy.example.com", "t", None),
            cfg("ftp://ntfy.example.com", "t", None),
            cfg("https://ntfy.example.com?x=1", "t", None),
            cfg("https://user:pw@ntfy.example.com", "t", None),
            cfg("https://ntfy.example.com", "", None),
            cfg("https://ntfy.example.com", "has space", None),
            cfg("https://ntfy.example.com", "a/b", None),
            cfg("https://ntfy.example.com", "t", Some("two words")),
            cfg("https://ntfy.example.com", "t", Some("")),
        ] {
            assert!(validate(&bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_address_is_the_server_and_the_topic() {
        assert_eq!(url(&cfg("https://ntfy.example.com/", "t", None)), "https://ntfy.example.com/t");
    }
}
