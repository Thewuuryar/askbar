#[cfg(unix)]
fn die(msg: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(code)
}

#[cfg(unix)]
#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if askbar_admin::healthcheck(&askbar_admin::socket_path()) { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let db = askbar_admin::AdminDb::open(&askbar_admin::db_path()).unwrap_or_else(|e| die(e, 1));
    // `askbar-admin reset` (on the host: `docker compose exec admin askbar-admin reset`, or
    // `docker compose run --rm admin reset` while the admin is stopped): the published password again,
    // to be changed at the next sign-in (ADR 0007).
    if std::env::args().nth(1).as_deref() == Some("reset") {
        askbar_admin::web::reset_to_default(&db).unwrap_or_else(|e| die(e, 1));
        println!(
            "The admin password is the published one again ({}/{}); you must change it at the next sign-in.",
            askbar_admin::web::ADMIN_USERNAME,
            askbar_admin::web::DEFAULT_PASSWORD
        );
        println!("Restart the admin to sign out every session: docker compose restart admin");
        std::process::exit(0);
    }
    if askbar_admin::web::ensure_default_credentials(&db).unwrap_or_else(|e| die(e, 1)) {
        tracing::warn!("first start: the admin password is the published one; sign in and change it now");
    }
    let token = askbar_admin::server_link::ensure_token(&askbar_admin::server_link::secrets_dir(), &askbar_admin::server_link::token_dir())
        .unwrap_or_else(|e| die(e, 1));
    let link = askbar_admin::server_link::ServerLink::new(&askbar_admin::server_link::server_url(), token);
    let secrets =
        askbar_admin::secrets::AdminSecrets::load_or_create(&askbar_admin::server_link::secrets_dir()).unwrap_or_else(|e| die(e, 1));
    let state = askbar_admin::AdminState::build(db, Some(link.clone()), secrets);
    let caddy = askbar_admin::caddy::CaddyLink::from_env();
    if let Err(e) = caddy.share_socket_dir() {
        eprintln!("warning: Caddy may not be able to make its admin socket: {e}");
    }
    state.set_caddy(caddy);
    // CLIProxyAPI's config and key are made here, before CPA starts (it waits for the admin to be healthy).
    match askbar_admin::cpa::ensure_config(&askbar_admin::cpa::config_dir(), &askbar_admin::server_link::secrets_dir()) {
        Ok((keys, secret_replaced)) => {
            if let Err(e) = askbar_admin::cpa::prepare_restart_done(&askbar_admin::cpa::config_dir()) {
                eprintln!("warning: CLIProxyAPI's supervisor cannot report restarts: {e}");
            }
            state.set_cpa(askbar_admin::cpa::CpaLink::new(&askbar_admin::cpa::cpa_url(), keys));
            if secret_replaced {
                // CPA reads its config only at start; its supervisor restarts it because the config
                // just changed (ADR 0037).
                tracing::info!("CLIProxyAPI's control secret was replaced; its supervisor restarts it to read the new one");
            }
        }
        Err(e) => eprintln!("warning: CLIProxyAPI is not managed here: {e}"),
    }
    // POST /api/admin/v1/services/admin/restart: answer, then exit; Docker starts the admin again.
    let _ = state.restarter.set(askbar_service::restart::Restarter::new(askbar_service::status::BootId::new(), || {}));
    // Docker does not restart an unhealthy container; the watchdog does, by exiting (ADR 0005).
    let socket = askbar_admin::socket_path();
    askbar_service::restart::start_watchdog(move || askbar_admin::healthcheck(&socket));
    askbar_admin::caddy::apply_at_startup(state.clone());
    // The CPA key is the admin's own; the server gets it again after every push (a server with an empty database lost it).
    let resend: askbar_admin::server_link::Resend = {
        let state = state.clone();
        std::sync::Arc::new(move || {
            let state = state.clone();
            Box::pin(async move { askbar_admin::cpa::managed_key(&state).await.into_iter().collect() })
        })
    };
    tokio::spawn(link.run(state.db.clone(), resend));
    if let Err(e) = askbar_admin::serve_unix(&askbar_admin::socket_path(), askbar_admin::router(state)).await {
        die(e, 1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("askbar-admin runs on Linux and other unix systems only");
    std::process::exit(1);
}
