//! What each research task has used (plan step P5.10): tokens against its cap, tool calls against its limit. Kept in
//! memory only, keyed by the task id (the generation id), and counted for the roles that `Role::is_task` names. The
//! figures are never logged; only how many tasks crossed 75% and 90% of the cap is counted, with no task or user named.

use askbar_proto::ErrorCode;
use std::collections::{HashMap, VecDeque};

/// Tasks remembered; the oldest is forgotten first (as `QuickCounts` does).
const MAX_TASKS: usize = 10_000;

#[derive(Default)]
struct Used {
    tokens: u64,
    tool_calls: u32,
    over_75: bool,
    over_90: bool,
}

#[derive(Default)]
pub struct TaskCounts {
    tasks: HashMap<String, Used>,
    order: VecDeque<String>,
    cap_75: u64,
    cap_90: u64,
}

impl TaskCounts {
    fn entry(&mut self, task: &str) -> &mut Used {
        if !self.tasks.contains_key(task) {
            if self.order.len() >= MAX_TASKS {
                if let Some(old) = self.order.pop_front() {
                    self.tasks.remove(&old);
                }
            }
            self.order.push_back(task.into());
        }
        self.tasks.entry(task.into()).or_default()
    }

    /// Whether the task has used `cap` tokens already.
    pub fn at_cap(&self, task: &str, cap: u32) -> bool {
        self.tasks.get(task).is_some_and(|u| u.tokens >= u64::from(cap))
    }

    /// Add what a call used; count each of the 75% and 90% marks once per task.
    pub fn add_tokens(&mut self, task: &str, tokens: u64, cap: u32) {
        let u = self.entry(task);
        u.tokens += tokens;
        let (hit_75, hit_90) = (u.tokens * 100 >= u64::from(cap) * 75, u.tokens * 100 >= u64::from(cap) * 90);
        let (new_75, new_90) = (hit_75 && !u.over_75, hit_90 && !u.over_90);
        (u.over_75, u.over_90) = (hit_75, hit_90);
        self.cap_75 += u64::from(new_75);
        self.cap_90 += u64::from(new_90);
    }

    /// Count one tool call, or refuse it when the task has made `limit` already.
    pub fn reserve_tool_call(&mut self, task: &str, limit: u32) -> Result<(), ErrorCode> {
        let u = self.entry(task);
        if u.tool_calls >= limit {
            return Err(ErrorCode::TaskToolCalls);
        }
        u.tool_calls += 1;
        Ok(())
    }

    /// Take back a `reserve_tool_call` whose approval was not given.
    pub fn release_tool_call(&mut self, task: &str) {
        if let Some(u) = self.tasks.get_mut(task) {
            u.tool_calls = u.tool_calls.saturating_sub(1);
        }
    }

    /// Tasks that crossed 75% and 90% of the cap since the process started.
    pub fn totals(&self) -> (u64, u64) {
        (self.cap_75, self.cap_90)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mark_is_counted_once_per_task_and_tasks_do_not_share() {
        let mut t = TaskCounts::default();
        t.add_tokens("a", 70, 100);
        assert_eq!(t.totals(), (0, 0));
        t.add_tokens("a", 5, 100);
        t.add_tokens("a", 1, 100);
        assert_eq!(t.totals(), (1, 0));
        t.add_tokens("a", 14, 100);
        t.add_tokens("a", 1, 100);
        assert_eq!(t.totals(), (1, 1));
        assert!(!t.at_cap("a", 101) && t.at_cap("a", 91));
        assert!(!t.at_cap("b", 1));
        t.add_tokens("b", 95, 100);
        assert_eq!(t.totals(), (2, 2), "one call may cross both marks");
    }

    #[test]
    fn the_bound_drops_the_oldest_task() {
        let mut t = TaskCounts::default();
        for i in 0..=MAX_TASKS {
            t.add_tokens(&format!("t{i}"), 10, 10);
        }
        assert!(!t.at_cap("t0", 10), "the oldest was forgotten");
        assert!(t.at_cap("t1", 10) && t.at_cap(&format!("t{MAX_TASKS}"), 10));
        assert_eq!(t.tasks.len(), MAX_TASKS);
    }

    #[test]
    fn tool_calls_stop_at_the_limit_and_a_release_gives_one_back() {
        let mut t = TaskCounts::default();
        assert_eq!(t.reserve_tool_call("a", 2), Ok(()));
        assert_eq!(t.reserve_tool_call("a", 2), Ok(()));
        assert_eq!(t.reserve_tool_call("a", 2), Err(ErrorCode::TaskToolCalls));
        assert_eq!(t.reserve_tool_call("b", 2), Ok(()));
        t.release_tool_call("a");
        assert_eq!(t.reserve_tool_call("a", 2), Ok(()));
    }
}
