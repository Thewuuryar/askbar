//! "Test" for a provider: send one short message through it and report what happened.
//! Shared by the admin site and the management API.

use crate::llm::{stream_chat, ChatMessage};
use crate::{store, AppState};
use serde::Serialize;
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Serialize)]
pub struct TestResult {
    pub ok: bool,
    /// The start of the model's reply on success, or the error. Never contains the key.
    pub message: String,
}

/// `Ok(None)` if there is no provider with this id.
pub async fn run(state: &AppState, id: &str) -> rusqlite::Result<Option<TestResult>> {
    let provider = {
        let db = state.db.lock().unwrap_or_else(|e| e.into_inner());
        let secrets = state.secrets();
        match store::load_provider(&db, &secrets, id)? {
            Some(p) => p,
            None => return Ok(None),
        }
    };
    let key = provider.api_key.clone();
    let (_keep, cancel) = oneshot::channel::<()>();
    let mut reply = String::new();
    let prompt = [ChatMessage::new("user", "Reply with the single word: ok")];
    let run = stream_chat(&state.http, &provider, "", &prompt, cancel, |d| {
        if reply.len() < 200 {
            reply.push_str(d);
        }
    });
    let outcome = tokio::time::timeout(Duration::from_secs(30), run).await;
    Ok(Some(match outcome {
        Ok(Ok(_)) if !reply.trim().is_empty() => TestResult { ok: true, message: reply.trim().chars().take(120).collect() },
        Ok(Ok(_)) => TestResult { ok: false, message: "The provider answered but sent no text.".into() },
        Ok(Err(e)) => TestResult { ok: false, message: redact(&e.to_string(), &key) },
        Err(_) => TestResult { ok: false, message: "Timed out after 30 seconds.".into() },
    }))
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
