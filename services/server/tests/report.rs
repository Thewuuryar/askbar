//! `stream_chat_report` against the real tests/mock_llm.py: timings, statuses and Retry-After.

use askbar_server::llm::{http_client, stream_chat_report, ChatMessage, Provider, ProviderKind, StreamOutcome, Usage};
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
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/e2e/mock_llm.py");
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

#[tokio::test]
async fn openai_usage_from_the_final_chunk_reaches_the_report() {
    let mock = start_mock();
    let run = |text: &'static str| {
        let p = provider(&mock, ProviderKind::OpenAi);
        async move {
            let (_k, cancel) = oneshot::channel::<()>();
            stream_chat_report(&http_client(), &p, "", &[ChatMessage::new("user", text)], cancel, |_| {}).await.unwrap()
        }
    };
    let r = run("hello").await;
    assert_eq!(r.usage, Some(Usage { input: Some(100), output: Some(50), cache_read: Some(0), cache_write: None, estimated: false }));
    let r = run("a cached prompt").await;
    assert_eq!(r.usage.unwrap().cache_read, Some(64), "cached tokens are reported when the mock says so");
    assert!(mock.log.lock().unwrap().iter().filter(|l| l.starts_with("POST")).all(|l| l.contains(r#"stream_options={"include_usage": true}"#)), "the request asked for usage");
}

#[tokio::test]
async fn anthropic_usage_from_message_start_and_message_delta_reaches_the_report() {
    let mock = start_mock();
    let run = |text: &'static str| {
        let p = provider(&mock, ProviderKind::Anthropic);
        async move {
            let (_k, cancel) = oneshot::channel::<()>();
            let mut reply = String::new();
            let r = stream_chat_report(&http_client(), &p, "sys", &[ChatMessage::new("user", text)], cancel, |d| reply.push_str(d)).await.unwrap();
            (r, reply)
        }
    };
    let (r, reply) = run("hello").await;
    assert!(reply.ends_with("Anything else?"), "the Anthropic endpoint streams text: {reply:?}");
    assert_eq!(r.outcome, StreamOutcome::Completed);
    assert_eq!(r.usage, Some(Usage { input: Some(100), output: Some(50), cache_read: Some(0), cache_write: Some(0), estimated: false }));
    let (r, _) = run("a cached prompt").await;
    assert_eq!(r.usage.unwrap().cache_read, Some(64));
    assert!(r.time_to_first_token.is_some());
}

/// A provider that answers every request with `body` (an SSE stream) and nothing else.
async fn canned_provider(body: &'static str) -> Provider {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = l.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut seen = Vec::new();
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                // The body's length is not needed to answer; read what is there and reply.
                let head = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body.as_bytes()).await;
            });
        }
    });
    Provider {
        name: "canned".into(),
        kind: ProviderKind::OpenAi,
        base_url: format!("http://127.0.0.1:{port}/v1"),
        api_key: "k".into(),
        api_key_env: String::new(),
        model: "m".into(),
        max_tokens: None,
        temperature: None,
        headers: Default::default(),
    }
}

#[tokio::test]
async fn a_completed_reply_with_no_usage_gets_a_marked_estimate() {
    // 16 characters of reply; system "abcd" (4) + question "efghijkl" (8) = 12 characters sent.
    let p = canned_provider(
        "data: {\"choices\":[{\"delta\":{\"content\":\"0123456789\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"abcdef\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_k, cancel) = oneshot::channel::<()>();
    let r = stream_chat_report(&http_client(), &p, "abcd", &[ChatMessage::new("user", "efghijkl")], cancel, |_| {}).await.unwrap();
    assert_eq!(r.usage, Some(Usage { input: Some(3), output: Some(4), cache_read: None, cache_write: None, estimated: true }));
}

#[tokio::test]
async fn real_usage_is_never_overwritten_by_the_estimate_and_failures_get_none() {
    let p = canned_provider(
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello there\"}}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n",
    )
    .await;
    let (_k, cancel) = oneshot::channel::<()>();
    let r = stream_chat_report(&http_client(), &p, "a long system prompt that would estimate high", &[ChatMessage::new("user", "q")], cancel, |_| {})
        .await
        .unwrap();
    assert_eq!(r.usage, Some(Usage { input: Some(9), output: Some(2), cache_read: None, cache_write: None, estimated: false }));

    // A failed request has no usage and no estimate.
    let mock = start_mock();
    let (_k, cancel) = oneshot::channel::<()>();
    let e = stream_chat_report(&http_client(), &provider(&mock, ProviderKind::OpenAi), "", &[ChatMessage::new("user", "please fail")], cancel, |_| {})
        .await
        .unwrap_err();
    assert_eq!(e.report.usage, None);
}
