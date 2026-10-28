//! End-to-end streaming tests against a local mock server speaking just enough
//! HTTP + SSE to stand in for OpenAI-compatible and Anthropic endpoints.

use askbar_core::{http_client, stream_chat, ChatError, ChatMessage, Config, Provider, ProviderKind, StreamOutcome};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

struct Mock {
    base: String,
    /// Raw request text (headers + body) of every request received.
    requests: Arc<Mutex<Vec<String>>>,
}

/// Serve `chunks` with `status`, pausing `gap_ms` between chunks. If
/// `hang_after` is set the connection stays open after the last chunk.
async fn serve(status: u16, chunks: Vec<String>, gap_ms: u64, hang_after: bool) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let log = requests.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let chunks = chunks.clone();
            let log = log.clone();
            tokio::spawn(async move {
                // Read headers, then the body by Content-Length.
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let header_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let len = head
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                while buf.len() < header_end + len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                log.lock().unwrap().push(String::from_utf8_lossy(&buf).to_string());

                let ctype = if status == 200 { "text/event-stream" } else { "application/json" };
                let head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
                );
                let _ = sock.write_all(head.as_bytes()).await;
                for c in chunks {
                    let frame = format!("{:x}\r\n{}\r\n", c.len(), c);
                    if sock.write_all(frame.as_bytes()).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(gap_ms)).await;
                }
                if hang_after {
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                } else {
                    let _ = sock.write_all(b"0\r\n\r\n").await;
                }
            });
        }
    });
    Mock { base, requests }
}

fn provider(kind: ProviderKind, base: &str, key: &str) -> Provider {
    let mut p = Config::builtin().providers.into_iter().find(|p| p.kind == kind).unwrap();
    p.base_url = base.to_string();
    p.api_key = key.to_string();
    p.api_key_env = String::new();
    p
}

fn msgs() -> Vec<ChatMessage> {
    vec![ChatMessage::new("user", "hello")]
}

async fn collect(p: &Provider, system: &str) -> (Result<StreamOutcome, ChatError>, String) {
    let (_tx, rx) = oneshot::channel();
    let mut text = String::new();
    let r = stream_chat(&http_client(), p, system, &msgs(), rx, |d| text.push_str(d)).await;
    (r, text)
}

fn sse(data: &str) -> String {
    format!("data: {data}\n\n")
}

#[tokio::test]
async fn openai_compatible_streams_text_and_sends_expected_request() {
    let mock = serve(
        200,
        vec![
            sse(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#),
            sse(r#"{"choices":[{"delta":{"content":"Hel"}}]}"#),
            // split an event across two network chunks, with a multi-byte char
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo wörld".to_string(),
            "\"}}]}\n\n".to_string(),
            sse("[DONE]"),
        ],
        5,
        false,
    )
    .await;
    let p = provider(ProviderKind::OpenAi, &format!("{}/v1", mock.base), "sk-test");
    let (r, text) = collect(&p, "sys prompt").await;
    assert_eq!(r.unwrap(), StreamOutcome::Completed);
    assert_eq!(text, "Hello wörld");

    let req = mock.requests.lock().unwrap()[0].clone();
    assert!(req.starts_with("POST /v1/chat/completions "), "{req}");
    assert!(req.to_lowercase().contains("authorization: bearer sk-test"), "{req}");
    let body: serde_json::Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["stream"], true);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], "sys prompt");
    assert_eq!(body["messages"][1]["content"], "hello");
}

#[tokio::test]
async fn openai_compatible_without_key_sends_no_auth_header() {
    let mock = serve(200, vec![sse(r#"{"choices":[{"delta":{"content":"ok"}}]}"#), sse("[DONE]")], 0, false).await;
    let p = provider(ProviderKind::OpenAi, &mock.base, "");
    let (r, text) = collect(&p, "").await;
    assert_eq!(r.unwrap(), StreamOutcome::Completed);
    assert_eq!(text, "ok");
    let req = mock.requests.lock().unwrap()[0].to_lowercase();
    assert!(!req.contains("authorization:"), "{req}");
}

#[tokio::test]
async fn anthropic_streams_text_and_sends_expected_request() {
    let events = [
        ("message_start", r#"{"type":"message_start","message":{"id":"m"}}"#),
        ("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
        ("ping", r#"{"type":"ping"}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi "}}"#),
        ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"there"}}"#),
        ("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
        ("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#),
        ("message_stop", r#"{"type":"message_stop"}"#),
    ];
    let chunks = events.iter().map(|(e, d)| format!("event: {e}\ndata: {d}\n\n")).collect();
    let mock = serve(200, chunks, 2, false).await;
    let p = provider(ProviderKind::Anthropic, &mock.base, "ak-test");
    let (r, text) = collect(&p, "sys prompt").await;
    assert_eq!(r.unwrap(), StreamOutcome::Completed);
    assert_eq!(text, "Hi there");

    let req = mock.requests.lock().unwrap()[0].clone();
    assert!(req.starts_with("POST /v1/messages "), "{req}");
    let lower = req.to_lowercase();
    assert!(lower.contains("x-api-key: ak-test"), "{req}");
    assert!(lower.contains("anthropic-version: 2023-06-01"), "{req}");
    let body: serde_json::Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["system"], "sys prompt");
    assert_eq!(body["max_tokens"], 4096);
    assert_eq!(body["messages"][0]["role"], "user");
}

#[tokio::test]
async fn http_errors_surface_the_providers_message() {
    let mock = serve(401, vec![r#"{"error":{"message":"Incorrect API key provided"}}"#.to_string()], 0, false).await;
    let p = provider(ProviderKind::OpenAi, &mock.base, "bad");
    let (r, text) = collect(&p, "").await;
    assert!(text.is_empty());
    match r.unwrap_err() {
        ChatError::Api { status, message, .. } => {
            assert_eq!(status, 401);
            assert_eq!(message, "Incorrect API key provided");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn mid_stream_error_event_keeps_earlier_text() {
    let mock = serve(
        200,
        vec![
            sse(r#"{"choices":[{"delta":{"content":"partial"}}]}"#),
            sse(r#"{"error":{"message":"upstream exploded"}}"#),
        ],
        0,
        false,
    )
    .await;
    let p = provider(ProviderKind::OpenAi, &mock.base, "k");
    let (r, text) = collect(&p, "").await;
    assert_eq!(text, "partial");
    assert!(r.unwrap_err().to_string().contains("upstream exploded"));
}

#[tokio::test]
async fn connection_refused_names_the_url() {
    // Bind then drop to get a port nothing listens on.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let p = provider(ProviderKind::OpenAi, &format!("http://127.0.0.1:{port}/v1"), "");
    let (r, _) = collect(&p, "").await;
    let msg = r.unwrap_err().to_string();
    assert!(msg.contains(&format!("127.0.0.1:{port}")), "{msg}");
}

#[tokio::test]
async fn cancel_stops_a_hanging_stream_and_keeps_partial_text() {
    let mock = serve(200, vec![sse(r#"{"choices":[{"delta":{"content":"so far"}}]}"#)], 0, true).await;
    let p = provider(ProviderKind::OpenAi, &mock.base, "k");
    let (tx, rx) = oneshot::channel();
    let mut text = String::new();
    let client = http_client();
    let m = msgs();

    let canceller = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let _ = tx.send(());
    });
    let started = std::time::Instant::now();
    let r = stream_chat(&client, &p, "", &m, rx, |d| text.push_str(d)).await;
    canceller.await.unwrap();

    assert_eq!(r.unwrap(), StreamOutcome::Cancelled);
    assert_eq!(text, "so far");
    assert!(started.elapsed() < std::time::Duration::from_secs(5), "cancel must not wait for the server");
}

#[tokio::test]
async fn missing_anthropic_key_fails_before_any_network_call() {
    let mock = serve(200, vec![], 0, false).await;
    let p = provider(ProviderKind::Anthropic, &mock.base, "");
    let (r, _) = collect(&p, "").await;
    assert!(matches!(r, Err(ChatError::Config(_))));
    assert!(mock.requests.lock().unwrap().is_empty());
}
