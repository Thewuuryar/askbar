//! "Test" for a provider: send one short message through it and report what happened (moved from the server, ADR 0054).

use crate::llm::{stream_chat, ChatMessage};
use crate::Gateway;
use serde::Serialize;
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Serialize)]
pub struct TestResult {
    pub ok: bool,
    /// The start of the model's reply on success, or the error. Never contains the key.
    pub message: String,
}

/// `None` if there is no provider with this id.
pub async fn run(gw: &Gateway, id: &str) -> Option<TestResult> {
    let provider = gw.provider(id)?;
    let key = provider.api_key.clone();
    let (_keep, cancel) = oneshot::channel::<()>();
    let mut reply = String::new();
    let prompt = [ChatMessage::new("user", "Reply with the single word: ok")];
    let run = stream_chat(gw.http(), &provider, "", &prompt, cancel, |d| {
        if reply.len() < 200 {
            reply.push_str(d);
        }
    });
    let outcome = tokio::time::timeout(Duration::from_secs(30), run).await;
    Some(match outcome {
        Ok(Ok(_)) if !reply.trim().is_empty() => TestResult { ok: true, message: reply.trim().chars().take(120).collect() },
        Ok(Ok(_)) => TestResult { ok: false, message: "The provider answered but sent no text.".into() },
        Ok(Err(e)) => TestResult { ok: false, message: redact(&e.to_string(), &key) },
        Err(_) => TestResult { ok: false, message: "Timed out after 30 seconds.".into() },
    })
}

/// Errors sometimes echo the credential back; never pass that on.
fn redact(msg: &str, key: &str) -> String {
    if key.len() >= 6 {
        msg.replace(key, "[key hidden]")
    } else {
        msg.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn redaction_hides_the_key() {
        assert_eq!(redact("bad key sk-abcdef123 given", "sk-abcdef123"), "bad key [key hidden] given");
        assert_eq!(redact("nothing here", ""), "nothing here");
    }
}
