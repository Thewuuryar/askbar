//! One error shape for every API: RFC 9457 problem details (`application/problem+json`).
//!
//! `error` is the stable code a program branches on; `title` is fixed per code; `detail` is the
//! sentence for a person; extension members (for example `period`) ride alongside. Codes are only
//! ever added, never renamed (see "Within v1" in docs/architecture.md).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How long a client is told to wait before sending again when the audit store is backed up.
pub const AUDIT_BACKLOG_RETRY_SECS: u64 = 5;

/// The media type of every error body.
pub const CONTENT_TYPE: &str = "application/problem+json";

/// Every error the APIs can answer with. The registry is this enum; the OpenAPI files list the same codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    QueueFull,
    BudgetExceeded,
    KillSwitch,
    AuditKeyMissing,
    AuditBacklog,
    ClientTooOld,
    ClientVersionMissing,
    Unauthorized,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    BadRequest,
    PayloadTooLarge,
    TooManyAttempts,
    ApiKeyExists,
    SetupRequired,
    ServerUnavailable,
    Conflict,
    Internal,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 19] = [
        ErrorCode::QueueFull,
        ErrorCode::BudgetExceeded,
        ErrorCode::KillSwitch,
        ErrorCode::AuditKeyMissing,
        ErrorCode::AuditBacklog,
        ErrorCode::ClientTooOld,
        ErrorCode::ClientVersionMissing,
        ErrorCode::Unauthorized,
        ErrorCode::Forbidden,
        ErrorCode::NotFound,
        ErrorCode::MethodNotAllowed,
        ErrorCode::BadRequest,
        ErrorCode::PayloadTooLarge,
        ErrorCode::TooManyAttempts,
        ErrorCode::ApiKeyExists,
        ErrorCode::SetupRequired,
        ErrorCode::ServerUnavailable,
        ErrorCode::Conflict,
        ErrorCode::Internal,
    ];

    /// The stable code, as it appears in the `error` member.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::QueueFull => "queue_full",
            ErrorCode::BudgetExceeded => "budget_exceeded",
            ErrorCode::KillSwitch => "kill_switch",
            ErrorCode::AuditKeyMissing => "audit_key_missing",
            ErrorCode::AuditBacklog => "audit_backlog",
            ErrorCode::ClientTooOld => "client_too_old",
            ErrorCode::ClientVersionMissing => "client_version_missing",
            ErrorCode::Unauthorized => "unauthorized",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::MethodNotAllowed => "method_not_allowed",
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::PayloadTooLarge => "payload_too_large",
            ErrorCode::TooManyAttempts => "too_many_attempts",
            ErrorCode::ApiKeyExists => "api_key_exists",
            ErrorCode::SetupRequired => "setup_required",
            ErrorCode::ServerUnavailable => "server_unavailable",
            ErrorCode::Conflict => "conflict",
            ErrorCode::Internal => "internal",
        }
    }

    /// The HTTP status that goes with the code.
    pub fn status(self) -> u16 {
        match self {
            ErrorCode::QueueFull | ErrorCode::ApiKeyExists | ErrorCode::Conflict => 409,
            ErrorCode::BudgetExceeded | ErrorCode::TooManyAttempts => 429,
            ErrorCode::KillSwitch | ErrorCode::AuditKeyMissing | ErrorCode::AuditBacklog | ErrorCode::SetupRequired => 503,
            ErrorCode::ClientTooOld => 426,
            ErrorCode::ClientVersionMissing | ErrorCode::BadRequest => 400,
            ErrorCode::Unauthorized => 401,
            ErrorCode::Forbidden => 403,
            ErrorCode::NotFound => 404,
            ErrorCode::MethodNotAllowed => 405,
            ErrorCode::PayloadTooLarge => 413,
            ErrorCode::ServerUnavailable => 502,
            ErrorCode::Internal => 500,
        }
    }

    /// A short fixed summary of the kind of problem (the same for every occurrence).
    pub fn title(self) -> &'static str {
        match self {
            ErrorCode::QueueFull => "Too many messages are waiting",
            ErrorCode::BudgetExceeded => "A usage limit has been reached",
            ErrorCode::KillSwitch => "Replies are switched off",
            ErrorCode::AuditKeyMissing => "The audit key has not been set up",
            ErrorCode::AuditBacklog => "The server is busy recording",
            ErrorCode::ClientTooOld => "This app is too old for the server",
            ErrorCode::ClientVersionMissing => "The app version is missing",
            ErrorCode::Unauthorized => "Not signed in",
            ErrorCode::Forbidden => "Not allowed",
            ErrorCode::NotFound => "Not found",
            ErrorCode::MethodNotAllowed => "Method not allowed",
            ErrorCode::BadRequest => "Bad request",
            ErrorCode::PayloadTooLarge => "Request too large",
            ErrorCode::TooManyAttempts => "Too many attempts",
            ErrorCode::ApiKeyExists => "An API key already exists",
            ErrorCode::SetupRequired => "Setup is not finished",
            ErrorCode::ServerUnavailable => "The Askbar server cannot be reached",
            ErrorCode::Conflict => "Conflict",
            ErrorCode::Internal => "Something went wrong on the server",
        }
    }

    /// The `type` member: a URN, so nothing needs to be hosted.
    pub fn type_urn(self) -> String {
        format!("urn:askbar:problem:{}", self.as_str())
    }

    /// Looks a code up by its `error` string.
    pub fn parse(s: &str) -> Option<ErrorCode> {
        ErrorCode::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// The body of an error response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    /// A sentence for a person.
    pub detail: String,
    /// The stable code (`queue_full`, ...). Programs branch on this, never on `detail`.
    pub error: String,
    /// Extension members such as `period`.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Problem {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Problem {
        Problem { kind: code.type_urn(), title: code.title().into(), status: code.status(), detail: detail.into(), error: code.as_str().into(), extra: Map::new() }
    }

    /// Adds an extension member.
    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Problem {
        self.extra.insert(key.into(), value.into());
        self
    }

    /// The registry entry for `error`, if it is a known code.
    pub fn code(&self) -> Option<ErrorCode> {
        ErrorCode::parse(&self.error)
    }

    pub fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("a problem serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_problem_has_the_rfc_members_the_code_and_extensions() {
        let p = Problem::new(ErrorCode::BudgetExceeded, "The usage limit for today has been reached.").with("period", "daily");
        let v: Value = serde_json::from_slice(&p.to_json_bytes()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "urn:askbar:problem:budget_exceeded",
                "title": "A usage limit has been reached",
                "status": 429,
                "detail": "The usage limit for today has been reached.",
                "error": "budget_exceeded",
                "period": "daily",
            })
        );
        let back: Problem = serde_json::from_value(v).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.extra["period"], "daily");
        assert_eq!(back.code(), Some(ErrorCode::BudgetExceeded));
    }

    #[test]
    fn codes_are_unique_lowercase_snake_and_round_trip() {
        let mut seen = std::collections::BTreeSet::new();
        for c in ErrorCode::ALL {
            let s = c.as_str();
            assert!(s.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_'), "{s}");
            assert!(seen.insert(s), "duplicate code {s}");
            assert_eq!(ErrorCode::parse(s), Some(c));
            assert_eq!(c.type_urn(), format!("urn:askbar:problem:{s}"));
            assert!(!c.title().is_empty());
        }
        assert_eq!(ErrorCode::parse("nope"), None);
    }

    #[test]
    fn statuses_are_the_ones_the_apis_already_used() {
        let status = |c: ErrorCode| c.status();
        assert_eq!(status(ErrorCode::QueueFull), 409);
        assert_eq!(status(ErrorCode::BudgetExceeded), 429);
        for c in [ErrorCode::KillSwitch, ErrorCode::AuditKeyMissing, ErrorCode::AuditBacklog, ErrorCode::SetupRequired] {
            assert_eq!(status(c), 503, "{c:?}");
        }
        assert_eq!(status(ErrorCode::ClientTooOld), 426);
        assert_eq!(status(ErrorCode::ClientVersionMissing), 400);
        assert_eq!(status(ErrorCode::Unauthorized), 401);
        assert_eq!(status(ErrorCode::NotFound), 404);
        assert_eq!(status(ErrorCode::TooManyAttempts), 429);
        assert_eq!(status(ErrorCode::ApiKeyExists), 409);
        assert_eq!(status(ErrorCode::ServerUnavailable), 502);
        assert_eq!(status(ErrorCode::Internal), 500);
    }

    #[test]
    fn every_code_is_listed_in_all() {
        // Adding a variant without adding it to ALL makes this match non-exhaustive at compile time.
        for c in ErrorCode::ALL {
            match c {
                ErrorCode::QueueFull | ErrorCode::BudgetExceeded | ErrorCode::KillSwitch | ErrorCode::AuditKeyMissing | ErrorCode::AuditBacklog
                | ErrorCode::ClientTooOld | ErrorCode::ClientVersionMissing | ErrorCode::Unauthorized | ErrorCode::Forbidden | ErrorCode::NotFound
                | ErrorCode::MethodNotAllowed | ErrorCode::BadRequest | ErrorCode::PayloadTooLarge | ErrorCode::TooManyAttempts
                | ErrorCode::ApiKeyExists | ErrorCode::SetupRequired | ErrorCode::ServerUnavailable | ErrorCode::Conflict | ErrorCode::Internal => {}
            }
        }
        assert_eq!(ErrorCode::ALL.len(), 19);
    }
}
