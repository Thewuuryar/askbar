use askbar_auditor::{disk, Auditor, Limits, DEFAULT_PORT};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

fn die(e: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {e}");
    std::process::exit(code)
}

/// Resolves `raw` (`host:port`, a name or an address) to exactly one socket address.
fn resolve_one(var: &str, raw: &str) -> SocketAddr {
    use std::net::ToSocketAddrs;
    let found: Vec<SocketAddr> = raw
        .to_socket_addrs()
        .unwrap_or_else(|e| die(format!("{var} has an address this service cannot resolve: {raw} ({e})"), 1))
        .collect();
    match found.as_slice() {
        [one] => *one,
        [] => die(format!("{var}: {raw} resolved to no address"), 1),
        many => die(format!("{var}: {raw} must resolve to one address, but gives {}", many.len()), 1),
    }
}

/// Where the token API listens. `ASKBAR_AUDIT_API_ADDRS` is a comma-separated list of `host:port`, each resolved to
/// the address its alias has on one network, so nothing on `audit-caddy` can reach it (ADR 0051). Without it,
/// `ASKBAR_AUDIT_API_ADDR`, default `0.0.0.0:8083` (a development run, with no networks to keep apart).
fn api_addrs() -> Vec<SocketAddr> {
    if let Some(list) = std::env::var("ASKBAR_AUDIT_API_ADDRS").ok().filter(|v| !v.trim().is_empty()) {
        return list.split(',').map(|raw| resolve_one("ASKBAR_AUDIT_API_ADDRS", raw.trim())).collect();
    }
    let raw = std::env::var("ASKBAR_AUDIT_API_ADDR").unwrap_or_else(|_| format!("0.0.0.0:{DEFAULT_PORT}"));
    vec![raw
        .parse()
        .unwrap_or_else(|_| die(format!("ASKBAR_AUDIT_API_ADDR is not a socket address: {raw} (example: 0.0.0.0:{DEFAULT_PORT})"), 1))]
}

/// `ASKBAR_AUDIT_INGEST_ADDR`: where Caddy's access lines are taken (`host:port`, one address). Unset: no ingest.
fn ingest_addr() -> Option<SocketAddr> {
    std::env::var("ASKBAR_AUDIT_INGEST_ADDR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|raw| resolve_one("ASKBAR_AUDIT_INGEST_ADDR", raw.trim()))
}

fn audit_dir() -> PathBuf {
    std::env::var("ASKBAR_AUDIT_DIR").ok().filter(|p| !p.trim().is_empty()).map_or_else(|| "/audit".into(), PathBuf::from)
}

/// `askbar-auditor healthcheck`: GET the health route locally; exit 0 on a 200.
fn healthy() -> bool {
    use std::io::{Read, Write};
    let first = api_addrs()[0];
    // A wildcard address is reached on the loopback; an address of one network is reached on itself.
    let target = if first.ip().is_unspecified() { SocketAddr::from(([127, 0, 0, 1], first.port())) } else { first };
    let timeout = Duration::from_secs(2);
    (|| -> std::io::Result<bool> {
        let mut s = std::net::TcpStream::connect_timeout(&target, timeout)?;
        s.set_read_timeout(Some(timeout))?;
        s.write_all(b"GET /api/audit/v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    })()
    .unwrap_or(false)
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthy() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let dir = audit_dir();
    let db = askbar_audit::AuditDb::open(&dir.join("audit.db")).unwrap_or_else(|e| die(e, 1));
    let limits = Limits::default();
    let recorder = askbar_audit::Recorder::spawn(db, limits.max_queued);
    let auditor = Auditor::new(recorder, limits, disk::free_space_of(&dir, Duration::from_secs(2)));
    // The rules that send alerts (plan step 4.2, ADR 0055), over the link to the alerter. Without its token nothing is sent.
    let alerts = askbar_auditor::alert_link::client(&askbar_auditor::token_dir());
    tokio::spawn(alerts.clone().drain_forever());
    let auditor = auditor.with_alerts(alerts);
    tokio::spawn(askbar_auditor::rules_every_30_seconds(auditor.clone()));
    // Docker does not restart an unhealthy container; the watchdog does, by exiting (ADR 0005).
    askbar_service::restart::start_watchdog(healthy);
    tokio::spawn(askbar_auditor::retention_daily(auditor.clone()));
    let token_dir = askbar_auditor::token_dir();
    if !token_dir.join("accept").is_dir() {
        die(
            format!(
                "{} is missing: bootstrap makes it. Run `docker compose up -d`, which runs bootstrap first",
                token_dir.join("accept").display()
            ),
            1,
        );
    }
    // Caddy's access lines, on a listener of their own that nothing else shares (ADR 0051).
    if let Some(addr) = ingest_addr() {
        let listener =
            tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(format!("cannot listen on {addr} for access lines: {e}"), 1));
        tracing::info!(%addr, "the auditor takes Caddy's access lines");
        tokio::spawn(askbar_auditor::ingest::serve(listener, auditor.clone()));
    }
    let app = askbar_auditor::router(auditor, askbar_auditor::links(&token_dir));
    let mut servers = tokio::task::JoinSet::new();
    for addr in api_addrs() {
        let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(format!("cannot listen on {addr}: {e}"), 1));
        tracing::info!(%addr, "the auditor is listening");
        let app = app.clone();
        servers.spawn(async move { axum::serve(listener, app).with_graceful_shutdown(stop_signal()).await });
    }
    while let Some(done) = servers.join_next().await {
        if let Err(e) = done.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string())) {
            die(e, 1);
        }
    }
}

async fn stop_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let Ok(mut term) = signal(SignalKind::terminate()) else { return };
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}
