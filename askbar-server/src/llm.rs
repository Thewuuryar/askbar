//! Streaming chat against OpenAI-compatible and Anthropic endpoints.
//!
//! Text in, text out. There is no tool calling anywhere in this module, so a
//! model can never be handed a way to touch the machine.

use askbar_proto::{SseEvent, SseParser};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

pub use askbar_proto::DEFAULT_SYSTEM_PROMPT;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Any OpenAI-compatible `/chat/completions` endpoint.
    #[serde(alias = "openai-compatible", alias = "openai_compatible")]
    OpenAi,
    /// The Anthropic Messages API.
    Anthropic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ProviderKind,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_key_env: String,
    pub model: String,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl Provider {
    pub fn effective_base_url(&self) -> String {
        let url = self.base_url.trim();
        if !url.is_empty() {
            return url.trim_end_matches('/').to_string();
        }
        match self.kind {
            ProviderKind::OpenAi => "https://api.openai.com/v1".into(),
            ProviderKind::Anthropic => "https://api.anthropic.com".into(),
        }
    }

    /// `api_key` wins; otherwise the environment variable named in `api_key_env`.
    pub fn resolve_key(&self) -> Option<String> {
        let direct = self.api_key.trim();
        if !direct.is_empty() {
            return Some(direct.to_string());
        }
        let var = self.api_key_env.trim();
        if var.is_empty() {
            return None;
        }
        std::env::var(var)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: &str) -> Self {
        Self { role: role.into(), content: content.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    Completed,
    Cancelled,
}

#[derive(Debug, thiserror::Error)]
pub enum ChatError {
    #[error("{0}")]
    Config(String),
    #[error("could not reach {url}: {message}")]
    Connect { url: String, message: String },
    #[error("{provider} returned {status}: {message}")]
    Api { provider: String, status: u16, message: String },
    #[error("stream failed: {0}")]
    Stream(String),
}

/// Tokens a reply used, as the provider reported them (or estimated, see `estimated`).
/// A field the provider did not report is `None`, not 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    /// Prompt tokens served from the provider's cache.
    pub cache_read: Option<u64>,
    /// Prompt tokens written to the provider's cache.
    pub cache_write: Option<u64>,
    /// True when the numbers are our own estimate, not the provider's.
    pub estimated: bool,
}

impl Usage {
    /// Takes every field `other` reports, keeping ours where it reports nothing.
    pub fn merge(&mut self, other: &Usage) {
        for (mine, theirs) in [
            (&mut self.input, other.input),
            (&mut self.output, other.output),
            (&mut self.cache_read, other.cache_read),
            (&mut self.cache_write, other.cache_write),
        ] {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
        self.estimated |= other.estimated;
    }
}

/// What happened during one request to a provider, for audit and cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamReport {
    pub outcome: StreamOutcome,
    pub usage: Option<Usage>,
    /// From sending the request to the first piece of text.
    pub time_to_first_token: Option<Duration>,
    /// From sending the request to the end (or the failure).
    pub total: Duration,
    /// The HTTP status of the response, if one arrived.
    pub http_status: Option<u16>,
    /// The provider's `Retry-After`, if it sent one (seconds form).
    pub retry_after: Option<Duration>,
    /// Why the provider says it stopped (`stop`, `end_turn`, ...), if it said.
    pub finish_reason: Option<String>,
}

/// A failed request, with what is known about it up to the failure.
#[derive(Debug)]
pub struct ReportedError {
    pub error: ChatError,
    pub report: StreamReport,
}

pub type HttpClient = reqwest::Client;

pub fn http_client() -> HttpClient {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("askbar/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client")
}

/// Drop empty messages and leading assistant turns, and merge consecutive
/// messages with the same role (a failed turn leaves two user messages in a
/// row, which some APIs reject).
pub fn normalize(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for m in messages {
        if m.content.trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == m.role => {
                last.content.push_str("\n\n");
                last.content.push_str(&m.content);
            }
            None if m.role != "user" => continue,
            _ => out.push(m.clone()),
        }
    }
    out
}

// ---------------------------------------------------------------- requests

fn messages_url(p: &Provider) -> String {
    let base = p.effective_base_url();
    match p.kind {
        ProviderKind::OpenAi => format!("{base}/chat/completions"),
        ProviderKind::Anthropic => {
            if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            }
        }
    }
}

pub fn build_body(p: &Provider, system: &str, messages: &[ChatMessage]) -> Value {
    let msgs: Vec<Value> = messages
        .iter()
        .map(|m| json!({ "role": m.role, "content": m.content }))
        .collect();
    match p.kind {
        ProviderKind::OpenAi => {
            let mut all = Vec::with_capacity(msgs.len() + 1);
            if !system.trim().is_empty() {
                all.push(json!({ "role": "system", "content": system }));
            }
            all.extend(msgs);
            // Ask for token counts in a final chunk; OpenAI-compatible servers that support it send it.
            let mut body = json!({ "model": p.model, "messages": all, "stream": true, "stream_options": { "include_usage": true } });
            if let Some(n) = p.max_tokens {
                body["max_tokens"] = json!(n);
            }
            if let Some(t) = p.temperature {
                body["temperature"] = json!(t);
            }
            body
        }
        ProviderKind::Anthropic => {
            let mut body = json!({
                "model": p.model,
                "max_tokens": p.max_tokens.unwrap_or(4096),
                "messages": msgs,
                "stream": true,
            });
            if !system.trim().is_empty() {
                body["system"] = json!(system);
            }
            if let Some(t) = p.temperature {
                body["temperature"] = json!(t);
            }
            body
        }
    }
}

fn build_request(
    client: &reqwest::Client,
    p: &Provider,
    system: &str,
    messages: &[ChatMessage],
) -> Result<reqwest::RequestBuilder, ChatError> {
    let key = p.resolve_key();
    let mut req = client
        .post(messages_url(p))
        .header("accept", "text/event-stream")
        .json(&build_body(p, system, messages));
    match p.kind {
        ProviderKind::OpenAi => {
            if let Some(k) = key {
                req = req.bearer_auth(k);
            }
        }
        ProviderKind::Anthropic => {
            let k = key.ok_or_else(|| missing_key(p))?;
            req = req.header("x-api-key", k).header("anthropic-version", "2023-06-01");
        }
    }
    for (name, value) in &p.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    Ok(req)
}

fn missing_key(p: &Provider) -> ChatError {
    let hint = if p.api_key_env.trim().is_empty() {
        "set api_key".to_string()
    } else {
        format!("set api_key or export {}", p.api_key_env.trim())
    };
    ChatError::Config(format!("No API key for provider \"{}\". In config.yaml, {hint}.", p.name))
}

// ------------------------------------------------------- event decoding

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Delta(String),
    /// Token counts the provider reported; partial (fields it did not report are `None`).
    Usage(Usage),
    Done,
    Skip,
}

fn count(v: &Value) -> Option<u64> {
    v.as_u64()
}

fn error_message(v: &Value) -> String {
    v.get("message")
        .and_then(Value::as_str)
        .or_else(|| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| v.to_string())
}

pub fn decode_event(kind: ProviderKind, ev: &SseEvent) -> Result<Step, ChatError> {
    let data = ev.data.trim();
    match kind {
        ProviderKind::OpenAi => {
            if data == "[DONE]" {
                return Ok(Step::Done);
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(Step::Skip) };
            if let Some(err) = v.get("error") {
                return Err(ChatError::Stream(error_message(err)));
            }
            match v["choices"][0]["delta"]["content"].as_str() {
                Some(s) if !s.is_empty() => Ok(Step::Delta(s.to_string())),
                // The final chunk of a stream asked to include usage: empty `choices` and a `usage`
                // object. (A chunk with text and usage together gives the text; the counts then
                // come from the stream's last chunk.)
                _ => match v.get("usage") {
                    Some(u) if u.is_object() => Ok(Step::Usage(Usage {
                        input: count(&u["prompt_tokens"]),
                        output: count(&u["completion_tokens"]),
                        cache_read: count(&u["prompt_tokens_details"]["cached_tokens"]),
                        cache_write: None,
                        estimated: false,
                    })),
                    _ => Ok(Step::Skip),
                },
            }
        }
        ProviderKind::Anthropic => {
            let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(Step::Skip) };
            let ty = v["type"].as_str().or(ev.event.as_deref()).unwrap_or("");
            match ty {
                "content_block_delta" if v["delta"]["type"] == "text_delta" => {
                    Ok(Step::Delta(v["delta"]["text"].as_str().unwrap_or("").to_string()))
                }
                // Input and cache counts come first, with the message...
                "message_start" if v["message"]["usage"].is_object() => {
                    let u = &v["message"]["usage"];
                    Ok(Step::Usage(Usage {
                        input: count(&u["input_tokens"]),
                        output: None, // the 1 here is a placeholder; message_delta has the real total
                        cache_read: count(&u["cache_read_input_tokens"]),
                        cache_write: count(&u["cache_creation_input_tokens"]),
                        estimated: false,
                    }))
                }
                // ...and the output count follows as a running total, the last one being final.
                "message_delta" if v["usage"].is_object() => Ok(Step::Usage(Usage {
                    output: count(&v["usage"]["output_tokens"]),
                    ..Default::default()
                })),
                "message_stop" => Ok(Step::Done),
                "error" => Err(ChatError::Stream(error_message(&v["error"]))),
                _ => Ok(Step::Skip),
            }
        }
    }
}

/// Pull a readable message out of an error response body.
pub fn extract_api_error(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(body) {
        for candidate in [&v["error"]["message"], &v["error"], &v["message"], &v["detail"]] {
            if let Some(s) = candidate.as_str() {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    let t = body.trim();
    if t.is_empty() {
        "(empty response)".into()
    } else if t.chars().count() > 300 {
        format!("{}…", t.chars().take(300).collect::<String>())
    } else {
        t.to_string()
    }
}

// ------------------------------------------------------------ streaming

/// A rough token count for `chars` characters of text: one token per four characters, rounded up.
/// Only used when a provider reports nothing.
pub fn estimate_tokens(chars: usize) -> u64 {
    chars.div_ceil(4) as u64
}

/// Stream one assistant reply, calling `on_delta` for each piece of text.
///
/// Resolving `cancel` (by sending on, or dropping, its sender) stops the
/// request and returns `Cancelled`. Text already delivered stays delivered.
pub async fn stream_chat<F>(
    client: &reqwest::Client,
    provider: &Provider,
    system: &str,
    messages: &[ChatMessage],
    cancel: oneshot::Receiver<()>,
    on_delta: F,
) -> Result<StreamOutcome, ChatError>
where
    F: FnMut(&str),
{
    stream_chat_report(client, provider, system, messages, cancel, on_delta)
        .await
        .map(|r| r.outcome)
        .map_err(|e| e.error)
}

/// Like `stream_chat`, and also says how it went: timings, the HTTP status, `Retry-After`. On
/// failure the error carries the report up to that point.
pub async fn stream_chat_report<F>(
    client: &reqwest::Client,
    provider: &Provider,
    system: &str,
    messages: &[ChatMessage],
    cancel: oneshot::Receiver<()>,
    on_delta: F,
) -> Result<StreamReport, ReportedError>
where
    F: FnMut(&str),
{
    let started = Instant::now();
    let mut report = StreamReport {
        outcome: StreamOutcome::Completed,
        usage: None,
        time_to_first_token: None,
        total: Duration::ZERO,
        http_status: None,
        retry_after: None,
        finish_reason: None,
    };
    // For the estimate if the provider reports no usage: what was sent, and what came back.
    let sent_chars = system.chars().count() + normalize(messages).iter().map(|m| m.content.chars().count()).sum::<usize>();
    let mut reply_chars = 0usize;
    let mut on_delta = on_delta;
    let result = run_request(
        client,
        provider,
        system,
        messages,
        cancel,
        |d: &str| {
            reply_chars += d.chars().count();
            on_delta(d)
        },
        started,
        &mut report,
    )
    .await;
    report.total = started.elapsed();
    match result {
        Ok(outcome) => {
            report.outcome = outcome;
            // A reply that finished with no counts from the provider gets an estimate, marked as
            // one. Counts the provider did give are never replaced.
            if outcome == StreamOutcome::Completed && report.usage.is_none() {
                report.usage = Some(Usage {
                    input: Some(estimate_tokens(sent_chars)),
                    output: Some(estimate_tokens(reply_chars)),
                    cache_read: None,
                    cache_write: None,
                    estimated: true,
                });
            }
            Ok(report)
        }
        Err(error) => Err(ReportedError { error, report }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_request<F>(
    client: &reqwest::Client,
    provider: &Provider,
    system: &str,
    messages: &[ChatMessage],
    mut cancel: oneshot::Receiver<()>,
    mut on_delta: F,
    started: Instant,
    report: &mut StreamReport,
) -> Result<StreamOutcome, ChatError>
where
    F: FnMut(&str),
{
    let messages = normalize(messages);
    if messages.is_empty() {
        return Err(ChatError::Config("Nothing to send.".into()));
    }
    let req = build_request(client, provider, system, &messages)?;
    let url = messages_url(provider);

    let resp = tokio::select! {
        r = req.send() => r.map_err(|e| ChatError::Connect { url: url.clone(), message: e.to_string() })?,
        _ = &mut cancel => return Ok(StreamOutcome::Cancelled),
    };

    let status = resp.status();
    report.http_status = Some(status.as_u16());
    report.retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ChatError::Api {
            provider: provider.name.clone(),
            status: status.as_u16(),
            message: extract_api_error(&body),
        });
    }

    // Counts arrive in pieces on some APIs; they are merged as they come.
    let note_usage = |u: Usage, report: &mut StreamReport| report.usage.get_or_insert_with(Usage::default).merge(&u);
    let mut deliver = |s: &str, report: &mut StreamReport| {
        if report.time_to_first_token.is_none() {
            report.time_to_first_token = Some(started.elapsed());
        }
        on_delta(s);
    };
    let mut stream = Box::pin(resp.bytes_stream());
    let mut parser = SseParser::default();
    loop {
        let chunk = tokio::select! {
            c = stream.next() => c,
            _ = &mut cancel => return Ok(StreamOutcome::Cancelled),
        };
        let events = match chunk {
            None => break,
            Some(Err(e)) => return Err(ChatError::Stream(e.to_string())),
            Some(Ok(bytes)) => parser.feed(&bytes),
        };
        for ev in events {
            match decode_event(provider.kind, &ev)? {
                Step::Delta(s) => deliver(&s, report),
                Step::Usage(u) => note_usage(u, report),
                Step::Done => return Ok(StreamOutcome::Completed),
                Step::Skip => {}
            }
        }
    }
    for ev in parser.finish() {
        match decode_event(provider.kind, &ev)? {
            Step::Delta(s) => deliver(&s, report),
            Step::Usage(u) => note_usage(u, report),
            Step::Done => break,
            Step::Skip => {}
        }
    }
    Ok(StreamOutcome::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str, c: &str) -> ChatMessage {
        ChatMessage::new(role, c)
    }

    fn provider(kind: ProviderKind) -> Provider {
        Provider {
            name: "test".into(),
            kind,
            base_url: String::new(),
            api_key: String::new(),
            api_key_env: match kind {
                ProviderKind::Anthropic => "ANTHROPIC_API_KEY",
                ProviderKind::OpenAi => "OPENAI_API_KEY",
            }
            .into(),
            model: "m".into(),
            max_tokens: (kind == ProviderKind::Anthropic).then_some(4096),
            temperature: None,
            headers: BTreeMap::new(),
        }
    }

    #[test]
    fn key_resolution_prefers_inline_then_env() {
        let mut p = provider(ProviderKind::Anthropic);
        p.api_key = "  sk-inline ".into();
        p.api_key_env = "ASKBAR_TEST_KEY_A".into();
        assert_eq!(p.resolve_key().as_deref(), Some("sk-inline"));

        p.api_key = String::new();
        assert_eq!(p.resolve_key(), None);
        std::env::set_var("ASKBAR_TEST_KEY_A", " from-env ");
        assert_eq!(p.resolve_key().as_deref(), Some("from-env"));
        std::env::remove_var("ASKBAR_TEST_KEY_A");
    }

    #[test]
    fn base_url_defaults_per_kind() {
        assert_eq!(provider(ProviderKind::OpenAi).effective_base_url(), "https://api.openai.com/v1");
        assert_eq!(provider(ProviderKind::Anthropic).effective_base_url(), "https://api.anthropic.com");
    }

    #[test]
    fn normalize_merges_and_trims() {
        let out = normalize(&[
            m("assistant", "stray"),
            m("user", "a"),
            m("user", "b"),
            m("assistant", "  "),
            m("assistant", "c"),
            m("user", "d"),
        ]);
        assert_eq!(out, vec![m("user", "a\n\nb"), m("assistant", "c"), m("user", "d")]);
        assert!(normalize(&[m("assistant", "only")]).is_empty());
    }

    #[test]
    fn urls() {
        let mut p = provider(ProviderKind::OpenAi);
        assert_eq!(messages_url(&p), "https://api.openai.com/v1/chat/completions");
        p.base_url = "http://localhost:11434/v1/".into();
        assert_eq!(messages_url(&p), "http://localhost:11434/v1/chat/completions");

        let mut a = provider(ProviderKind::Anthropic);
        assert_eq!(messages_url(&a), "https://api.anthropic.com/v1/messages");
        a.base_url = "https://api.anthropic.com/v1".into();
        assert_eq!(messages_url(&a), "https://api.anthropic.com/v1/messages");
        a.base_url = "https://proxy.example/anthropic".into();
        assert_eq!(messages_url(&a), "https://proxy.example/anthropic/v1/messages");
    }

    #[test]
    fn request_bodies() {
        let msgs = [m("user", "hi")];
        let mut p = provider(ProviderKind::OpenAi);
        let b = build_body(&p, "be brief", &msgs);
        assert_eq!(b["stream"], true);
        assert_eq!(b["stream_options"], json!({ "include_usage": true }), "OpenAI-style bodies ask for usage");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["content"], "hi");
        assert!(b.get("max_tokens").is_none(), "unset max_tokens must be omitted");
        p.max_tokens = Some(100);
        assert_eq!(build_body(&p, "", &msgs)["max_tokens"], 100);
        assert_eq!(build_body(&p, "", &msgs)["messages"][0]["role"], "user");

        let a = provider(ProviderKind::Anthropic);
        let b = build_body(&a, "be brief", &msgs);
        assert_eq!(b["system"], "be brief");
        assert!(b.get("stream_options").is_none(), "the Anthropic body is unchanged");
        assert_eq!(b["max_tokens"], 4096);
        assert_eq!(b["messages"][0]["role"], "user");
        assert!(build_body(&a, "  ", &msgs).get("system").is_none());
    }

    #[test]
    fn openai_usage_chunks_are_decoded_and_are_not_errors() {
        let kind = ProviderKind::OpenAi;
        // Final chunk: empty choices and usage, as sent when include_usage is on.
        let chunk = r#"{"id":"x","choices":[],"usage":{"prompt_tokens":120,"completion_tokens":35,"total_tokens":155,"prompt_tokens_details":{"cached_tokens":64}}}"#;
        assert_eq!(
            decode_event(kind, &ev(chunk)).unwrap(),
            Step::Usage(Usage { input: Some(120), output: Some(35), cache_read: Some(64), cache_write: None, estimated: false })
        );
        // No cached-token detail: unknown, not zero.
        let plain = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2}}"#;
        assert_eq!(
            decode_event(kind, &ev(plain)).unwrap(),
            Step::Usage(Usage { input: Some(10), output: Some(2), cache_read: None, cache_write: None, estimated: false })
        );
        // Usage riding on an otherwise empty last chunk (finish_reason, empty text).
        let last = r#"{"choices":[{"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":6}}"#;
        assert!(matches!(decode_event(kind, &ev(last)).unwrap(), Step::Usage(u) if u.output == Some(6)));
        // Providers that put `"usage": null` on every chunk, and ordinary text chunks, are untouched.
        assert_eq!(decode_event(kind, &ev(r#"{"choices":[{"delta":{"content":"hi"}}],"usage":null}"#)).unwrap(), Step::Delta("hi".into()));
        assert_eq!(decode_event(kind, &ev(r#"{"choices":[{"delta":{"role":"assistant"}}],"usage":null}"#)).unwrap(), Step::Skip);
    }

    #[test]
    fn the_estimate_is_one_token_per_four_characters_rounded_up() {
        assert_eq!([0, 1, 4, 5, 400].map(estimate_tokens), [0, 1, 1, 2, 100]);
    }

    #[test]
    fn merging_usage_keeps_what_was_reported_and_takes_what_is_new() {
        let mut u = Usage { input: Some(100), cache_read: Some(64), ..Default::default() };
        u.merge(&Usage { output: Some(7), ..Default::default() });
        u.merge(&Usage { output: Some(50), ..Default::default() }); // a running total replaces
        assert_eq!(u, Usage { input: Some(100), output: Some(50), cache_read: Some(64), cache_write: None, estimated: false });
    }

    #[test]
    fn anthropic_without_key_explains_how_to_fix_it() {
        let mut a = provider(ProviderKind::Anthropic);
        a.api_key = String::new();
        a.api_key_env = "ASKBAR_DEFINITELY_UNSET_VAR".into();
        let err = build_request(&http_client(), &a, "", &[m("user", "x")]).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("ASKBAR_DEFINITELY_UNSET_VAR") && msg.contains("config.yaml"), "{msg}");
    }

    fn ev(data: &str) -> SseEvent {
        SseEvent { event: None, data: data.into() }
    }

    #[test]
    fn decode_openai() {
        let k = ProviderKind::OpenAi;
        assert_eq!(
            decode_event(k, &ev(r#"{"choices":[{"delta":{"content":"Hi"}}]}"#)).unwrap(),
            Step::Delta("Hi".into())
        );
        // role-only first chunk, empty content, and non-JSON all skip
        assert_eq!(decode_event(k, &ev(r#"{"choices":[{"delta":{"role":"assistant"}}]}"#)).unwrap(), Step::Skip);
        assert_eq!(decode_event(k, &ev(r#"{"choices":[{"delta":{"content":""}}]}"#)).unwrap(), Step::Skip);
        assert_eq!(decode_event(k, &ev("not json")).unwrap(), Step::Skip);
        assert_eq!(decode_event(k, &ev("[DONE]")).unwrap(), Step::Done);
        let err = decode_event(k, &ev(r#"{"error":{"message":"rate limited"}}"#)).unwrap_err();
        assert!(err.to_string().contains("rate limited"));
    }

    #[test]
    fn decode_anthropic() {
        let k = ProviderKind::Anthropic;
        assert_eq!(
            decode_event(k, &ev(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Yo"}}"#))
                .unwrap(),
            Step::Delta("Yo".into())
        );
        for skip in [
            r#"{"type":"message_start","message":{}}"#,
            r#"{"type":"ping"}"#,
            r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{"}}"#,
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        ] {
            assert_eq!(decode_event(k, &ev(skip)).unwrap(), Step::Skip, "{skip}");
        }
        assert_eq!(decode_event(k, &ev(r#"{"type":"message_stop"}"#)).unwrap(), Step::Done);
        // Usage: input and cache counts with message_start, the output total with message_delta.
        let start = r#"{"type":"message_start","message":{"id":"m","usage":{"input_tokens":25,"output_tokens":1,"cache_read_input_tokens":10,"cache_creation_input_tokens":3}}}"#;
        assert_eq!(
            decode_event(k, &ev(start)).unwrap(),
            Step::Usage(Usage { input: Some(25), output: None, cache_read: Some(10), cache_write: Some(3), estimated: false })
        );
        let delta = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":15}}"#;
        assert_eq!(decode_event(k, &ev(delta)).unwrap(), Step::Usage(Usage { output: Some(15), ..Default::default() }));
        // Without the cache fields they stay unknown rather than 0.
        let bare = r#"{"type":"message_start","message":{"usage":{"input_tokens":7}}}"#;
        assert!(matches!(decode_event(k, &ev(bare)).unwrap(), Step::Usage(u) if u.input == Some(7) && u.cache_read.is_none() && u.cache_write.is_none()));
        let err = decode_event(k, &ev(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#))
            .unwrap_err();
        assert!(err.to_string().contains("Overloaded"));
    }

    #[test]
    fn api_error_extraction() {
        assert_eq!(extract_api_error(r#"{"error":{"message":"bad key"}}"#), "bad key");
        assert_eq!(extract_api_error(r#"{"error":"plain"}"#), "plain");
        assert_eq!(extract_api_error(r#"{"detail":"nope"}"#), "nope");
        assert_eq!(extract_api_error("  Bad Gateway "), "Bad Gateway");
        assert_eq!(extract_api_error(""), "(empty response)");
        assert!(extract_api_error(&"z".repeat(1000)).chars().count() <= 301);
    }
}
