//! The alerter's API against a real in-process auditor, a stand-in ntfy server and a stand-in mail server: tokens and their scopes, `503 not_configured`,
//! what each link may send, delivery with its priority, repeat limiting, retries, the test alert, the ledger across a restart,
//! and that no secret leaves (ADR 0055).

use askbar_alerter::{admin_links, admin_router, auditor_links, auditor_router, Alerter, Timing};
use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_service::audit_client::AuditClient;
use askbar_service::status::BootId;
use reqwest::Method as M;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MARKER_TOPIC: &str = "marker-topic-4f9a";
const MARKER_TOKEN: &str = "marker-token-7c21";
const MARKER_MAIL_PASSWORD: &str = "marker-mail-password-91be";
const MARKER_MAIL_USER: &str = "marker-mail-user-5d3a";

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

fn token(link: &str) -> String {
    format!("tok_{link}_secret")
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

/// What the stand-in ntfy server saw.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    priority: String,
    title: String,
    tags: String,
    authorization: String,
    body: String,
}

struct Sink {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// The status it answers with; `fail_first` of the requests get 500 before it.
    status: Arc<AtomicU16>,
    fail_first: Arc<AtomicUsize>,
}

async fn sink() -> Sink {
    let seen: Arc<Mutex<Vec<Seen>>> = Default::default();
    let status = Arc::new(AtomicU16::new(200));
    let fail_first = Arc::new(AtomicUsize::new(0));
    let (s, st, ff) = (seen.clone(), status.clone(), fail_first.clone());
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let (s, st, ff) = (s.clone(), st.clone(), ff.clone());
        async move {
            let (path, priority, title, tags, authorization) = {
                let h = |n: &str| req.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                (req.uri().path().to_string(), h("priority"), h("title"), h("tags"), h("authorization"))
            };
            let body = String::from_utf8_lossy(&axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap()).into_owned();
            s.lock().unwrap().push(Seen { path, priority, title, tags, authorization, body });
            let code = if ff.load(Ordering::SeqCst) > 0 {
                ff.fetch_sub(1, Ordering::SeqCst);
                500
            } else {
                st.load(Ordering::SeqCst)
            };
            axum::http::StatusCode::from_u16(code).unwrap()
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    Sink { url, seen, status, fail_first }
}

/// One message the stand-in mail server took.
#[derive(Debug, Clone)]
struct Mail {
    from: String,
    to: String,
    data: String,
}

/// A mail server on raw sockets: no TLS, no login. `reject` makes it refuse every recipient with the code it holds (0 accepts).
struct MailSink {
    port: u16,
    seen: Arc<Mutex<Vec<Mail>>>,
    reject: Arc<AtomicU16>,
}

async fn mail_sink() -> MailSink {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let seen: Arc<Mutex<Vec<Mail>>> = Default::default();
    let reject = Arc::new(AtomicU16::new(0));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let (s, r) = (seen.clone(), reject.clone());
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = l.accept().await else { return };
            let (s, r) = (s.clone(), r.clone());
            tokio::spawn(async move {
                let (rd, mut wr) = sock.into_split();
                let mut rd = BufReader::new(rd);
                let _ = wr.write_all(b"220 sink ready\r\n").await;
                let (mut from, mut to, mut line) = (String::new(), String::new(), String::new());
                loop {
                    line.clear();
                    if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let up = line.to_ascii_uppercase();
                    let reply: String = if up.starts_with("EHLO") || up.starts_with("HELO") {
                        "250 sink\r\n".into()
                    } else if up.starts_with("MAIL FROM") {
                        from = line.trim().to_string();
                        "250 ok\r\n".into()
                    } else if up.starts_with("RCPT TO") {
                        match r.load(Ordering::SeqCst) {
                            0 => {
                                to = line.trim().to_string();
                                "250 ok\r\n".into()
                            }
                            code => format!("{code} no\r\n"),
                        }
                    } else if up.starts_with("DATA") {
                        let _ = wr.write_all(b"354 go\r\n").await;
                        let mut data = String::new();
                        loop {
                            line.clear();
                            if rd.read_line(&mut line).await.unwrap_or(0) == 0 {
                                return;
                            }
                            if line == ".\r\n" {
                                break;
                            }
                            data.push_str(&line);
                        }
                        s.lock().unwrap().push(Mail { from: from.clone(), to: to.clone(), data });
                        "250 queued\r\n".into()
                    } else if up.starts_with("QUIT") {
                        let _ = wr.write_all(b"221 bye\r\n").await;
                        return;
                    } else {
                        "250 ok\r\n".into()
                    };
                    let _ = wr.write_all(reply.as_bytes()).await;
                }
            });
        }
    });
    MailSink { port, seen, reject }
}

struct T {
    auditor_url: String,
    admin_url: String,
    events_url: String,
    dir: PathBuf,
    alerter: Arc<Alerter>,
    auditor: Arc<Auditor>,
    http: reqwest::Client,
    sink: Sink,
    mail: MailSink,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Reply {
    status: u16,
    body: Value,
    text: String,
}

impl Reply {
    fn code(&self) -> String {
        self.body["error"].as_str().unwrap_or("").to_string()
    }
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

async fn start() -> T {
    let dir = std::env::temp_dir().join(format!("askbar-alerter-{}", ulid::Ulid::new()));
    start_in(dir, sink().await).await
}

/// An alerter and an auditor in `dir`; a second alerter in the same `dir` is a restart.
async fn start_in(dir: PathBuf, sink: Sink) -> T {
    // The auditor, with the alerter's link: `alerter-auditor`.
    let accept = dir.join("auditor-tokens/accept");
    std::fs::create_dir_all(&accept).unwrap();
    std::fs::write(accept.join("alerter-auditor.sha256"), format!("{}\n", hex(&token("alerter-auditor")))).unwrap();
    std::fs::write(accept.join("admin-auditor-read.sha256"), format!("{}\n", hex(&token("admin-auditor-read")))).unwrap();
    let limits = Limits::default();
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("audit/audit.db")).unwrap(), limits.max_queued);
    let free: FreeSpace = Arc::new(|| Some(100 * 1024 * 1024 * 1024));
    let auditor = Auditor::new(recorder, limits, free);
    let auditor_url = serve(askbar_auditor::router(auditor.clone(), askbar_auditor::links(&dir.join("auditor-tokens")))).await;
    // The alerter's own credential volume: the hashes it accepts, and the token it sends the auditor.
    let tokens = dir.join("alerter-token");
    std::fs::create_dir_all(tokens.join("accept")).unwrap();
    std::fs::create_dir_all(tokens.join("call")).unwrap();
    for link in ["auditor-alerter", "admin-alerter"] {
        std::fs::write(tokens.join("accept").join(format!("{link}.sha256")), format!("{}\n", hex(&token(link)))).unwrap();
    }
    std::fs::write(tokens.join("call/alerter-auditor.token"), token("alerter-auditor")).unwrap();
    let audit = AuditClient::new(&auditor_url, tokens.clone(), "alerter-auditor");
    let boot = BootId::new();
    let restarter = askbar_service::restart::Restarter::for_tests(boot.clone(), Duration::ZERO, Duration::from_millis(50), || {}, || {});
    let timing = Timing { backoff: vec![Duration::from_millis(20), Duration::from_millis(20)] };
    let alerter = Alerter::new(audit.clone(), boot.clone(), restarter, &dir.join("alerter-data"), timing);
    tokio::spawn(audit.clone().hello_until_done(boot.as_str().to_string()));
    tokio::spawn(audit.clone().drain_forever());
    tokio::spawn(alerter.clone().deliver_forever());
    let events_url = serve(auditor_router(alerter.clone(), auditor_links(&tokens))).await;
    let admin_url = serve(admin_router(alerter.clone(), admin_links(&tokens))).await;
    T { auditor_url, admin_url, events_url, dir, alerter, auditor, http: reqwest::Client::new(), sink, mail: mail_sink().await }
}

impl T {
    async fn call(&self, base: &str, token: Option<&str>, m: M, path: &str, body: Option<Value>) -> Reply {
        let mut r = self.http.request(m, format!("{base}/api/alerter/v1{path}"));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::Null), text }
    }

    /// As the admin, on its listener.
    async fn admin(&self, m: M, path: &str, body: Option<Value>) -> Reply {
        self.call(&self.admin_url, Some(&token("admin-alerter")), m, path, body).await
    }

    /// As the auditor, on its listener.
    async fn auditor(&self, m: M, path: &str, body: Option<Value>) -> Reply {
        self.call(&self.events_url, Some(&token("auditor-alerter")), m, path, body).await
    }

    async fn configure(&self) {
        let c = json!({ "version": 1, "ntfy": { "server": self.sink.url, "topic": MARKER_TOPIC, "token": MARKER_TOKEN } });
        assert_eq!(self.admin(M::PUT, "/config", Some(c)).await.status, 200);
    }

    /// Both channels: ntfy at its sink and email at the mail sink (no TLS, no login).
    async fn configure_both(&self, version: u64) {
        let c = json!({
            "version": version,
            "ntfy": { "server": self.sink.url, "topic": MARKER_TOPIC, "token": MARKER_TOKEN },
            "email": { "host": "127.0.0.1", "port": self.mail.port, "tls": "none", "from": "askbar@example.com", "to": "owner@example.com" }
        });
        assert_eq!(self.admin(M::PUT, "/config", Some(c)).await.status, 200);
    }

    /// Waits for `n` messages at the mail sink.
    async fn mail_has(&self, n: usize) -> Vec<Mail> {
        for _ in 0..200 {
            if self.mail.seen.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.mail.seen.lock().unwrap().clone()
    }

    /// Waits for `n` requests at the sink.
    async fn sink_has(&self, n: usize) -> Vec<Seen> {
        for _ in 0..200 {
            if self.sink.seen.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.sink.seen.lock().unwrap().clone()
    }

    /// The auditor's alert delivery records, once `n` are there.
    async fn deliveries(&self, n: usize) -> Vec<Value> {
        let mut rows = Vec::new();
        for _ in 0..200 {
            rows = self.auditor.recorder.reader().map(|c| askbar_audit::list_alert_deliveries(&c, None, None).unwrap()).unwrap_or_default();
            if rows.len() >= n {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        rows
    }
}

fn unsealed() -> Value {
    json!({ "kind": "unsealed", "severity": "warning", "at": now() })
}

#[tokio::test]
async fn every_route_needs_its_own_links_token_and_a_token_of_the_other_listener_is_no_good() {
    let t = start().await;
    let (a, b) = (&t.admin_url, &t.events_url);
    assert_eq!(t.call(a, None, M::GET, "/status", None).await.status, 401);
    assert_eq!(t.call(a, Some("tok_wrong"), M::GET, "/status", None).await.status, 401);
    // The auditor's token on the admin's listener, and the admin's on the auditor's, are unknown there.
    assert_eq!(t.call(a, Some(&token("auditor-alerter")), M::GET, "/status", None).await.status, 401);
    assert_eq!(t.call(b, Some(&token("admin-alerter")), M::POST, "/events", Some(unsealed())).await.status, 401);
    // The auditor's listener has no other route: the admin's routes do not exist there, so the auditor's token finds nothing.
    for (m, p) in [(M::GET, "/status"), (M::PUT, "/config"), (M::POST, "/restart"), (M::POST, "/test")] {
        assert_eq!(t.call(b, Some(&token("auditor-alerter")), m.clone(), p, Some(json!({}))).await.status, 404, "{m} {p}");
    }
    // On the admin's listener the auditor's token is unknown, so every route is 401 to it.
    for (m, p) in [(M::PUT, "/config"), (M::POST, "/restart"), (M::POST, "/test"), (M::POST, "/events")] {
        assert_eq!(t.call(a, Some(&token("auditor-alerter")), m.clone(), p, Some(json!({}))).await.status, 401, "{m} {p}");
    }
    // The health check needs no token, on both.
    for base in [a, b] {
        assert_eq!(t.http.get(format!("{base}/api/alerter/v1/health")).send().await.unwrap().status(), 200);
    }
}

#[tokio::test]
async fn it_answers_503_not_configured_until_the_admin_pushes_and_the_status_says_so() {
    let t = start().await;
    let st = t.admin(M::GET, "/status", None).await;
    assert_eq!((st.status, st.body["config_version"].clone()), (200, Value::Null));
    for (m, p, body) in [(M::POST, "/events", Some(unsealed())), (M::POST, "/test", None)] {
        let r = t.admin(m.clone(), p, body).await;
        assert_eq!((r.status, r.code()), (503, "not_configured".to_string()), "{m} {p}");
    }
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.status, 503);
    t.configure().await;
    assert_eq!(t.admin(M::GET, "/status", None).await.body["config_version"], 1);
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.status, 202);
}

#[tokio::test]
async fn a_configuration_that_cannot_be_used_is_refused_and_changes_nothing() {
    let t = start().await;
    for bad in [
        json!({ "version": 1, "ntfy": { "server": "not a url", "topic": "t" } }),
        json!({ "version": 1, "ntfy": { "server": "https://ntfy.example.com", "topic": "a b" } }),
        json!({ "version": 1, "smtp": {} }),
        json!({ "ntfy": null }),
    ] {
        let r = t.admin(M::PUT, "/config", Some(bad)).await;
        assert_eq!((r.status, r.code()), (400, "bad_request".to_string()));
    }
    assert_eq!(t.admin(M::GET, "/status", None).await.body["config_version"], Value::Null);
}

#[tokio::test]
async fn an_event_is_delivered_with_its_fixed_sentence_and_the_priority_of_its_severity() {
    let t = start().await;
    t.configure().await;
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.status, 202);
    let crit = json!({ "kind": "audit_disk_critical", "severity": "critical", "at": now(), "fields": { "free_mb": 200 } });
    assert_eq!(t.auditor(M::POST, "/events", Some(crit)).await.status, 202);
    let seen = t.sink_has(2).await;
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].path, format!("/{MARKER_TOPIC}"));
    assert_eq!((seen[0].priority.as_str(), seen[0].tags.as_str()), ("4", "warning"));
    assert_eq!(seen[0].title, "Audit content opened");
    assert!(seen[0].body.starts_with("The administrator unsealed audit content."), "{}", seen[0].body);
    assert_eq!((seen[1].priority.as_str(), seen[1].tags.as_str()), ("5", "rotating_light"));
    assert!(seen[1].body.contains("200 MB free"), "{}", seen[1].body);
    // The token goes as a bearer token, to the server that was configured.
    assert_eq!(seen[0].authorization, format!("Bearer {MARKER_TOKEN}"));
    // Each outcome is on record with the auditor: the kind, the channel and a count, and nothing else.
    let rows = t.deliveries(2).await;
    assert_eq!(rows.len(), 2);
    for r in &rows {
        assert_eq!(
            (r["caller"].as_str(), r["channel"].as_str(), r["outcome"].as_str(), r["attempts"].as_i64()),
            (Some("alerter"), Some("ntfy"), Some("sent"), Some(1))
        );
    }
    let mut kinds: Vec<&str> = rows.iter().map(|r| r["kind"].as_str().unwrap()).collect();
    kinds.sort();
    assert_eq!(kinds, ["audit_disk_critical", "unsealed"]);
}

#[tokio::test]
async fn each_link_may_send_only_its_own_kinds_and_only_what_a_kind_takes() {
    let t = start().await;
    t.configure().await;
    let chain = json!({ "kind": "chain_mismatch", "severity": "critical", "at": now() });
    // The auditor may not send the admin's kind, nor the admin the auditor's.
    assert_eq!((t.auditor(M::POST, "/events", Some(chain.clone())).await.status,), (403,));
    assert_eq!(t.admin(M::POST, "/events", Some(unsealed())).await.status, 403);
    assert_eq!(t.admin(M::POST, "/events", Some(chain)).await.status, 202);
    // The alerter's own kinds come over no link.
    for (k, fields) in [("test", json!({})), ("release_available", json!({ "version": "1.0.0" }))] {
        let e = json!({ "kind": k, "severity": "info", "at": now(), "fields": fields });
        assert_eq!(t.auditor(M::POST, "/events", Some(e.clone())).await.status, 403, "{k}");
        assert_eq!(t.admin(M::POST, "/events", Some(e)).await.status, 403, "{k}");
    }
    // Free text, a dedupe key, an unknown kind or field, the wrong severity, a missing or an extra field: 400, and nothing is sent.
    for bad in [
        json!({ "kind": "unsealed", "severity": "warning", "at": now(), "message": "hello" }),
        json!({ "kind": "unsealed", "severity": "warning", "at": now(), "dedupe_key": "x" }),
        json!({ "kind": "unsealed", "severity": "warning", "at": now(), "fields": { "text": "hi" } }),
        json!({ "kind": "kill_switch_changed", "severity": "warning", "at": now() }),
        json!({ "kind": "unsealed", "severity": "critical", "at": now() }),
        json!({ "kind": "audit_disk_low", "severity": "warning", "at": now() }),
        json!({ "kind": "unsealed", "severity": "warning", "at": now(), "fields": { "count": 1 } }),
        json!({ "kind": "component_silent", "severity": "critical", "at": now(), "fields": { "link": "<script>", "minutes": 5 } }),
    ] {
        let r = t.auditor(M::POST, "/events", Some(bad.clone())).await;
        assert_eq!((r.status, r.code()), (400, "bad_request".to_string()), "{bad}");
    }
    // Only the admin's chain_mismatch got through.
    let seen = t.sink_has(1).await;
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].title, "The audit chain does not match");
}

#[tokio::test]
async fn a_repeat_within_the_hour_is_held_back_counted_and_recorded_as_suppressed() {
    let t = start().await;
    t.configure().await;
    let silent = |minutes| json!({ "kind": "component_silent", "severity": "critical", "at": now(), "fields": { "link": "server-auditor", "minutes": minutes } });
    assert_eq!(t.auditor(M::POST, "/events", Some(silent(5))).await.body["suppressed"], false);
    // The same condition with a larger number is still the same alert.
    assert_eq!(t.auditor(M::POST, "/events", Some(silent(35))).await.body["suppressed"], true);
    // Another link is another alert.
    let other = json!({ "kind": "component_silent", "severity": "critical", "at": now(), "fields": { "link": "model-gateway-auditor", "minutes": 5 } });
    assert_eq!(t.auditor(M::POST, "/events", Some(other)).await.body["suppressed"], false);
    let seen = t.sink_has(2).await;
    assert_eq!(seen.len(), 2, "two deliveries, not three");
    let rows = t.deliveries(3).await;
    let mut outcomes: Vec<&str> = rows.iter().map(|r| r["outcome"].as_str().unwrap()).collect();
    outcomes.sort();
    assert_eq!(outcomes, ["sent", "sent", "suppressed"]);
}

#[tokio::test]
async fn the_ledger_survives_a_restart_so_the_count_does_not_start_again() {
    let t = start().await;
    t.configure().await;
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.body["suppressed"], false);
    t.sink_has(1).await;
    let (dir, sink) = (t.dir.clone(), sink_again(&t));
    // A new process on the same data directory.
    let t2 = start_in(dir, sink).await;
    t2.configure().await;
    assert_eq!(t2.auditor(M::POST, "/events", Some(unsealed())).await.body["suppressed"], true, "held back by the ledger");
}

/// A sink for a restarted alerter (the old one's address is not needed again).
fn sink_again(t: &T) -> Sink {
    Sink { url: t.sink.url.clone(), seen: t.sink.seen.clone(), status: t.sink.status.clone(), fail_first: t.sink.fail_first.clone() }
}

#[tokio::test]
async fn a_failed_delivery_is_retried_with_backoff_and_the_tries_are_recorded() {
    let t = start().await;
    t.configure().await;
    t.sink.fail_first.store(2, Ordering::SeqCst);
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.status, 202);
    assert_eq!(t.sink_has(3).await.len(), 3, "two failures and then the delivery");
    let rows = t.deliveries(1).await;
    assert_eq!((rows[0]["outcome"].as_str(), rows[0]["attempts"].as_i64()), (Some("sent"), Some(3)));
    let st = t.admin(M::GET, "/status", None).await.body;
    assert!(st["last_delivery_ok_at"].as_i64().is_some() && st["last_delivery_failed_at"].as_i64().is_some(), "{st}");
}

#[tokio::test]
async fn an_alert_that_never_gets_through_is_given_up_and_recorded_as_failed() {
    let t = start().await;
    t.configure().await;
    t.sink.status.store(500, Ordering::SeqCst);
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.status, 202);
    let rows = t.deliveries(1).await;
    assert_eq!((rows[0]["outcome"].as_str(), rows[0]["attempts"].as_i64()), (Some("failed"), Some(3)));
    assert_eq!(t.sink.seen.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn the_test_alert_says_what_each_channel_really_did() {
    let t = start().await;
    t.configure().await;
    let ok = t.admin(M::POST, "/test", None).await;
    assert_eq!(
        (ok.status, ok.body["results"][0]["channel"].clone(), ok.body["results"][0]["ok"].clone()),
        (200, json!("ntfy"), json!(true))
    );
    assert_eq!(t.sink.seen.lock().unwrap()[0].title, "Askbar test alert");
    assert_eq!(t.sink.seen.lock().unwrap()[0].priority, "3");
    for (code, problem) in [(401u16, "unauthorized"), (403, "unauthorized"), (500, "refused"), (404, "refused")] {
        t.sink.status.store(code, Ordering::SeqCst);
        let r = t.admin(M::POST, "/test", None).await;
        assert_eq!((r.body["results"][0]["ok"].clone(), r.body["results"][0]["problem"].as_str()), (json!(false), Some(problem)), "{code}");
    }
    // A server that is not there.
    let c = json!({ "version": 2, "ntfy": { "server": "http://127.0.0.1:9", "topic": MARKER_TOPIC } });
    assert_eq!(t.admin(M::PUT, "/config", Some(c)).await.status, 200);
    let r = t.admin(M::POST, "/test", None).await;
    assert_eq!(r.body["results"][0]["problem"], "unreachable");
    // With no channel at all, the result is empty.
    assert_eq!(t.admin(M::PUT, "/config", Some(json!({ "version": 3 }))).await.status, 200);
    assert_eq!(t.admin(M::POST, "/test", None).await.body["results"], json!([]));
}

#[tokio::test]
async fn a_marker_secret_is_in_no_response_no_ledger_and_no_audit_record() {
    let t = start().await;
    t.configure().await;
    let mut bodies = vec![t.admin(M::GET, "/status", None).await.text];
    bodies.push(t.admin(M::POST, "/test", None).await.text);
    bodies.push(t.auditor(M::POST, "/events", Some(unsealed())).await.text);
    // A refusal does not repeat a value it was sent.
    let bad = json!({ "version": 9, "ntfy": { "server": "https://x.example.com", "topic": MARKER_TOPIC, "token": "has a space", "extra": MARKER_TOKEN } });
    bodies.push(t.admin(M::PUT, "/config", Some(bad)).await.text);
    bodies.push(
        t.admin(
            M::PUT,
            "/config",
            Some(json!({ "version": 1, "ntfy": { "server": t.sink.url, "topic": MARKER_TOPIC, "token": MARKER_TOKEN } })),
        )
        .await
        .text,
    );
    t.sink_has(1).await;
    t.deliveries(1).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    for b in &bodies {
        assert!(!b.contains(MARKER_TOPIC) && !b.contains(MARKER_TOKEN), "a response held a secret: {b}");
    }
    // Nothing the alerter wrote to disk, and nothing the auditor stored, holds either.
    fn grep(dir: &std::path::Path, needles: &[&str], found: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                grep(&p, needles, found);
            } else if let Ok(bytes) = std::fs::read(&p) {
                let text = String::from_utf8_lossy(&bytes);
                for n in needles {
                    if text.contains(n) {
                        found.push(format!("{} holds {n}", p.display()));
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    grep(&t.dir.join("alerter-data"), &[MARKER_TOPIC, MARKER_TOKEN], &mut found);
    grep(&t.dir.join("audit"), &[MARKER_TOPIC, MARKER_TOKEN], &mut found);
    assert!(found.is_empty(), "{found:?}");
    // The sentence the sink got never held a token or a name an event could not carry.
    for s in t.sink.seen.lock().unwrap().iter() {
        assert!(!s.body.contains(MARKER_TOKEN) && !s.title.contains(MARKER_TOKEN));
    }
    let _ = (&t.auditor_url, &t.alerter);
}

#[tokio::test]
async fn the_admin_can_restart_it_even_before_it_is_configured() {
    let t = start().await;
    let r = t.admin(M::POST, "/restart", None).await;
    assert_eq!(r.status, 202, "{}", r.text);
}

#[tokio::test]
async fn an_alert_goes_to_every_channel_that_is_set_up_as_one_message_with_its_own_sentence() {
    let t = start().await;
    t.configure_both(1).await;
    assert_eq!(t.auditor(M::POST, "/events", Some(unsealed())).await.status, 202);
    let ntfy = t.sink_has(1).await;
    let mail = t.mail_has(1).await;
    assert_eq!((ntfy.len(), mail.len()), (1, 1));
    assert!(mail[0].from.contains("askbar@example.com") && mail[0].to.contains("owner@example.com"), "{mail:?}");
    assert!(mail[0].data.contains("Subject: [warning] ") && mail[0].data.contains(&ntfy[0].title), "{}", mail[0].data);
    // The body is quoted-printable: undo its soft line breaks to compare.
    let unwrapped = mail[0].data.replace("=\r\n", "");
    assert!(unwrapped.contains(&ntfy[0].body), "the same sentence goes to both: {}", mail[0].data);
    // Each channel's outcome is its own record at the auditor.
    let rows = t.deliveries(2).await;
    let mut channels: Vec<String> = rows.iter().map(|r| r["channel"].as_str().unwrap().to_string()).collect();
    channels.sort();
    assert_eq!((channels, rows[0]["outcome"].clone()), (vec!["email".to_string(), "ntfy".to_string()], json!("sent")));
}

#[tokio::test]
async fn email_alone_works_and_a_channel_that_fails_does_not_hold_up_the_other() {
    let t = start().await;
    let c = json!({ "version": 1, "email": { "host": "127.0.0.1", "port": t.mail.port, "tls": "none", "from": "a@example.com", "to": "b@example.com" } });
    assert_eq!(t.admin(M::PUT, "/config", Some(c)).await.status, 200);
    t.auditor(M::POST, "/events", Some(unsealed())).await;
    assert_eq!(t.mail_has(1).await.len(), 1);
    assert!(t.sink.seen.lock().unwrap().is_empty(), "no ntfy channel is set up");
    // ntfy is down, email is not: the email arrives at once, and ntfy is retried on its own.
    t.sink.status.store(500, Ordering::SeqCst);
    t.configure_both(2).await;
    t.auditor(M::POST, "/events", Some(json!({ "kind": "unseal_timeouts_changed", "severity": "warning", "at": now() }))).await;
    assert_eq!(t.mail_has(2).await.len(), 2);
    assert!(t.sink_has(2).await.len() >= 2, "ntfy was tried again");
    assert_eq!(t.mail.seen.lock().unwrap().len(), 2, "a retry of one channel does not send the other again");
}

#[tokio::test]
async fn the_test_alert_covers_email_and_names_why_it_failed_in_the_alerters_words() {
    let t = start().await;
    t.configure_both(1).await;
    let r = t.admin(M::POST, "/test", None).await;
    let by = |r: &Reply, ch: &str| r.body["results"].as_array().unwrap().iter().find(|x| x["channel"] == ch).cloned().unwrap();
    assert_eq!((by(&r, "email")["ok"].clone(), by(&r, "ntfy")["ok"].clone()), (json!(true), json!(true)));
    assert!(t.mail.seen.lock().unwrap()[0].data.contains("Askbar test alert"));
    // The server refuses the recipient.
    t.mail.reject.store(550, Ordering::SeqCst);
    let r = t.admin(M::POST, "/test", None).await;
    assert_eq!((by(&r, "email")["ok"].clone(), by(&r, "email")["problem"].clone()), (json!(false), json!("refused")));
    // Nothing listens.
    let down =
        json!({ "version": 2, "email": { "host": "127.0.0.1", "port": 9, "tls": "none", "from": "a@example.com", "to": "b@example.com" } });
    assert_eq!(t.admin(M::PUT, "/config", Some(down)).await.status, 200);
    let r = t.admin(M::POST, "/test", None).await;
    assert_eq!(by(&r, "email")["problem"], "unreachable");
    // TLS is required and the server does not offer it, so it is not sent in the clear.
    let tls = json!({ "version": 3, "email": { "host": "127.0.0.1", "port": t.mail.port, "tls": "starttls", "from": "a@example.com", "to": "b@example.com" } });
    t.mail.reject.store(0, Ordering::SeqCst);
    let before = t.mail.seen.lock().unwrap().len();
    assert_eq!(t.admin(M::PUT, "/config", Some(tls)).await.status, 200);
    let r = t.admin(M::POST, "/test", None).await;
    assert_eq!(
        (by(&r, "email")["ok"].clone(), by(&r, "email")["problem"].clone()),
        (json!(false), json!("refused")),
        "a server without STARTTLS is refused: {}",
        r.text
    );
    assert_eq!(t.mail.seen.lock().unwrap().len(), before, "nothing was sent without TLS");
}

#[tokio::test]
async fn an_email_configuration_that_cannot_be_used_is_refused_without_repeating_a_value() {
    let t = start().await;
    let mail = |m: Value| json!({ "version": 1, "email": m });
    let ok = json!({ "host": "mail.example.com", "port": 587, "tls": "starttls", "user": MARKER_MAIL_USER, "password": MARKER_MAIL_PASSWORD, "from": "a@example.com", "to": "b@example.com" });
    let mut bodies = Vec::new();
    for (change, why) in [
        (json!({ "tls": "none" }), "a password without TLS"),
        (json!({ "password": null }), "a user without a password"),
        (json!({ "host": "bad host" }), "a host with a space"),
        (json!({ "to": "a@example.com, b@example.com" }), "two recipients"),
        (json!({ "from": "nobody" }), "no address"),
        (json!({ "port": 0 }), "port 0"),
        (json!({ "port": 70000 }), "port out of range"),
        (json!({ "tls": "ssl" }), "an unknown TLS mode"),
        (json!({ "extra": MARKER_MAIL_PASSWORD }), "an unknown field"),
    ] {
        let mut m = ok.clone();
        for (k, v) in change.as_object().unwrap() {
            m[k] = v.clone();
        }
        let r = t.admin(M::PUT, "/config", Some(mail(m))).await;
        assert_eq!((r.status, r.code()), (400, "bad_request".to_string()), "{why}: {}", r.text);
        bodies.push(r.text);
    }
    assert!(!t.alerter.is_configured(), "a refused configuration changes nothing");
    // A good one is taken, and neither it nor anything after it shows the password or the user name.
    bodies.push(t.admin(M::PUT, "/config", Some(mail(ok))).await.text);
    bodies.push(t.admin(M::GET, "/status", None).await.text);
    bodies.push(t.admin(M::POST, "/test", None).await.text);
    t.auditor(M::POST, "/events", Some(unsealed())).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for b in &bodies {
        assert!(!b.contains(MARKER_MAIL_PASSWORD) && !b.contains(MARKER_MAIL_USER) && !b.contains("mail.example.com"), "{b}");
    }
    let mut found = Vec::new();
    for dir in ["alerter-data", "audit"] {
        fn walk(d: &std::path::Path, found: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, found);
                } else if let Ok(b) = std::fs::read(&p) {
                    let text = String::from_utf8_lossy(&b);
                    for n in [MARKER_MAIL_PASSWORD, MARKER_MAIL_USER, "mail.example.com", "b@example.com"] {
                        if text.contains(n) {
                            found.push(format!("{} holds {n}", p.display()));
                        }
                    }
                }
            }
        }
        walk(&t.dir.join(dir), &mut found);
    }
    assert!(found.is_empty(), "{found:?}");
}

// ------------------------------------------------------------------ the release check (ADR 0055, D11)

/// A stand-in for GitHub's latest-release API: it answers with `body` and status, and keeps the header names and the
/// `User-Agent` of each request it gets.
/// The `User-Agent` and the header names of one request.
type Asked = Arc<Mutex<Vec<(String, Vec<String>)>>>;

struct ReleaseMock {
    url: String,
    body: Arc<Mutex<String>>,
    status: Arc<AtomicU16>,
    asked: Asked,
}

async fn release_mock(tag: &str) -> ReleaseMock {
    let body = Arc::new(Mutex::new(json!({ "tag_name": tag, "name": "x", "draft": false, "prerelease": false }).to_string()));
    let status = Arc::new(AtomicU16::new(200));
    let asked: Asked = Default::default();
    let (b, st, a) = (body.clone(), status.clone(), asked.clone());
    let app = axum::Router::new().route(
        "/repos/Thewuuryar/askbar/releases/latest",
        axum::routing::get(move |h: axum::http::HeaderMap| {
            let (b, st, a) = (b.clone(), st.clone(), a.clone());
            async move {
                let ua = h.get("user-agent").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
                a.lock().unwrap().push((ua, h.keys().map(|k| k.as_str().to_string()).collect()));
                (axum::http::StatusCode::from_u16(st.load(Ordering::SeqCst)).unwrap(), b.lock().unwrap().clone())
            }
        }),
    );
    let url = format!("{}/repos/Thewuuryar/askbar/releases/latest", serve(app).await);
    ReleaseMock { url, body, status, asked }
}

fn point_at(t: &T, mock: &ReleaseMock) {
    t.alerter.releases.set_source(askbar_alerter::releases::Source {
        url: mock.url.clone(),
        current: "0.6.0".into(),
        every: Duration::from_millis(50),
        retry: Duration::from_millis(50),
    });
}

#[tokio::test]
async fn a_newer_release_is_announced_once_with_nothing_about_the_install_in_the_request() {
    let t = start().await;
    t.configure().await;
    let mock = release_mock("v1.2.0").await;
    point_at(&t, &mock);
    t.alerter.release_round().await;
    let seen = t.sink_has(1).await;
    assert_eq!(
        (seen.len(), seen[0].title.as_str(), seen[0].body.as_str(), seen[0].priority.as_str()),
        (1, "A new Askbar release", "Askbar 1.2.0 is available.", "3")
    );
    // Rounds after that, and a check that finds the same version, say nothing more.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(60)).await;
        t.alerter.release_round().await;
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(t.sink.seen.lock().unwrap().len(), 1, "one alert for one version");
    assert!(mock.asked.lock().unwrap().len() >= 2, "it did ask again");
    // The request: a fixed User-Agent, and no header that names the install (no cookie, no authorization, no custom header).
    for (ua, headers) in mock.asked.lock().unwrap().iter() {
        assert_eq!(ua, "askbar-release-check");
        for h in headers {
            assert!(["host", "user-agent", "accept"].contains(&h.as_str()), "an unexpected header: {h}");
        }
    }
    // A later release is announced too, and the admin can read what was found.
    *mock.body.lock().unwrap() = json!({ "tag_name": "v1.3.0" }).to_string();
    tokio::time::sleep(Duration::from_millis(60)).await;
    t.alerter.release_round().await;
    assert_eq!(t.sink_has(2).await[1].body, "Askbar 1.3.0 is available.");
    let r = t.admin(M::GET, "/releases", None).await;
    assert_eq!(
        (r.status, r.body["current"].clone(), r.body["latest"].clone(), r.body["newer"].clone()),
        (200, json!("0.6.0"), json!("1.3.0"), json!(true)),
        "{}",
        r.text
    );
    assert!(r.body["checked_at"].is_i64());
}

#[tokio::test]
async fn the_release_route_is_the_admins_alone_and_waits_for_the_configuration() {
    let t = start().await;
    assert_eq!(t.admin(M::GET, "/releases", None).await.code(), "not_configured");
    t.configure().await;
    let r = t.admin(M::GET, "/releases", None).await;
    assert_eq!((r.status, r.body["latest"].clone(), r.body["newer"].clone()), (200, Value::Null, json!(false)));
    assert_eq!(
        t.call(&t.events_url, Some(&token("auditor-alerter")), M::GET, "/releases", None).await.status,
        404,
        "the auditor's listener has no such route"
    );
    assert_eq!(t.call(&t.admin_url, Some("wrong"), M::GET, "/releases", None).await.status, 401);
}

#[tokio::test]
async fn the_same_version_an_older_one_a_failure_and_an_answer_that_is_not_a_release_announce_nothing() {
    let t = start().await;
    t.configure().await;
    let mock = release_mock("v0.6.0").await;
    point_at(&t, &mock);
    t.alerter.release_round().await;
    assert_eq!(t.admin(M::GET, "/releases", None).await.body["newer"], json!(false), "the same version");
    for (status, body, why) in [
        (200, json!({ "tag_name": "v0.5.0" }).to_string(), "an older version"),
        (500, "{}".to_string(), "an error"),
        (404, json!({ "message": "Not Found" }).to_string(), "no release"),
        (200, "not json".to_string(), "not JSON"),
        (200, json!({ "tag_name": "v2.0.0", "prerelease": true }).to_string(), "a pre-release"),
        (200, json!({ "tag_name": "v2.0.0", "draft": true }).to_string(), "a draft"),
        (200, json!({ "tag_name": "v2.0.0-rc1" }).to_string(), "a version with a suffix"),
        (200, json!({ "tag_name": "v2.0.0 <script>alert(1)</script>" }).to_string(), "a tag with markup"),
        (200, json!({ "tag_name": "v1.0.0\nInjected: yes" }).to_string(), "a tag with a line break"),
    ] {
        t.mock_status(&mock, status, &body);
        tokio::time::sleep(Duration::from_millis(60)).await;
        t.alerter.release_round().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(t.sink.seen.lock().unwrap().is_empty(), "{why}: an alert was sent");
        let r = t.admin(M::GET, "/releases", None).await;
        assert!(["0.5.0", "0.6.0"].contains(&r.body["latest"].as_str().unwrap_or("0.6.0")), "{why}: {}", r.text);
        assert_eq!(r.body["newer"], json!(false), "{why}");
    }
    assert!(t.admin(M::GET, "/releases", None).await.body["failed_at"].is_i64(), "a failure is remembered");
}

impl T {
    fn mock_status(&self, mock: &ReleaseMock, status: u16, body: &str) {
        mock.status.store(status, Ordering::SeqCst);
        *mock.body.lock().unwrap() = body.to_string();
    }
}

#[tokio::test]
async fn a_release_is_announced_when_a_channel_is_set_up_later_and_not_again_after_a_restart() {
    let t = start().await;
    let mock = release_mock("v1.2.0").await;
    point_at(&t, &mock);
    // Configured with no channel: the release is found but there is nowhere to say so, and nothing is marked as said.
    assert_eq!(t.admin(M::PUT, "/config", Some(json!({ "version": 1 }))).await.status, 200);
    t.alerter.release_round().await;
    assert_eq!(t.admin(M::GET, "/releases", None).await.body["latest"], json!("1.2.0"));
    assert!(t.sink.seen.lock().unwrap().is_empty());
    t.configure().await;
    t.alerter.release_round().await;
    assert_eq!(t.sink_has(1).await.len(), 1);
    // A restart on the same volume knows it has said so and when it asked.
    let asked = mock.asked.lock().unwrap().len();
    let dir = t.dir.clone();
    let sink = sink_again(&t);
    let again = start_in(dir.clone(), sink).await;
    again.alerter.releases.set_source(askbar_alerter::releases::Source {
        url: mock.url.clone(),
        current: "0.6.0".into(),
        every: Duration::from_secs(86_400),
        retry: Duration::from_secs(3600),
    });
    again.configure().await;
    again.alerter.release_round().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(again.sink.seen.lock().unwrap().len(), 1, "not announced a second time");
    assert_eq!(mock.asked.lock().unwrap().len(), asked, "and not asked again before a day is up");
}

#[tokio::test]
async fn with_the_check_off_nothing_is_asked() {
    let t = start().await;
    t.configure().await;
    let mock = release_mock("v9.0.0").await;
    t.alerter.releases.set_source(askbar_alerter::releases::Source {
        url: "off".into(),
        current: "0.6.0".into(),
        every: Duration::from_millis(1),
        retry: Duration::from_millis(1),
    });
    t.alerter.release_round().await;
    assert!(mock.asked.lock().unwrap().is_empty() && t.sink.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_running_check_announces_at_once_when_a_channel_is_set_up_after_the_release_was_found() {
    let t = start().await;
    let mock = release_mock("v1.2.0").await;
    t.alerter.releases.set_source(askbar_alerter::releases::Source {
        url: mock.url.clone(),
        current: "0.6.0".into(),
        every: Duration::from_secs(86_400),
        retry: Duration::from_secs(3600),
    });
    tokio::spawn(t.alerter.clone().releases_forever());
    // No channel: the check runs, finds it, and waits (an hour, if nothing wakes it).
    assert_eq!(t.admin(M::PUT, "/config", Some(json!({ "version": 1 }))).await.status, 200);
    for _ in 0..100 {
        if t.alerter.release_status().latest.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(t.alerter.release_status().latest.as_deref(), Some("1.2.0"));
    assert!(t.sink.seen.lock().unwrap().is_empty());
    t.configure().await;
    assert_eq!(t.sink_has(1).await[0].body, "Askbar 1.2.0 is available.", "the new settings woke the check");
    assert_eq!(mock.asked.lock().unwrap().len(), 1, "and it did not ask again");
}

#[tokio::test]
async fn the_export_is_the_ledger_and_release_state_encrypted_to_the_pushed_key_and_needs_one() {
    use askbar_service::backup;
    let t = start().await;
    // Before the settings arrive: not configured. With settings but no key: no export.
    assert_eq!(t.admin(M::POST, "/exports", None).await.code(), "not_configured");
    t.configure().await;
    let r = t.admin(M::POST, "/exports", None).await;
    assert_eq!((r.status, r.code().as_str()), (503, "backup_key_missing"));
    // A key that is not a public key is no key.
    let c = json!({ "version": 2, "backup_public_key": "not-a-key" });
    assert_eq!(t.admin(M::PUT, "/config", Some(c)).await.status, 200);
    assert_eq!(t.admin(M::POST, "/exports", None).await.code(), "backup_key_missing");
    // Put some state on the volume: a held-back alert writes the ledger.
    t.configure().await;
    t.auditor(M::POST, "/events", Some(unsealed())).await;
    t.sink_has(1).await;
    let key = backup::generate_with("a long backup passphrase 1", 10).unwrap();
    let c = json!({ "version": 3, "ntfy": { "server": t.sink.url, "topic": MARKER_TOPIC, "token": MARKER_TOKEN }, "backup_public_key": key.recipient });
    assert_eq!(t.admin(M::PUT, "/config", Some(c)).await.status, 200);
    let resp = t.http.post(format!("{}/api/alerter/v1/exports", t.admin_url)).bearer_auth(token("admin-alerter")).send().await.unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.headers()["content-type"], "application/octet-stream");
    let bytes = resp.bytes().await.unwrap();
    let plain_marker = [MARKER_TOKEN, MARKER_TOPIC];
    assert!(plain_marker.iter().all(|m| !String::from_utf8_lossy(&bytes).contains(m)));
    let identity = backup::unlock(&key.protected, "a long backup passphrase 1").unwrap();
    let files = askbar_service::tar::read(&backup::decrypt(&identity, &bytes).unwrap()).unwrap();
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"ledger.json"), "{names:?}");
    // What is in it holds no delivery secret.
    for (_, data) in &files {
        let text = String::from_utf8_lossy(data);
        assert!(!text.contains(MARKER_TOKEN) && !text.contains(MARKER_TOPIC));
    }
    // It is not on the auditor's listener at all.
    assert_eq!(t.auditor(M::POST, "/exports", None).await.status, 404);
}
