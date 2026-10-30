//! The Askbar admin service. It owns `admin.db` and the admin site, and talks to the server
//! only through the server's management API. It listens on a unix socket, never on a TCP port.

pub mod auth;
pub mod db;
pub mod mgmt;
pub mod web;

use axum::{
    http::StatusCode,
    middleware,
    routing::{get, post},
    Router,
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use db::AdminDb;

pub struct AdminState {
    pub db: Arc<Mutex<AdminDb>>,
    pub auth: auth::AdminAuth,
    /// The link to the server; `None` in tests that don't need one.
    pub server: Option<Arc<mgmt::ServerLink>>,
}

impl AdminState {
    pub fn new(db: AdminDb) -> Arc<AdminState> {
        Arc::new(AdminState { db: Arc::new(Mutex::new(db)), auth: auth::AdminAuth::default(), server: None })
    }

    pub fn with_server(db: AdminDb, server: Arc<mgmt::ServerLink>) -> Arc<AdminState> {
        Arc::new(AdminState { db: Arc::new(Mutex::new(db)), auth: auth::AdminAuth::default(), server: Some(server) })
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

pub fn router(state: Arc<AdminState>) -> Router {
    let protected: Router<Arc<AdminState>> = Router::new()
        .route("/admin/session", get(|| async { StatusCode::NO_CONTENT }))
        .route("/admin/logout", post(web::logout))
        .route("/admin/api/password", post(web::change_password))
        .route_layer(middleware::from_fn_with_state(state.clone(), web::require_session));
    Router::new()
        .route("/health", get(|| async { (StatusCode::OK, "ok") }))
        .route("/admin/login", post(web::login))
        .route("/admin/api/setup-status", get(web::setup_status))
        .route("/admin/api/setup/password", post(web::setup_password))
        .merge(protected)
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

/// `askbar-admin healthcheck`: ask the admin socket for `/health`. True on a 200.
pub fn healthcheck(path: &std::path::Path) -> bool {
    use std::io::{Read, Write};
    let attempt = || -> std::io::Result<bool> {
        let timeout = std::time::Duration::from_secs(2);
        let mut s = std::os::unix::net::UnixStream::connect(path)?;
        s.set_read_timeout(Some(timeout))?;
        s.set_write_timeout(Some(timeout))?;
        s.write_all(b"GET /health HTTP/1.0\r\nHost: admin\r\n\r\n")?;
        let mut head = [0u8; 12];
        s.read_exact(&mut head)?;
        Ok(&head == b"HTTP/1.1 200" || &head == b"HTTP/1.0 200")
    };
    attempt().unwrap_or(false)
}
