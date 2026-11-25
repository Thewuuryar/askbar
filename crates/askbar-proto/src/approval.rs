//! An approval for one tool action, and the canonical bytes of that action.
//!
//! This crate does not hash. `action_hash` is SHA-256 over [`canonical_bytes`], computed in the `approval` feature of
//! `askbar-service` (ring, no new crate), so the hash and the signed bytes are one definition.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub action_hash: String,
    pub task_id: String,
    pub user: String,
    pub one_time_id: String,
    /// UTC unix seconds.
    pub expires_at: i64,
    pub signature: String,
}

/// One tool call the model wants to make. `provenance` and `query_data` are additive: an older payload omits them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolAction {
    pub tool: String,
    pub arguments: Value,
    pub destination: String,
    /// Where the URL came from. Old payloads default to `search`, the stricter rule.
    #[serde(default)]
    pub provenance: Provenance,
    #[serde(default)]
    pub query_data: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    #[default]
    Search,
    User,
}

/// A quick-search tool call the gateway is asked to approve (plan W5.2). `generation_id` is the trace id of the reply,
/// so the approval and the reply's spans can be joined. Not signed as a whole: the gateway signs the action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuickSearchApprovalRequest {
    pub generation_id: String,
    pub user: String,
    pub tool: String,
    pub arguments: Value,
}

/// Why [`canonical_bytes`] refused an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalError {
    /// A float anywhere in the action: its text form is not fixed, so it cannot be signed.
    Float,
    /// The action could not be turned into a JSON value.
    Serialize,
}

/// The action as compact JSON with object keys sorted at every level and no floats. The same action gives the same
/// bytes whatever order its fields or keys arrived in.
pub fn canonical_bytes(action: &ToolAction) -> Result<Vec<u8>, CanonicalError> {
    let value = serde_json::to_value(action).map_err(|_| CanonicalError::Serialize)?;
    let mut out = String::new();
    write_canonical(&value, &mut out)?;
    Ok(out.into_bytes())
}

fn write_canonical(v: &Value, out: &mut String) -> Result<(), CanonicalError> {
    match v {
        Value::Number(n) if n.is_f64() => return Err(CanonicalError::Float),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let sorted: BTreeMap<&String, &Value> = map.iter().collect();
            out.push('{');
            for (i, (key, val)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(val, out)?;
            }
            out.push('}');
        }
        scalar => out.push_str(&scalar.to_string()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(arguments: &str) -> ToolAction {
        ToolAction {
            tool: "fetch".into(),
            arguments: serde_json::from_str(arguments).unwrap(),
            destination: "example.com".into(),
            provenance: Provenance::User,
            query_data: false,
        }
    }

    #[test]
    fn the_bytes_are_sorted_compact_json() {
        let bytes = canonical_bytes(&action(r#"{"z":[true,null],"a":{"y":"s","b":1}}"#)).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            r#"{"arguments":{"a":{"b":1,"y":"s"},"z":[true,null]},"destination":"example.com","provenance":"user","query_data":false,"tool":"fetch"}"#
        );
    }

    #[test]
    fn the_bytes_do_not_depend_on_field_or_key_order() {
        let a = canonical_bytes(&action(r#"{"x":1,"y":{"p":2,"q":3}}"#)).unwrap();
        let b = canonical_bytes(&action(r#"{"y":{"q":3,"p":2},"x":1}"#)).unwrap();
        assert_eq!(a, b);
        let reordered: ToolAction = serde_json::from_str(
            r#"{"query_data":false,"provenance":"user","destination":"example.com","arguments":{"x":1,"y":{"p":2,"q":3}},"tool":"fetch"}"#,
        )
        .unwrap();
        assert_eq!(canonical_bytes(&reordered).unwrap(), a);
    }

    #[test]
    fn a_float_is_refused_and_an_integer_is_not() {
        assert_eq!(canonical_bytes(&action(r#"{"n":1.5}"#)), Err(CanonicalError::Float));
        assert_eq!(canonical_bytes(&action(r#"{"n":[{"f":0.0}]}"#)), Err(CanonicalError::Float));
        assert!(canonical_bytes(&action(r#"{"n":-7}"#)).is_ok());
    }

    #[test]
    fn an_old_payload_without_the_additive_fields_parses_with_their_defaults() {
        let old: ToolAction = serde_json::from_str(r#"{"tool":"fetch","arguments":{},"destination":"example.com"}"#).unwrap();
        assert_eq!(old.provenance, Provenance::Search);
        assert!(!old.query_data);
    }

    #[test]
    fn an_approval_round_trips() {
        let a = Approval {
            action_hash: "h".into(),
            task_id: "t".into(),
            user: "u".into(),
            one_time_id: "o".into(),
            expires_at: 1_700_000_060,
            signature: "s".into(),
        };
        let json = serde_json::to_string(&a).unwrap();
        assert_eq!(serde_json::from_str::<Approval>(&json).unwrap(), a);
    }
}
