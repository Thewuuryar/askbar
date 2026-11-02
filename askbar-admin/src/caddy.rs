//! The HTTPS front end: turn the Network settings into a Caddyfile and hand it to Caddy.
//!
//! Caddy's admin API is a unix socket on a volume shared with this service, so there is no
//! network address to reach it on. The Cloudflare token reaches Caddy only as a file on a
//! volume that only this service writes and only Caddy reads; the Caddyfile refers to it with
//! `{file./...}`, so the token never appears in any config text or log.

use crate::network::{self, Network};
use crate::{auth::now, AdminState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Where Caddy itself sees the token file (its mount of the shared secrets volume).
pub const CADDY_TOKEN_PATH: &str = "/caddy-secrets/cf_token";
/// Where Caddy itself sees the admin service's socket (its mount of the admin's run volume).
pub const ADMIN_UPSTREAM: &str = "unix//run/admin/admin.sock";
/// Caddy's own admin listener, as it must be written in every config it is given, or loading
/// the config would take the listener away.
pub const CADDY_ADMIN_LISTEN: &str = "unix//run/caddy/admin.sock|0660";
pub const STAGING_CA: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// Where this service reaches Caddy's admin socket, and where it writes the token file.
#[derive(Clone, Default)]
pub struct CaddyLink {
    pub socket: Option<PathBuf>,
    pub secrets_dir: Option<PathBuf>,
}

impl CaddyLink {
    pub fn from_env() -> CaddyLink {
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        CaddyLink { socket: get("ASKBAR_CADDY_SOCKET").map(PathBuf::from), secrets_dir: get("ASKBAR_CADDY_SECRETS_DIR").map(PathBuf::from) }
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
/// which makes Caddy stop serving. The API is served at the hostname and this admin site at
/// `admin.` plus the hostname. `remote_ip` is added only when the allowlist is on, and then
/// anyone outside it has the connection dropped. Every interpolated value was validated when it
/// was saved (hostname, addresses); the token is never interpolated.
pub fn caddyfile(n: &Network, allowed: &[String]) -> String {
    let mut out = format!("{{\n\tadmin {CADDY_ADMIN_LISTEN}\n");
    if n.tls_enabled && !n.hostname.is_empty() {
        out.push_str(&format!("\tacme_dns cloudflare {{file.{CADDY_TOKEN_PATH}}}\n"));
        if n.acme_staging {
            out.push_str(&format!("\tacme_ca {STAGING_CA}\n"));
        }
        out.push_str("}\n");
        out.push_str(&format!("\n{host} {{\n\treverse_proxy server:8080 {{\n\t\tflush_interval -1\n\t}}\n}}\n", host = n.hostname));
        out.push_str(&format!("\nadmin.{host} {{\n", host = n.hostname));
        if n.allowlist_enabled {
            out.push_str(&format!("\t@outside not remote_ip {}\n\tabort @outside\n", allowed.join(" ")));
        }
        out.push_str(&format!("\treverse_proxy {ADMIN_UPSTREAM}\n}}\n"));
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
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(".cf_token.tmp");
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
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
pub async fn apply(state: &Arc<AdminState>) -> ProxyStatus {
    let result = apply_inner(state).await;
    *state.proxy.lock().unwrap_or_else(|e| e.into_inner()) = result.clone();
    result
}

async fn apply_inner(state: &Arc<AdminState>) -> ProxyStatus {
    let link = state.caddy.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let Some(socket) = link.socket else {
        return status(false, "The HTTPS front end is not set up (Caddy's admin socket is not configured).");
    };
    // Gather everything under the lock, then let go of it before any network call.
    let (net, token) = {
        let db = state.db.lock().await;
        let Ok(net) = network::load(db.conn()) else { return status(false, "Could not read the network settings.") };
        let token = network::stored_token(db.conn()).and_then(|enc| state.secrets.decrypt(&enc));
        (net, token)
    };
    let enabled = net.tls_enabled && !net.hostname.is_empty();
    let allowed = if net.allowlist_enabled { network::parse_cidr_list(&net.admin_cidr) } else { Some(Vec::new()) };
    let Some(allowed) = allowed else {
        return status(false, "The allowed addresses setting is not valid.");
    };
    if enabled && token.is_none() {
        return status(false, "HTTPS is on but no Cloudflare token is stored (or it can't be read).");
    }
    if let Some(dir) = &link.secrets_dir {
        let wanted = if enabled { token.as_deref() } else { None };
        if let Err(e) = sync_token_file(dir, wanted) {
            return status(false, format!("Could not write the token file for Caddy: {e}"));
        }
    }
    let config = caddyfile(&net, &allowed);
    let client = match reqwest::Client::builder().unix_socket(socket).build() {
        Ok(c) => c,
        Err(e) => return status(false, format!("Could not set up the connection to Caddy: {e}")),
    };
    let sent = client
        .post("http://caddy/load")
        .header("Content-Type", "text/caddyfile")
        .body(config)
        .timeout(Duration::from_secs(20))
        .send()
        .await;
    match sent {
        Err(_) => status(false, "Could not reach Caddy. Is it running?"),
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
pub fn apply_at_startup(state: Arc<AdminState>) {
    if state.caddy.lock().unwrap_or_else(|e| e.into_inner()).socket.is_none() {
        return;
    }
    tokio::spawn(async move {
        for attempt in 0..12 {
            if apply(&state).await.ok {
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

    fn net(tls: bool, staging: bool, allowlist: bool) -> Network {
        Network { tls_enabled: tls, hostname: "askbar.orraitu.com".into(), admin_cidr: String::new(), acme_staging: staging, allowlist_enabled: allowlist }
    }

    #[test]
    fn production_config_without_an_allowlist() {
        let got = caddyfile(&net(true, false, false), &[]);
        let want = "{
\tadmin unix//run/caddy/admin.sock|0660
\tacme_dns cloudflare {file./caddy-secrets/cf_token}
}

askbar.orraitu.com {
\treverse_proxy server:8080 {
\t\tflush_interval -1
\t}
}

admin.askbar.orraitu.com {
\treverse_proxy unix//run/admin/admin.sock
}
";
        assert_eq!(got, want);
        assert!(!got.contains("remote_ip") && !got.contains("abort"), "no matcher when the allowlist is off");
        assert!(!got.contains("origins"), "the admin socket has no origins line");
    }

    #[test]
    fn the_allowlist_adds_a_remote_ip_abort_matcher_only_when_on() {
        let nets = vec!["192.168.1.0/24".to_string(), "10.0.0.5/32".to_string(), "fd00::/8".to_string()];
        let on = caddyfile(&net(true, false, true), &nets);
        assert!(on.contains("\t@outside not remote_ip 192.168.1.0/24 10.0.0.5/32 fd00::/8\n\tabort @outside\n"), "{on}");
        // The matcher guards the admin host only; the API is open to paired devices.
        let (api, admin) = on.split_once("admin.askbar.orraitu.com").unwrap();
        assert!(!api.contains("remote_ip") && admin.contains("remote_ip"));
        // Off: the same addresses are ignored.
        assert!(!caddyfile(&net(true, false, false), &nets).contains("remote_ip"));
    }

    #[test]
    fn staging_adds_the_test_ca_and_nothing_else() {
        let prod = caddyfile(&net(true, false, false), &[]);
        let staging = caddyfile(&net(true, true, false), &[]);
        assert!(staging.contains(&format!("\tacme_ca {STAGING_CA}\n")));
        assert_eq!(staging.replace(&format!("\tacme_ca {STAGING_CA}\n"), ""), prod);
    }

    #[test]
    fn https_off_serves_nothing_but_keeps_caddys_admin_socket() {
        let off = caddyfile(&net(false, false, true), &["10.0.0.1/32".into()]);
        assert_eq!(off, "{\n\tadmin unix//run/caddy/admin.sock|0660\n}\n");
        assert!(!off.contains("askbar.orraitu.com"));
    }

    #[test]
    fn the_token_never_appears_in_the_config() {
        let got = caddyfile(&net(true, true, true), &["10.0.0.1/32".into()]);
        assert!(got.contains("{file./caddy-secrets/cf_token}"));
        assert!(!got.contains("cf_ABC") && !got.to_lowercase().contains("bearer"));
    }

    #[test]
    fn token_file_is_private_and_removed_when_not_needed() {
        let dir = std::env::temp_dir().join(format!("askbar-admin-cf-{}", ulid::Ulid::new()));
        sync_token_file(&dir, Some("tok_123")).unwrap();
        let f = dir.join("cf_token");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "tok_123");
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&f).unwrap().permissions()) & 0o777, 0o600);
        sync_token_file(&dir, Some("tok_456")).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "tok_456");
        sync_token_file(&dir, None).unwrap();
        assert!(!f.exists());
        sync_token_file(&dir, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
