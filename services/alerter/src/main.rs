use askbar_alerter::{admin_links, admin_router, auditor_links, auditor_router, data_dir, token_dir, Alerter, Timing};
use askbar_service::audit_client::AuditClient;
use askbar_service::boot::{self, die, listen_addr};
use askbar_service::status::BootId;
use std::net::SocketAddr;

/// `askbar-alerter healthcheck`: GET the health route on the admin's listener; exit 0 on a 200.
fn healthy() -> bool {
    std::env::var("ASKBAR_ALERTER_ADMIN_ADDR").is_ok_and(|addr| boot::healthy(addr, "/api/alerter/v1/health"))
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if healthy() { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let auditor_addr = listen_addr("ASKBAR_ALERTER_AUDITOR_ADDR", "alerter-auditor-host:8085");
    let admin_addr = listen_addr("ASKBAR_ALERTER_ADMIN_ADDR", "alerter-admin-host:8085");
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
    let audit = AuditClient::new(&audit_url, dir.clone(), "alerter-auditor");
    let boot_id = BootId::new();
    // Nothing is held that needs saving on the way out: the ledger is written as it changes, and the settings are the admin's.
    let restarter = askbar_service::restart::Restarter::new(boot_id.clone(), || {});
    let alerter = Alerter::new(audit.clone(), boot_id.clone(), restarter, &data_dir(), Timing::default());
    // Docker does not restart an unhealthy container; the watchdog does, by exiting (ADR 0005).
    askbar_service::restart::start_watchdog(healthy);
    tokio::spawn(audit.clone().hello_until_done(boot_id.as_str().to_string()));
    tokio::spawn(audit.clone().drain_forever());
    tokio::spawn(audit.clone().status_every_minute());
    tokio::spawn(alerter.clone().deliver_forever());
    tokio::spawn(alerter.clone().releases_forever());
    let auditor_app = auditor_router(alerter.clone(), auditor_links(&dir));
    let admin_app = admin_router(alerter, admin_links(&dir));
    let serve = |addr: SocketAddr, app: axum::Router, what: &'static str| async move {
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        tracing::info!(%addr, what, "the alerter is listening");
        axum::serve(listener, app).with_graceful_shutdown(boot::stop_signal()).await.map_err(|e| e.to_string())
    };
    if let Err(e) = tokio::try_join!(serve(auditor_addr, auditor_app, "for the auditor"), serve(admin_addr, admin_app, "for the admin")) {
        die(e, 1);
    }
}
