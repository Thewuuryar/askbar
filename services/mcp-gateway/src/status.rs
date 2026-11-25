//! `GET /status`: which configuration runs, which process it is, and how many tool calls ended in each outcome.
//! Counts, and the time and outcome word of the last search: no key, argument or URL is ever held here.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub config_version: Option<u64>,
    pub boot_id: String,
    /// Calls by outcome, e.g. `refused_not_configured`; an outcome appears after its first call.
    pub calls: BTreeMap<String, u64>,
    /// The last search request to `searxng`; absent until the first one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_search: Option<LastSearch>,
}

/// When the front last asked `searxng` and how it went: `ok` or an error code word. Never the query, a URL or any text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastSearch {
    /// Unix seconds.
    pub at: i64,
    pub outcome: String,
}
