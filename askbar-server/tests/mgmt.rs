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

// --------------------------------------------------------------- provider keys

const KEY: &str = "sk-live-VERY-SECRET-123";

/// An OpenAI-style endpoint that answers only to `KEY`, and echoes a rejected key back in its error.
async fn fake_llm() -> String {
    use axum::{http::HeaderMap, response::IntoResponse, routing::post};
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|headers: HeaderMap| async move {
            let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            if auth == format!("Bearer {KEY}") {
                ([("content-type", "text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n").into_response()
            } else {
                (axum::http::StatusCode::UNAUTHORIZED, format!("{{\"error\":{{\"message\":\"Incorrect API key provided: {}\"}}}}", auth.trim_start_matches("Bearer "))).into_response()
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

impl S {
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<serde_json::Value>) -> (u16, String) {
        let mut r = self.t.http.request(method, format!("{}{path}", self.t.url)).bearer_auth(TOKEN);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        (resp.status().as_u16(), resp.text().await.unwrap())
    }
    async fn push_provider(&self, base_url: &str) {
        let mut snap = snapshot();
        snap.providers[0].base_url = base_url.into();
        assert_eq!(self.put(&serde_json::to_value(snap).unwrap(), Some(TOKEN)).await.0, 200);
    }
    fn stored(&self, id: &str) -> Option<Vec<u8>> {
        self.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers WHERE id = ?1", [id], |r| r.get(0)).ok()
    }
}

#[tokio::test]
async fn a_key_is_stored_encrypted_and_never_returned() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    s.push_provider("http://x/v1").await;

    let (st, body) = s.call(M::PUT, "/mgmt/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
    assert_eq!((st, body.as_str()), (204, ""), "nothing comes back");
    let stored = s.stored("p1").expect("a row was made from the snapshot");
    assert!(!stored.windows(KEY.len()).any(|w| w == KEY.as_bytes()), "not stored in the clear");
    assert_eq!(s.state.secrets().decrypt(&stored).unwrap(), KEY, "encrypted with the master key");

    // Replacing it replaces; the metadata the row got from the snapshot is intact.
    let (st, _) = s.call(M::PUT, "/mgmt/v1/providers/p1/key", Some(json!({ "key": "sk-second-key-456" }))).await;
    assert_eq!(st, 204);
    assert_eq!(s.state.secrets().decrypt(&s.stored("p1").unwrap()).unwrap(), "sk-second-key-456");
    let name: String = s.state.db.lock().unwrap().conn().query_row("SELECT name FROM providers WHERE id = 'p1'", [], |r| r.get(0)).unwrap();
    assert_eq!(name, "name-p1");

    // Errors do not echo the key either.
    for (path, body, want) in [
        ("/mgmt/v1/providers/unknown/key", json!({ "key": KEY }), 404),
        ("/mgmt/v1/providers/p2/key", json!({ "key": "   " }), 400),
        ("/mgmt/v1/providers/p2/key", json!({ "key": "x".repeat(5000) }), 400),
    ] {
        let (st, text) = s.call(M::PUT, path, Some(body)).await;
        assert_eq!(st, want, "{path}");
        assert!(!text.contains(KEY));
    }
    assert!(s.stored("unknown").is_none());
    // There is no way to read it back.
    for (m, p) in [(M::GET, "/mgmt/v1/providers/p1/key"), (M::GET, "/mgmt/v1/providers/p1"), (M::GET, "/mgmt/v1/providers")] {
        let (st, text) = s.call(m, p, None).await;
        assert!(st == 404 || st == 405, "{p}: {st}");
        assert!(!text.contains("sk-"));
    }
    // Both endpoints need the token.
    let r = s.t.http.put(format!("{}/mgmt/v1/providers/p1/key", s.t.url)).json(&json!({ "key": KEY })).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn the_test_endpoint_runs_a_provider_and_hides_the_key() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let llm = fake_llm().await;
    s.push_provider(&llm).await;

    // No key yet: the provider is known (it is in the snapshot) but rejects the empty credential.
    let (st, body) = s.call(M::POST, "/mgmt/v1/providers/p1/test", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, &v["ok"]), (200, &json!(false)), "{body}");

    s.call(M::PUT, "/mgmt/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
    let (st, body) = s.call(M::POST, "/mgmt/v1/providers/p1/test", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, &v["ok"], v["message"].as_str()), (200, &json!(true), Some("ok")));
    assert!(!body.contains(KEY));

    // A wrong key: the provider's error echoes it, and the response must not.
    let wrong = "sk-the-wrong-key-999";
    s.call(M::PUT, "/mgmt/v1/providers/p1/key", Some(json!({ "key": wrong }))).await;
    let (st, body) = s.call(M::POST, "/mgmt/v1/providers/p1/test", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, &v["ok"]), (200, &json!(false)));
    assert!(!body.contains(wrong) && body.contains("[key hidden]"), "{body}");

    assert_eq!(s.call(M::POST, "/mgmt/v1/providers/nope/test", None).await.0, 404);
}

// ------------------------------------------------------- devices, usage, backup

#[tokio::test]
async fn devices_and_usage_are_listed_over_the_management_api() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let user = {
        let db = s.state.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        for name in ["Work laptop", "Phone"] {
            let code = askbar_server::auth::create_pair_code(&db, &user, name, 600).unwrap();
            askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        }
        let c = db.create_conversation(&user, "chat").unwrap();
        db.add_message(&user, &c, "user", "hi").unwrap();
        db.add_message(&user, &c, "assistant", "hello").unwrap();
        db.conn().execute("UPDATE devices SET revoked_at = 5 WHERE name = 'Phone'", []).unwrap();
        db.conn().execute("INSERT INTO token_usage (id, user_id, tokens_in, tokens_out, created_at) VALUES ('t1', ?1, 10, 20, 0)", [&user]).unwrap();
        user
    };
    let _ = user;

    let (st, body) = s.call(M::GET, "/mgmt/v1/devices", None).await;
    assert_eq!(st, 200);
    let devices: serde_json::Value = serde_json::from_str(&body).unwrap();
    let by_name = |n: &str| devices.as_array().unwrap().iter().find(|d| d["name"] == n).unwrap().clone();
    assert_eq!((by_name("Work laptop")["revoked"].clone(), by_name("Phone")["revoked"].clone()), (json!(false), json!(true)));
    assert!(!body.contains("abt_") && !body.contains("hash"), "no token material: {body}");

    let (st, body) = s.call(M::GET, "/mgmt/v1/usage", None).await;
    let u: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(st, 200);
    assert_eq!(
        (&u["conversations"], &u["messages"], &u["devices"]["active"], &u["devices"]["revoked"], &u["tokens_in"], &u["tokens_out"]),
        (&json!(1), &json!(2), &json!(1), &json!(1), &json!(10), &json!(20))
    );

    for p in ["/mgmt/v1/devices", "/mgmt/v1/usage", "/mgmt/v1/backup"] {
        let r = s.t.http.get(format!("{}{p}", s.t.url)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p} needs the token");
    }
}

#[tokio::test]
async fn the_backup_has_the_data_and_none_of_the_secrets() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let llm = "http://x/v1";
    s.push_provider(llm).await;
    s.call(M::PUT, "/mgmt/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
    let code_hash = "ab".repeat(32); // the snapshot lists this pair-code hash
    {
        let db = s.state.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let c = db.create_conversation(&user, "kept conversation").unwrap();
        db.add_message(&user, &c, "user", "question one").unwrap();
        let code = askbar_server::auth::create_pair_code(&db, &user, "pending", 600).unwrap();
        db.conn().execute("INSERT INTO redeemed_codes (hash, device_id, redeemed_at) VALUES ('used-hash-123', NULL, 0)", []).unwrap();
        let _ = code;
    }

    let r = s.t.http.get(format!("{}/mgmt/v1/backup", s.t.url)).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-disposition"].to_str().unwrap().contains("askbar-backup-"));
    let bytes = r.bytes().await.unwrap();
    assert_eq!(&bytes[..15], b"SQLite format 3");
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!has(KEY.as_bytes()), "no provider key");
    assert!(!has(code_hash.as_bytes()), "the snapshot's pair-code hashes are gone");
    assert!(!has(b"used-hash-123"), "so are redeemed-code hashes");

    let dir = std::env::temp_dir().join(format!("askbar-mgmt-backup-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("b.db");
    std::fs::write(&file, &bytes).unwrap();
    let c = rusqlite::Connection::open(&file).unwrap();
    let one = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(one("SELECT COUNT(*) FROM providers WHERE api_key_enc IS NOT NULL"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM pair_codes"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM redeemed_codes"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM conversations WHERE title = 'kept conversation'"), 1);
    let doc: String = c.query_row("SELECT body FROM config_snapshot", [], |r| r.get(0)).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&doc).unwrap();
    assert_eq!(doc["pair_codes"], json!([]));
    assert_eq!(doc["providers"].as_array().unwrap().len(), 2, "the rest of the snapshot stays");
    let _ = std::fs::remove_dir_all(&dir);
    // The live database is untouched by taking a backup.
    assert!(askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap().pair_codes.len() == 1);
}
