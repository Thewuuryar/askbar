//! The `fetch` SSRF matrix at the front, through `POST /tools/fetch/call`: a fake proxy (raw TCP, so it can hang, stream
//! without end and see `CONNECT`) stands in for the egress proxy and the web behind it, a fake auditor records the spans.
//! Each refusal has its own `fetch_refused` reason (or its own code), nothing leaves for a refused URL, the full URL is in a
//! span before every allowed hop, and the front's logs (at the level it runs at, `info`) hold no URL and no page text.

mod common;

use askbar_mcp_gateway::registry::Registry;
use askbar_mcp_gateway::{fetch, search};
use askbar_proto::approval::{Provenance, ToolAction};
use askbar_service::approval::sign;
use common::*;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

enum Page {
    Reply(u16, Vec<(&'static str, String)>, Vec<u8>),
    /// Accepts and never answers.
    Hang,
    /// The headers and a few bytes of a long body, then silence.
    Stall,
    /// A body that never ends.
    Endless,
}

type Pages = Arc<dyn Fn(&str) -> Page + Send + Sync>;

struct Hit {
    line: String,
    head: String,
    /// Whether the auditor already held a span with this request's full URL when it arrived.
    span_before: bool,
}

struct Proxy {
    url: String,
    hits: Arc<Mutex<Vec<Hit>>>,
    seen: Arc<OnceLock<Seen>>,
    endless_dropped: Arc<AtomicBool>,
}

impl Proxy {
    async fn start(pages: Pages) -> Proxy {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = Proxy {
            url: format!("http://{}", l.local_addr().unwrap()),
            hits: Default::default(),
            seen: Default::default(),
            endless_dropped: Default::default(),
        };
        let (hits, seen, dropped) = (p.hits.clone(), p.seen.clone(), p.endless_dropped.clone());
        tokio::spawn(async move {
            loop {
                let (mut s, _) = l.accept().await.unwrap();
                let (pages, hits, seen, dropped) = (pages.clone(), hits.clone(), seen.clone(), dropped.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        let mut tmp = [0u8; 4096];
                        match s.read(&mut tmp).await {
                            Ok(n) if n > 0 => buf.extend_from_slice(&tmp[..n]),
                            _ => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&buf).to_string();
                    let line = head.lines().next().unwrap_or("").to_string();
                    let mut parts = line.split(' ');
                    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                    let span_before = seen.get().is_some_and(|s| {
                        s.lock().unwrap().iter().any(|(p, b, _)| p.ends_with("/spans") && b["attrs"]["url"] == target.as_str())
                    });
                    hits.lock().unwrap().push(Hit { line, head, span_before });
                    let page = match (method.as_str(), target.strip_prefix("http://")) {
                        ("GET", Some(key)) => pages(key),
                        _ => Page::Reply(502, vec![], vec![]), // a CONNECT is only recorded
                    };
                    match page {
                        Page::Reply(status, headers, body) => {
                            let mut r = format!("HTTP/1.1 {status} X\r\nconnection: close\r\ncontent-length: {}\r\n", body.len());
                            for (k, v) in headers {
                                r += &format!("{k}: {v}\r\n");
                            }
                            let _ = s.write_all(r.as_bytes()).await;
                            let _ = s.write_all(b"\r\n").await;
                            let _ = s.write_all(&body).await;
                        }
                        Page::Hang => tokio::time::sleep(std::time::Duration::from_secs(60)).await,
                        Page::Stall => {
                            let _ = s.write_all(b"HTTP/1.1 200 X\r\ncontent-type: text/plain\r\ncontent-length: 60000\r\n\r\nabc").await;
                            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                        }
                        Page::Endless => {
                            let _ = s.write_all(b"HTTP/1.1 200 X\r\ncontent-type: text/plain\r\nconnection: close\r\n\r\n").await;
                            while s.write_all(&[b'a'; 8192]).await.is_ok() {}
                            dropped.store(true, Ordering::SeqCst);
                        }
                    }
                });
            }
        });
        p
    }

    fn lines(&self) -> Vec<String> {
        self.hits.lock().unwrap().iter().map(|h| h.line.clone()).collect()
    }

    fn count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

fn reply(ct: &'static str, body: &str) -> Page {
    Page::Reply(200, vec![("content-type", ct.into())], body.as_bytes().to_vec())
}

fn redirect(to: &str) -> Page {
    Page::Reply(302, vec![("location", to.into())], vec![])
}

/// A stand-in `searxng` that answers every search with the one result `http://a.example/found`.
async fn fake_searxng() -> String {
    let app = axum::Router::new().route(
        "/search",
        axum::routing::post(|| async { axum::Json(json!({"results": [{"title": "t", "url": "http://a.example/found", "content": "c"}]})) }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

struct Fx {
    rig: Rig,
    proxy: Proxy,
}

/// A front with the real fetch and search handlers, the fake proxy as its egress proxy, 64 KiB and a 1 s fetch limit.
async fn fx(pages: Pages) -> Fx {
    logs();
    let proxy = Proxy::start(pages).await;
    let store = Arc::<search::Remembered>::default();
    let schema = json!({"type": "object"});
    let registry = Registry::new([
        ("fetch", schema.clone(), fetch::handler_for(fetch::client(Some(&proxy.url)), store.clone())),
        ("search", schema, search::handler_for(Some(fake_searxng().await), store)),
    ]);
    let perms = json!({"alice": {"allowed": ["fetch", "search"]}});
    let rig = start(Auditor::Up, registry, perms.clone()).await;
    rig.push_with(perms, json!({"fetch_max_bytes": 65536, "fetch_timeout_secs": 1, "tool_call_timeout_secs": 5})).await;
    proxy.seen.set(rig.seen.clone()).ok();
    Fx { rig, proxy }
}

static IDS: AtomicU64 = AtomicU64::new(0);

async fn call(rig: &Rig, tool: &str, args: Value, provenance: Provenance, query_data: bool) -> (u16, Value) {
    let action = ToolAction { tool: tool.into(), arguments: args.clone(), destination: "web".into(), provenance, query_data };
    let id = format!("id{}", IDS.fetch_add(1, Ordering::SeqCst));
    let a = sign(&action, "t1", "alice", &id, now() + 60, &rig.sk).unwrap();
    let body = json!({"arguments": args, "destination": "web", "provenance": provenance, "query_data": query_data, "approval": a});
    rig.post(tool, &body).await
}

async fn fetch_as(rig: &Rig, url: &str, p: Provenance, query_data: bool) -> (u16, Value) {
    call(rig, "fetch", json!({"url": url}), p, query_data).await
}

async fn get(rig: &Rig, url: &str) -> (u16, Value) {
    fetch_as(rig, url, Provenance::User, false).await
}

/// `(code, reason)` of a refusal.
fn why(r: &(u16, Value)) -> (&str, &str) {
    (r.1["error"].as_str().unwrap_or(""), r.1["reason"].as_str().unwrap_or(""))
}

/// The URLs of the hop spans the auditor holds, in order.
fn hop_urls(rig: &Rig) -> Vec<String> {
    rig.spans()
        .iter()
        .filter_map(|(p, b, _)| p.ends_with("/spans").then(|| b["attrs"]["url"].as_str().map(String::from)).flatten())
        .collect()
}

fn site() -> Pages {
    Arc::new(|k| match k {
        "a.example/x" => reply("text/html", "<p>hello</p>"),
        _ => Page::Reply(404, vec![("content-type", "text/plain".into())], b"no".to_vec()),
    })
}

#[tokio::test]
async fn every_url_rule_refuses_with_its_own_reason_and_nothing_leaves() {
    let f = fx(site()).await;
    let user = Provenance::User;
    let cases = [
        // scheme, port, userinfo, query, fragment-free host forms
        ("ftp://a.example/", "scheme"),
        ("file:///etc/passwd", "scheme"),
        ("javascript:alert(1)", "bad_url"),
        ("http://a.example:22/", "port"),
        ("http://a.example:8080/", "port"),
        ("http://user:pw@a.example/", "userinfo"),
        ("http://evil.com\\@a.example/", "userinfo"),
        ("http://a.example/p?q=1", "query_string"),
        ("http://b\u{fc}cher.example/", "bad_host"),
        ("http://xn--bcher-kva.example%2f@x/", "userinfo"),
        ("http://a%2eexample/", "bad_host"),
        // IP literals in every form
        ("http://127.0.0.1/", "ip_literal"),
        ("http://2130706433/", "ip_literal"),
        ("http://017700000001/", "ip_literal"),
        ("http://0177.0.0.1/", "ip_literal"),
        ("http://0x7f000001/", "ip_literal"),
        ("http://0x7f.1/", "ip_literal"),
        ("http://127.1/", "ip_literal"),
        ("http://0x7f.0.0.0x1/", "ip_literal"),
        ("http://10.0.0.1/", "ip_literal"),
        ("http://169.254.169.254/latest/meta-data/", "ip_literal"),
        ("http://[::1]/", "ip_literal"),
        ("http://[::ffff:127.0.0.1]/", "ip_literal"),
        ("http://[fd00::1]:80/", "ip_literal"),
        // names that are never public
        ("http://localhost/", "local_name"),
        ("http://localhost./", "local_name"),
        ("http://a.localhost/", "local_name"),
        ("http://searxng/", "local_name"),
        ("http://model-gateway/", "local_name"),
        ("http://auditor.internal/", "local_name"),
        ("http://searxng:8080/", "port"),
    ];
    for (url, reason) in cases {
        let r = get(&f.rig, url).await;
        assert_eq!((r.0, why(&r)), (403, ("fetch_refused", reason)), "{url}: {r:?}");
    }
    let r = fetch_as(&f.rig, "http://127.0.0.1/p?q=1", user, true).await;
    assert_eq!(why(&r), ("fetch_refused", "ip_literal"), "the query flag lifts the query rule only");
    assert_eq!(f.proxy.count(), 0, "nothing left for a refused URL");
    assert!(hop_urls(&f.rig).is_empty(), "and no hop span was opened");
    assert_eq!(f.rig.outcome("tool_error").await, cases.len() as u64 + 1);
}

#[tokio::test]
async fn a_search_url_must_come_from_that_tasks_search_and_query_data_needs_the_signed_flag() {
    let f = fx(site()).await;
    let r = fetch_as(&f.rig, "http://a.example/found", Provenance::Search, false).await;
    assert_eq!(why(&r), ("fetch_refused", "not_in_results"));
    assert_eq!(f.proxy.count(), 0);
    let s = call(&f.rig, "search", json!({"query": "q"}), Provenance::User, false).await;
    assert_eq!(s.0, 200);
    let r = fetch_as(&f.rig, "http://a.example/found", Provenance::Search, false).await;
    assert_eq!((r.0, r.1["result"]["status"].as_u64()), (200, Some(404)), "answered by the page (a 404 here), not refused: {r:?}");
    assert_eq!(f.proxy.count(), 1, "a URL the search returned is fetched");
    assert_eq!(why(&fetch_as(&f.rig, "http://a.example/x?q=1", Provenance::User, false).await), ("fetch_refused", "query_string"));
    let r = fetch_as(&f.rig, "http://a.example/x?q=1", Provenance::User, true).await;
    assert_eq!(r.0, 200, "{r:?}");
    assert_eq!(hop_urls(&f.rig).last().map(String::as_str), Some("http://a.example/x?q=1"), "the span has the whole URL, query included");
}

#[tokio::test]
async fn each_redirect_hop_is_judged_and_spanned_before_it_is_requested() {
    let pages: Pages = Arc::new(|k| match k {
        "a.example/start" => redirect("/two"),
        "a.example/two" => redirect("http://b.example/end"),
        "b.example/end" => reply("text/plain", "the end"),
        "a.example/private" => redirect("http://169.254.169.254/latest"),
        "a.example/v6" => redirect("http://[::1]/"),
        "a.example/hex" => redirect("http://0x7f.1/"),
        "a.example/service" => redirect("http://searxng/"),
        "a.example/port" => redirect("http://model-gateway:8080/"),
        "a.example/scheme" => redirect("ftp://a.example/f"),
        "a.example/file" => redirect("file:///etc/passwd"),
        "a.example/query" => redirect("/x?secret=1"),
        "a.example/loop" => redirect("/loop"),
        k if k.starts_with("a.example/h") => redirect(&format!("/h{}", k[11..].parse::<u32>().unwrap() + 1)),
        _ => Page::Reply(404, vec![], vec![]),
    });
    let f = fx(pages).await;
    let r = get(&f.rig, "http://a.example/start").await;
    assert_eq!((r.0, r.1["result"]["text"].as_str(), r.1["result"]["url"].as_str()), (200, Some("the end"), Some("http://b.example/end")));
    assert_eq!(hop_urls(&f.rig), ["http://a.example/start", "http://a.example/two", "http://b.example/end"]);
    assert!(f.proxy.hits.lock().unwrap().iter().all(|h| h.span_before), "each hop's span (full URL) was on record before its request");

    for (path, reason) in [
        ("private", "ip_literal"),
        ("v6", "ip_literal"),
        ("hex", "ip_literal"),
        ("service", "local_name"),
        ("port", "port"),
        ("scheme", "scheme"),
        ("file", "scheme"),
        ("query", "query_string"),
        ("loop", "redirect_limit"),
        ("h0", "redirect_limit"), // a chain of distinct URLs longer than the cap of 3
    ] {
        let (before, spans) = (f.proxy.count(), hop_urls(&f.rig).len());
        let r = get(&f.rig, &format!("http://a.example/{path}")).await;
        assert_eq!((r.0, why(&r)), (403, ("fetch_refused", reason)), "{path}: {r:?}");
        let hops = if reason == "redirect_limit" { 4 } else { 1 };
        assert_eq!(
            (f.proxy.count() - before, hop_urls(&f.rig).len() - spans),
            (hops, hops),
            "{path}: only the allowed hops left, each spanned"
        );
    }
    assert!(f.proxy.hits.lock().unwrap().iter().all(|h| h.span_before));
    assert!(f.proxy.lines().iter().all(|l| !l.contains("169.254") && !l.contains("::1") && !l.contains("searxng")));
}

#[tokio::test]
async fn the_size_cap_is_exact_and_a_stream_that_never_ends_is_cut() {
    let pages: Pages = Arc::new(|k| match k {
        "a.example/exact" => reply("text/plain", &"a".repeat(65536)),
        "a.example/over" => reply("text/plain", &"a".repeat(65537)),
        "a.example/endless" => Page::Endless,
        _ => Page::Reply(404, vec![], vec![]),
    });
    let f = fx(pages).await;
    let r = get(&f.rig, "http://a.example/exact").await;
    assert_eq!((r.0, r.1["result"]["text"].as_str().map(str::len)), (200, Some(65536)), "a body of exactly the cap passes");
    assert_eq!(code(&get(&f.rig, "http://a.example/over").await), (502, "tool_result_too_large"));
    assert_eq!(code(&get(&f.rig, "http://a.example/endless").await), (502, "tool_result_too_large"));
    for _ in 0..50 {
        if f.proxy.endless_dropped.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(f.proxy.endless_dropped.load(Ordering::SeqCst), "the front stopped reading and dropped the connection");
}

#[tokio::test]
async fn slow_headers_and_a_slow_body_end_at_the_time_limit() {
    let pages: Pages = Arc::new(|k| match k {
        "a.example/headers" => Page::Hang,
        "a.example/body" => Page::Stall,
        _ => Page::Reply(404, vec![], vec![]),
    });
    let f = fx(pages).await;
    for path in ["headers", "body"] {
        let t = std::time::Instant::now();
        let r = get(&f.rig, &format!("http://a.example/{path}")).await;
        assert_eq!(code(&r), (504, "tool_timeout"), "{path}");
        assert!(t.elapsed() < std::time::Duration::from_secs(4), "{path}: ended at the 1 s limit, not the call's");
    }
}

#[tokio::test]
async fn only_the_fixed_content_types_without_encoding_are_returned() {
    let html_body = "<html><script>x()</script><b>bold</b></html>";
    let pages: Pages = Arc::new(move |k| match k {
        "a.example/html" => reply("text/html; charset=utf-8", html_body),
        "a.example/json" => reply("application/json", "{\"a\":1}"),
        "a.example/octet" => reply("application/octet-stream", html_body),
        "a.example/png" => reply("image/png", html_body),
        "a.example/none" => Page::Reply(200, vec![], html_body.as_bytes().to_vec()),
        "a.example/plain" => reply("text/plain", html_body),
        "a.example/gzip" => {
            Page::Reply(200, vec![("content-type", "text/html".into()), ("content-encoding", "gzip".into())], vec![0x1f, 0x8b])
        }
        "a.example/br" => Page::Reply(200, vec![("content-type", "text/plain".into()), ("content-encoding", "br".into())], b"x".to_vec()),
        "a.example/identity" => {
            Page::Reply(200, vec![("content-type", "text/plain".into()), ("content-encoding", "identity".into())], b"ok".to_vec())
        }
        _ => Page::Reply(404, vec![], vec![]),
    });
    let f = fx(pages).await;
    let text = |r: &(u16, Value)| r.1["result"]["text"].as_str().map(String::from);
    assert_eq!(text(&get(&f.rig, "http://a.example/html").await).as_deref(), Some("bold"), "html is reduced to its text");
    assert_eq!(get(&f.rig, "http://a.example/json").await.1["result"]["content_type"], "application/json");
    assert_eq!(text(&get(&f.rig, "http://a.example/identity").await).as_deref(), Some("ok"));
    // The type is the header's word: no sniffing. A wrong type that is HTML inside is refused; text/plain is text, kept as sent.
    assert_eq!(text(&get(&f.rig, "http://a.example/plain").await).as_deref(), Some(html_body));
    for (path, reason) in [
        ("octet", "content_type"),
        ("png", "content_type"),
        ("none", "content_type"),
        ("gzip", "content_encoding"),
        ("br", "content_encoding"),
    ] {
        let r = get(&f.rig, &format!("http://a.example/{path}")).await;
        assert_eq!((r.0, why(&r)), (403, ("fetch_refused", reason)), "{path}");
    }
    let heads: Vec<String> = f.proxy.hits.lock().unwrap().iter().map(|h| h.head.to_ascii_lowercase()).collect();
    assert!(heads.iter().all(|h| h.contains("accept-encoding: identity")), "the front asks for no compression");
}

#[tokio::test]
async fn the_request_leaves_as_a_proxy_request_naming_the_host_and_never_an_address() {
    let f = fx(site()).await;
    assert_eq!(get(&f.rig, "http://a.example/x").await.0, 200);
    assert_eq!(get(&f.rig, "http://a.example:80/x").await.0, 200);
    // :80 is the default port and is not repeated. https is a CONNECT to the name and port (the fake proxy refuses it: the answer is an upstream failure).
    assert_eq!(code(&get(&f.rig, "https://a.example/x").await), (502, "tool_upstream_failed"));
    let hits = f.proxy.hits.lock().unwrap();
    let lines: Vec<&str> = hits.iter().map(|h| h.line.as_str()).collect();
    assert_eq!(lines, ["GET http://a.example/x HTTP/1.1", "GET http://a.example/x HTTP/1.1", "CONNECT a.example:443 HTTP/1.1"]);
    assert!(hits[0].head.to_ascii_lowercase().contains("host: a.example\r\n"));
    assert!(hits.iter().all(|h| !h.line.contains("127.0.0.1") && !h.line.contains("::")), "the front resolved nothing: no address");
}

#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
    type Writer = Logs;
    fn make_writer(&'a self) -> Logs {
        self.clone()
    }
}

/// One process-wide subscriber at `info` (the level the front runs at) into a buffer, for every test in this file: a
/// thread-local one misses callsites that a parallel test registered first.
fn logs() -> &'static Logs {
    static LOGS: OnceLock<Logs> = OnceLock::new();
    LOGS.get_or_init(|| {
        let l = Logs::default();
        let s = tracing_subscriber::fmt().with_writer(l.clone()).with_ansi(false).with_max_level(tracing::Level::INFO).finish();
        tracing::subscriber::set_global_default(s).expect("the only subscriber of this binary");
        l
    })
}

#[tokio::test]
async fn no_url_or_page_text_reaches_a_log_or_the_status() {
    let pages: Pages = Arc::new(|k| match k {
        "leak-host.example/page-path" => reply("text/html", "<p>PAGE-SECRET-TEXT</p>"),
        "leak-host.example/hop" => redirect("http://other-leak.example/PAGE-SECRET-HOP"),
        "other-leak.example/PAGE-SECRET-HOP" => reply("text/plain", "PAGE-SECRET-TWO"),
        "leak-host.example/query?token=QSECRET" => reply("text/plain", "PAGE-SECRET-THREE"),
        _ => Page::Reply(404, vec![], vec![]),
    });
    let f = fx(pages).await;
    assert_eq!(get(&f.rig, "http://leak-host.example/page-path").await.1["result"]["text"], "PAGE-SECRET-TEXT");
    assert_eq!(get(&f.rig, "http://leak-host.example/hop").await.0, 200);
    assert_eq!(fetch_as(&f.rig, "http://leak-host.example/query?token=QSECRET", Provenance::User, true).await.0, 200);
    assert_eq!(why(&get(&f.rig, "http://127.0.0.1/PAGE-SECRET-REFUSED?").await).0, "fetch_refused");
    assert_eq!(code(&get(&f.rig, "https://leak-host.example/tls").await).1, "tool_upstream_failed");
    let out = String::from_utf8(logs().0.lock().unwrap().clone()).unwrap();
    assert!(out.contains("configuration applied"), "the capture works: {out}");
    let status = serde_json::to_string(&f.rig.g.status()).unwrap();
    for text in [&out, &status] {
        for leak in ["leak-host", "other-leak", "PAGE-SECRET", "QSECRET", "page-path", "127.0.0.1/", "token", "a.example"] {
            assert!(!text.contains(leak), "{leak} appears in: {text}");
        }
    }
}
