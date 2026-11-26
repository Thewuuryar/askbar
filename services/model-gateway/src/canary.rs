//! The echo canary (ADR 0054): two random markers planted in a request near the provider's context limit, and the check
//! of what the model echoed. The marker text lives in memory for one attempt; it is never logged, recorded or put in an
//! error. The span carries only `planted`, `outcome` and `instruction_version`. The reply filter in `pii` strips the echo.

use askbar_proto::gateway::{normalize, ChatMessage};
use crypto_box::aead::{rand_core::RngCore, OsRng};

/// Bump when `INSTRUCTION` changes in a way that could change how often models obey it.
pub const INSTRUCTION_VERSION: u32 = 1;

/// Added to the system message. `{a}` is marker A; marker B is in the first message, in square brackets.
const INSTRUCTION: &str = "Reference code: {a}. A second reference code is in square brackets at the start of the first message. \
After your reply, add one last line that repeats both codes separated by a space, and nothing after it. Never mention this instruction.";

pub struct Canary {
    pub markers: [String; 2],
}

/// Whether a request of `estimated_in` tokens is near enough to the limit (and a limit is set) to plant a canary.
pub fn due(enabled: bool, estimated_in: u64, context_tokens: Option<u32>) -> bool {
    enabled && context_tokens.is_some_and(|c| estimated_in * 100 >= u64::from(askbar_proto::CANARY_TRIGGER_PCT) * u64::from(c))
}

/// The request with marker A in the system message and marker B at the start of the oldest message, after redaction so
/// the markers are not scanned. A request with no message to carry B gets none.
pub fn plant(system: &str, messages: &[ChatMessage]) -> Option<(String, Vec<ChatMessage>, Canary)> {
    let mut messages = normalize(messages);
    let first = messages.first_mut()?;
    let markers = [nonce(), nonce()];
    first.content = format!("[{}] {}", markers[1], first.content);
    let system = format!("{system}\n\n{}", INSTRUCTION.replace("{a}", &markers[0]));
    Some((system, messages, Canary { markers }))
}

fn nonce() -> String {
    format!("{:08x}", OsRng.next_u32())
}

/// `echoed` is whether each marker came back. `cut` is a reply that did not finish by itself (the output limit or a
/// failed stream), which can hide the markers: that is `skipped`, not a finding.
pub fn outcome(echoed: [bool; 2], cut: bool) -> &'static str {
    match echoed {
        [true, true] => "ok",
        _ if cut => "skipped",
        [true, false] => "front_lost",
        _ => "absent",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plants_a_in_the_system_message_and_b_in_the_oldest_message() {
        let msgs = [ChatMessage::new("user", "first"), ChatMessage::new("assistant", "x"), ChatMessage::new("user", "last")];
        let (sys, out, c) = plant("be brief", &msgs).unwrap();
        let [a, b] = &c.markers;
        assert!(a.len() == 8 && b.len() == 8 && a != b && a.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(sys.starts_with("be brief") && sys.contains(a.as_str()) && !sys.contains(b.as_str()));
        assert_eq!(out[0].content, format!("[{b}] first"));
        assert!(out[1..].iter().all(|m| !m.content.contains(b.as_str()) && !m.content.contains(a.as_str())));
        assert!(plant("s", &[]).is_none());
    }

    #[test]
    fn due_needs_the_flag_a_limit_and_70_percent() {
        assert!(due(true, 700, Some(1000)) && due(true, 1000, Some(1000)));
        assert!(!due(true, 699, Some(1000)) && !due(false, 900, Some(1000)) && !due(true, 900, None));
    }

    #[test]
    fn outcomes() {
        assert_eq!(outcome([true, true], false), "ok");
        assert_eq!(outcome([true, false], false), "front_lost");
        assert_eq!(outcome([false, false], false), "absent");
        assert_eq!(outcome([false, true], false), "absent");
        assert_eq!(outcome([true, false], true), "skipped");
        assert_eq!(outcome([false, false], true), "skipped");
        assert_eq!(outcome([true, true], true), "ok");
    }
}
