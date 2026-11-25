//! Fixed limits of the front that no push changes.

/// One-time IDs held at once. A full store refuses new approvals (fail closed) rather than forget one; at the
/// 120 s default window this is a rate no household or small team reaches.
pub const MAX_ONE_TIME_IDS: usize = 10_000;

/// Tasks whose search results are remembered at once; the oldest task is forgotten first. A task's end is not signalled
/// to the front, so this bound is what frees the memory.
pub const MAX_REMEMBERED_TASKS: usize = 1_000;

/// The most bytes read from `searxng` for one search; a larger answer is an upstream failure.
pub const MAX_SEARXNG_BYTES: usize = 1024 * 1024;

/// Characters kept of a result's title and snippet, and the longest URL kept (a longer one is dropped).
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_SNIPPET_CHARS: usize = 500;
pub const MAX_URL_CHARS: usize = 2048;
