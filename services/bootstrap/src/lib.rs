//! `bootstrap`: creates the per-link tokens and exits (ADR 0049, ADR 0004).
//!
//! - [`links`]: the table of links it makes tokens for.
//! - [`write`]: the token and hash files, with the owner and mode each reader needs.

pub mod links;
pub mod write;
