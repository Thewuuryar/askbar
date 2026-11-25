//! The tool registry (ADR 0057): a tool name to the native handler that runs it. A name that is not registered is
//! refused before any span.

use crate::config::Config;
use crate::{fetch, search};
use askbar_proto::approval::ToolAction;
use askbar_proto::ErrorCode;
use askbar_service::audit_client::AuditClient;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Why a tool failed: the code, and for `fetch_refused` the stable rule that refused it (the problem's `reason` member).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolError {
    pub code: ErrorCode,
    pub reason: Option<&'static str>,
}

impl From<ErrorCode> for ToolError {
    fn from(code: ErrorCode) -> ToolError {
        ToolError { code, reason: None }
    }
}

pub type Outcome = Result<Value, ToolError>;
/// What a handler is given: the signed action (the call derefs to it) and who and what it is for. `audit` and `span_id`
/// (the call's own `tool` span) are for a tool that records more spans, as `fetch` does before each hop.
pub struct ToolCall {
    pub action: ToolAction,
    pub task_id: String,
    pub user: String,
    pub config: Arc<Config>,
    pub audit: Option<Arc<AuditClient>>,
    pub span_id: String,
}

impl std::ops::Deref for ToolCall {
    type Target = ToolAction;
    fn deref(&self) -> &ToolAction {
        &self.action
    }
}

pub type Handler = Box<dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = Outcome> + Send>> + Send + Sync>;

/// A tool: the schema of its arguments (listed by `GET /tools`) and its handler.
pub struct Registry(HashMap<&'static str, (Value, Handler)>, Arc<search::Remembered>);

impl Registry {
    /// The built-in tools. `search` goes to `searxng_addr` (`tool_not_available` without one); `fetch` goes through
    /// the egress proxy named by `ASKBAR_EGRESS_PROXY_URL` (direct when unset, as in the native smoke run).
    pub fn builtin(searxng_addr: Option<String>) -> Registry {
        let query = json!({"type": "object", "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 200}}});
        let url = json!({"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}});
        let store = Arc::<search::Remembered>::default(); // `fetch` reads what `search` remembers
        let proxy = std::env::var("ASKBAR_EGRESS_PROXY_URL").ok().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        let fetch = fetch::handler_for(fetch::client(proxy.as_deref()), store.clone());
        let mut r = Registry::new([("search", query, search::handler_for(searxng_addr, store.clone())), ("fetch", url, fetch)]);
        r.1 = store;
        r
    }

    /// A registry of the given handlers (the tests register their own).
    pub fn new(handlers: impl IntoIterator<Item = (&'static str, Value, Handler)>) -> Registry {
        Registry(handlers.into_iter().map(|(n, s, h)| (n, (s, h))).collect(), Default::default())
    }

    pub fn get(&self, tool: &str) -> Option<&Handler> {
        self.0.get(tool).map(|t| &t.1)
    }

    pub fn schema(&self, tool: &str) -> Option<&Value> {
        self.0.get(tool).map(|t| &t.0)
    }

    /// How the last search to `searxng` went, if the built-in `search` ran.
    pub fn last_search(&self) -> Option<crate::status::LastSearch> {
        self.1.last_search()
    }

    pub fn has(&self, tool: &str) -> bool {
        self.0.contains_key(tool)
    }
}

/// Boxes an async fn or closure as a [`Handler`].
pub fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(ToolCall) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    Box::new(move |a| Box::pin(f(a)))
}
