//! The native `fetch` tool (ADR 0057, ADR 0019 rules 1, 3 and 6): one signed URL, fetched through the egress proxy.
//!
//! Every URL rule is decided before the first network call ([`crate::fetch_rules`]); redirects are followed here, one
//! hop at a time, each hop re-checked and given its own `tool` span (with the full URL) that the auditor has acknowledged
//! before the request is sent. The response is read (a `gzip` body is decoded; the size cap counts decoded bytes), within the size and time limits, and only for
//! the fixed content types; HTML becomes plain text with the `html2text` crate. Nothing about a URL, a page or a
//! query is logged or counted: the front's status holds outcome counts only.

use crate::fetch_rules::{check, resolve, Target};
use crate::registry::{handler, Handler, ToolCall, ToolError};
use crate::search::Remembered;
use askbar_proto::approval::Provenance;
use askbar_proto::audit::{SpanKind, SpanStart};
use askbar_proto::mcp::{DATAMARK, FETCH_CONTENT_TYPES};
use askbar_proto::ErrorCode;
use askbar_service::audit_client::now_parts;
use askbar_service::ids::new_id;
use html2text::{config, render::TrivialDecorator};
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, LOCATION};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

const USER_AGENT: &str = concat!("askbar-fetch/", env!("CARGO_PKG_VERSION"));

/// The client: through the egress proxy when one is named (an address, never a secret), no redirects (every hop is
/// ours to check), no cookie store, no `Referer`, and no `HTTP(S)_PROXY` from the environment.
pub fn client(proxy: Option<&str>) -> reqwest::Client {
    let mut b = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(10))
        .user_agent(USER_AGENT);
    if let Some(url) = proxy {
        b = b.proxy(reqwest::Proxy::all(url).expect("ASKBAR_EGRESS_PROXY_URL"));
    }
    b.build().expect("reqwest client")
}

pub fn handler_for(client: reqwest::Client, store: Arc<Remembered>) -> Handler {
    let (client, store) = (Arc::new(client), store);
    handler(move |c: ToolCall| {
        let (client, store) = (client.clone(), store.clone());
        async move { run(&client, &store, &c).await }
    })
}

fn refused(reason: &'static str) -> ToolError {
    ToolError { code: ErrorCode::FetchRefused, reason: Some(reason) }
}

/// The one argument, `url`: a string, and nothing else.
fn url_of(args: &Value) -> Option<&str> {
    args.as_object().filter(|o| o.len() == 1)?.get("url")?.as_str()
}

async fn run(client: &reqwest::Client, store: &Remembered, c: &ToolCall) -> Result<Value, ToolError> {
    let url = url_of(&c.arguments).ok_or(ErrorCode::BadRequest)?;
    if c.provenance == Provenance::Search && !store.contains(&c.task_id, url) {
        return Err(refused("not_in_results"));
    }
    let first = check(url, c.query_data).map_err(refused)?;
    // A second under the call's own limit, so the typed error wins over `tool_timeout` from the call.
    let limit = c.config.fetch_timeout_secs.min(c.config.tool_call_timeout_secs - 1);
    match tokio::time::timeout(Duration::from_secs(u64::from(limit)), hops(client, c, first)).await {
        Ok(r) => r,
        Err(_) => Err(ErrorCode::ToolTimeout.into()),
    }
}

/// The request loop: a span, then a request, per hop. Redirects are followed up to the configured cap; a `search` URL
/// may not leave its host (the user's own URL may).
async fn hops(client: &reqwest::Client, c: &ToolCall, first: Target) -> Result<Value, ToolError> {
    let (max, mut cur) = (c.config.fetch_redirects, first.clone());
    for hop in 0..=max {
        let span = hop_span(c, &cur.url, hop).await?;
        let sent = client.get(&cur.url).header(ACCEPT_ENCODING, "gzip").send().await;
        let Ok(resp) = sent else {
            end_span(c, &span, "error", Some(ErrorCode::ToolUpstreamFailed));
            return Err(ErrorCode::ToolUpstreamFailed.into());
        };
        end_span(c, &span, "ok", None);
        let location = resp.status().is_redirection().then(|| resp.headers().get(LOCATION)).flatten();
        let Some(location) = location else {
            return read(resp, &cur.url, c).await;
        };
        if hop == max {
            return Err(refused("redirect_limit"));
        }
        let next = location
            .to_str()
            .map_err(|_| "bad_url")
            .and_then(|l| resolve(&cur, l))
            .and_then(|u| check(&u, c.query_data))
            .map_err(refused)?;
        if cur.https && !next.https {
            return Err(refused("https_to_http"));
        }
        if c.provenance == Provenance::Search && next.host != first.host {
            return Err(refused("cross_host_redirect"));
        }
        cur = next;
    }
    Err(refused("redirect_limit")) // unreachable: the last hop returns above
}

/// A `tool` span under the call's own, with the full URL (ADR 0019 rule 6). No acknowledgement, no request.
async fn hop_span(c: &ToolCall, url: &str, hop: u32) -> Result<String, ToolError> {
    let audit = c.audit.as_ref().ok_or(ErrorCode::AuditBacklog)?;
    let (at, ms) = now_parts();
    let id = new_id();
    let start = SpanStart {
        id: id.clone(),
        trace_id: c.task_id.clone(),
        parent_id: Some(c.span_id.clone()),
        kind: SpanKind::Tool,
        name: "fetch".into(),
        started_at: at,
        started_ms: ms,
        provider_id: None,
        model: None,
        user_id: Some(c.user.clone()),
        client_id: None,
        attrs: Some(json!({ "url": url, "hop": hop })),
    };
    audit.open(&start).await.map_err(|_| ErrorCode::AuditBacklog)?;
    Ok(id)
}

fn end_span(c: &ToolCall, id: &str, status: &str, code: Option<ErrorCode>) {
    if let Some(audit) = &c.audit {
        audit.finish(id, crate::call::finish(status, code.map(ErrorCode::as_str)));
    }
}

/// The final response: encoding and type checks, then at most `fetch_max_bytes` of body (more aborts the read).
async fn read(mut resp: reqwest::Response, url: &str, c: &ToolCall) -> Result<Value, ToolError> {
    // Only `gzip` is decoded, alone: any other coding, or a stacked list such as `gzip, gzip`, is refused.
    let enc: Vec<String> =
        resp.headers().get_all(CONTENT_ENCODING).iter().map(|v| v.to_str().unwrap_or("?").trim().to_ascii_lowercase()).collect();
    let gzip = match enc.as_slice() {
        [e] if e == "gzip" => true,
        all if all.iter().all(|e| e == "identity") => false,
        _ => return Err(refused("content_encoding")),
    };
    let ct = resp.headers().get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("");
    let ct = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if !FETCH_CONTENT_TYPES.contains(&ct.as_str()) {
        return Err(refused("content_type"));
    }
    let max = c.config.fetch_max_bytes;
    let too_large = || ToolError::from(ErrorCode::ToolResultTooLarge);
    if resp.content_length().is_some_and(|n| n > max) {
        return Err(too_large());
    }
    let status = resp.status().as_u16();
    // A decode error is the upstream's fault: `tool_upstream_failed`, as a broken connection is.
    let fail = |e: std::io::Error| {
        ToolError::from(if e.kind() == std::io::ErrorKind::FileTooLarge {
            ErrorCode::ToolResultTooLarge
        } else {
            ErrorCode::ToolUpstreamFailed
        })
    };
    let mut plain = Capped { buf: Vec::new(), max };
    let mut dec = gzip.then(|| flate2::write::GzDecoder::new(Capped { buf: Vec::new(), max }));
    while let Some(chunk) = resp.chunk().await.map_err(|_| ErrorCode::ToolUpstreamFailed)? {
        match dec.as_mut() {
            Some(d) => d.write_all(&chunk),
            None => plain.write_all(&chunk),
        }
        .map_err(fail)?;
    }
    // `finish` also fails on a stream cut short.
    let body = match dec {
        Some(d) => d.finish().map_err(fail)?.buf,
        None => plain.buf,
    };
    // shortcut: the body is read as UTF-8 whatever its charset says; upgrade if a charset-heavy site matters.
    let text = if ct.ends_with("html+xml") || ct == "text/html" {
        // A blocking thread, so a CPU-heavy page cannot stall the runtime; the tool-call time limit bounds the wait.
        let mut t = tokio::task::spawn_blocking(move || html_to_text(&body).map(|t| tidy(&t)))
            .await
            .map_err(|_| ErrorCode::ToolUpstreamFailed)?
            .map_err(|_| ErrorCode::ToolUpstreamFailed)?;
        // The output is capped like the input (table layout can add text).
        let mut end = t.len().min(max as usize);
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        t.truncate(end);
        t
    } else {
        String::from_utf8_lossy(&body).into_owned()
    };
    let text: String = text.chars().filter(|&ch| ch != DATAMARK && (ch == '\n' || ch == '\t' || !ch.is_control())).collect();
    // `truncated` is false for now: a body over the cap is an error, not a cut page.
    Ok(json!({ "url": url, "status": status, "content_type": ct, "text": text, "truncated": false }))
}

/// A buffer that refuses bytes past `max` (so a gzip bomb stops at the cap, with no allocation beyond it).
struct Capped {
    buf: Vec<u8>,
    max: u64,
}

impl std::io::Write for Capped {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if (self.buf.len() + b.len()) as u64 > self.max {
            return Err(std::io::ErrorKind::FileTooLarge.into());
        }
        self.buf.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// How deep the tags may nest before the page is cut off there: `html2text` takes time quadratic in the depth (100000
/// nested divs took a minute) and its stack use grows with it.
const MAX_DEPTH: usize = 1000;

/// Text of an HTML page by `html2text` (html5ever): no tags, link URLs or table borders, and no `script`, `style`, `noscript` or
/// `template` content. shortcut: the depth is counted by a rough scan (no quoting rules; void and implied-end tags skipped); a
/// page nested deeper than `MAX_DEPTH` is read only up to there.
fn html_to_text(html: &[u8]) -> Result<String, html2text::Error> {
    const NO_NEST: [&str; 15] = ["br", "hr", "img", "input", "meta", "link", "p", "li", "dt", "dd", "tr", "td", "th", "option", "wbr"];
    let (mut depth, mut i, mut end) = (0usize, 0, html.len());
    while let Some(lt) = html[i..].iter().position(|&b| b == b'<') {
        i += lt;
        let closing = html.get(i + 1) == Some(&b'/');
        let name: Vec<u8> =
            html[i + 1 + usize::from(closing)..].iter().take_while(|b| b.is_ascii_alphanumeric()).map(u8::to_ascii_lowercase).collect();
        if !name.is_empty() && name[0].is_ascii_alphabetic() && !NO_NEST.iter().any(|v| v.as_bytes() == name) {
            let gt = html[i..].iter().position(|&b| b == b'>').map_or(html.len(), |g| i + g);
            if closing {
                depth = depth.saturating_sub(1);
            } else if !html[i..gt].ends_with(b"/") {
                depth += 1;
                if depth > MAX_DEPTH {
                    end = i;
                    break;
                }
            }
        }
        i += 1;
    }
    config::with_decorator(TrivialDecorator::new()).no_table_borders().string_from_read(&no_noscript(&html[..end])[..], 1 << 16)
}

/// `html2text` renders `noscript` (it parses with scripting off); the page is read as a script-less client would not.
/// shortcut: a plain case-insensitive search for the tags, no nesting rules; upgrade if a page hides text in a malformed one.
fn no_noscript(html: &[u8]) -> Vec<u8> {
    let lower = html.to_ascii_lowercase();
    let find = |from: usize, pat: &[u8]| lower[from..].windows(pat.len()).position(|w| w == pat).map(|p| from + p);
    let (mut out, mut i) = (Vec::with_capacity(html.len()), 0);
    while let Some(open) = find(i, b"<noscript") {
        out.extend_from_slice(&html[i..open]);
        i = find(open, b"</noscript").and_then(|c| find(c, b">")).map_or(html.len(), |g| g + 1);
    }
    out.extend_from_slice(&html[i.min(html.len())..]);
    out
}

/// Whitespace runs to one space, blank lines dropped.
fn tidy(s: &str) -> String {
    s.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use askbar_proto::approval::ToolAction;
    use askbar_service::audit_client::AuditClient;
    use askbar_service::status::BootId;
    use axum::http::{HeaderName, HeaderValue, Response, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// What a fake page answers: status, headers, body.
    type Page = (u16, Vec<(&'static str, String)>, Vec<u8>);
    type Pages = Arc<dyn Fn(&str) -> Option<Page> + Send + Sync>;

    struct Rig {
        proxy: String,
        /// Requests the fake proxy (and so the fake web) saw: how many, and in order together with the auditor's spans.
        hits: Arc<AtomicUsize>,
        events: Arc<Mutex<Vec<String>>>,
        audit: Arc<AuditClient>,
        store: Arc<Remembered>,
    }

    /// A fake web behind a fake HTTP proxy: the front sends `GET http://host/path` to it, as it does to the egress proxy for
    /// `http` URLs, and `pages` answers by `host/path`. A fake auditor acknowledges every span.
    async fn rig(pages: Pages) -> Rig {
        let (hits, events): (Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) = Default::default();
        let (h, e) = (hits.clone(), events.clone());
        let web = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let (pages, h, e) = (pages.clone(), h.clone(), e.clone());
            async move {
                h.fetch_add(1, Ordering::SeqCst);
                let key = format!("{}{}", req.uri().host().unwrap_or(""), req.uri().path_and_query().map_or("", |p| p.as_str()));
                e.lock().unwrap().push(format!("request {key}"));
                let (status, headers, body) = pages(&key).unwrap_or((404, vec![], vec![]));
                let mut r = Response::builder().status(StatusCode::from_u16(status).unwrap());
                for (k, v) in headers {
                    r = r.header(HeaderName::from_static(k), HeaderValue::from_str(&v).unwrap());
                }
                r.body(axum::body::Body::from(body)).unwrap()
            }
        });
        let e = events.clone();
        let auditor = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let e = e.clone();
            async move {
                let path = req.uri().path().to_string();
                let body = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
                if path.ends_with("/spans") {
                    let v: Value = serde_json::from_slice(&body).unwrap();
                    e.lock().unwrap().push(format!("span {}", v["attrs"]["url"].as_str().unwrap_or("-")));
                }
                axum::Json(json!({"interrupted": 0}))
            }
        });
        let serve = |app: axum::Router| async move {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
            addr
        };
        let (web, auditor) = (serve(web).await, serve(auditor).await);
        let dir = std::env::temp_dir().join(format!("askbar-mcp-fetch-{}", BootId::new().as_str()));
        std::fs::create_dir_all(dir.join("call")).unwrap();
        std::fs::write(dir.join("call/mcp-gateway-auditor.token"), "t\n").unwrap();
        let audit = AuditClient::new(&format!("http://{auditor}"), dir, "mcp-gateway-auditor");
        tokio::spawn(audit.clone().hello_until_done("boot".into()));
        tokio::spawn(audit.clone().drain_forever());
        while !audit.ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Rig { proxy: format!("http://{web}"), hits, events, audit, store: Default::default() }
    }

    impl Rig {
        async fn fetch(&self, url: &str, provenance: Provenance, query_data: bool, config: Config) -> Result<Value, ToolError> {
            let call = ToolCall {
                action: ToolAction {
                    tool: "fetch".into(),
                    arguments: json!({ "url": url }),
                    destination: String::new(),
                    provenance,
                    query_data,
                },
                task_id: "t1".into(),
                user: "alice".into(),
                config: Arc::new(config),
                audit: Some(self.audit.clone()),
                span_id: "tool-span".into(),
            };
            let h = handler_for(client(Some(&self.proxy)), self.store.clone());
            h(call).await
        }

        async fn user(&self, url: &str) -> Result<Value, ToolError> {
            self.fetch(url, Provenance::User, false, Config::default()).await
        }
    }

    fn html(body: &str) -> Option<Page> {
        Some((200, vec![("content-type", "text/html; charset=utf-8".into())], body.as_bytes().to_vec()))
    }

    fn reason(e: ToolError) -> (ErrorCode, Option<&'static str>) {
        (e.code, e.reason)
    }

    fn refusal(r: &'static str) -> (ErrorCode, Option<&'static str>) {
        (ErrorCode::FetchRefused, Some(r))
    }

    #[tokio::test]
    async fn a_refused_url_never_reaches_the_network() {
        let rig = rig(Arc::new(|_| html("x"))).await;
        let cases = [
            ("http://a.example/", Provenance::Search, false, "not_in_results"),
            ("ftp://a.example/", Provenance::User, false, "scheme"),
            ("file:///etc/passwd", Provenance::User, false, "scheme"),
            ("gopher://a.example/", Provenance::User, false, "scheme"),
            ("http://a.example/p?q=1", Provenance::User, false, "query_string"),
            ("http://a.example:22/", Provenance::User, false, "port"),
            ("http://a.example:8080/", Provenance::User, false, "port"),
            ("http://u@a.example/", Provenance::User, false, "userinfo"),
            ("http://b\u{fc}cher.example/", Provenance::User, false, "bad_host"),
            ("http://127.0.0.1/", Provenance::User, false, "ip_literal"),
            ("http://2130706433/", Provenance::User, false, "ip_literal"),
            ("http://0x7f.1/", Provenance::User, false, "ip_literal"),
            ("http://017700000001/", Provenance::User, false, "ip_literal"),
            ("http://[::1]/", Provenance::User, false, "ip_literal"),
            ("http://[::ffff:127.0.0.1]/", Provenance::User, false, "ip_literal"),
            ("http://10.0.0.1/", Provenance::User, false, "ip_literal"),
            ("http://localhost/", Provenance::User, false, "local_name"),
            ("http://db.internal/", Provenance::User, false, "local_name"),
            ("http://a%2eexample/", Provenance::User, false, "bad_host"),
            ("http://evil.com\\@good.com/", Provenance::User, false, "userinfo"),
            ("http://evil.com\\good.com/", Provenance::User, false, "bad_host"),
            ("http://127.0.0.1:80/p?q=1", Provenance::User, true, "ip_literal"),
            ("https://a.example/p?q=1", Provenance::Search, true, "not_in_results"),
            ("http://a.example/\u{e9}", Provenance::User, false, "bad_url"),
            ("example.com", Provenance::User, false, "bad_url"),
            ("http://a.example/a b", Provenance::User, false, "bad_url"),
            ("http://a.example:0/", Provenance::User, false, "port"),
            ("http://x.local/", Provenance::User, false, "local_name"),
        ];
        for (url, prov, query_data, why) in cases {
            let r = rig.fetch(url, prov, query_data, Config::default()).await;
            assert_eq!(reason(r.unwrap_err()), refusal(why), "{url}");
        }
        assert_eq!(rig.hits.load(Ordering::SeqCst), 0, "no request for a refused URL");
        assert!(rig.events.lock().unwrap().is_empty(), "and no span either");
    }

    #[tokio::test]
    async fn the_allowed_forms_are_fetched() {
        let rig = rig(Arc::new(|k| (k.starts_with("a.example")).then(|| html("<p>hi</p>")).flatten())).await;
        for url in ["http://a.example/x", "http://a.example:80/x", "http://a.example:443/x"] {
            assert_eq!(rig.user(url).await.unwrap()["text"], "hi", "{url}");
        }
        // a query string only with the signed flag
        assert_eq!(rig.user("http://a.example/x?q=1").await.map_err(reason), Err(refusal("query_string")));
        let ok = rig.fetch("http://a.example/x?q=1", Provenance::User, true, Config::default()).await.unwrap();
        assert_eq!(ok["status"], 200);
        // a search URL, once the search returned it
        let u = "http://a.example/found";
        assert_eq!(rig.fetch(u, Provenance::Search, false, Config::default()).await.map_err(reason), Err(refusal("not_in_results")));
        rig.store.add("t1", &[u.to_string()], 50);
        assert!(rig.fetch(u, Provenance::Search, false, Config::default()).await.is_ok());
    }

    #[tokio::test]
    async fn redirects_are_checked_per_hop_with_a_span_before_each_request() {
        let pages: Pages = Arc::new(|k| match k {
            "a.example/start" => Some((302, vec![("location", "/two".into())], vec![])),
            "a.example/two" => Some((301, vec![("location", "http://b.example/end".into())], vec![])),
            "b.example/end" => html("<b>end</b>"),
            "a.example/q" => Some((302, vec![("location", "/x?secret=1".into())], vec![])),
            "a.example/priv" => Some((302, vec![("location", "http://127.0.0.1/".into())], vec![])),
            "a.example/meta" => Some((302, vec![("location", "http://[::1]:80/".into())], vec![])),
            "a.example/port" => Some((302, vec![("location", "http://a.example:8080/".into())], vec![])),
            "a.example/loop" => Some((302, vec![("location", "/loop".into())], vec![])),
            _ => None,
        });
        let rig = rig(pages).await;
        // `user` provenance follows a redirect to another host.
        let r = rig.user("http://a.example/start").await.unwrap();
        assert_eq!((r["text"].as_str(), r["url"].as_str()), (Some("end"), Some("http://b.example/end")));
        let ev = rig.events.lock().unwrap().clone();
        assert_eq!(
            ev,
            [
                "span http://a.example/start",
                "request a.example/start",
                "span http://a.example/two",
                "request a.example/two",
                "span http://b.example/end",
                "request b.example/end"
            ],
            "each hop's span is on record before its request"
        );
        // a hop is judged by the same rules
        for (path, why) in [("q", "query_string"), ("priv", "ip_literal"), ("meta", "ip_literal"), ("port", "port")] {
            let r = rig.user(&format!("http://a.example/{path}")).await;
            assert_eq!(r.map_err(reason), Err(refusal(why)), "{path}");
        }
        // the cap: a loop stops after the configured hops (the first request and three more)
        let before = rig.hits.load(Ordering::SeqCst);
        assert_eq!(rig.user("http://a.example/loop").await.map_err(reason), Err(refusal("redirect_limit")));
        assert_eq!(rig.hits.load(Ordering::SeqCst) - before, 4);
        let none = Config { fetch_redirects: 0, ..Config::default() };
        let r = rig.fetch("http://a.example/start", Provenance::User, false, none).await;
        assert_eq!(r.map_err(reason), Err(refusal("redirect_limit")));
    }

    #[tokio::test]
    async fn a_search_url_may_not_leave_its_host_and_https_may_not_drop_to_http() {
        let pages: Pages = Arc::new(|k| match k {
            "a.example/same" => Some((302, vec![("location", "/ok".into())], vec![])),
            "a.example/ok" => html("ok"),
            "a.example/away" => Some((302, vec![("location", "http://b.example/".into())], vec![])),
            _ => None,
        });
        let rig = rig(pages).await;
        for p in ["same", "away"] {
            rig.store.add("t1", &[format!("http://a.example/{p}")], 50);
        }
        let rig = &rig;
        let search = |p: &str| {
            let url = format!("http://a.example/{p}");
            async move { rig.fetch(&url, Provenance::Search, false, Config::default()).await }
        };
        assert_eq!(search("same").await.unwrap()["text"], "ok");
        assert_eq!(search("away").await.map_err(reason), Err(refusal("cross_host_redirect")));
        // https to http is the rules' job (the fake web speaks no TLS)
        let https = check("https://a.example/", false).unwrap();
        let next = resolve(&https, "http://a.example/").and_then(|u| check(&u, false)).unwrap();
        assert!(https.https && !next.https);
    }

    fn gz(b: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        e.write_all(b).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn capped_stops_at_its_limit() {
        let mut c = Capped { buf: Vec::new(), max: 4 };
        assert!(c.write_all(b"abcd").is_ok());
        assert_eq!(c.write_all(b"e").unwrap_err().kind(), std::io::ErrorKind::FileTooLarge);
        assert_eq!(c.buf, b"abcd");
    }

    #[tokio::test]
    async fn the_response_is_checked_before_it_is_read() {
        let pages: Pages = Arc::new(|k| match k {
            "a.example/gz" => Some((200, vec![("content-type", "text/plain".into()), ("content-encoding", "gzip".into())], gz(b"zipped"))),
            "a.example/br" => Some((200, vec![("content-type", "text/plain".into()), ("content-encoding", "br".into())], b"x".to_vec())),
            "a.example/id" => {
                Some((200, vec![("content-type", "text/plain".into()), ("content-encoding", "identity".into())], b"plain".to_vec()))
            }
            "a.example/png" => Some((200, vec![("content-type", "image/png".into())], b"x".to_vec())),
            "a.example/none" => Some((200, vec![], b"x".to_vec())),
            "a.example/big" => Some((200, vec![("content-type", "text/plain".into())], vec![b'a'; 70 * 1024])),
            "a.example/json" => Some((200, vec![("content-type", "Application/JSON".into())], b"{\"a\":1}".to_vec())),
            "a.example/missing" => Some((404, vec![("content-type", "text/plain".into())], b"nope".to_vec())),
            _ => None,
        });
        let rig = rig(pages).await;
        assert_eq!(rig.user("http://a.example/gz").await.unwrap()["text"], "zipped");
        assert_eq!(rig.user("http://a.example/br").await.map_err(reason), Err(refusal("content_encoding")));
        assert_eq!(rig.user("http://a.example/png").await.map_err(reason), Err(refusal("content_type")));
        assert_eq!(rig.user("http://a.example/none").await.map_err(reason), Err(refusal("content_type")));
        assert_eq!(rig.user("http://a.example/id").await.unwrap()["text"], "plain");
        assert_eq!(rig.user("http://a.example/json").await.unwrap()["content_type"], "application/json");
        let r = rig.user("http://a.example/missing").await.unwrap();
        assert_eq!((r["status"].as_u64(), r["text"].as_str()), (Some(404), Some("nope")));
        // the cap is the config's: 64 KiB is its lowest bound
        let small = Config { fetch_max_bytes: 64 * 1024, ..Config::default() };
        let r = rig.fetch("http://a.example/big", Provenance::User, false, small).await;
        assert_eq!(r.map_err(reason), Err((ErrorCode::ToolResultTooLarge, None)));
        assert!(rig.user("http://a.example/big").await.is_ok());
    }

    #[tokio::test]
    async fn a_request_with_no_acknowledged_span_is_not_sent() {
        let rig = rig(Arc::new(|_| html("x"))).await;
        let call = |audit| ToolCall {
            action: ToolAction {
                tool: "fetch".into(),
                arguments: json!({"url": "http://a.example/"}),
                destination: String::new(),
                provenance: Provenance::User,
                query_data: false,
            },
            task_id: "t1".into(),
            user: "alice".into(),
            config: Default::default(),
            audit,
            span_id: String::new(),
        };
        let h = handler_for(client(Some(&rig.proxy)), rig.store.clone());
        assert_eq!(reason(h(call(None)).await.unwrap_err()), (ErrorCode::AuditBacklog, None));
        assert_eq!(rig.hits.load(Ordering::SeqCst), 0);
    }

    fn text_of(page: &str) -> String {
        tidy(&html_to_text(page.as_bytes()).unwrap())
    }

    #[test]
    fn html_becomes_text() {
        let page = "<!doctype html><html><head><title>T</title><style>p{color:red}</style><script>var a = '<b>x</b>';</script></head>\
            <body><!-- note --><h1>Hi &amp; bye</h1><p>one <b>two</b>\n\n   three</p><noscript>no js</noscript><template><i>t</i></template>\
            <SCRIPT type=x>evil()</SCRIPT><a href=\"http://u.example/?x>y\">link</a> a &lt; b &#65;&#x42; &copy; &bogus; 1 < 2</body></html>";
        let t = text_of(page);
        for gone in ["color", "var a", "evil", "no js", "note", "u.example", "<b>", "["] {
            assert!(!t.contains(gone), "{gone} in {t:?}");
        }
        for kept in ["Hi & bye", "one two three", "link", "a < b AB \u{a9} &bogus; 1 < 2"] {
            assert!(t.contains(kept), "{kept} not in {t:?}");
        }
    }

    #[tokio::test]
    async fn hostile_html_is_bounded_and_never_panics() {
        // Through `spawn_blocking`, as `read` runs it: its default 2 MiB stack is the one that must be enough.
        let t0 = std::time::Instant::now();
        let nested = format!("{}deep{}", "<div>".repeat(200_000), "</div>".repeat(200_000)); // 1.2 MiB
        let table = format!("<table>{}x{}", "<tr><td><table>".repeat(50_000), "</table></td></tr>".repeat(50_000));
        let evasive = "<div></p>".repeat(100_000);
        let cells = format!("<table><tr>{}</tr></table>", "<td>c</td>".repeat(100_000));
        for page in [nested, table, evasive, cells] {
            tokio::task::spawn_blocking(move || text_of(&page)).await.unwrap();
        }
        assert!(t0.elapsed().as_secs() < 10, "{:?}", t0.elapsed());
        assert!(text_of(&format!("{}ok", "<div>".repeat(999))).contains("ok"));
        let attr = format!("<a href=\"{}\">t</a><p class='{}", "A".repeat(1 << 20), "b".repeat(1000));
        assert!(text_of(&attr).contains('t'));
        assert!(text_of("<p>one<b>two<i>three<div>four<span").contains("four"));
        assert!(text_of("<svg><script>alert(1)</script><text>vector</text></svg><p>after").contains("after"));
        assert!(!text_of("<svg><script>alert(1)</script></svg>").contains("alert"));
        assert_eq!(
            text_of("&amp;&amp;lt; &#0; &#xD800; &#x110000; &#99999999999; &xnotanentity &lt"),
            "&&lt; \u{fffd} \u{fffd} \u{fffd} \u{fffd} &xnotanentity <"
        );
        let garbage: Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(2654435761) >> 13) as u8).collect();
        let _ = html_to_text(&garbage);
        let _ = html_to_text(&[0xff, 0xfe, b'<', 0, b'>', 0xc0]);
    }

    #[tokio::test]
    async fn script_and_style_are_dropped_and_the_datamark_is_stripped() {
        let mark = DATAMARK;
        let body = format!("<style>x</style><script>y</script><p>a{mark}b&#710;c\u{1}d</p>");
        let plain = format!("p{mark}q\u{7}r");
        let pages: Pages = Arc::new(move |k| match k {
            "a.example/h" => html(&body),
            "a.example/t" => Some((200, vec![("content-type", "text/plain".into())], plain.clone().into_bytes())),
            _ => None,
        });
        let rig = rig(pages).await;
        assert_eq!(rig.user("http://a.example/h").await.unwrap()["text"], "abcd");
        assert_eq!(rig.user("http://a.example/t").await.unwrap()["text"], "pqr");
    }

    #[tokio::test]
    async fn the_status_holds_no_url_or_page_text() {
        let rig = rig(Arc::new(|_| html("PAGE-SECRET"))).await;
        let g = crate::Gateway::new(BootId::new(), None, None);
        g.count("tool_ok");
        let _ = rig.user("http://secret-host.example/path").await;
        let text = serde_json::to_string(&g.status()).unwrap();
        assert!(!text.contains("secret-host") && !text.contains("PAGE-SECRET"));
    }
}
