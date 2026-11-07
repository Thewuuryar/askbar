//! What happens when a provider fails. Only the provider the person asked for is tried, with
//! retries; there is no automatic fallback to another provider (ADR 0014).

use crate::llm::ChatError;

/// What a failed request means for the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// Worth another try on the same provider: the network, a timeout, a rate limit or the
    /// provider's own trouble (5xx), or a stream that broke.
    Retryable,
    /// The credentials are not accepted (401, 403, or none configured): another try cannot help,
    /// and the administrator should be told.
    AuthFailed,
    /// The request itself is refused (any other 4xx): the router does not retry it.
    Fatal,
}

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
    fn network_failures_and_broken_streams_retry_and_missing_keys_count_as_bad_credentials() {
        assert_eq!(classify(&ChatError::Connect { url: "u".into(), message: "refused".into() }), Failure::Retryable);
        assert_eq!(classify(&ChatError::Connect { url: "u".into(), message: "operation timed out".into() }), Failure::Retryable);
        assert_eq!(classify(&ChatError::Stream("connection reset".into())), Failure::Retryable);
        assert_eq!(classify(&ChatError::Config("No API key for provider \"x\". In config.yaml, set api_key.".into())), Failure::AuthFailed);
        assert_eq!(classify(&ChatError::Config("Nothing to send.".into())), Failure::Fatal);
    }
}
