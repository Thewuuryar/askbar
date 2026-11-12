//! Delivery channels: ntfy and email (ADR 0055).

pub mod ntfy;
pub mod smtp;

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
    /// The secure connection could not be made: the wrong TLS mode for the port, or a certificate that does not match the host.
    Tls,
}

impl Problem {
    pub fn as_str(self) -> &'static str {
        match self {
            Problem::Unreachable => "unreachable",
            Problem::Refused => "refused",
            Problem::Unauthorized => "unauthorized",
            Problem::Timeout => "timeout",
            Problem::Tls => "tls",
        }
    }
}
