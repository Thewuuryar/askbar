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
    let token = askbar_admin::server_link::ensure_token(&askbar_admin::server_link::secrets_dir(), &askbar_admin::server_link::token_dir()).unwrap_or_else(|e| die(e, 1));
    let link = askbar_admin::server_link::ServerLink::new(&askbar_admin::server_link::server_url(), token);
    let secrets = askbar_admin::secrets::AdminSecrets::load_or_create(&askbar_admin::server_link::secrets_dir()).unwrap_or_else(|e| die(e, 1));
    let state = askbar_admin::AdminState::build(db, Some(link.clone()), secrets);
    state.set_caddy(askbar_admin::caddy::CaddyLink::from_env());
    match askbar_admin::control_link::ControlLink::from_env(&askbar_admin::server_link::secrets_dir()) {
        Ok(link) => state.set_control(link),
        Err(e) => eprintln!("warning: the controller link is off: {e}"),
    }
    // CLIProxyAPI's config and key are made here, before CPA starts (it waits for the admin to be healthy).
    match askbar_admin::cpa::ensure_config(&askbar_admin::cpa::config_dir(), &askbar_admin::server_link::secrets_dir()) {
        Ok((keys, secret_replaced)) => {
            state.set_cpa(askbar_admin::cpa::CpaLink::new(&askbar_admin::cpa::cpa_url(), keys));
            if secret_replaced {
                // CPA was running with the old config: restart it so it reads the new control secret.
                let st = state.clone();
                tokio::spawn(async move {
                    let link = st.control.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(l) = link {
                        for _ in 0..10 {
                            if matches!(l.act("cpa", "restart").await, Ok((200, _))) {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                        }
                    }
                });
            }
        }
        Err(e) => eprintln!("warning: CLIProxyAPI is not managed here: {e}"),
    }
    askbar_admin::caddy::apply_at_startup(state.clone());
    tokio::spawn(link.run(state.db.clone()));
    if let Err(e) = askbar_admin::serve_unix(&askbar_admin::socket_path(), askbar_admin::router(state)).await {
        die(e, 1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("askbar-admin runs on Linux and other unix systems only");
    std::process::exit(1);
}
