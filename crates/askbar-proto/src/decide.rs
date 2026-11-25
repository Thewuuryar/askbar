//! The `/decide` call: a typed question the model answers, and its answer.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Judge,
    StepCheck,
    Research,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideRequest {
    pub decision: Decision,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideResponse {
    pub decision: Decision,
    pub output: Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_is_its_snake_case_name_on_the_wire() {
        assert_eq!(serde_json::to_string(&Decision::StepCheck).unwrap(), r#""step_check""#);
        let req: DecideRequest = serde_json::from_str(r#"{"decision":"research","input":{"q":"x"}}"#).unwrap();
        assert_eq!(req.decision, Decision::Research);
        let res = DecideResponse { decision: Decision::Judge, output: Value::Bool(true) };
        let json = serde_json::to_string(&res).unwrap();
        assert_eq!(serde_json::from_str::<DecideResponse>(&json).unwrap(), res);
    }

    #[test]
    fn an_unknown_decision_is_refused() {
        assert!(serde_json::from_str::<DecideRequest>(r#"{"decision":"vote","input":null}"#).is_err());
    }
}
