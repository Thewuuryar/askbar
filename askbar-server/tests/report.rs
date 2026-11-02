//! `stream_chat_report` against the real tests/mock_llm.py: timings, statuses and Retry-After.

use askbar_server::llm::{http_client, stream_chat_report, ChatMessage, Provider, ProviderKind, StreamOutcome};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub struct Mock {
    child: Child,
    pub port: u16,
    pub log: Arc<Mutex<Vec<String>>>,
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn start_mock() -> Mock {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/mock_llm.py");
    let mut child = Command::new("python3").args(["-u", script, &port.to_string()]).stdout(Stdio::piped()).spawn().expect("python3 is needed");
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

pub fn provider(mock: &Mock, kind: ProviderKind) -> Provider {
    Provider {
        name: "mock".into(),
        kind,
        base_url: format!("http://127.0.0.1:{}{}", mock.port, if kind == ProviderKind::OpenAi { "/v1" } else { "" }),
        api_key: "sk-test".into(),
        api_key_env: String::new(),
        model: "m".into(),
        max_tokens: None,
        temperature: None,
        headers: Default::default(),
    }
}

#[tokio::test]
async fn a_completed_reply_reports_timings_and_the_http_status() {
    let mock = start_mock();
    let (_keep, cancel) = oneshot::channel::<()>();
    let mut text = String::new();
    let r = stream_chat_report(&http_client(), &provider(&mock, ProviderKind::OpenAi), "", &[ChatMessage::new("user", "hello")], cancel, |d| {
        text.push_str(d)
    })
    .await
    .unwrap();
    assert_eq!(r.outcome, StreamOutcome::Completed);
    assert_eq!(r.http_status, Some(200));
    assert!(text.ends_with("Anything else?"));
    let ttft = r.time_to_first_token.expect("a first token arrived");
    assert!(ttft <= r.total, "first token {ttft:?} no later than the end {:?}", r.total);
    assert!(r.total > Duration::ZERO);
    assert_eq!(r.retry_after, None);
}

#[tokio::test]
async fn a_slow_reply_has_a_first_token_time_no_greater_than_the_total() {
    let mock = start_mock();
    let (stop, cancel) = oneshot::channel::<()>();
    let mut stop = Some(stop);
    let mut deltas = 0;
    let r = stream_chat_report(&http_client(), &provider(&mock, ProviderKind::OpenAi), "", &[ChatMessage::new("user", "be slow")], cancel, |_| {
        deltas += 1;
        if deltas == 3 {
            if let Some(s) = stop.take() {
                let _ = s.send(());
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(r.outcome, StreamOutcome::Cancelled);
    let ttft = r.time_to_first_token.unwrap();
    assert!(ttft <= r.total);
    assert!(r.total >= Duration::from_millis(250), "three slow deltas take a while: {:?}", r.total);
}

#[tokio::test]
async fn provider_failures_report_their_status() {
    let mock = start_mock();
    let (_k, cancel) = oneshot::channel::<()>();
    let e = stream_chat_report(&http_client(), &provider(&mock, ProviderKind::OpenAi), "", &[ChatMessage::new("user", "please fail")], cancel, |_| {})
        .await
        .unwrap_err();
    assert_eq!(e.report.http_status, Some(401));
    assert!(e.error.to_string().contains("Incorrect API key"));
    assert_eq!(e.report.time_to_first_token, None);
    assert!(e.report.total > Duration::ZERO);

    let (_k, cancel) = oneshot::channel::<()>();
    let e = stream_chat_report(&http_client(), &provider(&mock, ProviderKind::OpenAi), "", &[ChatMessage::new("user", "err 500")], cancel, |_| {})
        .await
        .unwrap_err();
    assert_eq!((e.report.http_status, e.report.retry_after), (Some(500), None));
}

#[tokio::test]
async fn a_429_reports_its_status_and_retry_after() {
    let mock = start_mock();
    let (_k, cancel) = oneshot::channel::<()>();
    let e = stream_chat_report(&http_client(), &provider(&mock, ProviderKind::OpenAi), "", &[ChatMessage::new("user", "limit 429")], cancel, |_| {})
        .await
        .unwrap_err();
    assert_eq!(e.report.http_status, Some(429));
    assert_eq!(e.report.retry_after, Some(Duration::from_secs(1)));
}

#[tokio::test]
async fn a_connection_that_cannot_be_made_has_no_status() {
    let mut p = provider(&start_mock(), ProviderKind::OpenAi);
    p.base_url = "http://127.0.0.1:9/v1".into();
    let (_k, cancel) = oneshot::channel::<()>();
    let e = stream_chat_report(&http_client(), &p, "", &[ChatMessage::new("user", "hi")], cancel, |_| {}).await.unwrap_err();
    assert_eq!(e.report.http_status, None);
    assert!(matches!(e.error, askbar_server::llm::ChatError::Connect { .. }));
}
