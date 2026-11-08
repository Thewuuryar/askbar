#![cfg(unix)]
//! The admin site's API against a real in-process server: changes land in `admin.db` and on
//! the server together, or not at all.

use askbar_admin::server_link::{self, ServerLink};
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
                ([("content-type", "text/event-stream")], "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n")
                    .into_response()
            } else {
                axum::http::StatusCode::UNAUTHORIZED.into_response()
            }
        }),
    );
    // The model list answers to the same key.
    let app = app.route(
        "/v1/models",
        axum::routing::get(|headers: HeaderMap| async move {
            if headers.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {KEY}")) {
                axum::Json(json!({ "data": [{ "id": "zeta" }, { "id": "alpha" }] })).into_response()
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
    let (secrets, token_dir) = (dir.join("admin-secrets"), dir.join("server-token"));
    let token = server_link::ensure_token(&secrets, &token_dir).unwrap();

    let sdb = askbar_server::Db::open_in_memory().unwrap();
    sdb.seed_server().unwrap();
    let server = AppState::new(sdb, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), askbar_server::MIN_CLIENT);
    let api = serve(askbar_server::client_router(server.clone())).await;
    let server_api_url = if reachable {
        serve(askbar_server::server_api::router(server.clone(), token_dir.join(server_link::HASH_FILE))).await
    } else {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        format!("http://{}", l.local_addr().unwrap()) // dropped: nothing listens
    };

    let db = AdminDb::open_in_memory().unwrap();
    web::init_password(&db, PW).unwrap();
    // One user to pair clients for (ADR 0008); the first change pushes it to the server.
    db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('U1', 'tester', 0)", []).unwrap();
    let link = ServerLink::new(&server_api_url, token);
    let admin = AdminState::with_server(db, link);
    let adm = serve(askbar_admin::router(admin.clone())).await;
    let http = reqwest::Client::new();
    let r = http
        .post(format!("{adm}/api/admin/v1/session"))
        .header("x-askbar-admin", "1")
        .json(&json!({ "password": PW }))
        .send()
        .await
        .unwrap();
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
    let (st, created) = t.call(M::POST, "/api/admin/v1/providers", Some(provider("mock", &llm, Some(KEY)))).await;
    assert_eq!(st, 201);
    let id = created["id"].as_str().unwrap().to_string();

    // The server has the provider in its snapshot and the key, encrypted.
    let snap = t.snapshot().unwrap();
    assert_eq!((snap.providers.len(), snap.providers[0].id.as_str(), snap.providers[0].name.as_str()), (1, id.as_str(), "mock"));
    let enc: Vec<u8> =
        t.server.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers WHERE id = ?1", [&id], |r| r.get(0)).unwrap();
    assert_eq!(t.server.secrets().decrypt(&enc).unwrap(), KEY);

    // This side never holds the key: not in admin.db, not in the list.
    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", None).await;
    assert_eq!((list[0]["name"].as_str(), list[0]["has_key"].clone()), (Some("mock"), json!(true)));
    assert!(!list.to_string().contains(KEY));
    let dump: String = t
        .admin
        .db
        .lock()
        .await
        .conn()
        .query_row("SELECT group_concat(name || model || headers || base_url) FROM providers", [], |r| r.get(0))
        .unwrap();
    assert!(!dump.contains(KEY));
    let snapshot_text = serde_json::to_string(&snap).unwrap();
    assert!(!snapshot_text.contains(KEY), "the snapshot carries no key");

    // Test goes through the server, which has the key.
    let (st, res) = t.call(M::POST, &format!("/api/admin/v1/providers/{id}/test"), None).await;
    assert_eq!((st, res["ok"].clone(), res["message"].as_str()), (200, json!(true), Some("ok")));
    assert!(!res.to_string().contains(KEY));

    // Editing keeps the key unless a new one comes; renaming reaches the server.
    assert_eq!(t.call(M::PUT, &format!("/api/admin/v1/providers/{id}"), Some(provider("renamed", &llm, None))).await.0, 204);
    assert_eq!(t.snapshot().unwrap().providers[0].name, "renamed");
    assert_eq!(
        t.call(M::POST, &format!("/api/admin/v1/providers/{id}/test"), None).await.1["ok"],
        json!(true),
        "the stored key still works"
    );

    // Validation and name clashes change nothing.
    assert_eq!(t.call(M::POST, "/api/admin/v1/providers", Some(json!({ "name": "", "type": "openai", "model": "m" }))).await.0, 400);
    assert_eq!(t.call(M::POST, "/api/admin/v1/providers", Some(provider("renamed", &llm, None))).await.0, 409);
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM providers").await, 1);

    // Delete removes it from both sides, key included.
    assert_eq!(t.call(M::DELETE, &format!("/api/admin/v1/providers/{id}"), None).await.0, 204);
    assert!(t.snapshot().unwrap().providers.is_empty());
    let left: i64 = t.server.db.lock().unwrap().conn().query_row("SELECT COUNT(*) FROM providers", [], |r| r.get(0)).unwrap();
    assert_eq!(left, 0);
    assert_eq!(t.call(M::DELETE, &format!("/api/admin/v1/providers/{id}"), None).await.0, 404);
    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/providers/{id}/test"), None).await.0, 404);
}

#[tokio::test]
async fn with_the_server_down_changes_get_502_and_nothing_is_saved() {
    let t = start(false).await;
    let llm = "http://x/v1";
    for (m, path, body) in [
        (M::POST, "/api/admin/v1/providers", Some(provider("p", llm, Some(KEY)))),
        (M::GET, "/api/admin/v1/clients", None),
        (M::POST, "/api/admin/v1/users/U1/pairing-keys", Some(json!({ "name": "laptop" }))),
        (M::PUT, "/api/admin/v1/settings", Some(json!({ "system_prompt": "x", "display": askbar_proto::DisplaySettings::default() }))),
        (M::GET, "/api/admin/v1/backup", None),
        (M::POST, "/api/admin/v1/clients/d1/revoke", None),
    ] {
        let (st, res) = t.call(m, path, body).await;
        assert_eq!(st, 502, "{path}");
        if path != "/api/admin/v1/backup" {
            assert!(res["detail"].as_str().unwrap().contains("offline"), "{path}: {res}");
        }
    }
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM providers").await, 0, "no half-saved provider");
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM pair_codes").await, 0);
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM settings").await, 0);

    // Reading what this side owns still works, and the status says why the rest does not.
    assert_eq!(t.call(M::GET, "/api/admin/v1/providers", None).await, (200, json!([])));
    let (st, status) = t.call(M::GET, "/api/admin/v1/status", None).await;
    assert_eq!((st, status["server_online"].clone()), (200, json!(false)));
    assert!(status["error"].is_string());
}

#[tokio::test]
async fn status_reports_a_server_that_is_there() {
    let t = start(true).await;
    let (_, status) = t.call(M::GET, "/api/admin/v1/status", None).await;
    assert_eq!(status["server_online"], json!(true));
}

#[tokio::test]
async fn a_pairing_string_pairs_a_client_which_can_then_be_revoked() {
    let t = start(true).await;
    let (st, out) = t.call(M::POST, "/api/admin/v1/users/U1/pairing-keys", Some(json!({ "name": "Work laptop" }))).await;
    assert_eq!(st, 200);
    let (url, code) = askbar_proto::decode_pairing(out["pairing"].as_str().unwrap()).expect("a pairing key");
    assert_eq!(
        url,
        format!("https://{}", t.adm.trim_start_matches("http://")),
        "no host name: the address the admin was opened with, over HTTPS"
    );
    assert_eq!(out["expires_in"], json!(600));

    // Only the hash is kept here; the server got the same hash.
    let stored: String = t.admin.db.lock().await.conn().query_row("SELECT hash FROM pair_codes", [], |r| r.get(0)).unwrap();
    assert_eq!(stored, server_link::sha256_hex(&code));
    assert_ne!(stored, code);
    assert_eq!(t.snapshot().unwrap().pair_codes[0].hash, stored);

    // The code pairs a client on the server's API, once.
    let pair = || {
        t.http
            .post(format!("{}/api/client/v1/pairings", t.api))
            .header("x-askbar-client-version", "0.5.0")
            .json(&json!({ "code": code }))
            .send()
    };
    let r = pair().await.unwrap();
    assert_eq!(r.status(), 200);
    let token = r.json::<Value>().await.unwrap()["token"].as_str().unwrap().to_string();
    assert_eq!(pair().await.unwrap().status(), 401, "a second use fails");

    let whoami = || t.http.get(format!("{}/api/client/v1/me", t.api)).bearer_auth(&token).header("x-askbar-client-version", "0.5.0").send();
    assert_eq!(whoami().await.unwrap().status(), 204);
    let (_, clients) = t.call(M::GET, "/api/admin/v1/clients", None).await;
    assert_eq!((clients[0]["name"].as_str(), clients[0]["revoked"].clone()), (Some("Work laptop"), json!(false)));
    let id = clients[0]["id"].as_str().unwrap().to_string();

    assert_eq!(t.call(M::POST, &format!("/api/admin/v1/clients/{id}/revoke"), None).await.0, 204);
    assert_eq!(whoami().await.unwrap().status(), 401, "the revoked client is refused");
    let (_, clients) = t.call(M::GET, "/api/admin/v1/clients", None).await;
    assert_eq!(clients[0]["revoked"], json!(true));
    assert_eq!(t.call(M::POST, "/api/admin/v1/clients/nope/revoke", None).await.0, 404);
    assert_eq!(t.call(M::POST, "/api/admin/v1/users/U1/pairing-keys", Some(json!({ "name": "x".repeat(61) }))).await.0, 400);
}

#[tokio::test]
async fn the_pairing_string_uses_the_public_https_name_when_https_is_on() {
    let t = start(true).await;
    t.admin.db.lock().await.conn().execute("UPDATE network_settings SET tls_enabled = 1, hostname = 'askbar.example.com'", []).unwrap();
    let (_, out) = t.call(M::POST, "/api/admin/v1/users/U1/pairing-keys", Some(json!({}))).await;
    assert_eq!(askbar_proto::decode_pairing(out["pairing"].as_str().unwrap()).unwrap().0, "https://askbar.example.com");
}

#[tokio::test]
async fn provider_billing_and_prices_round_trip_reach_the_server_and_are_validated() {
    let t = start(true).await;
    let base = json!({ "name": "metered one", "type": "openai", "base_url": "http://x/v1", "model": "m" });
    // Omitted fields mean what the server would assume: metered, unknown prices.
    let (st, body) = t.call(M::POST, "/api/admin/v1/providers", Some(base.clone())).await;
    assert_eq!(st, 201, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", None).await;
    assert_eq!((&list[0]["billing"], &list[0]["price_in"]), (&json!("metered"), &json!(null)));
    assert!(list[0].get("allow_as_fallback").is_none(), "there is no provider fallback (ADR 0014)");
    let sp = t.snapshot().unwrap().providers.into_iter().find(|p| p.id == id).unwrap();
    assert_eq!((sp.billing, sp.price_in, sp.price_out, sp.price_cached_in), (askbar_proto::Billing::Metered, None, None, None));

    // Setting them: stored, listed and pushed.
    let mut full = base.clone();
    full["billing"] = json!("subscription");
    full["allow_as_fallback"] = json!(true); // sent by an older admin page: accepted and ignored
    full["price_in"] = json!(2.5);
    full["price_out"] = json!(10.0);
    full["price_cached_in"] = json!(0.0);
    assert_eq!(t.call(M::PUT, &format!("/api/admin/v1/providers/{id}"), Some(full.clone())).await.0, 204);
    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", None).await;
    assert_eq!(
        (&list[0]["billing"], &list[0]["price_in"], &list[0]["price_out"], &list[0]["price_cached_in"]),
        (&json!("subscription"), &json!(2.5), &json!(10.0), &json!(0.0))
    );
    assert!(list[0].get("allow_as_fallback").is_none());
    let sp = t.snapshot().unwrap().providers.into_iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        (sp.billing, sp.price_in, sp.price_out, sp.price_cached_in),
        (askbar_proto::Billing::Subscription, Some(2.5), Some(10.0), Some(0.0))
    );
    assert!(!t.snapshot().unwrap().providers.iter().any(|p| serde_json::to_string(p).unwrap().contains("fallback")));

    // Validation matches the server's: billing words, and prices of zero or more.
    for (field, value) in
        [("billing", json!("free")), ("price_in", json!(-1.0)), ("price_out", json!(-0.5)), ("price_cached_in", json!(-3))]
    {
        let mut bad = base.clone();
        bad["name"] = json!("bad one");
        bad[field] = value;
        let (st, body) = t.call(M::POST, "/api/admin/v1/providers", Some(bad)).await;
        assert_eq!(st, 400, "{field}: {body}");
    }
    let (_, list) = t.call(M::GET, "/api/admin/v1/providers", None).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "nothing refused was saved");
}

#[tokio::test]
async fn the_generation_timeout_round_trips_reaches_the_server_and_is_range_checked() {
    let t = start(true).await;
    let disp = serde_json::to_value(askbar_proto::DisplaySettings::default()).unwrap();
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", None).await;
    assert_eq!(s["generation_timeout_secs"], json!(300), "the default until one is set");

    let save = |secs: serde_json::Value| json!({ "system_prompt": "x", "display": disp, "generation_timeout_secs": secs });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(save(json!(120)))).await.0, 204);
    assert_eq!(t.call(M::GET, "/api/admin/v1/settings", None).await.1["generation_timeout_secs"], json!(120));
    assert_eq!(t.snapshot().unwrap().generation_timeout_secs, 120, "pushed to the server");

    // Out of range, either side: refused, and nothing changes.
    for bad in [29, 3601, 0] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(save(json!(bad)))).await.0, 400, "{bad}");
    }
    assert_eq!(
        t.call(M::PUT, "/api/admin/v1/settings", Some(save(json!("soon")))).await.0,
        400,
        "not a number: a body that does not fit is a bad_request problem (it was a bare 422)"
    );
    assert_eq!(t.call(M::GET, "/api/admin/v1/settings", None).await.1["generation_timeout_secs"], json!(120));
    assert_eq!(t.snapshot().unwrap().generation_timeout_secs, 120);

    // The edges are fine, and a save that leaves the field out keeps the stored value.
    for ok in [30, 3600] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(save(json!(ok)))).await.0, 204, "{ok}");
    }
    let without = json!({ "system_prompt": "y", "display": disp });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(without)).await.0, 204);
    assert_eq!(t.call(M::GET, "/api/admin/v1/settings", None).await.1["generation_timeout_secs"], json!(3600));
}

#[tokio::test]
async fn the_kill_switch_and_its_abort_setting_round_trip_and_reach_the_server() {
    let t = start(true).await;
    let disp = serde_json::to_value(askbar_proto::DisplaySettings::default()).unwrap();
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", None).await;
    assert_eq!((&s["kill_switch"], &s["kill_switch_aborts_running"]), (&json!(false), &json!(true)), "off, and aborting by default");
    assert!(t.snapshot().is_none() || !t.snapshot().unwrap().kill_switch);

    let save = |kill: Option<bool>, aborts: Option<bool>| {
        let mut b = json!({ "system_prompt": "x", "display": disp });
        if let Some(k) = kill {
            b["kill_switch"] = json!(k);
        }
        if let Some(a) = aborts {
            b["kill_switch_aborts_running"] = json!(a);
        }
        b
    };
    for (kill, aborts) in [(true, true), (true, false), (false, false), (false, true)] {
        assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(save(Some(kill), Some(aborts)))).await.0, 204);
        let (_, s) = t.call(M::GET, "/api/admin/v1/settings", None).await;
        assert_eq!((&s["kill_switch"], &s["kill_switch_aborts_running"]), (&json!(kill), &json!(aborts)));
        let snap = t.snapshot().unwrap();
        assert_eq!((snap.kill_switch, snap.kill_switch_aborts_running), (kill, aborts), "the server has both");
    }
    // A save that leaves them out keeps what is stored.
    t.call(M::PUT, "/api/admin/v1/settings", Some(save(Some(true), Some(false)))).await;
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(save(None, None))).await.0, 204);
    let snap = t.snapshot().unwrap();
    assert_eq!((snap.kill_switch, snap.kill_switch_aborts_running), (true, false));
}

#[tokio::test]
async fn settings_and_per_user_display_defaults_reach_the_server() {
    let t = start(true).await;
    let (_, s) = t.call(M::GET, "/api/admin/v1/settings", None).await;
    assert_eq!(s["system_prompt"].as_str(), Some(askbar_proto::DEFAULT_SYSTEM_PROMPT), "the built-in prompt until one is set");
    assert_eq!(s["display"]["width"], json!(720));

    let new = json!({ "system_prompt": "Be terse.", "display": { "theme": "dark", "width": 900, "max_height": 700, "history_width": 300, "top_percent": 0.3, "hide_on_blur": false } });
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(new)).await.0, 204);
    let snap = t.snapshot().unwrap();
    assert_eq!((snap.system_prompt.as_deref(), snap.display.width, snap.display.theme.as_str()), (Some("Be terse."), 900, "dark"));
    assert_eq!(t.call(M::GET, "/api/admin/v1/settings", None).await.1["system_prompt"], json!("Be terse."));
    assert_eq!(
        t.call(M::PUT, "/api/admin/v1/settings", Some(json!({ "system_prompt": "x", "display": { "theme": "neon" } }))).await.0,
        400
    );
    assert_eq!(t.call(M::PUT, "/api/admin/v1/settings", Some(json!({ "system_prompt": "x".repeat(20_001), "display": {} }))).await.0, 400);
    assert_eq!(t.snapshot().unwrap().system_prompt.as_deref(), Some("Be terse."), "refused changes left it alone");

    let user: String = t.server.db.lock().unwrap().conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
    let mine = json!({ "theme": "light", "width": 500, "max_height": 400, "history_width": 200, "top_percent": 0.1, "hide_on_blur": true });
    assert_eq!(t.call(M::PUT, &format!("/api/admin/v1/users/{user}/display-defaults"), Some(mine)).await.0, 204);
    let snap = t.snapshot().unwrap();
    assert_eq!(snap.user_display_defaults[&user].width, 500);
    assert_eq!(snap.display.width, 900, "the server-wide default is unchanged");
    assert_eq!(t.call(M::PUT, "/api/admin/v1/users/nobody/display-defaults", Some(json!({}))).await.0, 404);
    assert_eq!(t.call(M::PUT, &format!("/api/admin/v1/users/{user}/display-defaults"), Some(json!({ "width": 5 }))).await.0, 400);
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
    t.call(M::POST, "/api/admin/v1/providers", Some(provider("mock", &llm, Some(KEY)))).await;
    t.call(M::POST, "/api/admin/v1/users/U1/pairing-keys", Some(json!({ "name": "pending" }))).await;
    t.admin.db.lock().await.conn().execute("UPDATE network_settings SET cloudflare_token = x'deadbeefcafe'", []).unwrap();
    {
        let db = t.server.db.lock().unwrap();
        let user: String = db.conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
        let c = db.create_conversation(&user, "kept conversation").unwrap();
        db.add_message(&user, &c, "user", "question one").unwrap();
    }

    let r = t.http.get(format!("{}/api/admin/v1/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
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

/// `audit.db` (the trace of replies) lives in its own volume and is not part of the admin backup;
/// the README says how to copy the volume. The archive must hold none of its tables.
#[tokio::test]
async fn the_backup_has_no_audit_store_in_it() {
    let t = start(true).await;
    let r = t.http.get(format!("{}/api/admin/v1/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let files = tar_files(&r.bytes().await.unwrap());
    assert!(
        files.iter().all(|(name, _)| !name.contains("audit")),
        "no audit.db in the archive: {:?}",
        files.iter().map(|(n, _)| n).collect::<Vec<_>>()
    );
    std::fs::create_dir_all(&t.dir).unwrap();
    for (name, data) in &files {
        let p = t.dir.join(format!("nb-{name}"));
        std::fs::write(&p, data).unwrap();
        let c = rusqlite::Connection::open(p).unwrap();
        for table in ["spans", "payloads"] {
            let n: i64 =
                c.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", [table], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "{name} has an audit table `{table}`");
        }
    }
}

#[tokio::test]
async fn the_backup_leaves_out_the_admin_api_key() {
    let t = start(true).await;
    let (_, made) = t.call(M::POST, "/api/admin/v1/keys", Some(json!({ "name": "ci" }))).await;
    let key_hash = askbar_admin::api_key::hash(made["key"].as_str().unwrap());
    let r = t.http.get(format!("{}/api/admin/v1/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
    let bytes = r.bytes().await.unwrap();
    let files = tar_files(&bytes);
    let (_, admin_db) = files.iter().find(|(n, _)| n == "admin.db").unwrap();
    assert!(!admin_db.windows(key_hash.len()).any(|w| w == key_hash.as_bytes()), "the key's hash is not in the backup");
    std::fs::create_dir_all(&t.dir).unwrap();
    let p = t.dir.join("keys-backup.db");
    std::fs::write(&p, admin_db).unwrap();
    let n: i64 = rusqlite::Connection::open(p).unwrap().query_row("SELECT COUNT(*) FROM admin_api_keys", [], |r| r.get(0)).unwrap();
    assert_eq!(n, 0);
    assert_eq!(t.admin_rows("SELECT COUNT(*) FROM admin_api_keys").await, 1, "the live database still has it");
}

#[tokio::test]
async fn all_of_it_needs_a_session() {
    let t = start(true).await;
    for (m, path) in [
        (M::GET, "/api/admin/v1/status"),
        (M::GET, "/api/admin/v1/providers"),
        (M::POST, "/api/admin/v1/providers"),
        (M::PUT, "/api/admin/v1/providers/x"),
        (M::DELETE, "/api/admin/v1/providers/x"),
        (M::POST, "/api/admin/v1/providers/x/test"),
        (M::GET, "/api/admin/v1/clients"),
        (M::POST, "/api/admin/v1/clients/x/revoke"),
        (M::POST, "/api/admin/v1/users/U1/pairing-keys"),
        (M::GET, "/api/admin/v1/settings"),
        (M::PUT, "/api/admin/v1/settings"),
        (M::PUT, "/api/admin/v1/users/x/display-defaults"),
        (M::GET, "/api/admin/v1/backup"),
    ] {
        let r = t.http.request(m, format!("{}{path}", t.adm)).header("x-askbar-admin", "1").json(&json!({})).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
}

#[tokio::test]
async fn the_admin_api_key_is_shown_once_listed_by_name_and_revoked() {
    let t = start(true).await;
    // None yet.
    assert_eq!(t.call(M::GET, "/api/admin/v1/keys", None).await, (200, json!([])));
    // Make it: the key is in this answer and nowhere else.
    let (st, made) = t.call(M::POST, "/api/admin/v1/keys", Some(json!({ "name": " build server " }))).await;
    assert_eq!(st, 201, "{made}");
    let key = made["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("askbar-admin-"), "{key}");
    assert_eq!((made["name"].as_str(), made["scope"].as_str()), (Some("build server"), Some("admin")));
    let id = made["id"].as_str().unwrap().to_string();
    // Afterwards: a name and times.
    let (_, listed) = t.call(M::GET, "/api/admin/v1/keys", None).await;
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["id"], id.as_str());
    assert!(listed[0].get("key").is_none() && listed[0].get("hash").is_none(), "{listed}");
    // The key shows up in no other answer: the keys, the status, the settings, a backup.
    let mut seen = vec![listed.to_string()];
    for path in
        ["/api/admin/v1/status", "/api/admin/v1/settings", "/api/admin/v1/network", "/api/admin/v1/providers", "/api/admin/v1/clients"]
    {
        seen.push(t.call(M::GET, path, None).await.1.to_string());
    }
    let backup =
        t.http.get(format!("{}/api/admin/v1/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap().bytes().await.unwrap();
    assert!(!backup.windows(key.len()).any(|w| w == key.as_bytes()), "not in a backup");
    assert!(seen.iter().all(|s| !s.contains(&key)), "not in any later answer");
    // One at a time.
    let (st, again) = t.call(M::POST, "/api/admin/v1/keys", Some(json!({ "name": "second" }))).await;
    assert_eq!((st, again["error"].as_str()), (409, Some("api_key_exists")), "{again}");
    // A bad name.
    t.call(M::DELETE, &format!("/api/admin/v1/keys/{id}"), None).await;
    for bad in ["", "   ", &"x".repeat(61)] {
        let (st, e) = t.call(M::POST, "/api/admin/v1/keys", Some(json!({ "name": bad }))).await;
        assert_eq!((st, e["error"].as_str()), (400, Some("bad_request")), "{bad:?}");
    }
    // Revoked: it stops working at once, and a new one can be made.
    assert_eq!(t.call(M::DELETE, &format!("/api/admin/v1/keys/{id}"), None).await.0, 404, "already revoked");
    let with_key = |k: &str| t.http.get(format!("{}/api/admin/v1/providers", t.adm)).bearer_auth(k).send();
    assert_eq!(with_key(&key).await.unwrap().status(), 401);
    let (st, second) = t.call(M::POST, "/api/admin/v1/keys", Some(json!({ "name": "second" }))).await;
    assert_eq!(st, 201);
    assert_eq!(with_key(second["key"].as_str().unwrap()).await.unwrap().status(), 200);
}

#[tokio::test]
async fn the_key_routes_need_a_session_and_a_key_cannot_make_or_revoke_keys() {
    let t = start(true).await;
    let (_, made) = t.call(M::POST, "/api/admin/v1/keys", Some(json!({ "name": "ci" }))).await;
    let key = made["key"].as_str().unwrap().to_string();
    for (m, path) in [
        (reqwest::Method::GET, "/api/admin/v1/keys"),
        (reqwest::Method::POST, "/api/admin/v1/keys"),
        (reqwest::Method::DELETE, "/api/admin/v1/keys/x"),
    ] {
        let signed_out = t.http.request(m.clone(), format!("{}{path}", t.adm)).header("x-askbar-admin", "1").send().await.unwrap();
        assert_eq!(signed_out.status(), 401, "{m} {path} with nothing");
        let with_key =
            t.http.request(m.clone(), format!("{}{path}", t.adm)).bearer_auth(&key).json(&json!({ "name": "evil" })).send().await.unwrap();
        assert_eq!(with_key.status(), 401, "{m} {path} with the key");
    }
    assert_eq!(t.call(M::GET, "/api/admin/v1/keys", None).await.1.as_array().unwrap().len(), 1, "the key made no key");
}

#[tokio::test]
async fn models_are_listed_by_the_server_with_a_typed_key_or_the_saved_one() {
    let t = start(true).await;
    let llm = fake_llm().await;
    let ask = |body: Value| t.call(M::POST, "/api/admin/v1/providers/models", Some(body));

    let (st, res) = ask(json!({ "type": "openai", "base_url": llm, "api_key": KEY })).await;
    assert_eq!((st, res["ok"].clone(), res["models"].clone()), (200, json!(true), json!([{ "id": "alpha" }, { "id": "zeta" }])));
    assert!(!res.to_string().contains(KEY));

    let (_, res) = ask(json!({ "type": "openai", "base_url": llm, "api_key": "wrong-key-123" })).await;
    assert_eq!(res["ok"], json!(false));
    assert!(!res.to_string().contains("wrong-key-123"));

    // A saved provider's key is used when the address is the saved one, and only then.
    let (_, created) = t.call(M::POST, "/api/admin/v1/providers", Some(provider("mock", &llm, Some(KEY)))).await;
    let id = created["id"].as_str().unwrap();
    let (_, res) = ask(json!({ "type": "openai", "base_url": llm, "provider_id": id })).await;
    assert_eq!(res["ok"], json!(true), "{res}");
    let (_, res) = ask(json!({ "type": "openai", "base_url": format!("{llm}x"), "provider_id": id })).await;
    assert_eq!(res["ok"], json!(false), "the saved key is not sent to another address");

    assert_eq!(ask(json!({ "type": "gemini" })).await.0, 400);
    assert_eq!(t.call(M::POST, "/api/admin/v1/providers/models", Some(json!({ "type": "openai" }))).await.0, 200);
}

#[tokio::test]
async fn models_get_502_when_the_server_is_down() {
    let t = start(false).await;
    let (st, _) = t.call(M::POST, "/api/admin/v1/providers/models", Some(json!({ "type": "openai" }))).await;
    assert_eq!(st, 502);
}

#[tokio::test]
async fn users_are_made_here_reach_the_server_and_each_gets_its_own_pairing_keys() {
    let t = start(true).await;
    let (st, made) = t.call(M::POST, "/api/admin/v1/users", Some(json!({ "name": " Alice " }))).await;
    assert_eq!(st, 201, "{made}");
    let alice = made["id"].as_str().unwrap().to_string();
    let (_, list) = t.call(M::GET, "/api/admin/v1/users", None).await;
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|u| u["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["tester", "Alice"], "oldest first, the name trimmed");
    // Pushed to the server, which now has the user.
    let snap = t.snapshot().unwrap();
    assert!(snap.users.iter().any(|u| u.id == alice && u.name == "Alice"));
    let on_server: String =
        t.server.db.lock().unwrap().conn().query_row("SELECT name FROM users WHERE id = ?1", [&alice], |r| r.get(0)).unwrap();
    assert_eq!(on_server, "Alice");

    // A pairing key for Alice pairs a client that belongs to Alice.
    let (st, out) = t.call(M::POST, &format!("/api/admin/v1/users/{alice}/pairing-keys"), Some(json!({ "name": "Alice's laptop" }))).await;
    assert_eq!(st, 200);
    let p = askbar_proto::Pairing::decode(out["pairing"].as_str().unwrap()).unwrap();
    let r = t
        .http
        .post(format!("{}/api/client/v1/pairings", t.api))
        .header("x-askbar-client-version", "0.5.0")
        .json(&json!({ "code": p.code }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let owner: String = t
        .server
        .db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT user_id FROM clients WHERE name = 'Alice''s laptop'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(owner, alice);

    // Refusals.
    assert_eq!(t.call(M::POST, "/api/admin/v1/users", Some(json!({ "name": "Alice" }))).await.0, 409, "names are unique");
    for bad in ["", "   ", &"x".repeat(61)] {
        assert_eq!(t.call(M::POST, "/api/admin/v1/users", Some(json!({ "name": bad }))).await.0, 400, "{bad:?}");
    }
    assert_eq!(t.call(M::POST, "/api/admin/v1/users/nobody/pairing-keys", Some(json!({}))).await.0, 404);
}

#[tokio::test]
async fn an_install_from_before_takes_over_the_servers_users_so_its_clients_keep_working() {
    let t = start(true).await;
    t.admin.db.lock().await.conn().execute("DELETE FROM users", []).unwrap(); // as before 1.9: no users here
    let owner: String = t.server.db.lock().unwrap().conn().query_row("SELECT id FROM users", [], |r| r.get(0)).unwrap();
    let (st, list) = t.call(M::GET, "/api/admin/v1/users", None).await;
    assert_eq!(st, 200);
    assert_eq!(
        (list[0]["id"].as_str(), list[0]["name"].as_str()),
        (Some(owner.as_str()), Some("owner")),
        "the same id, so its clients stay its own"
    );
    assert_eq!(list.as_array().unwrap().len(), 1);
    // Taken over once: the next read does not ask again or duplicate.
    let (_, again) = t.call(M::GET, "/api/admin/v1/users", None).await;
    assert_eq!(again, list);
}

#[tokio::test]
async fn a_provider_whose_key_the_server_lost_says_so_until_the_key_is_entered_again() {
    let t = start(true).await;
    let llm = fake_llm().await;
    let (_, created) = t.call(M::POST, "/api/admin/v1/providers", Some(provider("mock", &llm, Some(KEY)))).await;
    let id = created["id"].as_str().unwrap().to_string();
    let (_, _) = t.call(M::POST, "/api/admin/v1/providers", Some(provider("keyless", &llm, None))).await;
    let flags = || async {
        let (_, list) = t.call(M::GET, "/api/admin/v1/providers", None).await;
        list.as_array()
            .unwrap()
            .iter()
            .map(|p| (p["name"].as_str().unwrap().to_string(), p["key_missing"].as_bool().unwrap()))
            .collect::<Vec<_>>()
    };
    assert_eq!(flags().await, [("keyless".to_string(), false), ("mock".to_string(), false)]);

    // The server's database was replaced: it lists the provider (the snapshot) but holds no key.
    t.server.db.lock().unwrap().conn().execute("UPDATE providers SET api_key_enc = NULL WHERE id = ?1", [&id]).unwrap();
    assert_eq!(flags().await, [("keyless".to_string(), false), ("mock".to_string(), true)], "only the one the admin sent a key for");

    // Entering the key again (an edit with a key) clears it.
    let (st, _) = t.call(M::PUT, &format!("/api/admin/v1/providers/{id}"), Some(provider("mock", &llm, Some(KEY)))).await;
    assert_eq!(st, 204);
    assert_eq!(flags().await, [("keyless".to_string(), false), ("mock".to_string(), false)]);
}
