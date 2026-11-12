//! The restart loop rule (ADR 0051): three changes of a service's boot ID within ten minutes. The admin applies it to the
//! services it polls and the auditor to the admin's hellos, so the rule has one definition. It holds no clock and sends nothing:
//! the caller passes the time and decides what to do when [`RestartLoop::saw`] says a loop has begun.

use std::collections::VecDeque;

/// Restarts that make a loop.
pub const CHANGES: usize = 3;
/// The window they must fall in, in seconds.
pub const WINDOW_SECS: i64 = 10 * 60;

#[derive(Default)]
pub struct RestartLoop {
    last: Option<String>,
    /// When the boot ID changed, within the window.
    changes: VecDeque<i64>,
    alerted_at: Option<i64>,
}

impl RestartLoop {
    /// The boot ID the service reported at `now` (unix seconds). The first one seen is where the count starts, not a restart.
    /// `Some(count)` when a loop begins: the third change within the window, and another only after a quiet window, so a
    /// loop that goes on is said once.
    pub fn saw(&mut self, boot_id: &str, now: i64) -> Option<u32> {
        if boot_id.is_empty() {
            return None;
        }
        let changed = self.last.as_deref().is_some_and(|last| last != boot_id);
        self.last = Some(boot_id.to_string());
        if !changed {
            return None;
        }
        self.changes.push_back(now);
        while self.changes.front().is_some_and(|t| *t <= now - WINDOW_SECS) {
            self.changes.pop_front();
        }
        let quiet = self.alerted_at.is_none_or(|t| now - t >= WINDOW_SECS);
        if self.changes.len() < CHANGES || !quiet {
            return None;
        }
        self.alerted_at = Some(now);
        Some(self.changes.len() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_changes_in_ten_minutes_begin_a_loop_once_and_slow_restarts_are_not_one() {
        let mut l = RestartLoop::default();
        let said: Vec<_> =
            [("a", 0), ("a", 30), ("b", 60), ("c", 120), ("d", 180), ("e", 240)].into_iter().map(|(id, t)| l.saw(id, t)).collect();
        assert_eq!(said, [None, None, None, None, Some(3), None], "the first is a start; the loop is said once");
        assert_eq!(l.saw("", 300), None, "an empty boot ID says nothing");
        let mut slow = RestartLoop::default();
        let said: Vec<_> = ["a", "b", "c", "d", "e", "f"].iter().enumerate().map(|(i, id)| slow.saw(id, i as i64 * 301)).collect();
        assert!(said.iter().all(Option::is_none), "one restart every five minutes is never three in ten");
        // After a quiet window a new loop is said again.
        let (mut t, mut ids) = (10_000, ["g", "h", "i", "j"].into_iter());
        let said: Vec<_> = std::iter::from_fn(|| {
            ids.next().map(|id| {
                t += 1;
                l.saw(id, t)
            })
        })
        .collect();
        assert_eq!(said, [None, None, Some(3), None]);
    }
}
