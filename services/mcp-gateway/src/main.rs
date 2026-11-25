use askbar_mcp_gateway::{admin_links, admin_router, server_links, server_router, token_dir, Gateway};
use askbar_service::audit_client::AuditClient;
use askbar_service::boot::{self, die, listen_addr};
use askbar_service::status::BootId;
use std::net::SocketAddr;

/// `askbar-mcp-gateway healthcheck`: GET the health route on the admin's listener; exit 0 on a 200.
fn healthy() -> bool {
    std::env::var("ASKBAR_MCP_GATEWAY_ADMIN_ADDR").is_ok_and(|addr| boot::healthy(addr, "/api/mcp-gateway/v1/health"))
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthy() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let server_addr = listen_addr("ASKBAR_MCP_GATEWAY_SERVER_ADDR", "mcp-gateway-server-host:8087");
    let admin_addr = listen_addr("ASKBAR_MCP_GATEWAY_ADMIN_ADDR", "mcp-gateway-admin-host:8087");
    // An address from the environment, never a secret.
    let searxng = std::env::var("ASKBAR_SEARXNG_ADDR").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let boot_id = BootId::new();
    // The auditor's address and the link token, as the other services have them; no auditor, no calls (fail closed).
    let audit_url =
        std::env::var("ASKBAR_AUDIT_API_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| "http://auditor:8083".into());
    let audit = AuditClient::new(&audit_url, token_dir(), "mcp-gateway-auditor");
    tokio::spawn(audit.clone().hello_until_done(boot_id.as_str().to_string()));
    tokio::spawn(audit.clone().drain_forever());
    tokio::spawn(audit.clone().status_every_minute());
    let gateway = Gateway::new(boot_id, searxng, Some(audit));
    askbar_service::restart::start_watchdog(healthy);
    let dir = token_dir();
    let server_app = server_router(gateway.clone(), server_links(&dir));
    let admin_app = admin_router(gateway, admin_links(&dir));
    let serve = |addr: SocketAddr, app: axum::Router, what: &'static str| async move {
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        tracing::info!(%addr, what, "the mcp-gateway is listening");
        axum::serve(listener, app).with_graceful_shutdown(boot::stop_signal()).await.map_err(|e| e.to_string())
    };
    if let Err(e) = tokio::try_join!(serve(server_addr, server_app, "for the server"), serve(admin_addr, admin_app, "for the admin")) {
        die(e, 1);
    }
}
