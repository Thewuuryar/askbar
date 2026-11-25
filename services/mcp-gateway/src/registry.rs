//! The tool registry (ADR 0057): a tool name to the native handler that runs it. `fetch` is registered with a
//! stub until P5A.15; a name that is not registered is refused before any span.

use crate::config::Config;
use crate::search;
use askbar_proto::approval::ToolAction;
use askbar_proto::ErrorCode;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type Outcome = Result<Value, ErrorCode>;
/// What a handler is given: the signed action (the call derefs to it) and who and what it is for.
pub struct ToolCall {
    pub action: ToolAction,
    pub task_id: String,
    pub user: String,
    pub config: Arc<Config>,
}

impl std::ops::Deref for ToolCall {
    type Target = ToolAction;
    fn deref(&self) -> &ToolAction {
        &self.action
    }
}

pub type Handler = Box<dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = Outcome> + Send>> + Send + Sync>;

/// A tool: the schema of its arguments (listed by `GET /tools`) and its handler.
pub struct Registry(HashMap<&'static str, (Value, Handler)>);

impl Registry {
    /// The built-in tools. `search` goes to `searxng_addr` (`tool_not_available` without one); `fetch` answers
    /// `tool_not_available` until P5A.15.
    pub fn builtin(searxng_addr: Option<String>) -> Registry {
        let query = json!({"type": "object", "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 200}}});
        let url = json!({"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}});
        let store = Arc::<search::Remembered>::default(); // P5A.15's `fetch` shares it
        let fetch = handler(|_| async { Err(ErrorCode::ToolNotAvailable) });
        Registry::new([("search", query, search::handler_for(searxng_addr, store)), ("fetch", url, fetch)])
    }

    /// A registry of the given handlers (the tests register their own).
    pub fn new(handlers: impl IntoIterator<Item = (&'static str, Value, Handler)>) -> Registry {
        Registry(handlers.into_iter().map(|(n, s, h)| (n, (s, h))).collect())
    }

    pub fn get(&self, tool: &str) -> Option<&Handler> {
        self.0.get(tool).map(|t| &t.1)
    }

    pub fn schema(&self, tool: &str) -> Option<&Value> {
        self.0.get(tool).map(|t| &t.0)
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
