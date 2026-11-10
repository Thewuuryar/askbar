use askbar_auditor::{disk, Auditor, Limits, DEFAULT_PORT};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

fn die(e: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {e}");
    std::process::exit(code)
}

/// `ASKBAR_AUDIT_API_ADDR`, default `0.0.0.0:8083`: every interface, which is safe because the auditor joins only
/// `audit-*` networks, each shared with one caller.
fn listen_addr() -> SocketAddr {
    let raw = std::env::var("ASKBAR_AUDIT_API_ADDR").unwrap_or_else(|_| format!("0.0.0.0:{DEFAULT_PORT}"));
    raw.parse()
        .unwrap_or_else(|_| die(format!("ASKBAR_AUDIT_API_ADDR is not a socket address: {raw} (example: 0.0.0.0:{DEFAULT_PORT})"), 1))
}

fn audit_dir() -> PathBuf {
    std::env::var("ASKBAR_AUDIT_DIR").ok().filter(|p| !p.trim().is_empty()).map_or_else(|| "/audit".into(), PathBuf::from)
}

/// `askbar-auditor healthcheck`: GET the health route locally; exit 0 on a 200.
fn healthy() -> bool {
    use std::io::{Read, Write};
    let port = listen_addr().port();
    let timeout = Duration::from_secs(2);
    (|| -> std::io::Result<bool> {
        let mut s = std::net::TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
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
    let app = askbar_auditor::router(auditor, askbar_auditor::links(&token_dir));
    let addr = listen_addr();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(format!("cannot listen on {addr}: {e}"), 1));
    tracing::info!(%addr, "the auditor is listening");
    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(stop_signal()).await {
        die(e, 1);
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
