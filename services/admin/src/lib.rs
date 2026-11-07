//! The Askbar admin service. Unix only: it listens on a unix socket, and on other platforms this crate is empty.
//! It owns `admin.db` and the admin site, and talks to the server
//! only through the server's server API. It listens on a unix socket, never on a TCP port.
#![cfg(unix)]


pub mod admin_api;
pub mod api_key;
pub mod audit_key;
pub mod auth;
pub mod caddy;
pub mod cpa;
pub mod db;
pub mod server_link;
pub mod network;
pub mod routes;
pub mod secrets;
pub mod self_cert;
pub mod services;
pub mod web;

use axum::{
    http::StatusCode,
    middleware,
    response::Redirect,
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub use db::AdminDb;
pub use routes::Routes;

pub struct AdminState {
    /// Async lock: a change holds it while it waits for the server to accept the new snapshot.
    pub db: Arc<Mutex<AdminDb>>,
    pub auth: auth::AdminAuth,
    /// Limits guesses at the audit passphrase (see `audit_key::Limiter`).
    pub audit_opens: audit_key::Limiter,
    /// The link to the server; `None` in tests that don't need one.
    pub server: Option<Arc<server_link::ServerLink>>,
    /// Encrypts the Cloudflare token at rest.
    pub secrets: secrets::AdminSecrets,
    /// How to reach Caddy; set at start-up, changed by tests.
    pub caddy: std::sync::Mutex<caddy::CaddyLink>,
    /// How the last attempt to configure Caddy went.
    pub proxy: std::sync::Mutex<caddy::ProxyStatus>,
    /// What `POST /api/admin/v1/services/admin/restart` does; set once by `main` (tests set their own).
    pub restarter: std::sync::OnceLock<askbar_service::restart::Restarter>,
    /// The link to CLIProxyAPI, once its config exists.
    pub cpa: std::sync::Mutex<Option<Arc<cpa::CpaLink>>>,
    /// Logins started and not finished: state -> (provider, expiry in unix seconds).
    pub cpa_logins: std::sync::Mutex<std::collections::HashMap<String, (String, i64)>>,
    /// When recent service actions happened (unix seconds), for the rate limit.
    pub service_actions: std::sync::Mutex<std::collections::VecDeque<i64>>,
}

impl AdminState {
    pub fn new(db: AdminDb) -> Arc<AdminState> {
        AdminState::build(db, None, secrets::AdminSecrets::ephemeral())
    }

    pub fn with_server(db: AdminDb, server: Arc<server_link::ServerLink>) -> Arc<AdminState> {
        AdminState::build(db, Some(server), secrets::AdminSecrets::ephemeral())
    }

    pub fn build(db: AdminDb, server: Option<Arc<server_link::ServerLink>>, secrets: secrets::AdminSecrets) -> Arc<AdminState> {
        Arc::new(AdminState {
            db: Arc::new(Mutex::new(db)),
            auth: auth::AdminAuth::default(),
            audit_opens: audit_key::Limiter::default(),
            server,
            secrets,
            caddy: std::sync::Mutex::new(caddy::CaddyLink::default()),
            proxy: std::sync::Mutex::new(caddy::ProxyStatus::default()),
            restarter: Default::default(),
            cpa: std::sync::Mutex::new(None),
            cpa_logins: std::sync::Mutex::new(Default::default()),
            service_actions: std::sync::Mutex::new(Default::default()),
        })
    }

    pub fn set_caddy(&self, link: caddy::CaddyLink) {
        *self.caddy.lock().unwrap_or_else(|e| e.into_inner()) = link;
    }

    pub fn set_cpa(&self, link: Arc<cpa::CpaLink>) {
        *self.cpa.lock().unwrap_or_else(|e| e.into_inner()) = Some(link);
    }

    /// Is the server there? (False when there is no link at all.)
    pub fn server_online(&self) -> bool {
        self.server.as_ref().is_some_and(|s| s.server_online())
    }
}

/// Where the admin socket is: `ASKBAR_ADMIN_SOCKET`, default `/run/admin/admin.sock`.
pub fn socket_path() -> PathBuf {
    std::env::var("ASKBAR_ADMIN_SOCKET")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/admin/admin.sock"))
}

/// Where `admin.db` is: `ASKBAR_ADMIN_DATA_DIR/admin.db`, default `/admin-data`.
pub fn db_path() -> PathBuf {
    let dir = std::env::var("ASKBAR_ADMIN_DATA_DIR").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "/admin-data".into());
    PathBuf::from(dir).join("admin.db")
}

fn asset(content_type: &'static str, body: &'static str) -> impl axum::response::IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, content_type)], body)
}

/// All of the admin's routes, with the list of what was registered (see `routes`).
pub fn routes(state: &Arc<AdminState>) -> Routes {
    // Every API route is under `/api/admin/v1` (askbar_proto::paths::ADMIN_V1).
    let v1 = |rest: &str| askbar_proto::paths::join(askbar_proto::paths::ADMIN_V1, rest);
    // What a script with an admin API key may also call.
    let api = Routes::new()
        .get(&v1("/status"), admin_api::status)
        .get(&v1("/providers"), admin_api::list_providers)
        .post(&v1("/providers"), admin_api::create_provider)
        .put(&v1("/providers/{id}"), admin_api::update_provider)
        .delete(&v1("/providers/{id}"), admin_api::delete_provider)
        .post(&v1("/providers/{id}/test"), admin_api::test_provider)
        .post(&v1("/providers/models"), admin_api::list_models)
        .get(&v1("/clients"), admin_api::list_clients)
        .post(&v1("/clients/{id}/revoke"), admin_api::revoke_client)
        .post(&v1("/pairings"), admin_api::create_pairing)
        .put(&v1("/setup/audit-key"), admin_api::setup_audit_key)
        .get(&v1("/audit/traces"), admin_api::audit_traces)
        .get(&v1("/audit/traces/{id}"), admin_api::audit_trace)
        .get(&v1("/audit/stats"), admin_api::audit_stats)
        .get(&v1("/audit/provider-health"), admin_api::audit_provider_health)
        .get(&v1("/audit/access"), admin_api::audit_access)
        .post(&v1("/audit/rotate-key"), admin_api::rotate_audit_key)
        .post(&v1("/audit/payloads/{span_id}/open"), admin_api::open_audit_payloads)
        .get(&v1("/settings"), admin_api::get_settings)
        .put(&v1("/settings"), admin_api::put_settings)
        .put(&v1("/users/{id}/display-defaults"), admin_api::put_user_display)
        .get(&v1("/backup"), admin_api::backup)
        .get(&v1("/network"), network::get_network)
        .put(&v1("/network"), network::put_network)
        .post(&v1("/network/apply"), network::apply_network)
        .signed_in_or_key(state);
    // What only the person's own session may do: the password, the API key, and looking at or ending the session.
    let session_only = Routes::new()
        .put(&v1("/password"), web::change_password)
        .get(&v1("/session"), || async { StatusCode::NO_CONTENT })
        .delete(&v1("/session"), web::logout)
        .get(&v1("/cpa"), cpa::status)
        .post(&v1("/cpa/logins"), cpa::start_login)
        .get(&v1("/cpa/logins/{state}"), cpa::login_status)
        .post(&v1("/cpa/logins/{state}/callback"), cpa::finish_login)
        .delete(&v1("/cpa/accounts/{name}"), cpa::remove_account)
        .get(&v1("/services"), services::list)
        .post(&v1("/services/{name}/restart"), services::restart)
        .post(&v1("/keys"), admin_api::create_key)
        .get(&v1("/keys"), admin_api::list_keys)
        .delete(&v1("/keys/{id}"), admin_api::revoke_key)
        .signed_in(state);
    // Open, but a change still wants the browser header.
    let open = Routes::new().post(&v1("/session"), web::login).put(&v1("/setup/password"), web::setup_password).browser_header();
    Routes::new()
        .get("/admin", || async { Redirect::to("/admin/") })
        .get("/admin/", || async { asset("text/html; charset=utf-8", include_str!("ui/index.html")) })
        .get("/admin/app.js", || async { asset("text/javascript; charset=utf-8", include_str!("ui/app.js")) })
        .get("/admin/audit.js", || async { asset("text/javascript; charset=utf-8", include_str!("ui/audit.js")) })
        .get("/admin/tokens.css", || async { asset("text/css; charset=utf-8", include_str!("ui/tokens.css")) })
        .get("/admin/style.css", || async { asset("text/css; charset=utf-8", include_str!("ui/style.css")) })
        .get(&v1("/health"), || async { (StatusCode::OK, "ok") })
        .get(&v1("/setup"), web::setup_status)
        .merge(open)
        .merge(session_only)
        .merge(api)
}

pub fn router(state: Arc<AdminState>) -> Router {
    let (router, _) = routes(&state).into_parts();
    router
        .fallback(askbar_proto::http::not_found)
        .method_not_allowed_fallback(askbar_proto::http::method_not_allowed)
        .layer(middleware::from_fn(web::security_headers))
        .with_state(state)
}

/// Bind the unix socket at `path` (replacing a stale one), restrict it to its owner and
/// group, and serve `app` on it until the process ends.
pub async fn serve_unix(path: &std::path::Path, app: Router) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let listener = tokio::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?;
    tracing::info!(socket = %path.display(), "listening");
    axum::serve(listener, app).await
}

/// `askbar-admin healthcheck`: ask the admin socket for `/api/admin/v1/health`. True on a 200.
pub fn healthcheck(path: &std::path::Path) -> bool {
    use std::io::{Read, Write};
    let attempt = || -> std::io::Result<bool> {
        let timeout = std::time::Duration::from_secs(2);
        let mut s = std::os::unix::net::UnixStream::connect(path)?;
        s.set_read_timeout(Some(timeout))?;
        s.set_write_timeout(Some(timeout))?;
        s.write_all(b"GET /api/admin/v1/health HTTP/1.0\r\nHost: admin\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    };
    attempt().unwrap_or(false)
}
