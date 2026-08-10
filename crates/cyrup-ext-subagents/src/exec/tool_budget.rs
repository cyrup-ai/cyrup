//! Per-run child tool-call budgets — a 1:1 port of
//! `pi-subagents/src/runs/shared/tool-budget.ts` (present since well before the ported v0.34.0
//! baseline; `agent-serializer.ts`'s `KNOWN_FIELDS` has carried `toolBudget` at every tag this
//! crate could have been cut from).
//!
//! A budget has three parts:
//!
//! * `hard` — after this many tool calls, [`ToolBudgetBlock`]-listed tools are REFUSED so the child
//!   is forced to finalize from the context it already has;
//! * `soft` (optional) — an advisory threshold; the first tool call at or past it earns the child a
//!   one-time nudge, and nothing else;
//! * `block` — which tools the hard limit refuses. Omitted normalizes to
//!   [`DEFAULT_TOOL_BUDGET_BLOCK`] (`read`/`grep`/`find`/`ls` — the browsing tools), and the literal
//!   `"*"` refuses everything.
//!
//! The budget crosses the process boundary as JSON in [`TOOL_BUDGET_ENV`], exactly as pi ships it
//! in `PI_SUBAGENT_TOOL_BUDGET` (`tool-budget.ts:63-71`); the child-side enforcement lives in
//! [`crate::prompt_runtime`], the port of `subagent-prompt-runtime.ts::registerToolBudget`
//! (`subagent-prompt-runtime.ts:171-190`).

use crate::discovery::types::{AllToolsMarker, ResolvedToolBudget, ToolBudgetBlock};

/// pi `DEFAULT_TOOL_BUDGET_BLOCK` (`tool-budget.ts:3`): the browsing/search tools an
/// over-budget child is stopped from starting NEW work with.
pub const DEFAULT_TOOL_BUDGET_BLOCK: [&str; 4] = ["read", "grep", "find", "ls"];

/// pi `TOOL_BUDGET_ENV` (`tool-budget.ts:4`, `PI_SUBAGENT_TOOL_BUDGET`) under this crate's
/// `CYRUP_SUBAGENT_*` rename.
pub const TOOL_BUDGET_ENV: &str = "CYRUP_SUBAGENT_TOOL_BUDGET";

/// pi `normalizeToolBudgetBlock` (`tool-budget.ts:6-10`): `"*"` passes through; an omitted list
/// becomes the default block list; an explicit list is trimmed, emptied-out entries dropped, and
/// de-duplicated with FIRST-occurrence order preserved (JS `new Set(...)` iteration order).
#[must_use]
pub fn normalize_tool_budget_block(block: Option<&ToolBudgetBlock>) -> ToolBudgetBlock {
    match block {
        Some(ToolBudgetBlock::All(_)) => ToolBudgetBlock::All(AllToolsMarker),
        None => ToolBudgetBlock::Names(
            DEFAULT_TOOL_BUDGET_BLOCK
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        ),
        Some(ToolBudgetBlock::Names(names)) => {
            let mut seen = std::collections::HashSet::new();
            ToolBudgetBlock::Names(
                names
                    .iter()
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .filter(|n| seen.insert(n.clone()))
                    .collect(),
            )
        }
    }
}

/// pi `validateToolBudgetConfig` (`tool-budget.ts:12-31`): validate a raw JSON value into a
/// [`ResolvedToolBudget`], or return pi's own error string verbatim.
///
/// `Ok(None)` is pi's `{}` return for `raw === undefined`; every other rejection is `Err(message)`.
/// The `label` is interpolated into the message exactly as pi does, so a frontmatter rejection and
/// an env rejection read differently, as upstream.
///
/// # Errors
/// Returns pi's own validation message when the value is not an object, `hard` is missing/not an
/// integer >= 1, `soft` is present but not an integer >= 1 or exceeds `hard`, or `block` is neither
/// `"*"` nor a non-empty array of non-blank strings.
pub fn validate_tool_budget_config(
    raw: Option<&serde_json::Value>,
    label: &str,
) -> Result<Option<ResolvedToolBudget>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Some(obj) = raw.as_object() else {
        return Err(format!(
            "{label} must be an object with hard and optional soft/block."
        ));
    };

    let hard = match obj.get("hard") {
        Some(v) => match as_positive_integer(v) {
            Some(n) if n >= 1 => n,
            _ => return Err(format!("{label}.hard must be an integer >= 1.")),
        },
        None => return Err(format!("{label}.hard must be an integer >= 1.")),
    };

    let soft = match obj.get("soft") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match as_positive_integer(v) {
            Some(n) if n >= 1 => Some(n),
            _ => return Err(format!("{label}.soft must be an integer >= 1 when provided.")),
        },
    };
    if let Some(soft) = soft
        && soft > hard
    {
        return Err(format!("{label}.soft must be <= {label}.hard."));
    }

    let block = match obj.get("block") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s == "*" => Some(ToolBudgetBlock::All(AllToolsMarker)),
        Some(serde_json::Value::Array(items)) => {
            if items.is_empty() {
                return Err(format!("{label}.block must contain at least one tool name."));
            }
            let mut names = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) if !s.trim().is_empty() => names.push(s.to_string()),
                    _ => return Err(format!("{label}.block must contain non-empty tool names.")),
                }
            }
            Some(ToolBudgetBlock::Names(names))
        }
        Some(_) => {
            return Err(format!(
                "{label}.block must be \"*\" or an array of tool names."
            ));
        }
    };

    Ok(Some(ResolvedToolBudget {
        hard,
        soft,
        block: normalize_tool_budget_block(block.as_ref()),
    }))
}

/// JS `Number.isInteger(v) && v >= 1`: a JSON number that is a NON-NEGATIVE integer. A fractional
/// or negative number, or any non-number, yields `None` so the caller can emit pi's message.
fn as_positive_integer(value: &serde_json::Value) -> Option<u32> {
    let n = value.as_f64()?;
    if !n.is_finite() || n.fract() != 0.0 || n < 0.0 || n > f64::from(u32::MAX) {
        return None;
    }
    // Lossless: `n` is a finite non-negative integer <= u32::MAX, checked immediately above.
    Some(n as u32)
}

/// pi `shouldBlockToolForBudget` (`tool-budget.ts:47-50`): a call is refused only once the count
/// PASSES `hard` (i.e. `nextToolCount > hard`) and the tool is in the block set.
#[must_use]
pub fn should_block_tool_for_budget(
    budget: &ResolvedToolBudget,
    tool_name: &str,
    next_tool_count: u32,
) -> bool {
    if next_tool_count <= budget.hard {
        return false;
    }
    match &budget.block {
        ToolBudgetBlock::All(_) => true,
        ToolBudgetBlock::Names(names) => names.iter().any(|n| n == tool_name),
    }
}

/// pi `toolBudgetSoftNudge` (`tool-budget.ts:52-54`) — verbatim, including the `soft`/`hard`
/// interpolation and the singular/plural "call"/"calls".
#[must_use]
pub fn tool_budget_soft_nudge(budget: &ResolvedToolBudget, tool_count: u32) -> String {
    let plural = if tool_count == 1 { "" } else { "s" };
    // pi interpolates `budget.soft` directly; it is always `Some` at every call site (the nudge
    // only fires when a soft threshold exists), and JS would render an absent one as "undefined".
    let soft = budget
        .soft
        .map_or_else(|| "undefined".to_string(), |s| s.to_string());
    format!(
        "Tool budget soft limit reached after {tool_count} tool call{plural} (soft {soft}, hard {}). Stop starting new browsing/search work and finalize from the context you already have.",
        budget.hard
    )
}

/// pi `toolBudgetBlockedMessage` (`tool-budget.ts:56-58`) — verbatim.
#[must_use]
pub fn tool_budget_blocked_message(
    budget: &ResolvedToolBudget,
    tool_name: &str,
    tool_count: u32,
) -> String {
    let plural = if tool_count == 1 { "" } else { "s" };
    format!(
        "Tool budget hard limit reached after {tool_count} tool call{plural} (hard {}). The '{tool_name}' tool is blocked so you can finalize from the context you already have.",
        budget.hard
    )
}

/// pi `encodeToolBudgetEnv` (`tool-budget.ts:63-65`): the resolved budget as JSON, or `None`.
#[must_use]
pub fn encode_tool_budget_env(budget: Option<&ResolvedToolBudget>) -> Option<String> {
    budget.and_then(|b| serde_json::to_string(b).ok())
}

/// pi `decodeToolBudgetEnv` (`tool-budget.ts:67-72`): parse and re-validate the env payload.
///
/// A blank/absent value is `Ok(None)`. Unlike pi (which lets `JSON.parse` throw), a MALFORMED
/// payload is reported through the same `Err(String)` channel as a semantically-invalid one — the
/// child has no exception to propagate and a panic is forbidden by this workspace's no-panic policy.
///
/// # Errors
/// Returns the validation message when the payload is not JSON or fails
/// [`validate_tool_budget_config`].
pub fn decode_tool_budget_env(value: Option<&str>) -> Result<Option<ResolvedToolBudget>, String> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|err| format!("{TOOL_BUDGET_ENV} is not valid JSON: {err}"))?;
    validate_tool_budget_config(Some(&parsed), TOOL_BUDGET_ENV)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    fn v(json: &str) -> serde_json::Value {
        serde_json::from_str(json).expect("test fixture is valid JSON")
    }

    #[test]
    fn undefined_is_not_an_error() {
        assert_eq!(validate_tool_budget_config(None, "toolBudget"), Ok(None));
    }

    #[test]
    fn hard_must_be_an_integer_at_least_one() {
        for bad in ["{}", "{\"hard\": 0}", "{\"hard\": 1.5}", "{\"hard\": \"3\"}"] {
            assert_eq!(
                validate_tool_budget_config(Some(&v(bad)), "toolBudget"),
                Err("toolBudget.hard must be an integer >= 1.".to_string()),
                "input {bad}"
            );
        }
    }

    #[test]
    fn non_object_roots_are_rejected_with_pis_message() {
        for bad in ["[]", "3", "\"x\"", "null"] {
            assert_eq!(
                validate_tool_budget_config(Some(&v(bad)), "toolBudget"),
                Err("toolBudget must be an object with hard and optional soft/block.".to_string()),
                "input {bad}"
            );
        }
    }

    #[test]
    fn soft_must_be_positive_and_not_exceed_hard() {
        assert_eq!(
            validate_tool_budget_config(Some(&v("{\"hard\": 5, \"soft\": 0}")), "toolBudget"),
            Err("toolBudget.soft must be an integer >= 1 when provided.".to_string())
        );
        assert_eq!(
            validate_tool_budget_config(Some(&v("{\"hard\": 5, \"soft\": 6}")), "toolBudget"),
            Err("toolBudget.soft must be <= toolBudget.hard.".to_string())
        );
    }

    #[test]
    fn block_must_be_star_or_a_non_empty_string_array() {
        assert_eq!(
            validate_tool_budget_config(Some(&v("{\"hard\": 5, \"block\": []}")), "toolBudget"),
            Err("toolBudget.block must contain at least one tool name.".to_string())
        );
        assert_eq!(
            validate_tool_budget_config(Some(&v("{\"hard\": 5, \"block\": [\" \"]}")), "toolBudget"),
            Err("toolBudget.block must contain non-empty tool names.".to_string())
        );
        assert_eq!(
            validate_tool_budget_config(Some(&v("{\"hard\": 5, \"block\": \"all\"}")), "toolBudget"),
            Err("toolBudget.block must be \"*\" or an array of tool names.".to_string())
        );
    }

    #[test]
    fn an_omitted_block_normalizes_to_pis_default_browsing_tools() {
        let budget = validate_tool_budget_config(Some(&v("{\"hard\": 4}")), "toolBudget")
            .expect("valid")
            .expect("some");
        assert_eq!(budget.hard, 4);
        assert_eq!(budget.soft, None);
        assert_eq!(
            budget.block,
            ToolBudgetBlock::Names(vec![
                "read".into(),
                "grep".into(),
                "find".into(),
                "ls".into()
            ])
        );
    }

    #[test]
    fn an_explicit_block_is_trimmed_and_deduplicated_in_first_seen_order() {
        let budget = validate_tool_budget_config(
            Some(&v("{\"hard\": 2, \"block\": [\" bash \", \"read\", \"bash\"]}")),
            "toolBudget",
        )
        .expect("valid")
        .expect("some");
        assert_eq!(
            budget.block,
            ToolBudgetBlock::Names(vec!["bash".into(), "read".into()])
        );
    }

    #[test]
    fn star_blocks_every_tool_once_hard_is_passed() {
        let budget = validate_tool_budget_config(
            Some(&v("{\"hard\": 2, \"block\": \"*\"}")),
            "toolBudget",
        )
        .expect("valid")
        .expect("some");
        assert!(!should_block_tool_for_budget(&budget, "anything", 2));
        assert!(should_block_tool_for_budget(&budget, "anything", 3));
    }

    #[test]
    fn a_named_block_only_refuses_the_listed_tools() {
        let budget = validate_tool_budget_config(Some(&v("{\"hard\": 1}")), "toolBudget")
            .expect("valid")
            .expect("some");
        assert!(should_block_tool_for_budget(&budget, "read", 2));
        assert!(!should_block_tool_for_budget(&budget, "bash", 2));
        assert!(!should_block_tool_for_budget(&budget, "read", 1));
    }

    #[test]
    fn messages_match_upstream_text_including_pluralization() {
        let budget = validate_tool_budget_config(
            Some(&v("{\"hard\": 3, \"soft\": 1}")),
            "toolBudget",
        )
        .expect("valid")
        .expect("some");
        assert_eq!(
            tool_budget_soft_nudge(&budget, 1),
            "Tool budget soft limit reached after 1 tool call (soft 1, hard 3). Stop starting new browsing/search work and finalize from the context you already have."
        );
        assert_eq!(
            tool_budget_blocked_message(&budget, "read", 4),
            "Tool budget hard limit reached after 4 tool calls (hard 3). The 'read' tool is blocked so you can finalize from the context you already have."
        );
    }

    #[test]
    fn env_round_trips_through_encode_and_decode() {
        let budget = validate_tool_budget_config(
            Some(&v("{\"hard\": 6, \"soft\": 2, \"block\": [\"read\"]}")),
            "toolBudget",
        )
        .expect("valid")
        .expect("some");
        let encoded = encode_tool_budget_env(Some(&budget)).expect("encodes");
        assert_eq!(decode_tool_budget_env(Some(&encoded)), Ok(Some(budget)));
        assert_eq!(encode_tool_budget_env(None), None);
        assert_eq!(decode_tool_budget_env(None), Ok(None));
        assert_eq!(decode_tool_budget_env(Some("   ")), Ok(None));
    }

    #[test]
    fn a_star_block_survives_the_env_round_trip_as_the_star_literal() {
        let budget = validate_tool_budget_config(
            Some(&v("{\"hard\": 1, \"block\": \"*\"}")),
            "toolBudget",
        )
        .expect("valid")
        .expect("some");
        let encoded = encode_tool_budget_env(Some(&budget)).expect("encodes");
        assert!(encoded.contains("\"block\":\"*\""), "encoded: {encoded}");
        assert_eq!(decode_tool_budget_env(Some(&encoded)), Ok(Some(budget)));
    }

    #[test]
    fn a_malformed_env_payload_is_an_error_not_a_panic() {
        assert!(decode_tool_budget_env(Some("{not json")).is_err());
        assert_eq!(
            decode_tool_budget_env(Some("{\"hard\": 0}")),
            Err(format!("{TOOL_BUDGET_ENV}.hard must be an integer >= 1."))
        );
    }
}
