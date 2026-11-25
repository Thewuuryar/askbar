//! Research reports and the judge (plan phase 5B): the task limits with their bounds, the approval request for a task's
//! tool call, and the fixed input and output of the judge. The admin pushes limits within these bounds to `model-gateway`
//! and the server, and each refuses a value outside them. Summaries and the draft are untrusted text: data, never
//! instructions.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Tool calls one task may make in all, shared by its researchers.
pub const MIN_TOOL_CALLS: u32 = 1;
pub const DEFAULT_TOOL_CALLS: u32 = 8;
pub const MAX_TOOL_CALLS: u32 = 32;

/// Tokens one task's model calls may use in all.
pub const MIN_TOKEN_CAP: u32 = 20_000;
pub const DEFAULT_TOKEN_CAP: u32 = 300_000;
pub const MAX_TOKEN_CAP: u32 = 2_000_000;

/// Researchers that run in parallel (one per sub-question).
pub const MIN_RESEARCHERS: u32 = 2;
pub const DEFAULT_RESEARCHERS: u32 = 4;
pub const MAX_RESEARCHERS: u32 = 4;

/// Seconds to wait for the other researchers once two have finished.
pub const MIN_WAIT_AFTER_TWO_SECS: u32 = 1;
pub const DEFAULT_WAIT_AFTER_TWO_SECS: u32 = 8;
pub const MAX_WAIT_AFTER_TWO_SECS: u32 = 60;

/// Seconds the research stage may take before it stops.
pub const MIN_STAGE_SECS: u32 = 10;
pub const DEFAULT_STAGE_SECS: u32 = 90;
pub const MAX_STAGE_SECS: u32 = 600;

/// Tokens a researcher's summary of its pages may have.
pub const MIN_SUMMARY_TOKENS: u32 = 100;
pub const DEFAULT_SUMMARY_TOKENS: u32 = 800;
pub const MAX_SUMMARY_TOKENS: u32 = 4_000;

/// The task limits as they are pushed. The default is the `DEFAULT_*` values, so a service that has not been pushed
/// any runs on the defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLimits {
    pub tool_calls: u32,
    pub token_cap: u32,
    pub researchers_max: u32,
    pub wait_after_two_secs: u32,
    pub stage_secs: u32,
    pub summary_tokens: u32,
}

impl TaskLimits {
    pub const DEFAULT: TaskLimits = TaskLimits {
        tool_calls: DEFAULT_TOOL_CALLS,
        token_cap: DEFAULT_TOKEN_CAP,
        researchers_max: DEFAULT_RESEARCHERS,
        wait_after_two_secs: DEFAULT_WAIT_AFTER_TWO_SECS,
        stage_secs: DEFAULT_STAGE_SECS,
        summary_tokens: DEFAULT_SUMMARY_TOKENS,
    };

    /// Every value within its bounds. The message names no value.
    pub fn validate(&self) -> Result<(), &'static str> {
        let ok = (MIN_TOOL_CALLS..=MAX_TOOL_CALLS).contains(&self.tool_calls)
            && (MIN_TOKEN_CAP..=MAX_TOKEN_CAP).contains(&self.token_cap)
            && (MIN_RESEARCHERS..=MAX_RESEARCHERS).contains(&self.researchers_max)
            && (MIN_WAIT_AFTER_TWO_SECS..=MAX_WAIT_AFTER_TWO_SECS).contains(&self.wait_after_two_secs)
            && (MIN_STAGE_SECS..=MAX_STAGE_SECS).contains(&self.stage_secs)
            && (MIN_SUMMARY_TOKENS..=MAX_SUMMARY_TOKENS).contains(&self.summary_tokens);
        if ok {
            Ok(())
        } else {
            Err("A task limit is outside its bounds (askbar-proto, research).")
        }
    }
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A research task's tool call the gateway is asked to approve (`POST /approvals/research`). `task_id` is the reply's
/// generation id. Not signed as a whole: the gateway signs the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResearchApprovalRequest {
    pub task_id: String,
    pub user: String,
    pub tool: String,
    pub arguments: Value,
}

/// One researcher's summary, as the judge sees it. `source` is the number the draft cites ("Source 1").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeSummary {
    pub source: u32,
    pub urls: Vec<String>,
    pub text: String,
}

/// What the judge sees (`decision: judge`): the question, the summaries and the draft, never raw page text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JudgeInput {
    pub task_id: String,
    pub user_id: String,
    pub question: String,
    pub summaries: Vec<JudgeSummary>,
    pub draft: String,
}

/// Why the judge refused a draft. `bad_format` is the gateway's own: the model's answer did not match the fixed format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JudgeReason {
    UnsupportedClaim,
    ContradictsSource,
    OffTopic,
    Unsafe,
    BadCitation,
    BadFormat,
}

/// The judge's answer, exactly `{"verdict":"approve"}` or `{"verdict":"refuse","reason":"<code>"}`. Anything else (an
/// extra field, an unknown verdict or reason) does not parse, and the caller treats that as a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case", deny_unknown_fields)]
pub enum JudgeOutput {
    Approve {},
    Refuse { reason: JudgeReason },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_default_sits_within_its_bounds() {
        let bounds = [
            (MIN_TOOL_CALLS, DEFAULT_TOOL_CALLS, MAX_TOOL_CALLS),
            (MIN_TOKEN_CAP, DEFAULT_TOKEN_CAP, MAX_TOKEN_CAP),
            (MIN_RESEARCHERS, DEFAULT_RESEARCHERS, MAX_RESEARCHERS),
            (MIN_WAIT_AFTER_TWO_SECS, DEFAULT_WAIT_AFTER_TWO_SECS, MAX_WAIT_AFTER_TWO_SECS),
            (MIN_STAGE_SECS, DEFAULT_STAGE_SECS, MAX_STAGE_SECS),
            (MIN_SUMMARY_TOKENS, DEFAULT_SUMMARY_TOKENS, MAX_SUMMARY_TOKENS),
        ];
        for (min, default, max) in bounds {
            assert!(min <= default && default <= max, "{min} <= {default} <= {max}");
        }
        assert!(TaskLimits::DEFAULT.validate().is_ok());
        assert_eq!(TaskLimits::default(), TaskLimits::DEFAULT);
    }

    #[test]
    fn a_value_outside_its_bounds_is_refused() {
        let d = TaskLimits::DEFAULT;
        assert!(TaskLimits { tool_calls: 0, ..d }.validate().is_err());
        assert!(TaskLimits { tool_calls: MAX_TOOL_CALLS + 1, ..d }.validate().is_err());
        assert!(TaskLimits { token_cap: MIN_TOKEN_CAP - 1, ..d }.validate().is_err());
        assert!(TaskLimits { token_cap: MAX_TOKEN_CAP + 1, ..d }.validate().is_err());
        assert!(TaskLimits { researchers_max: 1, ..d }.validate().is_err());
        assert!(TaskLimits { researchers_max: MAX_RESEARCHERS + 1, ..d }.validate().is_err());
        assert!(TaskLimits { wait_after_two_secs: 0, ..d }.validate().is_err());
        assert!(TaskLimits { stage_secs: MAX_STAGE_SECS + 1, ..d }.validate().is_err());
        assert!(TaskLimits { summary_tokens: MIN_SUMMARY_TOKENS - 1, ..d }.validate().is_err());
    }

    #[test]
    fn the_judge_answers_in_the_fixed_format_and_nothing_else() {
        let approve: JudgeOutput = serde_json::from_str(r#"{"verdict":"approve"}"#).unwrap();
        assert_eq!(serde_json::to_string(&approve).unwrap(), r#"{"verdict":"approve"}"#);
        let refuse: JudgeOutput = serde_json::from_str(r#"{"verdict":"refuse","reason":"bad_citation"}"#).unwrap();
        assert_eq!(refuse, JudgeOutput::Refuse { reason: JudgeReason::BadCitation });
        assert_eq!(serde_json::to_string(&refuse).unwrap(), r#"{"verdict":"refuse","reason":"bad_citation"}"#);
        for bad in [
            r#"{"verdict":"approve","reason":"off_topic"}"#,
            r#"{"verdict":"approve","note":"x"}"#,
            r#"{"verdict":"refuse"}"#,
            r#"{"verdict":"refuse","reason":"rude"}"#,
            r#"{"verdict":"refuse","reason":"off_topic","x":1}"#,
            r#"{"verdict":"maybe"}"#,
            r#"{"reason":"off_topic"}"#,
        ] {
            assert!(serde_json::from_str::<JudgeOutput>(bad).is_err(), "{bad}");
        }
        for r in [
            JudgeReason::UnsupportedClaim,
            JudgeReason::ContradictsSource,
            JudgeReason::OffTopic,
            JudgeReason::Unsafe,
            JudgeReason::BadCitation,
            JudgeReason::BadFormat,
        ] {
            assert_eq!(serde_json::from_value::<JudgeReason>(serde_json::to_value(r).unwrap()).unwrap(), r);
        }
    }

    #[test]
    fn the_judge_input_and_approval_request_refuse_unknown_fields() {
        let ok = r#"{"task_id":"t","user_id":"u","question":"q","summaries":[{"source":1,"urls":["https://e.example/"],"text":"s"}],"draft":"d"}"#;
        let input: JudgeInput = serde_json::from_str(ok).unwrap();
        assert_eq!(serde_json::from_str::<JudgeInput>(&serde_json::to_string(&input).unwrap()).unwrap(), input);
        assert!(serde_json::from_str::<JudgeInput>(&ok.replace("\"draft\"", "\"x\":1,\"draft\"")).is_err());
        assert!(serde_json::from_str::<JudgeInput>(&ok.replace("\"text\"", "\"x\":1,\"text\"")).is_err());
        let req = r#"{"task_id":"t","user":"u","tool":"search","arguments":{}}"#;
        assert!(serde_json::from_str::<ResearchApprovalRequest>(req).is_ok());
        assert!(serde_json::from_str::<ResearchApprovalRequest>(&req.replace("\"tool\"", "\"x\":1,\"tool\"")).is_err());
    }
}
