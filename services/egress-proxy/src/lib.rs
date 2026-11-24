//! `egress-proxy` (ADR 0056): the one way out of the stack. Default deny, one allowlist per source.
//!
//! - **[`policy`]** is the pure part: the host check, the address classes and the verdict. No I/O, no clock, no panic on any input.
//! - **[`proxy`]** is the listeners: it parses a request head, asks `policy`, resolves once, connects to that address and splices.
//! - **[`api`]** is the admin's side: `GET /status`, `PUT /config`, `POST /restart` under `/api/egress-proxy/v1`.
//!
//! The config is in memory only. It never sees content: HTTPS is a `CONNECT` tunnel with no interception.

#![forbid(unsafe_code)]

pub mod api;
pub mod policy;
pub mod proxy;

pub use api::{admin_links, admin_router, routes, token_dir};

use askbar_proto::egress::{normalize_host, Denied, EgressConfig, EgressStatus, Limits, SourceStats, RECENT_DENIED};
use askbar_service::audit_client::now_parts;
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpStream;

/// The admin's listener port, as in the compose file (`ASKBAR_EGRESS_PROXY_ADMIN_ADDR`).
pub const DEFAULT_PORT: u16 = 8086;

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;
/// Name and port to addresses. Called once per connection.
pub type Resolver = Arc<dyn Fn(String, u16) -> BoxFut<std::io::Result<Vec<SocketAddr>>> + Send + Sync>;
/// Opens the one connection to an address the proxy has validated.
pub type Connector = Arc<dyn Fn(SocketAddr) -> BoxFut<std::io::Result<TcpStream>> + Send + Sync>;

pub fn system_resolver() -> Resolver {
    Arc::new(|host, port| Box::pin(async move { Ok(tokio::net::lookup_host((host, port)).await?.collect()) }))
}

pub fn system_connector() -> Connector {
    Arc::new(|addr| Box::pin(TcpStream::connect(addr)))
}

/// `a=host:port,b=host:port` as pairs, for the sources and relays variables. A name is `a-z0-9-` and appears once.
pub fn split_pairs(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for item in raw.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let (name, addr) = item.split_once('=').ok_or("an item is not name=address")?;
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
            return Err("a source name is not lower case letters, digits and hyphens".into());
        }
        if out.iter().any(|(n, _)| n == name) {
            return Err("a source name appears twice".into());
        }
        out.push((name.to_string(), addr.trim().to_string()));
    }
    Ok(out)
}

#[derive(Default)]
struct State {
    total: usize,
    open: BTreeMap<String, usize>,
    stats: BTreeMap<String, SourceStats>,
    denied: VecDeque<Denied>,
}

/// What the listeners and the API share.
pub struct Proxy {
    config: RwLock<Option<Arc<EgressConfig>>>,
    sources: Vec<String>,
    resolver: Resolver,
    connector: Connector,
    boot_id: BootId,
    pub(crate) restarter: Restarter,
    state: Mutex<State>,
}

/// One open tunnel; dropping it frees the slot.
pub(crate) struct Slot {
    proxy: Arc<Proxy>,
    source: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut st = self.proxy.state.lock().unwrap_or_else(|e| e.into_inner());
        st.total = st.total.saturating_sub(1);
        if let Some(n) = st.open.get_mut(&self.source) {
            *n = n.saturating_sub(1);
        }
    }
}

impl Proxy {
    /// `sources` are the names that have a listener; a config naming another is refused.
    pub fn new(sources: Vec<String>, resolver: Resolver, connector: Connector, boot_id: BootId, restarter: Restarter) -> Arc<Proxy> {
        Arc::new(Proxy { config: RwLock::new(None), sources, resolver, connector, boot_id, restarter, state: Mutex::default() })
    }

    /// The config in force: empty (everything denied, default limits) until the first push.
    pub(crate) fn config(&self) -> Arc<EgressConfig> {
        self.config.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
    }

    /// Replaces the whole config and returns its version. The API checks it first with [`Proxy::check`].
    pub fn apply(&self, c: EgressConfig) -> u64 {
        let v = c.version();
        *self.config.write().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(c));
        v
    }

    /// The first thing wrong with a pushed config, named without any value from it.
    pub fn check(&self, c: &EgressConfig) -> Result<(), String> {
        c.limits.check().map_err(|f| format!("the limit {f} is out of its bounds"))?;
        if c.sources.keys().any(|s| !self.sources.contains(s)) {
            return Err("a source has no listener on this proxy".into());
        }
        let bad_host = c.sources.values().flat_map(|r| &r.entries).any(|e| !e.any_public && normalize_host(&e.host).is_err());
        if bad_host {
            return Err("an entry's host is not one the proxy can match".into());
        }
        Ok(())
    }

    pub fn status(&self) -> EgressStatus {
        let cfg = self.config.read().unwrap_or_else(|e| e.into_inner()).clone();
        let st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        EgressStatus {
            configured: cfg.is_some(),
            version: cfg.map(|c| c.version()),
            boot_id: self.boot_id.as_str().to_string(),
            open_tunnels: st.total as u32,
            sources: self.sources.iter().map(|s| (s.clone(), st.stats.get(s).cloned().unwrap_or_default())).collect(),
            recent_denied: st.denied.iter().cloned().collect(),
        }
    }

    /// A tunnel slot for `source`, or none when a cap in `lim` is reached.
    pub(crate) fn slot(self: &Arc<Self>, source: &str, lim: &Limits) -> Option<Slot> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let mine = st.open.get(source).copied().unwrap_or(0);
        if st.total >= lim.max_tunnels as usize || mine >= lim.max_tunnels_per_source as usize {
            return None;
        }
        st.total += 1;
        *st.open.entry(source.to_string()).or_default() += 1;
        Some(Slot { proxy: self.clone(), source: source.to_string() })
    }

    /// The log line and the count for an allowed connection. Only the source and the host name, as validated.
    pub(crate) fn note_allowed(&self, source: &str, host: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stats.entry(source.to_string()).or_default().allowed += 1;
        tracing::info!(source, host, verdict = "allow", "egress");
    }

    /// The log line, the count and the `recent_denied` entry for a refusal. `host` is as validated, or `invalid`.
    pub(crate) fn note_denied(&self, source: &str, host: &str, port: Option<u16>, reason: &str) {
        {
            let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
            *st.stats.entry(source.to_string()).or_default().denied.entry(reason.to_string()).or_default() += 1;
            st.denied.push_front(Denied { source: source.into(), host: host.into(), port, reason: reason.into(), at: now_parts().0 });
            st.denied.truncate(RECENT_DENIED);
        }
        tracing::info!(source, host, verdict = "deny", reason, "egress");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_are_names_and_addresses() {
        assert_eq!(split_pairs("a=h:1, b=h:2,").unwrap(), [("a".into(), "h:1".into()), ("b".into(), "h:2".into())]);
        assert!(split_pairs("").unwrap().is_empty());
        for bad in ["a", "=h:1", "A=h:1", "a=h:1,a=h:2", "a b=h:1"] {
            assert!(split_pairs(bad).is_err(), "{bad}");
        }
    }
}
