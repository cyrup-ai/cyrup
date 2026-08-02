//! Usage / cost accounting (arch-01 §6.4 / func-01 §11, R-01-036/037).
//!
//! `Usage`/`Cost` themselves live in `cyrup-core` (reused verbatim, never redefined — arch-00 §3).
//! This module owns the pricing function, including the Anthropic 1h cache-write rule.

use crate::model::ModelCost;
use cyrup_core::{Cost, Usage};

/// The four per-1e6-token rates actually applied to a request: `(input, output, cacheRead,
/// cacheWrite)`.
type Rates = (f64, f64, f64, f64);

/// Pick the rates for this request (Pi `calculateCost`'s tier loop, models.ts:640-648).
///
/// The tier key is the request's *total input* usage — `input + cacheRead + cacheWrite` (output is
/// not counted). A tier applies when that total strictly exceeds its `inputTokensAbove`, and the
/// highest such threshold wins; a tier replaces all four base rates, it does not merge with them.
fn select_rates(cost: &ModelCost, u: &Usage) -> Rates {
    let input_tokens = u
        .input
        .saturating_add(u.cache_read)
        .saturating_add(u.cache_write);
    let mut rates: Rates = (cost.input, cost.output, cost.cache_read, cost.cache_write);
    let mut matched: Option<u64> = None;
    for tier in cost.tiers.iter().flatten() {
        if input_tokens > tier.input_tokens_above
            && matched.is_none_or(|m| tier.input_tokens_above > m)
        {
            rates = (tier.input, tier.output, tier.cache_read, tier.cache_write);
            matched = Some(tier.input_tokens_above);
        }
    }
    rates
}

/// Compute the precomputed `Cost` for a `Usage` vector against a model's per-1e6-token rates
/// (func-01 R-01-036). Long-context tiers are resolved first via [`select_rates`]. The Anthropic 1h
/// cache-write rule (R-01-037) prices "long" (1h) cache writes at `2 × input` and the remaining
/// "short" writes at the `cacheWrite` rate — both read from the *selected* tier, matching Pi.
pub fn compute_cost(cost: &ModelCost, u: &Usage) -> Cost {
    let (r_input, r_output, r_cache_read, r_cache_write) = select_rates(cost, u);
    let per = |rate: f64, toks: u64| rate / 1e6 * toks as f64;
    let input = per(r_input, u.input);
    let output = per(r_output, u.output);
    let cache_read = per(r_cache_read, u.cache_read);

    // Anthropic 1h: long writes priced at 2× input; short at the cacheWrite rate (R-01-037).
    let long = u.cache_write_1h.unwrap_or(0);
    let short = u.cache_write.saturating_sub(long);
    let cache_write = (r_cache_write * short as f64 + r_input * 2.0 * long as f64) / 1e6;

    Cost {
        input,
        output,
        cache_read,
        cache_write,
        total: input + output + cache_read + cache_write,
    }
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
        ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_write: 3.75,
            tiers: None,
        }
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
        let u = Usage {
            cache_write: 1_000_000,
            ..Default::default()
        };
        let c = compute_cost(&rates(), &u);
        // all short: 1,000,000 @ 3.75/1e6 = 3.75
        assert!((c.cache_write - 3.75).abs() < 1e-9);
    }

    // ---- Long-context pricing tiers (Pi `calculateCost`, models.ts:639-658) ----

    /// The gpt-5.4 shape from Pi's generator: base rates plus a `2× input / 1.5× output /
    /// 2× cacheRead` tier above 272,000 input tokens.
    fn tiered() -> ModelCost {
        ModelCost {
            input: 2.5,
            output: 15.0,
            cache_read: 0.25,
            cache_write: 0.0,
            tiers: Some(vec![crate::model::ModelCostTier {
                input_tokens_above: 272_000,
                input: 5.0,
                output: 22.5,
                cache_read: 0.5,
                cache_write: 0.0,
            }]),
        }
    }

    #[test]
    fn below_threshold_bills_at_the_base_rates() {
        let u = Usage {
            input: 100_000,
            output: 1_000,
            ..Default::default()
        };
        let c = compute_cost(&tiered(), &u);
        // 100_000 @ 2.5/1e6 = 0.25 ; 1_000 @ 15/1e6 = 0.015
        assert!((c.input - 0.25).abs() < 1e-9, "input {}", c.input);
        assert!((c.output - 0.015).abs() < 1e-9, "output {}", c.output);
    }

    #[test]
    fn above_threshold_bills_input_and_output_at_the_tier_rates() {
        let u = Usage {
            input: 300_000,
            output: 1_000,
            ..Default::default()
        };
        let c = compute_cost(&tiered(), &u);
        // Long context: 300_000 @ 5.0/1e6 = 1.5 (NOT 0.75), 1_000 @ 22.5/1e6 = 0.0225.
        assert!((c.input - 1.5).abs() < 1e-9, "input {}", c.input);
        assert!((c.output - 0.0225).abs() < 1e-9, "output {}", c.output);
        assert!((c.total - 1.5225).abs() < 1e-9, "total {}", c.total);
    }

    #[test]
    fn threshold_is_strictly_exceeded_not_reached() {
        let at = Usage {
            input: 272_000,
            ..Default::default()
        };
        let over = Usage {
            input: 272_001,
            ..Default::default()
        };
        // Exactly at the threshold stays on the base rate (Pi `inputTokens > tier.inputTokensAbove`).
        assert!((compute_cost(&tiered(), &at).input - 0.68).abs() < 1e-9);
        // One token over flips to the tier rate.
        assert!((compute_cost(&tiered(), &over).input - 1.360_005).abs() < 1e-9);
    }

    #[test]
    fn tier_key_counts_cache_reads_and_writes_not_output() {
        // 200k fresh input + 100k cache reads = 300k total input usage -> tier applies, even though
        // no single component exceeds the threshold. Output is not part of the key.
        let u = Usage {
            input: 200_000,
            cache_read: 100_000,
            output: 10_000_000,
            ..Default::default()
        };
        let c = compute_cost(&tiered(), &u);
        assert!((c.input - 1.0).abs() < 1e-9, "input {}", c.input);
        assert!((c.cache_read - 0.05).abs() < 1e-9, "cache_read {}", c.cache_read);
    }

    #[test]
    fn highest_matching_tier_wins_regardless_of_declaration_order() {
        let cost = ModelCost {
            input: 1.0,
            output: 1.0,
            cache_read: 0.0,
            cache_write: 0.0,
            tiers: Some(vec![
                crate::model::ModelCostTier {
                    input_tokens_above: 1_000_000,
                    input: 100.0,
                    output: 1.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
                crate::model::ModelCostTier {
                    input_tokens_above: 200_000,
                    input: 10.0,
                    output: 1.0,
                    cache_read: 0.0,
                    cache_write: 0.0,
                },
            ]),
        };
        // 500k: only the 200k tier matches.
        let mid = Usage {
            input: 500_000,
            ..Default::default()
        };
        assert!((compute_cost(&cost, &mid).input - 5.0).abs() < 1e-9);
        // 2M: both match, the 1M tier (highest threshold) wins even though it is declared first.
        let high = Usage {
            input: 2_000_000,
            ..Default::default()
        };
        assert!((compute_cost(&cost, &high).input - 200.0).abs() < 1e-9);
    }

    #[test]
    fn anthropic_1h_cache_writes_use_the_selected_tier_input_rate() {
        let cost = ModelCost {
            input: 3.0,
            output: 15.0,
            cache_read: 0.30,
            cache_write: 3.75,
            tiers: Some(vec![crate::model::ModelCostTier {
                input_tokens_above: 200_000,
                input: 6.0,
                output: 22.5,
                cache_read: 0.60,
                cache_write: 7.5,
            }]),
        };
        // 1,000,000 cache writes (all 1h) puts total input usage over 200k, so the 1h rule prices
        // them at 2 × the TIER input rate (12.0), not 2 × the base input rate (6.0).
        let u = Usage {
            cache_write: 1_000_000,
            cache_write_1h: Some(1_000_000),
            ..Default::default()
        };
        let c = compute_cost(&cost, &u);
        assert!((c.cache_write - 12.0).abs() < 1e-9, "got {}", c.cache_write);
    }

    #[test]
    fn apply_cost_sets_total_tokens() {
        let mut u = Usage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 0,
            ..Default::default()
        };
        apply_cost(&rates(), &mut u);
        assert_eq!(u.total_tokens, 35);
        assert!(u.cost.total > 0.0);
    }
}
