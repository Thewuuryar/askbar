//! Delivery channels. ntfy now; email follows in its own PR (ADR 0055).

pub mod ntfy;

/// Why a delivery did not work, in the alerter's own words: never the server's, which can carry a name or a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// Nothing answered.
    Unreachable,
    /// The server answered with an error.
    Refused,
    /// The server said 401 or 403: the token is wrong or missing.
    Unauthorized,
    /// No answer in time.
    Timeout,
}

impl Problem {
    pub fn as_str(self) -> &'static str {
        match self {
            Problem::Unreachable => "unreachable",
            Problem::Refused => "refused",
            Problem::Unauthorized => "unauthorized",
            Problem::Timeout => "timeout",
        }
    }
}
