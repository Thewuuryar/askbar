//! What the server keeps of conversation text going to a model: messages, the history sent with a question, and the token
//! counts of a reply. Nothing here calls a provider: that is `model-gateway`'s (ADR 0054). Text in, text out, and no tool
//! calling anywhere, so a model can never be handed a way to touch the machine.

pub use askbar_proto::DEFAULT_SYSTEM_PROMPT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: &str, content: &str) -> Self {
        Self { role: role.into(), content: content.into() }
    }
}

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

/// Drop empty messages and leading assistant turns, and never send two messages with the same role
/// in a row (some APIs reject that).
///
/// Two user messages in a row mean the earlier one got no reply: its turn failed, or was stopped,
/// timed out or interrupted before any text. Only the last of them is kept. Sending the unanswered
/// ones again would repeat whatever made them fail (a question the provider refuses would stick the
/// conversation for good) and mix old questions into the new one; the person retries a question on
/// purpose instead. Consecutive assistant messages are joined.
pub fn normalize(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    let mut out: Vec<ChatMessage> = Vec::new();
    for m in messages {
        if m.content.trim().is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == m.role && m.role == "user" => *last = m.clone(),
            Some(last) if last.role == m.role => {
                last.content.push_str("\n\n");
                last.content.push_str(&m.content);
            }
            None if m.role != "user" => continue,
            _ => out.push(m.clone()),
        }
    }
    out
}

/// Of a `normalize`d history (question, reply, ..., the new question), the new question and the
/// `turns` question-and-reply pairs before it. 0 keeps everything.
pub fn last_turns(messages: Vec<ChatMessage>, turns: u32) -> Vec<ChatMessage> {
    let keep = (turns as usize).saturating_mul(2).saturating_add(1);
    if turns == 0 || messages.len() <= keep {
        return messages;
    }
    messages[messages.len() - keep..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(role: &str, c: &str) -> ChatMessage {
        ChatMessage::new(role, c)
    }

    #[test]
    fn last_turns_keeps_the_new_question_and_the_pairs_before_it() {
        let conv = || vec![m("user", "q1"), m("assistant", "a1"), m("user", "q2"), m("assistant", "a2"), m("user", "q3")];
        assert_eq!(last_turns(conv(), 0), conv(), "0 sends everything");
        assert_eq!(last_turns(conv(), 1), vec![m("user", "q2"), m("assistant", "a2"), m("user", "q3")]);
        assert_eq!(last_turns(conv(), 2), conv());
        assert_eq!(last_turns(conv(), 1000), conv(), "more than there is sends all");
        assert_eq!(last_turns(vec![m("user", "only")], 1), vec![m("user", "only")], "the new question is always sent");
        // After normalize, a failed question takes no place in the count.
        let h = normalize(&[
            m("user", "q1"),
            m("assistant", "a1"),
            m("user", "failed"),
            m("user", "q2"),
            m("assistant", "a2"),
            m("user", "q3"),
        ]);
        assert_eq!(last_turns(h, 1), vec![m("user", "q2"), m("assistant", "a2"), m("user", "q3")]);
    }

    #[test]
    fn normalize_keeps_the_last_unanswered_question_and_trims() {
        let out = normalize(&[
            m("assistant", "stray"),
            m("user", "a"),
            m("user", "b"),
            m("assistant", "  "),
            m("assistant", "c"),
            m("user", "d"),
        ]);
        assert_eq!(out, vec![m("user", "b"), m("assistant", "c"), m("user", "d")], "a got no reply, so only b is sent");
        assert!(normalize(&[m("assistant", "only")]).is_empty());
        // A failed question is not carried into the next turn.
        let out = normalize(&[m("user", "hello"), m("assistant", "hi"), m("user", "fail"), m("user", "429"), m("user", "hello again")]);
        assert_eq!(out, vec![m("user", "hello"), m("assistant", "hi"), m("user", "hello again")]);
        // Two assistant messages in a row are still joined.
        assert_eq!(normalize(&[m("user", "q"), m("assistant", "x"), m("assistant", "y")]), vec![m("user", "q"), m("assistant", "x\n\ny")]);
    }

    #[test]
    fn merging_usage_keeps_what_was_reported_and_takes_what_is_new() {
        let mut u = Usage { input: Some(100), cache_read: Some(64), ..Default::default() };
        u.merge(&Usage { output: Some(7), ..Default::default() });
        u.merge(&Usage { output: Some(50), ..Default::default() }); // a running total replaces
        assert_eq!(u, Usage { input: Some(100), output: Some(50), cache_read: Some(64), cache_write: None, estimated: false });
    }
}
