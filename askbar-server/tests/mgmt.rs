use askbar_server::{mgmt, secrets::Secrets, AppState};
use sha2::{Digest, Sha256};

const TOKEN: &str = "mgt_this-is-the-raw-management-token";

fn hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

struct T {
    url: String,
    hash_file: std::path::PathBuf,
    dir: std::path::PathBuf,
    http: reqwest::Client,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn start(write_hash: bool) -> T {
    let dir = std::env::temp_dir().join(format!("askbar-mgmt-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    if write_hash {
        std::fs::write(&hash_file, format!("{}\n", hex(TOKEN))).unwrap();
    }
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = mgmt::router(state, hash_file.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, hash_file, dir, http: reqwest::Client::new() }
}

impl T {
    async fn health(&self, auth: Option<&str>) -> u16 {
        let mut r = self.http.get(format!("{}/mgmt/v1/health", self.url));
        if let Some(a) = auth {
            r = r.header("authorization", a);
        }
        r.send().await.unwrap().status().as_u16()
    }
}

#[tokio::test]
async fn the_right_token_gets_through() {
    let t = start(true).await;
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 200);
    assert_eq!(t.health(Some(&format!("bearer {TOKEN}"))).await, 200, "the scheme is case-insensitive");
}

#[tokio::test]
async fn no_token_a_wrong_token_or_the_stored_hash_is_401() {
    let t = start(true).await;
    assert_eq!(t.health(None).await, 401);
    assert_eq!(t.health(Some("Bearer ")).await, 401);
    assert_eq!(t.health(Some(&format!("Basic {TOKEN}"))).await, 401);
    assert_eq!(t.health(Some("Bearer mgt_wrong")).await, 401);
    assert_eq!(t.health(Some(&format!("Bearer {}", hex(TOKEN)))).await, 401, "the hash the server holds is not a credential");
    // Unknown routes do not get past the check either.
    let r = t.http.get(format!("{}/mgmt/v1/nope", t.url)).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn it_fails_closed_when_the_hash_file_is_missing_or_unusable() {
    let t = start(false).await;
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 401, "no hash file");

    // Appears later: read on each request, so it works without a restart.
    std::fs::write(&t.hash_file, hex(TOKEN)).unwrap();
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 200);

    // Replaced (rotated): the old token stops working at once.
    std::fs::write(&t.hash_file, hex("mgt_a-newer-token")).unwrap();
    assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 401);
    assert_eq!(t.health(Some("Bearer mgt_a-newer-token")).await, 200);

    // Garbage or empty content never matches, whatever is sent.
    for junk in ["", "not a hash", "abc"] {
        std::fs::write(&t.hash_file, junk).unwrap();
        assert_eq!(t.health(Some(&format!("Bearer {junk}"))).await, 401, "{junk:?}");
        assert_eq!(t.health(Some(&format!("Bearer {TOKEN}"))).await, 401, "{junk:?}");
    }

    // And gone again.
    std::fs::remove_file(&t.hash_file).unwrap();
    assert_eq!(t.health(Some("Bearer mgt_a-newer-token")).await, 401);
}

#[test]
fn the_listen_address_is_never_a_wildcard() {
    assert!(mgmt::parse_addr("172.30.0.2:8082").is_ok());
    assert!(mgmt::parse_addr("127.0.0.1:8082").is_ok());
    for bad in ["0.0.0.0:8082", "[::]:8082", "8082", "mgmt:8082", ""] {
        assert!(mgmt::parse_addr(bad).is_err(), "{bad:?}");
    }
    assert!(mgmt::parse_addr("0.0.0.0:8082").unwrap_err().contains("every network"));
}

// ---------------------------------------------------------------- snapshot

use askbar_proto::{Budget, DisplaySettings, Snapshot, SnapshotPairCode, SnapshotProvider};
use serde_json::json;

fn provider(id: &str) -> SnapshotProvider {
    SnapshotProvider {
        id: id.into(),
        name: format!("name-{id}"),
        kind: "openai".into(),
        base_url: "http://x/v1".into(),
        model: "m".into(),
        max_tokens: Some(100),
        temperature: None,
        headers: Default::default(),
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        version: 0,
        providers: vec![provider("p1"), provider("p2")],
        default_provider_id: Some("p1".into()),
        fallback_order: vec!["p2".into()],
        system_prompt: Some("Be brief.".into()),
        display: DisplaySettings::default(),
        user_display_defaults: [("u1".to_string(), DisplaySettings { width: 900, ..Default::default() })].into(),
        pair_codes: vec![SnapshotPairCode { hash: "ab".repeat(32), user_id: "u1".into(), device_name: "laptop".into(), expires_at: 4_000_000_000 }],
        revoked_device_ids: vec!["d9".into()],
        budget: Budget { daily_tokens: Some(1000), monthly_tokens: None },
        kill_switch: false,
        audit_retention_days: 30,
    }
}

struct S {
    t: T,
    state: std::sync::Arc<AppState>,
}

async fn start_with_state() -> S {
    let dir = std::env::temp_dir().join(format!("askbar-mgmt-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    std::fs::write(&hash_file, hex(TOKEN)).unwrap();
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT, "http://localhost:8080");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = mgmt::router(state.clone(), hash_file.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    S { t: T { url, hash_file, dir, http: reqwest::Client::new() }, state }
}

impl S {
    async fn put(&self, body: &serde_json::Value, token: Option<&str>) -> (u16, serde_json::Value) {
        let mut r = self.t.http.put(format!("{}/mgmt/v1/snapshot", self.t.url)).json(body);
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        let resp = r.send().await.unwrap();
        (resp.status().as_u16(), resp.json().await.unwrap_or(json!(null)))
    }
}

#[tokio::test]
async fn a_pushed_snapshot_is_stored_and_the_version_goes_up() {
    let s = start_with_state().await;
    assert!(askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().is_none(), "nothing before the first push");

    let (st, body) = s.put(&serde_json::to_value(snapshot()).unwrap(), Some(TOKEN)).await;
    assert_eq!((st, &body["version"]), (200, &json!(1)));
    let stored = askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap();
    assert_eq!(stored, Snapshot { version: 1, ..snapshot() }, "stored as sent, with the server's version");

    // Pushing again replaces it and bumps the version; a version in the body is ignored.
    let mut next = snapshot();
    next.version = 77;
    next.kill_switch = true;
    next.providers.pop();
    next.fallback_order.clear();
    let (st, body) = s.put(&serde_json::to_value(next).unwrap(), Some(TOKEN)).await;
    assert_eq!((st, &body["version"]), (200, &json!(2)));
    let stored = askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap();
    assert_eq!((stored.version, stored.kill_switch, stored.providers.len()), (2, true, 1));
    let rows: i64 = s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM config_snapshot", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1, "one snapshot, replaced in place");
}

#[tokio::test]
async fn retention_below_seven_days_is_rejected_and_changes_nothing() {
    let s = start_with_state().await;
    s.put(&serde_json::to_value(snapshot()).unwrap(), Some(TOKEN)).await;
    for days in [0, 1, 6] {
        let mut bad = snapshot();
        bad.audit_retention_days = days;
        bad.kill_switch = true;
        let (st, body) = s.put(&serde_json::to_value(bad).unwrap(), Some(TOKEN)).await;
        assert_eq!(st, 400, "{days} days");
        assert!(body["error"].as_str().unwrap().contains("at least 7"), "{body}");
    }
    let (st, _) = s.put(&serde_json::to_value(Snapshot { audit_retention_days: 7, ..snapshot() }).unwrap(), Some(TOKEN)).await;
    assert_eq!(st, 200, "7 is the minimum and is accepted");
    let stored = askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap();
    assert_eq!((stored.version, stored.kill_switch), (2, false), "the rejected pushes left no trace");
}

#[tokio::test]
async fn other_invalid_snapshots_are_rejected_and_the_endpoint_needs_the_token() {
    let s = start_with_state().await;
    let mut dangling = snapshot();
    dangling.default_provider_id = Some("nope".into());
    assert_eq!(s.put(&serde_json::to_value(dangling).unwrap(), Some(TOKEN)).await.0, 400);
    let mut dupes = snapshot();
    dupes.providers.push(provider("p1"));
    assert_eq!(s.put(&serde_json::to_value(dupes).unwrap(), Some(TOKEN)).await.0, 400);
    let mut kind = snapshot();
    kind.providers[0].kind = "gopher".into();
    assert_eq!(s.put(&serde_json::to_value(kind).unwrap(), Some(TOKEN)).await.0, 400);
    let mut disp = snapshot();
    disp.display.width = 5;
    assert_eq!(s.put(&serde_json::to_value(disp).unwrap(), Some(TOKEN)).await.0, 400);

    assert_eq!(s.put(&serde_json::to_value(snapshot()).unwrap(), None).await.0, 401);
    assert_eq!(s.put(&serde_json::to_value(snapshot()).unwrap(), Some("wrong")).await.0, 401);
    assert!(askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().is_none(), "nothing was stored");
}
