use askbar_proto::{ChatRequest, Settings};
use askbar_server::{admin, auth, secrets::Secrets, AppState};
use serde_json::json;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PASSWORD: &str = "correct horse battery";

struct Mock {
    child: Child,
    port: u16,
    log: Arc<Mutex<Vec<String>>>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_mock() -> Mock {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
    let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let out = BufReader::new(child.stdout.take().unwrap());
    let l = log.clone();
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            l.lock().unwrap().push(line.clone());
            let _ = tx.send(());
        }
    });
    rx.recv_timeout(Duration::from_secs(10)).expect("mock did not start");
    Mock { child, port, log }
}

struct T {
    api: String,
    admin: String,
    cookie: String,
    token: String,
    http: reqwest::Client,
    mock: Mock,
}

async fn serve(app: axum::Router) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    addr
}

async fn start() -> T {
    let mock = start_mock();
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    admin::init_password(&db, PASSWORD).unwrap();
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, type, base_url, model, created_at) VALUES ('p','mock','openai',?1,'m',0)",
            [format!("http://127.0.0.1:{}/v1", mock.port)],
        )
        .unwrap();
    let code = auth::create_pair_code(&db, &user, "t", 600).unwrap();
    let state = AppState::new(db, Secrets::new("0123456789abcdef0123456789abcdef").unwrap());
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let (api, admin_url) = (serve(askbar_server::api_router(state.clone())).await, serve(askbar_server::admin_router(state)).await);
    let http = reqwest::Client::new();
    let r = http.post(format!("{admin_url}/admin/login")).header("x-askbar-admin", "1").json(&json!({ "password": PASSWORD })).send().await.unwrap();
    let cookie = r.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_string();
    T { api, admin: admin_url, cookie, token, http, mock }
}

impl T {
    async fn admin(&self, method: reqwest::Method, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
        let mut r = self.http.request(method, format!("{}/admin/api/settings", self.admin)).header("x-askbar-admin", "1").header("cookie", &self.cookie);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let st = resp.status().as_u16();
        (st, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(json!(null)))
    }
    async fn client_settings(&self) -> Settings {
        self.http.get(format!("{}/api/v1/settings", self.api)).bearer_auth(&self.token).header("x-askbar-client-version", "0.1.0").send().await.unwrap().json().await.unwrap()
    }
    async fn chat(&self, text: &str) {
        let r = self
            .http
            .post(format!("{}/api/v1/chat", self.api))
            .bearer_auth(&self.token)
            .header("x-askbar-client-version", "0.1.0")
            .json(&ChatRequest { conversation_id: None, provider: "mock".into(), text: text.into() })
            .send()
            .await
            .unwrap();
        r.text().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn saved_settings_reach_clients_and_the_next_message() {
    let t = start().await;

    // Defaults first: the built-in prompt, default display.
    let (st, s) = t.admin(reqwest::Method::GET, None).await;
    assert_eq!(st, 200);
    assert!(s["system_prompt"].as_str().unwrap().contains("quick-question popup"));
    assert_eq!(t.client_settings().await, Settings::default());

    let new = json!({
        "system_prompt": "You are a pirate. Say arr.",
        "display": { "theme": "dark", "width": 900, "max_height": 500, "history_width": 300, "top_percent": 0.3, "hide_on_blur": false }
    });
    assert_eq!(t.admin(reqwest::Method::PUT, Some(new)).await.0, 204);

    let seen = t.client_settings().await;
    assert_eq!((seen.display.theme.as_str(), seen.display.width, seen.display.max_height), ("dark", 900, 500));
    assert_eq!((seen.display.history_width, seen.display.hide_on_blur), (300, false));
    let body = serde_json::to_string(&seen).unwrap();
    assert!(!body.contains("pirate"), "the system prompt is never served to clients");

    t.chat("hello").await;
    let log = t.mock.log.lock().unwrap().join("\n");
    assert!(log.contains("system='You are a pirate. Say arr.'"), "{log}");

    // Changing it again applies to the following message.
    let again = json!({ "system_prompt": "Answer in French.", "display": serde_json::to_value(&seen.display).unwrap() });
    assert_eq!(t.admin(reqwest::Method::PUT, Some(again)).await.0, 204);
    t.chat("second").await;
    assert!(t.mock.log.lock().unwrap().iter().any(|l| l.contains("system='Answer in French.'") && l.contains("last='second'")));
}

#[tokio::test]
async fn invalid_settings_are_refused_and_change_nothing() {
    let t = start().await;
    let ok = serde_json::to_value(Settings::default().display).unwrap();
    for (label, body) in [
        ("theme", json!({ "system_prompt": "x", "display": { "theme": "neon" } })),
        ("width", json!({ "system_prompt": "x", "display": { "width": 10 } })),
        ("top", json!({ "system_prompt": "x", "display": { "top_percent": 3.0 } })),
        ("long prompt", json!({ "system_prompt": "x".repeat(20_001), "display": ok })),
    ] {
        let (st, e) = t.admin(reqwest::Method::PUT, Some(body)).await;
        assert_eq!(st, 400, "{label}");
        assert!(e["error"].is_string());
    }
    assert_eq!(t.client_settings().await, Settings::default());
    assert!(t.admin(reqwest::Method::GET, None).await.1["system_prompt"].as_str().unwrap().contains("quick-question"));
}

#[tokio::test]
async fn settings_need_an_admin_session() {
    let t = start().await;
    let r = t.http.get(format!("{}/admin/api/settings", t.admin)).bearer_auth(&t.token).send().await.unwrap();
    assert_eq!(r.status(), 401, "a device token is not an admin session");
}
