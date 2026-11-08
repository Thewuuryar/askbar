use askbar_server::{secrets::Secrets, server_api, AppState};
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
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
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
        audit_public_key: None,
    }
}

struct S {
    t: T,
    state: std::sync::Arc<AppState>,
}

async fn start_with_state() -> S {
    let dir = std::env::temp_dir().join(format!("server-token-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&dir).unwrap();
    let hash_file = dir.join("token.sha256");
    std::fs::write(&hash_file, hex(TOKEN)).unwrap();
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(state.clone(), hash_file.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    S { t: T { url, hash_file, dir, http: reqwest::Client::new() }, state }
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
                ([("content-type", "text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n")
                    .into_response()
            } else {
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    format!("{{\"error\":{{\"message\":\"Incorrect API key provided: {}\"}}}}", auth.trim_start_matches("Bearer ")),
                )
                    .into_response()
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

    let (st, body) = s.call(M::PUT, "/api/server/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
    assert_eq!((st, body.as_str()), (204, ""), "nothing comes back");
    let stored = s.stored("p1").expect("a row was made from the snapshot");
    assert!(!stored.windows(KEY.len()).any(|w| w == KEY.as_bytes()), "not stored in the clear");
    assert_eq!(s.state.secrets().decrypt(&stored).unwrap(), KEY, "encrypted with the master key");

    // Replacing it replaces. The row holds the id and the key and nothing else.
    let (st, _) = s.call(M::PUT, "/api/server/v1/providers/p1/key", Some(json!({ "key": "sk-second-key-456" }))).await;
    assert_eq!(st, 204);
    assert_eq!(s.state.secrets().decrypt(&s.stored("p1").unwrap()).unwrap(), "sk-second-key-456");
    let rows: i64 =
        s.state.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM providers WHERE id = 'p1'", [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1);

    // Errors do not echo the key either.
    for (path, body, want) in [
        ("/api/server/v1/providers/unknown/key", json!({ "key": KEY }), 404),
        ("/api/server/v1/providers/p2/key", json!({ "key": "   " }), 400),
        ("/api/server/v1/providers/p2/key", json!({ "key": "x".repeat(5000) }), 400),
    ] {
        let (st, text) = s.call(M::PUT, path, Some(body)).await;
        assert_eq!(st, want, "{path}");
        assert!(!text.contains(KEY));
    }
    assert!(s.stored("unknown").is_none());
    // There is no way to read it back.
    for (m, p) in
        [(M::GET, "/api/server/v1/providers/p1/key"), (M::GET, "/api/server/v1/providers/p1"), (M::GET, "/api/server/v1/providers")]
    {
        let (st, text) = s.call(m, p, None).await;
        assert!(st == 404 || st == 405, "{p}: {st}");
        assert!(!text.contains("sk-"));
    }
    // Both endpoints need the token.
    let r = s.t.http.put(format!("{}/api/server/v1/providers/p1/key", s.t.url)).json(&json!({ "key": KEY })).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn the_test_endpoint_runs_a_provider_and_hides_the_key() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let llm = fake_llm().await;
    s.push_provider(&llm).await;

    // No key yet: the provider is known (it is in the snapshot) but rejects the empty credential.
    let (st, body) = s.call(M::POST, "/api/server/v1/providers/p1/test", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, &v["ok"]), (200, &json!(false)), "{body}");

    s.call(M::PUT, "/api/server/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
    let (st, body) = s.call(M::POST, "/api/server/v1/providers/p1/test", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, &v["ok"], v["message"].as_str()), (200, &json!(true), Some("ok")));
    assert!(!body.contains(KEY));

    // A wrong key: the provider's error echoes it, and the response must not.
    let wrong = "sk-the-wrong-key-999";
    s.call(M::PUT, "/api/server/v1/providers/p1/key", Some(json!({ "key": wrong }))).await;
    let (st, body) = s.call(M::POST, "/api/server/v1/providers/p1/test", None).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, &v["ok"]), (200, &json!(false)));
    assert!(!body.contains(wrong) && body.contains("[key hidden]"), "{body}");

    assert_eq!(s.call(M::POST, "/api/server/v1/providers/nope/test", None).await.0, 404);
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
    use reqwest::Method as M;
    let s = start_with_state().await;
    let llm = "http://x/v1";
    s.push_provider(llm).await;
    s.call(M::PUT, "/api/server/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
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
async fn users_are_listed_and_a_provider_dropped_from_the_snapshot_loses_its_key() {
    use reqwest::Method as M;
    let s = start_with_state().await;
    let (st, body) = s.call(M::GET, "/api/server/v1/users", None).await;
    let users: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((st, users.as_array().unwrap().len(), users[0]["name"].as_str()), (200, 1, Some("owner")));
    assert!(users[0]["id"].as_str().unwrap().len() == 26);

    s.push_provider("http://x/v1").await;
    s.call(M::PUT, "/api/server/v1/providers/p1/key", Some(json!({ "key": KEY }))).await;
    s.call(M::PUT, "/api/server/v1/providers/p2/key", Some(json!({ "key": "sk-other-key-777" }))).await;
    assert!(s.stored("p1").is_some() && s.stored("p2").is_some());

    // The admin drops p2: its stored key goes with it, p1's stays.
    let mut next = snapshot();
    next.providers.retain(|p| p.id == "p1");
    assert_eq!(s.put(&serde_json::to_value(next).unwrap(), Some(TOKEN)).await.0, 200);
    assert!(s.stored("p2").is_none(), "no key left behind for a deleted provider");
    assert!(s.stored("p1").is_some());
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
        askbar_server::testing::store_key(&db, &s.state.secrets(), "p1", "sk-test");
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let client: String = db.conn().query_row("SELECT id FROM clients", [], |r| r.get(0)).unwrap();
        (user, client)
    };
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
    .ok()
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

// ------------------------------------------------------------------ audit endpoints

struct AuditStack {
    url: String,
    http: reqwest::Client,
    state: std::sync::Arc<AppState>,
    _mock: std::process::Child,
    _dir: std::path::PathBuf,
}

impl Drop for AuditStack {
    fn drop(&mut self) {
        let _ = self._mock.kill();
        let _ = self._mock.wait();
    }
}

async fn audit_stack() -> AuditStack {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
    let mut child =
        Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    std::thread::spawn(move || for _ in out.lines() {});

    let (state, dir) = askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{port}/v1"));
    let hash_file = dir.join("token.sha256");
    std::fs::write(&hash_file, hex(TOKEN)).unwrap();
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let app = server_api::router(state.clone(), hash_file);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    AuditStack { url, http: reqwest::Client::new(), state, _mock: child, _dir: dir }
}

impl AuditStack {
    async fn chat(&self, prompt: &str) -> String {
        use futures_util::StreamExt;
        let (user, client) = {
            let db = self.state.db.lock().unwrap();
            let user = db.seed_server().unwrap();
            (user, "dev-1".to_string())
        };
        let sub = askbar_server::generation::submit(
            &self.state,
            askbar_server::generation::Submit {
                space: askbar_server::testing::personal_space(&self.state, &user),
                user_id: user,
                client_id: client,
                conversation_id: None,
                provider_id: "p1".into(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
        .ok()
        .expect("accepted");
        let _: Vec<_> = sub.live.subscribe(0).collect().await;
        sub.generation_id
    }

    async fn get(&self, path: &str) -> (u16, String) {
        self.state.audit.flush().await;
        let r = self.http.get(format!("{}{path}", self.url)).bearer_auth(TOKEN).send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    }
}

/// No message content: no field with such a name anywhere in a response, and none of the words
/// that were typed or answered.
fn assert_metadata_only(body: &str) {
    for forbidden in ["content", "\"text\"", "prompt", "reply", "ciphertext", "message\":", "SECRET-WORD", "Anything else?"] {
        assert!(!body.contains(forbidden), "{forbidden:?} in {body}");
    }
}

#[tokio::test]
async fn audit_traces_are_listed_filtered_and_paged_with_metadata_only() {
    let a = audit_stack().await;
    let ok1 = a.chat("SECRET-WORD one").await;
    let bad = a.chat("please fail").await;
    let ok2 = a.chat("SECRET-WORD two").await;

    let (st, body) = a.get("/api/server/v1/audit/traces").await;
    assert_eq!(st, 200);
    assert_metadata_only(&body);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let ids: Vec<&str> = v["traces"].as_array().unwrap().iter().map(|t| t["trace_id"].as_str().unwrap()).collect();
    assert_eq!(ids, [ok2.as_str(), bad.as_str(), ok1.as_str()], "newest first");
    let t = &v["traces"][0];
    assert_eq!((t["status"].as_str(), t["provider_id"].as_str(), t["model"].as_str()), (Some("done"), Some("p1"), Some("m")));
    assert_eq!((t["tokens_in"].as_i64(), t["tokens_out"].as_i64()), (Some(100), Some(50)));
    assert!(t["ttft_ms"].is_i64() && t["latency_ms"].is_i64());
    assert_eq!(v["traces"][1]["error_code"], "http_401");

    // Filters and the cursor.
    let (_, body) = a.get("/api/server/v1/audit/traces?status=error").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["traces"].as_array().unwrap().len(), 1);
    assert_eq!(v["traces"][0]["trace_id"], bad.as_str());
    let (_, body) = a.get("/api/server/v1/audit/traces?provider=p1&limit=2").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["traces"].as_array().unwrap().len(), 2);
    let (_, body) = a.get(&format!("/api/server/v1/audit/traces?before={bad}")).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["traces"].as_array().unwrap().len(), 1, "only older than the cursor");
    assert_eq!(v["traces"][0]["trace_id"], ok1.as_str());
    let (_, body) = a.get("/api/server/v1/audit/traces?provider=nope").await;
    assert_eq!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["traces"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn one_trace_returns_all_its_spans_and_unknown_ones_are_404() {
    let a = audit_stack().await;
    let id = a.chat("SECRET-WORD hello").await;
    let (st, body) = a.get(&format!("/api/server/v1/audit/traces/{id}")).await;
    assert_eq!(st, 200);
    assert_metadata_only(&body);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let kinds: Vec<&str> = v["spans"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["gate", "turn", "attempt"], "the kill switch gate it passed, then the turn and its attempt");
    assert_eq!(v["spans"][0]["name"], "kill_switch");
    assert_eq!(v["spans"][2]["parent_id"], v["spans"][1]["id"]);
    assert_eq!(v["spans"][2]["attrs"]["http_status"], 200);
    // How many sealed items each span has: the attempt its request, the turn its reply, a gate none.
    let sealed: Vec<i64> = v["spans"].as_array().unwrap().iter().map(|s| s["sealed"].as_i64().unwrap()).collect();
    assert_eq!(sealed, [0, 1, 1]);
    assert_eq!(a.get("/api/server/v1/audit/traces/no-such-trace").await.0, 404);
}

#[tokio::test]
async fn audit_stats_count_rate_percentiles_and_tokens() {
    let a = audit_stack().await;
    for _ in 0..3 {
        a.chat("SECRET-WORD hi").await;
    }
    a.chat("please fail").await;
    let (st, body) = a.get("/api/server/v1/audit/stats").await;
    assert_eq!(st, 200);
    assert_metadata_only(&body);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["requests"], 4);
    assert_eq!((v["by_status"]["done"].as_i64(), v["by_status"]["error"].as_i64()), (Some(3), Some(1)));
    assert!((v["error_rate"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    assert_eq!((v["tokens"]["in"].as_i64(), v["tokens"]["out"].as_i64()), (Some(300), Some(150)));
    let (p50, p95) = (v["latency_ms"]["p50"].as_i64().unwrap(), v["latency_ms"]["p95"].as_i64().unwrap());
    assert!(p50 > 0 && p95 >= p50, "{p50} {p95}");
    assert!(v["ttft_ms"]["p50"].is_i64());

    // A window that starts in the future holds nothing.
    let (_, body) = a.get(&format!("/api/server/v1/audit/stats?since={}", askbar_server::auth::now() + 3600)).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!((v["requests"].as_i64(), v["error_rate"].as_f64()), (Some(0), Some(0.0)));
    assert!(v["latency_ms"]["p50"].is_null());
}

#[tokio::test]
async fn the_audit_endpoints_need_the_server_token_and_a_recording_store() {
    let a = audit_stack().await;
    for p in ["/api/server/v1/audit/traces", "/api/server/v1/audit/traces/x", "/api/server/v1/audit/stats"] {
        let r = a.http.get(format!("{}{p}", a.url)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{p}");
    }
    // A server with no audit store says so instead of pretending the trail is empty.
    let s = start_with_state().await;
    let (st, _) = s.call(reqwest::Method::GET, "/api/server/v1/audit/stats", None).await;
    assert_eq!(st, 503);
}

#[tokio::test]
async fn sealed_payloads_are_served_as_ciphertext_only_and_unknown_spans_are_404() {
    use base64::Engine;
    let a = audit_stack().await;
    let id = a.chat("SECRET-WORD tell me").await;
    let (_, body) = a.get(&format!("/api/server/v1/audit/traces/{id}")).await;
    let spans: serde_json::Value = serde_json::from_str(&body).unwrap();
    let span_of =
        |kind: &str| spans["spans"].as_array().unwrap().iter().find(|s| s["kind"] == kind).unwrap()["id"].as_str().unwrap().to_string();

    let (st, body) = a.get(&format!("/api/server/v1/audit/payloads/{}", span_of("attempt"))).await;
    assert_eq!(st, 200);
    assert!(!body.contains("SECRET-WORD") && !body.contains("Anything else?"), "no plaintext: {body}");
    let list: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["role"], "request");
    assert_eq!(list[0]["key_id"], askbar_server::testing::test_audit_key().key_id.as_str());
    let ct = base64::engine::general_purpose::STANDARD.decode(list[0]["ciphertext_b64"].as_str().unwrap()).unwrap();
    // The caller can open it with the key; the server's endpoint never did.
    let opened = crypto_box::SecretKey::from(askbar_server::testing::TEST_AUDIT_SECRET).unseal(&ct).unwrap();
    assert!(String::from_utf8(opened).unwrap().contains("SECRET-WORD tell me"));
    let keys: Vec<&str> = list[0].as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys.len(), 3, "role, key_id, ciphertext_b64 and nothing else: {keys:?}");

    let (_, body) = a.get(&format!("/api/server/v1/audit/payloads/{}", span_of("turn"))).await;
    let turn: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(turn[0]["role"], "reply");
    assert!(!body.contains("Anything else?"));

    // A span that exists but has nothing sealed is an empty list; one that does not exist is 404.
    let (st, body) = a.get("/api/server/v1/audit/payloads/01NOSUCHSPAN").await;
    assert_eq!(st, 404, "{body}");
    let r = a.http.get(format!("{}/api/server/v1/audit/payloads/{}", a.url, span_of("turn"))).send().await.unwrap();
    assert_eq!(r.status(), 401, "needs the token");
}

fn set_prices(a: &AuditStack, price_in: Option<f64>, price_out: Option<f64>, price_cached: Option<f64>) {
    askbar_server::testing::update_snapshot(&a.state.db.lock().unwrap(), |snap| {
        let p = snap.providers.iter_mut().find(|p| p.id == "p1").unwrap();
        (p.price_in, p.price_out, p.price_cached_in) = (price_in, price_out, price_cached);
    });
}

fn approx(v: &serde_json::Value, want: f64) -> bool {
    v.as_f64().is_some_and(|got| (got - want).abs() < 1e-9)
}

#[tokio::test]
async fn cost_appears_in_traces_and_one_trace_computed_from_the_current_prices() {
    let a = audit_stack().await;
    set_prices(&a, Some(2.0), Some(10.0), Some(0.5));
    let plain = a.chat("hello").await; // 100 in, 50 out
    let cached = a.chat("a cached prompt").await; // 100 in of which 64 cached, 50 out
    let failed = a.chat("please fail").await;

    let (_, body) = a.get("/api/server/v1/audit/traces").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let cost_of = |id: &str| v["traces"].as_array().unwrap().iter().find(|t| t["trace_id"] == id).unwrap()["cost"].clone();
    assert!(approx(&cost_of(&plain), (100.0 * 2.0 + 50.0 * 10.0) / 1e6), "{}", cost_of(&plain));
    assert!(
        approx(&cost_of(&cached), (36.0 * 2.0 + 64.0 * 0.5 + 50.0 * 10.0) / 1e6),
        "uncached input, cached input and output: {}",
        cost_of(&cached)
    );
    assert!(cost_of(&failed).is_null(), "nothing was used, so there is no cost to state");

    let (_, body) = a.get(&format!("/api/server/v1/audit/traces/{plain}")).await;
    let t: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(approx(&t["cost"], 0.0007), "{}", t["cost"]);
    let attempt = t["spans"].as_array().unwrap().iter().find(|s| s["kind"] == "attempt").unwrap();
    assert!(approx(&attempt["cost"], 0.0007), "the attempt is priced by its own provider");

    // A corrected price re-prices history at once: nothing about cost was stored.
    set_prices(&a, Some(4.0), Some(10.0), Some(0.5));
    let (_, body) = a.get(&format!("/api/server/v1/audit/traces/{plain}")).await;
    assert!(approx(&serde_json::from_str::<serde_json::Value>(&body).unwrap()["cost"], (100.0 * 4.0 + 500.0) / 1e6));

    // A price that is needed and missing gives null, not 0.
    set_prices(&a, Some(4.0), None, None);
    let (_, body) = a.get(&format!("/api/server/v1/audit/traces/{plain}")).await;
    assert!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["cost"].is_null());
    let (_, body) = a.get("/api/server/v1/audit/traces").await;
    assert!(!body.contains("\"cost\":0.0"), "never a made-up zero: {body}");
}

#[tokio::test]
async fn stats_total_the_cost_with_a_breakdown_by_provider_and_by_day_and_say_what_is_unknown() {
    let a = audit_stack().await;
    set_prices(&a, Some(2.0), Some(10.0), None);
    for _ in 0..3 {
        a.chat("hello").await;
    }
    a.chat("please fail").await;

    let (_, body) = a.get("/api/server/v1/audit/stats").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(approx(&v["cost"], 3.0 * 0.0007), "{}", v["cost"]);
    assert_eq!(v["cost_unknown_requests"], 0);
    let by_provider = v["by_provider"].as_array().unwrap();
    assert_eq!(by_provider.len(), 1);
    assert_eq!((by_provider[0]["provider_id"].as_str(), by_provider[0]["requests"].as_i64()), (Some("p1"), Some(4)));
    assert_eq!((by_provider[0]["tokens"]["in"].as_i64(), by_provider[0]["tokens"]["out"].as_i64()), (Some(300), Some(150)));
    assert!(approx(&by_provider[0]["cost"], 0.0021));
    let by_day = v["by_day"].as_array().unwrap();
    assert_eq!(by_day.len(), 1);
    let today = by_day[0]["day"].as_str().unwrap();
    assert_eq!(today.len(), 10);
    assert_eq!(&today[4..5], "-");
    assert!(approx(&by_day[0]["cost"], 0.0021) && by_day[0]["requests"] == 4);

    // With a price missing, the figure is unknown (null) and the response says how many requests are why.
    set_prices(&a, Some(2.0), None, None);
    let (_, body) = a.get("/api/server/v1/audit/stats").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["cost"].is_null() && v["by_provider"][0]["cost"].is_null() && v["by_day"][0]["cost"].is_null());
    assert_eq!(v["cost_unknown_requests"], 3);
    assert_metadata_only(&body);
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
    // Unknown provider key target, and the audit endpoints with no store recording.
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
