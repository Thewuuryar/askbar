use askbar_mcp_gateway::{links, router, token_dir, Gateway};
use askbar_service::audit_client::AuditClient;
use askbar_service::boot::{self, die, listen_addr};
use askbar_service::status::BootId;

/// `askbar-mcp-gateway healthcheck`: GET the health route; exit 0 on a 200.
fn healthy() -> bool {
    std::env::var("ASKBAR_MCP_GATEWAY_ADDR").is_ok_and(|addr| boot::healthy(addr, "/api/mcp-gateway/v1/health"))
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthy() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let addr = listen_addr("ASKBAR_MCP_GATEWAY_ADDR", "mcp-gateway-host:8087");
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
    let app = router(gateway, links(&token_dir()));
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(format!("cannot listen on {addr}: {e}"), 1));
    tracing::info!(%addr, "the mcp-gateway is listening");
    if let Err(e) = axum::serve(listener, app).with_graceful_shutdown(boot::stop_signal()).await {
        die(e, 1);
    }
}
