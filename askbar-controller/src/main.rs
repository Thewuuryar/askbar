use std::sync::Arc;

fn die(msg: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(code)
}

/// `askbar-controller healthcheck`: GET the health endpoint on the controller's own address.
fn healthcheck() -> bool {
    use std::io::{Read, Write};
    let Some(addr) = std::env::var("ASKBAR_CONTROL_ADDR").ok().and_then(|a| a.parse::<std::net::SocketAddr>().ok()) else { return false };
    let attempt = || -> std::io::Result<bool> {
        let t = std::time::Duration::from_secs(2);
        let mut s = std::net::TcpStream::connect_timeout(&addr, t)?;
        s.set_read_timeout(Some(t))?;
        s.write_all(b"GET /api/control/v1/health HTTP/1.0\r\nHost: controller\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    };
    attempt().unwrap_or(false)
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthcheck() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let addr = askbar_controller::parse_addr(
        &std::env::var("ASKBAR_CONTROL_ADDR").unwrap_or_else(|_| die("ASKBAR_CONTROL_ADDR is not set (example: 172.32.0.2:8083)", 1)),
    )
    .unwrap_or_else(|e| die(e, 1));
    let docker = askbar_controller::docker::Docker::new(askbar_controller::socket_path());
    // Only this Compose project's containers are ever touched: the project this container belongs to, or the one named.
    let project = match std::env::var("ASKBAR_COMPOSE_PROJECT").ok().filter(|p| !p.trim().is_empty()) {
        Some(p) => p,
        None => {
            let me = std::env::var("HOSTNAME").unwrap_or_default();
            docker.project_of(&me).await.unwrap_or_else(|e| die(format!("could not find this container's Compose project: {e}"), 1))
        }
    };
    tracing::info!(%project, "controlling");
    let cfg = Arc::new(askbar_controller::Config { docker, project, token_hash_file: askbar_controller::token_hash_path() });
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(e, 1));
    tracing::info!(%addr, "listening");
    if let Err(e) = axum::serve(listener, askbar_controller::router(cfg)).await {
        die(e, 1);
    }
}
