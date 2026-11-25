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
    /// The reply the decision is for, so its call joins the reply's trace and counts against its task. Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    /// Whose decision it is (the user's budget and pause apply). Additive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
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
        assert_eq!((req.generation_id, req.user_id), (None, None), "an old request has neither");
        let with: DecideRequest = serde_json::from_str(r#"{"decision":"judge","input":null,"generation_id":"g","user_id":"u"}"#).unwrap();
        assert_eq!((with.generation_id.as_deref(), with.user_id.as_deref()), (Some("g"), Some("u")));
        let res = DecideResponse { decision: Decision::Judge, output: Value::Bool(true) };
        let json = serde_json::to_string(&res).unwrap();
        assert_eq!(serde_json::from_str::<DecideResponse>(&json).unwrap(), res);
    }

    #[test]
    fn an_unknown_decision_is_refused() {
        assert!(serde_json::from_str::<DecideRequest>(r#"{"decision":"vote","input":null}"#).is_err());
    }
}
