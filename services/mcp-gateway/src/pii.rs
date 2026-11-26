//! PII on a tool call's arguments, after the approval and before the `tool` span (plan PII-P2.6). Only counts leave here:
//! no detected value, query or URL goes into an error, a log or an attribute.
//!
//! - `search`: a secret refuses the call; any other span is cut out of the query, the cleaned query is what runs, and the
//!   result says how many were left out. A query with nothing left is refused.
//! - `fetch`: a secret in the URL refuses the call; other PII is only counted, because cutting a part out of an address
//!   changes the address (a URL comes from a search result or the user's own choice).
//!
//! `detect_only` counts and changes nothing. The approval covered the original arguments and was checked before this runs.

use askbar_proto::pii::{PiiConfig, PiiMode};
use askbar_service::pii::{detect, Span, SECRETS};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// What the check found, by type, and how many spans the query lost.
#[derive(Default)]
pub struct Checked {
    pub counts: BTreeMap<&'static str, u64>,
    pub left_out: u64,
}

impl Checked {
    /// The span attributes: counts by type and `left_out`; `None` when nothing was found.
    pub fn attrs(&self) -> Option<Value> {
        (!self.counts.is_empty()).then(|| json!({ "pii": self.counts, "left_out": self.left_out }))
    }
}

/// Why a call is refused: the types found (never a value) and what to tell the caller.
pub struct Blocked {
    pub types: Vec<&'static str>,
    pub detail: &'static str,
}

fn types(spans: impl Iterator<Item = &'static str>) -> Vec<&'static str> {
    spans.collect::<std::collections::BTreeSet<_>>().into_iter().collect()
}

fn counts(spans: &[Span]) -> BTreeMap<&'static str, u64> {
    let mut m = BTreeMap::new();
    for s in spans {
        *m.entry(s.ty).or_default() += 1;
    }
    m
}

/// Checks `args` of `tool` and, for `search` in enforce mode, replaces `args.query` with the cleaned one.
pub fn check(cfg: &PiiConfig, tool: &str, args: &mut Value) -> Result<Checked, Blocked> {
    let key = match tool {
        "search" => "query",
        "fetch" => "url",
        _ => return Ok(Checked::default()),
    };
    let Some(text) = args.get(key).and_then(Value::as_str) else { return Ok(Checked::default()) };
    let spans = detect(text, cfg);
    let mut out = Checked { counts: counts(&spans), left_out: 0 };
    if spans.is_empty() || cfg.mode != PiiMode::Enforce {
        return Ok(out);
    }
    let secrets = types(spans.iter().map(|s| s.ty).filter(|t| SECRETS.contains(t)));
    if !secrets.is_empty() {
        return Err(Blocked { types: secrets, detail: "The call holds a secret that cannot be sent. Remove it and try again." });
    }
    if key == "url" {
        return Ok(out);
    }
    let mut clean = text.to_string();
    for s in spans.iter().rev() {
        clean.replace_range(s.start..s.end, " ");
    }
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return Err(Blocked {
            types: types(spans.iter().map(|s| s.ty)),
            detail: "The query held only private values, so there is nothing left to search.",
        });
    }
    out.left_out = spans.len() as u64;
    args[key] = Value::String(clean);
    Ok(out)
}
