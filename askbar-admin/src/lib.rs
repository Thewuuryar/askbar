//! The Askbar admin service. It owns `admin.db` and the admin site, and talks to the server
//! only through the server's management API. It listens on a unix socket, never on a TCP port.

pub mod auth;
pub mod db;

use axum::{http::StatusCode, routing::get, Router};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use db::AdminDb;

pub struct AdminState {
    pub db: Mutex<AdminDb>,
    pub auth: auth::AdminAuth,
}

impl AdminState {
    pub fn new(db: AdminDb) -> Arc<AdminState> {
        Arc::new(AdminState { db: Mutex::new(db), auth: auth::AdminAuth::default() })
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
    Router::new().route("/health", get(|| async { (StatusCode::OK, "ok") })).with_state(state)
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
