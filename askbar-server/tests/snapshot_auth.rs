//! Pairing and revocation follow the snapshot: codes are checked by hash and recorded as
//! redeemed; devices the snapshot lists as revoked are refused.

use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode};
use askbar_server::{mgmt, secrets::Secrets, AppState};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const MGMT_TOKEN: &str = "mgt_test-token";
const CODE: &str = "pairing-code-one";

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

struct T {
    api: String,
    mgmt: String,
    user: String,
    state: Arc<AppState>,
    http: reqwest::Client,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

async fn start() -> T {
    let dir = std::env::temp_dir().join(format!("askbar-snapauth-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("token.sha256"), hex(MGMT_TOKEN)).unwrap();
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    T {
        api: serve(askbar_server::api_router(state.clone())).await,
        mgmt: serve(mgmt::router(state.clone(), dir.join("token.sha256"))).await,
        user,
        state,
        http: reqwest::Client::new(),
        dir,
    }
}

impl T {
    fn snapshot(&self, codes: &[(&str, i64)], revoked: &[&str]) -> Snapshot {
        Snapshot {
            version: 0,
            providers: vec![],
            default_provider_id: None,
            fallback_order: vec![],
            system_prompt: None,
            display: DisplaySettings::default(),
            user_display_defaults: Default::default(),
            pair_codes: codes
                .iter()
                .map(|(c, exp)| SnapshotPairCode { hash: hex(c), user_id: self.user.clone(), device_name: "laptop".into(), expires_at: *exp })
                .collect(),
            revoked_device_ids: revoked.iter().map(|s| s.to_string()).collect(),
            budget: Budget::default(),
            kill_switch: false,
            audit_retention_days: 30,
        }
    }
    async fn push(&self, s: &Snapshot) {
        let r = self.http.put(format!("{}/mgmt/v1/snapshot", self.mgmt)).bearer_auth(MGMT_TOKEN).json(s).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }
    async fn pair(&self, code: &str) -> Option<String> {
        let r = self
            .http
            .post(format!("{}/api/v1/pair", self.api))
            .header("x-askbar-client-version", "0.3.0")
            .json(&json!({ "code": code }))
            .send()
            .await
            .unwrap();
        match r.status().as_u16() {
            200 => Some(r.json::<serde_json::Value>().await.unwrap()["token"].as_str().unwrap().to_string()),
            401 => None,
            other => panic!("unexpected {other}"),
        }
    }
    async fn whoami(&self, token: &str) -> u16 {
        self.http.get(format!("{}/api/v1/whoami", self.api)).bearer_auth(token).header("x-askbar-client-version", "0.3.0").send().await.unwrap().status().as_u16()
    }
    fn device_id(&self) -> String {
        self.state.db.lock().unwrap().conn().query_row("SELECT id FROM devices LIMIT 1", [], |r| r.get(0)).unwrap()
    }
}

#[tokio::test]
async fn a_code_in_the_snapshot_pairs_once_and_a_reused_code_fails() {
    let t = start().await;
    let soon = askbar_server::auth::now() + 600;
    t.push(&t.snapshot(&[(CODE, soon)], &[])).await;

    let token = t.pair(CODE).await.expect("the first redemption works");
    assert_eq!(t.whoami(&token).await, 204);
    assert!(t.pair(CODE).await.is_none(), "the same code a second time fails");
    let (n, name): (i64, String) =
        t.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*), MAX(name) FROM devices", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((n, name.as_str()), (1, "laptop"), "one device, named from the snapshot's entry");

    // Redeemed codes are recorded by hash, and stay refused even if the admin pushes the code again.
    let recorded: String = t.state.db.lock().unwrap().conn().query_row("SELECT hash FROM redeemed_codes", [], |r| r.get(0)).unwrap();
    assert_eq!(recorded, hex(CODE));
    t.push(&t.snapshot(&[(CODE, soon)], &[])).await;
    assert!(t.pair(CODE).await.is_none());
}

#[tokio::test]
async fn unknown_expired_and_racing_codes() {
    let t = start().await;
    assert!(t.pair(CODE).await.is_none(), "no snapshot yet, so nothing can pair");
    let now = askbar_server::auth::now();
    t.push(&t.snapshot(&[(CODE, now + 600), ("expired-code", now - 1)], &[])).await;
    assert!(t.pair("not-in-the-snapshot").await.is_none());
    assert!(t.pair("expired-code").await.is_none());

    let results = futures_util::future::join_all((0..6).map(|_| t.pair(CODE))).await;
    assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1, "exactly one of six racing redemptions wins");
}

#[tokio::test]
async fn a_device_the_snapshot_lists_as_revoked_is_refused() {
    let t = start().await;
    let soon = askbar_server::auth::now() + 600;
    t.push(&t.snapshot(&[(CODE, soon)], &[])).await;
    let token = t.pair(CODE).await.unwrap();
    assert_eq!(t.whoami(&token).await, 204);
    let id = t.device_id();

    // Revoked only through the snapshot: the devices row itself is untouched.
    t.push(&t.snapshot(&[], &[&id])).await;
    assert_eq!(t.whoami(&token).await, 401, "refused");
    let revoked_at: Option<i64> = t.state.db.lock().unwrap().conn().query_row("SELECT revoked_at FROM devices", [], |r| r.get(0)).unwrap();
    assert_eq!(revoked_at, None);

    // Other devices are not affected, and the snapshot decides: take the id out and it works again.
    t.push(&t.snapshot(&[], &["some-other-device"])).await;
    assert_eq!(t.whoami(&token).await, 204);
}
