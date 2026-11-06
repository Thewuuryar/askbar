//! The admin's side of the controller API: ask how the services are doing, and start, stop or restart one.
//! The raw token lives in the admin's secrets directory; the controller holds only its SHA-256, in a volume
//! it mounts read-only (see `server_link::ensure_token_as`, which is the same scheme as the server token).

use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TOKEN_FILE: &str = "control.token";
/// The services the admin may control. The controller enforces the same list; this copy only gives a clear
/// 404 before the call.
pub const SERVICES: [&str; 3] = ["server", "caddy", "cpa"];
pub const ACTIONS: [&str; 3] = ["start", "stop", "restart"];

/// `ASKBAR_CONTROL_TOKEN_DIR`, default `/control-token`.
pub fn token_dir() -> PathBuf {
    std::env::var("ASKBAR_CONTROL_TOKEN_DIR").ok().filter(|p| !p.trim().is_empty()).map(PathBuf::from).unwrap_or_else(|| "/control-token".into())
}

/// `ASKBAR_CONTROL_URL`, default `http://controller:8083`.
pub fn url() -> String {
    std::env::var("ASKBAR_CONTROL_URL").ok().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "http://controller:8083".into())
}

pub struct ControlLink {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl ControlLink {
    pub fn new(base: &str, token: String) -> std::sync::Arc<ControlLink> {
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(3)).timeout(Duration::from_secs(90)).build().expect("reqwest client");
        std::sync::Arc::new(ControlLink { http, base: base.trim_end_matches('/').to_string(), token })
    }

    /// Make the token and its hash file if needed, then link to the controller named by the environment.
    pub fn from_env(secrets_dir: &Path) -> std::io::Result<std::sync::Arc<ControlLink>> {
        let token = crate::server_link::ensure_token_as(secrets_dir, &token_dir(), TOKEN_FILE, "ctl_")?;
        Ok(ControlLink::new(&url(), token))
    }

    async fn call(&self, method: reqwest::Method, path: &str) -> Result<(u16, Vec<u8>), String> {
        let r = self
            .http
            .request(method, format!("{}{}{path}", self.base, askbar_proto::paths::CONTROL_V1))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| format!("The controller did not answer: {}", e.without_url()))?;
        let status = r.status().as_u16();
        let body = r.bytes().await.map_err(|e| format!("The controller did not answer: {}", e.without_url()))?;
        Ok((status, body.to_vec()))
    }

    pub async fn services(&self) -> Result<(u16, Vec<u8>), String> {
        self.call(reqwest::Method::GET, "/services").await
    }

    pub async fn act(&self, name: &str, action: &str) -> Result<(u16, Vec<u8>), String> {
        self.call(reqwest::Method::POST, &format!("/services/{name}/{action}")).await
    }
}
