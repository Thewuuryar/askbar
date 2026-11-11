//! What happens to a reply when the auditor or the gateway cannot do its part (ADR 0010 rule 3, ADR 0054): the audit
//! trail is never skipped, so model work stops with `503 audit_backlog` instead, and a reply already running finishes.
//! Each test runs the server, the gateway and the auditor (in-process, the rig) and the real mock LLM.

use askbar_server::generation::{submit, Submit, SubmitError};
use askbar_server::testing::Rig;
use askbar_server::AppState;
use axum::response::IntoResponse;
use futures_util::StreamExt;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The mock LLM, with the lines it logs (one per request).
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
    let mut child =
        Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
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

impl Mock {
    fn provider_calls(&self) -> usize {
        self.log.lock().unwrap().iter().filter(|l| l.contains("POST")).count()
    }
}

struct Stack {
    state: Arc<AppState>,
    rig: &'static Rig,
    mock: Mock,
    user: String,
    client: String,
}

async fn stack() -> Stack {
    let mock = start_mock();
    let (state, _dir, rig) = askbar_server::testing::state_with_audit(&format!("http://127.0.0.1:{}/v1", mock.port)).await;
    let (user, client) = {
        let db = state.db.lock().unwrap();
        let user = db.seed_server().unwrap();
        let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
        askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
        let client: String = db.conn().query_row("SELECT id FROM clients ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
        (user, client)
    };
    Stack { state, rig, mock, user, client }
}

impl Stack {
    fn submit(&self, prompt: &str) -> Result<askbar_server::generation::Submitted, SubmitError> {
        submit(
            &self.state,
            Submit {
                space: askbar_server::testing::personal_space(&self.state, &self.user),
                user_id: self.user.clone(),
                client_id: self.client.clone(),
                conversation_id: None,
                provider_id: "p1".into(),
                prompt: prompt.into(),
                client_request_id: None,
            },
        )
    }

    /// The events of one reply, to its end.
    async fn run(&self, prompt: &str) -> Vec<askbar_server::generation::GenEvent> {
        self.submit(prompt).expect("accepted").live.subscribe(0).collect().await
    }

    fn set_free(&self, bytes: u64) {
        self.rig.free.store(bytes, Ordering::Relaxed);
    }
}

const LOW: u64 = 1024;
const PLENTY: u64 = 100 * 1024 * 1024 * 1024;

async fn until(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn a_full_audit_disk_stops_new_replies_before_any_provider_is_called_and_they_resume_when_there_is_room() {
    use askbar_server::generation::GenEvent;
    let s = stack().await;
    s.set_free(LOW); // the auditor refuses new spans

    // The first reply gets as far as the gateway, which cannot record its turn, so it calls no provider.
    let events = s.run("hello").await;
    let Some(GenEvent::Error { message, .. }) = events.last() else { panic!("the reply fails: {events:?}") };
    assert!(message.contains("nothing was sent to the provider"), "{message}");
    assert_eq!(s.mock.provider_calls(), 0, "no provider was called");

    // The server's own gate spans were refused too, so it stops taking new ones: 503 audit_backlog with Retry-After.
    until("the server notices the auditor refuses", || !s.state.audit.healthy()).await;
    let refused = s.submit("again").err().expect("refused");
    let response = refused.into_response();
    assert_eq!(response.status(), 503);
    assert!(response.headers().contains_key("retry-after"));
    assert_eq!(s.mock.provider_calls(), 0);

    // Room again: replies run, and the refusal the gateway kept is recorded.
    s.set_free(PLENTY);
    until("the server records again", || s.state.audit.healthy()).await;
    let events = s.run("hello again").await;
    assert!(matches!(events.last(), Some(GenEvent::Done)), "{events:?}");
    assert_eq!(s.mock.provider_calls(), 1);
    let refused = s.rig.wait_rows("SELECT status, error_code FROM spans WHERE kind = 'turn' AND status = 'refused'").await;
    assert_eq!(refused[0][1], "audit_backlog");
}

#[tokio::test]
async fn a_reply_already_running_finishes_when_the_auditor_runs_out_of_room() {
    use askbar_server::generation::GenEvent;
    let s = stack().await;
    let sub = s.submit("brisk answer please").expect("accepted");
    let mut stream = Box::pin(sub.live.subscribe(0));
    // Wait for text, so the turn span is open, then the disk fills.
    loop {
        if matches!(stream.next().await, Some(GenEvent::Delta { .. })) {
            break;
        }
    }
    s.set_free(LOW);
    let rest: Vec<GenEvent> = stream.by_ref().collect().await;
    assert!(matches!(rest.last(), Some(GenEvent::Done)), "the reply that was running finishes: {rest:?}");
    // The finish record and the sealed reply are still taken: only new spans are refused.
    s.state.audit.flush().await;
    let turn = s
        .rig
        .wait_rows(&format!("SELECT status FROM spans WHERE trace_id = '{}' AND kind = 'turn' AND status != 'running'", sub.generation_id))
        .await;
    assert_eq!(turn[0][0], "done");
    let sealed =
        s.rig.rows(&format!("SELECT COUNT(*) FROM payloads p JOIN spans s ON s.id = p.span_id WHERE s.trace_id = '{}'", sub.generation_id));
    assert!(sealed[0][0].as_i64().unwrap() >= 2, "the request and the reply: {sealed:?}");
}

#[tokio::test]
async fn an_auditor_that_cannot_be_reached_stops_replies_at_the_server_and_nothing_is_called() {
    let mock = start_mock();
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    askbar_server::testing::add_provider(
        &db,
        askbar_server::testing::provider("p1", "mock", &format!("http://127.0.0.1:{}/v1", mock.port), "m"),
    );
    let rig = Rig::start().await;
    // A server whose link to the auditor goes nowhere: it never gets its boot ID accepted.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let client =
        askbar_service::audit_client::AuditClient::new(&format!("http://127.0.0.1:{dead}"), std::env::temp_dir(), "server-auditor");
    let state = AppState::new(db, askbar_server::MIN_CLIENT)
        .with_audit(askbar_server::audit::Recorder::new(client))
        .with_gateway(askbar_server::gateway::GatewayClient::new(&rig.gateway_url, std::env::temp_dir()));
    let refused = submit(
        &state,
        Submit {
            space: askbar_server::testing::personal_space(&state, &user),
            user_id: user,
            client_id: "d1".into(),
            conversation_id: None,
            provider_id: "p1".into(),
            prompt: "hello".into(),
            client_request_id: None,
        },
    )
    .err()
    .expect("refused");
    assert_eq!(refused.into_response().status(), 503);
    assert_eq!(mock.provider_calls(), 0);
}

#[tokio::test]
async fn a_gateway_with_no_configuration_fails_the_reply_with_a_sentence() {
    use askbar_server::generation::GenEvent;
    let s = stack().await;
    // The gateway has just started and the admin has not given it its settings.
    let fresh = askbar_model_gateway_unconfigured(&s).await;
    let events = fresh.run("hello").await;
    let Some(GenEvent::Error { message, code }) = events.last() else { panic!("{events:?}") };
    assert_eq!(*code, "provider_error");
    assert!(message.contains("starting up"), "{message}");
    assert_eq!(s.mock.provider_calls(), 0);
}

/// A second state on the same rig whose gateway link points at a gateway nothing has configured.
async fn askbar_model_gateway_unconfigured(s: &Stack) -> Stack {
    let other = Rig::start().await; // a rig gives a gateway with no configuration until `sync`
    let db = askbar_server::Db::open_in_memory().unwrap();
    let user = db.seed_server().unwrap();
    askbar_server::testing::add_provider(
        &db,
        askbar_server::testing::provider("p1", "mock", &format!("http://127.0.0.1:{}/v1", s.mock.port), "m"),
    );
    let code = askbar_server::testing::add_pair_code(&db, &user, "laptop");
    askbar_server::auth::redeem_pair_code(&db, &code).unwrap().unwrap();
    let client: String = db.conn().query_row("SELECT id FROM clients ORDER BY created_at DESC LIMIT 1", [], |r| r.get(0)).unwrap();
    let state = other.attach(AppState::new(db, askbar_server::MIN_CLIENT)).await; // no `sync`: no configuration
    let rig: &'static Rig = Box::leak(Box::new(other));
    // The mock is the first stack's; it is only counted, never started again.
    let mock = Mock { child: Command::new("true").spawn().unwrap(), port: s.mock.port, log: s.mock.log.clone() };
    Stack { state, rig, mock, user, client }
}
