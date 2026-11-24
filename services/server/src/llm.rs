//! What the server keeps of conversation text going to a model: messages, the history sent with a question, and the token
//! counts of a reply. Nothing here calls a provider: that is `model-gateway`'s (ADR 0054). Text in, text out, and no tool
//! calling anywhere, so a model can never be handed a way to touch the machine.

pub use askbar_proto::DEFAULT_SYSTEM_PROMPT;

pub use askbar_proto::gateway::ChatMessage;

/// Tokens a reply used, as the provider reported them (or estimated, see `estimated`).
/// A field the provider did not report is `None`, not 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
    /// Prompt tokens served from the provider's cache.
    pub cache_read: Option<u64>,
    /// Prompt tokens written to the provider's cache.
    pub cache_write: Option<u64>,
    /// True when the numbers are our own estimate, not the provider's.
    pub estimated: bool,
}

impl Usage {
    /// Takes every field `other` reports, keeping ours where it reports nothing.
    pub fn merge(&mut self, other: &Usage) {
        for (mine, theirs) in [
            (&mut self.input, other.input),
            (&mut self.output, other.output),
            (&mut self.cache_read, other.cache_read),
            (&mut self.cache_write, other.cache_write),
        ] {
            if theirs.is_some() {
                *mine = theirs;
            }
        }
        self.estimated |= other.estimated;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_usage_keeps_what_was_reported_and_takes_what_is_new() {
        let mut u = Usage { input: Some(100), cache_read: Some(64), ..Default::default() };
        u.merge(&Usage { output: Some(7), ..Default::default() });
        u.merge(&Usage { output: Some(50), ..Default::default() }); // a running total replaces
        assert_eq!(u, Usage { input: Some(100), output: Some(50), cache_read: Some(64), cache_write: None, estimated: false });
    }
}
