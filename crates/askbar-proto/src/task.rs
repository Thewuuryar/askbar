//! A task: one run of the MCP gateway's agent loop, and how it ended (or whether it is still running).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub user: String,
    pub status: TaskStatus,
    /// What the task is. Only `research` exists yet; Phase 6 adds more. Absent in an older payload: `research`.
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub tool_calls: u32,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub token_cap: u64,
}

fn default_kind() -> String {
    "research".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Done,
    Stopped,
    Interrupted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_round_trips_and_its_status_is_a_lower_case_word() {
        let t = Task {
            id: "t1".into(),
            user: "u1".into(),
            status: TaskStatus::Interrupted,
            kind: "research".into(),
            conversation_id: Some("c1".into()),
            tool_calls: 3,
            tokens_used: 1200,
            token_cap: 300_000,
        };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"id":"t1","user":"u1","status":"interrupted","kind":"research","conversation_id":"c1","tool_calls":3,"tokens_used":1200,"token_cap":300000}"#
        );
        assert_eq!(serde_json::from_str::<Task>(&json).unwrap(), t);
    }

    #[test]
    fn an_old_task_without_the_additions_parses_with_their_defaults() {
        let t: Task = serde_json::from_str(r#"{"id":"t1","user":"u1","status":"running"}"#).unwrap();
        assert_eq!((t.kind.as_str(), t.conversation_id, t.tool_calls, t.tokens_used, t.token_cap), ("research", None, 0, 0, 0));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        assert!(serde_json::from_str::<Task>(r#"{"id":"t","user":"u","status":"done","x":1}"#).is_err());
    }
}
