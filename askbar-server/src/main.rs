use std::net::SocketAddr;

fn addr_from_env(var: &str) -> Result<SocketAddr, String> {
    let v = std::env::var(var).map_err(|_| format!("{var} is not set (example: 0.0.0.0:8080)"))?;
    v.parse().map_err(|_| format!("{var} is not a valid socket address: {v}"))
}

async fn serve(addr: SocketAddr, app: axum::Router) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, app).await
}

fn data_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("ASKBAR_DATA_DIR").unwrap_or_else(|_| "data".into()))
}

/// Where `audit.db` lives: `ASKBAR_AUDIT_DIR`, else the data directory. In the compose setup this
/// is a volume only the server mounts.
fn audit_dir() -> std::path::PathBuf {
    std::env::var("ASKBAR_AUDIT_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| data_dir())
}

fn die(e: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {e}");
    std::process::exit(code)
}

/// `askbar-server healthcheck`: GET the local API health endpoint; exit 0 if it answers 200.
fn run_healthcheck() -> ! {
    use std::io::{Read, Write};
    let port = std::env::var("ASKBAR_CLIENT_API_ADDR")
        .ok()
        .and_then(|a| a.parse::<SocketAddr>().ok())
        .map_or(8080, |a| a.port());
    let ok = (|| -> std::io::Result<bool> {
        let timeout = std::time::Duration::from_secs(2);
        let mut s = std::net::TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
        s.set_read_timeout(Some(timeout))?;
        s.write_all(b"GET /api/client/v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    })();
    std::process::exit(if ok.unwrap_or(false) { 0 } else { 1 })
}

/// On SIGTERM or Ctrl-C: keep what running replies have so far, then exit.
async fn stop_on_signal(state: std::sync::Arc<askbar_server::AppState>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let Ok(mut term) = signal(SignalKind::terminate()) else { return };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("stopping: saving running replies");
    askbar_server::generation::shutdown(&state);
    std::process::exit(0);
}

/// Once a day (and shortly after start-up): delete audit records older than `audit_retention_days`
/// from the snapshot. The setting is read each time, so a change applies the next day.
async fn audit_retention_daily(state: std::sync::Arc<askbar_server::AppState>) {
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(24 * 3600));
    loop {
        tick.tick().await;
        let days = {
            let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
            askbar_server::snapshot::current(&db).ok().flatten().map(|s| s.audit_retention_days).unwrap_or(30)
        };
        match state.audit.prune(days, askbar_server::auth::now()).await {
            Ok((0, 0)) => {}
            Ok((spans, payloads)) => tracing::info!(spans, payloads, days, "removed audit records past retention"),
            Err(e) => tracing::warn!("audit retention failed: {e}"),
        }
    }
}

/// Finished generation rows are only kept for a day; the messages they produced stay.
async fn prune_generations_hourly(state: std::sync::Arc<askbar_server::AppState>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        tick.tick().await;
        let cutoff = askbar_server::auth::now() - 24 * 3600;
        let pruned = state.db.lock().unwrap_or_else(|e| e.into_inner()).prune_generations(cutoff);
        match pruned {
            Ok(0) => {}
            Ok(n) => tracing::info!(pruned = n, "removed old generation rows"),
            Err(e) => tracing::warn!("could not prune generation rows: {e}"),
        }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("healthcheck") {
        run_healthcheck();
    }
    tracing_subscriber::fmt().with_target(false).init();
    let (api, server_api) = match (addr_from_env("ASKBAR_CLIENT_API_ADDR"), addr_from_env("ASKBAR_SERVER_API_ADDR")) {
        (Ok(a), Ok(m)) => (a, m),
        (a, m) => {
            for e in [a.err(), m.err()].into_iter().flatten() {
                eprintln!("error: {e}");
            }
            std::process::exit(2);
        }
    };
    // The server API is for the admin service only: one specific address, never a wildcard.
    if let Err(e) = askbar_server::server_api::check_not_wildcard(&server_api) {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
    let key_path = askbar_server::secrets::key_path();
    let db = askbar_server::Db::open(&data_dir().join("askbar.db")).unwrap_or_else(|e| die(e, 1));
    let secrets = askbar_server::secrets::ensure_key_file(&key_path, &db).unwrap_or_else(|e| die(e, 2));
    match db.seed_server() {
        Ok(user) => tracing::info!(%user, "database ready"),
        Err(e) => die(e, 1),
    }
    match askbar_server::snapshot::current(&db) {
        Ok(Some(s)) => tracing::info!(version = s.version, providers = s.providers.len(), "configuration snapshot loaded"),
        Ok(None) => tracing::warn!("no configuration yet: the admin service has not pushed one"),
        Err(e) => die(format!("could not read the configuration snapshot: {e}"), 1),
    }
    // Replies that were running when the last process stopped did not finish.
    match askbar_server::generation::sweep_on_start(&db) {
        Ok((0, 0)) => {}
        Ok((running, queued)) => tracing::warn!(running, queued, "marked generations from the previous run as interrupted"),
        Err(e) => die(format!("could not clean up generations: {e}"), 1),
    }
    let audit_db = askbar_server::audit::AuditDb::open(&audit_dir().join("audit.db")).unwrap_or_else(|e| die(e, 1));
    match audit_db.sweep_running() {
        Ok(0) => {}
        Ok(n) => tracing::warn!(spans = n, "marked audit spans from the previous run as interrupted"),
        Err(e) => die(format!("could not clean up the audit store: {e}"), 1),
    }
    let state = askbar_server::AppState::new(db, secrets, askbar_server::MIN_CLIENT)
        .with_audit(askbar_server::audit::Recorder::spawn(audit_db, 4096));
    tokio::spawn(stop_on_signal(state.clone()));
    tokio::spawn(audit_retention_daily(state.clone()));
    tokio::spawn(prune_generations_hourly(state.clone()));
    let server_api_app = askbar_server::server_api::router(state.clone(), askbar_server::server_api::token_hash_path());
    let result = tokio::try_join!(serve(api, askbar_server::client_router(state)), serve(server_api, server_api_app));
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
