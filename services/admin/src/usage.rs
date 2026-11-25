//! `GET /api/admin/v1/usage/roles`: what quick search used per day (searches, fetches, refusals) and what its two model roles
//! (`search.query`, `search.writer`) used in tokens and cost. Read from the auditor like the Audit pages (ADR 0051), never from the
//! server's tables: the newest [`TRACES`] traces, then each one's spans. Counts, tokens and cost only; a span's attributes and
//! sealed payloads are never forwarded, so no query, URL or page text can appear.
//! Cost is priced here from the admin's prices, as everywhere else (`cost.rs`).

use crate::cost::{cost_of, PriceOf};
use crate::web::{err, Failure};
use crate::AdminState;
use axum::{extract::Query, extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The window a caller may ask for, and the default.
const MAX_DAYS: u32 = 30;
const DEFAULT_DAYS: u32 = 7;
/// The most traces read per call (the auditor's own page maximum): one list and at most this many span reads.
const TRACES: usize = 200;
const ROLES: [&str; 2] = ["search.query", "search.writer"];

#[derive(Deserialize)]
pub struct UsageQuery {
    days: Option<u32>,
}

/// `YYYY-MM-DD` (UTC) of unix seconds.
pub fn day_of(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", yoe + era * 400 + i64::from(m <= 2))
}

#[derive(Default)]
struct RoleDay {
    turns: u64,
    tokens: [u64; 3],
    cost: f64,
    cost_unknown_turns: u64,
}

#[derive(Default)]
struct Day {
    searches: u64,
    fetches: u64,
    refusals: u64,
}

/// The figures of the given traces (each the list of its spans), counting spans that started at or after `since`.
fn aggregate(traces: &[Vec<Value>], since: i64, price_of: PriceOf) -> (Vec<Value>, Vec<Value>) {
    let mut days: BTreeMap<String, Day> = BTreeMap::new();
    let mut roles: BTreeMap<(String, &str), RoleDay> = BTreeMap::new();
    for spans in traces {
        let provider = spans.iter().rev().find(|s| s["kind"] == "attempt").and_then(|s| s["provider_id"].as_str());
        for s in spans.iter().filter(|s| s["started_at"].as_i64().is_some_and(|t| t >= since)) {
            let day = day_of(s["started_at"].as_i64().unwrap_or(0));
            let name = s["name"].as_str().unwrap_or("");
            match (s["kind"].as_str(), name) {
                (Some("tool"), _) if s["status"] == "refused" => days.entry(day).or_default().refusals += 1,
                (Some("tool"), "search") => days.entry(day).or_default().searches += 1,
                (Some("tool"), "fetch") => days.entry(day).or_default().fetches += 1,
                (Some("gate"), "quick_search_refused" | "quick_search_approval") => days.entry(day).or_default().refusals += 1,
                (Some("turn"), _) => {
                    let Some(role) = ROLES.into_iter().find(|r| s["attrs"]["role"] == *r) else { continue };
                    let n = |k: &str| s[k].as_i64();
                    let r = roles.entry((day, role)).or_default();
                    r.turns += 1;
                    for (i, k) in ["tokens_in", "tokens_out", "tokens_cached"].into_iter().enumerate() {
                        r.tokens[i] += n(k).unwrap_or(0).max(0) as u64;
                    }
                    // A turn that failed before any reply has no tokens and costs nothing.
                    if n("tokens_in").is_some() || n("tokens_out").is_some() {
                        match cost_of(price_of, provider.or(s["provider_id"].as_str()), n("tokens_in"), n("tokens_out"), n("tokens_cached"))
                        {
                            Some(c) => r.cost += c,
                            None => r.cost_unknown_turns += 1,
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let per_day =
        days.iter().rev().map(|(d, v)| json!({ "day": d, "searches": v.searches, "fetches": v.fetches, "refusals": v.refusals })).collect();
    let per_role = roles
        .iter()
        .rev()
        .map(|((d, role), v)| {
            json!({
                "day": d, "role": role, "turns": v.turns, "tokens_in": v.tokens[0], "tokens_out": v.tokens[1], "tokens_cached": v.tokens[2],
                "cost": (v.cost_unknown_turns == 0).then_some(v.cost), "cost_unknown_turns": v.cost_unknown_turns,
            })
        })
        .collect();
    (per_day, per_role)
}

fn away() -> Failure {
    err(StatusCode::SERVICE_UNAVAILABLE, "The auditor cannot be reached, so the usage cannot be read now.")
}

pub async fn roles(State(state): State<Arc<AdminState>>, Query(q): Query<UsageQuery>) -> Result<Json<Value>, Failure> {
    let days = q.days.unwrap_or(DEFAULT_DAYS);
    if !(1..=MAX_DAYS).contains(&days) {
        return Err(err(StatusCode::BAD_REQUEST, &format!("The days must be 1 to {MAX_DAYS}.")));
    }
    let since = crate::auth::now() - i64::from(days) * 86_400;
    let auditor = crate::admin_api::auditor(&state)?;
    let Some((200, list)) = auditor.get(&format!("/traces?limit={TRACES}")).await else { return Err(away()) };
    let list = list.as_array().cloned().unwrap_or_default();
    // The list is newest first; a full page whose oldest trace is still in the window may leave older ones unread.
    let in_window: Vec<&Value> = list.iter().filter(|t| t["started_at"].as_i64().is_some_and(|a| a >= since)).collect();
    let truncated = list.len() >= TRACES && in_window.len() == list.len();
    let mut traces = Vec::new();
    for id in in_window
        .iter()
        .filter_map(|t| t["trace_id"].as_str())
        .filter(|id| id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    {
        let Some((200, spans)) = auditor.get(&format!("/traces/{id}")).await else { return Err(away()) };
        traces.push(spans.as_array().cloned().unwrap_or_default());
    }
    let price_of = crate::admin_api::price_of(&state).await?;
    let (per_day, per_role) = aggregate(&traces, since, &price_of);
    Ok(Json(json!({ "days": days, "traces_read": traces.len(), "truncated": truncated, "per_day": per_day, "roles": per_role })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_utc_dates() {
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(day_of(951_782_400), "2000-02-29");
        assert_eq!(day_of(1_793_000_000), "2026-10-26");
    }
}
