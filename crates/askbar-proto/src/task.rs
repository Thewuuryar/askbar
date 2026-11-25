//! A task: one run of the MCP gateway's agent loop, and how it ended (or whether it is still running).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub user: String,
    pub status: TaskStatus,
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
        let t = Task { id: "t1".into(), user: "u1".into(), status: TaskStatus::Interrupted };
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"id":"t1","user":"u1","status":"interrupted"}"#);
        assert_eq!(serde_json::from_str::<Task>(&json).unwrap(), t);
    }

    #[test]
    fn an_unknown_field_is_refused() {
        assert!(serde_json::from_str::<Task>(r#"{"id":"t","user":"u","status":"done","x":1}"#).is_err());
    }
}
