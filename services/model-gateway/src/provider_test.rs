//! "Test" for a provider (ADR 0014, plan step 3.4): one short fixed message, streamed through the provider, and how it
//! went: pass or fail, the time to the first text, and a details view of the request that was made.
//!
//! The details hold the method and path, the status code, an error type and the timings, and nothing else: never the
//! key, the reply, or a provider's error text, which can echo what was sent. The words in `message` are ours. Nothing of
//! the result is stored. The audit record is metadata only: a turn span named `provider_test`, on record before the
//! provider is called like any model call, finished with the outcome and the first-text time and no payload.

use crate::llm::{messages_url, stream_chat_report, ChatError, ChatMessage};
use crate::{finish, start_of, Gateway, OpenTurn, STALE_TURN_GRACE};
use askbar_proto::audit::SpanKind;
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// How long a test may take before it is called a failure.
const LIMIT: Duration = Duration::from_secs(30);
/// What is sent: fixed, so a test never carries anything of a person's.
const PROMPT: &str = "Reply with the single word: ok";

/// The request that was made and what came of it. Secrets never appear.
#[derive(Serialize)]
pub struct TestDetails {
    pub method: &'static str,
    /// The path of the request only: no host, query or credentials.
    pub path: String,
    /// The HTTP status of the answer, when there was one.
    pub status: Option<u16>,
    /// What went wrong, as a short word: `connect`, `auth`, `not_found`, `rate_limited`, `rejected`, `provider_error`,
    /// `http_error`, `stream`, `config`, `empty`, `timeout` or `audit`. Absent on a pass.
    pub error_type: Option<&'static str>,
    pub ttft_ms: Option<u64>,
    pub total_ms: u64,
}

#[derive(Serialize)]
pub struct TestResult {
    pub ok: bool,
    /// A sentence of ours. Never the reply, never a provider's own words, never the key.
    pub message: String,
    /// Time from sending the request to the first text, on a pass.
    pub ttft_ms: Option<u64>,
    pub details: TestDetails,
}

/// `None` if there is no provider with this id.
pub async fn run(gw: &Gateway, id: &str) -> Option<TestResult> {
    let provider = gw.provider(id)?;
    let path = path_of(&messages_url(&provider));
    let fail = |why: &'static str, message: &str, status: Option<u16>, total: Duration| TestResult {
        ok: false,
        message: message.into(),
        ttft_ms: None,
        details: TestDetails { method: "POST", path: path.clone(), status, error_type: Some(why), ttft_ms: None, total_ms: ms(total) },
    };

    // On record before the provider is called, like every model call (ADR 0010 rule 2).
    let span = askbar_service::ids::new_id();
    let mut start = start_of(&span, &span, None, SpanKind::Turn, "provider_test");
    start.provider_id = Some(id.to_string());
    start.model = Some(provider.model.clone());
    if gw.audit.open(&start).await.is_err() {
        return Some(fail(
            "audit",
            "The audit record could not be made in time, so nothing was sent to the provider. Try again in a few seconds.",
            None,
            Duration::ZERO,
        ));
    }
    gw.turns
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(OpenTurn { span_id: span.clone(), deadline: Instant::now() + LIMIT + STALE_TURN_GRACE });

    let (_keep, cancel) = oneshot::channel::<()>();
    let mut got_text = false;
    let prompt = [ChatMessage::new("user", PROMPT)];
    let run = stream_chat_report(gw.http(), &provider, "", &prompt, cancel, |d| {
        got_text |= !d.trim().is_empty();
    });
    let started = Instant::now();
    let outcome = tokio::time::timeout(LIMIT, run).await;
    let result = match outcome {
        Ok(Ok(report)) if got_text => {
            let ttft = report.time_to_first_token.map(ms);
            TestResult {
                ok: true,
                message: match ttft {
                    Some(t) => format!("Streaming works. The first text came after {t} ms."),
                    None => "Streaming works.".into(),
                },
                ttft_ms: ttft,
                details: TestDetails {
                    method: "POST",
                    path: path.clone(),
                    status: report.http_status,
                    error_type: None,
                    ttft_ms: ttft,
                    total_ms: ms(report.total),
                },
            }
        }
        Ok(Ok(report)) => fail("empty", "The provider answered but sent no text.", report.http_status, report.total),
        Ok(Err(e)) => {
            let (why, message) = describe(&e.error);
            fail(why, message, e.report.http_status, e.report.total)
        }
        Err(_) => fail("timeout", "Timed out after 30 seconds.", None, started.elapsed()),
    };

    // Metadata only: the outcome, the first-text time and the status. No payload is ever appended.
    gw.turns.lock().unwrap_or_else(|e| e.into_inner()).retain(|t| t.span_id != span);
    let mut end = finish(if result.ok { "done" } else { "error" });
    end.provider_id = Some(id.to_string());
    end.model = Some(provider.model.clone());
    end.ttft_ms = result.ttft_ms;
    end.error_code = result.details.error_type.map(str::to_string);
    end.attrs = Some(serde_json::json!({ "http_status": result.details.status, "total_ms": result.details.total_ms, "streamed": true }));
    gw.audit.finish(&span, end);
    Some(result)
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// The path of a URL and nothing else.
fn path_of(url: &str) -> String {
    reqwest::Url::parse(url).map(|u| u.path().to_string()).unwrap_or_else(|_| "/".into())
}

/// An error type and a sentence of ours for what went wrong. A provider's own text is never used: it can echo the key.
fn describe(e: &ChatError) -> (&'static str, &'static str) {
    match e {
        ChatError::Config(_) => ("config", "The provider is not set up completely (a key may be missing)."),
        ChatError::Connect { .. } => ("connect", "Could not reach the provider's address. Check the address and that the provider is up."),
        ChatError::Stream(_) => ("stream", "The reply stopped part way."),
        ChatError::Api { status, .. } => match *status {
            401 | 403 => ("auth", "The provider refused the key."),
            404 => ("not_found", "The provider has no such address or model."),
            429 => ("rate_limited", "The provider is limiting requests right now."),
            400 | 422 => ("rejected", "The provider refused the request."),
            500..=599 => ("provider_error", "The provider had an error of its own."),
            _ => ("http_error", "The provider answered with an unexpected status."),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_path_of_the_request_is_kept() {
        assert_eq!(path_of("https://user:pw@api.example.com/v1/chat/completions?key=sk-x"), "/v1/chat/completions");
        assert_eq!(path_of("not a url"), "/");
    }

    #[test]
    fn every_failure_has_a_type_and_a_sentence_that_repeats_nothing_from_the_provider() {
        let api = ChatError::Api { provider: "p".into(), status: 401, message: "bad key sk-abcdef".into() };
        assert_eq!(describe(&api), ("auth", "The provider refused the key."));
        for status in [400, 403, 404, 418, 429, 500, 503] {
            let (why, message) = describe(&ChatError::Api { provider: "p".into(), status, message: "sk-abcdef".into() });
            assert!(!why.is_empty() && !message.contains("sk-abcdef"));
        }
        assert_eq!(describe(&ChatError::Connect { url: "http://x".into(), message: "refused".into() }).0, "connect");
    }
}
