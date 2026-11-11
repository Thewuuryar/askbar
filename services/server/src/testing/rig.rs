//! An in-process auditor and model-gateway for tests (feature `test-rig`): a state attached to a rig makes its replies
//! the way the stack does, through a real gateway that records to a real auditor.
//!
//! The tokens and hashes are files in a temporary directory, as `bootstrap` makes them in the stack. A test attaches its
//! state with [`Rig::attach`], gives the gateway what the state's snapshot and stored keys describe with
//! [`Rig::sync`] (after every change to providers or keys), and reads what was recorded with [`Rig::rows`].

use crate::AppState;
use askbar_audit::{AuditDb, Recorder};
use askbar_auditor::{disk::FreeSpace, Auditor, Limits};
use askbar_proto::gateway::{GatewayConfig, GatewayProvider};
use askbar_proto::CatalogEntry;
use askbar_service::audit_client::AuditClient;
use askbar_service::restart::Restarter;
use askbar_service::status::BootId;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

pub struct Rig {
    pub dir: PathBuf,
    pub auditor: Arc<Auditor>,
    pub audit_url: String,
    pub gateway: Arc<askbar_model_gateway::Gateway>,
    pub gateway_url: String,
    /// What the auditor thinks its disk has free; set low to make it refuse new spans.
    pub free: Arc<AtomicU64>,
    server_tokens: PathBuf,
    /// The admin's token directory: its `admin-auditor-read` token, for tests of the admin's Audit pages.
    pub admin_tokens: PathBuf,
    /// The key to give the gateway for each provider (the server holds none; ADR 0054).
    keys: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Rig {
    pub async fn start() -> Rig {
        let dir = std::env::temp_dir().join(format!("askbar-rig-{}", crate::ids::new_id()));
        let (audit_tokens, gw_tokens, server_tokens) = (dir.join("audit-token"), dir.join("gateway-token"), dir.join("server-token"));
        let admin_tokens = dir.join("admin-token");
        for d in [&audit_tokens, &gw_tokens, &server_tokens, &admin_tokens] {
            std::fs::create_dir_all(d.join("accept")).unwrap();
            std::fs::create_dir_all(d.join("call")).unwrap();
        }
        // One token per link; its hash goes to the receiver, the token to the caller.
        let link = |name: &str, hash_dir: &PathBuf, token_dir: &PathBuf| {
            let token = format!("tok_{name}");
            std::fs::write(hash_dir.join("accept").join(format!("{name}.sha256")), format!("{}\n", hex(&token))).unwrap();
            std::fs::write(token_dir.join("call").join(format!("{name}.token")), format!("{token}\n")).unwrap();
        };
        link("server-auditor", &audit_tokens, &server_tokens);
        link("model-gateway-auditor", &audit_tokens, &gw_tokens);
        link("model-gateway-auditor-read", &audit_tokens, &gw_tokens);
        link("server-model-gateway", &gw_tokens, &server_tokens);
        link("admin-auditor-read", &audit_tokens, &admin_tokens);
        link("admin-auditor-append", &audit_tokens, &admin_tokens);

        let limits = Limits::default();
        let recorder = Recorder::spawn(AuditDb::open(&dir.join("audit/audit.db")).unwrap(), limits.max_queued);
        let free = Arc::new(AtomicU64::new(100 * 1024 * 1024 * 1024));
        let f = free.clone();
        let free_space: FreeSpace = Arc::new(move || Some(f.load(Ordering::Relaxed)));
        let auditor = Auditor::new(recorder, limits, free_space);
        let audit_url = serve(askbar_auditor::router(auditor.clone(), askbar_auditor::links(&audit_tokens))).await;

        let client = AuditClient::new(&audit_url, gw_tokens.clone(), "model-gateway-auditor");
        let boot = BootId::new();
        let restarter = Restarter::for_tests(boot.clone(), Duration::ZERO, Duration::from_secs(1), || {}, || {});
        let gateway = askbar_model_gateway::Gateway::new(client.clone(), boot.clone(), restarter);
        gateway.set_usage_reader(askbar_model_gateway::usage::Reader::new(&audit_url, gw_tokens.clone()));
        tokio::spawn(gateway.clone().watch_usage());
        tokio::spawn(client.clone().hello_until_done(boot.as_str().to_string()));
        tokio::spawn(client.clone().drain_forever());
        let gateway_url = serve(askbar_model_gateway::server_router(gateway.clone(), askbar_model_gateway::server_links(&gw_tokens))).await;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.ready() {
            assert!(Instant::now() < deadline, "the gateway never said hello");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Rig { dir, auditor, audit_url, gateway, gateway_url, free, server_tokens, admin_tokens, keys: Default::default() }
    }

    /// A state like the old `state_with_audit`: an in-memory database with the provider "p1" at `base_url` (its key
    /// stored), a user, attached to a rig and synced. The rig is leaked, so what it recorded stays readable for the whole
    /// test; the directory returned holds the auditor's `audit.db`, and the rig is there to `sync` after the test changes providers or keys.
    pub async fn state_with_provider(base_url: &str) -> (Arc<AppState>, PathBuf, &'static Rig) {
        let rig = Rig::start().await;
        let db = crate::Db::open_in_memory().unwrap();
        db.seed_server().unwrap();
        super::add_provider(&db, super::provider("p1", "mock", base_url, "m"));
        rig.set_key("p1", "sk-test");
        let state = rig.attach(AppState::new(db, crate::MIN_CLIENT)).await;
        rig.sync(&state);
        let dir = rig.dir.join("audit");
        let rig: &'static Rig = Box::leak(Box::new(rig));
        (state, dir, rig)
    }

    /// `state` with the auditor and the gateway attached: its records and replies go through the rig.
    pub async fn attach(&self, state: Arc<AppState>) -> Arc<AppState> {
        let client = AuditClient::new(&self.audit_url, self.server_tokens.clone(), "server-auditor");
        let boot = state.boot_id.clone();
        tokio::spawn(client.clone().hello_until_done(boot.as_str().to_string()));
        tokio::spawn(client.clone().drain_forever());
        // The auditor must have this process's boot ID before a reply starts, or the server refuses it (as it does for
        // the first moments after it starts in the stack).
        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.ready() {
            assert!(Instant::now() < deadline, "the server never said hello to the auditor");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // `flush` waits for the gateway's records too: a reply's spans come from both.
        let recorder = crate::audit::Recorder::new(client).also_flush(self.gateway.audit().clone());
        state.with_audit(recorder).with_gateway(crate::gateway::GatewayClient::new(&self.gateway_url, self.server_tokens.clone()))
    }

    /// Remember `key` as the one to give the gateway for provider `id`, at the next `sync`.
    pub fn set_key(&self, id: &str, key: &str) {
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), key.to_string());
    }

    /// Give the gateway what `state`'s snapshot describes: its providers, each with the key `set_key` gave it, and its audit
    /// key. A state with no audit key gives the gateway nothing (it runs nothing without one).
    pub fn sync(&self, state: &AppState) {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let Some(snap) = crate::snapshot::current(&db).ok().flatten() else { return };
        let keys = self.keys.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let providers = snap
            .providers
            .iter()
            .map(|p| GatewayProvider {
                id: p.id.clone(),
                name: p.name.clone(),
                kind: p.kind.clone(),
                base_url: p.base_url.clone(),
                model: p.model.clone(),
                max_tokens: p.max_tokens,
                temperature: p.temperature,
                headers: p.headers.clone(),
                api_key: keys.get(&p.id).cloned().unwrap_or_default(),
                billing: p.billing,
            })
            .collect();
        let mut ordered: Vec<_> = snap.providers.iter().collect();
        ordered.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(pos) = snap.default_provider_id.as_ref().and_then(|id| ordered.iter().position(|p| &p.id == id)) {
            let first = ordered.remove(pos);
            ordered.insert(0, first);
        }
        let catalog: Vec<CatalogEntry> =
            ordered.iter().map(|p| CatalogEntry { provider_id: p.id.clone(), model: p.model.clone() }).collect();
        let default_model = catalog.first().map(CatalogEntry::id);
        let config = GatewayConfig {
            version: snap.version,
            providers,
            audit_public_key: snap.audit_public_key,
            generation_timeout_secs: snap.generation_timeout_secs,
            budget: snap.budget,
            kill_switch: snap.kill_switch,
            kill_switch_aborts_running: snap.kill_switch_aborts_running,
            paused_users: snap.paused_user_ids,
            // Every provider's own model is approved, and the snapshot's default provider's is the default (else the first by name).
            catalog,
            default_model,
        };
        self.gateway.apply_config(&config).expect("the snapshot makes a usable gateway configuration");
    }

    /// A span as only the gateway may open it (a `turn`, say), with an optional sealed payload, and finished if `done`: for
    /// a test that needs a trail to read. Returns once the auditor has everything. The id is the span's.
    pub async fn seed(
        &self,
        trace: &str,
        kind: crate::audit::Kind,
        name: &str,
        payload: Option<(&str, crate::seal::Sealed)>,
        done: bool,
    ) -> String {
        use askbar_proto::audit as wire;
        let client = self.gateway.audit();
        let (at, ms) = askbar_service::audit_client::now_parts();
        let id = crate::ids::new_id();
        let start = wire::SpanStart {
            id: id.clone(),
            trace_id: trace.to_string(),
            parent_id: None,
            kind,
            name: name.to_string(),
            started_at: at,
            started_ms: ms,
            provider_id: None,
            model: None,
            user_id: None,
            client_id: None,
            attrs: None,
        };
        client.open(&start).await.expect("the auditor takes the span");
        if let Some((role, sealed)) = payload {
            client.payload(&id, role, &sealed.key_id, &sealed.bytes);
        }
        if done {
            client.finish(
                &id,
                wire::SpanFinish {
                    status: "done".into(),
                    ended_at: at,
                    ended_ms: ms,
                    provider_id: None,
                    model: None,
                    tokens_in: None,
                    tokens_out: None,
                    tokens_cached: None,
                    ttft_ms: None,
                    error_code: None,
                    attrs: None,
                },
            );
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while client.queued() > 0 {
            assert!(Instant::now() < deadline, "the auditor never took the records");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        id
    }

    /// Rows of the auditor's store, as JSON values.
    pub fn rows(&self, sql: &str) -> Vec<Vec<Value>> {
        let conn = self.auditor.recorder.reader().unwrap();
        let mut stmt = conn.prepare(sql).unwrap();
        let n = stmt.column_count();
        stmt.query_map([], |r| {
            Ok((0..n)
                .map(|i| match r.get_ref(i).unwrap() {
                    askbar_audit::rusqlite::types::ValueRef::Null => Value::Null,
                    askbar_audit::rusqlite::types::ValueRef::Integer(v) => json!(v),
                    askbar_audit::rusqlite::types::ValueRef::Real(v) => json!(v),
                    askbar_audit::rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    askbar_audit::rusqlite::types::ValueRef::Blob(b) => json!(format!("blob:{}", b.len())),
                })
                .collect())
        })
        .unwrap()
        .map(Result::unwrap)
        .collect()
    }

    /// Wait (up to 10 seconds) until `sql` returns a row: what a service records arrives through its queue.
    pub async fn wait_rows(&self, sql: &str) -> Vec<Vec<Value>> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let r = self.rows(sql);
            if !r.is_empty() {
                return r;
            }
            assert!(Instant::now() < deadline, "no rows for: {sql}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}
