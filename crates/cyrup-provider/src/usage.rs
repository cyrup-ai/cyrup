//! Usage / cost accounting (arch-01 §6.4 / func-01 §11, R-01-036/037).
//!
//! `Usage`/`Cost` themselves live in `cyrup-core` (reused verbatim, never redefined — arch-00 §3).
//! This module owns the pricing function, including the Anthropic 1h cache-write rule.

use crate::model::ModelCost;
use cyrup_core::{Cost, Usage};

/// Compute the precomputed `Cost` for a `Usage` vector against a model's per-1e6-token rates
/// (func-01 R-01-036). The Anthropic 1h cache-write rule (R-01-037) prices "long" (1h) cache
/// writes at `2 × input` and the remaining "short" writes at the `cacheWrite` rate.
pub fn compute_cost(cost: &ModelCost, u: &Usage) -> Cost {
    let per = |rate: f64, toks: u64| rate / 1e6 * toks as f64;
    let input = per(cost.input, u.input);
    let output = per(cost.output, u.output);
    let cache_read = per(cost.cache_read, u.cache_read);

    // Anthropic 1h: long writes priced at 2× input; short at the cacheWrite rate (R-01-037).
    let long = u.cache_write_1h.unwrap_or(0);
    let short = u.cache_write.saturating_sub(long);
    let cache_write = (cost.cache_write * short as f64 + cost.input * 2.0 * long as f64) / 1e6;

    Cost { input, output, cache_read, cache_write, total: input + output + cache_read + cache_write }
}

/// Recompute `total_tokens` and the precomputed `cost` on a usage vector in place (func-01 R-01-027).
pub fn apply_cost(cost: &ModelCost, usage: &mut Usage) {
    usage.total_tokens = usage.input + usage.output + usage.cache_read + usage.cache_write;
    usage.cost = compute_cost(cost, usage);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn rates() -> ModelCost {
        // USD per 1e6 tokens.
        ModelCost { input: 3.0, output: 15.0, cache_read: 0.30, cache_write: 3.75 }
    }

    #[test]
    fn per_component_and_total() {
        let u = Usage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 0,
            ..Default::default()
        };
        let c = compute_cost(&rates(), &u);
        assert!((c.input - 3.0).abs() < 1e-9);
        assert!((c.output - 15.0).abs() < 1e-9);
        assert!((c.cache_read - 0.30).abs() < 1e-9);
        assert!((c.cache_write).abs() < 1e-9);
        assert!((c.total - 18.30).abs() < 1e-9);
    }

    #[test]
    fn anthropic_1h_cache_write_rule() {
        // 1,000,000 total cache writes, of which 400,000 are 1h (long) writes.
        let u = Usage {
            cache_write: 1_000_000,
            cache_write_1h: Some(400_000),
            ..Default::default()
        };
        let c = compute_cost(&rates(), &u);
        // short = 600,000 @ 3.75/1e6 = 2.25 ; long = 400,000 @ (3.0*2)/1e6 = 2.4 ; total = 4.65
        assert!((c.cache_write - 4.65).abs() < 1e-9, "got {}", c.cache_write);
        assert!((c.total - 4.65).abs() < 1e-9);
    }

    #[test]
    fn long_writes_default_to_zero() {
        let u = Usage { cache_write: 1_000_000, ..Default::default() };
        let c = compute_cost(&rates(), &u);
        // all short: 1,000,000 @ 3.75/1e6 = 3.75
        assert!((c.cache_write - 3.75).abs() < 1e-9);
    }

    #[test]
    fn apply_cost_sets_total_tokens() {
        let mut u = Usage { input: 10, output: 20, cache_read: 5, cache_write: 0, ..Default::default() };
        apply_cost(&rates(), &mut u);
        assert_eq!(u.total_tokens, 35);
        assert!(u.cost.total > 0.0);
    }
}
