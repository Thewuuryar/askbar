//! "Which models does this provider offer?" Asks the provider's own model list, from the server (where the
//! keys are and the network is), so the admin can offer a dropdown. Shared by the server API.

use crate::llm::{Provider, ProviderKind};
use crate::{store, AppState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);
/// The most bytes read from one page of a provider's answer.
const MAX_BODY: usize = 4 * 1024 * 1024;
/// The most models returned.
pub const MAX_MODELS: usize = 1000;
/// Anthropic pages its list; follow at most this many pages.
const MAX_PAGES: usize = 5;

#[derive(Debug, Deserialize)]
pub struct ModelsRequest {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    /// Used once for this request; never stored or logged.
    #[serde(default)]
    pub api_key: Option<String>,
    /// A saved provider whose stored key may be used.
    #[serde(default)]
    pub provider_id: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ModelsResult {
    pub ok: bool,
    /// On failure, why (never contains the key).
    pub message: String,
    pub models: Vec<ModelInfo>,
}

impl ModelsResult {
    fn fail(message: impl Into<String>) -> ModelsResult {
        ModelsResult { ok: false, message: message.into(), models: vec![] }
    }
}

fn same_url(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// The URL of the model list (first page).
fn list_url(p: &Provider) -> String {
    let base = p.effective_base_url();
    match p.kind {
        ProviderKind::OpenAi => format!("{base}/models"),
        ProviderKind::Anthropic if base.ends_with("/v1") => format!("{base}/models"),
        ProviderKind::Anthropic => format!("{base}/v1/models"),
    }
}

/// Models from one response body, and the id to continue from when the provider says there are more.
fn parse(body: &Value) -> (Vec<ModelInfo>, Option<String>) {
    let items = body["data"].as_array().or_else(|| body["models"].as_array()).cloned().unwrap_or_default();
    let models = items
        .iter()
        .filter_map(|m| {
            let id = m["id"].as_str().or_else(|| m["name"].as_str())?.trim();
            if id.is_empty() {
                return None;
            }
            let label = m["display_name"].as_str().map(str::trim).filter(|l| !l.is_empty() && *l != id).map(String::from);
            Some(ModelInfo { id: id.to_string(), label })
        })
        .collect();
    let next = if body["has_more"].as_bool() == Some(true) { body["last_id"].as_str().map(String::from) } else { None };
    (models, next)
}

fn finish(mut all: Vec<ModelInfo>) -> Vec<ModelInfo> {
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all.dedup_by(|a, b| a.id == b.id);
    all.truncate(MAX_MODELS);
    all
}

fn redact(msg: &str, key: &str) -> String {
    if key.len() >= 6 {
        msg.replace(key, "[key hidden]")
    } else {
        msg.to_string()
    }
}

async fn read_limited(mut r: reqwest::Response) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(|e| e.without_url().to_string())? {
        out.extend_from_slice(&chunk);
        if out.len() > MAX_BODY {
            return Err("The provider's model list is too large.".into());
        }
    }
    Ok(out)
}

async fn fetch(http: &reqwest::Client, p: &Provider, key: &str) -> Result<Vec<ModelInfo>, String> {
    let mut all = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut url = list_url(p);
        if p.kind == ProviderKind::Anthropic {
            url.push_str("?limit=1000");
            if let Some(a) = &after {
                url.push_str(&format!("&after_id={}", a.replace(|c: char| !(c.is_ascii_alphanumeric() || "-_.".contains(c)), "")));
            }
        }
        let mut req = http.get(&url).timeout(TIMEOUT);
        match p.kind {
            ProviderKind::OpenAi if !key.is_empty() => req = req.bearer_auth(key),
            ProviderKind::OpenAi => {}
            ProviderKind::Anthropic => {
                if key.is_empty() {
                    return Err("An API key is needed to list Anthropic models.".into());
                }
                req = req.header("x-api-key", key).header("anthropic-version", "2023-06-01");
            }
        }
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                "The provider did not answer within 10 seconds.".to_string()
            } else {
                format!("Could not reach the provider: {}", e.without_url())
            }
        })?;
        let status = resp.status();
        let bytes = read_limited(resp).await?;
        let text = String::from_utf8_lossy(&bytes);
        if !status.is_success() {
            return Err(format!("The provider answered {}: {}", status.as_u16(), crate::llm::extract_api_error(&text)));
        }
        let json: Value = serde_json::from_str(&text).map_err(|_| "The provider's model list was not understood.".to_string())?;
        let (models, next) = parse(&json);
        all.extend(models);
        match next {
            Some(n) if p.kind == ProviderKind::Anthropic => after = Some(n),
            _ => break,
        }
    }
    Ok(finish(all))
}

/// List the models for `req`. A saved provider's key is used only when no key was given and the address is
/// the one saved with it, so the key can never be sent somewhere the saved provider does not point.
pub async fn run(state: &AppState, req: ModelsRequest) -> rusqlite::Result<ModelsResult> {
    let kind = match req.kind.as_str() {
        "openai" => ProviderKind::OpenAi,
        "anthropic" => ProviderKind::Anthropic,
        _ => return Ok(ModelsResult::fail("Type must be openai or anthropic.")),
    };
    let url = req.base_url.trim();
    if !url.is_empty() && !(url.starts_with("http://") || url.starts_with("https://")) {
        return Ok(ModelsResult::fail("Base URL must start with http:// or https://."));
    }
    let mut key = req.api_key.as_deref().map(str::trim).unwrap_or("").to_string();
    let mut headers = Default::default();
    if let Some(id) = req.provider_id.as_deref() {
        let saved = {
            let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
            let secrets = state.secrets();
            store::load_provider(&db, &secrets, id)?
        };
        let Some(saved) = saved else {
            return Ok(ModelsResult::fail("No such provider."));
        };
        let same = saved.kind == kind
            && same_url(&saved.effective_base_url(), &Provider { base_url: url.to_string(), kind, ..saved.clone() }.effective_base_url());
        if same {
            headers = saved.headers.clone();
            if key.is_empty() {
                key = saved.api_key.clone();
            }
        }
    }
    let p = Provider {
        name: String::new(),
        kind,
        base_url: url.to_string(),
        api_key: key.clone(),
        api_key_env: String::new(),
        model: String::new(),
        max_tokens: None,
        temperature: None,
        headers,
    };
    Ok(match fetch(&state.http, &p, &key).await {
        Ok(models) if models.is_empty() => ModelsResult::fail("The provider lists no models."),
        Ok(models) => ModelsResult { ok: true, message: String::new(), models },
        Err(e) => ModelsResult::fail(redact(&e, &key)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openai_and_anthropic_lists_parse() {
        let (m, next) = parse(&json!({"data":[{"id":"gpt-b"},{"id":"gpt-a"},{"id":" "},{"object":"x"}]}));
        assert_eq!(m.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["gpt-b", "gpt-a"]);
        assert!(next.is_none());
        let (m, next) = parse(&json!({"data":[{"id":"claude-x","display_name":"Claude X"}],"has_more":true,"last_id":"claude-x"}));
        assert_eq!(m, vec![ModelInfo { id: "claude-x".into(), label: Some("Claude X".into()) }]);
        assert_eq!(next.as_deref(), Some("claude-x"));
    }

    #[test]
    fn lists_are_sorted_and_deduplicated() {
        let v = finish(vec![
            ModelInfo { id: "b".into(), label: None },
            ModelInfo { id: "a".into(), label: None },
            ModelInfo { id: "b".into(), label: None },
        ]);
        assert_eq!(v.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }

    #[test]
    fn urls() {
        let p = |kind, base: &str| Provider {
            name: String::new(),
            kind,
            base_url: base.into(),
            api_key: String::new(),
            api_key_env: String::new(),
            model: String::new(),
            max_tokens: None,
            temperature: None,
            headers: Default::default(),
        };
        assert_eq!(list_url(&p(ProviderKind::OpenAi, "http://cpa:8317/v1/")), "http://cpa:8317/v1/models");
        assert_eq!(list_url(&p(ProviderKind::OpenAi, "")), "https://api.openai.com/v1/models");
        assert_eq!(list_url(&p(ProviderKind::Anthropic, "")), "https://api.anthropic.com/v1/models");
        assert_eq!(list_url(&p(ProviderKind::Anthropic, "https://x.test/v1")), "https://x.test/v1/models");
    }

    #[test]
    fn redaction() {
        assert_eq!(redact("bad sk-secret-1", "sk-secret-1"), "bad [key hidden]");
    }
}
