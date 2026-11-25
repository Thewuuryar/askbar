//! Shared by `approvals`, `audit`, `permissions` and `fetch`: a front on two listeners with a fake auditor, the real
//! signer (`askbar_service::approval::sign`: the front's tests do not depend on the model-gateway's crate), and a
//! registry whose handlers count their runs. (`call.rs` keeps its own copy of these helpers.)
#![allow(dead_code)]

use askbar_mcp_gateway::registry::{handler, Registry};
use askbar_mcp_gateway::{admin_links, admin_router, server_links, server_router, Gateway};
use askbar_proto::approval::{Provenance, ToolAction};
use askbar_service::approval::{generate_keypair, sign};
use askbar_service::audit_client::AuditClient;
use askbar_service::status::BootId;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub const ADMIN: &str = "admmcp_secret";
pub const SERVER: &str = "srvmcp_secret";

/// What the fake auditor was asked: path, body, and the front's tool-run count at that moment.
pub type Seen = Arc<Mutex<Vec<(String, Value, u64)>>>;

#[derive(Clone, Copy, PartialEq)]
pub enum Auditor {
    Up,
    /// `503` to a span start.
    Busy,
    /// `500` to a span start.
    Fail,
    /// Answers a span start after 3 s, past the 1 s acknowledgement wait.
    Slow,
    /// Nothing listens.
    Down,
}

pub struct Rig {
    pub seen: Seen,
    pub url: String,
    pub admin_url: String,
    pub g: Arc<Gateway>,
    pub sk: Vec<u8>,
    pub pk: Vec<u8>,
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

async fn fake_auditor(mode: Auditor, gw: Arc<OnceLock<Arc<Gateway>>>, seen: Seen) -> String {
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let (gw, seen) = (gw.clone(), seen.clone());
        async move {
            let path = req.uri().path().to_string();
            let body = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap();
            let runs = gw.get().map_or(0, |g| g.tool_runs.load(Ordering::SeqCst));
            seen.lock().unwrap().push((path.clone(), serde_json::from_slice(&body).unwrap_or(Value::Null), runs));
            let start = path.ends_with("/spans");
            if mode == Auditor::Slow && start {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            let status = match mode {
                Auditor::Busy if start => 503,
                Auditor::Fail if start => 500,
                _ => 200,
            };
            (axum::http::StatusCode::from_u16(status).unwrap(), axum::Json(json!({"interrupted": 0})))
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    if mode != Auditor::Down {
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    }
    url
}

/// A front with `registry`, not yet pushed a config.
pub async fn start_unpushed(mode: Auditor, registry: Registry) -> Rig {
    let dir = std::env::temp_dir().join(format!("askbar-mcp-test-{}", BootId::new().as_str()));
    std::fs::create_dir_all(dir.join("accept")).unwrap();
    for (name, t) in [("admin-mcp-gateway", ADMIN), ("server-mcp-gateway", SERVER)] {
        std::fs::write(dir.join("accept").join(format!("{name}.sha256")), format!("{}\n", hex(&Sha256::digest(t.as_bytes())))).unwrap();
    }
    std::fs::create_dir_all(dir.join("call")).unwrap();
    std::fs::write(dir.join("call/mcp-gateway-auditor.token"), "mcpaud_secret\n").unwrap();
    let (cell, seen): (Arc<OnceLock<Arc<Gateway>>>, Seen) = Default::default();
    let audit_url = fake_auditor(mode, cell.clone(), seen.clone()).await;
    let audit = AuditClient::new(&audit_url, dir.clone(), "mcp-gateway-auditor");
    let boot = BootId::new();
    let g = Gateway::with_registry(boot.clone(), None, Some(audit.clone()), registry);
    cell.set(g.clone()).ok();
    tokio::spawn(audit.clone().hello_until_done(boot.as_str().to_string()));
    tokio::spawn(audit.clone().drain_forever());
    while !audit.ready() && mode != Auditor::Down {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let serve = |app: axum::Router| async move {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/api/mcp-gateway/v1", l.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        url
    };
    let url = serve(server_router(g.clone(), server_links(&dir))).await;
    let admin_url = serve(admin_router(g.clone(), admin_links(&dir))).await;
    let (sk, pk) = generate_keypair().unwrap();
    Rig { seen, url, admin_url, g, sk, pk }
}

/// A front pushed the approval key and `permissions` (the push is the real `PUT /config`).
pub async fn start(mode: Auditor, registry: Registry, permissions: Value) -> Rig {
    let rig = start_unpushed(mode, registry).await;
    rig.push(permissions).await;
    rig
}

impl Rig {
    pub async fn push(&self, permissions: Value) {
        self.push_with(permissions, json!({})).await;
    }

    /// Pushes the key, `permissions` and any field of `extra` (which wins).
    pub async fn push_with(&self, permissions: Value, extra: Value) {
        let mut cfg = json!({"fetch_max_bytes": 1048576, "fetch_timeout_secs": 20, "fetch_redirects": 3, "tool_call_timeout_secs": 30,
            "one_time_id_window_secs": 120, "search_results": 8, "remembered_urls": 50,
            "approval_public_key": hex(&self.pk), "approval_key_version": 1, "permissions": permissions});
        for (k, v) in extra.as_object().unwrap() {
            cfg[k] = v.clone();
        }
        let r = reqwest::Client::new().put(format!("{}/config", self.admin_url)).bearer_auth(ADMIN).json(&cfg).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }

    /// The span records (start and finish) the auditor was sent.
    pub fn spans(&self) -> Vec<(String, Value, u64)> {
        self.seen.lock().unwrap().iter().filter(|(p, _, _)| p.contains("/spans")).cloned().collect()
    }

    pub async fn post(&self, tool: &str, body: &Value) -> (u16, Value) {
        let r = reqwest::Client::new().post(format!("{}/tools/{tool}/call", self.url)).bearer_auth(SERVER).json(body).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    /// How many calls ended in `outcome`, as `GET /status` on the admin link says.
    pub async fn outcome(&self, outcome: &str) -> u64 {
        let r = reqwest::Client::new().get(format!("{}/status", self.admin_url)).bearer_auth(ADMIN).send().await.unwrap();
        r.json::<Value>().await.unwrap()["calls"][outcome].as_u64().unwrap_or(0)
    }
}

/// `(status, error code)` of an answer.
pub fn code(r: &(u16, Value)) -> (u16, &str) {
    (r.0, r.1["error"].as_str().unwrap_or(""))
}

/// A registry whose `fetch` and `search` count their runs (the counter must stay 0 on every refusal).
pub fn counting() -> (Registry, Arc<AtomicU64>) {
    let n = Arc::new(AtomicU64::new(0));
    let h = || {
        let n = n.clone();
        handler(move |_| {
            let n = n.clone();
            async move {
                n.fetch_add(1, Ordering::SeqCst);
                Ok(json!({"ran": true}))
            }
        })
    };
    let schema = json!({"type": "object"});
    (Registry::new([("fetch", schema.clone(), h()), ("search", schema, h())]), n)
}

pub fn action(tool: &str, arg: &str) -> ToolAction {
    ToolAction {
        tool: tool.into(),
        arguments: json!({"url": arg}),
        destination: "example.com".into(),
        provenance: Provenance::User,
        query_data: false,
    }
}

/// A call body for `tool` and `arg`, approved with `sk` for `user` and `task` (the real signer).
pub fn signed(sk: &[u8], tool: &str, arg: &str, user: &str, task: &str, id: &str, expires_at: i64) -> Value {
    let a = sign(&action(tool, arg), task, user, id, expires_at, sk).unwrap();
    json!({"arguments": {"url": arg}, "destination": "example.com", "provenance": "user", "approval": a})
}
