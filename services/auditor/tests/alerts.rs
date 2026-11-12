//! The auditor's alert rules against a real in-process auditor (plan step 4.2, ADR 0051, ADR 0055): what it sends the alerter
//! because of the records it takes, the requests on its links and the time. The alerter is a collector; the clock is a number.

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::rules::Alerts;
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_proto::alerter::{Event, Fields, Kind, Sender, Severity};
use reqwest::Method as M;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const ADMIN_APPEND: &str = "admin-auditor-append";
const ADMIN_READ: &str = "admin-auditor-read";
const SERVER: &str = "server-auditor";
const GATEWAY: &str = "model-gateway-auditor";
const ALERTER: &str = "alerter-auditor";

/// What the rules sent, in order.
#[derive(Default)]
struct Collector(Mutex<Vec<Event>>);

impl Alerts for Collector {
    fn send(&self, kind: Kind, severity: Severity, fields: Fields) {
        self.0.lock().unwrap().push(Event { kind, severity, at: 0, fields });
    }
}

struct T {
    url: String,
    dir: PathBuf,
    auditor: Arc<Auditor>,
    http: reqwest::Client,
    sent: Arc<Collector>,
    /// What the volume has free, by the byte.
    free: Arc<AtomicU64>,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn token(link: &str) -> String {
    format!("tok_{link}_secret")
}

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

const GIB: u64 = 1024 * 1024 * 1024;

/// An auditor with rules over a collector, on `dir` (a data directory already holding records, or a new one).
async fn start_on(dir: PathBuf) -> T {
    let accept = dir.join("tokens/accept");
    std::fs::create_dir_all(&accept).unwrap();
    for link in [ADMIN_APPEND, ADMIN_READ, SERVER, GATEWAY, ALERTER] {
        std::fs::write(accept.join(format!("{link}.sha256")), format!("{}\n", hex(&token(link)))).unwrap();
    }
    let limits = Limits::default();
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("data/audit.db")).unwrap(), limits.max_queued);
    let free = Arc::new(AtomicU64::new(100 * GIB));
    let f = free.clone();
    let free_space: FreeSpace = Arc::new(move || Some(f.load(Ordering::Relaxed)));
    let sent = Arc::new(Collector::default());
    let auditor = Auditor::new(recorder, limits, free_space).with_alerts(sent.clone());
    let app = askbar_auditor::router(auditor.clone(), askbar_auditor::links(&dir.join("tokens")));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, dir, auditor, http: reqwest::Client::new(), sent, free }
}

async fn start() -> T {
    start_on(std::env::temp_dir().join(format!("askbar-auditor-alerts-{}", ulid::Ulid::new()))).await
}

impl T {
    async fn as_(&self, link: &str, m: M, path: &str, body: Option<Value>) -> u16 {
        let mut r = self.http.request(m, format!("{}/api/audit/v1{path}", self.url)).bearer_auth(token(link));
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send().await.unwrap().status().as_u16()
    }

    /// The admin records an action: `requested`, then `done` with the facts it adds.
    async fn admin_done(&self, category: &str, action: &str, detail: Value) {
        let id = ulid::Ulid::new().to_string();
        for (outcome, detail) in [("requested", json!({})), ("done", detail)] {
            let body = json!({ "id": format!("{id}{outcome}"), "at": 1, "category": category, "action": action, "outcome": outcome, "detail": detail });
            assert_eq!(self.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(body)).await, 201);
        }
    }

    fn events(&self) -> Vec<Event> {
        self.sent.0.lock().unwrap().clone()
    }

    fn kinds(&self) -> Vec<Kind> {
        self.events().iter().map(|e| e.kind).collect()
    }
}

/// Everything the auditor sends is an event the alerter takes from its link.
fn assert_taken(events: &[Event]) {
    for e in events {
        assert_eq!(e.check(Sender::Auditor), Ok(()), "{e:?}");
    }
}

#[tokio::test]
async fn an_unseal_is_an_alert_once_it_is_recorded_and_the_request_that_began_it_is_not() {
    let t = start().await;
    let id = ulid::Ulid::new().to_string();
    let asked = json!({ "id": id, "at": 1, "category": "sealed_content", "action": "unseal", "outcome": "requested", "detail": { "method": "POST" } });
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(asked)).await, 201);
    assert!(t.kinds().is_empty(), "asked is not done");
    t.admin_done("sealed_content", "unseal", json!({ "status": 201 })).await;
    assert_eq!(t.kinds(), [Kind::Unsealed]);
    assert_taken(&t.events());
    assert_eq!(t.events()[0].fields, Fields::default());
}

#[tokio::test]
async fn changing_how_long_content_stays_unsealed_is_an_alert_and_saving_the_same_is_not() {
    let t = start().await;
    let save = |idle: u32, total: u32| json!({ "unseal_idle_minutes": idle, "unseal_total_minutes": total, "audit_retention_days": 30 });
    t.admin_done("settings", "update", save(15, 60)).await;
    assert!(t.kinds().is_empty(), "the defaults again");
    t.admin_done("settings", "update", save(10, 60)).await;
    assert_eq!(t.kinds(), [Kind::UnsealTimeoutsChanged]);
    t.admin_done("settings", "update", save(10, 60)).await;
    assert_eq!(t.kinds().len(), 1);
    assert_taken(&t.events());
    // The auditor starts again on the same records: what was recorded is where it begins, so the same limits say nothing and
    // another value does.
    let dir = t.dir.clone();
    drop(t.auditor.clone());
    let t2 = start_on(dir.clone()).await;
    t2.admin_done("settings", "update", save(10, 60)).await;
    assert!(t2.kinds().is_empty(), "{:?}", t2.kinds());
    t2.admin_done("settings", "update", save(10, 90)).await;
    assert_eq!(t2.kinds(), [Kind::UnsealTimeoutsChanged]);
    std::mem::forget(t); // the directory belongs to t2 now; t2 removes it
}

#[tokio::test]
async fn a_new_or_replaced_backup_key_is_an_alert() {
    let t = start().await;
    t.admin_done("secret", "backup_key_setup", json!({})).await;
    t.admin_done("secret", "backup_key_rotate", json!({})).await;
    assert_eq!(t.kinds(), [Kind::BackupKeyChanged, Kind::BackupKeyChanged]);
    assert_taken(&t.events());
}

#[tokio::test]
async fn a_new_retention_is_an_alert_naming_its_days_and_pushing_the_same_one_every_half_minute_is_not() {
    let t = start().await;
    let put = |days: u32| t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": days })));
    assert_eq!(put(30).await, 200, "the default again");
    assert!(t.kinds().is_empty());
    assert_eq!(put(90).await, 200);
    assert_eq!(put(90).await, 200);
    assert_eq!(put(7).await, 200);
    assert_eq!(
        t.events().iter().map(|e| (e.kind, e.fields.count)).collect::<Vec<_>>(),
        [(Kind::RetentionChanged, Some(90)), (Kind::RetentionChanged, Some(7))]
    );
    assert_taken(&t.events());
    // A refused change (under the floor) is no alert: nothing changed.
    assert_eq!(put(3).await, 400);
    assert_eq!(t.kinds().len(), 2);
}

#[tokio::test]
async fn a_volume_running_out_of_room_is_an_alert_with_the_megabytes_left_each_time_the_time_rules_run() {
    let t = start().await;
    t.auditor.tick_rules(1_000);
    assert!(t.kinds().is_empty(), "100 GiB free");
    t.free.store(900 * 1024 * 1024, Ordering::Relaxed);
    t.auditor.tick_rules(1_030);
    t.auditor.tick_rules(1_060);
    assert_eq!(
        t.events().iter().map(|e| (e.kind, e.severity, e.fields.free_mb)).collect::<Vec<_>>(),
        [(Kind::AuditDiskLow, Severity::Warning, Some(900))]
    );
    t.free.store(100 * 1024 * 1024, Ordering::Relaxed);
    t.auditor.tick_rules(1_090);
    assert_eq!(
        t.events().last().map(|e| (e.kind, e.severity, e.fields.free_mb)),
        Some((Kind::AuditDiskCritical, Severity::Critical, Some(100)))
    );
    assert_taken(&t.events());
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

#[tokio::test]
async fn the_admin_restarting_three_times_in_ten_minutes_is_a_restart_loop_from_its_hellos() {
    let t = start().await;
    for boot in ["b1", "b2", "b3"] {
        assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/hello", Some(json!({ "boot_id": boot }))).await, 200);
    }
    assert!(t.kinds().is_empty(), "two restarts");
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/hello", Some(json!({ "boot_id": "b4" }))).await, 200);
    assert_eq!(
        t.events().iter().map(|e| (e.kind, e.fields.service.clone(), e.fields.count, e.fields.minutes)).collect::<Vec<_>>(),
        [(Kind::RestartLoop, Some("admin".to_string()), Some(3), Some(10))]
    );
    assert_taken(&t.events());
    // The server's hellos are not the auditor's to count.
    for boot in ["s1", "s2", "s3", "s4", "s5"] {
        assert_eq!(t.as_(SERVER, M::POST, "/hello", Some(json!({ "boot_id": boot }))).await, 200);
    }
    assert_eq!(t.kinds().len(), 1);
}

#[tokio::test]
async fn links_that_say_nothing_for_five_minutes_are_silent_and_a_link_that_speaks_is_not() {
    let t = start().await;
    // The server and the admin's read link speak; the gateway and the alerter never do.
    assert_eq!(t.as_(SERVER, M::GET, "/status", None).await, 200);
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/status", None).await, 200);
    t.auditor.tick_rules(now() + 60);
    assert!(t.kinds().is_empty(), "one minute");
    t.auditor.tick_rules(now() + 301);
    // Every link was last heard a moment ago on this clock, so all of them are past five minutes at +301: the rule is a
    // comparison with the time of the last request, which the status route reports.
    let silent: Vec<_> = t.events().iter().map(|e| (e.kind, e.severity, e.fields.minutes)).collect();
    assert_eq!(silent.len(), 4, "{silent:?}");
    assert!(silent.iter().all(|s| *s == (Kind::ComponentSilent, Severity::Critical, Some(5))));
    assert_taken(&t.events());
    let status: Value =
        t.http.get(format!("{}/api/audit/v1/status", t.url)).bearer_auth(token(SERVER)).send().await.unwrap().json().await.unwrap();
    assert!(status["last_requests"]["server"].is_i64(), "the time of each link's last request is what it compares");
}

#[tokio::test]
async fn a_reply_with_no_access_line_for_fifteen_minutes_is_caddy_silent_and_a_line_that_comes_is_not() {
    let t = start().await;
    t.auditor.rules().unwrap().watch_caddy();
    assert_eq!(t.as_(GATEWAY, M::POST, "/hello", Some(json!({ "boot_id": "g" }))).await, 200);
    let chat = json!({ "id": "t1", "trace_id": "t1", "kind": "turn", "name": "chat", "started_at": now(), "started_ms": now() * 1000 });
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(chat)).await, 201);
    // The server and the others keep calling, so only Caddy is quiet; a minute at a time up to the fifteenth.
    let start_at = now();
    for minute in 1..15 {
        t.auditor.tick_rules(start_at + minute * 60);
    }
    let caddy = |t: &T| t.kinds().iter().filter(|k| **k == Kind::CaddySilent).count();
    assert_eq!(caddy(&t), 0);
    t.auditor.tick_rules(start_at + 15 * 60 + 1);
    assert_eq!(caddy(&t), 1);
    let e = t.events().into_iter().find(|e| e.kind == Kind::CaddySilent).unwrap();
    assert_eq!((e.severity, e.fields.minutes), (Severity::Warning, Some(15)));
    assert_taken(&[e]);
    // Caddy's line arrives (the way Caddy sends it: a line on the ingest stream), and the next reply is judged afresh.
    let line = r#"{"ts":1.0,"request":{"method":"POST","host":"h","uri":"/api/client/v1/x"},"status":200}"#;
    askbar_auditor::ingest::read_lines(std::io::Cursor::new(format!("{line}\n").into_bytes()), &t.auditor).await;
    t.auditor.tick_rules(start_at + 20 * 60);
    assert_eq!(caddy(&t), 1, "it spoke; nothing new");
}
