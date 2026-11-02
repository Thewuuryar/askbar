//! Streaming chat against OpenAI-compatible and Anthropic endpoints.
//!
//! Text in, text out. There is no tool calling anywhere in this module, so a
//! model can never be handed a way to touch the machine.

use askbar_proto::{SseEvent, SseParser};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;
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
            let mut body = json!({ "model": p.model, "messages": all, "stream": true });
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
    Done,
    Skip,
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
                _ => Ok(Step::Skip),
            }
        }
        ProviderKind::Anthropic => {
            let Ok(v) = serde_json::from_str::<Value>(data) else { return Ok(Step::Skip) };
            let ty = v["type"].as_str().or(ev.event.as_deref()).unwrap_or("");
            match ty {
                "content_block_delta" if v["delta"]["type"] == "text_delta" => {
                    Ok(Step::Delta(v["delta"]["text"].as_str().unwrap_or("").to_string()))
                }
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

/// Stream one assistant reply, calling `on_delta` for each piece of text.
///
/// Resolving `cancel` (by sending on, or dropping, its sender) stops the
/// request and returns `Cancelled`. Text already delivered stays delivered.
pub async fn stream_chat<F>(
    client: &reqwest::Client,
    provider: &Provider,
    system: &str,
    messages: &[ChatMessage],
    mut cancel: oneshot::Receiver<()>,
    mut on_delta: F,
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
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(ChatError::Api {
            provider: provider.name.clone(),
            status: status.as_u16(),
            message: extract_api_error(&body),
        });
    }

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
                Step::Delta(s) => on_delta(&s),
                Step::Done => return Ok(StreamOutcome::Completed),
                Step::Skip => {}
            }
        }
    }
    for ev in parser.finish() {
        match decode_event(provider.kind, &ev)? {
            Step::Delta(s) => on_delta(&s),
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
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["content"], "hi");
        assert!(b.get("max_tokens").is_none(), "unset max_tokens must be omitted");
        p.max_tokens = Some(100);
        assert_eq!(build_body(&p, "", &msgs)["max_tokens"], 100);
        assert_eq!(build_body(&p, "", &msgs)["messages"][0]["role"], "user");

        let a = provider(ProviderKind::Anthropic);
        let b = build_body(&a, "be brief", &msgs);
        assert_eq!(b["system"], "be brief");
        assert_eq!(b["max_tokens"], 4096);
        assert_eq!(b["messages"][0]["role"], "user");
        assert!(build_body(&a, "  ", &msgs).get("system").is_none());
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
