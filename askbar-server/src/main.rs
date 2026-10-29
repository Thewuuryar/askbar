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

/// `askbar-server import --config <config.yaml> --db <history.db>`
fn run_import(args: &[String]) {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let (Some(config), Some(db)) = (flag("--config"), flag("--db")) else {
        die("usage: askbar-server import --config <config.yaml> --db <history.db>", 2)
    };
    let secrets = askbar_server::secrets::load_key_file(&askbar_server::secrets::key_path())
        .unwrap_or_else(|e| die(e, 2))
        .unwrap_or_else(|| die("no master key yet: finish setup on the admin page, or run `askbar-server generate-master-key`", 2));
    let server = askbar_core::Db::open(&data_dir().join("askbar.db")).unwrap_or_else(|e| die(e, 1));
    let report = askbar_server::import::import(&server, &secrets, config.as_ref(), db.as_ref())
        .unwrap_or_else(|e| die(e, 1));
    for w in &report.warnings {
        eprintln!("warning: {w}");
    }
    println!(
        "imported {} conversations, {} messages, {} providers",
        report.conversations, report.messages, report.providers
    );
}

/// `askbar-server create-pair-code [--name <device>] [--url <public api url>]`
fn run_create_pair_code(args: &[String]) {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
    let url = flag("--url")
        .cloned()
        .or_else(|| std::env::var("ASKBAR_PUBLIC_URL").ok())
        .unwrap_or_else(|| "http://localhost:8080".into());
    let name = flag("--name").map(String::as_str).unwrap_or("");
    let db = askbar_core::Db::open(&data_dir().join("askbar.db")).unwrap_or_else(|e| die(e, 1));
    let user = db.seed_server().unwrap_or_else(|e| die(e, 1));
    let code = askbar_server::auth::create_pair_code(&db, &user, name, askbar_server::auth::PAIR_CODE_TTL_SECS)
        .unwrap_or_else(|e| die(e, 1));
    println!("{}", askbar_proto::encode_pairing(url.trim_end_matches('/'), &code));
    eprintln!("This code works once and expires in 10 minutes.");
}

/// `askbar-server generate-master-key`: create the master key file if there is none.
fn run_generate_master_key() {
    let path = askbar_server::secrets::key_path();
    match askbar_server::secrets::generate_key_file(&path) {
        Ok(_) => println!("created {}", path.display()),
        Err(e) => die(e, 1),
    }
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
    if args.first().map(String::as_str) == Some("generate-master-key") {
        return run_generate_master_key();
    }
    if args.first().map(String::as_str) == Some("create-pair-code") {
        return run_create_pair_code(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("import") {
        return run_import(&args[1..]);
    }
    tracing_subscriber::fmt().with_target(false).init();
    let (api, admin) = match (addr_from_env("ASKBAR_API_ADDR"), addr_from_env("ASKBAR_ADMIN_ADDR")) {
        (Ok(a), Ok(b)) => (a, b),
        (a, b) => {
            for e in [a.err(), b.err()].into_iter().flatten() {
                eprintln!("error: {e}");
            }
            std::process::exit(2);
        }
    };
    let key_path = askbar_server::secrets::key_path();
    let db = askbar_core::Db::open(&data_dir().join("askbar.db")).unwrap_or_else(|e| die(e, 1));
    askbar_server::rotate::recover(&db, &key_path);
    let secrets = askbar_server::secrets::load_key_file(&key_path).unwrap_or_else(|e| die(e, 2));
    if secrets.is_none() {
        tracing::warn!(path = %key_path.display(), "no master key yet: the server is in setup mode (open the admin page)");
    }
    match db.seed_server() {
        Ok(user) => tracing::info!(%user, "database ready"),
        Err(e) => die(e, 1),
    }
    if db.conn().query_row("SELECT 1 FROM settings WHERE key = 'admin_password_hash'", [], |_| Ok(())).is_err() {
        tracing::warn!("no admin password yet: open the admin page to finish setup");
    }
    let public_url = std::env::var("ASKBAR_PUBLIC_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| format!("http://localhost:{}", api.port()));
    let state = askbar_server::AppState::build(db, secrets, askbar_server::MIN_CLIENT, &public_url, key_path);
    let result = tokio::try_join!(
        serve(api, askbar_server::api_router(state.clone())),
        serve(admin, askbar_server::admin_router(state))
    );
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
