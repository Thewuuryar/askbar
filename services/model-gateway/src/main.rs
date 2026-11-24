use askbar_model_gateway::{admin_links, admin_router, server_links, server_router, token_dir, Gateway};
use askbar_service::audit_client::AuditClient;
use askbar_service::boot::{self, die, listen_addr};
use askbar_service::status::BootId;
use std::net::SocketAddr;

/// `askbar-model-gateway healthcheck`: GET the health route on the server's listener; exit 0 on a 200.
fn healthy() -> bool {
    std::env::var("ASKBAR_GATEWAY_SERVER_ADDR").is_ok_and(|addr| boot::healthy(addr, "/api/model-gateway/v1/health"))
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthy() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let server_addr = listen_addr("ASKBAR_GATEWAY_SERVER_ADDR", "gateway-server-host:8084");
    let admin_addr = listen_addr("ASKBAR_GATEWAY_ADMIN_ADDR", "gateway-admin-host:8084");
    let dir = token_dir();
    if !dir.join("accept").is_dir() {
        die(
            format!(
                "{} is missing: bootstrap makes it. Run `docker compose up -d`, which runs bootstrap first",
                dir.join("accept").display()
            ),
            1,
        );
    }
    let audit_url =
        std::env::var("ASKBAR_AUDIT_API_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "http://auditor:8083".into());
    let audit = AuditClient::new(&audit_url, dir.clone(), "model-gateway-auditor");
    let boot_id = BootId::new();
    // Nothing is held that needs saving: a restart is how a key leaves memory.
    let restarter = askbar_service::restart::Restarter::new(boot_id.clone(), || {});
    let gw = Gateway::new(audit.clone(), boot_id.clone(), restarter);
    // The budget is held against what the auditor recorded, read with this gateway's own read-only token.
    gw.set_usage_reader(askbar_model_gateway::usage::Reader::new(&audit_url, dir.clone()));
    // Docker does not restart an unhealthy container; the watchdog does, by exiting (ADR 0005).
    askbar_service::restart::start_watchdog(healthy);
    tokio::spawn(audit.clone().hello_until_done(boot_id.as_str().to_string()));
    tokio::spawn(audit.clone().drain_forever());
    // Keeps the link from looking silent to the auditor while the gateway has nothing to record (ADR 0051).
    tokio::spawn(audit.clone().status_every_minute());
    tokio::spawn(gw.clone().watch_turns());
    tokio::spawn(gw.clone().watch_usage());
    let server_app = server_router(gw.clone(), server_links(&dir));
    let admin_app = admin_router(gw, admin_links(&dir));
    let serve = |addr: SocketAddr, app: axum::Router, what: &'static str| async move {
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        tracing::info!(%addr, what, "the gateway is listening");
        axum::serve(listener, app).with_graceful_shutdown(boot::stop_signal()).await.map_err(|e| e.to_string())
    };
    if let Err(e) = tokio::try_join!(serve(server_addr, server_app, "for the server"), serve(admin_addr, admin_app, "for the admin")) {
        die(e, 1);
    }
}
