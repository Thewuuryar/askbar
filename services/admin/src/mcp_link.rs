//! The admin's link to the MCP gateway's front (ADR 0028, ADR 0057).
//!
//! The front refuses every tool call until it is given a config, and holds it in memory only. Every 12 seconds the admin reads
//! its `GET /status` and pushes with `PUT /config` when it is unconfigured or runs another version (a hash of the config, so a
//! restarted front is seen at once). The config is the default bounded limits, the per-user permissions and the approval public
//! key with its version. Each push is recorded in the auditor first (counts and versions: no key, no user) and is not made if
//! that fails. A front that does not answer is a log line and a sentence for the Services page. `ASKBAR_MCP_GATEWAY_URL` unset
//! means no link is made (`main`).
//!
//! No private key, token or user is logged, returned or put in an error here.

use crate::link_client::LinkClient;
use crate::mcp::UserTools;
use crate::AdminState;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

/// The link's name, as `bootstrap` names it.
pub const LINK: &str = askbar_proto::links::ADMIN_MCP_GATEWAY;
/// How often the admin reads the front's status (ADR 0028: every 10 to 15 seconds).
const CHECK_EVERY: Duration = Duration::from_secs(12);

/// The front's admin listener, or `None` (no push): `ASKBAR_MCP_GATEWAY_URL`, `http://mcp-gateway-admin-host:8087` in the stack.
pub fn front_url() -> Option<String> {
    std::env::var("ASKBAR_MCP_GATEWAY_URL").ok().filter(|v| !v.trim().is_empty())
}

pub fn read_token(token_dir: &Path) -> std::io::Result<String> {
    crate::read_link_token(token_dir, LINK)
}

/// The front's `Config` (services/mcp-gateway/src/config.rs): the same fields in the same order, because the version is a hash
/// of the serialized content. A test compares it with the front's own.
#[derive(Debug, Clone, Serialize)]
pub struct McpConfig {
    pub fetch_max_bytes: u64,
    pub fetch_timeout_secs: u32,
    pub fetch_redirects: u32,
    pub tool_call_timeout_secs: u32,
    pub one_time_id_window_secs: u32,
    pub search_results: u32,
    pub remembered_urls: u32,
    pub tool_result_max_bytes: u64,
    pub approval_public_key: Option<String>,
    pub approval_key_version: u64,
    pub permissions: BTreeMap<String, UserTools>,
    #[serde(skip_serializing_if = "askbar_proto::pii::PiiConfig::is_default")]
    pub pii: askbar_proto::pii::PiiConfig,
}

impl McpConfig {
    /// The first 8 bytes of the SHA-256 of the content, as the front computes it.
    pub fn version(&self) -> u64 {
        let h = Sha256::digest(serde_json::to_vec(self).expect("a config serializes"));
        u64::from_be_bytes(h[..8].try_into().expect("8 bytes"))
    }
}

/// What the front should run, from `admin.db`.
pub fn build_config(conn: &Connection) -> McpConfig {
    let key = crate::approval_key::public_key(conn);
    McpConfig {
        fetch_max_bytes: crate::limits::get(conn, "fetch_max_bytes"),
        fetch_timeout_secs: crate::limits::get(conn, "fetch_timeout_secs") as u32,
        fetch_redirects: crate::limits::get(conn, "fetch_redirects") as u32,
        tool_call_timeout_secs: crate::limits::get(conn, "tool_call_timeout_secs") as u32,
        one_time_id_window_secs: crate::limits::get(conn, "one_time_id_window_secs") as u32,
        search_results: crate::limits::get(conn, "search_results") as u32,
        remembered_urls: crate::limits::get(conn, "remembered_urls") as u32,
        tool_result_max_bytes: crate::limits::get(conn, "tool_result_max_bytes"),
        approval_public_key: key.as_ref().map(|(_, k)| k.iter().map(|b| format!("{b:02x}")).collect()),
        approval_key_version: key.map_or(0, |(v, _)| v),
        permissions: crate::mcp::permissions_config(conn).unwrap_or_default(),
        pii: crate::privacy::read(conn),
    }
}

/// The front's `GET /status`, the fields read here.
#[derive(Clone, Deserialize)]
struct Status {
    config_version: Option<u64>,
    boot_id: String,
    #[serde(default)]
    calls: BTreeMap<String, u64>,
    #[serde(default)]
    last_search: Option<LastSearch>,
}

/// How the front's last search to `searxng` went: unix seconds and `ok` or an error code word. No text.
#[derive(Clone, Serialize, Deserialize)]
pub struct LastSearch {
    pub at: i64,
    pub outcome: String,
}

/// How a check went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sync {
    /// The front runs this config already.
    Current,
    /// The config was pushed, under this version.
    Pushed(u64),
    /// The front did not answer.
    Unreachable,
    /// It was not pushed, and why (a sentence for a person): the record could not be made.
    NotReady(String),
    /// The front refused the config.
    Refused(u16),
}

pub struct McpLink {
    link: LinkClient,
    /// The last status the front answered (kept in memory only).
    last: std::sync::Mutex<Option<Status>>,
}

impl McpLink {
    pub fn new(base: &str, token: String) -> Arc<McpLink> {
        Arc::new(McpLink {
            last: Default::default(),
            link: LinkClient::new(base, token, LINK, "mcp-gateway", askbar_proto::paths::MCP_GATEWAY_V1, Duration::from_secs(15)),
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

    /// The config version and the call counts by outcome of the last status the front answered.
    pub fn reported(&self) -> (Option<u64>, BTreeMap<String, u64>) {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone().map(|s| (s.config_version, s.calls)).unwrap_or_default()
    }

    /// The front's last search to `searxng`, from the last status it answered.
    pub fn last_search(&self) -> Option<LastSearch> {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|s| s.last_search.clone())
    }

    /// `GET /status`: `None` if the front did not answer or did not take the token.
    async fn status_now(&self) -> Option<Status> {
        let result = self.link.request(reqwest::Method::GET, askbar_service::routes::STATUS, None).await;
        let ok = match &result {
            Ok((200, body)) => serde_json::from_slice::<Status>(body).ok(),
            _ => None,
        };
        self.link.online.store(ok.is_some(), Ordering::Relaxed);
        if let Some(st) = &ok {
            *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(st.clone());
            self.link.restarts.saw(&st.boot_id);
        } else {
            self.link.set_error(Some(match result {
                Ok((401, _)) => "The MCP gateway does not accept this admin's token.".into(),
                Ok(_) => "The MCP gateway answered with something this admin does not understand.".into(),
                Err(()) => "The MCP gateway does not answer.".into(),
            }));
        }
        ok
    }

    /// `PUT /config`, as an action of its own: recorded before it is made (counts and versions, no key, no user), and not made
    /// when it cannot be (ADR 0010).
    async fn put_config(&self, state: &AdminState, config: &McpConfig, version: u64) -> Result<(), Sync> {
        let body = serde_json::to_value(config).unwrap_or_default();
        let send = || self.link.request(reqwest::Method::PUT, askbar_service::routes::CONFIG, Some(&body));
        let sent = match state.actions.get() {
            None => Ok(send().await),
            Some(log) => {
                log.around(
                    "config_push",
                    "mcp-gateway",
                    serde_json::json!({
                        "target": "mcp-gateway", "version": version, "users": config.permissions.len(),
                        "approval_key_version": config.approval_key_version,
                    }),
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

    /// Read the front's status and push the config if it needs it.
    pub async fn sync(&self, state: &AdminState) -> Sync {
        let config = build_config(state.db.lock().await.conn());
        let version = config.version();
        let Some(status) = self.status_now().await else { return Sync::Unreachable };
        if status.config_version == Some(version) {
            self.link.set_error(None);
            return Sync::Current;
        }
        match self.put_config(state, &config, version).await {
            Ok(()) => {
                self.link.set_error(None);
                tracing::info!(version, "pushed the config to the MCP gateway");
                Sync::Pushed(version)
            }
            Err(out) => {
                self.link.set_error(Some(match &out {
                    Sync::NotReady(why) => why.clone(),
                    Sync::Refused(status) => format!("The MCP gateway refused the config (it answered {status})."),
                    _ => "The MCP gateway did not answer the config.".into(),
                }));
                out
            }
        }
    }

    /// Check every 12 seconds, and at once when something changed, for as long as the process runs.
    pub async fn run(self: Arc<Self>, state: Arc<AdminState>) {
        let mut was_online = None;
        loop {
            let online = !matches!(self.sync(&state).await, Sync::Unreachable);
            if was_online != Some(online) {
                if online {
                    tracing::info!("the MCP gateway answers");
                } else {
                    tracing::warn!("the MCP gateway does not answer; its config cannot be pushed and it refuses every call until it does");
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
