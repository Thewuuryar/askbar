use askbar_server::{server_api, AppState};
use sha2::{Digest, Sha256};

const TOKEN: &str = "mgt_this-is-the-raw-server-token";

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
    let dir = std::env::temp_dir().join(format!("server-token-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    if write_hash {
        std::fs::write(&hash_file, format!("{}\n", hex(TOKEN))).unwrap();
    }
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, askbar_server::MIN_CLIENT);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(state, hash_file.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { url, hash_file, dir, http: reqwest::Client::new() }
}

impl T {
    async fn health(&self, auth: Option<&str>) -> u16 {
        let mut r = self.http.get(format!("{}/api/server/v1/health", self.url));
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
    let r = t.http.get(format!("{}/api/server/v1/nope", t.url)).send().await.unwrap();
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
    assert!(server_api::parse_addr("172.30.0.2:8082").is_ok());
    assert!(server_api::parse_addr("127.0.0.1:8082").is_ok());
    // `.invalid` never resolves (RFC 2606), so a name that does not exist is refused.
    for bad in ["0.0.0.0:8082", "[::]:8082", "8082", "nowhere.invalid:8082", ""] {
        assert!(server_api::parse_addr(bad).is_err(), "{bad:?}");
    }
    assert!(server_api::parse_addr("0.0.0.0:8082").unwrap_err().contains("every network"));
}

#[test]
fn a_name_must_resolve_to_exactly_one_address() {
    let a: std::net::SocketAddr = "172.18.0.4:8082".parse().unwrap();
    let b: std::net::SocketAddr = "172.19.0.4:8082".parse().unwrap();
    assert_eq!(server_api::one_addr("server-api-host:8082", vec![a]), Ok(a));
    assert_eq!(server_api::one_addr("server-api-host:8082", vec![a, a]), Ok(a), "the same address twice is one");
    let two = server_api::one_addr("server-api-host:8082", vec![a, b]).unwrap_err();
    assert!(two.contains("one network") && two.contains("172.18.0.4") && two.contains("172.19.0.4"), "{two}");
    assert!(server_api::one_addr("server-api-host:8082", vec![]).is_err());
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
        billing: Default::default(),
        price_in: None,
        price_out: None,
        price_cached_in: None,
    }
}

fn snapshot() -> Snapshot {
    Snapshot {
        version: 0,
        providers: vec![provider("p1"), provider("p2")],
        default_provider_id: Some("p1".into()),
        system_prompt: Some("Be brief.".into()),
        display: DisplaySettings::default(),
        user_display_defaults: [("u1".to_string(), DisplaySettings { width: 900, ..Default::default() })].into(),
        users: vec![askbar_proto::SnapshotUser { id: "u1".into(), name: "owner".into() }],
        pair_codes: vec![SnapshotPairCode {
            hash: "ab".repeat(32),
            user_id: "u1".into(),
            client_name: "laptop".into(),
            expires_at: 4_000_000_000,
        }],
        revoked_client_ids: vec!["d9".into()],
        budget: Budget { daily_tokens: Some(1000), monthly_tokens: None },
        kill_switch: false,
        kill_switch_aborts_running: true,
        audit_retention_days: 30,
        generation_timeout_secs: 300,
        history_turns: 0,
        audit_public_key: None,
    }
}

struct S {
    t: T,
    state: std::sync::Arc<AppState>,
    rig: askbar_server::testing::Rig,
}

async fn start_with_state() -> S {
    start_state(true).await
}

/// `with_rig = false`: a state that sends no records and reads no audit store, as one started with no auditor is.
async fn start_state(with_rig: bool) -> S {
    let dir = std::env::temp_dir().join(format!("server-token-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    std::fs::write(&hash_file, hex(TOKEN)).unwrap();
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    // Replies go through an in-process gateway and auditor (ADR 0054).
    let rig = askbar_server::testing::Rig::start().await;
    let state = AppState::new(db, askbar_server::MIN_CLIENT);
    let state = if with_rig { rig.attach(state).await } else { state };
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(state.clone(), hash_file.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    S { t: T { url, hash_file, dir, http: reqwest::Client::new() }, state, rig }
}

impl S {
    async fn put(&self, body: &serde_json::Value, token: Option<&str>) -> (u16, serde_json::Value) {
        let mut r = self.t.http.put(format!("{}/api/server/v1/config", self.t.url)).json(body);
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
    // The server records the version it now runs, with its own token (ADR 0028 rule 5).
    let rows = s.rig.wait_rows("SELECT service, category, action, outcome, detail_json FROM admin_actions").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][..4], [json!("server"), json!("config"), json!("applied"), json!("done")]);
    assert_eq!(rows[0][4], json!("{\"version\":1}"));

    // Pushing again replaces it and bumps the version; a version in the body is ignored.
    let mut next = snapshot();
    next.version = 77;
    next.kill_switch = true;
    next.providers.pop();
    let (st, body) = s.put(&serde_json::to_value(next).unwrap(), Some(TOKEN)).await;
    assert_eq!((st, &body["version"]), (200, &json!(2)));
    let stored = askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap();
    assert_eq!((stored.version, stored.kill_switch, stored.providers.len()), (2, true, 1));
    let rows: i64 = s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM config_snapshot", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1, "one snapshot, replaced in place");
}

#[tokio::test]
async fn status_reports_the_config_version_and_a_boot_id_that_changes_on_restart() {
    let s = start_with_state().await;
    let get = || async {
        s.t.http
            .get(format!("{}/api/server/v1/status", s.t.url))
            .bearer_auth(TOKEN)
            .send()
            .await
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    };
    let before = get().await;
    assert_eq!(before["config_version"], json!(null), "nothing pushed yet");
    let boot = before["boot_id"].as_str().unwrap().to_string();
    assert_eq!(boot.len(), 26, "a ULID: {boot}");
    s.put(&serde_json::to_value(snapshot()).unwrap(), Some(TOKEN)).await;
    s.put(&serde_json::to_value(snapshot()).unwrap(), Some(TOKEN)).await;
    let after = get().await;
    assert_eq!((after["config_version"].clone(), after["boot_id"].as_str()), (json!(2), Some(boot.as_str())), "same process, newer config");
    // Another process (a restart) has another boot ID.
    let other = start_with_state().await;
    assert_ne!(other.state.boot_id, s.state.boot_id);
}

#[tokio::test]
async fn restart_answers_202_with_the_boot_id_then_stops_and_needs_the_token() {
    let s = start_with_state().await;
    let url = format!("{}/api/server/v1/restart", s.t.url);
    let r = s.t.http.post(&url).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(r.status(), 500, "not set up in this process (only main sets it)");
    let (stopped, exited) =
        (std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)), std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)));
    let (st, ex) = (stopped.clone(), exited.clone());
    let _ = s.state.restarter.set(askbar_service::restart::Restarter::for_tests(
        s.state.boot_id.clone(),
        std::time::Duration::ZERO,
        std::time::Duration::from_secs(1),
        move || st.store(true, std::sync::atomic::Ordering::SeqCst),
        move || ex.store(true, std::sync::atomic::Ordering::SeqCst),
    ));
    assert_eq!(s.t.http.post(&url).send().await.unwrap().status(), 401);
    let r = s.t.http.post(&url).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(r.status(), 202);
    assert_eq!(r.json::<serde_json::Value>().await.unwrap()["boot_id"], s.state.boot_id.as_str());
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(stopped.load(std::sync::atomic::Ordering::SeqCst) && exited.load(std::sync::atomic::Ordering::SeqCst));
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
        assert!(body["detail"].as_str().unwrap().contains("at least 7"), "{body}");
        assert_eq!(body["error"], "bad_request");
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

const KEY: &str = "sk-live-VERY-SECRET-123";

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
}

// ------------------------------------------------------- clients, usage, backup

#[tokio::test]
async fn clients_and_usage_are_listed_over_the_server_api() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let user = {
        let db = s.state.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        for name in ["Work laptop", "Phone"] {
            let code = askbar_server::testing::add_pair_code_expiring(&db, &user, name, 600);
            askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        }
        db.conn().execute("UPDATE clients SET revoked_at = 5 WHERE name = 'Phone'", []).unwrap();
        db.conn()
            .execute("INSERT INTO token_usage (id, user_id, tokens_in, tokens_out, created_at) VALUES ('t1', ?1, 10, 20, 0)", [&user])
            .unwrap();
        user
    };
    {
        let space = askbar_server::testing::personal_space(&s.state, &user);
        let db = space.lock();
        let c = db.create_conversation(&user, "chat").unwrap();
        db.add_message(&user, &c, "user", "hi").unwrap();
        db.add_message(&user, &c, "assistant", "hello").unwrap();
    }

    let (st, body) = s.call(M::GET, "/api/server/v1/clients", None).await;
    assert_eq!(st, 200);
    let clients: serde_json::Value = serde_json::from_str(&body).unwrap();
    let by_name = |n: &str| clients.as_array().unwrap().iter().find(|d| d["name"] == n).unwrap().clone();
    assert_eq!((by_name("Work laptop")["revoked"].clone(), by_name("Phone")["revoked"].clone()), (json!(false), json!(true)));
    assert!(!body.contains("abt_") && !body.contains("hash"), "no token material: {body}");

    let (st, body) = s.call(M::GET, "/api/server/v1/usage", None).await;
    let u: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(st, 200);
    assert_eq!(
        (&u["conversations"], &u["messages"], &u["clients"]["active"], &u["clients"]["revoked"], &u["tokens_in"], &u["tokens_out"]),
        (&json!(1), &json!(2), &json!(1), &json!(1), &json!(10), &json!(20))
    );

    for p in ["/api/server/v1/clients", "/api/server/v1/usage", "/api/server/v1/status"] {
        let r = s.t.http.get(format!("{}{p}", s.t.url)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p} needs the token");
    }
    let r = s.t.http.post(format!("{}/api/server/v1/exports", s.t.url)).send().await.unwrap();
    assert_eq!(r.status(), 401, "exports need the token");
    // The old paths are gone (ADR 0022).
    for (m, p) in [("GET", "/api/server/v1/backup"), ("PUT", "/api/server/v1/snapshot"), ("POST", "/api/server/v1/models/list")] {
        let r = s.t.http.request(m.parse().unwrap(), format!("{}{p}", s.t.url)).bearer_auth(TOKEN).send().await.unwrap();
        assert_eq!(r.status(), 404, "{m} {p}");
    }
}

#[tokio::test]
async fn the_backup_has_the_data_and_none_of_the_secrets() {
    let s = start_with_state().await;
    let llm = "http://x/v1";
    s.push_provider(llm).await;
    let code_hash = "ab".repeat(32); // the snapshot lists this pair-code hash
    let space_id = {
        let user: String = s.state.db.lock().unwrap().conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let space = askbar_server::testing::personal_space(&s.state, &user);
        let c = space.lock().create_conversation(&user, "kept conversation").unwrap();
        space.lock().add_message(&user, &c, "user", "question one").unwrap();
        let db = s.state.db.lock().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "pending");
        db.conn().execute("INSERT INTO redeemed_codes (hash, client_id, redeemed_at) VALUES ('used-hash-123', NULL, 0)", []).unwrap();
        let _ = code;
        // A paired client asks for a code to pair another app of its user (step 1.9).
        db.conn()
            .execute("INSERT INTO clients (id, user_id, name, hash, created_at) VALUES ('C1', ?1, 'laptop', 'token-hash', 0)", [&user])
            .unwrap();
        let auth = askbar_server::auth::ClientAuth { user_id: user.clone(), client_id: "C1".into() };
        askbar_server::auth::new_client_pair_code(&db, &auth, "phone").unwrap().expect("a code");
        space.id.clone()
    };
    let client_code_hash: String =
        s.state.db.lock().unwrap().conn().query_row("SELECT hash FROM client_pair_codes", [], |r| r.get(0)).unwrap();

    let r = s.t.http.post(format!("{}/api/server/v1/exports", s.t.url)).bearer_auth(TOKEN).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/x-tar");
    assert!(r.headers()["content-disposition"].to_str().unwrap().contains("askbar-server-backup-"));
    let bytes = r.bytes().await.unwrap();
    // The control database and one file per space (ADR 0009 mechanism 4).
    let files = askbar_service::tar::read(&bytes).expect("a tar");
    let mut expected = vec!["control.db".to_string()];
    expected.extend(s.state.db.lock().unwrap().space_ids().unwrap().iter().map(|id| format!("spaces/{id}.db")));
    assert_eq!(files.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(), expected);
    for (name, data) in &files {
        assert_eq!(&data[..15], b"SQLite format 3", "{name}");
    }
    let space_index = files.iter().position(|(n, _)| n == &format!("spaces/{space_id}.db")).expect("the user's space is in it");
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!has(KEY.as_bytes()), "no provider key");
    assert!(!has(code_hash.as_bytes()), "the snapshot's pair-code hashes are gone");
    assert!(!has(b"used-hash-123"), "so are redeemed-code hashes");
    assert!(!has(client_code_hash.as_bytes()), "and the hashes of codes a client asked for");

    let dir = std::env::temp_dir().join(format!("server-token-backup-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("b.db");
    std::fs::write(&file, &files[0].1).unwrap();
    let c = rusqlite::Connection::open(&file).unwrap();
    let one = |sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(one("SELECT COUNT(*) FROM providers WHERE api_key_enc IS NOT NULL"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM redeemed_codes"), 0);
    assert_eq!(one("SELECT COUNT(*) FROM client_pair_codes"), 0);
    std::fs::write(dir.join("s.db"), &files[space_index].1).unwrap();
    let space_db = rusqlite::Connection::open(dir.join("s.db")).unwrap();
    let kept: i64 = space_db.query_row("SELECT COUNT(*) FROM conversations WHERE title = 'kept conversation'", [], |r| r.get(0)).unwrap();
    assert_eq!(kept, 1, "the space's history is in its own file");
    let doc: String = c.query_row("SELECT body FROM config_snapshot", [], |r| r.get(0)).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&doc).unwrap();
    assert_eq!(doc["pair_codes"], json!([]));
    assert_eq!(doc["providers"].as_array().unwrap().len(), 2, "the rest of the snapshot stays");
    let _ = std::fs::remove_dir_all(&dir);
    // The live database is untouched by taking a backup.
    assert_eq!(
        askbar_server::snapshot::current(&s.state.db.lock().unwrap()).unwrap().unwrap().pair_codes.len(),
        2,
        "both listed codes are still there"
    );
    let live: i64 = s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM client_pair_codes", [], |r| r.get(0)).unwrap();
    assert_eq!(live, 1, "and so is the client's code");
}

#[tokio::test]
async fn users_are_listed_and_the_server_holds_no_provider_key_nor_a_route_to_set_or_use_one() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let (st, body) = s.call(M::GET, "/api/server/v1/users", None).await;
    let users: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, users.as_array().unwrap().len(), users[0]["name"].as_str()), (200, 1, Some("owner")));
    assert!(users[0]["id"].as_str().unwrap().len() == 26);

    // Provider keys live in the gateway, which the admin gives them (ADR 0054): the server has no route that takes a
    // key, tries a provider or lists its models, and its provider health says nothing about keys.
    s.push_provider("http://x/v1").await;
    for (m, p) in
        [(M::PUT, "/api/server/v1/providers/p1/key"), (M::POST, "/api/server/v1/providers/p1/test"), (M::POST, "/api/server/v1/models")]
    {
        let (st, body) = s.call(m.clone(), p, Some(json!({ "key": KEY }))).await;
        assert_eq!(st, 404, "{m} {p}");
        assert!(!body.contains(KEY));
    }
    let (st, body) = s.call(M::GET, "/api/server/v1/providers/health", None).await;
    assert_eq!(st, 200);
    assert!(serde_json::from_str::<serde_json::Value>(&body).unwrap().get("missing_key").is_none(), "{body}");
}

#[tokio::test]
async fn taking_a_backup_leaves_no_temporary_file_behind() {
    let s = start_with_state().await;
    let leftovers = || -> usize {
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("askbar-backup-"))
            .count()
    };
    let before = leftovers();
    for _ in 0..3 {
        let r = s.t.http.post(format!("{}/api/server/v1/exports", s.t.url)).bearer_auth(TOKEN).send().await.unwrap();
        assert_eq!(r.status(), 200);
        r.bytes().await.unwrap();
    }
    // The file is removed when the download ends; give the stream a moment to drop.
    for _ in 0..40 {
        if leftovers() <= before {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(leftovers() <= before, "backup files were left in the temp directory");
}

#[tokio::test]
async fn a_generation_with_the_mocks_usage_adds_to_the_overview_totals() {
    use reqwest::Method as M;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
    let mut child =
        Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap(); // "mock LLM on ..."

    let s = start_with_state().await;
    let (user, client) = {
        let db = s.state.db.lock().unwrap();
        askbar_server::testing::add_provider(
            &db,
            askbar_server::testing::provider("p1", "mock", &format!("http://127.0.0.1:{port}/v1"), "m"),
        );
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let client: String = db.conn().query_row("SELECT id FROM clients", [], |r| r.get(0)).unwrap();
        (user, client)
    };
    s.rig.set_key("p1", "sk-test");
    s.rig.sync(&s.state); // the gateway gets the provider and its key
    let sub = askbar_server::generation::submit(
        &s.state,
        askbar_server::generation::Submit {
            space: askbar_server::testing::personal_space(&s.state, &user),
            user_id: user,
            client_id: client,
            conversation_id: None,
            provider_id: "p1".into(),
            prompt: "hello".into(),
            client_request_id: None,
        },
    )
    .expect("the generation starts");
    // Wait for it to end and its usage to be recorded.
    for _ in 0..200 {
        if s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM token_usage", [], |r| r.get::<_, i64>(0)).unwrap() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let _ = sub;

    let (st, body) = s.call(M::GET, "/api/server/v1/usage", None).await;
    let u: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(st, 200);
    assert_eq!((&u["tokens_in"], &u["tokens_out"]), (&json!(100), &json!(50)), "the totals are the mock's usage chunk");
    let _ = child.kill();
    let _ = child.wait();
}

#[tokio::test]
async fn server_api_failures_are_problems_with_a_code_and_a_sentence() {
    let t = start(true).await;
    let auth = format!("Bearer {TOKEN}");
    let url = |p: &str| format!("{}{p}", t.url);
    let check = |r: reqwest::Response, status: u16, code: &'static str| async move {
        assert_eq!(r.status().as_u16(), status, "{code}");
        assert_eq!(r.headers()["content-type"], "application/problem+json", "{code}");
        let p: askbar_proto::Problem = r.json().await.unwrap();
        assert_eq!((p.error.as_str(), p.status), (code, status));
        assert!(!p.detail.is_empty(), "{code}: a sentence for a person");
    };
    // No token and a wrong token.
    let r = t.http.get(url("/api/server/v1/health")).send().await.unwrap();
    assert_eq!(r.headers()["www-authenticate"], "Bearer");
    check(r, 401, "unauthorized").await;
    check(t.http.get(url("/api/server/v1/health")).header("authorization", "Bearer wrong").send().await.unwrap(), 401, "unauthorized")
        .await;
    // Unknown route and wrong method (after the token check).
    check(t.http.get(url("/api/server/v1/nope")).header("authorization", &auth).send().await.unwrap(), 404, "not_found").await;
    check(t.http.delete(url("/api/server/v1/health")).header("authorization", &auth).send().await.unwrap(), 405, "method_not_allowed")
        .await;
    // A body that is not JSON.
    check(
        t.http
            .put(url("/api/server/v1/config"))
            .header("authorization", &auth)
            .header("content-type", "application/json")
            .body("nope")
            .send()
            .await
            .unwrap(),
        400,
        "bad_request",
    )
    .await;
    // A snapshot the server refuses says why, in detail.
    let r = t
        .http
        .put(url("/api/server/v1/config"))
        .header("authorization", &auth)
        .json(&serde_json::json!({ "audit_retention_days": 3 }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let p: askbar_proto::Problem = r.json().await.unwrap();
    assert_eq!(p.error, "bad_request");
    assert!(p.detail.contains("at least 7"), "{}", p.detail);
    // A route that is not there, such as the old way to set a provider key (the server holds none, ADR 0054).
    check(
        t.http
            .put(url("/api/server/v1/providers/nope/key"))
            .header("authorization", &auth)
            .json(&serde_json::json!({ "key": "k" }))
            .send()
            .await
            .unwrap(),
        404,
        "not_found",
    )
    .await;
}
