//! The admin's link to the `egress-proxy` (ADR 0056).
//!
//! The proxy denies everything until it is given an allowlist per source, and holds it in memory only. The admin derives the
//! allowlists (never typed): `model-gateway` gets its providers' base URLs, `alerter` its ntfy server, its mail server (the
//! `relay` entry) and GitHub, `cpa` the fixed [`CPA_EGRESS_HOSTS`], and, when the proxy reports them, `searxng` (the fixed engine hosts) and `fetch` (`any_public`, ports 443 and 80). Every 12 seconds it reads the proxy's `GET /status` and
//! pushes with `PUT /config` when the proxy is unconfigured or runs another version (a hash of the config, so a restarted
//! proxy is seen at once). No entry is `allow_private` unless the administrator approved that destination for that source
//! (`egress_approvals`, P4.4a): an approval is the only way a private address is allowed, and its change is an audited action. Each push is recorded in the auditor first (source, count, version:
//! no host) and is not made if that fails. A proxy that does not answer is a log line and a sentence for the Services page; it
//! never blocks saving settings, since the push retries. `ASKBAR_EGRESS_PROXY_URL` unset means no link is made (`main`).
//!
//! No host, topic or token is logged, returned or put in an error here.

use crate::alerter_link::{read_email, read_ntfy};
use crate::link_client::LinkClient;
use crate::web::{err, Failure};
use crate::AdminState;
use askbar_proto::egress::{normalize_host, Host};
use askbar_proto::egress::{Denied, EgressConfig, EgressStatus, Entry, SourceRules};
use axum::{extract::Query, extract::State, http::StatusCode, Json};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The link's name, as `bootstrap` names it.
pub const LINK: &str = askbar_proto::links::ADMIN_EGRESS_PROXY;
/// How often the admin reads the proxy's status (ADR 0028: every 10 to 15 seconds).
const CHECK_EVERY: Duration = Duration::from_secs(12);

/// The proxy's address, or `None` (no push): `ASKBAR_EGRESS_PROXY_URL`, `http://egress-admin-host:8086` in the stack.
pub fn proxy_url() -> Option<String> {
    std::env::var("ASKBAR_EGRESS_PROXY_URL").ok().filter(|v| !v.trim().is_empty())
}

/// The token this admin sends the proxy, made by `bootstrap` (ADR 0049).
pub fn token_file(token_dir: &Path) -> PathBuf {
    token_dir.join("call").join(format!("{LINK}.token"))
}

pub fn read_token(token_dir: &Path) -> std::io::Result<String> {
    crate::read_link_token(token_dir, LINK)
}

/// The entry for the host and port of a URL (`https` 443, `http` 80 and `plain_http`). `None` for anything else, or a URL with
/// userinfo: an entry the proxy would refuse is not pushed.
fn entry_of_url(url: &str) -> Option<Entry> {
    let (plain, rest) = match url.trim().split_once("://")? {
        (s, r) if s.eq_ignore_ascii_case("https") => (false, r),
        (s, r) if s.eq_ignore_ascii_case("http") => (true, r),
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('@') {
        return None;
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !p.contains(']') => (h, p.parse().ok()?),
        _ => (authority, if plain { 80 } else { 443 }),
    };
    (!host.is_empty()).then(|| Entry { host: host.to_ascii_lowercase(), port, plain_http: plain, ..plain_entry() })
}

fn plain_entry() -> Entry {
    Entry { host: String::new(), port: 443, plain_http: false, allow_private: false, relay: false, any_public: false }
}

fn rules(mut entries: Vec<Entry>) -> SourceRules {
    entries.sort_by(|a, b| (&a.host, a.port, a.plain_http, a.relay).cmp(&(&b.host, b.port, b.plain_http, b.relay)));
    entries.dedup();
    SourceRules { entries }
}

/// The sources the proxy has a listener for.
const SOURCES: [&str; 3] = ["model-gateway", "alerter", "cpa"];
const MAX_NOTE_CHARS: usize = 200;

/// One approved private destination, as stored and as the routes show it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub source: String,
    pub host: String,
    #[serde(default = "https_port")]
    pub port: u16,
    #[serde(default)]
    pub plain_http: bool,
    #[serde(default)]
    pub note: String,
}

fn https_port() -> u16 {
    443
}

pub fn read_approvals(conn: &Connection) -> rusqlite::Result<Vec<Approval>> {
    let mut st = conn.prepare("SELECT source, host, port, plain_http, note FROM egress_approvals ORDER BY source, host, port")?;
    let rows =
        st.query_map([], |r| Ok(Approval { source: r.get(0)?, host: r.get(1)?, port: r.get(2)?, plain_http: r.get(3)?, note: r.get(4)? }))?;
    rows.collect()
}

/// An approval's destination becomes `allow_private` on the source's entry for that host and port (the derived one, so a LAN mail
/// relay keeps `relay`), or is a new entry.
fn approve(entries: &mut Vec<Entry>, a: &Approval) {
    let mut found = false;
    for e in entries.iter_mut().filter(|e| e.host == a.host && e.port == a.port) {
        (e.allow_private, e.plain_http, found) = (true, e.plain_http || a.plain_http, true);
    }
    if !found {
        entries.push(Entry { host: a.host.clone(), port: a.port, plain_http: a.plain_http, allow_private: true, ..plain_entry() });
    }
}

/// What the proxy should run, from `admin.db`. `searxng` and `fetch` (P5A.19) are included only when the proxy reports a listener
/// for them in `status`: it refuses a config naming a source it does not have.
pub fn build_config(conn: &Connection, secrets: &crate::secrets::AdminSecrets, status: &EgressStatus) -> EgressConfig {
    let gateway = crate::server_link::build_snapshot(conn)
        .map(|s| s.providers.iter().filter_map(|p| entry_of_url(&p.base_url)).collect())
        .unwrap_or_default();
    let mut alerter: Vec<Entry> = read_ntfy(conn, secrets).and_then(|n| entry_of_url(&n.server)).into_iter().collect();
    if let Some(mail) = read_email(conn, secrets) {
        alerter.push(Entry { host: mail.host.to_ascii_lowercase(), port: mail.port, relay: true, ..plain_entry() });
    }
    alerter.push(Entry { host: "api.github.com".into(), ..plain_entry() });
    let cpa = crate::constants::CPA_EGRESS_HOSTS.iter().map(|h| Entry { host: (*h).into(), ..plain_entry() }).collect();
    let searxng = crate::constants::SEARXNG_EGRESS_HOSTS.iter().map(|h| Entry { host: (*h).into(), ..plain_entry() }).collect();
    // The fetch client sends http targets in absolute form (no CONNECT), so port 80 needs `plain_http`. `any_public` takes no host.
    let fetch = vec![Entry { any_public: true, ..plain_entry() }, Entry { any_public: true, port: 80, plain_http: true, ..plain_entry() }];
    let approvals = read_approvals(conn).unwrap_or_default();
    EgressConfig {
        sources: [("model-gateway", gateway), ("alerter", alerter), ("cpa", cpa), ("searxng", searxng), ("fetch", fetch)]
            .into_iter()
            .filter(|(name, _)| SOURCES[..3].contains(name) || status.sources.contains_key(*name))
            .map(|(name, mut entries)| {
                approvals.iter().filter(|a| a.source == name).for_each(|a| approve(&mut entries, a));
                (name.to_string(), rules(entries))
            })
            .collect(),
        limits: crate::constants::EGRESS_LIMITS,
    }
}

type St = State<Arc<AdminState>>;

/// `GET /api/admin/v1/egress/approvals`.
pub async fn list_approvals(State(state): St) -> Result<Json<Vec<Approval>>, Failure> {
    let db = state.db.lock().await;
    read_approvals(db.conn()).map(Json).map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "The admin could not read the approvals."))
}

/// A source and host the proxy would take, the host as it normalizes it (an IPv6 literal bracketed).
fn checked(source: &str, host: &str, port: u16) -> Result<String, Failure> {
    if !SOURCES.contains(&source) {
        return Err(err(StatusCode::BAD_REQUEST, "There is no such source."));
    }
    if port == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "The port is not valid."));
    }
    match normalize_host(host) {
        Ok(Host::Name(n)) => Ok(n),
        Ok(Host::Ip(std::net::IpAddr::V4(a))) => Ok(a.to_string()),
        Ok(Host::Ip(std::net::IpAddr::V6(a))) => Ok(format!("[{a}]")),
        Err(_) => Err(err(StatusCode::BAD_REQUEST, "The host is not valid: a plain ASCII name or an IP address, with no port.")),
    }
}

/// `PUT /api/admin/v1/egress/approvals`: approve a private destination for one source (replacing the same one). The record of the
/// change is made by the audit layer first; the proxy gets the new allowlists at once.
pub async fn put_approval(
    State(state): St,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    askbar_proto::http::ApiJson(mut a): askbar_proto::http::ApiJson<Approval>,
) -> Result<Json<Approval>, Failure> {
    a.host = checked(&a.source, &a.host, a.port)?;
    if a.note.chars().count() > MAX_NOTE_CHARS {
        return Err(err(StatusCode::BAD_REQUEST, "The note is too long."));
    }
    if let Some(axum::Extension(n)) = &notes {
        n.add("source", a.source.clone().into());
        n.add("host", a.host.clone().into());
        n.add("port", a.port.into());
    }
    state
        .db
        .lock()
        .await
        .conn()
        .execute(
            "INSERT OR REPLACE INTO egress_approvals (source, host, port, plain_http, note) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![a.source, a.host, a.port, a.plain_http, a.note],
        )
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "The admin could not store the approval."))?;
    if let Some(e) = state.egress.get() {
        e.mark_dirty();
    }
    Ok(Json(a))
}

/// `DELETE /api/admin/v1/egress/approvals?source=&host=&port=`: withdraw one (port defaults to 443).
pub async fn delete_approval(
    State(state): St,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    Query(q): Query<HashMap<String, String>>,
) -> Result<StatusCode, Failure> {
    let port = match q.get("port") {
        None => 443,
        Some(p) => p.parse().map_err(|_| err(StatusCode::BAD_REQUEST, "The port is not valid."))?,
    };
    let host = checked(q.get("source").map_or("", String::as_str), q.get("host").map_or("", String::as_str), port)?;
    let source = &q["source"];
    if let Some(axum::Extension(n)) = &notes {
        n.add("source", source.clone().into());
        n.add("host", host.clone().into());
        n.add("port", port.into());
    }
    let gone = state
        .db
        .lock()
        .await
        .conn()
        .execute("DELETE FROM egress_approvals WHERE source = ?1 AND host = ?2 AND port = ?3", params![source, host, port])
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "The admin could not remove the approval."))?;
    if gone == 0 {
        return Err(err(StatusCode::NOT_FOUND, "There is no such approval."));
    }
    if let Some(e) = state.egress.get() {
        e.mark_dirty();
    }
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/admin/v1/egress/denied`: the proxy's recent denied attempts from the last status the admin read, newest first. A
/// proxy that does not answer gives `online: false` and the reason, not an empty list.
pub async fn list_denied(State(state): St) -> Json<serde_json::Value> {
    Json(match state.egress.get() {
        None => serde_json::json!({ "online": false, "error": "No link to the egress proxy is set up.", "denied": [] }),
        Some(l) => serde_json::json!({ "online": l.online(), "error": l.last_error(), "denied": l.denied() }),
    })
}

/// How a check went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// The proxy runs this config already.
    Current,
    /// The config was pushed, under this version.
    Pushed(u64),
    /// The proxy did not answer.
    Unreachable,
    /// It was not pushed, and why (a sentence for a person): the record could not be made.
    NotReady(String),
    /// The proxy refused the config.
    Refused(u16),
}

pub struct EgressLink {
    link: LinkClient,
    /// `recent_denied` of the last status the proxy answered (kept in memory only, like the proxy's own).
    denied: std::sync::Mutex<Vec<Denied>>,
}

impl EgressLink {
    pub fn new(base: &str, token: String) -> Arc<EgressLink> {
        Arc::new(EgressLink {
            denied: Default::default(),
            link: LinkClient::new(base, token, LINK, "egress-proxy", askbar_proto::paths::EGRESS_PROXY_V1, Duration::from_secs(15)),
        })
    }

    pub fn set_notifier(&self, n: Arc<crate::notify::Notifier>) {
        self.link.set_notifier(n);
    }

    pub fn online(&self) -> bool {
        self.link.online()
    }

    pub fn last_error(&self) -> Option<String> {
        self.link.last_error()
    }

    pub fn mark_dirty(&self) {
        self.link.mark_dirty();
    }

    pub fn denied(&self) -> Vec<Denied> {
        self.denied.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `GET /status`: `None` if the proxy did not answer or did not take the token.
    pub async fn status_now(&self) -> Option<EgressStatus> {
        let result = self.link.request(reqwest::Method::GET, askbar_service::routes::STATUS, None).await;
        let ok = match &result {
            Ok((200, body)) => serde_json::from_slice::<EgressStatus>(body).ok(),
            _ => None,
        };
        self.link.online.store(ok.is_some(), Ordering::Relaxed);
        if let Some(st) = &ok {
            *self.denied.lock().unwrap_or_else(|e| e.into_inner()) = st.recent_denied.clone();
            self.link.restarts.saw(&st.boot_id);
        } else {
            self.link.set_error(Some(match result {
                Ok((401, _)) => "The egress proxy does not accept this admin's token.".into(),
                Ok(_) => "The egress proxy answered with something this admin does not understand.".into(),
                Err(()) => "The egress proxy does not answer.".into(),
            }));
        }
        ok
    }

    /// `PUT /config`, as an action of its own: recorded before it is made (source, count and version, no host), and not made
    /// when it cannot be (ADR 0010).
    async fn put_config(&self, state: &AdminState, config: &EgressConfig, version: u64) -> Result<(), Sync> {
        let body = serde_json::to_value(config).unwrap_or_default();
        let send = || self.link.request(reqwest::Method::PUT, askbar_service::routes::CONFIG, Some(&body));
        let counts: serde_json::Map<String, serde_json::Value> =
            config.sources.iter().map(|(name, r)| (name.clone(), r.entries.len().into())).collect();
        let sent = match state.actions.get() {
            None => Ok(send().await),
            Some(log) => {
                log.around(
                    "config_push",
                    "egress-proxy",
                    serde_json::json!({ "target": "egress-proxy", "version": version, "sources": counts }),
                    send(),
                    |r| serde_json::json!({ "status": r.as_ref().ok().map(|(s, _)| *s) }),
                )
                .await
            }
        };
        match sent {
            Err(why) => Err(Sync::NotReady(why.sentence().to_string())),
            Ok(Ok((200, _))) => Ok(()),
            Ok(Ok((status, _))) => Err(Sync::Refused(status)),
            Ok(Err(())) => {
                self.link.online.store(false, Ordering::Relaxed);
                Err(Sync::Unreachable)
            }
        }
    }

    /// Read the proxy's status and push the allowlists if it needs them.
    pub async fn sync(&self, state: &AdminState) -> Sync {
        let Some(status) = self.status_now().await else { return Sync::Unreachable };
        let config = {
            let db = state.db.lock().await;
            build_config(db.conn(), &state.secrets, &status)
        };
        let version = config.version();
        if status.configured && status.version == Some(version) {
            self.link.set_error(None);
            return Sync::Current;
        }
        match self.put_config(state, &config, version).await {
            Ok(()) => {
                self.link.set_error(None);
                tracing::info!(version, "pushed the allowlists to the egress proxy");
                Sync::Pushed(version)
            }
            Err(out) => {
                self.link.set_error(Some(match &out {
                    Sync::NotReady(why) => why.clone(),
                    Sync::Refused(status) => format!("The egress proxy refused the allowlists (it answered {status})."),
                    _ => "The egress proxy did not answer the allowlists.".into(),
                }));
                out
            }
        }
    }

    /// Check every 12 seconds, and at once when something changed, for as long as the process runs.
    pub async fn run(self: Arc<Self>, state: Arc<AdminState>) {
        let mut was_online = None;
        loop {
            let outcome = self.sync(&state).await;
            let online = !matches!(outcome, Sync::Unreachable);
            if was_online != Some(online) {
                if online {
                    tracing::info!("the egress proxy answers");
                } else {
                    tracing::warn!(
                        "the egress proxy does not answer; allowlists cannot be pushed and nothing leaves the stack until it does"
                    );
                }
                was_online = Some(online);
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = self.link.dirty.notified() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_gives_its_host_and_port_and_plain_http_only_for_http() {
        let e = entry_of_url("https://API.Example.com/v1").unwrap();
        assert_eq!((e.host.as_str(), e.port, e.plain_http), ("api.example.com", 443, false));
        let e = entry_of_url("http://mock-llm:8765/v1").unwrap();
        assert_eq!((e.host.as_str(), e.port, e.plain_http), ("mock-llm", 8765, true));
        assert_eq!(entry_of_url("http://h/").unwrap().port, 80);
        for bad in ["ftp://h", "h:443", "https://u:p@h/", "https:///x"] {
            assert!(entry_of_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn the_defaults_are_within_the_proxys_bounds() {
        assert_eq!(crate::constants::EGRESS_LIMITS.check(), Ok(()));
    }
}
