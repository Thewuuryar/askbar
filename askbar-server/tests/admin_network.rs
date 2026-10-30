use askbar_server::{admin, secrets::Secrets, AppState};
use reqwest::Method as M;
use serde_json::{json, Value};
use std::sync::Arc;

const PW: &str = "first password 123";
const TOKEN: &str = "cf_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";

struct T {
    adm: String,
    state: Arc<AppState>,
    cookie: String,
    http: reqwest::Client,
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    admin::init_password(&db, PW).unwrap();
    let state = AppState::with_config(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap(), "0.1.0", "http://localhost:8080");
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let adm = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::admin_router(state.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let r = http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { adm, state, cookie, http }
}

impl T {
    async fn call(&self, m: M, path: &str, body: Value) -> (u16, Value) {
        let r = self.http.request(m, format!("{}{path}", self.adm)).header("x-askbar-admin", "1").header("cookie", &self.cookie).json(&body).send().await.unwrap();
        let st = r.status().as_u16();
        (st, serde_json::from_str(&r.text().await.unwrap()).unwrap_or(Value::Null))
    }
    async fn save(&self, body: Value) -> (u16, Value) {
        self.call(M::PUT, "/admin/api/network", body).await
    }
    fn stored_token(&self) -> Option<String> {
        self.state.db.lock().unwrap().conn().query_row("SELECT value FROM settings WHERE key='cloudflare_token'", [], |r| r.get(0)).ok()
    }
}

fn full() -> Value {
    json!({ "tls_enabled": true, "hostname": "Askbar.Orraitu.com", "admin_cidr": "192.168.1.0/24, 10.0.0.5", "acme_staging": true, "cloudflare_token": TOKEN })
}

#[tokio::test]
async fn saving_normalises_and_encrypts_the_token() {
    let t = start().await;
    let (st, v) = t.call(M::GET, "/admin/api/network", json!({})).await;
    assert_eq!(st, 200);
    assert_eq!((v["tls_enabled"].as_bool(), v["has_token"].as_bool(), v["api_port"].as_u64(), v["admin_port"].as_u64()), (Some(false), Some(false), Some(8443), Some(9443)));
    assert_eq!(v["public_url"], "http://localhost:8080");

    let (st, v) = t.save(full()).await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["hostname"], "askbar.orraitu.com", "lower-cased");
    assert_eq!(v["admin_cidr"], "192.168.1.0/24 10.0.0.5/32", "normalised");
    assert_eq!((v["tls_enabled"].as_bool(), v["acme_staging"].as_bool(), v["has_token"].as_bool()), (Some(true), Some(true), Some(true)));
    assert_eq!(v["public_url"], "https://askbar.orraitu.com:8443");
    assert!(!v.to_string().contains(TOKEN) && !v.to_string().contains("cloudflare_token"), "write-only");

    // At rest the token is ciphertext that only the master key opens.
    let stored = t.stored_token().unwrap();
    assert!(!stored.contains(TOKEN));
    assert_eq!(t.state.secrets().unwrap().decrypt_b64(&stored).unwrap(), TOKEN);
    let (_, again) = t.call(M::GET, "/admin/api/network", json!({})).await;
    assert!(!again.to_string().contains(TOKEN));
}

#[tokio::test]
async fn leaving_the_token_blank_keeps_it_and_clear_removes_it() {
    let t = start().await;
    t.save(full()).await;
    let before = t.stored_token().unwrap();
    let mut keep = full();
    keep.as_object_mut().unwrap().remove("cloudflare_token");
    keep["acme_staging"] = json!(false);
    let (st, v) = t.save(keep.clone()).await;
    assert_eq!((st, v["has_token"].as_bool(), v["acme_staging"].as_bool()), (200, Some(true), Some(false)));
    assert_eq!(t.stored_token().unwrap(), before);
    keep["cloudflare_token"] = json!("");
    assert_eq!(t.save(keep.clone()).await.0, 200);
    assert_eq!(t.stored_token().unwrap(), before, "empty string also keeps it");

    // Removing it while HTTPS is on is refused; with HTTPS off it works.
    keep["clear_token"] = json!(true);
    let (st, e) = t.save(keep.clone()).await;
    assert_eq!(st, 400);
    assert!(e["error"].as_str().unwrap().contains("token"));
    assert_eq!(t.stored_token().unwrap(), before);
    keep["tls_enabled"] = json!(false);
    let (st, v) = t.save(keep).await;
    assert_eq!((st, v["has_token"].as_bool()), (200, Some(false)));
    assert!(t.stored_token().is_none());
}

#[tokio::test]
async fn invalid_input_is_refused_and_changes_nothing() {
    let t = start().await;
    for (label, patch) in [
        ("url as hostname", json!({ "hostname": "https://askbar.orraitu.com" })),
        ("port in hostname", json!({ "hostname": "askbar.orraitu.com:8443" })),
        ("no dot", json!({ "hostname": "localhost" })),
        ("bad cidr", json!({ "admin_cidr": "192.168.1.0/99" })),
        ("junk cidr", json!({ "admin_cidr": "everyone" })),
        ("token with space", json!({ "cloudflare_token": "abc def abc def abc def abc def" })),
        ("token with newline", json!({ "cloudflare_token": "abcdefghijklmnopqrstuvwxyz\nimport evil" })),
        ("token with brace", json!({ "cloudflare_token": "abcdefghijklmnopqrstuvwxyz}{" })),
        ("short token", json!({ "cloudflare_token": "abc" })),
        ("tls without hostname", json!({ "hostname": "" })),
        ("tls without networks", json!({ "admin_cidr": "" })),
    ] {
        let mut body = full();
        for (k, v) in patch.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (st, e) = t.save(body).await;
        assert_eq!(st, 400, "{label}");
        assert!(e["error"].is_string(), "{label}");
    }
    let (_, v) = t.call(M::GET, "/admin/api/network", json!({})).await;
    assert_eq!((v["tls_enabled"].as_bool(), v["hostname"].as_str(), v["has_token"].as_bool()), (Some(false), Some(""), Some(false)));
    assert!(t.stored_token().is_none());
}

#[tokio::test]
async fn http_mode_saves_without_a_token_and_pairing_follows_the_setting() {
    let t = start().await;
    let (st, _) = t.save(json!({ "tls_enabled": false, "hostname": "", "admin_cidr": "", "acme_staging": false })).await;
    assert_eq!(st, 200);
    let pairing = |t: &T| {
        let url = format!("{}/admin/api/pair", t.adm);
        let (http, cookie) = (t.http.clone(), t.cookie.clone());
        async move {
            let v: Value = http.post(url).header("x-askbar-admin", "1").header("cookie", cookie).json(&json!({})).send().await.unwrap().json().await.unwrap();
            askbar_proto::decode_pairing(v["pairing"].as_str().unwrap()).unwrap().0
        }
    };
    assert_eq!(pairing(&t).await, "http://localhost:8080");
    assert_eq!(t.save(full()).await.0, 200);
    assert_eq!(pairing(&t).await, "https://askbar.orraitu.com:8443");
    t.state.set_tls_ports(9000, 9001);
    assert_eq!(pairing(&t).await, "https://askbar.orraitu.com:9000");
}

#[tokio::test]
async fn network_needs_a_session_and_a_master_key() {
    let t = start().await;
    assert_eq!(t.http.get(format!("{}/admin/api/network", t.adm)).send().await.unwrap().status(), 401);

    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    admin::init_password(&db, PW).unwrap();
    let state = AppState::build(db, None, "0.1.0", "http://localhost:8080", std::env::temp_dir().join("nope").join("master.key"));
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let adm = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::admin_router(state);
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let r = t.http.post(format!("{adm}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PW })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    let r = t.http.put(format!("{adm}/admin/api/network")).header("x-askbar-admin", "1").header("cookie", cookie).json(&full()).send().await.unwrap();
    assert_eq!(r.status(), 503);
}

#[tokio::test]
async fn the_backup_leaves_out_the_token_but_keeps_the_rest() {
    let t = start().await;
    t.save(full()).await;
    let r = t.http.get(format!("{}/admin/api/backup", t.adm)).header("cookie", &t.cookie).send().await.unwrap();
    let bytes = r.bytes().await.unwrap();
    let stored = t.stored_token().unwrap();
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(!has(stored.as_bytes()) && !has(TOKEN.as_bytes()) && !has(b"cloudflare_token"));
    assert!(has(b"askbar.orraitu.com"), "non-secret network settings are in the backup");
}
