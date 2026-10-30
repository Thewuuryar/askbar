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
    use std::io::{Read, Write};
    let port = std::env::var("ASKBAR_API_ADDR")
        .ok()
        .and_then(|a| a.parse::<SocketAddr>().ok())
        .map_or(8080, |a| a.port());
    let ok = (|| -> std::io::Result<bool> {
        let timeout = std::time::Duration::from_secs(2);
        let mut s = std::net::TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), timeout)?;
        s.set_read_timeout(Some(timeout))?;
        s.write_all(b"GET /api/v1/health HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    })();
    std::process::exit(if ok.unwrap_or(false) { 0 } else { 1 })
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("healthcheck") {
        run_healthcheck();
    }
    tracing_subscriber::fmt().with_target(false).init();
    let (api, mgmt) = match (addr_from_env("ASKBAR_API_ADDR"), addr_from_env("ASKBAR_MGMT_ADDR")) {
        (Ok(a), Ok(m)) => (a, m),
        (a, m) => {
            for e in [a.err(), m.err()].into_iter().flatten() {
                eprintln!("error: {e}");
            }
            std::process::exit(2);
        }
    };
    // The management API is for the admin service only: one specific address, never a wildcard.
    if let Err(e) = askbar_server::mgmt::check_not_wildcard(&mgmt) {
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
    let state = askbar_server::AppState::new(db, secrets, askbar_server::MIN_CLIENT);
    let mgmt_app = askbar_server::mgmt::router(state.clone(), askbar_server::mgmt::token_hash_path());
    let result = tokio::try_join!(serve(api, askbar_server::api_router(state)), serve(mgmt, mgmt_app));
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
