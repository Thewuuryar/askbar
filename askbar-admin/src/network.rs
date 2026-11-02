//! Network settings: the public hostname, the Cloudflare token for certificates, the optional
//! allowlist of addresses that may open the admin site, and the Let's Encrypt staging switch.
//! None of this needs a restart; the Caddy step turns it into proxy config.

use crate::web::{err, Failure};
use crate::AdminState;
use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Network {
    pub tls_enabled: bool,
    /// The API's name, for example `askbar.example.com`. The admin site is `admin.` plus this.
    pub hostname: String,
    /// Space- or comma-separated addresses and CIDRs allowed to open the admin site.
    pub admin_cidr: String,
    pub acme_staging: bool,
    /// When off (the default) anyone who can reach the admin site may try to log in.
    #[serde(default)]
    pub allowlist_enabled: bool,
}

pub fn load(conn: &rusqlite::Connection) -> rusqlite::Result<Network> {
    conn.query_row(
        "SELECT tls_enabled, hostname, allowlist, staging_ca, allowlist_enabled FROM network_settings WHERE id = 1",
        [],
        |r| {
            Ok(Network {
                tls_enabled: r.get::<_, i64>(0)? != 0,
                hostname: r.get(1)?,
                admin_cidr: r.get(2)?,
                acme_staging: r.get::<_, i64>(3)? != 0,
                allowlist_enabled: r.get::<_, i64>(4)? != 0,
            })
        },
    )
}

/// The encrypted token as stored, if any.
pub fn stored_token(conn: &rusqlite::Connection) -> Option<Vec<u8>> {
    conn.query_row("SELECT cloudflare_token FROM network_settings WHERE id = 1", [], |r| r.get::<_, Option<Vec<u8>>>(0)).ok().flatten().filter(|v| !v.is_empty())
}

/// A DNS name: dot-separated labels of letters, digits and hyphens. No scheme, port or path.
pub fn valid_hostname(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 || !h.contains('.') {
        return false;
    }
    h.split('.').all(|l| {
        !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    })
}

/// Parse `1.2.3.0/24`, `::1/128` or a bare address into canonical CIDR text.
fn parse_cidr(item: &str) -> Option<String> {
    let (addr, prefix) = match item.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (item, None),
    };
    let ip: IpAddr = addr.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix: u8 = match prefix {
        Some(p) => p.parse().ok()?,
        None => max,
    };
    (prefix <= max).then(|| format!("{ip}/{prefix}"))
}

/// Normalise a list of addresses and CIDRs; `None` if any item is invalid or the list is empty.
pub fn parse_cidr_list(text: &str) -> Option<Vec<String>> {
    let items: Vec<&str> = text.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).collect();
    if items.is_empty() {
        return None;
    }
    items.into_iter().map(parse_cidr).collect()
}

/// Cloudflare tokens are plain alphanumerics with `_` and `-`. Anything else could break out of a config file.
pub fn valid_token(t: &str) -> bool {
    (20..=200).contains(&t.len()) && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

#[derive(Serialize)]
pub struct NetworkView {
    #[serde(flatten)]
    network: Network,
    has_token: bool,
    /// What new pairing strings will point at.
    public_url: String,
    /// How the last attempt to configure the HTTPS front end went.
    proxy: crate::caddy::ProxyStatus,
}

fn view(state: &AdminState, conn: &rusqlite::Connection) -> rusqlite::Result<NetworkView> {
    Ok(NetworkView {
        network: load(conn)?,
        has_token: stored_token(conn).is_some(),
        public_url: crate::api::public_url(conn),
        proxy: state.proxy.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    })
}

pub async fn get_network(State(state): State<Arc<AdminState>>) -> Result<Json<NetworkView>, StatusCode> {
    let db = state.db.lock().await;
    view(&state, db.conn()).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Deserialize)]
pub struct NetworkInput {
    #[serde(flatten)]
    network: Network,
    /// Write-only. Missing or empty keeps the stored token.
    #[serde(default)]
    cloudflare_token: Option<String>,
    #[serde(default)]
    clear_token: bool,
}

fn bad(msg: &str) -> Failure {
    err(StatusCode::BAD_REQUEST, msg)
}

/// Validate and store the settings.
async fn save(state: &AdminState, input: NetworkInput) -> Result<(), Failure> {
    let mut n = input.network;
    n.hostname = n.hostname.trim().to_ascii_lowercase();
    if !n.hostname.is_empty() && !valid_hostname(&n.hostname) {
        return Err(bad("The hostname must be a plain DNS name such as askbar.example.com (no https://, port or path)."));
    }
    let cidrs = if n.admin_cidr.trim().is_empty() {
        None
    } else {
        Some(parse_cidr_list(&n.admin_cidr).ok_or_else(|| bad("Allowed addresses must be addresses or CIDRs such as 192.168.1.0/24, separated by spaces or commas."))?)
    };
    n.admin_cidr = cidrs.as_ref().map(|c| c.join(" ")).unwrap_or_default();
    let new_token = input.cloudflare_token.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if let Some(t) = new_token {
        if !valid_token(t) {
            return Err(bad("That does not look like a Cloudflare API token (letters, digits, - and _ only)."));
        }
    }

    let db = state.db.lock().await;
    let token_after = match (new_token.is_some(), input.clear_token) {
        (true, _) => true,
        (false, true) => false,
        (false, false) => stored_token(db.conn()).is_some(),
    };
    if n.tls_enabled {
        if n.hostname.is_empty() {
            return Err(bad("A hostname is required to turn on HTTPS."));
        }
        if !token_after {
            return Err(bad("A Cloudflare API token is required to turn on HTTPS."));
        }
    }
    if n.allowlist_enabled && cidrs.is_none() {
        return Err(bad("Add at least one address or network before restricting the admin site to an allowlist (or you would lock yourself out)."));
    }

    let fail = |_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the network settings.");
    let tx = db.conn().unchecked_transaction().map_err(fail)?;
    tx.execute(
        "UPDATE network_settings SET tls_enabled = ?1, hostname = ?2, allowlist = ?3, staging_ca = ?4, allowlist_enabled = ?5 WHERE id = 1",
        rusqlite::params![n.tls_enabled, n.hostname, n.admin_cidr, n.acme_staging, n.allowlist_enabled],
    )
    .map_err(fail)?;
    if let Some(t) = new_token {
        tx.execute("UPDATE network_settings SET cloudflare_token = ?1 WHERE id = 1", [state.secrets.encrypt(t)]).map_err(fail)?;
    } else if input.clear_token {
        tx.execute("UPDATE network_settings SET cloudflare_token = NULL WHERE id = 1", []).map_err(fail)?;
    }
    tx.commit().map_err(fail)?;
    Ok(())
}

pub async fn put_network(State(state): State<Arc<AdminState>>, Json(input): Json<NetworkInput>) -> Result<Json<NetworkView>, Failure> {
    save(&state, input).await?;
    // Saved. Now tell Caddy; if that fails the settings stay and the result says why.
    crate::caddy::apply(&state).await;
    let db = state.db.lock().await;
    view(&state, db.conn()).map(Json).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Saved, but could not read the settings back."))
}

/// Push the saved settings to Caddy again (for example after starting it).
pub async fn apply_network(State(state): State<Arc<AdminState>>) -> Json<crate::caddy::ProxyStatus> {
    Json(crate::caddy::apply(&state).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames() {
        for ok in ["askbar.orraitu.com", "a-b.example.org", "nas.local", "x1.y2.z3"] {
            assert!(valid_hostname(ok), "{ok}");
        }
        for bad in ["", "localhost", "https://askbar.example.com", "askbar.example.com:8443", "askbar.example.com/x", "-a.example.com", "a..b", "a b.example.com", "ex_ample.com", &format!("{}.com", "a".repeat(64))] {
            assert!(!valid_hostname(bad), "{bad}");
        }
    }

    #[test]
    fn cidrs() {
        assert_eq!(parse_cidr_list("192.168.1.0/24").unwrap(), ["192.168.1.0/24"]);
        assert_eq!(parse_cidr_list("10.0.0.5, ::1 fd00::/8").unwrap(), ["10.0.0.5/32", "::1/128", "fd00::/8"]);
        for bad in ["", "   ", "192.168.1.0/33", "192.168.1/24", "nope", "10.0.0.1/", "10.0.0.1/-1", "::1/129"] {
            assert!(parse_cidr_list(bad).is_none(), "{bad:?}");
        }
        assert!(parse_cidr_list("10.0.0.0/8 junk").is_none(), "one bad item spoils the list");
    }

    #[test]
    fn tokens() {
        assert!(valid_token("abcdEFGH1234_-abcdEFGH1234"));
        for bad in ["short", "has space in the middle 1234567890", "line\nbreak1234567890123456", "quote\"1234567890123456789", "brace}1234567890123456789", &"a".repeat(201)] {
            assert!(!valid_token(bad), "{bad:?}");
        }
    }
}
