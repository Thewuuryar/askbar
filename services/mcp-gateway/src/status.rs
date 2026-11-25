//! `GET /status`: which configuration runs, which process it is, and how many tool calls ended in each outcome.
//! Counts only: no key, argument or URL is ever held here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub config_version: Option<u64>,
    pub boot_id: String,
    /// Calls by outcome, e.g. `refused_not_configured`; an outcome appears after its first call.
    pub calls: BTreeMap<String, u64>,
}
