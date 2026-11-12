//! Email: one message to one address through an SMTP server, with `lettre` (ADR 0055). The host, the user, the password and the
//! addresses are secrets: none is logged, and none is in an error. Mail is plain text: the alerter's own sentence, nothing a
//! sender wrote.

use super::Problem;
use crate::templates::Rendered;
use askbar_proto::alerter::{Severity, SmtpConfig, SmtpTls};
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use std::time::Duration;

/// How long one SMTP command may take, and the whole delivery.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(15);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(60);

/// A host name or address: letters, digits, `.`, `-` and `:` (an IPv6 address), no spaces or `/`.
fn plain_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// Whether `c` can be used. The reason is the alerter's own words and names no value.
pub fn validate(c: &SmtpConfig) -> Result<(), &'static str> {
    if !plain_host(c.host.trim()) || c.host != c.host.trim() {
        return Err("The mail server must be a host name or an address, without spaces.");
    }
    if c.port == 0 {
        return Err("The mail server's port is 1 to 65535.");
    }
    match (&c.user, &c.password) {
        (Some(u), Some(p)) => {
            if u.is_empty() || p.is_empty() || u.len() > 256 || p.len() > 1024 || u.bytes().chain(p.bytes()).any(|b| b < b' ' || b == 0x7f)
            {
                return Err("The mail user name or password is empty, too long or has a control character.");
            }
            if c.tls == SmtpTls::None {
                return Err("A mail password is not sent without TLS: choose STARTTLS or TLS, or remove the user name and password.");
            }
        }
        (None, None) => {}
        _ => return Err("The mail user name and password go together: give both or neither."),
    }
    mailbox(&c.from).ok_or("The from address is not an email address.")?;
    mailbox(&c.to).ok_or("The to address is not an email address.")?;
    Ok(())
}

fn mailbox(address: &str) -> Option<Mailbox> {
    // A bare address only: no name, no angle brackets, no list, and nothing a header could be split with.
    if address.len() > 254 || address.bytes().any(|b| b <= b' ' || b == 0x7f || matches!(b, b'<' | b'>' | b',' | b';' | b'"')) {
        return None;
    }
    address.parse().ok()
}

fn transport(c: &SmtpConfig) -> Result<AsyncSmtpTransport<Tokio1Executor>, Problem> {
    let mut b = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(c.host.clone()).port(c.port).timeout(Some(COMMAND_TIMEOUT));
    b = match c.tls {
        SmtpTls::None => b.tls(Tls::None),
        SmtpTls::Starttls => b.tls(Tls::Required(TlsParameters::new(c.host.clone()).map_err(|_| Problem::Tls)?)),
        SmtpTls::Tls => b.tls(Tls::Wrapper(TlsParameters::new(c.host.clone()).map_err(|_| Problem::Tls)?)),
    };
    if let (Some(u), Some(p)) = (&c.user, &c.password) {
        b = b.credentials(Credentials::new(u.clone(), p.clone()));
    }
    Ok(b.build())
}

/// How a failed send reads, in the alerter's words.
fn problem(e: &lettre::transport::smtp::Error) -> Problem {
    if e.is_timeout() {
        Problem::Timeout
    } else if e.is_tls() {
        Problem::Tls
    } else if let Some(code) = e.status() {
        // 530, 534 and 535 are what servers answer to a login they refuse; 454 is a temporary failure to authenticate.
        if matches!(u16::from(code), 454 | 530 | 534 | 535) {
            Problem::Unauthorized
        } else {
            Problem::Refused
        }
    } else if e.is_response() || e.is_client() {
        Problem::Refused
    } else {
        Problem::Unreachable
    }
}

/// Sends one alert as one message. The subject and the body are the alerter's own text.
pub async fn send(c: &SmtpConfig, severity: Severity, r: &Rendered) -> Result<(), Problem> {
    let (from, to) = (mailbox(&c.from).ok_or(Problem::Refused)?, mailbox(&c.to).ok_or(Problem::Refused)?);
    let prefix = match severity {
        Severity::Info => "",
        Severity::Warning => "[warning] ",
        Severity::Critical => "[critical] ",
    };
    let mail = Message::builder()
        .from(from)
        .to(to)
        .subject(format!("{prefix}{}", r.title))
        .header(ContentType::TEXT_PLAIN)
        .body(r.body.clone())
        .map_err(|_| Problem::Refused)?;
    let transport = transport(c)?;
    match tokio::time::timeout(TOTAL_TIMEOUT, transport.send(mail)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(problem(&e)),
        Err(_) => Err(Problem::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SmtpConfig {
        SmtpConfig {
            host: "mail.example.com".into(),
            port: 587,
            tls: SmtpTls::Starttls,
            user: Some("u".into()),
            password: Some("p".into()),
            from: "askbar@example.com".into(),
            to: "owner@example.com".into(),
        }
    }

    #[test]
    fn a_config_is_usable_only_with_a_plain_host_good_addresses_and_a_password_that_travels_under_tls() {
        assert_eq!(validate(&cfg()), Ok(()));
        let relay = SmtpConfig { tls: SmtpTls::None, user: None, password: None, port: 25, ..cfg() };
        assert_eq!(validate(&relay), Ok(()));
        let bad: Vec<SmtpConfig> = vec![
            SmtpConfig { host: "".into(), ..cfg() },
            SmtpConfig { host: "mail example.com".into(), ..cfg() },
            SmtpConfig { host: "smtp://mail.example.com".into(), ..cfg() },
            SmtpConfig { port: 0, ..cfg() },
            SmtpConfig { password: None, ..cfg() },
            SmtpConfig { user: None, ..cfg() },
            SmtpConfig { tls: SmtpTls::None, ..cfg() },
            SmtpConfig { from: "not an address".into(), ..cfg() },
            SmtpConfig { to: "a@example.com, b@example.com".into(), ..cfg() },
            SmtpConfig { to: "Owner <owner@example.com>".into(), ..cfg() },
            SmtpConfig { to: "owner@example.com\r\nBcc: x@example.com".into(), ..cfg() },
        ];
        for b in bad {
            assert!(validate(&b).is_err(), "{b:?}");
        }
    }
}
