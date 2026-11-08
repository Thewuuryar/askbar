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
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
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
    askbar_server::testing::add_provider(
        &db,
        askbar_server::testing::provider("p1", "mock", &format!("http://127.0.0.1:{}/v1", mock.port), "m"),
    );
    askbar_server::testing::store_key(&db, &secrets, "p1", "sk-test");
    let code = askbar_server::testing::add_pair_code(&db, &user, "t");
    let state = AppState::new(db, secrets, askbar_server::MIN_CLIENT);
    let token = auth::redeem_pair_code(&state.db.lock().unwrap(), &code).unwrap().unwrap();
    let app = askbar_server::client_router(state.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    T { addr, state, token, mock }
}

impl T {
    async fn chat(&self, client: &reqwest::Client, conv: Option<&str>, provider: &str, text: &str) -> reqwest::Response {
        client
            .post(format!("http://{}/api/client/v1/chat", self.addr))
            .bearer_auth(&self.token)
            .json(&ChatRequest {
                conversation_id: conv.map(String::from),
                provider: provider.into(),
                text: text.into(),
                client_request_id: None,
            })
            .send()
            .await
            .unwrap()
    }
    /// (status, text, error, assistant_message_id) of every generation, oldest first.
    fn generations(&self) -> Vec<(String, String, Option<String>, Option<String>)> {
        let db = self.state.db.lock().unwrap();
        let mut stmt = db.conn().prepare("SELECT status, text, error, assistant_message_id FROM generations ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect();
        rows
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

fn wait_for<T>(what: &str, f: impl FnMut() -> Option<T>) -> T {
    wait_for_secs(10, what, f)
}

fn wait_for_secs<T>(secs: u64, what: &str, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
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
    let r = t.chat(&http, None, "p1", "Hello there").await;
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
    let events = parse_events(&r.text().await.unwrap().replace("\r\n", "\n"));

    // Our own event names and shapes, in order: meta, provider, deltas..., done.
    assert!(matches!(&events[0], (n, ChatEvent::Meta { .. }) if n == "meta"));
    assert!(
        matches!(&events[1], (n, ChatEvent::Provider { id, name }) if n == "provider" && id == "p1" && name == "mock"),
        "{:?}",
        events[1]
    );
    assert!(matches!(events.last().unwrap(), (n, ChatEvent::Done) if n == "done"));
    let mut reply = String::new();
    for (name, e) in &events[2..events.len() - 1] {
        let ChatEvent::Delta { text } = e else { panic!("unexpected {e:?}") };
        assert_eq!(name, "delta");
        reply.push_str(text);
    }
    assert!(reply.ends_with("Anything else?"), "got {reply:?}");

    let ChatEvent::Meta { conversation_id, .. } = &events[0].1 else { unreachable!() };
    let saved = t.messages();
    assert_eq!(saved, [("user".to_string(), "Hello there".to_string()), ("assistant".to_string(), reply.clone())]);

    // The new conversation is visible, with a ULID id, and continues with history.
    let r = t.chat(&http, Some(conversation_id), "p1", "And again").await;
    assert_eq!(r.status(), 200);
    r.text().await.unwrap();
    assert_eq!(t.messages().len(), 4);
    assert!(t.mock.log.lock().unwrap().iter().any(|l| l.contains("msgs=4")), "second request carries the history plus the system prompt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_disconnect_does_not_cut_the_reply_short() {
    // Contract changed in 1.5: the reply no longer stops (or is saved as a partial) when the
    // client leaves; it keeps going and is saved once, whole. See the test below for the full text.
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    assert_eq!(r.status(), 200);
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while !got.contains("event: delta") || got.matches("event: delta").count() < 3 {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    drop(stream); // the client goes away mid-stream
    drop(http);

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(t.messages().iter().all(|(r, _)| r != "assistant"), "no partial reply is saved while it still runs");
    assert!(!t.mock.log.lock().unwrap().iter().any(|l| l.contains("client disconnected")), "the upstream request stays open");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reply_keeps_generating_after_the_client_disconnects() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    assert_eq!(r.status(), 200);
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while !got.contains("event: delta") {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    drop(stream); // the client goes away after the first delta
    drop(http);

    let full = "Streaming slowly so you can press Stop. ".repeat(40);
    // The mock takes about a minute to send the whole slow reply.
    let saved =
        wait_for_secs(120, "the full slow reply to be saved", || t.messages().into_iter().find(|(r, c)| r == "assistant" && c == &full));
    assert_eq!(saved.1, full);
    assert!(
        !t.mock.log.lock().unwrap().iter().any(|l| l.contains("client disconnected")),
        "the upstream request must not be closed when the client leaves"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn provider_failure_becomes_an_error_event_and_keeps_the_question() {
    let t = start().await;
    let http = client();
    let r = t.chat(&http, None, "p1", "please fail").await;
    assert_eq!(r.status(), 200);
    let events = parse_events(&r.text().await.unwrap().replace("\r\n", "\n"));
    let (name, ChatEvent::Error { message, code }) = events.last().unwrap() else { panic!("{events:?}") };
    assert_eq!(name, "error");
    assert_eq!(code.as_deref(), Some("provider_error"), "a program branches on the code, not the words");
    assert!(message.contains("401"));
    assert!(!message.contains("sk-test"));
    assert_eq!(t.messages(), [("user".to_string(), "please fail".to_string())]);
}

/// A question that got no reply is not sent again with the next one: a question the provider refuses
/// must not stick the conversation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_question_is_not_sent_again_with_the_next_one() {
    let t = start().await;
    let http = client();
    let events = parse_events(&t.chat(&http, None, "p1", "hello").await.text().await.unwrap().replace("\r\n", "\n"));
    let ChatEvent::Meta { conversation_id, .. } = events[0].1.clone() else { panic!("first event must be meta") };
    let failed =
        parse_events(&t.chat(&http, Some(&conversation_id), "p1", "please fail").await.text().await.unwrap().replace("\r\n", "\n"));
    assert_eq!(failed.last().unwrap().0, "error");
    let next = parse_events(&t.chat(&http, Some(&conversation_id), "p1", "hello again").await.text().await.unwrap().replace("\r\n", "\n"));
    assert_eq!(next.last().unwrap().0, "done", "{next:?}");
    let posts: Vec<String> = t.mock.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).cloned().collect();
    let last = posts.last().unwrap();
    assert!(last.contains("last='hello again'"), "the failed question is left out: {last}");
    // The person still sees every question.
    let users: Vec<String> = t.messages().into_iter().filter(|(r, _)| r == "user").map(|(_, c)| c).collect();
    assert_eq!(users, ["hello", "please fail", "hello again"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bad_requests_are_plain_http_errors() {
    let t = start().await;
    let http = client();
    assert_eq!(t.chat(&http, None, "nope", "hi").await.status(), 404);
    assert_eq!(t.chat(&http, Some("01NOSUCH"), "p1", "hi").await.status(), 404);
    assert_eq!(t.chat(&http, None, "p1", "   ").await.status(), 400);
    // Providers are addressed by id; the name "mock" is not accepted in place of one.
    assert_eq!(t.chat(&http, None, "mock", "hi").await.status(), 404);
    assert!(t.messages().is_empty());
    let r = http
        .post(format!("http://{}/api/client/v1/chat", t.addr))
        .json(&ChatRequest { conversation_id: None, provider: "p1".into(), text: "hi".into(), client_request_id: None })
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
}

fn client_builder() -> reqwest::ClientBuilder {
    let mut h = reqwest::header::HeaderMap::new();
    h.insert(askbar_proto::CLIENT_VERSION_HEADER, "0.5.0".parse().unwrap());
    reqwest::Client::builder().default_headers(h)
}

fn client() -> reqwest::Client {
    client_builder().build().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_user_cannot_read_or_continue_my_conversation() {
    let t = start().await;
    let http = client();
    let r = t.chat(&http, None, "p1", "My private question").await;
    assert_eq!(r.status(), 200);
    let body = r.text().await.unwrap();
    let events = parse_events(&body);
    let ChatEvent::Meta { conversation_id, .. } = events[0].1.clone() else { panic!("first event must be meta") };
    wait_for("the reply to be saved", || (t.messages().len() == 2).then_some(()));

    // A second user with their own client.
    let token_b = {
        let db = t.state.db.lock().unwrap();
        db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
        let code = askbar_server::testing::add_pair_code(&db, "B", "b-laptop");
        auth::redeem_pair_code(&db, &code).unwrap().unwrap()
    };
    let get = |path: String| {
        let (http, token, addr) = (http.clone(), token_b.clone(), t.addr);
        async move { http.get(format!("http://{addr}{path}")).bearer_auth(token).send().await.unwrap() }
    };
    let list = get("/api/client/v1/conversations".into()).await;
    assert_eq!(list.status(), 200);
    assert_eq!(list.text().await.unwrap(), "[]");
    assert_eq!(get(format!("/api/client/v1/conversations/{conversation_id}")).await.status(), 404);

    // Continuing it is refused and writes nothing.
    let r = http
        .post(format!("http://{}/api/client/v1/chat", t.addr))
        .bearer_auth(&token_b)
        .json(&ChatRequest {
            conversation_id: Some(conversation_id.clone()),
            provider: "p1".into(),
            text: "intrusion".into(),
            client_request_id: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(t.messages().len(), 2);

    // Deleting it is refused too.
    let r =
        http.delete(format!("http://{}/api/client/v1/conversations/{conversation_id}", t.addr)).bearer_auth(&token_b).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let own =
        http.get(format!("http://{}/api/client/v1/conversations/{conversation_id}", t.addr)).bearer_auth(&t.token).send().await.unwrap();
    assert_eq!(own.status(), 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_snapshots_system_prompt_is_sent_with_the_next_message_and_never_served() {
    let t = start().await;
    let http = client();
    // Built-in prompt until the admin pushes one.
    t.chat(&http, None, "p1", "hello").await.text().await.unwrap();
    wait_for("the first reply", || t.mock.log.lock().unwrap().iter().any(|l| l.contains("last='hello'")).then_some(()));
    let log = t.mock.log.lock().unwrap().join("\n");
    assert!(log.contains("quick-question popup"), "{log}");

    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.system_prompt = Some("You are a pirate. Say arr.".into()));
    t.chat(&http, None, "p1", "second").await.text().await.unwrap();
    wait_for("the second reply", || {
        t.mock
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("system='You are a pirate. Say arr.'") && l.contains("last='second'"))
            .then_some(())
    });

    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.system_prompt = Some("Answer in French.".into()));
    t.chat(&http, None, "p1", "third").await.text().await.unwrap();
    wait_for("the third reply", || {
        t.mock.log.lock().unwrap().iter().any(|l| l.contains("system='Answer in French.'") && l.contains("last='third'")).then_some(())
    });

    // Clients can read the display settings, but the prompt is never among them.
    let settings = http
        .get(format!("http://{}/api/client/v1/settings", t.addr))
        .bearer_auth(&t.token)
        .header("x-askbar-client-version", "0.5.0")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!settings.contains("French") && !settings.contains("pirate"), "{settings}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_generation_row_holds_the_full_text_and_the_message_id() {
    let t = start().await;
    let http = client();
    let r = t.chat(&http, None, "p1", "Hello there").await;
    r.text().await.unwrap();
    let rows = t.generations();
    assert_eq!(rows.len(), 1);
    let (status, text, error, message_id) = &rows[0];
    assert_eq!(status, "done");
    assert!(text.ends_with("Anything else?"), "got {text:?}");
    assert_eq!(error, &None);
    let db = t.state.db.lock().unwrap();
    let id: String = db.conn().query_row("SELECT id FROM messages WHERE role = 'assistant'", [], |r| r.get(0)).unwrap();
    assert_eq!(message_id.as_deref(), Some(id.as_str()));
    let (provider, user_msg): (String, Option<String>) =
        db.conn().query_row("SELECT provider_id, user_message_id FROM generations", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!(provider, "p1");
    assert!(user_msg.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_generation_row_is_an_error_row() {
    let t = start().await;
    let http = client();
    t.chat(&http, None, "p1", "please fail").await.text().await.unwrap();
    let rows = t.generations();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "error");
    assert!(rows[0].2.as_deref().unwrap_or("").contains("Incorrect API key"), "got {:?}", rows[0].2);
    assert_eq!(rows[0].3, None, "no reply text, so no assistant message");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn progress_is_flushed_to_the_row_while_the_reply_runs() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while got.matches("event: delta").count() < 2 {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    // The row says running, and its text catches up within a flush interval or two.
    let text = wait_for("text to be flushed", || {
        let rows = t.generations();
        (rows[0].0 == "running" && !rows[0].1.is_empty()).then(|| rows[0].1.clone())
    });
    assert!(text.starts_with("Streaming"), "got {text:?}");
    assert!(t.messages().iter().all(|(r, _)| r != "assistant"), "the message is written once, at the end");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meta_carries_the_generation_id_and_deltas_carry_ascending_ids() {
    let t = start().await;
    let body = t.chat(&client(), None, "p1", "Hello there").await.text().await.unwrap().replace("\r\n", "\n");
    let events = parse_events(&body);
    let ChatEvent::Meta { generation_id, .. } = &events[0].1 else { panic!("first event must be meta") };
    let generation_id = generation_id.clone().expect("meta names the generation");
    let stored: String = t.state.db.lock().unwrap().conn().query_row("SELECT id FROM generations", [], |r| r.get(0)).unwrap();
    assert_eq!(generation_id, stored);

    // Every delta block has `id: n`, counting from 1; other events have none.
    let mut ids = Vec::new();
    for block in body.split("\n\n") {
        let id = block.lines().find_map(|l| l.strip_prefix("id:")).map(|v| v.trim().parse::<u64>().unwrap());
        if block.contains("event: delta") {
            ids.push(id.expect("a delta has an id"));
        } else {
            assert_eq!(id, None, "only deltas carry ids: {block:?}");
        }
    }
    assert!(ids.len() > 3);
    assert_eq!(ids, (1..=ids.len() as u64).collect::<Vec<_>>());
}

impl T {
    fn second_user_token(&self) -> String {
        let db = self.state.db.lock().unwrap();
        db.conn().execute("INSERT INTO users (id, name, created_at) VALUES ('B','other',0)", []).unwrap();
        let code = askbar_server::testing::add_pair_code(&db, "B", "b-laptop");
        auth::redeem_pair_code(&db, &code).unwrap().unwrap()
    }
    async fn get_with(&self, token: &str, path: &str) -> reqwest::Response {
        client().get(format!("http://{}{path}", self.addr)).bearer_auth(token).send().await.unwrap()
    }
}

/// The (id, text) of every delta in an SSE body, in order.
fn deltas(body: &str) -> Vec<(u64, String)> {
    let body = body.replace("\r\n", "\n");
    body.split("\n\n")
        .filter(|b| b.contains("event: delta"))
        .map(|b| {
            let id = b.lines().find_map(|l| l.strip_prefix("id:")).unwrap().trim().parse().unwrap();
            let data = b.lines().find_map(|l| l.strip_prefix("data:")).unwrap().trim_start();
            let ChatEvent::Delta { text } = serde_json::from_str(data).unwrap() else { panic!() };
            (id, text)
        })
        .collect()
}

fn generation_id(body: &str) -> String {
    let events = parse_events(&body.replace("\r\n", "\n"));
    let ChatEvent::Meta { generation_id, .. } = &events[0].1 else { panic!("first event must be meta") };
    generation_id.clone().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resuming_after_three_deltas_sends_the_rest_exactly_once() {
    let t = start().await;
    let body = t.chat(&client(), None, "p1", "Hello there").await.text().await.unwrap();
    let all = deltas(&body);
    assert!(all.len() > 5);
    let id = generation_id(&body);

    let rest = t.get_with(&t.token, &format!("/api/client/v1/generations/{id}/stream?after=3")).await;
    assert_eq!(rest.status(), 200);
    let rest_body = rest.text().await.unwrap();
    assert_eq!(deltas(&rest_body), all[3..].to_vec(), "deltas 4.. exactly once, with their original ids");
    assert!(rest_body.contains("event: done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resuming_a_running_generation_replays_then_follows() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while got.matches("event: delta").count() < 4 {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    let id = generation_id(&got);
    drop(stream);

    let resumed = t.get_with(&t.token, &format!("/api/client/v1/generations/{id}/stream?after=2")).await;
    let mut s = resumed.bytes_stream();
    let mut body = String::new();
    while deltas(&body).len() < 4 {
        body.push_str(&String::from_utf8_lossy(&s.next().await.unwrap().unwrap()));
    }
    let seen: Vec<u64> = deltas(&body).iter().map(|d| d.0).collect();
    assert_eq!(&seen[..4], &[3, 4, 5, 6], "starts right after id 2, then follows live");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_only_the_database_remembers_sends_its_text_once_then_the_end() {
    let t = start().await;
    let body = t.chat(&client(), None, "p1", "Hello there").await.text().await.unwrap();
    let id = generation_id(&body);
    let text: String = deltas(&body).into_iter().map(|d| d.1).collect();
    t.state.generations.remove(&id); // as after a restart

    let whole = t.get_with(&t.token, &format!("/api/client/v1/generations/{id}/stream")).await.text().await.unwrap();
    assert_eq!(deltas(&whole), [(1, text)]);
    assert!(whole.contains("event: done"));

    let tail = t.get_with(&t.token, &format!("/api/client/v1/generations/{id}/stream?after=5")).await.text().await.unwrap();
    assert!(deltas(&tail).is_empty() && tail.contains("event: done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn another_users_generation_stream_is_404() {
    let t = start().await;
    let body = t.chat(&client(), None, "p1", "Hello there").await.text().await.unwrap();
    let id = generation_id(&body);
    let other = t.second_user_token();
    assert_eq!(t.get_with(&other, &format!("/api/client/v1/generations/{id}/stream")).await.status(), 404);
    assert_eq!(t.get_with(&other, "/api/client/v1/generations/nope/stream").await.status(), 404);
}

impl T {
    async fn cancel_with(&self, token: &str, id: &str) -> reqwest::Response {
        client().post(format!("http://{}/api/client/v1/generations/{id}/cancel", self.addr)).bearer_auth(token).send().await.unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_stops_the_upstream_keeps_the_partial_text_and_is_repeatable() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while got.matches("event: delta").count() < 3 {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    let id = generation_id(&got);

    assert_eq!(t.cancel_with(&t.token, &id).await.status(), 204);
    wait_for_secs(5, "the upstream to see the stop", || {
        t.mock.log.lock().unwrap().iter().any(|l| l.contains("client disconnected")).then_some(())
    });
    let partial = wait_for("the partial reply to be saved", || t.messages().into_iter().find(|(r, _)| r == "assistant"));
    assert!(partial.1.starts_with("Streaming") && partial.1.len() < 1000, "got {} bytes", partial.1.len());
    let rows = t.generations();
    assert_eq!((rows[0].0.as_str(), rows[0].1.as_str()), ("cancelled", partial.1.as_str()));
    assert!(rows[0].3.is_some());

    // The attached stream ends cleanly, and a second cancel is still 204.
    let mut rest = String::new();
    while let Some(Ok(b)) = stream.next().await {
        rest.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(rest.contains("event: done"), "got {rest:?}");
    assert_eq!(t.cancel_with(&t.token, &id).await.status(), 204);
    assert_eq!(t.generations()[0].0, "cancelled", "a repeat changes nothing");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_finished_generation_is_204_and_another_users_is_404() {
    let t = start().await;
    let body = t.chat(&client(), None, "p1", "Hello there").await.text().await.unwrap();
    let id = generation_id(&body);
    assert_eq!(t.cancel_with(&t.token, &id).await.status(), 204);
    assert_eq!(t.generations()[0].0, "done", "cancel after the end does not rewrite it");
    let other = t.second_user_token();
    assert_eq!(t.cancel_with(&other, &id).await.status(), 404);
    assert_eq!(t.cancel_with(&t.token, "no-such-id").await.status(), 404);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_generation_is_listed_and_a_finished_one_is_not() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    let mut stream = r.bytes_stream();
    let mut got = String::new();
    while got.matches("event: delta").count() < 2 {
        got.push_str(&String::from_utf8_lossy(&stream.next().await.unwrap().unwrap()));
    }
    let id = generation_id(&got);
    let conversation = {
        let events = parse_events(&got.replace("\r\n", "\n").split("\n\n").take(1).collect::<Vec<_>>().join("\n\n"));
        let ChatEvent::Meta { conversation_id, .. } = events[0].1.clone() else { panic!() };
        conversation_id
    };

    let list: Vec<askbar_proto::PendingGeneration> =
        t.get_with(&t.token, "/api/client/v1/generations?status=running").await.json().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        (list[0].generation_id.as_str(), list[0].status.as_str(), list[0].prompt.as_str()),
        (id.as_str(), "running", "please be slow")
    );
    let detail: askbar_proto::ConversationDetail =
        t.get_with(&t.token, &format!("/api/client/v1/conversations/{conversation}")).await.json().await.unwrap();
    assert_eq!(detail.pending.len(), 1);
    assert_eq!(detail.pending[0].generation_id, id);

    // Another user sees neither.
    let other = t.second_user_token();
    let theirs: Vec<askbar_proto::PendingGeneration> =
        t.get_with(&other, "/api/client/v1/generations?status=running").await.json().await.unwrap();
    assert!(theirs.is_empty());

    assert_eq!(t.cancel_with(&t.token, &id).await.status(), 204);
    wait_for("the generation to end", || (t.generations()[0].0 == "cancelled").then_some(()));
    let list: Vec<askbar_proto::PendingGeneration> =
        t.get_with(&t.token, "/api/client/v1/generations?status=running").await.json().await.unwrap();
    assert!(list.is_empty());
    let body = t.get_with(&t.token, &format!("/api/client/v1/conversations/{conversation}")).await.text().await.unwrap();
    assert!(!body.contains("pending"), "left out when nothing is pending: {body}");
    assert_eq!(t.get_with(&t.token, "/api/client/v1/generations?status=bogus").await.status(), 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_posts_with_one_client_request_id_make_one_generation_and_one_user_message() {
    let t = start().await;
    let send = |text: &'static str| {
        let (addr, token) = (t.addr, t.token.clone());
        async move {
            client()
                .post(format!("http://{addr}/api/client/v1/chat"))
                .bearer_auth(token)
                .json(&ChatRequest {
                    conversation_id: None,
                    provider: "p1".into(),
                    text: text.into(),
                    client_request_id: Some("req-1".into()),
                })
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        }
    };
    let first = send("Hello there").await;
    let second = send("Hello there").await; // a retry, after the first has finished
    assert_eq!(generation_id(&first), generation_id(&second));
    assert_eq!(deltas(&first), deltas(&second), "the retry gets the same reply");
    assert!(second.contains("event: done"));
    assert_eq!(t.generations().len(), 1);
    assert_eq!(t.messages().iter().filter(|(r, _)| r == "user").count(), 1);
    let user = t.user_id();
    assert_eq!(t.state.db.lock().unwrap().list_conversations(&user).unwrap().len(), 1, "no second conversation");
    assert_eq!(t.mock.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).count(), 1, "one upstream request");

    // The same id from another client is a different send.
    let other = t.second_user_token();
    let third = client()
        .post(format!("http://{}/api/client/v1/chat", t.addr))
        .bearer_auth(&other)
        .json(&ChatRequest {
            conversation_id: None,
            provider: "p1".into(),
            text: "Hello there".into(),
            client_request_id: Some("req-1".into()),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(third.status(), 200);
    assert_ne!(generation_id(&third.text().await.unwrap()), generation_id(&first));
}

impl T {
    fn user_id(&self) -> String {
        self.state.db.lock().unwrap().seed_server().unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_repeat_while_the_reply_runs_attaches_to_it() {
    let t = start().await;
    let post = || {
        client_builder()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap()
            .post(format!("http://{}/api/client/v1/chat", t.addr))
            .bearer_auth(&t.token)
            .json(&ChatRequest {
                conversation_id: None,
                provider: "p1".into(),
                text: "please be slow".into(),
                client_request_id: Some("r".into()),
            })
            .send()
    };
    let mut s1 = post().await.unwrap().bytes_stream();
    let mut got1 = String::new();
    while got1.matches("event: delta").count() < 2 {
        got1.push_str(&String::from_utf8_lossy(&s1.next().await.unwrap().unwrap()));
    }
    let mut s2 = post().await.unwrap().bytes_stream();
    let mut got2 = String::new();
    while got2.matches("event: delta").count() < 3 {
        got2.push_str(&String::from_utf8_lossy(&s2.next().await.unwrap().unwrap()));
    }
    assert_eq!(generation_id(&got1), generation_id(&got2));
    assert_eq!(deltas(&got2)[0].0, 1, "the attach replays from the start");
    assert_eq!(t.generations().len(), 1);
    assert_eq!(t.messages().iter().filter(|(r, _)| r == "user").count(), 1);
}

fn conversation_id_of(body: &str) -> String {
    let events = parse_events(&body.replace("\r\n", "\n").split("\n\n").take(1).collect::<Vec<_>>().join("\n\n"));
    let ChatEvent::Meta { conversation_id, .. } = events[0].1.clone() else { panic!("first event must be meta") };
    conversation_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sends_during_a_reply_are_queued_up_to_five_and_the_sixth_is_refused() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let r = t.chat(&http, None, "p1", "please be slow").await;
    let mut first = r.bytes_stream();
    let mut got = String::new();
    while got.matches("event: delta").count() < 1 {
        got.push_str(&String::from_utf8_lossy(&first.next().await.unwrap().unwrap()));
    }
    let conversation = conversation_id_of(&got);
    let first_meta: Vec<ChatEvent> =
        parse_events(&got.replace("\r\n", "\n").split("\n\n").take(1).collect::<String>()).into_iter().map(|e| e.1).collect();
    assert!(matches!(&first_meta[0], ChatEvent::Meta { queued: false, .. }), "the first send runs at once");

    let mut held = Vec::new(); // keep the queued connections open
    for n in 1..=5 {
        let r = t.chat(&http, Some(&conversation), "p1", &format!("queued {n}")).await;
        assert_eq!(r.status(), 200, "send {n} is accepted");
        let mut s = r.bytes_stream();
        let mut meta = String::new();
        while !meta.contains("\n\n") && !meta.contains("\r\n\r\n") {
            meta.push_str(&String::from_utf8_lossy(&s.next().await.unwrap().unwrap()));
        }
        let ChatEvent::Meta { queued, generation_id, .. } = parse_events(&meta.replace("\r\n", "\n")).remove(0).1 else { panic!() };
        assert!(queued, "send {n} waits");
        assert!(generation_id.is_some());
        held.push(s);
    }
    let sixth = t.chat(&http, Some(&conversation), "p1", "queued 6").await;
    assert_eq!(sixth.status(), 409);
    assert_eq!(sixth.headers()["content-type"], "application/problem+json");
    let p: askbar_proto::Problem = sixth.json().await.unwrap();
    assert_eq!(p.error, "queue_full", "the refusal says why, so a client need not guess from the status");

    // Queued questions are not messages yet; only the first question is stored.
    let users: Vec<String> = t.messages().into_iter().filter(|(r, _)| r == "user").map(|m| m.1).collect();
    assert_eq!(users, ["please be slow"]);
    let rows = t.generations();
    assert_eq!(rows.iter().filter(|r| r.0 == "queued").count(), 5);
    assert_eq!(rows.iter().filter(|r| r.0 == "running").count(), 1);
    let list: Vec<askbar_proto::PendingGeneration> =
        t.get_with(&t.token, "/api/client/v1/generations?status=running").await.json().await.unwrap();
    assert_eq!(
        list.iter().filter(|g| g.status == "queued").map(|g| g.prompt.as_str()).collect::<Vec<_>>(),
        ["queued 1", "queued 2", "queued 3", "queued 4", "queued 5"]
    );
    drop(held);
}

/// Reads until `n` deltas have arrived, returning what was read so far.
async fn read_deltas<B: AsRef<[u8]>>(s: &mut (impl futures_util::Stream<Item = reqwest::Result<B>> + Unpin), n: usize) -> String {
    let mut got = String::new();
    while got.matches("event: delta").count() < n {
        got.push_str(&String::from_utf8_lossy(s.next().await.unwrap().unwrap().as_ref()));
    }
    got
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_message_starts_when_the_reply_ends_and_sees_it_in_its_history() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let mut first = t.chat(&http, None, "p1", "please be slow").await.bytes_stream();
    let got = read_deltas(&mut first, 2).await;
    let (conversation, first_id) = (conversation_id_of(&got), generation_id(&got));
    let second = t.chat(&http, Some(&conversation), "p1", "second").await;
    let mut second = second.bytes_stream();

    assert_eq!(t.cancel_with(&t.token, &first_id).await.status(), 204); // end the first reply early
    let mut body = String::new();
    while let Some(Ok(b)) = second.next().await {
        body.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(body.contains("event: delta") && body.contains("event: done"), "the queued message runs: {body:?}");

    let stored = t.messages();
    let roles: Vec<&str> = stored.iter().map(|m| m.0.as_str()).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"], "prompt, reply, prompt, reply");
    assert_eq!(stored[0].1, "please be slow");
    assert_eq!(stored[2].1, "second");
    assert!(stored[3].1.ends_with("Anything else?"));
    // The second request carried the system prompt, the first question, the first reply and itself.
    assert!(
        t.mock.log.lock().unwrap().iter().any(|l| l.contains("last='second'") && l.contains("msgs=4")),
        "log: {:?}",
        t.mock.log.lock().unwrap()
    );
    let rows = t.generations();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["cancelled", "done"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_queued_message_leaves_no_trace_in_the_messages() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let mut first = t.chat(&http, None, "p1", "please be slow").await.bytes_stream();
    let got = read_deltas(&mut first, 2).await;
    let (conversation, first_id) = (conversation_id_of(&got), generation_id(&got));
    let mut queued = t.chat(&http, Some(&conversation), "p1", "never runs").await.bytes_stream();
    let mut meta = String::new();
    while !meta.contains("\n\n") && !meta.contains("\r\n\r\n") {
        meta.push_str(&String::from_utf8_lossy(&queued.next().await.unwrap().unwrap()));
    }
    let queued_id = generation_id(&meta);

    assert_eq!(t.cancel_with(&t.token, &queued_id).await.status(), 204);
    let mut rest = String::new();
    while let Some(Ok(b)) = queued.next().await {
        rest.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(rest.contains("event: done") && !rest.contains("event: delta"), "got {rest:?}");
    assert!(t.messages().iter().all(|m| m.1 != "never runs"), "its question was never stored");
    assert_eq!(t.cancel_with(&t.token, &queued_id).await.status(), 204, "repeatable");

    // Ending the first reply starts nothing: the queue is empty.
    assert_eq!(t.cancel_with(&t.token, &first_id).await.status(), 204);
    wait_for("the first reply to end", || (t.generations()[0].0 == "cancelled").then_some(()));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rows = t.generations();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["cancelled", "cancelled"]);
    assert!(t.messages().iter().all(|m| m.1 != "never runs"));
    assert_eq!(t.messages().len(), 2, "just the first question and its partial reply");
    assert!(!t.mock.log.lock().unwrap().iter().any(|l| l.contains("never runs")), "never sent upstream");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_conversation_mid_reply_stops_it_and_starts_nothing_queued() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let mut first = t.chat(&http, None, "p1", "please be slow").await.bytes_stream();
    let got = read_deltas(&mut first, 2).await;
    let conversation = conversation_id_of(&got);
    let mut queued = t.chat(&http, Some(&conversation), "p1", "waiting").await.bytes_stream();
    let mut meta = String::new();
    while !meta.contains("\n\n") && !meta.contains("\r\n\r\n") {
        meta.push_str(&String::from_utf8_lossy(&queued.next().await.unwrap().unwrap()));
    }

    let r =
        http.delete(format!("http://{}/api/client/v1/conversations/{conversation}", t.addr)).bearer_auth(&t.token).send().await.unwrap();
    assert_eq!(r.status(), 204);
    wait_for_secs(5, "the upstream to see the stop", || {
        t.mock.log.lock().unwrap().iter().any(|l| l.contains("client disconnected")).then_some(())
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(t.generations().is_empty(), "no generation rows left: {:?}", t.generations());
    assert!(t.messages().is_empty(), "no stray messages");
    assert_eq!(t.mock.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).count(), 1, "the queued message never started");
    let (first_id, queued_id) = (generation_id(&got), generation_id(&meta));
    for id in [first_id, queued_id] {
        assert!(t.state.generations.get(&id).is_none_or(|l| l.is_finished()), "{id} is not still running");
    }
    let list: Vec<askbar_proto::PendingGeneration> =
        t.get_with(&t.token, "/api/client/v1/generations?status=running").await.json().await.unwrap();
    assert!(list.is_empty());
    // Both attached connections end.
    let mut rest = String::new();
    while let Some(Ok(b)) = queued.next().await {
        rest.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(rest.contains("event: done"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_saves_the_partial_reply_marks_it_interrupted_and_writes_it_once() {
    let t = start().await;
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let mut first = t.chat(&http, None, "p1", "please be slow").await.bytes_stream();
    let got = read_deltas(&mut first, 3).await;
    let conversation = conversation_id_of(&got);
    let mut queued = t.chat(&http, Some(&conversation), "p1", "waiting").await.bytes_stream();
    let mut meta = String::new();
    while !meta.contains("\n\n") && !meta.contains("\r\n\r\n") {
        meta.push_str(&String::from_utf8_lossy(&queued.next().await.unwrap().unwrap()));
    }

    askbar_server::generation::shutdown(&t.state);
    let mut rest = String::new();
    while let Some(Ok(b)) = first.next().await {
        rest.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(rest.contains("event: error") && rest.contains("restarted"), "got {rest:?}");
    assert!(rest.contains(r#""code":"interrupted""#), "got {rest:?}");
    tokio::time::sleep(Duration::from_millis(500)).await; // let the stopped task finish

    let rows = t.generations();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["interrupted", "interrupted"]);
    let assistant: Vec<String> = t.messages().into_iter().filter(|m| m.0 == "assistant").map(|m| m.1).collect();
    assert_eq!(assistant.len(), 1, "the partial reply is written once");
    assert!(assistant[0].starts_with("Streaming sl"), "got {:?}", assistant[0]);
    assert!(t.messages().iter().all(|m| m.1 != "waiting"), "the queued message was not run or stored");
    assert_eq!(t.mock.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).count(), 1, "nothing new started");
}

impl T {
    /// (status, started_at, deadline_at) of every generation, oldest first.
    fn deadlines(&self) -> Vec<(String, Option<i64>, Option<i64>)> {
        let db = self.state.db.lock().unwrap();
        let mut stmt = db.conn().prepare("SELECT status, started_at, deadline_at FROM generations ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
        rows
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_deadline_is_start_plus_the_configured_timeout_read_when_it_starts() {
    let t = start().await;
    let http = client();
    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.generation_timeout_secs = 90);
    t.chat(&http, None, "p1", "Hello there").await.text().await.unwrap();
    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.generation_timeout_secs = 120);
    t.chat(&http, None, "p1", "Hello again").await.text().await.unwrap();

    let d = t.deadlines();
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].2.unwrap() - d[0].1.unwrap(), 90);
    assert_eq!(d[1].2.unwrap() - d[1].1.unwrap(), 120, "the change applies to generations started afterwards only");

    // The test override replaces the setting.
    askbar_server::testing::override_generation_timeout(&t.state, Duration::from_secs(2));
    t.chat(&http, None, "p1", "Hello once more").await.text().await.unwrap();
    let d = t.deadlines();
    assert_eq!(d[2].2.unwrap() - d[2].1.unwrap(), 2);
    assert_eq!(d[0].2.unwrap() - d[0].1.unwrap(), 90, "earlier rows are untouched");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_queued_message_gets_its_deadline_when_it_starts_not_when_it_was_queued() {
    let t = start().await;
    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.generation_timeout_secs = 100);
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let mut first = t.chat(&http, None, "p1", "please be slow").await.bytes_stream();
    let got = read_deltas(&mut first, 2).await;
    let (conversation, first_id) = (conversation_id_of(&got), generation_id(&got));
    let mut second = t.chat(&http, Some(&conversation), "p1", "second").await.bytes_stream();
    let mut meta = String::new();
    while !meta.contains("\n\n") && !meta.contains("\r\n\r\n") {
        meta.push_str(&String::from_utf8_lossy(&second.next().await.unwrap().unwrap()));
    }
    let d = t.deadlines();
    assert_eq!((d[1].0.as_str(), d[1].1, d[1].2), ("queued", None, None), "no clock while it waits");

    // The setting changes while it waits; it starts under the new value.
    askbar_server::testing::update_snapshot(&t.state.db.lock().unwrap(), |s| s.generation_timeout_secs = 60);
    assert_eq!(t.cancel_with(&t.token, &first_id).await.status(), 204);
    wait_for("the queued message to start", || (t.deadlines()[1].0 == "running" || t.deadlines()[1].0 == "done").then_some(()));
    let d = t.deadlines();
    assert_eq!(d[1].2.unwrap() - d[1].1.unwrap(), 60);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_generation_that_runs_out_of_time_keeps_its_text_reports_it_and_the_queue_moves_on() {
    let t = start().await;
    askbar_server::testing::override_generation_timeout(&t.state, Duration::from_secs(2));
    let http = client_builder().pool_max_idle_per_host(0).build().unwrap();
    let mut first = t.chat(&http, None, "p1", "please be slow").await.bytes_stream();
    let got = read_deltas(&mut first, 2).await;
    let conversation = conversation_id_of(&got);
    let mut second = t.chat(&http, Some(&conversation), "p1", "second").await.bytes_stream();

    // The first stream ends with the timeout error.
    let mut rest = got;
    while let Some(Ok(b)) = first.next().await {
        rest.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(rest.contains("event: error") && rest.contains("Timed out after 2 seconds"), "got {rest:?}");
    assert!(rest.contains(r#""code":"timed_out""#), "the timeout says so in a code: {rest:?}");

    // The upstream was closed, the text so far is kept as the assistant message, the row says why.
    wait_for_secs(5, "the upstream to see the stop", || {
        t.mock.log.lock().unwrap().iter().any(|l| l.contains("client disconnected")).then_some(())
    });
    let rows = t.generations();
    assert_eq!(rows[0].0, "timed_out");
    assert_eq!(rows[0].2.as_deref(), Some("Timed out after 2 seconds"));
    assert!(rows[0].1.starts_with("Streaming slowly") && rows[0].1.len() < 400, "partial text kept: {:?}", rows[0].1);
    let assistant: Vec<String> = t.messages().into_iter().filter(|m| m.0 == "assistant").map(|m| m.1).collect();
    assert_eq!(assistant[0], rows[0].1, "saved as the assistant message");
    assert!(rows[0].3.is_some());

    // The queued message started afterwards and ran to the end under its own clock.
    let mut body = String::new();
    while let Some(Ok(b)) = second.next().await {
        body.push_str(&String::from_utf8_lossy(&b));
    }
    assert!(body.contains("event: done"), "got {body:?}");
    let rows = t.generations();
    assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), ["timed_out", "done"]);
    let roles: Vec<String> = t.messages().into_iter().map(|m| m.0).collect();
    assert_eq!(roles, ["user", "assistant", "user", "assistant"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_finished_chat_records_one_token_usage_row_and_a_failed_one_none() {
    let t = start().await;
    let http = client();
    t.chat(&http, None, "p1", "Hello there").await.text().await.unwrap();
    let rows = |t: &T| -> Vec<(String, Option<String>, Option<String>, i64, i64)> {
        let db = t.state.db.lock().unwrap();
        let mut stmt = db.conn().prepare("SELECT user_id, client_id, provider_id, tokens_in, tokens_out FROM token_usage").unwrap();
        let rows =
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).unwrap().map(Result::unwrap).collect();
        rows
    };
    let r = rows(&t);
    assert_eq!(r.len(), 1);
    let client: String = t.state.db.lock().unwrap().conn().query_row("SELECT id FROM clients", [], |r| r.get(0)).unwrap();
    assert_eq!(
        (r[0].1.as_deref(), r[0].2.as_deref(), r[0].3, r[0].4),
        (Some(client.as_str()), Some("p1"), 100, 50),
        "the mock's usage chunk"
    );
    assert_eq!(r[0].0, t.user_id());

    t.chat(&http, None, "p1", "please fail").await.text().await.unwrap();
    assert_eq!(rows(&t).len(), 1, "a request that failed before any reply has no usage");
}
