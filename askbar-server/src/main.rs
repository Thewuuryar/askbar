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
    let secrets = askbar_server::secrets::Secrets::from_env().unwrap_or_else(|e| die(e, 2));
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

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
    if let Err(e) = askbar_server::secrets::Secrets::from_env() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
    match askbar_core::Db::open(&data_dir().join("askbar.db")).and_then(|db| db.seed_server()) {
        Ok(user) => tracing::info!(%user, "database ready"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
    let result = tokio::try_join!(
        serve(api, askbar_server::api_router()),
        serve(admin, askbar_server::admin_router())
    );
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
