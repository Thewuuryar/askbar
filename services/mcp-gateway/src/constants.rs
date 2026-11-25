//! Fixed limits of the front that no push changes.

/// One-time IDs held at once. A full store refuses new approvals (fail closed) rather than forget one; at the
/// 120 s default window this is a rate no household or small team reaches.
pub const MAX_ONE_TIME_IDS: usize = 10_000;
