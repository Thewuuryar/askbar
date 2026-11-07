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
/// Where Caddy sees this install's own certificate and key (its mount of the shared secrets volume).
/// Used until a host name with a real certificate is set; see `self_cert`.
pub const SELF_CERT_PATH: &str = "/caddy-secrets/self.crt";
pub const SELF_KEY_PATH: &str = "/caddy-secrets/self.key";
/// Who may open the admin site while there is no host name, unless the allowlist setting names
/// others: loopback, the private ranges, Tailscale's range and their IPv6 counterparts (ADR 0007).
/// Caddy checks the connection's source, never a forwarded header.
pub const PRIVATE_RANGES: [&str; 8] = ["127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "::1/128", "fc00::/7", "fe80::/10"];
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

    /// Let Caddy make its admin socket in the directory the admin shares with it. The admin's image owns
    /// that directory (uid and group 10001); Caddy runs in group 10001 with every capability dropped
    /// ([ADR 0032](../../../docs/adr/0032-container-hardening.md)), so it can write there only through the
    /// group bit. Set on every start, because a volume made by an earlier release is 0755.
    pub fn share_socket_dir(&self) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        match self.socket.as_deref().and_then(Path::parent) {
            Some(dir) if dir.is_dir() => std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o770)),
            _ => Ok(()),
        }
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

/// The JSON body of Caddy's own 404: the same problem the servers send, so every 404 on the host
/// reads alike.
fn not_found_body() -> String {
    use askbar_proto::{http::default_detail, ErrorCode, Problem};
    let p = Problem::new(ErrorCode::NotFound, default_detail(ErrorCode::NotFound));
    String::from_utf8(p.to_json_bytes()).expect("a problem is UTF-8 JSON")
}

/// The three kinds of path on the one host: `/admin` and below go to the admin's unix socket (behind
/// `allowed`, when it is not empty); the client API (and `/api/v1`, which the server answers with
/// "update the app") goes to the server's client listener. The match names those two prefixes and
/// no wider one, because the server API (`/api/server`) is for the admin only and has no public
/// route; anything else gets a 404 problem.
fn routes(allowed: &[String]) -> String {
    use askbar_proto::paths;
    let (admin, client, old) = (paths::ADMIN, paths::CLIENT, paths::TOMBSTONE_V1);
    let mut out = format!("\t@admin path {admin} {admin}/*\n\thandle @admin {{\n");
    if !allowed.is_empty() {
        out.push_str(&format!("\t\t@outside not remote_ip {}\n\t\tabort @outside\n", allowed.join(" ")));
    }
    out.push_str(&format!("\t\treverse_proxy {ADMIN_UPSTREAM}\n\t}}\n"));
    out.push_str(&format!("\t@client path {client}/* {old}/*\n\thandle @client {{\n\t\treverse_proxy server:8080 {{\n\t\t\tflush_interval -1\n\t\t}}\n\t}}\n"));
    out.push_str(&format!("\thandle {{\n\t\theader Content-Type {}\n\t\trespond `{}` 404\n\t}}\n", askbar_proto::problem::CONTENT_TYPE, not_found_body()));
    out
}

/// The full Caddyfile for the current settings.
///
/// - **Host name and HTTPS on:** one site for that name, with a certificate from Let's Encrypt
///   through Cloudflare DNS. The admin allowlist applies only when it is switched on.
/// - **Otherwise (from the first run):** every address on port 443, with this install's own
///   certificate (`self_cert`), handed out whatever name the client asks for, so clients reach it
///   by address. The admin site is open to `allowed` if the allowlist is on, else to
///   [`PRIVATE_RANGES`]. Port 80 redirects to HTTPS.
///
/// `allowed` is the parsed allowlist, empty when the setting is off. Every interpolated value was
/// validated when it was saved (hostname, addresses); the token is never interpolated.
pub fn caddyfile(n: &Network, allowed: &[String]) -> String {
    let mut out = format!("{{\n\tadmin {CADDY_ADMIN_LISTEN}\n");
    if n.tls_enabled && !n.hostname.is_empty() {
        out.push_str(&format!("\tacme_dns cloudflare {{file.{CADDY_TOKEN_PATH}}}\n"));
        if n.acme_staging {
            out.push_str(&format!("\tacme_ca {STAGING_CA}\n"));
        }
        out.push_str("}\n");
        out.push_str(&format!("\n{host} {{\n{routes}}}\n", host = n.hostname, routes = routes(allowed)));
    } else {
        let name = crate::self_cert::NAME;
        out.push_str(&format!("\tdefault_sni {name}\n\tfallback_sni {name}\n}}\n"));
        let private: Vec<String> = PRIVATE_RANGES.iter().map(|r| r.to_string()).collect();
        let guard = if allowed.is_empty() { &private[..] } else { allowed };
        out.push_str(&format!("\n:443 {{\n\ttls {SELF_CERT_PATH} {SELF_KEY_PATH}\n{routes}}}\n", routes = routes(guard)));
        out.push_str("\nhttp:// {\n\tredir https://{host}{uri} permanent\n}\n");
    }
    out
}

/// The config Caddy starts with, before the admin has pushed one: its admin socket only. The admin
/// pushes the real one when it starts; `deploy/caddy/bootstrap.Caddyfile` says the same (a test
/// keeps them equal).
pub fn bootstrap_caddyfile() -> String {
    format!("{{\n\tadmin {CADDY_ADMIN_LISTEN}\n}}\n")
}

/// Write (or remove) the token file Caddy reads. Atomic, mode 0640: Caddy reads it through the admin's
/// group, with every capability dropped (ADR 0032).
fn sync_token_file(dir: &Path, token: Option<&str>) -> std::io::Result<()> {
    let target = dir.join("cf_token");
    match token {
        Some(t) => {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::create_dir_all(dir)?;
            let tmp = dir.join(".cf_token.tmp");
            let _ = std::fs::remove_file(&tmp);
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o640).open(&tmp)?;
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
    if !enabled {
        let Some(dir) = &link.secrets_dir else {
            return status(false, "There is no place for this install's own certificate (ASKBAR_CADDY_SECRETS_DIR is not set).");
        };
        if let Err(e) = crate::self_cert::ensure(dir) {
            return status(false, format!("Could not make this install's own certificate: {e}"));
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
            status(
                true,
                if enabled {
                    "Caddy is serving the configuration. Certificates are requested the first time it is used."
                } else {
                    "No host name yet: Caddy serves HTTPS on every address with this install's own certificate."
                },
            )
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

    const NOT_FOUND: &str = r#"{"type":"urn:askbar:problem:not_found","title":"Not found","status":404,"detail":"There is nothing at this address.","error":"not_found"}"#;

    #[test]
    fn staging_adds_the_test_ca_and_nothing_else() {
        let prod = caddyfile(&net(true, false, false), &[]);
        let staging = caddyfile(&net(true, true, false), &[]);
        assert!(staging.contains(&format!("\tacme_ca {STAGING_CA}\n")));
        assert_eq!(staging.replace(&format!("\tacme_ca {STAGING_CA}\n"), ""), prod);
    }

    #[test]
    fn without_a_host_name_every_address_gets_https_with_this_installs_certificate() {
        let got = caddyfile(&net(false, false, false), &[]);
        let want = format!(
            "{{
\tadmin unix//run/caddy/admin.sock|0660
\tdefault_sni askbar.internal
\tfallback_sni askbar.internal
}}

:443 {{
\ttls /caddy-secrets/self.crt /caddy-secrets/self.key
\t@admin path /admin /admin/*
\thandle @admin {{
\t\t@outside not remote_ip 127.0.0.0/8 10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 100.64.0.0/10 ::1/128 fc00::/7 fe80::/10
\t\tabort @outside
\t\treverse_proxy unix//run/admin/admin.sock
\t}}
\t@client path /api/client/* /api/v1/*
\thandle @client {{
\t\treverse_proxy server:8080 {{
\t\t\tflush_interval -1
\t\t}}
\t}}
\thandle {{
\t\theader Content-Type application/problem+json
\t\trespond `{NOT_FOUND}` 404
\t}}
}}

http:// {{
\tredir https://{{host}}{{uri}} permanent
}}
"
        );
        assert_eq!(got, want);
        // A host name saved with HTTPS off changes nothing: there is no certificate for it yet.
        assert!(!got.contains("askbar.orraitu.com") && !got.contains("acme_dns") && !got.contains("cf_token"));
        assert!(!got.contains("8081"), "there is no plain-HTTP first-run site any more");
    }

    #[test]
    fn without_a_host_name_the_admin_is_open_to_private_addresses_unless_the_allowlist_names_others() {
        let mine = vec!["192.168.1.0/24".to_string()];
        let on = caddyfile(&net(false, false, true), &mine);
        let (admin, rest) = on.split_once("\t@client path").unwrap();
        assert!(admin.contains("\t\t@outside not remote_ip 192.168.1.0/24\n\t\tabort @outside\n"), "{admin}");
        assert!(!rest.contains("remote_ip"), "the client API is open to paired apps: {rest}");
        let off = caddyfile(&net(false, false, false), &[]);
        assert!(off.contains(&format!("remote_ip {}\n", PRIVATE_RANGES.join(" "))), "{off}");
        assert_eq!(off.matches("remote_ip").count(), 1, "only the admin path is guarded");
    }

    #[test]
    fn compose_publishes_only_80_and_443_and_the_bootstrap_file_is_the_admin_socket_alone() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let compose = std::fs::read_to_string(root.join("compose.yaml")).unwrap();
        assert!(compose.contains("\"443:443\"") && compose.contains("\"80:80\""), "Caddy publishes 443 and 80");
        assert!(!compose.contains("8081") && !compose.contains("admin-setup"), "the plain-HTTP first-run site is gone");
        // The file Caddy starts with, before the admin has pushed anything.
        let boot = std::fs::read_to_string(root.join("deploy/caddy/bootstrap.Caddyfile")).unwrap();
        let body: String = boot.lines().filter(|l| !l.starts_with('#')).collect::<Vec<_>>().join("\n");
        assert_eq!(body.trim(), bootstrap_caddyfile().trim());
        assert!(caddyfile(&net(false, false, false), &[]).starts_with(bootstrap_caddyfile().trim_end_matches("}\n")), "every config keeps Caddy's admin socket");
    }

    #[test]
    fn the_token_never_appears_in_the_config() {
        let got = caddyfile(&net(true, true, true), &["10.0.0.1/32".into()]);
        assert!(got.contains("{file./caddy-secrets/cf_token}"));
        assert!(!got.contains("cf_ABC") && !got.to_lowercase().contains("bearer"));
    }

    #[test]
    fn one_host_serves_the_admin_the_client_api_and_a_404_for_everything_else() {
        let got = caddyfile(&net(true, false, false), &[]);
        let want = format!(
            "{{
\tadmin unix//run/caddy/admin.sock|0660
\tacme_dns cloudflare {{file./caddy-secrets/cf_token}}
}}

askbar.orraitu.com {{
\t@admin path /admin /admin/*
\thandle @admin {{
\t\treverse_proxy unix//run/admin/admin.sock
\t}}
\t@client path /api/client/* /api/v1/*
\thandle @client {{
\t\treverse_proxy server:8080 {{
\t\t\tflush_interval -1
\t\t}}
\t}}
\thandle {{
\t\theader Content-Type application/problem+json
\t\trespond `{NOT_FOUND}` 404
\t}}
}}
"
        );
        assert_eq!(got, want);
        assert!(!got.contains("admin.askbar.orraitu.com"), "there is no second host name");
        assert!(!got.contains("self.crt") && !got.contains("default_sni"), "a real certificate replaces this install's own");
    }

    #[test]
    fn the_server_api_is_never_proxied_and_gets_the_404_like_any_other_path() {
        for got in [caddyfile(&net(true, false, false), &[]), caddyfile(&net(false, false, false), &[])] {
            let matcher = got.lines().find(|l| l.trim_start().starts_with("@client")).unwrap();
            assert_eq!(matcher.trim(), "@client path /api/client/* /api/v1/*", "only the client API and the old prefix, never a wider /api/*");
            assert!(!got.contains("/api/server") && !got.contains("path /api/*"), "{got}");
            let proxied: Vec<&str> = got.lines().filter(|l| l.contains("reverse_proxy")).collect();
            // The admin socket and the client API; nothing else.
            assert_eq!(proxied.len(), 2, "{proxied:?}");
            assert_eq!(proxied.iter().filter(|l| l.contains("server:8080")).count(), 1, "{proxied:?}");
            assert!(!got.contains("8082") && !got.contains("172.30"), "the server API's address is not in the config");
        }
    }

    #[test]
    fn the_fixed_404_body_is_the_same_problem_the_servers_send() {
        let p = askbar_proto::Problem::new(askbar_proto::ErrorCode::NotFound, askbar_proto::http::default_detail(askbar_proto::ErrorCode::NotFound));
        assert_eq!(String::from_utf8(p.to_json_bytes()).unwrap(), NOT_FOUND);
    }

    #[test]
    fn the_allowlist_guards_the_admin_path_only() {
        let nets = vec!["192.168.1.0/24".to_string(), "10.0.0.5/32".to_string(), "fd00::/8".to_string()];
        let on = caddyfile(&net(true, false, true), &nets);
        let (admin, rest) = on.split_once("\t@client path").unwrap();
        assert!(admin.contains("\t\t@outside not remote_ip 192.168.1.0/24 10.0.0.5/32 fd00::/8\n\t\tabort @outside\n\t\treverse_proxy unix//run/admin/admin.sock"), "{admin}");
        assert!(!rest.contains("remote_ip"), "the client API is open to paired apps: {rest}");
        assert!(!caddyfile(&net(true, false, false), &[]).contains("remote_ip"), "off: no matcher with a host name");
    }

    #[test]
    fn the_socket_directory_is_shared_with_the_group_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("askbar-admin-run-{}", ulid::Ulid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = CaddyLink { socket: Some(dir.join("admin.sock")), secrets_dir: None };
        link.share_socket_dir().unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o770);
        CaddyLink { socket: None, secrets_dir: None }.share_socket_dir().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_file_is_owner_and_group_only_and_removed_when_not_needed() {
        let dir = std::env::temp_dir().join(format!("askbar-admin-cf-{}", ulid::Ulid::new()));
        sync_token_file(&dir, Some("tok_123")).unwrap();
        let f = dir.join("cf_token");
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "tok_123");
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&f).unwrap().permissions()) & 0o777, 0o640);
        sync_token_file(&dir, Some("tok_456")).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "tok_456");
        sync_token_file(&dir, None).unwrap();
        assert!(!f.exists());
        sync_token_file(&dir, None).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
