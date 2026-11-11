//! The Audit pages' figures with the admin's prices applied (ADR 0051): the auditor holds none, so every cost it sends is
//! null, and the admin prices each provider and each day from the tokens (so a corrected price changes every figure at
//! once, and nothing about cost is stored). Moved from the server, which priced them while it still held the store.

use crate::cost::{cost_of, PriceOf, Prices};
use askbar_proto::SnapshotProvider;

/// A lookup of prices by provider id, from the providers as they are now.
pub fn price_lookup(providers: Vec<SnapshotProvider>) -> impl Fn(&str) -> Option<Prices> {
    move |id: &str| providers.iter().find(|p| p.id == id).map(Prices::of)
}

/// Put `cost` on a row that has a `provider_id` and token counts (a trace summary, a span).
pub fn add_cost(row: &mut serde_json::Value, price_of: PriceOf) {
    let n = |k: &str| row[k].as_i64();
    let cost = cost_of(price_of, row["provider_id"].as_str(), n("tokens_in"), n("tokens_out"), n("tokens_cached"));
    row["cost"] = serde_json::json!(cost);
}

/// The cost of one group of requests (one provider, perhaps one day): known, or unknown with how many requests it
/// leaves unknown. A group with no counts costs nothing to price.
fn group_cost(price_of: PriceOf, provider: Option<&str>, tokens: &serde_json::Value, with_tokens: i64) -> (Option<f64>, i64) {
    if with_tokens == 0 {
        return (Some(0.0), 0);
    }
    let n = |k: &str| tokens[k].as_i64();
    match cost_of(price_of, provider, n("in"), n("out"), n("cached")) {
        Some(c) => (Some(c), 0),
        None => (None, with_tokens),
    }
}

/// The auditor's figures with this server's prices applied: it holds none, so every cost it sends is null. Each
/// provider and each day is priced from its tokens, so a corrected price changes every figure at once.
pub fn price_stats(mut v: serde_json::Value, price_of: PriceOf) -> serde_json::Value {
    let (mut total_known, mut total_unknown) = (0.0, 0);
    if let Some(rows) = v["by_provider"].as_array_mut() {
        for r in rows {
            let (cost, unknown) =
                group_cost(price_of, r["provider_id"].as_str(), &r["tokens"], r["requests_with_tokens"].as_i64().unwrap_or(0));
            r["cost"] = serde_json::json!(cost);
            r["cost_unknown_requests"] = unknown.into();
            total_known += cost.unwrap_or(0.0);
            total_unknown += unknown;
        }
    }
    v["cost"] = serde_json::json!((total_unknown == 0).then_some(total_known));
    v["cost_unknown_requests"] = total_unknown.into();
    let groups = v["by_day_provider"].as_array().cloned().unwrap_or_default();
    if let Some(days) = v["by_day"].as_array_mut() {
        for d in days {
            let (mut known, mut unknown) = (0.0, 0);
            for g in groups.iter().filter(|g| g["day"] == d["day"]) {
                let (cost, u) =
                    group_cost(price_of, g["provider_id"].as_str(), &g["tokens"], g["requests_with_tokens"].as_i64().unwrap_or(0));
                known += cost.unwrap_or(0.0);
                unknown += u;
            }
            d["cost"] = serde_json::json!((unknown == 0).then_some(known));
            d["cost_unknown_requests"] = unknown.into();
        }
    }
    // The pieces the pages never needed: they are the auditor's working figures.
    if let Some(o) = v.as_object_mut() {
        o.remove("by_day_provider");
    }
    if let Some(rows) = v["by_provider"].as_array_mut() {
        rows.iter_mut().for_each(|r| {
            r.as_object_mut().map(|o| o.remove("requests_with_tokens"));
        });
    }
    if let Some(rows) = v["by_day"].as_array_mut() {
        rows.iter_mut().for_each(|r| {
            r.as_object_mut().map(|o| o.remove("requests_with_tokens"));
        });
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(id: &str, price_in: Option<f64>, price_out: Option<f64>) -> SnapshotProvider {
        serde_json::from_value(json!({
            "id": id, "name": id, "type": "openai", "model": "m", "price_in": price_in, "price_out": price_out
        }))
        .unwrap()
    }

    fn stats() -> serde_json::Value {
        json!({
            "requests": 3, "cost": null, "cost_unknown_requests": 0,
            "by_provider": [
                { "provider_id": "p1", "requests": 2, "requests_with_tokens": 2, "tokens": { "in": 1_000_000, "out": 500_000, "cached": 0 } },
                { "provider_id": "p2", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 100, "out": 100, "cached": 0 } }
            ],
            "by_day": [
                { "day": "2026-11-10", "requests": 2, "requests_with_tokens": 2, "tokens": { "in": 0, "out": 0, "cached": 0 } },
                { "day": "2026-11-11", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 0, "out": 0, "cached": 0 } }
            ],
            "by_day_provider": [
                { "day": "2026-11-10", "provider_id": "p1", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 400_000, "out": 100_000, "cached": 0 } },
                { "day": "2026-11-10", "provider_id": "p2", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 100, "out": 100, "cached": 0 } },
                { "day": "2026-11-11", "provider_id": "p1", "requests": 1, "requests_with_tokens": 1, "tokens": { "in": 600_000, "out": 400_000, "cached": 0 } }
            ]
        })
    }

    fn close(v: &serde_json::Value, want: f64) -> bool {
        v.as_f64().is_some_and(|x| (x - want).abs() < 1e-9)
    }

    #[test]
    fn each_provider_and_each_day_is_priced_from_its_tokens_and_the_helper_fields_go() {
        // p1: 2.0 per million in, 4.0 per million out. p2 has no price at all.
        let providers = vec![provider("p1", Some(2.0), Some(4.0)), provider("p2", None, None)];
        let priced = price_stats(stats(), &price_lookup(providers));
        // p1 cost: 1M * 2 + 0.5M * 4 = 4.0.
        assert!(close(&priced["by_provider"][0]["cost"], 4.0), "{priced}");
        assert_eq!(priced["by_provider"][0]["cost_unknown_requests"], 0);
        // p2 cannot be priced: unknown, never zero.
        assert!(priced["by_provider"][1]["cost"].is_null());
        assert_eq!(priced["by_provider"][1]["cost_unknown_requests"], 1);
        assert!(priced["cost"].is_null(), "one unknown makes the total unknown");
        assert_eq!(priced["cost_unknown_requests"], 1);
        // Day 1 holds p1 and p2, so it is unknown; day 2 holds p1 only: 0.6M * 2 + 0.4M * 4 = 2.8.
        assert!(priced["by_day"][0]["cost"].is_null());
        assert!(close(&priced["by_day"][1]["cost"], 2.8), "{priced}");
        // The auditor's working figures are not for the page.
        assert!(priced.get("by_day_provider").is_none() && priced["by_provider"][0].get("requests_with_tokens").is_none());
    }

    #[test]
    fn with_every_price_known_the_total_is_the_sum_and_a_corrected_price_changes_it() {
        let both = vec![provider("p1", Some(2.0), Some(4.0)), provider("p2", Some(1.0), Some(1.0))];
        let priced = price_stats(stats(), &price_lookup(both));
        assert!(close(&priced["cost"], 4.0 + 0.0002), "{}", priced["cost"]);
        assert_eq!(priced["cost_unknown_requests"], 0);
        let corrected = vec![provider("p1", Some(3.0), Some(4.0)), provider("p2", Some(1.0), Some(1.0))];
        assert!(close(&price_stats(stats(), &price_lookup(corrected))["cost"], 5.0 + 0.0002));
    }

    #[test]
    fn a_row_gets_its_cost_from_its_provider_and_unknown_without_a_price() {
        let lookup = price_lookup(vec![provider("p1", Some(2.0), Some(4.0))]);
        let mut known = json!({ "provider_id": "p1", "tokens_in": 1_000_000, "tokens_out": 250_000, "tokens_cached": 0 });
        add_cost(&mut known, &lookup);
        assert!(close(&known["cost"], 3.0));
        let mut unknown = json!({ "provider_id": "gone", "tokens_in": 10, "tokens_out": 10, "tokens_cached": 0 });
        add_cost(&mut unknown, &lookup);
        assert!(unknown["cost"].is_null());
        let mut none = json!({ "provider_id": "p1", "tokens_in": null, "tokens_out": null, "tokens_cached": null });
        add_cost(&mut none, &lookup);
        assert!(none["cost"].is_null(), "no counts: no cost");
    }
}
