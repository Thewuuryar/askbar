//! The HTTPS front end: turn the Network settings into a Caddyfile and hand it to Caddy.
//!
//! The Cloudflare token reaches Caddy only as a file on a volume that only the server
//! writes and only Caddy reads; the Caddyfile refers to it with `{file./...}`, so the
//! token itself never appears in any config text or log.

use crate::network::{self, Network};
use crate::{auth::now, AppState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Where Caddy sees the token file (its mount of the shared secrets volume).
pub const CADDY_TOKEN_PATH: &str = "/caddy-secrets/cf_token";
pub const STAGING_CA: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Where the server can reach Caddy's admin API, and where it writes the token file.
#[derive(Clone, Default)]
pub struct CaddyLink {
    pub admin_url: Option<String>,
    pub secrets_dir: Option<PathBuf>,
}

impl CaddyLink {
    pub fn from_env() -> CaddyLink {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        CaddyLink { admin_url: get("ASKBAR_CADDY_ADMIN"), secrets_dir: get("ASKBAR_CADDY_SECRETS_DIR").map(PathBuf::from) }
    }
}

#[derive(Clone, Serialize)]
pub struct ProxyStatus {
    pub ok: bool,
    pub message: String,
    /// Unix time of the last attempt, 0 if none yet.
    pub at: i64,
}

impl Default for ProxyStatus {
    fn default() -> Self {
        ProxyStatus { ok: false, message: "Not applied yet.".into(), at: 0 }
    }
}

fn status(ok: bool, message: impl Into<String>) -> ProxyStatus {
    ProxyStatus { ok, message: message.into(), at: now() }
}

/// The full Caddyfile for the current settings. With HTTPS off it is just the admin listener,
/// which makes Caddy stop serving. Every interpolated value was validated when it was saved
/// (hostname, CIDRs); the token is never interpolated.
pub fn caddyfile(n: &Network, cidrs: &[String]) -> String {
    let mut out = String::from("{\n\tadmin 0.0.0.0:2019 {\n\t\torigins caddy:2019\n\t}\n");
    if n.tls_enabled && !n.hostname.is_empty() {
        out.push_str("\tauto_https disable_redirects\n");
        out.push_str(&format!("\tacme_dns cloudflare {{file.{CADDY_TOKEN_PATH}}}\n"));
        if n.acme_staging {
            out.push_str(&format!("\tacme_ca {STAGING_CA}\n"));
        }
        out.push_str("}\n");
        out.push_str(&format!(
            "\n{host}:8443 {{\n\treverse_proxy server:8080 {{\n\t\tflush_interval -1\n\t}}\n}}\n",
            host = n.hostname
        ));
        out.push_str(&format!(
            "\n{host}:9443 {{\n\t@allowed remote_ip {nets}\n\thandle @allowed {{\n\t\treverse_proxy server:8081\n\t}}\n\trespond 403\n}}\n",
            host = n.hostname,
            nets = cidrs.join(" ")
        ));
    } else {
        out.push_str("}\n");
    }
    out
}

/// Write (or remove) the token file Caddy reads. Atomic, mode 0600.
fn sync_token_file(dir: &Path, token: Option<&str>) -> std::io::Result<()> {
    let target = dir.join("cf_token");
    match token {
        Some(t) => {
            use std::io::Write;
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(".cf_token.tmp");
            let _ = std::fs::remove_file(&tmp);
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
            let mut f = opts.open(&tmp)?;
            f.write_all(t.as_bytes())?;
            f.sync_all()?;
            std::fs::rename(&tmp, &target)
        }
        None => match std::fs::remove_file(&target) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// Push the current settings to Caddy and remember how it went.
pub async fn apply(state: &Arc<AppState>) -> ProxyStatus {
    let result = apply_inner(state).await;
    *state.proxy.lock().unwrap_or_else(|e| e.into_inner()) = result.clone();
    result
}

async fn apply_inner(state: &Arc<AppState>) -> ProxyStatus {
    let link = state.caddy.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(admin) = link.admin_url else {
        return status(false, "The HTTPS front end is not set up on this server (start it with the tls compose profile).");
    };
    // Gather everything under the lock, then let go of it before any network call.
    let (net, token) = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let Ok(net) = network::load(&db) else { return status(false, "Could not read the network settings.") };
        let token = match (state.secrets(), network::stored_token(&db)) {
            (Some(s), Some(enc)) => s.decrypt_b64(&enc).ok(),
            _ => None,
        };
        (net, token)
    };
    let enabled = net.tls_enabled && !net.hostname.is_empty();
    let Some(cidrs) = network::parse_cidr_list(&net.admin_cidr).or_else(|| (!enabled).then(Vec::new)) else {
        return status(false, "The allowed networks setting is not valid.");
    };
    if enabled && token.is_none() {
        return status(false, "HTTPS is on but no Cloudflare token is stored (or it can't be read with the current master key).");
    }
    if let Some(dir) = &link.secrets_dir {
        let wanted = if enabled { token.as_deref() } else { None };
        if let Err(e) = sync_token_file(dir, wanted) {
            return status(false, format!("Could not write the token file for Caddy: {e}"));
        }
    }
    let config = caddyfile(&net, &cidrs);
    let url = format!("{}/load", admin.trim_end_matches('/'));
    let sent = state
        .http
        .post(&url)
        .header("Content-Type", "text/caddyfile")
        .body(config)
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    match sent {
        Err(_) => status(false, "Could not reach Caddy. Is the tls compose profile running?"),
        Ok(r) if r.status().is_success() => {
            status(true, if enabled { "Caddy is serving the configuration. Certificates are requested the first time it is used." } else { "HTTPS is off; Caddy is not serving any site." })
        }
        Ok(r) => {
            let code = r.status();
            let body = r.text().await.unwrap_or_default();
            status(false, format!("Caddy refused the configuration ({code}): {}", body.trim().chars().take(400).collect::<String>()))
        }
    }
}

/// At start-up Caddy may still be coming up, so keep trying for a while.
pub fn apply_at_startup(state: Arc<AppState>) {
    if state.caddy.lock().unwrap_or_else(|e| e.into_inner()).admin_url.is_none() {
        return;
    }
    tokio::spawn(async move {
        for attempt in 0..12 {
            let ready = state.setup_complete();
            if ready && apply(&state).await.ok {
                tracing::info!("HTTPS front end configured");
                return;
            }
            if attempt == 11 {
                tracing::warn!("could not configure Caddy at start-up; use Network > Apply in the admin page");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(tls: bool, staging: bool) -> Network {
        Network { tls_enabled: tls, hostname: "askbar.orraitu.com".into(), admin_cidr: String::new(), acme_staging: staging }
    }

    #[test]
    fn production_config() {
        let got = caddyfile(&net(true, false), &["192.168.1.0/24".into(), "10.0.0.5/32".into()]);
        let want = "{
\tadmin 0.0.0.0:2019 {
\t\torigins caddy:2019
\t}
\tauto_https disable_redirects
\tacme_dns cloudflare {file./caddy-secrets/cf_token}
}

askbar.orraitu.com:8443 {
\treverse_proxy server:8080 {
\t\tflush_interval -1
\t}
}

askbar.orraitu.com:9443 {
\t@allowed remote_ip 192.168.1.0/24 10.0.0.5/32
\thandle @allowed {
\t\treverse_proxy server:8081
\t}
\trespond 403
}
";
        assert_eq!(got, want);
    }

    #[test]
    fn staging_adds_the_test_ca_and_nothing_else() {
        let prod = caddyfile(&net(true, false), &["127.0.0.1/32".into()]);
        let staging = caddyfile(&net(true, true), &["127.0.0.1/32".into()]);
        let extra = format!("\tacme_ca {STAGING_CA}\n");
        assert_eq!(staging.replace(&extra, ""), prod);
        assert!(staging.contains(&extra));
    }

    #[test]
    fn https_off_serves_nothing() {
        let off = caddyfile(&net(false, false), &[]);
        assert_eq!(off, "{\n\tadmin 0.0.0.0:2019 {\n\t\torigins caddy:2019\n\t}\n}\n");
        assert!(!off.contains("askbar") && !off.contains("acme"));
    }

    #[test]
    fn the_token_never_appears_in_the_config() {
        let text = caddyfile(&net(true, false), &["127.0.0.1/32".into()]);
        assert!(text.contains("{file./caddy-secrets/cf_token}"));
        assert!(!text.contains("cf_ABC"));
    }

    #[test]
    fn token_file_is_private_and_removed_when_not_needed() {
        let dir = std::env::temp_dir().join(format!("askbar-cf-{}", ulid::Ulid::new()));
        sync_token_file(&dir, Some("tok-123")).unwrap();
        let f = dir.join("cf_token");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "tok-123");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&f).unwrap().permissions()) & 0o777, 0o600);
        sync_token_file(&dir, Some("tok-456")).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "tok-456");
        assert!(!dir.join(".cf_token.tmp").exists());
        sync_token_file(&dir, None).unwrap();
        assert!(!f.exists());
        sync_token_file(&dir, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
