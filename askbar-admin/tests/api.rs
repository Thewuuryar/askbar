#![cfg(unix)]
//! The admin site's API against a real in-process server: changes land in `admin.db` and on
//! the server together, or not at all.

use askbar_admin::mgmt::{self, ServerLink};
use askbar_admin::{web, AdminDb, AdminState};
use askbar_server::{secrets::Secrets, AppState};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

const PW: &str = "correct horse battery";
const KEY: &str = "sk-live-VERY-SECRET-123";

struct T {
    adm: String,
    api: String,
    cookie: String,
    http: reqwest::Client,
    admin: Arc<AdminState>,
    server: Arc<AppState>,
    dir: PathBuf,
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

/// An OpenAI-style endpoint that answers only to `KEY`.
async fn fake_llm() -> String {
    use axum::{http::HeaderMap, response::IntoResponse, routing::post};
    let app = axum::Router::new().route(
        "/v1/chat/completions",
        post(|headers: HeaderMap| async move {
            let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
            if auth == format!("Bearer {KEY}") {
                ([("content-type", "text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n").into_response()
            } else {
                axum::http::StatusCode::UNAUTHORIZED.into_response()
            }
        }),
    );
    format!("{}/v1", serve(app).await)
}

/// `reachable = false` points the admin at an address where nothing listens.
async fn start(reachable: bool) -> T {
    let dir = std::env::temp_dir().join(format!("askbar-adminapi-{}", ulid::Ulid::new()));
    let (secrets, mgmt_dir) = (dir.join("admin-secrets"), dir.join("askbar-mgmt"));
    let token = mgmt::ensure_token(&secrets, &mgmt_dir).unwrap();

    let sdb = askbar_server::Db::open_in_memory().unwrap();
    sdb.seed_server().unwrap();
    let server = AppState::new(sdb, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    let api = serve(askbar_server::api_router(server.clone())).await;
    let mgmt_url = if reachable {
        serve(askbar_server::mgmt::router(server.clone(), mgmt_dir.join(mgmt::HASH_FILE))).await
    } else {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap()) // dropped: nothing listens
    };

    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PW).unwrap();
    let link = ServerLink::new(&mgmt_url, token);
    let admin = AdminState::with_server(db, link);
    let adm = serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { adm, api, cookie, http, admin, server, dir }
}

impl T {
    async fn call(&self, m: M, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.http.request(m, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").header("cookie", &self.cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let st = resp.status().as_u16();
        (st, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }
    fn snapshot(&self) -> Option<askbar_proto::Snapshot> {
        askbar_server::snapshot::current(&self.server.db.lock().unwrap()).unwrap()
    }
    async fn admin_rows(&self, sql: &str) -> i64 {
        self.admin.db.lock().await.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }
}

fn provider(name: &str, base: &str, key: Option<&str>) -> Value {
    let mut v = json!({ "name": name, "type": "openai", "base_url": base, "model": "m1", "max_tokens": 100 });
    if let Some(k) = key {
        v["api_key"] = json!(k);
    }
    v
}

#[tokio::test]
async fn providers_are_saved_here_pushed_to_the_server_and_the_key_goes_only_to_the_server() {
    let t = start(true).await;
    let llm = fake_llm().await;
    let (st, created) = t.call(M::POST, "/admin/api/providers", Some(provider("mock", &llm, Some(KEY)))).await;
    assert_eq!(st, 201);
    let id = created["id"].as_str().unwrap().to_string();

    // The server has the provider in its snapshot and the key, encrypted.
    let snap = t.snapshot().unwrap();
    assert_eq!((snap.providers.len(), snap.providers[0].id.as_str(), snap.providers[0].name.as_str()), (1, id.as_str(), "mock"));
    let enc: Vec<u8> = t.server.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers WHERE id = ?1", [&id], |r| r.get(0)).unwrap();
    assert_eq!(t.server.secrets().decrypt(&enc).unwrap(), KEY);

    // This side never holds the key: not in admin.db, not in the list.
    let (_, list) = t.call(M::GET, "/admin/api/providers", None).await;
    assert_eq!((list[0]["name"].as_str(), list[0]["has_key"].clone()), (Some("mock"), json!(true)));
    assert!(!list.to_string().contains(KEY));
    let dump: String = t.admin.db.lock().await.conn().query_row("SELECT group_concat(name || model || headers || base_url) FROM providers", [], |r| r.get(0)).unwrap();
    assert!(!dump.contains(KEY));
    let snapshot_text = serde_json::to_string(&snap).unwrap();
    assert!(!snapshot_text.contains(KEY), "the snapshot carries no key");

    // Test goes through the server, which has the key.
    let (st, res) = t.call(M::POST, &format!("/admin/api/providers/{id}/test"), None).await;
    assert_eq!((st, res["ok"].clone(), res["message"].as_str()), (200, json!(true), Some("ok")));
    assert!(!res.to_string().contains(KEY));

    // Editing keeps the key unless a new one comes; renaming reaches the server.
    assert_eq!(t.call(M::PUT, &format!("/admin/api/providers/{id}"), Some(provider("renamed", &llm, None))).await.0, 204);
    assert_eq!(t.snapshot().unwrap().providers[0].name, "renamed");
    assert_eq!(t.call(M::POST, &format!("/admin/api/providers/{id}/test"), None).await.1["ok"], json!(true), "the stored key still works");

    // Validation and name clashes change nothing.
    assert_eq!(t.call(M::POST, "/admin/api/providers", Some(json!({ "name": "", "type": "openai", "model": "m" }))).await.0, 400);
    assert_eq!(t.call(M::POST, "/admin/api/providers", Some(provider("renamed", &llm, None))).await.0, 409);
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM providers").await, 1);

    // Delete removes it from both sides, key included.
    assert_eq!(t.call(M::DELETE, &format!("/admin/api/providers/{id}"), None).await.0, 204);
    assert!(t.snapshot().unwrap().providers.is_empty());
    let left: i64 = t.server.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM providers", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
    assert_eq!(t.call(M::DELETE, &format!("/admin/api/providers/{id}"), None).await.0, 404);
    assert_eq!(t.call(M::POST, &format!("/admin/api/providers/{id}/test"), None).await.0, 404);
}

#[tokio::test]
async fn with_the_server_down_changes_get_502_and_nothing_is_saved() {
    let t = start(false).await;
    let llm = "http://x/v1";
    for (m, path, body) in [
        (M::POST, "/admin/api/providers", Some(provider("p", llm, Some(KEY)))),
        (M::GET, "/admin/api/devices", None),
        (M::POST, "/admin/api/pair", Some(json!({ "name": "laptop" }))),
        (M::PUT, "/admin/api/settings", Some(json!({ "system_prompt": "x", "display": askbar_proto::DisplaySettings::default() }))),
        (M::GET, "/admin/api/backup", None),
        (M::POST, "/admin/api/devices/d1/revoke", None),
    ] {
        let (st, res) = t.call(m, path, body).await;
        assert_eq!(st, 502, "{path}");
        if path != "/admin/api/backup" {
            assert!(res["error"].as_str().unwrap().contains("offline"), "{path}: {res}");
        }
    }
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM providers").await, 0, "no half-saved provider");
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM pair_codes").await, 0);
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM settings").await, 0);

    // Reading what this side owns still works, and the status says why the rest does not.
    assert_eq!(t.call(M::GET, "/admin/api/providers", None).await, (200, json!([])));
    let (st, status) = t.call(M::GET, "/admin/api/status", None).await;
    assert_eq!((st, status["server_online"].clone()), (200, json!(false)));
    assert!(status["error"].is_string());
}

#[tokio::test]
async fn status_reports_a_server_that_is_there() {
    let t = start(true).await;
    let (_, status) = t.call(M::GET, "/admin/api/status", None).await;
    assert_eq!(status["server_online"], json!(true));
}

#[tokio::test]
async fn a_pairing_string_pairs_a_device_which_can_then_be_revoked() {
    let t = start(true).await;
    let (st, out) = t.call(M::POST, "/admin/api/pair", Some(json!({ "name": "Work laptop" }))).await;
    assert_eq!(st, 200);
    let (url, code) = askbar_proto::decode_pairing(out["pairing"].as_str().unwrap()).expect("a pairing string");
    assert_eq!(url, "http://localhost:8080", "no HTTPS configured: the fallback address");
    assert_eq!(out["expires_in"], json!(600));

    // Only the hash is kept here; the server got the same hash.
    let stored: String = t.admin.db.lock().await.conn().query_row("SELECT hash FROM pair_codes", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, mgmt::sha256_hex(&code));
    assert_ne!(stored, code);
    assert_eq!(t.snapshot().unwrap().pair_codes[0].hash, stored);

    // The code pairs a device on the server's API, once.
    let pair = || t.http.post(format!("{}/api/v1/pair", t.api)).header("x-askbar-client-version", "0.3.0").json(&json!({ "code": code })).send();
    let r = pair().await.unwrap();
    assert_eq!(r.status(), 200);
    let token = r.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!(pair().await.unwrap().status(), 401, "a second use fails");

    let whoami = || t.http.get(format!("{}/api/v1/whoami", t.api)).bearer_auth(&token).header("x-askbar-client-version", "0.3.0").send();
    assert_eq!(whoami().await.unwrap().status(), 204);
    let (_, devices) = t.call(M::GET, "/admin/api/devices", None).await;
    assert_eq!((devices[0]["name"].as_str(), devices[0]["revoked"].clone()), (Some("Work laptop"), json!(false)));
    let id = devices[0]["id"].as_str().unwrap().to_string();

    assert_eq!(t.call(M::POST, &format!("/admin/api/devices/{id}/revoke"), None).await.0, 204);
    assert_eq!(whoami().await.unwrap().status(), 401, "the revoked device is refused");
    let (_, devices) = t.call(M::GET, "/admin/api/devices", None).await;
    assert_eq!(devices[0]["revoked"], json!(true));
    assert_eq!(t.call(M::POST, "/admin/api/devices/nope/revoke", None).await.0, 404);
    assert_eq!(t.call(M::POST, "/admin/api/pair", Some(json!({ "name": "x".repeat(61) }))).await.0, 400);
}

#[tokio::test]
async fn the_pairing_string_uses_the_public_https_name_when_https_is_on() {
    let t = start(true).await;
    t.admin.db.lock().await.conn().execute("UPDATE network_settings SET tls_enabled = 1, hostname = 'askbar.example.com'", []).unwrap();
    let (_, out) = t.call(M::POST, "/admin/api/pair", Some(json!({}))).await;
    assert_eq!(askbar_proto::decode_pairing(out["pairing"].as_str().unwrap()).unwrap().0, "https://askbar.example.com");
}

#[tokio::test]
async fn settings_and_per_user_display_defaults_reach_the_server() {
    let t = start(true).await;
    let (_, s) = t.call(M::GET, "/admin/api/settings", None).await;
    assert_eq!(s["system_prompt"].as_str(), Some(askbar_proto::DEFAULT_SYSTEM_PROMPT), "the built-in prompt until one is set");
    assert_eq!(s["display"]["width"], json!(720));

    let new = json!({ "system_prompt": "Be terse.", "display": { "theme": "dark", "width": 900, "max_height": 700, "history_width": 300, "top_percent": 0.3, "hide_on_blur": false } });
    assert_eq!(t.call(M::PUT, "/admin/api/settings", Some(new)).await.0, 204);
    let snap = t.snapshot().unwrap();
    assert_eq!((snap.system_prompt.as_deref(), snap.display.width, snap.display.theme.as_str()), (Some("Be terse."), 900, "dark"));
    assert_eq!(t.call(M::GET, "/admin/api/settings", None).await.1["system_prompt"], json!("Be terse."));
    assert_eq!(t.call(M::PUT, "/admin/api/settings", Some(json!({ "system_prompt": "x", "display": { "theme": "neon" } }))).await.0, 400);
    assert_eq!(t.call(M::PUT, "/admin/api/settings", Some(json!({ "system_prompt": "x".repeat(20_001), "display": {} }))).await.0, 400);
    assert_eq!(t.snapshot().unwrap().system_prompt.as_deref(), Some("Be terse."), "refused changes left it alone");

    let user: String = t.server.db.lock().unwrap().conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
    let mine = json!({ "theme": "light", "width": 500, "max_height": 400, "history_width": 200, "top_percent": 0.1, "hide_on_blur": true });
    assert_eq!(t.call(M::PUT, &format!("/admin/api/users/{user}/display-defaults"), Some(mine)).await.0, 204);
    let snap = t.snapshot().unwrap();
    assert_eq!(snap.user_display_defaults[&user].width, 500);
    assert_eq!(snap.display.width, 900, "the server-wide default is unchanged");
    assert_eq!(t.call(M::PUT, "/admin/api/users/nobody/display-defaults", Some(json!({}))).await.0, 404);
    assert_eq!(t.call(M::PUT, &format!("/admin/api/users/{user}/display-defaults"), Some(json!({ "width": 5 }))).await.0, 400);
}

fn tar_files(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 512 <= bytes.len() && bytes[at] != 0 {
        let name = String::from_utf8_lossy(&bytes[at..at + 100]).trim_end_matches('\0').to_string();
        let size = usize::from_str_radix(String::from_utf8_lossy(&bytes[at + 124..at + 135]).trim(), 8).unwrap();
        out.push((name, bytes[at + 512..at + 512 + size].to_vec()));
        at += 512 + size.div_ceil(512) * 512;
    }
    out
}

#[tokio::test]
async fn the_backup_holds_both_databases_and_none_of_the_secrets() {
    let t = start(true).await;
    let llm = fake_llm().await;
    t.call(M::POST, "/admin/api/providers", Some(provider("mock", &llm, Some(KEY)))).await;
    t.call(M::POST, "/admin/api/pair", Some(json!({ "name": "pending" }))).await;
    t.admin.db.lock().await.conn().execute("UPDATE network_settings SET cloudflare_token = x'deadbeefcafe'", []).unwrap();
    {
        let db = t.server.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let c = db.create_conversation(&user, "kept conversation").unwrap();
        db.add_message(&user, &c, "user", "question one").unwrap();
    }

    let r = t.http.get(format!("{}/admin/api/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-disposition"].to_str().unwrap().contains(".tar"));
    let bytes = r.bytes().await.unwrap();
    let files = tar_files(&bytes);
    assert_eq!(files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["server.db", "admin.db"]);
    for (_, data) in &files {
        assert_eq!(&data[..15], b"SQLite format 3");
        assert!(!data.windows(KEY.len()).any(|w| w == KEY.as_bytes()), "no provider key in either");
    }

    let open = |name: &str, data: &[u8]| {
        let p = t.dir.join(name);
        std::fs::create_dir_all(&t.dir).unwrap();
        std::fs::write(&p, data).unwrap();
        rusqlite::Connection::open(p).unwrap()
    };
    let server_db = open("s.db", &files[0].1);
    let admin_db = open("a.db", &files[1].1);
    let one = |c: &rusqlite::Connection, sql: &str| c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(one(&server_db, "SELECT COUNT(*) FROM conversations WHERE title = 'kept conversation'"), 1);
    assert_eq!(one(&server_db, "SELECT COUNT(*) FROM providers WHERE api_key_enc IS NOT NULL"), 0);
    assert_eq!(one(&admin_db, "SELECT COUNT(*) FROM providers"), 1, "the admin's own data is in it");
    assert_eq!(one(&admin_db, "SELECT COUNT(*) FROM admin_credentials"), 0, "no password hash");
    assert_eq!(one(&admin_db, "SELECT COUNT(*) FROM pair_codes"), 0, "no pairing-code hashes");
    assert_eq!(one(&admin_db, "SELECT COUNT(*) FROM network_settings WHERE cloudflare_token IS NOT NULL"), 0, "no Cloudflare token");
    // The live admin database still has them.
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM admin_credentials").await, 1);
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM pair_codes").await, 1);
}

#[tokio::test]
async fn all_of_it_needs_a_session() {
    let t = start(true).await;
    for (m, path) in [
        (M::GET, "/admin/api/status"),
        (M::GET, "/admin/api/providers"),
        (M::POST, "/admin/api/providers"),
        (M::PUT, "/admin/api/providers/x"),
        (M::DELETE, "/admin/api/providers/x"),
        (M::POST, "/admin/api/providers/x/test"),
        (M::GET, "/admin/api/devices"),
        (M::POST, "/admin/api/devices/x/revoke"),
        (M::POST, "/admin/api/pair"),
        (M::GET, "/admin/api/settings"),
        (M::PUT, "/admin/api/settings"),
        (M::PUT, "/admin/api/users/x/display-defaults"),
        (M::GET, "/admin/api/backup"),
    ] {
        let r = t.http.request(m, format!("{}{path}", t.adm)).header("x-askbar-admin", "1").json(&json!({})).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
}
