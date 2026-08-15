//! SUBA-021 (second half) — the per-run USAGE BUDGET: the Rust port of
//! `pi-subagents/src/runs/shared/usage-budget.ts` (in-baseline — present at both v0.43.0 and
//! v0.47.1, which is what killed this item's "post-baseline" framing).
//!
//! A usage budget bounds a run by *reported* consumption rather than by wall clock or turn count:
//!
//! * `tokens` — input + output tokens summed across the run;
//! * `costUsd` — the run's reported dollar cost.
//!
//! Each metric takes a `hard` limit and an optional `soft` one, and every evaluation lands on one of
//! three outcomes ([`UsageBudgetOutcome`]). `soft` is advisory — it flags the run without ending it.
//! `hard` is terminal: [`usage_budget_state`] sets `exhausted`, and the consuming runner ends the
//! run with [`usage_budget_exceeded_message`] as its error, which is the shape SUBA-021's Verify
//! line asks for (*"a run exceeding its usage budget must terminate with pi's budget result
//! shape"*).
//!
//! # Two things this is NOT
//!
//! * It is not a pre-flight estimate. Upstream's own `source: "reported"`
//!   (`usage-budget.ts:53`) says so: the state is recomputed from totals the child has ALREADY
//!   reported, so the bound is enforced at the next evaluation point, not predictively. A single
//!   turn can therefore overshoot `hard` — upstream accepts that and so does this port.
//! * It is not the turn budget ([`crate::exec::turn_budget`]) and it is not the tool budget
//!   ([`crate::exec::tool_budget`]). Those bound *how many* turns/tool calls a child takes; this
//!   bounds what those calls cost. All three can be live at once.

use serde::{Deserialize, Serialize};

/// pi `UsageBudgetLimitConfig` (`shared/types.ts`) — one metric's bounds.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBudgetLimit {
    /// pi `soft?` — advisory. Flagged, never terminal. Must be `<= hard` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft: Option<f64>,
    /// pi `hard` — terminal. A positive, finite number; there is no "unlimited" spelling, because an
    /// unlimited metric is expressed by omitting the metric.
    pub hard: f64,
}

/// pi `UsageBudgetConfig` (`shared/types.ts`) — at least one metric must be present
/// (`usage-budget.ts:31`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBudgetConfig {
    /// pi `tokens?` — bounds `inputTokens + outputTokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<UsageBudgetLimit>,
    /// pi `costUsd?` — bounds the reported dollar cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<UsageBudgetLimit>,
}

/// pi `UsageBudgetState["tokens"]["outcome"]` (`usage-budget.ts:40`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UsageBudgetOutcome {
    /// Below every limit this metric declares.
    WithinBudget,
    /// At or past `soft`, below `hard` — advisory only.
    SoftExceeded,
    /// At or past `hard` — terminal.
    HardExceeded,
}

/// pi's per-metric state (`usage-budget.ts:35-42`): the limits, what was used, and the outcome.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBudgetMetricState {
    /// pi `soft?`, carried through from the config by upstream's `...limit` spread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft: Option<f64>,
    /// pi `hard`.
    pub hard: f64,
    /// pi `used` — the reported total for this metric.
    pub used: f64,
    /// pi `outcome`.
    pub outcome: UsageBudgetOutcome,
}

/// pi `UsageBudgetState` (`shared/types.ts`), the object a run's `status.json`/result carries as
/// `usageBudget` (`subagent-runner.ts:4411`, `async-status.ts:336`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBudgetState {
    /// pi `version: 1`.
    pub version: u32,
    /// pi `source: "reported"` — see the module doc's first "is NOT".
    pub source: UsageBudgetSource,
    /// pi `tokens?`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<UsageBudgetMetricState>,
    /// pi `costUsd?`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<UsageBudgetMetricState>,
    /// pi `exhausted` — true iff some metric is `hard-exceeded`.
    pub exhausted: bool,
    /// pi `reason?` — which metric exhausted it, `tokens` winning a tie (`usage-budget.ts:50`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UsageBudgetReason>,
}

/// pi's literal `source: "reported"` (`usage-budget.ts:53`), as a one-variant enum rather than a
/// `&'static str` so [`UsageBudgetState`] can derive `Deserialize` — `SingleResult` embeds it and
/// must round-trip through a result file like every other field on that struct.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageBudgetSource {
    /// The state was computed from totals the child has ALREADY reported — never an estimate.
    #[default]
    Reported,
}

/// pi's `reason` discriminant (`usage-budget.ts:50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageBudgetReason {
    /// The token metric hit its hard limit.
    Tokens,
    /// The cost metric hit its hard limit.
    CostUsd,
}

/// The reported totals a state is computed against — pi's `CostSummary` subset
/// (`usage-budget.ts:44-49`), where every absent field reads as `0`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsageTotals {
    /// pi `totals?.inputTokens ?? 0`.
    pub input_tokens: f64,
    /// pi `totals?.outputTokens ?? 0`.
    pub output_tokens: f64,
    /// pi `totals?.costUsd ?? 0`.
    pub cost_usd: f64,
}

impl From<&cyrup_core::Usage> for UsageTotals {
    /// pi's `currentUsageTotals()` (`subagent-runner.ts:2135`) reads a `CostSummary`; cyrup's
    /// equivalent aggregate is `cyrup_core::Usage`, whose `input`/`output`/`cost.total` are the same
    /// three numbers.
    ///
    /// Cache-read/cache-write tokens are deliberately EXCLUDED from the token metric, matching
    /// upstream's `inputTokens + outputTokens` (`usage-budget.ts:46-48`) and this crate's existing
    /// `progress.tokens` convention (`AgentProgress::snapshot`, pi `execution.ts:646`).
    fn from(usage: &cyrup_core::Usage) -> Self {
        Self {
            #[allow(clippy::cast_precision_loss)]
            input_tokens: usage.input as f64,
            #[allow(clippy::cast_precision_loss)]
            output_tokens: usage.output as f64,
            cost_usd: usage.cost.total,
        }
    }
}

/// pi `validateLimit` (`usage-budget.ts:3-12`) — one metric's validation, with upstream's four
/// verbatim errors.
///
/// # Errors
///
/// `<label> must be an object.`, `<label>.<key> is not supported.`, `<label>.hard must be a
/// positive number.`, `<label>.soft must be a positive number.`, and `<label>.soft must be less
/// than or equal to <label>.hard.`
fn validate_limit(value: &serde_json::Value, label: &str) -> Result<UsageBudgetLimit, String> {
    let Some(raw) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    if let Some(unknown) = raw.keys().find(|key| *key != "soft" && *key != "hard") {
        return Err(format!("{label}.{unknown} is not supported."));
    }
    let hard = raw.get("hard").and_then(serde_json::Value::as_f64);
    let Some(hard) = hard.filter(|h| h.is_finite() && *h > 0.0) else {
        return Err(format!("{label}.hard must be a positive number."));
    };
    let soft = match raw.get("soft") {
        None | Some(serde_json::Value::Null) => None,
        Some(raw_soft) => {
            let Some(soft) = raw_soft
                .as_f64()
                .filter(|s| s.is_finite() && *s > 0.0)
            else {
                return Err(format!("{label}.soft must be a positive number."));
            };
            if soft > hard {
                return Err(format!(
                    "{label}.soft must be less than or equal to {label}.hard."
                ));
            }
            Some(soft)
        }
    };
    Ok(UsageBudgetLimit { soft, hard })
}

/// pi `validateUsageBudgetConfig` (`usage-budget.ts:14-33`) — `Ok(None)` for an absent budget,
/// `Err` with upstream's verbatim text for a malformed one.
///
/// Note the `label` parameter: upstream passes `"usageBudget"` for the tool parameter and a
/// dotted config path elsewhere, so the SAME validator produces the right sentence on either
/// surface. That is why this takes a label rather than hard-coding one.
///
/// # Errors
///
/// `<label> must be an object.`, `<label>.<key> is not supported.`, `<label> must include tokens or
/// costUsd.`, and everything [`validate_limit`] raises.
pub fn validate_usage_budget_config(
    value: Option<&serde_json::Value>,
    label: &str,
) -> Result<Option<UsageBudgetConfig>, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let Some(raw) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    if let Some(unknown) = raw
        .keys()
        .find(|key| *key != "tokens" && *key != "costUsd")
    {
        return Err(format!("{label}.{unknown} is not supported."));
    }
    let mut budget = UsageBudgetConfig::default();
    if let Some(tokens) = raw.get("tokens").filter(|v| !v.is_null()) {
        budget.tokens = Some(validate_limit(tokens, &format!("{label}.tokens"))?);
    }
    if let Some(cost) = raw.get("costUsd").filter(|v| !v.is_null()) {
        budget.cost_usd = Some(validate_limit(cost, &format!("{label}.costUsd"))?);
    }
    if budget.tokens.is_none() && budget.cost_usd.is_none() {
        return Err(format!("{label} must include tokens or costUsd."));
    }
    Ok(Some(budget))
}

/// pi `metricState` (`usage-budget.ts:35-42`). Both comparisons are `>=`, so landing EXACTLY on a
/// limit trips it — a budget of 1000 tokens is spent at 1000, not at 1001.
fn metric_state(limit: Option<UsageBudgetLimit>, used: f64) -> Option<UsageBudgetMetricState> {
    let limit = limit?;
    let outcome = if used >= limit.hard {
        UsageBudgetOutcome::HardExceeded
    } else if limit.soft.is_some_and(|soft| used >= soft) {
        UsageBudgetOutcome::SoftExceeded
    } else {
        UsageBudgetOutcome::WithinBudget
    };
    Some(UsageBudgetMetricState {
        soft: limit.soft,
        hard: limit.hard,
        used,
        outcome,
    })
}

/// pi `usageBudgetState` (`usage-budget.ts:44-59`) — the projection a run publishes and the runner
/// branches on. `None` for a run that declared no budget, which is what keeps the field off the wire
/// entirely for unbudgeted runs.
#[must_use]
pub fn usage_budget_state(
    config: Option<UsageBudgetConfig>,
    totals: Option<UsageTotals>,
) -> Option<UsageBudgetState> {
    let config = config?;
    let totals = totals.unwrap_or_default();
    let tokens = metric_state(
        config.tokens,
        totals.input_tokens + totals.output_tokens,
    );
    let cost_usd = metric_state(config.cost_usd, totals.cost_usd);
    // pi's ternary chain: `tokens` wins the tie, and only a HARD breach produces a reason.
    let reason = if tokens.is_some_and(|t| t.outcome == UsageBudgetOutcome::HardExceeded) {
        Some(UsageBudgetReason::Tokens)
    } else if cost_usd.is_some_and(|c| c.outcome == UsageBudgetOutcome::HardExceeded) {
        Some(UsageBudgetReason::CostUsd)
    } else {
        None
    };
    Some(UsageBudgetState {
        version: 1,
        source: UsageBudgetSource::Reported,
        tokens,
        cost_usd,
        exhausted: reason.is_some(),
        reason,
    })
}

/// pi `usageBudgetExceededMessage` (`usage-budget.ts:61-65`), verbatim — including the SIX-decimal
/// cost rendering (`toFixed(6)`), which is what makes a sub-cent budget's message readable.
#[must_use]
pub fn usage_budget_exceeded_message(state: &UsageBudgetState) -> String {
    match (state.reason, state.tokens, state.cost_usd) {
        (Some(UsageBudgetReason::Tokens), Some(tokens), _) => format!(
            "Usage budget exhausted: reported tokens {} reached hard limit {}.",
            format_number(tokens.used),
            format_number(tokens.hard)
        ),
        (Some(UsageBudgetReason::CostUsd), _, Some(cost)) => format!(
            "Usage budget exhausted: reported cost ${:.6} reached hard limit ${:.6}.",
            cost.used, cost.hard
        ),
        _ => "Usage budget exhausted.".to_string(),
    }
}

/// JS renders a whole `number` without a trailing `.0` (`${1000}` is `"1000"`, not `"1000.0"`), and
/// the token half of [`usage_budget_exceeded_message`] interpolates raw numbers. Reproducing that is
/// what keeps the message byte-identical for the integral token counts it always carries in
/// practice.
fn format_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn budget(json: serde_json::Value) -> UsageBudgetConfig {
        validate_usage_budget_config(Some(&json), "usageBudget")
            .expect("valid")
            .expect("present")
    }

    /// THE behaviour SUBA-021's Verify line asks for: a run past its hard limit is exhausted and
    /// carries pi's verbatim message. Before this module there was no token/cost bound on a run at
    /// all beyond the model's own limits. pi `usageBudgetState`/`usageBudgetExceededMessage`
    /// (`usage-budget.ts:44-65`).
    #[test]
    fn a_run_past_its_hard_token_limit_is_exhausted_with_pis_verbatim_message() {
        let config = budget(serde_json::json!({ "tokens": { "soft": 800, "hard": 1000 } }));

        let within = usage_budget_state(
            Some(config),
            Some(UsageTotals {
                input_tokens: 400.0,
                output_tokens: 399.0,
                ..UsageTotals::default()
            }),
        )
        .expect("budgeted");
        assert!(!within.exhausted);
        assert_eq!(
            within.tokens.expect("tokens").outcome,
            UsageBudgetOutcome::WithinBudget
        );

        // `soft` flags but never terminates.
        let soft = usage_budget_state(
            Some(config),
            Some(UsageTotals {
                input_tokens: 800.0,
                ..UsageTotals::default()
            }),
        )
        .expect("budgeted");
        assert!(!soft.exhausted, "soft is advisory");
        assert_eq!(
            soft.tokens.expect("tokens").outcome,
            UsageBudgetOutcome::SoftExceeded
        );

        // Landing EXACTLY on `hard` trips it (`used >= limit.hard`).
        let hard = usage_budget_state(
            Some(config),
            Some(UsageTotals {
                input_tokens: 600.0,
                output_tokens: 400.0,
                ..UsageTotals::default()
            }),
        )
        .expect("budgeted");
        assert!(hard.exhausted);
        assert_eq!(hard.reason, Some(UsageBudgetReason::Tokens));
        assert_eq!(
            usage_budget_exceeded_message(&hard),
            "Usage budget exhausted: reported tokens 1000 reached hard limit 1000."
        );
    }

    /// The cost metric, and its SIX-decimal rendering (`toFixed(6)`), which a sub-cent budget needs
    /// to be readable at all.
    #[test]
    fn the_cost_metric_exhausts_with_pis_six_decimal_rendering() {
        let config = budget(serde_json::json!({ "costUsd": { "hard": 0.5 } }));
        let state = usage_budget_state(
            Some(config),
            Some(UsageTotals {
                cost_usd: 0.512_5,
                ..UsageTotals::default()
            }),
        )
        .expect("budgeted");
        assert_eq!(state.reason, Some(UsageBudgetReason::CostUsd));
        assert_eq!(
            usage_budget_exceeded_message(&state),
            "Usage budget exhausted: reported cost $0.512500 reached hard limit $0.500000."
        );

        // `tokens` wins a tie (`usage-budget.ts:50`).
        let both = budget(serde_json::json!({
            "tokens": { "hard": 10 },
            "costUsd": { "hard": 1 }
        }));
        let state = usage_budget_state(
            Some(both),
            Some(UsageTotals {
                input_tokens: 10.0,
                output_tokens: 0.0,
                cost_usd: 5.0,
            }),
        )
        .expect("budgeted");
        assert_eq!(state.reason, Some(UsageBudgetReason::Tokens));
    }

    /// pi `validateUsageBudgetConfig`/`validateLimit` (`usage-budget.ts:3-33`) — every refusal,
    /// byte-for-byte, on BOTH labels upstream uses the validator with.
    #[test]
    fn a_malformed_usage_budget_is_refused_with_pis_verbatim_texts() {
        assert_eq!(
            validate_usage_budget_config(None, "usageBudget").expect("absent is fine"),
            None
        );
        for (input, expected) in [
            (serde_json::json!([]), "usageBudget must be an object."),
            (
                serde_json::json!({ "seconds": 1 }),
                "usageBudget.seconds is not supported.",
            ),
            (
                serde_json::json!({}),
                "usageBudget must include tokens or costUsd.",
            ),
            (
                serde_json::json!({ "tokens": 5 }),
                "usageBudget.tokens must be an object.",
            ),
            (
                serde_json::json!({ "tokens": { "max": 5 } }),
                "usageBudget.tokens.max is not supported.",
            ),
            (
                serde_json::json!({ "tokens": { "hard": 0 } }),
                "usageBudget.tokens.hard must be a positive number.",
            ),
            (
                serde_json::json!({ "tokens": { "hard": -1 } }),
                "usageBudget.tokens.hard must be a positive number.",
            ),
            (
                serde_json::json!({ "costUsd": { "hard": 1, "soft": 0 } }),
                "usageBudget.costUsd.soft must be a positive number.",
            ),
            (
                serde_json::json!({ "costUsd": { "hard": 1, "soft": 2 } }),
                "usageBudget.costUsd.soft must be less than or equal to usageBudget.costUsd.hard.",
            ),
        ] {
            assert_eq!(
                validate_usage_budget_config(Some(&input), "usageBudget").expect_err("refused"),
                expected,
                "for {input}"
            );
        }
        // The label is a parameter, so the config surface produces its own dotted sentence.
        assert_eq!(
            validate_usage_budget_config(
                Some(&serde_json::json!({ "seconds": 1 })),
                "config.usageBudget"
            )
            .expect_err("refused"),
            "config.usageBudget.seconds is not supported."
        );
    }

    /// An unbudgeted run publishes NOTHING (`config` undefined → `undefined`), which is what keeps
    /// `usageBudget` off the wire entirely rather than shipping a vacuous "unlimited" object.
    #[test]
    fn an_unbudgeted_run_has_no_state_and_the_wire_shape_is_pis_camel_case() {
        assert_eq!(usage_budget_state(None, None), None);

        let state = usage_budget_state(
            Some(budget(serde_json::json!({ "costUsd": { "hard": 2 } }))),
            None,
        )
        .expect("budgeted");
        assert_eq!(state.cost_usd.expect("cost").used, 0.0, "absent totals read as 0");
        let value = serde_json::to_value(state).expect("serializes");
        assert_eq!(value["version"], 1);
        assert_eq!(value["source"], "reported");
        assert_eq!(value["exhausted"], false);
        assert!(value.get("tokens").is_none(), "an undeclared metric is omitted");
        assert!(value.get("reason").is_none());
        assert_eq!(value["costUsd"]["outcome"], "within-budget");
    }
}
