use askbar_server::{admin, secrets::Secrets, store, AppState};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

const PASSWORD: &str = "correct horse battery";
const KEY: &str = "sk-super-secret-key-123";

struct Mock(Child, u16);
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_mock() -> Mock {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
    let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let out = BufReader::new(child.stdout.take().unwrap());
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    rx.recv_timeout(Duration::from_secs(10)).expect("mock did not start");
    Mock(child, port)
}

struct T {
    addr: String,
    state: Arc<AppState>,
    http: reqwest::Client,
    cookie: String,
}

async fn start() -> T {
    let db = askbar_server::Db::open_in_memory().unwrap();
    db.seed_server().unwrap();
    admin::init_password(&db, PASSWORD).unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    let app = askbar_server::admin_router(state.clone());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    let http = reqwest::Client::new();
    let r = http.post(format!("{addr}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PASSWORD })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { addr, state, http, cookie }
}

impl T {
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut r = self.http.request(method, format!("{}{path}", self.addr)).header("x-askbar-admin", "1").header("cookie", &self.cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }
    fn stored_key(&self, name: &str) -> String {
        let db = self.state.db.lock().unwrap();
        let id: String = db.conn().query_row("SELECT id FROM providers WHERE name = ?1", [name], |r| r.get(0)).unwrap();
        store::load_provider(&db, &self.state.secrets().unwrap(), &id).unwrap().unwrap().api_key
    }
}

fn provider(name: &str, base: &str, key: Option<&str>) -> Value {
    let mut v = json!({ "name": name, "type": "openai", "base_url": base, "model": "mock-model", "max_tokens": 100 });
    if let Some(k) = key {
        v["api_key"] = json!(k);
    }
    v
}

use reqwest::Method as M;

#[tokio::test]
async fn crud_never_returns_the_key_and_edit_keeps_or_replaces_it() {
    let t = start().await;
    let (st, created) = t.call(M::POST, "/admin/api/providers", Some(provider("mock", "http://x/v1", Some(KEY)))).await;
    assert_eq!(st, 201);
    let id = created["id"].as_str().unwrap().to_string();

    let (_, list) = t.call(M::GET, "/admin/api/providers", None).await;
    let text = list.to_string();
    assert!(!text.contains(KEY) && !text.contains("api_key"), "{text}");
    assert_eq!(list[0]["has_key"], true);
    assert_eq!((list[0]["name"].as_str(), list[0]["model"].as_str(), list[0]["max_tokens"].as_u64()), (Some("mock"), Some("mock-model"), Some(100)));
    assert_eq!(t.stored_key("mock"), KEY);
    // At rest it is encrypted.
    let raw: Vec<u8> = t.state.db.lock().unwrap().conn().query_row("SELECT api_key_enc FROM providers", [], |r| r.get(0)).unwrap();
    assert!(!raw.windows(KEY.len()).any(|w| w == KEY.as_bytes()));

    // Editing without a key (or with an empty one) keeps the stored key.
    let mut edit = provider("mock2", "http://y/v1", None);
    assert_eq!(t.call(M::PUT, &format!("/admin/api/providers/{id}"), Some(edit.clone())).await.0, 204);
    edit["api_key"] = json!("");
    assert_eq!(t.call(M::PUT, &format!("/admin/api/providers/{id}"), Some(edit.clone())).await.0, 204);
    assert_eq!(t.stored_key("mock2"), KEY);
    // A non-empty key replaces it.
    edit["api_key"] = json!("sk-new");
    assert_eq!(t.call(M::PUT, &format!("/admin/api/providers/{id}"), Some(edit)).await.0, 204);
    assert_eq!(t.stored_key("mock2"), "sk-new");

    assert_eq!(t.call(M::DELETE, &format!("/admin/api/providers/{id}"), None).await.0, 204);
    assert_eq!(t.call(M::DELETE, &format!("/admin/api/providers/{id}"), None).await.0, 404);
    assert_eq!(t.call(M::GET, "/admin/api/providers", None).await.1, json!([]));
}

#[tokio::test]
async fn validation_and_conflicts() {
    let t = start().await;
    assert_eq!(t.call(M::POST, "/admin/api/providers", Some(provider("a", "http://x/v1", None))).await.0, 201);
    assert_eq!(t.call(M::POST, "/admin/api/providers", Some(provider("a", "http://x/v1", None))).await.0, 409, "duplicate name");
    for bad in [
        json!({ "name": "", "type": "openai", "model": "m" }),
        json!({ "name": "b", "type": "gemini", "model": "m" }),
        json!({ "name": "b", "type": "openai", "model": " " }),
        json!({ "name": "b", "type": "openai", "model": "m", "base_url": "ftp://x" }),
    ] {
        let (st, body) = t.call(M::POST, "/admin/api/providers", Some(bad)).await;
        assert_eq!(st, 400);
        assert!(body["error"].is_string());
    }
    assert_eq!(t.call(M::PUT, "/admin/api/providers/nope", Some(provider("z", "", None))).await.0, 404);
}

#[tokio::test]
async fn providers_require_a_session() {
    let t = start().await;
    let r = t.http.get(format!("{}/admin/api/providers", t.addr)).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = t.http.post(format!("{}/admin/api/providers", t.addr)).header("cookie", &t.cookie).json(&provider("x", "", None)).send().await.unwrap();
    assert_eq!(r.status(), 403, "no admin header");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_button_reports_success_and_failure_without_leaking_the_key() {
    let t = start().await;
    let mock = start_mock();
    let (_, c) = t.call(M::POST, "/admin/api/providers", Some(provider("good", &format!("http://127.0.0.1:{}/v1", mock.1), Some(KEY)))).await;
    let (st, r) = t.call(M::POST, &format!("/admin/api/providers/{}/test", c["id"].as_str().unwrap()), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["ok"], true, "{r}");
    assert!(!r["message"].as_str().unwrap().is_empty());

    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let (_, c) = t.call(M::POST, "/admin/api/providers", Some(provider("dead", &format!("http://127.0.0.1:{dead}/v1"), Some(KEY)))).await;
    let (st, r) = t.call(M::POST, &format!("/admin/api/providers/{}/test", c["id"].as_str().unwrap()), None).await;
    assert_eq!(st, 200);
    assert_eq!(r["ok"], false);
    assert!(r["message"].as_str().unwrap().contains("could not reach"), "{r}");
    assert!(!r.to_string().contains(KEY));
    assert_eq!(t.call(M::POST, "/admin/api/providers/nope/test", None).await.0, 404);
}

#[tokio::test]
async fn the_page_is_served_with_a_locked_down_policy() {
    let t = start().await;
    for (path, ctype) in [("/admin/", "text/html"), ("/admin/app.js", "text/javascript"), ("/admin/style.css", "text/css")] {
        let r = t.http.get(format!("{}{path}", t.addr)).send().await.unwrap();
        assert_eq!(r.status(), 200, "{path}");
        assert!(r.headers()["content-type"].to_str().unwrap().starts_with(ctype));
        assert!(r.headers()["content-security-policy"].to_str().unwrap().contains("default-src 'none'"));
        assert_eq!(r.headers()["x-frame-options"], "DENY");
    }
    let html = t.http.get(format!("{}/admin/", t.addr)).send().await.unwrap().text().await.unwrap();
    assert!(!html.contains("<style") && !html.contains(" style=") && !html.contains("onclick"), "CSP forbids inline code and styles");
    assert!(html.contains("CLIProxyAPI"));
    assert!(html.contains("https://github.com/Thewuuryar/askbar"), "AGPL section 13: the page links to the source");
    let js = t.http.get(format!("{}/admin/app.js", t.addr)).send().await.unwrap().text().await.unwrap();
    assert!(js.contains("http://cpa:8317/v1"));
}
