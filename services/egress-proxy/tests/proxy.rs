//! The listeners against a local fake upstream: a fake resolver maps names, a fake connector records the address it was given
//! and reaches the fake upstream (an echo server) instead.

use askbar_egress_proxy::proxy::Mode;
use askbar_egress_proxy::{BoxFut, Connector, Proxy, Resolver};
use askbar_proto::egress::{EgressConfig, Entry, Limits, SourceRules};
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

type Asked = Arc<Mutex<Vec<(String, u16)>>>;
type Dialed = Arc<Mutex<Vec<SocketAddr>>>;
type Answers = Arc<Mutex<HashMap<String, VecDeque<Vec<IpAddr>>>>>;

struct Rig {
    proxy: Arc<Proxy>,
    heads: BTreeMap<String, SocketAddr>,
    relays: BTreeMap<String, SocketAddr>,
    asked: Asked,
    dialed: Dialed,
    answers: Answers,
}

async fn echo_server() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            tokio::spawn(async move {
                let mut b = [0u8; 4096];
                while let Ok(n) = s.read(&mut b).await {
                    if n == 0 || s.write_all(&b[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

/// `connector_hangs`: the connector never answers (the connect timeout test).
async fn rig_with(connector_hangs: bool) -> Rig {
    let upstream = echo_server().await;
    let (asked, dialed): (Asked, Dialed) = Default::default();
    let answers: Answers = Default::default();
    let resolver: Resolver = {
        let (asked, answers) = (asked.clone(), answers.clone());
        Arc::new(move |name, port| {
            asked.lock().unwrap().push((name.clone(), port));
            let mut m = answers.lock().unwrap();
            // The last answer repeats; earlier ones are given once, in order.
            let ips = match m.get_mut(&name) {
                Some(q) if q.len() > 1 => q.pop_front().unwrap(),
                Some(q) => q.front().cloned().unwrap_or_default(),
                None => vec![],
            };
            Box::pin(async move { Ok(ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect()) }) as BoxFut<_>
        })
    };
    let connector: Connector = {
        let dialed = dialed.clone();
        Arc::new(move |addr| {
            dialed.lock().unwrap().push(addr);
            if connector_hangs {
                return Box::pin(std::future::pending());
            }
            Box::pin(TcpStream::connect(upstream))
        })
    };
    let names = ["a", "b"].map(String::from).to_vec();
    let boot = BootId::new();
    let proxy = Proxy::new(names.clone(), resolver, connector, boot.clone(), Restarter::new(boot, || {}));
    let (mut heads, mut relays) = (BTreeMap::new(), BTreeMap::new());
    for n in names {
        for (mode, map) in [(Mode::Http, &mut heads), (Mode::Relay, &mut relays)] {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            map.insert(n.clone(), l.local_addr().unwrap());
            tokio::spawn(proxy.clone().serve(l, n.clone(), mode));
        }
    }
    Rig { proxy, heads, relays, asked, dialed, answers }
}

async fn rig() -> Rig {
    rig_with(false).await
}

fn entry(host: &str, port: u16) -> Entry {
    Entry { host: host.into(), port, plain_http: false, allow_private: false, relay: false, any_public: false }
}

fn private(host: &str, port: u16) -> Entry {
    Entry { allow_private: true, ..entry(host, port) }
}

fn config(sources: &[(&str, Vec<Entry>)], limits: Limits) -> EgressConfig {
    EgressConfig { sources: sources.iter().map(|(n, e)| (n.to_string(), SourceRules { entries: e.clone() })).collect(), limits }
}

impl Rig {
    fn apply(&self, sources: &[(&str, Vec<Entry>)]) {
        self.proxy.apply(config(sources, Limits::default()));
    }
    fn map(&self, name: &str, ips: &[&[&str]]) {
        let q = ips.iter().map(|a| a.iter().map(|i| i.parse().unwrap()).collect()).collect();
        self.answers.lock().unwrap().insert(name.into(), q);
    }
}

/// Send `bytes`, read until the proxy closes (or 3 s), return what came back.
async fn ask(addr: SocketAddr, bytes: &[u8]) -> String {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(bytes).await.unwrap();
    let mut out = Vec::new();
    let _ = timeout(Duration::from_secs(3), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

fn connect(authority: &str) -> Vec<u8> {
    format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").into_bytes()
}

/// Read until `stop` has been seen (or 3 s).
async fn read_until(s: &mut TcpStream, stop: &str) -> String {
    let mut out = Vec::new();
    let mut b = [0u8; 4096];
    while !String::from_utf8_lossy(&out).contains(stop) {
        match timeout(Duration::from_secs(3), s.read(&mut b)).await {
            Ok(Ok(n)) if n > 0 => out.extend_from_slice(&b[..n]),
            _ => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A tunnel opened through `addr` to `authority`, past its `200`.
async fn tunnel(addr: SocketAddr, authority: &str) -> (TcpStream, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(&connect(authority)).await.unwrap();
    let head = read_until(&mut s, "\r\n\r\n").await;
    (s, head)
}

fn denied(answer: &str, reason: &str) {
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    assert!(answer.contains(&format!("X-Askbar-Egress-Reason: {reason}\r\n")), "{answer}");
    assert!(answer.ends_with("\r\n\r\n"), "a denial has no body: {answer}");
}

#[tokio::test]
async fn an_allowed_host_passes_both_ways() {
    let r = rig().await;
    r.apply(&[("a", vec![private("api.test", 443)])]);
    r.map("api.test", &[&["127.0.0.1"]]);
    let (mut s, head) = tunnel(r.heads["a"], "api.test:443").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s.write_all(b"ping").await.unwrap();
    assert_eq!(read_until(&mut s, "ping").await, "ping");
    assert_eq!(r.proxy.status().sources["a"].allowed, 1);
}

#[tokio::test]
async fn an_unlisted_host_is_denied_and_so_is_another_sources_entry() {
    let r = rig().await;
    r.apply(&[("a", vec![private("api.test", 443)])]);
    r.map("api.test", &[&["127.0.0.1"]]);
    denied(&ask(r.heads["a"], &connect("other.test:443")).await, "not_listed");
    // Source isolation: a's entry is nothing on b's listener.
    denied(&ask(r.heads["b"], &connect("api.test:443")).await, "not_listed");
    assert!(r.dialed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn before_any_config_everything_is_denied() {
    let r = rig().await;
    denied(&ask(r.heads["a"], &connect("api.test:443")).await, "not_listed");
    denied(&ask(r.heads["a"], b"GET http://api.test/ HTTP/1.1\r\n\r\n").await, "not_listed");
    assert!(!r.proxy.status().configured);
}

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_needs_allow_private() {
    let r = rig().await;
    r.apply(&[("a", vec![entry("api.test", 443)])]);
    r.map("api.test", &[&["127.0.0.1"]]);
    denied(&ask(r.heads["a"], &connect("api.test:443")).await, "private_address");
    // One non-public answer among public ones refuses too.
    r.map("api.test", &[&["93.184.216.34", "10.0.0.1"]]);
    denied(&ask(r.heads["a"], &connect("api.test:443")).await, "private_address");
    assert!(r.dialed.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_name_is_resolved_once_and_the_connector_gets_the_validated_address() {
    let r = rig().await;
    r.apply(&[("a", vec![entry("api.test", 443)])]);
    // The second answer, which a rebinding server would give a second lookup, is loopback.
    r.map("api.test", &[&["93.184.216.34"], &["127.0.0.1"]]);
    let (_s, head) = tunnel(r.heads["a"], "api.test:443").await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(*r.asked.lock().unwrap(), [("api.test".to_string(), 443)]);
    assert_eq!(*r.dialed.lock().unwrap(), ["93.184.216.34:443".parse::<SocketAddr>().unwrap()]);
}

#[tokio::test]
async fn bad_request_heads_are_400() {
    let r = rig().await;
    r.apply(&[("a", vec![private("a.test", 443)])]);
    let long = format!("CONNECT a.test:443 HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(9000));
    let unterminated = format!("CONNECT a.test:443 HTTP/1.1\r\nX: {}", "a".repeat(9000));
    let cases: [&[u8]; 9] = [
        b"GET /x HTTP/1.1\r\n\r\n",
        b"CONNECT a.test:443/x HTTP/1.1\r\n\r\n",
        b"CONNECT a.test:443\r\n\r\n",
        b"CONNECT a.test:443 HTTP/1.1\n\n",
        b"CONNECT a.test:443 HTTP/1.1\r\nX: a\0b\r\n\r\n",
        b"GET https://a.test/ HTTP/1.1\r\n\r\n",
        b"CONNECT a.test:443 HTTP/1.1\r\nno colon\r\n\r\n",
        long.as_bytes(),
        unterminated.as_bytes(),
    ];
    for c in cases {
        let a = ask(r.heads["a"], c).await;
        assert!(a.starts_with("HTTP/1.1 400"), "{:?} gave {a}", String::from_utf8_lossy(&c[..c.len().min(40)]));
    }
}

#[tokio::test]
async fn a_hung_connect_times_out() {
    let r = rig_with(true).await;
    r.proxy.apply(config(&[("a", vec![private("api.test", 443)])], Limits { connect_secs: 1, ..Limits::default() }));
    r.map("api.test", &[&["127.0.0.1"]]);
    assert!(ask(r.heads["a"], &connect("api.test:443")).await.starts_with("HTTP/1.1 504"));
}

#[tokio::test]
async fn an_idle_tunnel_is_closed() {
    let r = rig().await;
    r.proxy.apply(config(&[("a", vec![private("api.test", 443)])], Limits { idle_secs: 1, ..Limits::default() }));
    r.map("api.test", &[&["127.0.0.1"]]);
    let (mut s, _) = tunnel(r.heads["a"], "api.test:443").await;
    let mut b = [0u8; 8];
    let end = timeout(Duration::from_secs(4), s.read(&mut b)).await.expect("closed within 4 s");
    assert!(matches!(end, Ok(0) | Err(_)));
}

#[tokio::test]
async fn the_129th_tunnel_of_a_source_is_503_and_a_slot_comes_back() {
    let r = rig().await;
    r.apply(&[("a", vec![private("api.test", 443)]), ("b", vec![private("api.test", 443)])]);
    r.map("api.test", &[&["127.0.0.1"]]);
    let mut held = Vec::new();
    for _ in 0..128 {
        let (s, head) = tunnel(r.heads["a"], "api.test:443").await;
        assert!(head.starts_with("HTTP/1.1 200"), "{head}");
        held.push(s);
    }
    assert!(ask(r.heads["a"], &connect("api.test:443")).await.starts_with("HTTP/1.1 503"));
    // Another source is not held up by it.
    assert!(tunnel(r.heads["b"], "api.test:443").await.1.starts_with("HTTP/1.1 200"));
    held.pop();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tunnel(r.heads["a"], "api.test:443").await.1.starts_with("HTTP/1.1 200"));
}

#[tokio::test]
async fn the_total_cap_holds_across_sources() {
    let r = rig().await;
    let limits = Limits { max_tunnels: 1, max_tunnels_per_source: 1, ..Limits::default() };
    r.proxy.apply(config(&[("a", vec![private("api.test", 443)]), ("b", vec![private("api.test", 443)])], limits));
    r.map("api.test", &[&["127.0.0.1"]]);
    let (_s, head) = tunnel(r.heads["a"], "api.test:443").await;
    assert!(head.starts_with("HTTP/1.1 200"));
    assert!(ask(r.heads["b"], &connect("api.test:443")).await.starts_with("HTTP/1.1 503"));
}

#[tokio::test]
async fn plain_http_needs_plain_http_and_is_sent_in_origin_form() {
    let r = rig().await;
    r.apply(&[("a", vec![private("ntfy.lan", 80)]), ("b", vec![Entry { plain_http: true, ..private("ntfy.lan", 80) }])]);
    r.map("ntfy.lan", &[&["127.0.0.1"]]);
    let req = b"POST http://ntfy.lan/topic?x=1 HTTP/1.1\r\nHost: other\r\nProxy-Authorization: p\r\nContent-Length: 2\r\n\r\nhi";
    denied(&ask(r.heads["a"], req).await, "plain_http");
    let mut s = TcpStream::connect(r.heads["b"]).await.unwrap();
    s.write_all(req).await.unwrap();
    // The fake upstream echoes what it got: the rewritten request, then the body.
    let got = read_until(&mut s, "\r\n\r\nhi").await;
    assert_eq!(got, "POST /topic?x=1 HTTP/1.1\r\nHost: ntfy.lan\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");
}

#[tokio::test]
async fn the_relay_reaches_only_its_entrys_target() {
    let r = rig().await;
    let relay = Entry { relay: true, ..private("mail.test", 587) };
    r.apply(&[("a", vec![relay, private("other.test", 443)])]);
    r.map("mail.test", &[&["127.0.0.1"]]);
    let mut s = TcpStream::connect(r.relays["a"]).await.unwrap();
    s.write_all(b"EHLO").await.unwrap();
    assert_eq!(read_until(&mut s, "EHLO").await, "EHLO");
    assert_eq!(*r.asked.lock().unwrap(), [("mail.test".to_string(), 587)]);
    // A source with no relay entry gets a closed connection and nothing dialed.
    assert_eq!(ask(r.relays["b"], b"EHLO").await, "");
    assert_eq!(r.asked.lock().unwrap().len(), 1);
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

#[tokio::test]
async fn a_denial_logs_the_source_host_and_verdict_and_nothing_else() {
    let logs = Logs::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_target(false).finish());
    let r = rig().await;
    r.apply(&[("a", vec![])]);
    let req = b"GET http://evil.test:8443/secret/path?token=abc123 HTTP/1.1\r\nX-Key: hunter2\r\n\r\n";
    denied(&ask(r.heads["a"], req).await, "not_listed");
    let out = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
    for want in ["evil.test", "deny", "not_listed", "source=\"a\""] {
        assert!(out.contains(want), "the log lacks {want}: {out}");
    }
    for leak in ["secret", "path", "token", "abc123", "hunter2", "X-Key", "8443", "GET"] {
        assert!(!out.contains(leak), "the log shows {leak}: {out}");
    }
}

#[tokio::test]
async fn recent_denied_keeps_the_newest_fifty_with_no_path() {
    let r = rig().await;
    r.apply(&[("a", vec![])]);
    for i in 0..55 {
        let req = format!("GET http://h{i}.test/secret/{i} HTTP/1.1\r\n\r\n");
        denied(&ask(r.heads["a"], req.as_bytes()).await, "not_listed");
    }
    let s = r.proxy.status();
    assert_eq!(s.recent_denied.len(), 50);
    assert_eq!(s.recent_denied[0].host, "h54.test");
    assert_eq!(s.recent_denied[49].host, "h5.test");
    assert_eq!((s.recent_denied[0].port, s.recent_denied[0].reason.as_str()), (Some(80), "not_listed"));
    assert!(!serde_json::to_string(&s).unwrap().contains("secret"));
    assert_eq!(s.sources["a"].denied["not_listed"], 55);
}
