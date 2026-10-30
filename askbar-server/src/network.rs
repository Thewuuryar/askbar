//! Network settings: the public hostname, the Cloudflare token for certificates,
//! which addresses may reach the admin site, and the Let's Encrypt staging switch.
//! None of this needs a restart; the Caddy step turns it into proxy config.

use crate::{admin::err, admin::Failure, AppState};
use axum::{extract::State, http::StatusCode, Json};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::{atomic::Ordering, Arc};

pub const TOKEN_KEY: &str = "cloudflare_token";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Network {
    pub tls_enabled: bool,
    pub hostname: String,
    /// Space- or comma-separated CIDRs (or single addresses) allowed to reach the admin site.
    pub admin_cidr: String,
    pub acme_staging: bool,
}

fn get(db: &crate::Db, key: &str) -> rusqlite::Result<Option<String>> {
    db.conn().query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()
}

pub fn load(db: &crate::Db) -> rusqlite::Result<Network> {
    Ok(Network {
        tls_enabled: get(db, "net.tls_enabled")?.as_deref() == Some("1"),
        hostname: get(db, "net.hostname")?.unwrap_or_default(),
        admin_cidr: get(db, "net.admin_cidr")?.unwrap_or_default(),
        acme_staging: get(db, "net.acme_staging")?.as_deref() == Some("1"),
    })
}

/// The encrypted token as stored (base64), if any.
pub fn stored_token(db: &crate::Db) -> Option<String> {
    get(db, TOKEN_KEY).ok().flatten().filter(|v| !v.is_empty())
}

pub fn has_token(db: &crate::Db) -> bool {
    get(db, TOKEN_KEY).ok().flatten().is_some_and(|v| !v.is_empty())
}

/// What goes into pairing strings: the HTTPS address when TLS is on, otherwise the configured fallback.
pub fn effective_public_url(db: &crate::Db, state: &AppState) -> String {
    match load(db) {
        Ok(n) if n.tls_enabled && !n.hostname.is_empty() => {
            format!("https://{}:{}", n.hostname, state.tls_api_port.load(Ordering::Relaxed))
        }
        _ => state.public_url.clone(),
    }
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

/// Normalise a list of CIDRs; `None` if any item is invalid or the list is empty.
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
    api_port: u16,
    admin_port: u16,
    /// What new pairing strings will point at.
    public_url: String,
    /// How the last attempt to configure the HTTPS front end went.
    proxy: crate::caddy::ProxyStatus,
}

fn view(state: &AppState, db: &crate::Db) -> rusqlite::Result<NetworkView> {
    Ok(NetworkView {
        network: load(db)?,
        has_token: has_token(db),
        api_port: state.tls_api_port.load(Ordering::Relaxed),
        admin_port: state.tls_admin_port.load(Ordering::Relaxed),
        public_url: effective_public_url(db, state),
        proxy: state.proxy.lock().unwrap_or_else(|e| e.into_inner()).clone(),
    })
}

pub async fn get_network(State(state): State<Arc<AppState>>) -> Result<Json<NetworkView>, StatusCode> {
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    view(&state, &db).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
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

/// Validate and store the settings. Synchronous so no database lock is held across an await.
fn save(state: &AppState, input: NetworkInput) -> Result<(), Failure> {
    let mut n = input.network;
    n.hostname = n.hostname.trim().to_ascii_lowercase();
    if !n.hostname.is_empty() && !valid_hostname(&n.hostname) {
        return Err(bad("The hostname must be a plain DNS name such as askbar.example.com (no https://, port or path)."));
    }
    let cidrs = if n.admin_cidr.trim().is_empty() {
        None
    } else {
        Some(parse_cidr_list(&n.admin_cidr).ok_or_else(|| bad("Allowed networks must be addresses or CIDRs such as 192.168.1.0/24, separated by spaces or commas."))?)
    };
    n.admin_cidr = cidrs.as_ref().map(|c| c.join(" ")).unwrap_or_default();
    let new_token = input.cloudflare_token.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if let Some(t) = new_token {
        if !valid_token(t) {
            return Err(bad("That does not look like a Cloudflare API token (letters, digits, - and _ only)."));
        }
    }

    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    let secrets = state.secrets();
    let token_after = match (new_token.is_some(), input.clear_token) {
        (true, _) => true,
        (false, true) => false,
        (false, false) => has_token(&db),
    };
    if n.tls_enabled {
        if n.hostname.is_empty() {
            return Err(bad("A hostname is required to turn on HTTPS."));
        }
        if cidrs.is_none() {
            return Err(bad("Say which networks may reach the admin site (for example 192.168.1.0/24) before turning on HTTPS."));
        }
        if !token_after {
            return Err(bad("A Cloudflare API token is required to turn on HTTPS."));
        }
    }

    let fail = |_| err(StatusCode::INTERNAL_SERVER_ERROR, "Could not save the network settings.");
    let tx = db.conn().unchecked_transaction().map_err(fail)?;
    let upsert = "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value";
    for (k, v) in [
        ("net.tls_enabled", if n.tls_enabled { "1" } else { "0" }),
        ("net.hostname", n.hostname.as_str()),
        ("net.admin_cidr", n.admin_cidr.as_str()),
        ("net.acme_staging", if n.acme_staging { "1" } else { "0" }),
    ] {
        tx.execute(upsert, rusqlite::params![k, v]).map_err(fail)?;
    }
    if let Some(t) = new_token {
        tx.execute(upsert, rusqlite::params![TOKEN_KEY, secrets.encrypt_b64(t)]).map_err(fail)?;
    } else if input.clear_token {
        tx.execute("DELETE FROM settings WHERE key = ?1", [TOKEN_KEY]).map_err(fail)?;
    }
    tx.commit().map_err(fail)?;
    Ok(())
}

pub async fn put_network(State(state): State<Arc<AppState>>, Json(input): Json<NetworkInput>) -> Result<Json<NetworkView>, Failure> {
    save(&state, input)?;
    // Saved. Now tell Caddy; if that fails the settings stay and the result says why.
    crate::caddy::apply(&state).await;
    let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
    view(&state, &db).map(Json).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "Saved, but could not read the settings back."))
}

/// Push the saved settings to Caddy again (for example after starting the tls profile).
pub async fn apply_network(State(state): State<Arc<AppState>>) -> Json<crate::caddy::ProxyStatus> {
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
