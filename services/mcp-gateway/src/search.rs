//! The native `search` tool (ADR 0057): the query goes to `searxng` as a POST form body (so it is in no URL or access log),
//! the answer is cut to the configured result count, and each task's result URLs are remembered for the `fetch` rule
//! ([`Remembered::contains`]). The query and the results are never logged; the front counts outcomes only.

use crate::constants::*;
use crate::fetch_rules::check;
use crate::registry::{handler, Handler, ToolCall};
use askbar_proto::ErrorCode;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_QUERY_CHARS: usize = 200;

/// Per task, the URLs its searches returned: the oldest task is dropped past [`MAX_REMEMBERED_TASKS`], the oldest URL of a task past its bound.
#[derive(Default)]
pub struct Remembered(Mutex<VecDeque<(String, VecDeque<String>)>>);

impl Remembered {
    /// Remembers `urls` for the task, keeping at most `cap` of its newest.
    pub(crate) fn add(&self, task_id: &str, urls: &[String], cap: usize) {
        let mut tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let i = tasks.iter().position(|(t, _)| t == task_id).unwrap_or_else(|| {
            if tasks.len() >= MAX_REMEMBERED_TASKS {
                tasks.pop_front();
            }
            tasks.push_back((task_id.to_string(), VecDeque::new()));
            tasks.len() - 1
        });
        let list = &mut tasks[i].1;
        for u in urls {
            if !list.contains(u) {
                list.push_back(u.clone());
            }
        }
        while list.len() > cap {
            list.pop_front();
        }
    }

    /// Whether a search of this task returned this URL (read by `fetch`, P5A.15).
    pub fn contains(&self, task_id: &str, url: &str) -> bool {
        let tasks = self.0.lock().unwrap_or_else(|e| e.into_inner());
        tasks.iter().any(|(t, urls)| t == task_id && urls.iter().any(|u| u == url))
    }
}

/// The `search` handler. Without a `searxng` address the tool is not available.
pub fn handler_for(addr: Option<String>, store: Arc<Remembered>) -> Handler {
    let client = reqwest::Client::builder().no_proxy().build().expect("a client without TLS builds");
    let (addr, client) = (Arc::new(addr), Arc::new(client));
    handler(move |c: ToolCall| {
        let (addr, client, store) = (addr.clone(), client.clone(), store.clone());
        async move {
            let addr = addr.as_deref().ok_or(ErrorCode::ToolNotAvailable)?;
            let query = query_of(&c.arguments).ok_or(ErrorCode::BadRequest)?;
            // A second under the call's own limit, so a slow `searxng` answers the typed error and not `tool_timeout`.
            let limit = Duration::from_secs(u64::from(c.config.tool_call_timeout_secs) - 1);
            let body = ask(&client, addr, query, limit).await.ok_or(ErrorCode::ToolUpstreamFailed)?;
            let results = parse(&body, c.config.search_results as usize).ok_or(ErrorCode::ToolUpstreamFailed)?;
            let urls: Vec<String> = results.iter().map(|r| r["url"].as_str().unwrap_or_default().to_string()).collect();
            store.add(&c.task_id, &urls, c.config.remembered_urls as usize);
            Ok(json!({ "results": results }))
        }
    })
}

/// The one argument, `query`: a non-blank string of at most 200 characters, and nothing else.
fn query_of(args: &Value) -> Option<&str> {
    let o = args.as_object().filter(|o| o.len() == 1)?;
    let q = o.get("query")?.as_str()?;
    (!q.trim().is_empty() && q.chars().count() <= MAX_QUERY_CHARS).then_some(q)
}

/// POSTs the form and reads at most [`MAX_SEARXNG_BYTES`]; None on any failure (the cause is not kept, it could hold the query).
async fn ask(client: &reqwest::Client, addr: &str, query: &str, limit: Duration) -> Option<Vec<u8>> {
    let mut r = client.post(format!("http://{addr}/search")).timeout(limit).form(&[("q", query), ("format", "json")]).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = r.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_SEARXNG_BYTES {
            return None;
        }
    }
    Some(body)
}

/// The first `n` usable results as `{title, url, snippet}`; a result whose URL `fetch` would refuse is skipped. None if the body is not a `searxng` answer.
fn parse(body: &[u8], n: usize) -> Option<Vec<Value>> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let text = |r: &Value, k: &str, max: usize| r[k].as_str().unwrap_or_default().chars().take(max).collect::<String>();
    let results = v["results"].as_array()?.iter().filter_map(|r| {
        let url = r["url"].as_str().filter(|u| http_url(u))?;
        Some(json!({ "title": text(r, "title", MAX_TITLE_CHARS), "url": url, "snippet": text(r, "content", MAX_SNIPPET_CHARS) }))
    });
    Some(results.take(n).collect())
}

/// A URL `fetch` can take, so a remembered URL is always fetchable. The query-string rule is left out here: a URL with one is
/// fetchable when its action is signed with `query_data` (ADR 0019 rule 3).
fn http_url(u: &str) -> bool {
    u.len() <= MAX_URL_CHARS && check(u, true).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_are_http_or_https_only() {
        for ok in ["http://a.example/x", "HTTPS://a.example"] {
            assert!(http_url(ok), "{ok}");
        }
        for bad in [
            "javascript:alert(1)",
            "ftp://a.example",
            "file:///etc/passwd",
            "https://",
            "https:///x",
            "//a.example",
            "https://a b",
            "https://a\n.example",
        ] {
            assert!(!http_url(bad), "{bad}");
        }
    }

    #[test]
    fn the_store_is_bounded_per_task_and_in_tasks() {
        let s = Remembered::default();
        let urls: Vec<String> = (0..5).map(|i| format!("https://a.example/{i}")).collect();
        s.add("t1", &urls, 3);
        assert!(!s.contains("t1", &urls[1]) && s.contains("t1", &urls[2]) && s.contains("t1", &urls[4]));
        assert!(!s.contains("t2", &urls[4]));
        for i in 0..=MAX_REMEMBERED_TASKS {
            s.add(&format!("k{i}"), &urls[..1], 3);
        }
        assert!(!s.contains("t1", &urls[4]) && !s.contains("k0", &urls[0]) && s.contains("k1", &urls[0]));
    }

    #[tokio::test]
    async fn a_search_remembers_its_urls_for_its_own_task_only() {
        let app = axum::Router::new().route(
            "/search",
            axum::routing::post(|| async {
                axum::Json(json!({"results": [{"title": "t", "url": "https://r.example/1", "content": "c"}]}))
            }),
        );
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let store = Arc::<Remembered>::default();
        let h = handler_for(Some(addr), store.clone());
        let call = |task: &str| ToolCall {
            action: askbar_proto::approval::ToolAction {
                tool: "search".into(),
                arguments: json!({"query": "q"}),
                destination: String::new(),
                provenance: Default::default(),
                query_data: false,
            },
            task_id: task.into(),
            user: "alice".into(),
            config: Default::default(),
            audit: None,
            span_id: String::new(),
        };
        assert!(h(call("t1")).await.is_ok());
        assert!(store.contains("t1", "https://r.example/1"));
        assert!(!store.contains("t2", "https://r.example/1"));
    }
}
