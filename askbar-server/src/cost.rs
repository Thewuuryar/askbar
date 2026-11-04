//! What a reply cost, worked out when it is asked for, never stored.
//!
//! Prices live on the provider in the snapshot, per million tokens. The usage rows and audit spans
//! hold only token counts, so correcting a price re-prices all of history the next time it is read.
//! An unknown price gives an unknown cost (`None`, shown as null), never 0: a wrong zero would
//! look like a bargain.

use askbar_proto::{Billing, SnapshotProvider};

/// Token counts as providers report them for one reply.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tokens {
    /// Input tokens as reported. For OpenAI-style providers this *includes* the cached ones; for
    /// Anthropic it does not (see `includes_cached`).
    pub input: u64,
    pub output: u64,
    /// Input tokens served from the provider's cache.
    pub cached: u64,
}

/// The price list of one provider (per million tokens) and how it is billed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub billing: Billing,
    pub input: Option<f64>,
    pub output: Option<f64>,
    pub cached_input: Option<f64>,
    /// True when the provider's `input` count already contains the cached tokens (OpenAI style),
    /// so the uncached part is `input - cached`. False for Anthropic, where they are separate.
    pub includes_cached: bool,
}

impl Prices {
    pub fn of(p: &SnapshotProvider) -> Prices {
        Prices {
            billing: p.billing,
            input: p.price_in,
            output: p.price_out,
            cached_input: p.price_cached_in,
            includes_cached: p.kind != "anthropic",
        }
    }
}

fn term(tokens: u64, price: Option<f64>) -> Option<f64> {
    if tokens == 0 {
        return Some(0.0); // nothing to price: an absent price does not matter
    }
    price.map(|per_million| tokens as f64 * per_million / 1_000_000.0)
}

/// `uncached input x price_in + cached x price_cached_in + output x price_out`, per million tokens.
/// A subscription provider costs 0. `None` when a price that is needed is missing.
pub fn cost(tokens: Tokens, prices: Prices) -> Option<f64> {
    if prices.billing == Billing::Subscription {
        return Some(0.0);
    }
    let uncached = if prices.includes_cached { tokens.input.saturating_sub(tokens.cached) } else { tokens.input };
    Some(term(uncached, prices.input)? + term(tokens.cached, prices.cached_input)? + term(tokens.output, prices.output)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metered(input: Option<f64>, output: Option<f64>, cached: Option<f64>, includes_cached: bool) -> Prices {
        Prices { billing: Billing::Metered, input, output, cached_input: cached, includes_cached }
    }
    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    #[test]
    fn cost_is_tokens_times_the_price_per_million() {
        let p = metered(Some(2.5), Some(10.0), Some(0.25), true);
        // 1M uncached in, 1M out, nothing cached: 2.5 + 10.
        assert!(close(cost(Tokens { input: 1_000_000, output: 1_000_000, cached: 0 }, p), 12.5));
        // 100 in, 50 out (the mock's numbers): 0.00025 + 0.0005.
        assert!(close(cost(Tokens { input: 100, output: 50, cached: 0 }, p), 0.00075));
        assert!(close(cost(Tokens::default(), p), 0.0), "no tokens, no cost");
    }

    #[test]
    fn cached_tokens_are_priced_separately_and_taken_out_of_openai_style_input() {
        // OpenAI style: input 1,000,000 includes 600,000 cached: 400,000 at 2.5 + 600,000 at 0.25.
        let openai = metered(Some(2.5), Some(10.0), Some(0.25), true);
        assert!(close(cost(Tokens { input: 1_000_000, output: 0, cached: 600_000 }, openai), 1.0 + 0.15));
        // Anthropic style: the cached tokens are on top of the input count.
        let anthropic = metered(Some(3.0), Some(15.0), Some(0.3), false);
        assert!(close(cost(Tokens { input: 400_000, output: 0, cached: 600_000 }, anthropic), 1.2 + 0.18));
        // More cached than input (odd data) never goes negative.
        assert!(close(cost(Tokens { input: 10, output: 0, cached: 20 }, openai), 20.0 * 0.25 / 1e6));
    }

    #[test]
    fn a_missing_price_that_is_needed_gives_unknown_never_zero() {
        let none = metered(None, None, None, true);
        assert_eq!(cost(Tokens { input: 1, output: 0, cached: 0 }, none), None);
        assert_eq!(cost(Tokens { input: 0, output: 1, cached: 0 }, none), None);
        // Cached tokens with no cached price: unknown, not "priced like input".
        let no_cached = metered(Some(2.0), Some(8.0), None, true);
        assert_eq!(cost(Tokens { input: 100, output: 10, cached: 50 }, no_cached), None);
        // One missing price does not matter when its tokens are zero.
        assert!(close(cost(Tokens { input: 1_000_000, output: 0, cached: 0 }, metered(Some(2.0), None, None, true)), 2.0));
        assert_eq!(cost(Tokens { input: 1_000_000, output: 1, cached: 0 }, metered(Some(2.0), None, None, true)), None);
        // A price of 0 is a price (free), unlike a missing one.
        assert!(close(cost(Tokens { input: 5, output: 5, cached: 0 }, metered(Some(0.0), Some(0.0), None, true)), 0.0));
    }

    #[test]
    fn a_subscription_provider_costs_nothing_whatever_the_prices_say() {
        let sub = Prices { billing: Billing::Subscription, ..metered(None, None, None, true) };
        assert_eq!(cost(Tokens { input: 5_000_000, output: 5_000_000, cached: 1 }, sub), Some(0.0));
        let priced_sub = Prices { billing: Billing::Subscription, ..metered(Some(99.0), Some(99.0), Some(99.0), true) };
        assert_eq!(cost(Tokens { input: 5_000_000, output: 5_000_000, cached: 1 }, priced_sub), Some(0.0));
    }

    #[test]
    fn prices_come_from_the_snapshot_provider_and_anthropic_input_excludes_cached() {
        let mut p = crate::testing::provider("p", "p", "http://x", "m");
        p.price_in = Some(1.0);
        let prices = Prices::of(&p);
        assert_eq!((prices.input, prices.output, prices.includes_cached), (Some(1.0), None, true));
        p.kind = "anthropic".into();
        assert!(!Prices::of(&p).includes_cached);
    }

    #[test]
    fn a_corrected_price_changes_what_the_same_tokens_cost() {
        let t = Tokens { input: 2_000_000, output: 0, cached: 0 };
        assert!(close(cost(t, metered(Some(1.0), Some(1.0), None, true)), 2.0));
        assert!(close(cost(t, metered(Some(1.5), Some(1.0), None, true)), 3.0), "nothing was stored, so history re-prices");
    }
}
