use askbar_egress_proxy::proxy::Mode;
use askbar_egress_proxy::{admin_links, admin_router, split_pairs, system_connector, system_resolver, token_dir, Proxy};
use askbar_service::boot::{self, die, listen_addr, parse_listen_addr};
use askbar_service::status::BootId;
use std::net::SocketAddr;

/// `askbar-egress-proxy healthcheck`: GET the health route on the admin's listener; exit 0 on a 200.
fn healthy() -> bool {
    std::env::var("ASKBAR_EGRESS_PROXY_ADMIN_ADDR").is_ok_and(|addr| boot::healthy(addr, "/api/egress-proxy/v1/health"))
}

/// `name=address,...` from `var`: each address is a listener address (never a wildcard).
fn pairs(var: &str, example: &str, required: bool) -> Vec<(String, SocketAddr)> {
    let raw = std::env::var(var).unwrap_or_default();
    if raw.trim().is_empty() && required {
        die(format!("{var} is not set (example: {example})"), 2);
    }
    split_pairs(&raw)
        .unwrap_or_else(|e| die(format!("{var}: {e} (example: {example})"), 2))
        .into_iter()
        .map(|(name, addr)| (name, parse_listen_addr(&addr, var, example)))
        .collect()
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthy() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let admin_addr = listen_addr("ASKBAR_EGRESS_PROXY_ADMIN_ADDR", "egress-admin-host:8086");
    let sources = pairs("ASKBAR_EGRESS_PROXY_SOURCES", "model-gateway=egress-model-gateway-host:3128", true);
    let relays = pairs("ASKBAR_EGRESS_PROXY_RELAYS", "alerter=egress-alerter-host:2525", false);
    if let Some((name, _)) = relays.iter().find(|(n, _)| !sources.iter().any(|(s, _)| s == n)) {
        die(format!("ASKBAR_EGRESS_PROXY_RELAYS names {name}, which is not a source"), 2);
    }
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
    let boot_id = BootId::new();
    // Nothing is held that needs saving: the config is in memory and the admin pushes it again.
    let restarter = askbar_service::restart::Restarter::new(boot_id.clone(), || {});
    let proxy = Proxy::new(sources.iter().map(|(n, _)| n.clone()).collect(), system_resolver(), system_connector(), boot_id, restarter);
    // Docker does not restart an unhealthy container; the watchdog does, by exiting (ADR 0005).
    askbar_service::restart::start_watchdog(healthy);
    let bind = |addr: SocketAddr| async move {
        tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| die(format!("cannot listen on {addr}: {e}"), 1))
    };
    for (list, mode) in [(sources, Mode::Http), (relays, Mode::Relay)] {
        for (name, addr) in list {
            tokio::spawn(proxy.clone().serve(bind(addr).await, name, mode));
        }
    }
    let admin = bind(admin_addr).await;
    tracing::info!(%admin_addr, "the egress proxy is listening");
    let app = admin_router(proxy, admin_links(&dir));
    if let Err(e) = axum::serve(admin, app).with_graceful_shutdown(boot::stop_signal()).await {
        die(e, 1);
    }
}
