//! The listeners. One per source (a request head: `CONNECT host:port` or an absolute-form `http://`) and one relay port
//! per source that has a `relay` entry (a plain tunnel to that entry). Order: parse the head, [`decide`], take a tunnel
//! slot, resolve once, check every address, connect to that exact address, answer, splice.
//! Logs carry the source, the host as validated, the verdict and a reason code; never a URL, path, port or header.

use crate::policy::{check_addrs, decide, Verdict};
use crate::Proxy;
use askbar_proto::egress::{normalize_host, parse_authority, Entry, Host, Limits, SourceRules, DRAIN_MILLIS, REASON_HEADER};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The source's own listener: an HTTP request head first.
    Http,
    /// The source's relay port: no head, a tunnel to the `relay` entry.
    Relay,
}

/// Headers a plain request does not carry on: the proxy sets `Host` and `Connection: close` itself.
const DROPPED: [&str; 6] = ["host", "connection", "proxy-connection", "proxy-authorization", "keep-alive", "upgrade"];

struct Req {
    authority: String,
    /// The request as sent upstream (origin-form); empty for `CONNECT`.
    rewritten: Vec<u8>,
}

fn label(h: &Host) -> String {
    match h {
        Host::Name(n) => n.clone(),
        Host::Ip(ip) => ip.to_string(),
    }
}

/// A NUL, a bare LF or a bare CR anywhere in what has been read.
fn malformed(b: &[u8]) -> bool {
    b.iter().enumerate().any(|(i, &c)| match c {
        0 => true,
        b'\n' => i == 0 || b[i - 1] != b'\r',
        b'\r' => b.get(i + 1).is_some_and(|&n| n != b'\n'),
        _ => false,
    })
}

async fn read_head(c: &mut TcpStream, lim: &Limits) -> Option<(Vec<u8>, Vec<u8>)> {
    let read = async {
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 2048]);
        loop {
            let n = c.read(&mut chunk).await.ok().filter(|&n| n > 0)?;
            buf.extend_from_slice(&chunk[..n]);
            if malformed(&buf) {
                return None;
            }
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let rest = buf.split_off(i + 4);
                return (buf.len() <= lim.head_bytes as usize).then_some((buf, rest));
            }
            if buf.len() > lim.head_bytes as usize {
                return None;
            }
        }
    };
    timeout(Duration::from_secs(lim.head_read_secs.into()), read).await.ok().flatten()
}

/// `None` is a bad request (`400`): not `METHOD SP target SP HTTP/1.x`, too many or malformed header lines, a `CONNECT` with a
/// path, or a method other than `CONNECT` whose target is not `http://`.
fn parse_head(head: &[u8], lim: &Limits) -> Option<Req> {
    let mut lines = std::str::from_utf8(head).ok()?.strip_suffix("\r\n\r\n")?.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target, version) = (first.next()?, first.next()?, first.next()?);
    let headers: Vec<&str> = lines.collect();
    let ok_header = |h: &&str| h.split_once(':').is_some_and(|(n, _)| !n.is_empty() && n.bytes().all(|b| b > b' ' && b != 0x7f));
    if first.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") || headers.len() > lim.header_lines as usize {
        return None;
    }
    if !headers.iter().all(ok_header) {
        return None;
    }
    if method == "CONNECT" {
        return (!target.contains(['/', '?', '#'])).then(|| Req { authority: target.into(), rewritten: Vec::new() });
    }
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_uppercase()) || target.contains('#') {
        return None;
    }
    let rest = target.get(..7).filter(|s| s.eq_ignore_ascii_case("http://")).map(|_| &target[7..])?;
    let (authority, tail) = rest.split_at(rest.find(['/', '?']).unwrap_or(rest.len()));
    let path = if tail.starts_with('/') { tail.to_string() } else { format!("/{tail}") };
    let mut out = format!("{method} {path} HTTP/1.1\r\nHost: {authority}\r\n");
    for h in headers.iter().filter(|h| !DROPPED.contains(&h.split_once(':').map_or("", |(n, _)| n).to_ascii_lowercase().as_str())) {
        out.push_str(h);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    Some(Req { authority: authority.into(), rewritten: out.into_bytes() })
}

/// A fixed answer with no body, then close. A `403` names its reason in the header. What the client still sends is read and
/// dropped for a moment, so closing does not reset the connection before it has read the answer.
async fn reply(c: &mut TcpStream, status: u16, reason: Option<&str>) {
    let text = match status {
        400 => "Bad Request",
        403 => "Forbidden",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Gateway Timeout",
    };
    let header = reason.filter(|_| status == 403).map(|r| format!("{REASON_HEADER}: {r}\r\n")).unwrap_or_default();
    let _ = c.write_all(format!("HTTP/1.1 {status} {text}\r\n{header}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await;
    let _ = c.shutdown().await;
    let mut sink = [0u8; 1024];
    let _ = timeout(Duration::from_millis(DRAIN_MILLIS), async { while c.read(&mut sink).await.is_ok_and(|n| n > 0) {} }).await;
}

struct Clock {
    start: Instant,
    last_ms: AtomicU64,
}

impl Clock {
    fn touch(&self) {
        self.last_ms.store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }
    fn quiet(&self) -> Duration {
        self.start.elapsed().saturating_sub(Duration::from_millis(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// Copy one direction until EOF, an error, or `idle` with no bytes in either direction.
async fn pump(mut r: OwnedReadHalf, mut w: OwnedWriteHalf, idle: Duration, clock: &Clock) {
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        match timeout(idle, r.read(&mut buf)).await {
            Ok(Ok(n)) if n > 0 => {
                clock.touch();
                if !matches!(timeout(idle, w.write_all(&buf[..n])).await, Ok(Ok(()))) {
                    break;
                }
            }
            Ok(_) => break,
            Err(_) if clock.quiet() >= idle => break,
            Err(_) => {}
        }
    }
    let _ = w.shutdown().await;
}

async fn splice(a: TcpStream, b: TcpStream, idle: Duration) {
    let (clock, (ar, aw), (br, bw)) = (Clock { start: Instant::now(), last_ms: AtomicU64::new(0) }, a.into_split(), b.into_split());
    tokio::join!(pump(ar, bw, idle, &clock), pump(br, aw, idle, &clock));
}

impl Proxy {
    /// Accept on `listener` for `source` until the task is dropped.
    pub async fn serve(self: Arc<Self>, listener: TcpListener, source: String, mode: Mode) {
        loop {
            match listener.accept().await {
                Ok((c, _)) => {
                    let (p, s) = (self.clone(), source.clone());
                    tokio::spawn(async move { p.handle(c, &s, mode).await });
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    async fn handle(self: &Arc<Self>, mut c: TcpStream, source: &str, mode: Mode) {
        let cfg = self.config();
        let (lim, empty) = (cfg.limits, SourceRules::default());
        let rules = cfg.sources.get(source).unwrap_or(&empty);
        let (host, port, first) = match mode {
            Mode::Http => {
                let req = match read_head(&mut c, &lim).await {
                    Some((head, rest)) => parse_head(&head, &lim).map(|r| (r, rest)),
                    None => None,
                };
                let Some((req, rest)) = req else {
                    self.note_denied(source, "invalid", None, "bad_request");
                    return reply(&mut c, 400, None).await;
                };
                let plain = !req.rewritten.is_empty();
                let (host, port) = match parse_authority(&req.authority, if plain { 80 } else { 443 }) {
                    Ok(x) => x,
                    Err(why) => {
                        self.note_denied(source, "invalid", None, why.code());
                        return reply(&mut c, 403, Some(why.code())).await;
                    }
                };
                let entry = match decide(rules, &host, port, plain) {
                    Verdict::Allow(e) => e,
                    Verdict::Deny(why) => {
                        self.note_denied(source, &label(&host), Some(port), why.code());
                        return reply(&mut c, 403, Some(why.code())).await;
                    }
                };
                self.connect(c, source, entry, host, port, [req.rewritten, rest].concat(), !plain, &lim).await;
                return;
            }
            Mode::Relay => {
                let target =
                    rules.entries.iter().filter(|e| e.relay && !e.any_public).find_map(|e| Some((e, normalize_host(&e.host).ok()?)));
                let Some((entry, host)) = target else {
                    return self.note_denied(source, "invalid", None, "not_listed");
                };
                (host, entry.port, Vec::new())
            }
        };
        match decide(rules, &host, port, false) {
            Verdict::Allow(e) => self.connect(c, source, e, host, port, first, false, &lim).await,
            Verdict::Deny(why) => self.note_denied(source, &label(&host), Some(port), why.code()),
        }
    }

    /// Slot, one resolution, the address check, one connection, the answer, the splice.
    #[allow(clippy::too_many_arguments)]
    async fn connect(
        self: &Arc<Self>,
        mut c: TcpStream,
        source: &str,
        entry: &Entry,
        host: Host,
        port: u16,
        first: Vec<u8>,
        says_200: bool,
        lim: &Limits,
    ) {
        let name = label(&host);
        let Some(_slot) = self.slot(source, lim) else {
            self.note_denied(source, &name, Some(port), "busy");
            return reply(&mut c, 503, None).await;
        };
        let dial = async {
            let addrs: Vec<SocketAddr> = match &host {
                Host::Ip(ip) => vec![SocketAddr::new(*ip, port)],
                Host::Name(n) => {
                    let found = (self.resolver)(n.clone(), port).await.map_err(|_| (502, "unreachable"))?;
                    let ips: Vec<_> = found.iter().map(|a| a.ip()).collect();
                    check_addrs(entry, &ips).map_err(|why| (403, why.code()))?;
                    ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect()
                }
            };
            for a in addrs {
                if let Ok(s) = (self.connector)(a).await {
                    return Ok(s);
                }
            }
            Err((502, "unreachable"))
        };
        let up = match timeout(Duration::from_secs(lim.connect_secs.into()), dial).await {
            Ok(Ok(s)) => s,
            Ok(Err((status, why))) => {
                self.note_denied(source, &name, Some(port), why);
                return reply(&mut c, status, Some(why)).await;
            }
            Err(_) => {
                self.note_denied(source, &name, Some(port), "timeout");
                return reply(&mut c, 504, None).await;
            }
        };
        self.note_allowed(source, &name);
        let mut up = up;
        let started = async {
            if says_200 {
                c.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
            }
            up.write_all(&first).await
        };
        if started.await.is_ok() {
            splice(c, up, Duration::from_secs(lim.idle_secs.into())).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_request_becomes_origin_form_with_close() {
        let lim = Limits::default();
        let head =
            b"GET http://Ntfy.lan:80/a/b?x=1 HTTP/1.1\r\nHost: evil\r\nProxy-Authorization: x\r\nConnection: keep-alive\r\nX-A: 1\r\n\r\n";
        let r = parse_head(head, &lim).unwrap();
        assert_eq!(r.authority, "Ntfy.lan:80");
        assert_eq!(
            String::from_utf8(r.rewritten).unwrap(),
            "GET /a/b?x=1 HTTP/1.1\r\nHost: Ntfy.lan:80\r\nX-A: 1\r\nConnection: close\r\n\r\n"
        );
        assert!(parse_head(b"GET http://h?q HTTP/1.1\r\n\r\n", &lim).unwrap().rewritten.starts_with(b"GET /?q HTTP/1.1"));
    }
}
