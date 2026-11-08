//! Pairing and revocation follow the snapshot: codes are checked by hash and recorded as
//! redeemed; clients the snapshot lists as revoked are refused.

use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode};
use askbar_server::{server_api, secrets::Secrets, AppState};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const SERVER_TOKEN: &str = "mgt_test-token";
const CODE: &str = "pairing-code-one";

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

struct T {
    api: String,
    server_api: String,
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
    std::fs::write(dir.join("token.sha256"), hex(SERVER_TOKEN)).unwrap();
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    T {
        api: serve(askbar_server::client_router(state.clone())).await,
        server_api: serve(server_api::router(state.clone(), dir.join("token.sha256"))).await,
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
            system_prompt: None,
            display: DisplaySettings::default(),
            user_display_defaults: Default::default(),
            users: vec![],
            pair_codes: codes
                .iter()
                .map(|(c, exp)| SnapshotPairCode { hash: hex(c), user_id: self.user.clone(), client_name: "laptop".into(), expires_at: *exp })
                .collect(),
            revoked_client_ids: revoked.iter().map(|s| s.to_string()).collect(),
            budget: Budget::default(),
            kill_switch: false,
            kill_switch_aborts_running: true,
            audit_retention_days: 30,
            generation_timeout_secs: 300,
            audit_public_key: None,
        }
    }
    async fn push(&self, s: &Snapshot) {
        let r = self.http.put(format!("{}/api/server/v1/config", self.server_api)).bearer_auth(SERVER_TOKEN).json(s).send().await.unwrap();
        assert_eq!(r.status(), 200);
    }
    async fn pair(&self, code: &str) -> Option<String> {
        let r = self
            .http
            .post(format!("{}/api/client/v1/pairings", self.api))
            .header("x-askbar-client-version", "0.5.0")
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
        self.http.get(format!("{}/api/client/v1/me", self.api)).bearer_auth(token).header("x-askbar-client-version", "0.5.0").send().await.unwrap().status().as_u16()
    }
    fn client_id(&self) -> String {
        self.state.db.lock().unwrap().conn().query_row("SELECT id FROM clients LIMIT 1", [], |r| r.get(0)).unwrap()
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
        t.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*), MAX(name) FROM clients", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((n, name.as_str()), (1, "laptop"), "one client, named from the snapshot's entry");

    // Redeemed codes are recorded by hash, and stay refused even if the admin pushes the code again.
    let recorded: String = t.state.db.lock().unwrap().conn().query_row("SELECT hash FROM redeemed_codes", [], |r| r.get(0)).unwrap();
    assert_eq!(recorded, hex(CODE));
    t.push(&t.snapshot(&[(CODE, soon)], &[])).await;
    assert!(t.pair(CODE).await.is_none());
}

#[tokio::test]
async fn unknown_expired_and_racing_codes() {
    let t = start().await;
    // No snapshot yet, so nothing can pair: the server is not configured (ADR 0028 rule 3).
    let r = t.http.post(format!("{}/api/client/v1/pairings", t.api)).header("x-askbar-client-version", "0.5.0").json(&json!({ "code": CODE })).send().await.unwrap();
    assert_eq!(r.status(), 503);
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "not_configured");
    let now = askbar_server::auth::now();
    t.push(&t.snapshot(&[(CODE, now + 600), ("expired-code", now - 1)], &[])).await;
    assert!(t.pair("not-in-the-snapshot").await.is_none());
    assert!(t.pair("expired-code").await.is_none());

    let results = futures_util::future::join_all((0..6).map(|_| t.pair(CODE))).await;
    assert_eq!(results.iter().filter(|r| r.is_some()).count(), 1, "exactly one of six racing redemptions wins");
}

#[tokio::test]
async fn a_client_the_snapshot_lists_as_revoked_is_refused() {
    let t = start().await;
    let soon = askbar_server::auth::now() + 600;
    t.push(&t.snapshot(&[(CODE, soon)], &[])).await;
    let token = t.pair(CODE).await.unwrap();
    assert_eq!(t.whoami(&token).await, 204);
    let id = t.client_id();

    // Revoked only through the snapshot: the clients row itself is untouched.
    t.push(&t.snapshot(&[], &[&id])).await;
    assert_eq!(t.whoami(&token).await, 401, "refused");
    let revoked_at: Option<i64> = t.state.db.lock().unwrap().conn().query_row("SELECT revoked_at FROM clients", [], |r| r.get(0)).unwrap();
    assert_eq!(revoked_at, None);

    // Other clients are not affected, and the snapshot decides: take the id out and it works again.
    t.push(&t.snapshot(&[], &["some-other-client"])).await;
    assert_eq!(t.whoami(&token).await, 204);
}

impl T {
    async fn new_key(&self, token: &str, name: &str) -> reqwest::Response {
        self.http
            .post(format!("{}/api/client/v1/me/pairing-keys", self.api))
            .bearer_auth(token)
            .header("x-askbar-client-version", "0.5.0")
            .json(&json!({ "name": name }))
            .send()
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn users_arrive_with_the_config_and_are_renamed_by_it() {
    let t = start().await;
    let mut s = t.snapshot(&[], &[]);
    s.users = vec![askbar_proto::SnapshotUser { id: "U-ALICE".into(), name: "Alice".into() }];
    t.push(&s).await;
    s.users[0].name = "Alice B.".into();
    t.push(&s).await;
    let name: String = t.state.db.lock().unwrap().conn().query_row("SELECT name FROM users WHERE id = 'U-ALICE'", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "Alice B.");
    // A user listed twice, or without a name, is refused and nothing is stored.
    for bad in [
        vec![askbar_proto::SnapshotUser { id: "x".into(), name: "a".into() }, askbar_proto::SnapshotUser { id: "x".into(), name: "b".into() }],
        vec![askbar_proto::SnapshotUser { id: "y".into(), name: " ".into() }],
    ] {
        let mut b = t.snapshot(&[], &[]);
        b.users = bad;
        let r = t.http.put(format!("{}/api/server/v1/config", t.server_api)).bearer_auth(SERVER_TOKEN).json(&b).send().await.unwrap();
        assert_eq!(r.status(), 400);
    }
}

#[tokio::test]
async fn a_code_for_a_user_the_server_does_not_know_pairs_nothing() {
    let t = start().await;
    let mut s = t.snapshot(&[], &[]);
    s.pair_codes = vec![SnapshotPairCode { hash: hex(CODE), user_id: "nobody".into(), client_name: "x".into(), expires_at: askbar_server::auth::now() + 600 }];
    t.push(&s).await;
    assert!(t.pair(CODE).await.is_none());
}

#[tokio::test]
async fn a_paired_client_gets_a_code_that_pairs_another_client_of_the_same_user_once() {
    let t = start().await;
    t.push(&t.snapshot(&[(CODE, askbar_server::auth::now() + 600)], &[])).await;
    let first = t.pair(CODE).await.unwrap();
    let r = t.new_key(&first, "Work laptop").await;
    assert_eq!(r.status(), 201);
    let body: askbar_proto::PairingKeyResponse = r.json().await.unwrap();
    assert_eq!(body.expires_in, 600);
    let second = t.pair(&body.code).await.expect("the client's code pairs a new client");
    assert_eq!(t.whoami(&second).await, 204);
    assert!(t.pair(&body.code).await.is_none(), "once only");
    let rows: Vec<(String, String)> = {
        let db = t.state.db.lock().unwrap();
        let mut st = db.conn().prepare("SELECT user_id, name FROM clients ORDER BY created_at, name").unwrap();
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        v
    };
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(u, _)| *u == t.user), "both belong to the same user: {rows:?}");
    assert!(rows.iter().any(|(_, n)| n == "Work laptop"));
    // Only the hash is stored.
    let stored: String = t.state.db.lock().unwrap().conn().query_row("SELECT hash FROM client_pair_codes", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, hex(&body.code));
}

#[tokio::test]
async fn at_most_five_unused_client_codes_per_user_and_a_token_is_needed() {
    let t = start().await;
    t.push(&t.snapshot(&[(CODE, askbar_server::auth::now() + 600)], &[])).await;
    let token = t.pair(CODE).await.unwrap();
    for _ in 0..5 {
        assert_eq!(t.new_key(&token, "").await.status(), 201);
    }
    let r = t.new_key(&token, "").await;
    assert_eq!(r.status(), 429);
    assert!(r.headers().contains_key("retry-after"));
    assert_eq!(r.json::<askbar_proto::Problem>().await.unwrap().error, "too_many_attempts");
    assert_eq!(t.new_key("abt_not-a-token", "").await.status(), 401);
    assert_eq!(t.new_key(&token, &"x".repeat(61)).await.status(), 400);
}
