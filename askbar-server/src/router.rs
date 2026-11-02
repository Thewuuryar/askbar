//! Which provider serves a generation, and what happens when one fails.
//!
//! `plan` is the pure part: given the snapshot and the provider the person asked for, it lists
//! the providers to try, in order. (Retries, fallback and the gates arrive in the steps after
//! it.)

use askbar_proto::{Billing, Snapshot};

/// One candidate in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRef {
    pub id: String,
    /// The provider this one is standing in for, or `None` for the one that was asked for.
    pub fallback_from: Option<String>,
}

/// The providers to try for a request for `requested`, in order: that one first (always, even if it
/// is metered: the person chose it), then each of the snapshot's `fallback_order` in turn, left
/// out when it is the requested one, appears twice, is not in the snapshot, or is metered without
/// `allow_as_fallback` (a fallback must not spend money the administrator did not agree to spend).
/// A subscription provider is always eligible.
pub fn plan(snapshot: &Snapshot, requested: &str) -> Vec<ProviderRef> {
    let find = |id: &str| snapshot.providers.iter().find(|p| p.id == id);
    let mut out = Vec::new();
    if find(requested).is_some() {
        out.push(ProviderRef { id: requested.to_string(), fallback_from: None });
    }
    for id in &snapshot.fallback_order {
        let Some(p) = find(id) else { continue };
        if id == requested || out.iter().any(|r| &r.id == id) {
            continue;
        }
        if p.billing == Billing::Metered && !p.allow_as_fallback {
            continue;
        }
        out.push(ProviderRef { id: id.clone(), fallback_from: Some(requested.to_string()) });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{empty_snapshot, provider};

    fn snap(providers: &[(&str, Billing, bool)], fallback: &[&str]) -> Snapshot {
        let mut s = empty_snapshot();
        for (id, billing, allow) in providers {
            let mut p = provider(id, id, "http://x/v1", "m");
            p.billing = *billing;
            p.allow_as_fallback = *allow;
            s.providers.push(p);
        }
        s.fallback_order = fallback.iter().map(|s| s.to_string()).collect();
        s
    }

    fn ids(plan: &[ProviderRef]) -> Vec<&str> {
        plan.iter().map(|r| r.id.as_str()).collect()
    }

    #[test]
    fn the_requested_provider_comes_first_then_the_fallback_order() {
        let s = snap(&[("a", Billing::Subscription, false), ("b", Billing::Subscription, false), ("c", Billing::Subscription, false)], &["b", "c"]);
        let p = plan(&s, "a");
        assert_eq!(ids(&p), ["a", "b", "c"]);
        assert_eq!(p[0].fallback_from, None);
        assert_eq!(p[1].fallback_from.as_deref(), Some("a"));
        // Asking for one that is itself in the fallback list does not repeat it.
        assert_eq!(ids(&plan(&s, "b")), ["b", "c"]);
        assert_eq!(ids(&plan(&s, "c")), ["c", "b"]);
    }

    #[test]
    fn a_metered_provider_is_skipped_as_a_fallback_unless_it_allows_it() {
        let s = snap(&[("a", Billing::Subscription, false), ("m1", Billing::Metered, false), ("m2", Billing::Metered, true), ("sub", Billing::Subscription, false)], &["m1", "m2", "sub"]);
        assert_eq!(ids(&plan(&s, "a")), ["a", "m2", "sub"]);
        // But a metered provider that was asked for is used, whatever its flag.
        assert_eq!(ids(&plan(&s, "m1")), ["m1", "m2", "sub"]);
    }

    #[test]
    fn unknown_duplicate_and_missing_entries_are_left_out() {
        let s = snap(&[("a", Billing::Subscription, false), ("b", Billing::Subscription, false)], &["ghost", "b", "b", "a"]);
        assert_eq!(ids(&plan(&s, "a")), ["a", "b"]);
        // Asking for something the snapshot does not have plans only the fallbacks (the caller refuses it earlier).
        assert_eq!(ids(&plan(&s, "nope")), ["b", "a"]);
        assert!(plan(&empty_snapshot(), "a").is_empty());
    }

    #[test]
    fn no_fallback_order_means_a_plan_of_one() {
        let s = snap(&[("a", Billing::Metered, false), ("b", Billing::Subscription, false)], &[]);
        assert_eq!(ids(&plan(&s, "a")), ["a"]);
    }
}
