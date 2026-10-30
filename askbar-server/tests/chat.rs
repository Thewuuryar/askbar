//! Chat streaming against the real tests/mock_llm.py.

use askbar_proto::{ChatEvent, ChatRequest};
use askbar_server::{auth, secrets::Secrets, AppState};
use futures_util::StreamExt;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    let mut child = Command::new("python3")
        .args(["-u", script, &port.to_string()])
        .stdout(Stdio::piped())
        .spawn()
        .expect("python3 is needed for these tests");
    let log = Arc::new(Mutex::new(Vec::new()));
    let out = BufReader::new(child.stdout.take().unwrap());
    let l = log.clone();
    std::thread::spawn(move || {
        for line in out.lines().map_while(Result::ok) {
            l.lock().unwrap().push(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "mock did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    Mock { child, port, log }
}

struct T {
    addr: SocketAddr,
    state: Arc<AppState>,
    token: String,
    mock: Mock,
}

async fn start() -> T {
    let mock = start_mock();
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    let secrets = Secrets::new("0123456789abcdef0123456789abcdef").unwrap();
    db.conn()
        .execute(
            "INSERT INTO providers (id, name, type, base_url, model, api_key_enc, created_at) VALUES ('p1','mock','openai',?1,'m',?2,0)",
            rusqlite::params![format!("http://127.0.0.1:{}/v1", mock.port), secrets.encrypt("sk-test")],
        )
        .unwrap();
    let code = auth::create_pair_code(&db, &user, "t", 600).unwrap();
    let state = AppState::new(db, secrets);
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let app = askbar_server::api_router(state.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { addr, state, token, mock }
}

impl T {
    async fn chat(&self, client: &reqwest::Client, conv: Option<&str>, provider: &str, text: &str) -> reqwest::Response {
        client
            .post(format!("http://{}/api/v1/chat", self.addr))
            .bearer_auth(&self.token)
            .json(&ChatRequest { conversation_id: conv.map(String::from), provider: provider.into(), text: text.into() })
            .send()
            .await
            .unwrap()
    }
    fn messages(&self) -> Vec<(String, String)> {
        let db = self.state.db.lock().unwrap();
        let mut stmt = db.conn().prepare("SELECT role, content FROM messages ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        rows
    }
}

fn parse_events(body: &str) -> Vec<(String, ChatEvent)> {
    body.split("\n\n")
        .filter(|b| !b.trim().is_empty() && !b.trim_start().starts_with(':'))
        .map(|block| {
            let mut name = String::new();
            let mut data = String::new();
            for line in block.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    name = v.trim().into();
                } else if let Some(v) = line.strip_prefix("data:") {
                    data.push_str(v.trim_start());
                }
            }
            (name, serde_json::from_str(&data).unwrap())
        })
        .collect()
}

fn wait_for<T>(what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn complete_reply_is_streamed_and_saved_in_full() {
    let t = start().await;
    let http = client();
    let r = t.chat(&http, None, "mock", "Hello there").await;
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let events = parse_events(&r.text().await.unwrap().replace("\r\n", "\n"));

    // Our own event names and shapes, in order: meta, deltas..., done.
    assert!(matches!(&events[0], (n, ChatEvent::Meta { .. }) if n == "meta"));
    assert!(matches!(events.last().unwrap(), (n, ChatEvent::Done) if n == "done"));
    let mut reply = String::new();
    for (name, e) in &events[1..events.len() - 1] {
        let ChatEvent::Delta { text } = e else { panic!("unexpected {e:?}") };
        assert_eq!(name, "delta");
        reply.push_str(text);
    }
    assert!(reply.ends_with("Anything else?"), "got {reply:?}");

    let ChatEvent::Meta { conversation_id } = &events[0].1 else { unreachable!() };
    let saved = t.messages();
    assert_eq!(saved, [("user".to_string(), "Hello there".to_string()), ("assistant".to_string(), reply.clone())]);

    // The new conversation is visible, with a ULID id, and continues with history.
    let r = t.chat(&http, Some(conversation_id), "mock", "And again").await;
    assert_eq!(r.status(), 200);
    r.text().await.unwrap();
    assert_eq!(t.messages().len(), 4);
    assert!(t.mock.log.lock().unwrap().iter().any(|l| l.contains("msgs=4")), "second request carries the history plus the system prompt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_disconnect_saves_partial_reply_and_closes_upstream() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "mock", "please be slow").await;
    assert_eq!(r.status(), 200);
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while !got.contains("event: delta") || got.matches("event: delta").count() < 3 {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    drop(stream); // the client goes away mid-stream
    drop(http);

    let partial = wait_for("partial reply to be saved", || t.messages().into_iter().find(|(r, _)| r == "assistant"));
    assert!(!partial.1.trim().is_empty());
    assert!(partial.1.len() < 1000, "should not have waited for the whole slow reply ({} bytes)", partial.1.len());
    wait_for("upstream to see the disconnect", || {
        t.mock.log.lock().unwrap().iter().any(|l| l.contains("client disconnected")).then_some(())
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_failure_becomes_an_error_event_and_keeps_the_question() {
    let t = start().await;
    let http = client();
    let r = t.chat(&http, None, "mock", "please fail").await;
    assert_eq!(r.status(), 200);
    let events = parse_events(&r.text().await.unwrap().replace("\r\n", "\n"));
    let (name, ChatEvent::Error { message }) = events.last().unwrap() else { panic!("{events:?}") };
    assert_eq!(name, "error");
    assert!(message.contains("401"));
    assert!(!message.contains("sk-test"));
    assert_eq!(t.messages(), [("user".to_string(), "please fail".to_string())]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_requests_are_plain_http_errors() {
    let t = start().await;
    let http = client();
    assert_eq!(t.chat(&http, None, "nope", "hi").await.status(), 404);
    assert_eq!(t.chat(&http, Some("01NOSUCH"), "mock", "hi").await.status(), 404);
    assert_eq!(t.chat(&http, None, "mock", "   ").await.status(), 400);
    assert!(t.messages().is_empty());
    let r = http
        .post(format!("http://{}/api/v1/chat", t.addr))
        .json(&ChatRequest { conversation_id: None, provider: "mock".into(), text: "hi".into() })
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

fn client_builder() -> reqwest::ClientBuilder {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.1.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h)
}

fn client() -> reqwest::Client {
    client_builder().build().unwrap()
}
