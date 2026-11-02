//! The Askbar admin service. Unix only: it listens on a unix socket, and on other platforms this crate is empty.
//! It owns `admin.db` and the admin site, and talks to the server
//! only through the server's server API. It listens on a unix socket, never on a TCP port.
#![cfg(unix)]


pub mod admin_api;
pub mod audit_key;
pub mod auth;
pub mod caddy;
pub mod db;
pub mod server_link;
pub mod network;
pub mod secrets;
pub mod web;

use axum::{
    http::StatusCode,
    middleware,
    response::Redirect,
    routing::{get, post, put},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

pub use db::AdminDb;

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
        })
    }

    pub fn set_caddy(&self, link: caddy::CaddyLink) {
        *self.caddy.lock().unwrap_or_else(|e| e.into_inner()) = link;
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

pub fn router(state: Arc<AdminState>) -> Router {
    // Every API route is under `/admin/api/v1` (askbar_proto::paths::ADMIN_V1).
    let v1 = |rest: &str| askbar_proto::paths::join(askbar_proto::paths::ADMIN_V1, rest);
    let session = || middleware::from_fn_with_state(state.clone(), web::require_session);
    let protected: Router<Arc<AdminState>> = Router::new()
        .route(&v1("/password"), put(web::change_password))
        .route(&v1("/status"), get(admin_api::status))
        .route(&v1("/providers"), get(admin_api::list_providers).post(admin_api::create_provider))
        .route(&v1("/providers/{id}"), put(admin_api::update_provider).delete(admin_api::delete_provider))
        .route(&v1("/providers/{id}/test"), post(admin_api::test_provider))
        .route(&v1("/clients"), get(admin_api::list_clients))
        .route(&v1("/clients/{id}/revoke"), post(admin_api::revoke_client))
        .route(&v1("/pairings"), post(admin_api::create_pairing))
        .route(&v1("/setup/audit-key"), put(admin_api::setup_audit_key))
        .route(&v1("/audit/traces"), get(admin_api::audit_traces))
        .route(&v1("/audit/traces/{id}"), get(admin_api::audit_trace))
        .route(&v1("/audit/stats"), get(admin_api::audit_stats))
        .route(&v1("/audit/provider-health"), get(admin_api::audit_provider_health))
        .route(&v1("/audit/access"), get(admin_api::audit_access))
        .route(&v1("/audit/rotate-key"), post(admin_api::rotate_audit_key))
        .route(&v1("/audit/payloads/{span_id}/open"), post(admin_api::open_audit_payloads))
        .route(&v1("/settings"), get(admin_api::get_settings).put(admin_api::put_settings))
        .route(&v1("/users/{id}/display-defaults"), put(admin_api::put_user_display))
        .route(&v1("/backup"), get(admin_api::backup))
        .route(&v1("/network"), get(network::get_network).put(network::put_network))
        .route(&v1("/network/apply"), post(network::apply_network))
        .route_layer(session());
    Router::new()
        .route("/admin", get(|| async { Redirect::to("/admin/") }))
        .route("/admin/", get(|| async { asset("text/html; charset=utf-8", include_str!("ui/index.html")) }))
        .route("/admin/app.js", get(|| async { asset("text/javascript; charset=utf-8", include_str!("ui/app.js")) }))
        .route("/admin/audit.js", get(|| async { asset("text/javascript; charset=utf-8", include_str!("ui/audit.js")) }))
        .route("/admin/style.css", get(|| async { asset("text/css; charset=utf-8", include_str!("ui/style.css")) }))
        .route(&v1("/health"), get(|| async { (StatusCode::OK, "ok") }))
        // The session: signing in is open, looking at it and ending it need the session itself.
        .route(&v1("/session"), get(|| async { StatusCode::NO_CONTENT }).delete(web::logout).route_layer(session()).post(web::login))
        .route(&v1("/setup"), get(web::setup_status))
        .route(&v1("/setup/password"), put(web::setup_password))
        .merge(protected)
        .fallback(askbar_proto::http::not_found)
        .method_not_allowed_fallback(askbar_proto::http::method_not_allowed)
        .layer(middleware::from_fn(web::require_admin_header))
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

/// `askbar-admin healthcheck`: ask the admin socket for `/admin/api/v1/health`. True on a 200.
pub fn healthcheck(path: &std::path::Path) -> bool {
    use std::io::{Read, Write};
    let attempt = || -> std::io::Result<bool> {
        let timeout = std::time::Duration::from_secs(2);
        let mut s = std::os::unix::net::UnixStream::connect(path)?;
        s.set_read_timeout(Some(timeout))?;
        s.set_write_timeout(Some(timeout))?;
        s.write_all(b"GET /admin/api/v1/health HTTP/1.0\r\nHost: admin\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    };
    attempt().unwrap_or(false)
}
