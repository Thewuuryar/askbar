//! Bounds for the MCP gateway's tools (phase 5A). The admin pushes values within these bounds and the gateway refuses
//! one outside them. Fixed rules (allowed content types, ports, the datamark character) are not here: they are not
//! pushed, so no push can widen them. The approval expiry is `DEFAULT_APPROVAL_TTL_SECS` in the crate root.

/// Bytes one fetch reads from a response. Text pages rarely pass 1 MiB; 5 MiB caps what a push may ask for.
pub const MIN_FETCH_MAX_BYTES: u64 = 64 * 1024;
pub const DEFAULT_FETCH_MAX_BYTES: u64 = 1024 * 1024;
pub const MAX_FETCH_MAX_BYTES: u64 = 5 * 1024 * 1024;

/// Seconds one fetch may take end to end. 20 leaves room inside a 30 s tool call for a slow host and one retry-free read.
pub const MIN_FETCH_TIMEOUT_SECS: u32 = 1;
pub const DEFAULT_FETCH_TIMEOUT_SECS: u32 = 20;
pub const MAX_FETCH_TIMEOUT_SECS: u32 = 60;

/// Redirects a fetch follows before it gives up. Three covers http-to-https and canonical-host hops; each hop is checked
/// against the allowlist again. Zero means no redirects at all.
pub const MIN_FETCH_REDIRECTS: u32 = 0;
pub const DEFAULT_FETCH_REDIRECTS: u32 = 3;
pub const MAX_FETCH_REDIRECTS: u32 = 5;

/// Seconds one tool call may run. Past two minutes a call holds a model turn with nothing to show for it.
pub const MIN_TOOL_CALL_TIMEOUT_SECS: u32 = 5;
pub const DEFAULT_TOOL_CALL_TIMEOUT_SECS: u32 = 30;
pub const MAX_TOOL_CALL_TIMEOUT_SECS: u32 = 120;

/// Seconds a one-time ID handed out by a search stays usable. Short, so an ID cannot be stored and replayed later.
pub const MIN_ONE_TIME_ID_WINDOW_SECS: u32 = 10;
pub const DEFAULT_ONE_TIME_ID_WINDOW_SECS: u32 = 120;
pub const MAX_ONE_TIME_ID_WINDOW_SECS: u32 = 600;

/// Results one search returns. Eight fits a model's context; twenty is the most a push may ask for.
pub const MIN_SEARCH_RESULTS: u32 = 1;
pub const DEFAULT_SEARCH_RESULTS: u32 = 8;
pub const MAX_SEARCH_RESULTS: u32 = 20;

/// URLs a task remembers as fetchable (those a search returned). Fifty covers a few rounds of searching; the cap bounds memory.
pub const MIN_REMEMBERED_URLS: u32 = 10;
pub const DEFAULT_REMEMBERED_URLS: u32 = 50;
pub const MAX_REMEMBERED_URLS: u32 = 200;

/// The built-in tools a user can be given. Off by default (ADR 0019): a user holds a tool only when the admin pushes it.
pub const TOOLS: [&str; 2] = ["search", "fetch"];

/// Users with a permission entry, and the longest user ID. A household or small team never nears 1000 users.
pub const MAX_PERMISSION_USERS: usize = 1000;
pub const MAX_PERMISSION_USER_LEN: usize = 128;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_default_sits_within_its_bounds() {
        let bounds = [
            (MIN_FETCH_MAX_BYTES, DEFAULT_FETCH_MAX_BYTES, MAX_FETCH_MAX_BYTES),
            (u64::from(MIN_FETCH_TIMEOUT_SECS), u64::from(DEFAULT_FETCH_TIMEOUT_SECS), u64::from(MAX_FETCH_TIMEOUT_SECS)),
            (u64::from(MIN_FETCH_REDIRECTS), u64::from(DEFAULT_FETCH_REDIRECTS), u64::from(MAX_FETCH_REDIRECTS)),
            (u64::from(MIN_TOOL_CALL_TIMEOUT_SECS), u64::from(DEFAULT_TOOL_CALL_TIMEOUT_SECS), u64::from(MAX_TOOL_CALL_TIMEOUT_SECS)),
            (u64::from(MIN_ONE_TIME_ID_WINDOW_SECS), u64::from(DEFAULT_ONE_TIME_ID_WINDOW_SECS), u64::from(MAX_ONE_TIME_ID_WINDOW_SECS)),
            (u64::from(MIN_SEARCH_RESULTS), u64::from(DEFAULT_SEARCH_RESULTS), u64::from(MAX_SEARCH_RESULTS)),
            (u64::from(MIN_REMEMBERED_URLS), u64::from(DEFAULT_REMEMBERED_URLS), u64::from(MAX_REMEMBERED_URLS)),
        ];
        for (min, default, max) in bounds {
            assert!(min <= default && default <= max, "{min} <= {default} <= {max}");
        }
    }
}
