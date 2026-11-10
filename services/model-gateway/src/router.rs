//! What a failed try means for the gateway: retry, tell the administrator, or give up (ADR 0014: only the
//! provider asked for is tried, and there is no fallback to another).

use crate::llm::ChatError;
pub use askbar_proto::gateway::FailureClass as Failure;

/// Sorts a failure into what to do about it.
pub fn classify(e: &ChatError) -> Failure {
    match e {
        ChatError::Connect { .. } | ChatError::Stream(_) => Failure::Retryable,
        ChatError::Api { status, .. } => match *status {
            401 | 403 => Failure::AuthFailed,
            408 | 429 | 500..=599 => Failure::Retryable,
            _ => Failure::Fatal,
        },
        // "No API key for provider ...": the same as a key that is refused.
        ChatError::Config(m) if m.contains("API key") => Failure::AuthFailed,
        ChatError::Config(_) => Failure::Fatal,
    }
}

/// A short stable code for why a request failed: what the server and the audit trail get instead of the
/// provider's words.
pub fn error_code(e: &ChatError) -> String {
    match e {
        ChatError::Config(_) => "config".into(),
        ChatError::Connect { .. } => "connect".into(),
        ChatError::Api { status, .. } => format!("http_{status}"),
        ChatError::Stream(_) => "stream".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(status: u16) -> ChatError {
        ChatError::Api { provider: "p".into(), status, message: "m".into() }
    }

    #[test]
    fn each_status_has_its_class() {
        for status in [408, 429, 500, 502, 503, 504, 599] {
            assert_eq!(classify(&api(status)), Failure::Retryable, "{status}");
        }
        for status in [401, 403] {
            assert_eq!(classify(&api(status)), Failure::AuthFailed, "{status}");
        }
        for status in [400, 404, 405, 409, 413, 422, 451] {
            assert_eq!(classify(&api(status)), Failure::Fatal, "{status}");
        }
    }

    #[test]
    fn network_failures_retry_and_a_missing_key_counts_as_a_refused_one() {
        assert_eq!(classify(&ChatError::Connect { url: "u".into(), message: "refused".into() }), Failure::Retryable);
        assert_eq!(classify(&ChatError::Stream("connection reset".into())), Failure::Retryable);
        assert_eq!(classify(&ChatError::Config("No API key for provider \"x\".".into())), Failure::AuthFailed);
        assert_eq!(classify(&ChatError::Config("Nothing to send.".into())), Failure::Fatal);
    }

    #[test]
    fn a_code_names_the_class_of_failure_and_never_the_words() {
        assert_eq!(error_code(&api(429)), "http_429");
        assert_eq!(error_code(&ChatError::Stream("sk-secret in here".into())), "stream");
        assert_eq!(error_code(&ChatError::Connect { url: "http://x".into(), message: "m".into() }), "connect");
    }
}
