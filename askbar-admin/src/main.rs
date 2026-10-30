fn die(msg: impl std::fmt::Display, code: i32) -> ! {
    eprintln!("error: {msg}");
    std::process::exit(code)
}

#[tokio::main]
async fn main() {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        std::process::exit(if askbar_admin::healthcheck(&askbar_admin::socket_path()) { 0 } else { 1 });
    }
    tracing_subscriber::fmt().with_target(false).init();
    let db = askbar_admin::AdminDb::open(&askbar_admin::db_path()).unwrap_or_else(|e| die(e, 1));
    let token = askbar_admin::mgmt::ensure_token(&askbar_admin::mgmt::secrets_dir(), &askbar_admin::mgmt::mgmt_dir()).unwrap_or_else(|e| die(e, 1));
    let link = askbar_admin::mgmt::ServerLink::new(&askbar_admin::mgmt::server_url(), token);
    let state = askbar_admin::AdminState::with_server(db, link.clone());
    tokio::spawn(link.run(state.db.clone()));
    if let Err(e) = askbar_admin::serve_unix(&askbar_admin::socket_path(), askbar_admin::router(state)).await {
        die(e, 1);
    }
}
