//! The tool registry (ADR 0057): a tool name to the native handler that runs it. `search` and `fetch` are
//! registered with a stub until P5A.14 and P5A.15; a name that is not registered is refused before any span.

use askbar_proto::approval::ToolAction;
use askbar_proto::ErrorCode;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

pub type Outcome = Result<Value, ErrorCode>;
pub type Handler = Box<dyn Fn(ToolAction) -> Pin<Box<dyn Future<Output = Outcome> + Send>> + Send + Sync>;

/// A tool: the schema of its arguments (listed by `GET /tools`) and its handler.
pub struct Registry(HashMap<&'static str, (Value, Handler)>);

impl Registry {
    /// The built-in tools. Both answer `tool_not_available` until their steps land.
    pub fn builtin() -> Registry {
        let stub = || handler(|_| async { Err(ErrorCode::ToolNotAvailable) });
        let query = json!({"type": "object", "required": ["query"], "properties": {"query": {"type": "string", "maxLength": 200}}});
        let url = json!({"type": "object", "required": ["url"], "properties": {"url": {"type": "string"}}});
        Registry::new([("search", query, stub()), ("fetch", url, stub())])
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
    F: Fn(ToolAction) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Outcome> + Send + 'static,
{
    Box::new(move |a| Box::pin(f(a)))
}
