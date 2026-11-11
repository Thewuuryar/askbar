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

fn die(e: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {e}");
    std::process::exit(code)
}

/// `askbar-server healthcheck`: GET the local API health endpoint; exit 0 if it answers 200.
fn run_healthcheck() -> ! {
    std::process::exit(if healthy() { 0 } else { 1 })
}

/// True if the local client API answers its health endpoint with 200 within 2 seconds.
fn healthy() -> bool {
    use std::io::{Read, Write};
    let port = std::env::var("ASKBAR_CLIENT_API_ADDR").ok().and_then(|a| a.parse::<SocketAddr>().ok()).map_or(8080, |a| a.port());
    let ok = (|| -> std::io::Result<bool> {
        let timeout = std::time::Duration::from_secs(2);
        let mut s = std::net::TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
        s.set_read_timeout(Some(timeout))?;
        s.write_all(b"GET /api/client/v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    })();
    ok.unwrap_or(false)
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

/// Finished generation rows are only kept for a day; the messages they produced stay.
async fn prune_generations_hourly(state: std::sync::Arc<askbar_server::AppState>) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
    loop {
        tick.tick().await;
        let cutoff = askbar_server::auth::now() - 24 * 3600;
        for space in state.spaces.opened() {
            match space.lock().prune_generations(cutoff) {
                Ok(0) => {}
                Ok(n) => tracing::info!(space = %space.id, pruned = n, "removed old generation rows"),
                Err(e) => tracing::warn!(space = %space.id, "could not prune generation rows: {e}"),
            }
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
    let server_api_addr = std::env::var("ASKBAR_SERVER_API_ADDR")
        .map_err(|_| "ASKBAR_SERVER_API_ADDR is not set (example: server-api-host:8082)".to_string())
        .and_then(|raw| askbar_server::server_api::parse_addr(&raw));
    let (api, server_api) = match (addr_from_env("ASKBAR_CLIENT_API_ADDR"), server_api_addr) {
        (Ok(a), Ok(m)) => (a, m),
        (a, m) => {
            for e in [a.err(), m.err()].into_iter().flatten() {
                eprintln!("error: {e}");
            }
            std::process::exit(2);
        }
    };
    // The server API is for the admin service only: `parse_addr` gives one specific address, never a wildcard.
    // The control database and one file per space (ADR 0009). The single `askbar.db` of earlier
    // versions is never opened, read or deleted: pre-1.0 storage changes wipe and re-pair (ADR 0018).
    let old = data_dir().join("askbar.db");
    if old.exists() {
        tracing::warn!(path = %old.display(), "this database is from an earlier version and is not used; it can be deleted");
    }
    let db = askbar_server::Db::open(&data_dir().join("control.db")).unwrap_or_else(|e| die(e, 1));
    // Users come from the admin's configuration (ADR 0008); the server no longer makes one of its own.
    tracing::info!("database ready");
    match askbar_server::snapshot::current(&db) {
        Ok(Some(s)) => tracing::info!(version = s.version, providers = s.providers.len(), "configuration snapshot loaded"),
        Ok(None) => tracing::warn!("no configuration yet: the admin service has not pushed one"),
        Err(e) => die(format!("could not read the configuration snapshot: {e}"), 1),
    }
    // Every space's file is opened (and migrated) now, so a bad one stops the start-up here.
    let spaces = askbar_server::Spaces::on_disk(data_dir().join("spaces"));
    let opened = spaces.open_all(&db).unwrap_or_else(|e| die(format!("could not open the space files: {e}"), 1));
    tracing::info!(spaces = opened.len(), "space files ready");
    // Replies that were running when the last process stopped did not finish.
    for space in &opened {
        match askbar_server::generation::sweep_on_start(&space.lock()) {
            Ok((0, 0)) => {}
            Ok((running, queued)) => {
                tracing::warn!(space = %space.id, running, queued, "marked generations from the previous run as interrupted")
            }
            Err(e) => die(format!("could not clean up generations in space {}: {e}", space.id), 1),
        }
    }
    // Records go to the auditor and replies come from the gateway, each over its own link (ADR 0010, ADR 0054). The
    // auditor ends the spans an earlier process left running when this one says its boot ID.
    let token_dir = std::env::var("ASKBAR_SERVER_TOKEN_DIR")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map_or_else(|| "/server-token".into(), std::path::PathBuf::from);
    let env_url =
        |var: &str, default: &str| std::env::var(var).ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| default.to_string());
    let audit_url = env_url("ASKBAR_AUDIT_API_URL", "http://auditor:8083");
    let audit_client = askbar_service::audit_client::AuditClient::new(&audit_url, token_dir.clone(), "server-auditor");
    let state = askbar_server::AppState::new(db, askbar_server::MIN_CLIENT);
    tokio::spawn(audit_client.clone().hello_until_done(state.boot_id.as_str().to_string()));
    tokio::spawn(audit_client.clone().drain_forever());
    let state = state.with_spaces(spaces).with_audit(askbar_server::audit::Recorder::new(audit_client)).with_gateway(
        askbar_server::gateway::GatewayClient::new(&env_url("ASKBAR_GATEWAY_URL", "http://gateway-server-host:8084"), token_dir),
    );
    tokio::spawn(stop_on_signal(state.clone()));
    // POST /api/server/v1/restart saves running replies, as a stop does, then exits (ADR 0005).
    let on_stop = state.clone();
    let _ = state
        .restarter
        .set(askbar_service::restart::Restarter::new(state.boot_id.clone(), move || askbar_server::generation::shutdown(&on_stop)));
    // Docker does not restart an unhealthy container; the watchdog does, by exiting (ADR 0005).
    askbar_service::restart::start_watchdog(healthy);
    tokio::spawn(prune_generations_hourly(state.clone()));
    let token_hash = askbar_server::server_api::token_hash_path();
    askbar_server::server_api::check_token_hash_file(&token_hash).unwrap_or_else(|e| die(e, 1));
    let server_api_app = askbar_server::server_api::router(state.clone(), token_hash);
    let result = tokio::try_join!(serve(api, askbar_server::client_router(state)), serve(server_api, server_api_app));
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
