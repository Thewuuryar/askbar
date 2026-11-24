//! What every service's `main` does the same way: exit on a fatal error, read a listener address, answer
//! `healthcheck`, and stop on SIGTERM or Ctrl-C.

use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

/// Prints `error: {e}` to stderr and exits with `code`.
pub fn die(e: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {e}");
    std::process::exit(code)
}

/// A listener address as a name and a port (`alerter-admin-host:8085`) from the environment variable `var`: the alias
/// exists only on that caller's network, so the listener is on that network alone. Never a wildcard, because the service
/// also sits on an outbound network (ADR 0054, ADR 0055).
pub fn listen_addr(var: &str, example: &str) -> SocketAddr {
    let raw = std::env::var(var).unwrap_or_else(|_| die(format!("{var} is not set (example: {example})"), 2));
    parse_listen_addr(&raw, var, example)
}

/// The rules of [`listen_addr`] for a value already read (`var` only names it in the error): one resolvable, non-wildcard address.
pub fn parse_listen_addr(raw: &str, var: &str, example: &str) -> SocketAddr {
    let mut addrs: Vec<SocketAddr> = match raw.trim().parse::<SocketAddr>() {
        Ok(a) => vec![a],
        Err(_) => raw
            .trim()
            .to_socket_addrs()
            .unwrap_or_else(|e| die(format!("{var} is not an address this service can resolve: {raw} ({e}; example: {example})"), 2))
            .collect(),
    };
    addrs.sort();
    addrs.dedup();
    match addrs.as_slice() {
        [a] if !a.ip().is_unspecified() => *a,
        [a] => die(format!("{var} is a wildcard address ({}): it must be the address on one network", a.ip()), 2),
        [] => die(format!("{var} resolved to no address: {raw}"), 2),
        many => die(format!("{var} must resolve to one address, but {raw} gives {} of them", many.len()), 2),
    }
}

/// `healthcheck`: GET `path` on `addr` (the first address it resolves to); true on a 200.
pub fn healthy(addr: impl ToSocketAddrs, path: &str) -> bool {
    use std::io::{Read, Write};
    let Some(addr) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else { return false };
    let timeout = Duration::from_secs(2);
    (|| -> std::io::Result<bool> {
        let mut s = std::net::TcpStream::connect_timeout(&addr, timeout)?;
        s.set_read_timeout(Some(timeout))?;
        s.write_all(format!("GET {path} HTTP/1.0\r\nHost: localhost\r\n\r\n").as_bytes())?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    })()
    .unwrap_or(false)
}

/// Resolves on SIGTERM or Ctrl-C, for graceful shutdown.
pub async fn stop_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut term) = signal(SignalKind::terminate()) else { return };
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
