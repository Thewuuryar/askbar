//! Caddy's access logs (plan step 2.9, ADR 0051): a listener of its own that takes newline-delimited JSON from Caddy's
//! `net` log writer. That writer cannot carry a token, so the only control is the network: the listener is bound to the
//! address of its alias on `audit-caddy`, a network of Caddy and the auditor alone. It reads and never answers, so Caddy
//! cannot read anything through it, and a line is only ever appended.
//!
//! A line becomes one row with a short list of fields. No header and no query string is ever kept, because either can
//! carry a secret. A line that is not Caddy's access log, or is over 16 KiB, is not stored and is counted. Access lines
//! are best effort (they are not fail-closed like model calls): the recorder takes them only while its queue is under
//! half full.

use crate::Auditor;
use askbar_audit::AccessLogWrite;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

/// The longest line taken. Caddy's access lines are a few hundred bytes plus headers; this leaves room for those.
pub const MAX_LINE: usize = 16 * 1024;
/// Caddy keeps one connection; a few more cover a reconnect.
const MAX_CONNECTIONS: usize = 8;

fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect()
}

fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key)?.as_str()
}

/// One line of Caddy's JSON access log as a row, or `None` if it is not one. Only the fields below are read.
pub fn parse_line(line: &[u8]) -> Option<AccessLogWrite> {
    let v: Value = serde_json::from_slice(line).ok()?;
    let req = v.get("request")?;
    let method = text(req, "method")?;
    if method.is_empty() || method.len() > 16 || !method.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    // The path ends where the query or a fragment begins: what follows may be a token.
    let uri = text(req, "uri")?;
    let path = uri.split(['?', '#']).next().unwrap_or("");
    let status = v.get("status")?.as_i64().filter(|s| (100..=599).contains(s))?;
    let at = v.get("ts")?.as_f64()?.floor() as i64;
    let seconds = v.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    let user_agent = req
        .get("headers")
        .and_then(|h| h.get("User-Agent"))
        .and_then(|a| a.get(0))
        .and_then(Value::as_str)
        .map(|a| clean(a, 200))
        .filter(|a| !a.is_empty());
    Some(AccessLogWrite {
        at,
        method: method.to_ascii_uppercase(),
        host: clean(text(req, "host").unwrap_or(""), 253),
        path: clean(path, 2048),
        status,
        bytes: v.get("size").and_then(Value::as_i64).unwrap_or(0).max(0),
        duration_ms: (seconds * 1000.0).round().max(0.0) as i64,
        remote_ip: clean(text(req, "client_ip").or_else(|| text(req, "remote_ip")).unwrap_or(""), 64),
        proto: clean(text(req, "proto").unwrap_or(""), 16),
        user_agent,
    })
}

/// Takes connections for as long as the process runs, each read to its end.
pub async fn serve(listener: TcpListener, auditor: Arc<Auditor>) {
    let room = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            continue;
        };
        let Ok(permit) = room.clone().try_acquire_owned() else { continue }; // too many: the stream is dropped
        let auditor = auditor.clone();
        tokio::spawn(async move {
            let _permit = permit;
            read_lines(stream, &auditor).await;
        });
    }
}

/// Reads lines from `stream` into the recorder until it ends. A line over [`MAX_LINE`] ends the connection (Caddy
/// connects again), so a sender cannot make this hold more than one line's worth of memory.
pub async fn read_lines<R: tokio::io::AsyncRead + Unpin>(stream: R, auditor: &Auditor) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = match (&mut reader).take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if n > MAX_LINE {
            auditor.access_refused.fetch_add(1, Ordering::Relaxed);
            tracing::warn!("an access line over {MAX_LINE} bytes ended its connection");
            return;
        }
        match parse_line(&buf) {
            Some(line) => {
                auditor.recorder.access_log(line);
            }
            None if buf.iter().all(u8::is_ascii_whitespace) => {}
            None => {
                auditor.access_refused.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"{"level":"info","ts":1731312000.789,"logger":"http.log.access.log0","msg":"handled request","request":{"remote_ip":"198.51.100.4","remote_port":"5000","client_ip":"203.0.113.7","proto":"HTTP/2.0","method":"post","host":"askbar.example","uri":"/api/client/v1/pair?code=SECRET-CODE#frag","headers":{"Authorization":["Bearer SECRET-TOKEN"],"User-Agent":["Askbar/1.0\u0007"],"Cookie":["askbar_session=SECRET-COOKIE"]}},"bytes_read":10,"user_id":"","duration":0.0123,"size":512,"status":201,"resp_headers":{"Set-Cookie":["x=SECRET-SET"]}}"#;

    #[test]
    fn a_caddy_line_keeps_the_short_list_of_fields_and_nothing_that_could_hold_a_secret() {
        let l = parse_line(LINE.as_bytes()).unwrap();
        assert_eq!(l.at, 1731312000);
        assert_eq!((l.method.as_str(), l.host.as_str(), l.path.as_str()), ("POST", "askbar.example", "/api/client/v1/pair"));
        assert_eq!((l.status, l.bytes, l.duration_ms), (201, 512, 12));
        assert_eq!((l.remote_ip.as_str(), l.proto.as_str()), ("203.0.113.7", "HTTP/2.0"), "the client address, not the proxy's");
        assert_eq!(l.user_agent.as_deref(), Some("Askbar/1.0"), "control characters are dropped");
        let all = format!("{l:?}");
        for secret in ["SECRET-CODE", "SECRET-TOKEN", "SECRET-COOKIE", "SECRET-SET", "frag"] {
            assert!(!all.contains(secret), "{secret} was kept");
        }
    }

    #[test]
    fn what_is_not_a_caddy_access_line_is_refused() {
        for bad in [
            "",
            "not json",
            r#"{"msg":"a different log line"}"#,
            r#"{"ts":1.0,"request":{"method":"GET","uri":"/"},"status":99}"#,
            r#"{"ts":1.0,"request":{"method":"GE T","uri":"/"},"status":200}"#,
            r#"{"request":{"method":"GET","uri":"/"},"status":200}"#,
        ] {
            assert!(parse_line(bad.as_bytes()).is_none(), "{bad}");
        }
        // Long values are cut, not refused.
        let long = format!(
            r#"{{"ts":1.0,"request":{{"method":"GET","host":"h","uri":"/{}","headers":{{"User-Agent":["{}"]}}}},"status":200}}"#,
            "a".repeat(5000),
            "u".repeat(900)
        );
        let l = parse_line(long.as_bytes()).unwrap();
        assert_eq!((l.path.len(), l.user_agent.unwrap().len()), (2048, 200));
    }
}
