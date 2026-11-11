//! The auditor's API against a real in-process auditor: tokens and their scopes, acknowledged appends, boot IDs,
//! retention, and the limits that make it refuse (ADR 0051, ADR 0010).

use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_proto::audit as wire;
use reqwest::Method as M;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const SERVER: &str = "server-auditor";
const GATEWAY: &str = "model-gateway-auditor";
const ADMIN_APPEND: &str = "admin-auditor-append";
const ADMIN_READ: &str = "admin-auditor-read";

struct T {
    url: String,
    dir: PathBuf,
    auditor: Arc<Auditor>,
    http: reqwest::Client,
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

async fn start() -> T {
    start_with(Limits::default()).await
}

async fn start_with(limits: Limits) -> T {
    let dir = std::env::temp_dir().join(format!("askbar-auditor-{}", ulid::Ulid::new()));
    let accept = dir.join("tokens/accept");
    std::fs::create_dir_all(&accept).unwrap();
    for link in [SERVER, GATEWAY, ADMIN_APPEND, ADMIN_READ] {
        std::fs::write(accept.join(format!("{link}.sha256")), format!("{}\n", hex(&token(link)))).unwrap();
    }
    let recorder = Recorder::spawn(AuditDb::open(&dir.join("data/audit.db")).unwrap(), limits.max_queued);
    // Plenty of room, unless a test says otherwise.
    let free = Arc::new(AtomicU64::new(100 * 1024 * 1024 * 1024));
    let f = free.clone();
    let free_space: FreeSpace = Arc::new(move || Some(f.load(Ordering::Relaxed)));
    let auditor = Auditor::new(recorder, limits, free_space);
    let app = askbar_auditor::router(auditor.clone(), askbar_auditor::links(&dir.join("tokens")));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, dir, auditor, http: reqwest::Client::new(), free }
}

struct Reply {
    status: u16,
    body: Value,
    retry_after: Option<String>,
}

impl Reply {
    fn code(&self) -> String {
        self.body["error"].as_str().unwrap_or("").to_string()
    }
}

impl T {
    async fn call(&self, token: Option<&str>, m: M, path: &str, body: Option<Value>) -> Reply {
        let mut r = self.http.request(m, format!("{}/api/audit/v1{path}", self.url));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").map(|v| v.to_str().unwrap().to_string());
        let text = resp.text().await.unwrap();
        Reply { status, body: serde_json::from_str(&text).unwrap_or(Value::String(text)), retry_after }
    }

    /// As the link `link`, with its own token.
    async fn as_(&self, link: &str, m: M, path: &str, body: Option<Value>) -> Reply {
        self.call(Some(&token(link)), m, path, body).await
    }

    fn sql<T: rusqlite_free::FromSql>(&self, sql: &str) -> T {
        self.auditor.recorder.reader().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

/// `rusqlite` is the audit crate's; the tests only need `FromSql` for `query_row`.
mod rusqlite_free {
    pub use askbar_audit::rusqlite::types::FromSql;
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn span(id: &str, trace: &str, kind: &str) -> Value {
    json!({ "id": id, "trace_id": trace, "kind": kind, "name": "n", "started_at": now(), "started_ms": now() * 1000 })
}

fn finish(status: &str) -> Value {
    json!({ "status": status, "ended_at": now(), "ended_ms": now() * 1000 })
}

// ------------------------------------------------------------------ tokens and scopes

#[tokio::test]
async fn without_a_token_or_with_a_wrong_one_every_route_but_health_is_401() {
    let t = start().await;
    assert_eq!(t.call(None, M::GET, "/health", None).await.status, 200, "the health check carries nothing and needs no token");
    for (m, p) in [(M::POST, "/spans"), (M::GET, "/traces"), (M::GET, "/status"), (M::PUT, "/retention"), (M::POST, "/admin-actions")] {
        for tok in [None, Some("wrong"), Some(hex(&token(SERVER)).as_str())] {
            let r = t.call(tok, m.clone(), p, Some(json!({}))).await;
            assert_eq!((r.status, r.code()), (401, "unauthorized".into()), "{m} {p} {tok:?}");
        }
    }
}

#[tokio::test]
async fn each_link_may_call_only_its_own_routes_and_anything_else_is_403() {
    let t = start().await;
    let ok = |r: Reply, want: u16, what: &str| assert_eq!(r.status, want, "{what}: {:?}", r.body);

    // The server's link: hello, its own gate spans, finish and payloads. It opens no model-call span and reads nothing.
    ok(t.as_(SERVER, M::POST, "/hello", Some(json!({ "boot_id": "b1" }))).await, 200, "server hello");
    ok(t.as_(SERVER, M::POST, "/spans", Some(span("g1", "t1", "gate"))).await, 201, "server opens a gate");
    for kind in ["turn", "attempt", "tool", "agent_step"] {
        let r = t.as_(SERVER, M::POST, "/spans", Some(span("x1", "t1", kind))).await;
        assert_eq!((r.status, r.code()), (403, "forbidden".into()), "the server may not open a {kind} span");
    }
    ok(t.as_(SERVER, M::POST, "/spans/g1/finish", Some(finish("done"))).await, 201, "server finishes");
    for (m, p) in [
        (M::GET, "/traces"),
        (M::GET, "/traces/t1"),
        (M::GET, "/stats"),
        (M::GET, "/admin-actions"),
        (M::GET, "/retention"),
        (M::PUT, "/retention"),
    ] {
        assert_eq!(t.as_(SERVER, m.clone(), p, Some(json!({ "days": 30 }))).await.status, 403, "server {m} {p}");
    }
    let r = t.as_(SERVER, M::POST, "/admin-actions", Some(json!({}))).await;
    assert_eq!(r.status, 403, "only the admin's append token records admin actions");

    // The gateway's link: every kind of span.
    for (i, kind) in ["turn", "attempt", "tool", "agent_step", "gate"].iter().enumerate() {
        ok(t.as_(GATEWAY, M::POST, "/spans", Some(span(&format!("m{i}"), "t2", kind))).await, 201, kind);
    }
    assert_eq!(t.as_(GATEWAY, M::GET, "/traces", None).await.status, 403);

    // The admin's append token: admin actions and its boot ID, nothing else.
    let action = json!({ "id": "a1", "at": now(), "category": "settings", "action": "x", "outcome": "done" });
    ok(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(action)).await, 201, "admin action");
    ok(t.as_(ADMIN_APPEND, M::POST, "/hello", Some(json!({ "boot_id": "ab" }))).await, 200, "admin hello");
    for (m, p) in [(M::POST, "/spans"), (M::GET, "/traces"), (M::GET, "/admin-actions"), (M::PUT, "/retention"), (M::GET, "/retention")] {
        assert_eq!(t.as_(ADMIN_APPEND, m.clone(), p, Some(span("z", "t", "gate"))).await.status, 403, "admin append {m} {p}");
    }

    // The admin's read token: reads and retention, no append of any kind.
    for p in ["/traces", "/stats", "/token-counts", "/admin-actions", "/prunes", "/retention", "/status", "/chain/head", "/chain/verify"] {
        ok(t.as_(ADMIN_READ, M::GET, p, None).await, 200, p);
    }
    for link in [SERVER, GATEWAY, ADMIN_APPEND] {
        for p in ["/chain/head", "/chain/verify", "/chain/entries/1"] {
            assert_eq!(t.as_(link, M::GET, p, None).await.status, 403, "{link} {p}");
        }
    }
    ok(t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": 45 }))).await, 200, "retention");
    for (m, p) in [
        (M::POST, "/spans"),
        (M::POST, "/hello"),
        (M::POST, "/admin-actions"),
        (M::POST, "/spans/g1/finish"),
        (M::POST, "/spans/g1/payloads"),
    ] {
        assert_eq!(t.as_(ADMIN_READ, m.clone(), p, Some(json!({}))).await.status, 403, "admin read {m} {p}");
    }
    // Nothing deletes or changes a record: there is no such route for any token.
    for link in [SERVER, GATEWAY, ADMIN_APPEND, ADMIN_READ] {
        for (m, p) in [
            (M::DELETE, "/spans/g1"),
            (M::PUT, "/spans/g1"),
            (M::DELETE, "/admin-actions"),
            (M::DELETE, "/traces/t1"),
            (M::PATCH, "/admin-actions"),
        ] {
            let r = t.as_(link, m.clone(), p, None).await;
            assert!(r.status == 403 || r.status == 404 || r.status == 405, "{link} {m} {p}: {}", r.status);
        }
    }
}

#[tokio::test]
async fn a_link_with_no_hash_file_accepts_nothing_and_a_stranger_is_ignored() {
    let t = start().await;
    let accept = t.dir.join("tokens/accept");
    std::fs::write(accept.join("stranger-auditor.sha256"), hex("tok_stranger")).unwrap();
    assert_eq!(t.call(Some("tok_stranger"), M::GET, "/status", None).await.status, 401, "a hash file for a link the auditor does not know");
    assert_eq!(t.as_(SERVER, M::GET, "/status", None).await.status, 200);
    std::fs::remove_file(accept.join("server-auditor.sha256")).unwrap();
    assert_eq!(t.as_(SERVER, M::GET, "/status", None).await.status, 401, "no hash file, no access");
    std::fs::write(accept.join("server-auditor.sha256"), "not a hash").unwrap();
    assert_eq!(t.as_(SERVER, M::GET, "/status", None).await.status, 401, "a damaged one accepts nothing too");
}

#[tokio::test]
async fn the_caller_is_the_one_the_token_belongs_to_never_anything_in_the_body() {
    let t = start().await;
    let mut body = span("s1", "t1", "gate");
    body["service"] = json!("admin");
    body["boot_id"] = json!("spoofed");
    assert_eq!(t.as_(SERVER, M::POST, "/spans", Some(body)).await.status, 201);
    assert_eq!(t.sql::<String>("SELECT service FROM spans WHERE id = 's1'"), "server");
    assert_ne!(t.sql::<Option<String>>("SELECT boot_id FROM spans WHERE id = 's1'").as_deref(), Some("spoofed"));

    let action = json!({ "id": "a1", "at": now(), "category": "settings", "action": "x", "outcome": "done", "service": "server", "caller": "server" });
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(action)).await.status, 201);
    let rows = t.as_(ADMIN_READ, M::GET, "/admin-actions", None).await.body;
    assert_eq!(rows[0]["service"], "admin");
}

// ------------------------------------------------------------------ boot IDs, retention

#[tokio::test]
async fn a_new_boot_id_ends_that_services_earlier_unfinished_spans_as_interrupted() {
    let t = start().await;
    let hello = |b: &str| Some(json!({ "boot_id": b }));
    assert_eq!(t.as_(SERVER, M::POST, "/hello", hello("b1")).await.body, json!({ "interrupted": 0 }));
    t.as_(SERVER, M::POST, "/spans", Some(span("g1", "t1", "gate"))).await;
    t.as_(SERVER, M::POST, "/spans", Some(span("g2", "t1", "gate"))).await;
    t.as_(SERVER, M::POST, "/spans/g2/finish", Some(finish("done"))).await;
    t.as_(GATEWAY, M::POST, "/hello", hello("gw1")).await;
    t.as_(GATEWAY, M::POST, "/spans", Some(span("m1", "t2", "turn"))).await;
    assert_eq!(t.sql::<String>("SELECT boot_id FROM spans WHERE id = 'g1'"), "b1", "a span carries its process's boot ID");

    let r = t.as_(SERVER, M::POST, "/hello", hello("b2")).await;
    assert_eq!((r.status, r.body), (200, json!({ "interrupted": 1 })), "only g1: g2 finished");
    assert_eq!(t.sql::<String>("SELECT status FROM spans WHERE id = 'g1'"), "interrupted");
    assert_eq!(t.sql::<String>("SELECT status FROM spans WHERE id = 'g2'"), "done");
    assert_eq!(t.sql::<String>("SELECT status FROM spans WHERE id = 'm1'"), "running", "another service's span is not touched");
    assert_eq!(
        t.as_(SERVER, M::POST, "/hello", hello("b2")).await.body,
        json!({ "interrupted": 0 }),
        "the same boot ID again ends nothing"
    );
}

#[tokio::test]
async fn retention_defaults_to_30_days_and_can_never_be_set_below_7() {
    let t = start().await;
    let r = t.as_(ADMIN_READ, M::GET, "/retention", None).await;
    assert_eq!(r.body, json!({ "days": 30, "floor_days": 7 }));
    for days in [0, 1, 6] {
        let r = t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": days }))).await;
        assert_eq!((r.status, r.code()), (400, "bad_request".into()), "{days}");
    }
    assert_eq!(t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": 3651 }))).await.status, 400);
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/retention", None).await.body["days"], 30, "a refused change changes nothing");
    assert_eq!(t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": 7 }))).await.status, 200);
    assert_eq!(t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": 45 }))).await.body["days"], 45);
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/retention", None).await.body["days"], 45);
    assert_eq!(t.auditor.retention_days(), 45);
}

#[tokio::test]
async fn a_prune_deletes_what_is_past_retention_and_leaves_its_record() {
    let t = start().await;
    t.as_(ADMIN_READ, M::PUT, "/retention", Some(json!({ "days": 7 }))).await;
    // Old rows, written the way an earlier version of the store would have.
    let old = now() - 20 * 86_400;
    let db = AuditDb::open(&t.dir.join("data/audit.db")).unwrap();
    db.conn()
        .execute("INSERT INTO spans (id, trace_id, kind, name, status, started_at) VALUES ('old','tOld','turn','n','done',?1)", [old])
        .unwrap();
    db.conn()
        .execute("INSERT INTO payloads (span_id, role, key_id, ciphertext, created_at) VALUES ('old','reply','k',x'01',?1)", [old])
        .unwrap();
    t.as_(GATEWAY, M::POST, "/spans", Some(span("new", "tNew", "turn"))).await;
    assert_eq!(t.auditor.prune_once(now()).await, Ok((1, 1)));
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM spans"), 1);
    let prunes: Vec<wire::PruneRecord> = serde_json::from_value(t.as_(ADMIN_READ, M::GET, "/prunes", None).await.body).unwrap();
    assert_eq!((prunes[0].spans, prunes[0].payloads), (1, 1));
}

// ------------------------------------------------------------------ limits

#[tokio::test]
async fn an_append_is_refused_at_the_backlog_limit_and_nothing_is_recorded() {
    let t = start_with(Limits { max_queued: 3, ..Limits::default() }).await;
    t.auditor.recorder.stall_writer().await; // the writer holds one record, so one is queued already
    let url = t.url.clone();
    let tasks: Vec<_> = (0..2)
        .map(|i| {
            let (http, url) = (t.http.clone(), url.clone());
            tokio::spawn(async move {
                http.post(format!("{url}/api/audit/v1/spans"))
                    .bearer_auth(token(GATEWAY))
                    .json(&span(&format!("s{i}"), "t1", "turn"))
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .as_u16()
            })
        })
        .collect();
    for _ in 0..200 {
        if t.auditor.recorder.backlog().queued >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let r = t.as_(GATEWAY, M::POST, "/spans", Some(span("refused", "t1", "turn"))).await;
    assert_eq!((r.status, r.code()), (503, "audit_backlog".into()), "{:?}", r.body);
    assert!(r.retry_after.is_some(), "the caller is told when to try again");
    // Every kind of append is refused, not only a new span.
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans/s0/finish", Some(finish("done"))).await.status, 503);
    assert_eq!(
        t.as_(
            ADMIN_APPEND,
            M::POST,
            "/admin-actions",
            Some(json!({ "id": "a", "at": 1, "category": "c", "action": "a", "outcome": "done" }))
        )
        .await
        .status,
        503
    );
    // Reads are not appends.
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/traces", None).await.status, 200);

    t.auditor.recorder.pause_writer(false);
    for task in tasks {
        assert_eq!(task.await.unwrap(), 201);
    }
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM spans WHERE id = 'refused'"), 0, "a refused append recorded nothing");
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM spans"), 2);
    assert_eq!(
        t.as_(GATEWAY, M::POST, "/spans", Some(span("later", "t1", "turn"))).await.status,
        201,
        "accepted again once it has caught up"
    );
}

#[tokio::test]
async fn an_append_is_refused_when_the_oldest_queued_record_has_waited_too_long() {
    let t = start_with(Limits { max_wait: Duration::from_millis(60), ..Limits::default() }).await;
    t.auditor.recorder.stall_writer().await;
    tokio::time::sleep(Duration::from_millis(120)).await;
    let r = t.as_(GATEWAY, M::POST, "/spans", Some(span("s", "t", "turn"))).await;
    assert_eq!((r.status, r.code()), (503, "audit_backlog".into()));
    t.auditor.recorder.pause_writer(false);
    t.auditor.recorder.flush().await;
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(span("s", "t", "turn"))).await.status, 201);
}

#[tokio::test]
async fn below_the_disk_floor_a_new_span_is_refused_but_the_records_of_open_spans_are_not() {
    let t = start().await;
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(span("open", "t1", "turn"))).await.status, 201);
    t.free.store(100 * 1024 * 1024, Ordering::Relaxed); // 100 MiB: under the 256 MiB floor
    let r = t.as_(GATEWAY, M::POST, "/spans", Some(span("new", "t2", "turn"))).await;
    assert_eq!((r.status, r.code()), (503, "audit_backlog".into()), "{:?}", r.body);
    assert!(r.retry_after.is_some());
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM spans WHERE id = 'new'"), 0);
    // Room is left for what the open span still has to write.
    let ct = base64_of(b"sealed bytes");
    assert_eq!(
        t.as_(GATEWAY, M::POST, "/spans/open/payloads", Some(json!({ "role": "reply", "key_id": "k1", "ciphertext_b64": ct })))
            .await
            .status,
        201
    );
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans/open/finish", Some(finish("done"))).await.status, 201);
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/traces", None).await.status, 200, "reads are untouched");
    t.free.store(300 * 1024 * 1024, Ordering::Relaxed);
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(span("new", "t2", "turn"))).await.status, 201, "room again");
}

fn base64_of(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ------------------------------------------------------------------ retries, validation

#[tokio::test]
async fn a_retry_of_an_append_whose_answer_was_lost_records_it_once() {
    let t = start().await;
    for _ in 0..2 {
        assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(span("s1", "t1", "turn"))).await.status, 201);
        assert_eq!(t.as_(GATEWAY, M::POST, "/spans/s1/finish", Some(finish("done"))).await.status, 201);
        let ct = json!({ "role": "reply", "key_id": "k1", "ciphertext_b64": base64_of(b"same sealed bytes") });
        assert_eq!(t.as_(GATEWAY, M::POST, "/spans/s1/payloads", Some(ct)).await.status, 201);
        let a = json!({ "id": "act1", "at": now(), "category": "settings", "action": "x", "outcome": "done" });
        assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(a)).await.status, 201);
    }
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM spans"), 1);
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM payloads"), 1);
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM admin_actions"), 1);
}

#[tokio::test]
async fn a_bad_request_is_a_400_that_does_not_echo_what_it_was_given_and_an_unknown_span_is_a_404() {
    let t = start().await;
    let secret = "sk-this-must-not-come-back";
    let mut cases: Vec<(&str, M, String, Value)> = vec![
        (
            "an id with a space",
            M::POST,
            "/spans".into(),
            json!({ "id": format!("a b {secret}"), "trace_id": "t", "kind": "gate", "name": "n", "started_at": 1, "started_ms": 1 }),
        ),
        (
            "an unknown kind",
            M::POST,
            "/spans".into(),
            json!({ "id": "s", "trace_id": "t", "kind": "admin_action", "name": "n", "started_at": 1, "started_ms": 1 }),
        ),
        (
            "attributes that are not an object",
            M::POST,
            "/spans".into(),
            json!({ "id": "s", "trace_id": "t", "kind": "gate", "name": "n", "started_at": 1, "started_ms": 1, "attrs": [secret] }),
        ),
        (
            "attributes too large",
            M::POST,
            "/spans".into(),
            json!({ "id": "s", "trace_id": "t", "kind": "gate", "name": "n", "started_at": 1, "started_ms": 1, "attrs": { "x": "y".repeat(20_000) } }),
        ),
        ("a body that is not JSON", M::POST, "/hello".into(), json!(secret)),
    ];
    let r = t.as_(GATEWAY, M::POST, "/spans", Some(span("ok", "t", "turn"))).await;
    assert_eq!(r.status, 201);
    cases.push((
        "a finish that is still running",
        M::POST,
        "/spans/ok/finish".into(),
        json!({ "status": "running", "ended_at": 1, "ended_ms": 1 }),
    ));
    cases.push((
        "a payload that is not base64",
        M::POST,
        "/spans/ok/payloads".into(),
        json!({ "role": "reply", "key_id": "k", "ciphertext_b64": format!("!!{secret}!!") }),
    ));
    cases.push(("an empty payload", M::POST, "/spans/ok/payloads".into(), json!({ "role": "reply", "key_id": "k", "ciphertext_b64": "" })));
    for (what, m, p, body) in cases {
        let r = t.as_(GATEWAY, m, &p, Some(body)).await;
        assert_eq!((r.status, r.code()), (400, "bad_request".into()), "{what}: {:?}", r.body);
        assert!(!r.body.to_string().contains(secret), "{what}: the answer echoed the input");
    }
    let a = json!({ "id": "a", "at": 1, "category": "c", "action": "x", "outcome": "maybe" });
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(a)).await.status, 400, "an outcome is done or refused");
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans/nope/finish", Some(finish("done"))).await.status, 404);
    let ct = json!({ "role": "reply", "key_id": "k", "ciphertext_b64": base64_of(b"x") });
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans/nope/payloads", Some(ct)).await.status, 404);
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/traces/nope", None).await.status, 404);
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/spans/nope/payloads", None).await.status, 404);
    assert_eq!(t.sql::<i64>("SELECT COUNT(*) FROM spans"), 1, "none of the refused ones was recorded");
}

// ------------------------------------------------------------------ what the reads look like

#[tokio::test]
async fn the_reads_are_the_wire_types_hold_no_prices_and_return_payloads_only_as_stored() {
    let t = start().await;
    t.as_(GATEWAY, M::POST, "/hello", Some(json!({ "boot_id": "gw1" }))).await;
    let mut turn = span("tu1", "tr1", "turn");
    turn["user_id"] = json!("u1");
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(turn)).await.status, 201);
    let mut attempt = span("at1", "tr1", "attempt");
    attempt["parent_id"] = json!("tu1");
    attempt["provider_id"] = json!("p1");
    attempt["model"] = json!("m1");
    attempt["attrs"] = json!({ "retry": 0 });
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans", Some(attempt)).await.status, 201);
    let mut done = finish("done");
    done["tokens_in"] = json!(100);
    done["tokens_out"] = json!(50);
    done["tokens_cached"] = json!(10);
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans/tu1/finish", Some(done)).await.status, 201);
    let sealed = vec![0xde, 0xad, 0xbe, 0xef, 7];
    let body = json!({ "role": "reply", "key_id": "0123456789abcdef", "ciphertext_b64": base64_of(&sealed) });
    assert_eq!(t.as_(GATEWAY, M::POST, "/spans/tu1/payloads", Some(body)).await.status, 201);
    let action = json!({ "id": "ac1", "at": 5, "category": "settings", "action": "kill switch on", "outcome": "done", "detail": { "kill_switch": true } });
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(action)).await.status, 201);

    let traces: Vec<wire::TraceSummary> = serde_json::from_value(t.as_(ADMIN_READ, M::GET, "/traces", None).await.body).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!((traces[0].trace_id.as_str(), traces[0].provider_id.as_deref(), traces[0].tokens_in), ("tr1", Some("p1"), Some(100)));
    let spans: Vec<wire::Span> = serde_json::from_value(t.as_(ADMIN_READ, M::GET, "/traces/tr1", None).await.body).unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans.iter().find(|s| s.id == "tu1").unwrap().sealed, 1, "the turn has one sealed payload");
    let payloads: Vec<wire::StoredPayload> =
        serde_json::from_value(t.as_(ADMIN_READ, M::GET, "/spans/tu1/payloads", None).await.body).unwrap();
    assert_eq!(
        payloads,
        vec![wire::StoredPayload { role: "reply".into(), key_id: "0123456789abcdef".into(), ciphertext_b64: base64_of(&sealed) }]
    );
    let counts: Vec<wire::TokenCount> =
        serde_json::from_value(t.as_(ADMIN_READ, M::GET, "/token-counts?since=0", None).await.body).unwrap();
    assert_eq!(
        counts,
        vec![wire::TokenCount {
            provider_id: Some("p1".into()),
            model: Some("m1".into()),
            requests: 1,
            tokens_in: 100,
            tokens_out: 50,
            tokens_cached: 10
        }]
    );
    let stats = t.as_(ADMIN_READ, M::GET, "/stats?since=0", None).await.body;
    assert_eq!(stats["requests"], 1);
    assert!(stats["cost"].is_null(), "the auditor holds no prices: {stats}");
    let actions: Vec<wire::AdminActionRecord> =
        serde_json::from_value(t.as_(ADMIN_READ, M::GET, "/admin-actions", None).await.body).unwrap();
    assert_eq!((actions[0].service.as_str(), actions[0].detail.clone()), ("admin", Some(json!({ "kill_switch": true }))));
    let status = t.as_(SERVER, M::GET, "/status", None).await.body;
    assert!(status["config_version"].is_null() && status["boot_id"].as_str().is_some_and(|b| b.len() == 26), "{status}");
}

#[tokio::test]
async fn every_route_in_the_table_is_one_the_router_answers() {
    let t = start().await;
    let default_not_found = askbar_proto::http::default_detail(askbar_proto::ErrorCode::NotFound);
    for route in askbar_auditor::routes() {
        let link = askbar_auditor::LINKS
            .iter()
            .find(|(_, _, rs)| rs.iter().any(|(m, p)| *m == route.method && route.path.ends_with(p)))
            .map(|(name, _, _)| *name);
        let path = route.path.trim_start_matches("/api/audit/v1").replace('*', "x");
        let r = t.call(link.map(token).as_deref(), M::from_bytes(route.method.as_bytes()).unwrap(), &path, Some(json!({}))).await;
        assert_ne!(r.status, 405, "{} {}", route.method, route.path);
        if r.status == 404 {
            assert_ne!(r.body["detail"], json!(default_not_found), "{} {} is not registered", route.method, route.path);
        }
    }
}

#[tokio::test]
async fn the_chain_head_moves_with_every_record_and_a_full_check_finds_an_edit() {
    let t = start().await;
    let head0 = t.as_(ADMIN_READ, M::GET, "/chain/head", None).await.body;
    assert_eq!((head0["seq"].as_i64(), head0["hash"].as_str().map(str::len)), (Some(0), Some(64)));
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/chain/entries/1", None).await.status, 404, "an empty chain has no entry");

    assert_eq!(t.as_(SERVER, M::POST, "/spans", Some(span("c1", "tc", "gate"))).await.status, 201);
    let action = json!({ "id": "ca", "at": now(), "category": "settings", "action": "x", "outcome": "done" });
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(action)).await.status, 201);
    let head = t.as_(ADMIN_READ, M::GET, "/chain/head", None).await.body;
    assert_eq!(head["seq"], 2);
    let entry = t.as_(ADMIN_READ, M::GET, "/chain/entries/2", None).await.body;
    assert_eq!(entry, head, "the head is the entry with its number");
    assert_eq!(t.as_(ADMIN_READ, M::GET, "/chain/entries/x", None).await.status, 400);

    let report = t.as_(ADMIN_READ, M::GET, "/chain/verify", None).await.body;
    assert_eq!(
        (report["ok"].as_bool(), report["entries"].as_i64(), report["problems"].as_array().map(Vec::len)),
        (Some(true), Some(2), Some(0))
    );

    // Someone edits a row in the file behind the auditor's back (the store's own triggers are the first thing to go).
    let conn = askbar_audit::rusqlite::Connection::open(t.dir.join("data/audit.db")).unwrap();
    conn.execute_batch("DROP TRIGGER admin_actions_are_never_changed; UPDATE admin_actions SET outcome = 'refused' WHERE id = 'ca';")
        .unwrap();
    let report = t.as_(ADMIN_READ, M::GET, "/chain/verify", None).await.body;
    assert_eq!(report["ok"], false);
    assert_eq!(report["problems"][0]["kind"], "admin_action");
    assert_eq!(report["problems"][0]["ref"], "ca");
    assert_eq!(report["head"], head, "the chain itself is whole: only the record was edited");
}

#[tokio::test]
async fn a_service_records_the_configuration_version_it_applied_and_only_a_service_may() {
    let t = start().await;
    let rec = |id: &str, v: i64| json!({ "id": id, "at": now(), "version": v });
    assert_eq!(t.as_(SERVER, M::POST, "/config-applied", Some(rec("cfg1", 7))).await.status, 201);
    assert_eq!(t.as_(GATEWAY, M::POST, "/config-applied", Some(rec("cfg2", 7))).await.status, 201);
    // The same record again (a retry) is one record.
    assert_eq!(t.as_(SERVER, M::POST, "/config-applied", Some(rec("cfg1", 7))).await.status, 201);
    for link in [ADMIN_APPEND, ADMIN_READ] {
        assert_eq!(t.as_(link, M::POST, "/config-applied", Some(rec("x", 1))).await.status, 403, "{link}");
    }
    assert_eq!(t.as_(SERVER, M::POST, "/config-applied", Some(json!({ "id": "bad id!", "at": 1, "version": 1 }))).await.status, 400);
    assert_eq!(t.as_(SERVER, M::POST, "/config-applied", Some(json!({ "id": "c3", "at": 1 }))).await.status, 400, "no version");

    let rows = t.as_(ADMIN_READ, M::GET, "/admin-actions", None).await.body;
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for r in rows {
        assert_eq!((r["category"].as_str(), r["action"].as_str(), r["outcome"].as_str()), (Some("config"), Some("applied"), Some("done")));
        assert_eq!(r["detail"]["version"], 7);
    }
    let mut who: Vec<&str> = rows.iter().map(|r| r["service"].as_str().unwrap()).collect();
    who.sort();
    assert_eq!(who, ["model-gateway", "server"], "the caller is the token's, never the body's");
}

#[tokio::test]
async fn an_admin_action_has_a_request_and_then_how_it_ended() {
    let t = start().await;
    for (id, outcome) in [("a1", "requested"), ("a2", "done"), ("a3", "refused"), ("a4", "failed")] {
        let body = json!({ "id": id, "at": now(), "category": "settings", "action": "x", "outcome": outcome });
        assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(body)).await.status, 201, "{outcome}");
    }
    let body = json!({ "id": "a5", "at": now(), "category": "settings", "action": "x", "outcome": "maybe" });
    assert_eq!(t.as_(ADMIN_APPEND, M::POST, "/admin-actions", Some(body)).await.status, 400);
}
