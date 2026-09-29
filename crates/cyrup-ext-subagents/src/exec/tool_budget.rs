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
//! in `PI_SUBAGENT_TOOL_BUDGET` (`tool-budget.ts:70-80`); the child-side enforcement lives in
//! [`crate::prompt_runtime`], the port of `subagent-prompt-runtime.ts::registerToolBudget`
//! (`subagent-prompt-runtime.ts:306-325`).

use crate::discovery::types::{AllToolsMarker, ResolvedToolBudget, ToolBudgetBlock};

/// pi `DEFAULT_TOOL_BUDGET_BLOCK` (`tool-budget.ts:3`): the browsing/search tools an
/// over-budget child is stopped from starting NEW work with.
pub const DEFAULT_TOOL_BUDGET_BLOCK: [&str; 4] = ["read", "grep", "find", "ls"];

/// pi `TOOL_BUDGET_ENV` (`tool-budget.ts:4`, `PI_SUBAGENT_TOOL_BUDGET`) under this crate's
/// `CYRUP_SUBAGENT_*` rename.
pub const TOOL_BUDGET_ENV: &str = "CYRUP_SUBAGENT_TOOL_BUDGET";

/// pi `TOOL_BUDGET_ZERO_AUTH_ENV` (`tool-budget.ts:5`, `PI_SUBAGENT_TOOL_BUDGET_ZERO_AUTH`, added
/// at v0.36.0) under the same rename: the parent's authorisation for a `hard: 0` budget — "this
/// child may make no tool calls at all" — which is otherwise rejected on decode exactly as any
/// other `hard < 1`. Written `"1"` or not at all (`pi-args.ts:1032` @v0.64.0), read child-side by
/// [`HardMinimum::from_env`] (`subagent-prompt-runtime.ts:693`).
///
/// No `PI_` read alias, matching its sibling [`TOOL_BUDGET_ENV`] in this module: both are written
/// by THIS crate's parent into the child env, never expected from an operator's shell.
///
/// `[CYRUP-DELTA]`: upstream **removed** this env var at v0.67.0 (`git grep ZERO_AUTH` is non-empty
/// at v0.64.0 and empty at v0.67.0/v0.68.0), because its children became in-process and carry
/// the authorisation on `ChildRuntimeConfig.toolBudget` (`subagent-prompt-runtime.ts:453`
/// @v0.68.0). cyrup still spawns a subprocess per child, so the env var remains the right
/// transport here. Its only upstream producer is the prompt-template delegation bridge
/// (`slash/delegation-adapters.ts:301` @v0.68.0), which is `SUBA-022`; until that exists nothing
/// in this crate writes it, and a public `hard: 0` is refused on every surface, as upstream's
/// public path (`minimumHard: 1`) refuses it.
pub const TOOL_BUDGET_ZERO_AUTH_ENV: &str = "CYRUP_SUBAGENT_TOOL_BUDGET_ZERO_AUTH";

/// pi's `options.minimumHard?: 0 | 1` on `validateToolBudgetConfig` (`tool-budget.ts:16, 21`):
/// the lowest `hard` a budget may declare. Upstream's type admits exactly these two values, so the
/// Rust side is a two-variant enum rather than an integer that would accept `7`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HardMinimum {
    /// The default: a budget must allow at least one tool call.
    #[default]
    One,
    /// Authorised by [`TOOL_BUDGET_ZERO_AUTH_ENV`]: a `hard: 0` budget is legal and blocks the
    /// child's FIRST call to any blocked tool.
    Zero,
}

impl HardMinimum {
    /// pi `{ allowZero: process.env[TOOL_BUDGET_ZERO_AUTH_ENV] === "1" }`
    /// (`subagent-prompt-runtime.ts:693`) — exact string equality, no trim, as upstream.
    #[must_use]
    pub fn from_env(get: &dyn Fn(&str) -> Option<String>) -> Self {
        match get(TOOL_BUDGET_ZERO_AUTH_ENV).as_deref() {
            Some("1") => Self::Zero,
            _ => Self::One,
        }
    }

    const fn floor(self) -> u32 {
        match self {
            Self::One => 1,
            Self::Zero => 0,
        }
    }
}

/// pi `normalizeToolBudgetBlock` (`tool-budget.ts:7-11`): `"*"` passes through; an omitted list
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

/// pi `validateToolBudgetConfig` (`tool-budget.ts:13-39`): validate a raw JSON value into a
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
    validate_tool_budget_config_with(raw, label, HardMinimum::One)
}

/// [`validate_tool_budget_config`] with pi's `options.minimumHard` made explicit
/// (`tool-budget.ts:13-24`): the `hard` floor is `minimum`, and the rejection message interpolates
/// it (`${label}.hard must be an integer >= ${minimumHard}.`) exactly as upstream.
///
/// # Errors
/// As [`validate_tool_budget_config`], with the `hard` floor lowered to `0` under
/// [`HardMinimum::Zero`].
pub fn validate_tool_budget_config_with(
    raw: Option<&serde_json::Value>,
    label: &str,
    minimum: HardMinimum,
) -> Result<Option<ResolvedToolBudget>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let Some(obj) = raw.as_object() else {
        return Err(format!(
            "{label} must be an object with hard and optional soft/block."
        ));
    };

    let floor = minimum.floor();
    let hard = match obj.get("hard") {
        Some(v) => match as_positive_integer(v) {
            Some(n) if n >= floor => n,
            _ => return Err(format!("{label}.hard must be an integer >= {floor}.")),
        },
        None => return Err(format!("{label}.hard must be an integer >= {floor}.")),
    };

    let soft = match obj.get("soft") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => match as_positive_integer(v) {
            Some(n) if n >= 1 => Some(n),
            _ => {
                return Err(format!(
                    "{label}.soft must be an integer >= 1 when provided."
                ));
            }
        },
    };
    if let Some(soft) = soft
        && soft > hard
    {
        return Err(format!("{label}.soft must be <= {label}.hard."));
    }

    let block = match obj.get("block") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(s)) if s == "*" => {
            Some(ToolBudgetBlock::All(AllToolsMarker))
        }
        Some(serde_json::Value::Array(items)) => {
            if items.is_empty() {
                return Err(format!(
                    "{label}.block must contain at least one tool name."
                ));
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

/// pi `shouldBlockToolForBudget` (`tool-budget.ts:57-60`): a call is refused only once the count
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

/// pi `toolBudgetSoftNudge` (`tool-budget.ts:62-64`) — verbatim, including the `soft`/`hard`
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

/// pi `toolBudgetBlockedMessage` (`tool-budget.ts:66-68`) — verbatim.
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

/// pi `TOOL_BUDGET_BLOCKED_MESSAGE` (`tool-budget.ts:69`) — the recognizer for the exact string
/// [`tool_budget_blocked_message`] renders, anchored at both ends so an incidental occurrence of the
/// phrase inside ordinary tool output cannot be mistaken for a block. The three captures are
/// upstream's own (`(\d+)` count, `(\d+)` hard, `'([^']+)'` tool).
///
/// `(?-u:\d)` because upstream builds the `RegExp` without the `u` flag, which makes JavaScript's
/// `\d` ASCII-only; the Rust engine's default `\d` also matches other Unicode decimal digits, and a
/// count written in Devanagari digits is not a string this run rendered.
static TOOL_BUDGET_BLOCKED_MESSAGE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(
    || {
        regex::Regex::new(
            r"^Tool budget hard limit reached after ((?-u:\d)+) tool calls? \(hard ((?-u:\d)+)\)\. The '([^']+)' tool is blocked so you can finalize from the context you already have\.$",
        )
        .unwrap_or_else(|_| {
            unreachable!("TOOL_BUDGET_BLOCKED_MESSAGE is a literal and always compiles")
        })
    },
);

/// pi `isToolBudgetBlockedMessage(budget, resultText, blockedTool)` (`tool-budget.ts:76-84`).
///
/// A tool result counts as a hard block only when the WHOLE trimmed message matches the runtime
/// format AND the embedded hard limit and tool name belong to THIS run — upstream's own comment at
/// `:70-75`: "incidental occurrences of the phrase inside ordinary tool output are rejected".
///
/// SUBA-118 — the parent needs this because
/// [`crate::exec::abort_recovery::plan_abort_recovery`]'s `tool_budget_exhausted` rung
/// (`abort-recovery.ts:111`) refuses a resume for a child that was stopped by its budget, and the
/// budget is enforced CHILD-side (it crosses as [`TOOL_BUDGET_ENV`]), so the child's own blocked
/// tool result is the only evidence the parent ever sees.
#[must_use]
pub fn is_tool_budget_blocked_message(
    budget: &ResolvedToolBudget,
    result_text: &str,
    blocked_tool: Option<&str>,
) -> bool {
    let Some(tool) = blocked_tool.map(str::trim).filter(|t| !t.is_empty()) else {
        return false;
    };
    let Some(captures) = TOOL_BUDGET_BLOCKED_MESSAGE.captures(result_text.trim()) else {
        return false;
    };
    let count = captures.get(1).and_then(|m| m.as_str().parse::<u64>().ok());
    let hard = captures.get(2).and_then(|m| m.as_str().parse::<u64>().ok());
    let named = captures.get(3).map(|m| m.as_str());
    // Upstream's `Number(hard)` / `Number(blockedToolCount)` cannot fail on a `\d+` capture; a Rust
    // parse still can (an integer wider than `u64`), and a message claiming an absurd count is not
    // one this run rendered, so an unparseable capture is NOT a block.
    match (count, hard, named) {
        (Some(count), Some(hard), Some(named)) => {
            hard == u64::from(budget.hard) && count > u64::from(budget.hard) && named == tool
        }
        _ => false,
    }
}

/// pi `encodeToolBudgetEnv` (`tool-budget.ts:70-72`): the resolved budget as JSON, or `None`.
#[must_use]
pub fn encode_tool_budget_env(budget: Option<&ResolvedToolBudget>) -> Option<String> {
    budget.and_then(|b| serde_json::to_string(b).ok())
}

/// pi `decodeToolBudgetEnv(value, { allowZero })` (`tool-budget.ts:74-80`): parse and re-validate
/// the env payload, with `minimum` standing for upstream's `allowZero ? { minimumHard: 0 } :
/// undefined` — the caller states the authorisation at the call site rather than this function
/// reading the env for it.
///
/// A blank/absent value is `Ok(None)`. Unlike pi (which lets `JSON.parse` throw), a MALFORMED
/// payload is reported through the same `Err(String)` channel as a semantically-invalid one — the
/// child has no exception to propagate and a panic is forbidden by this workspace's no-panic policy.
///
/// # Errors
/// Returns the validation message when the payload is not JSON or fails
/// [`validate_tool_budget_config_with`].
pub fn decode_tool_budget_env(
    value: Option<&str>,
    minimum: HardMinimum,
) -> Result<Option<ResolvedToolBudget>, String> {
    let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let parsed: serde_json::Value = serde_json::from_str(value)
        .map_err(|err| format!("{TOOL_BUDGET_ENV} is not valid JSON: {err}"))?;
    validate_tool_budget_config_with(Some(&parsed), TOOL_BUDGET_ENV, minimum)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

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
        for bad in [
            "{}",
            "{\"hard\": 0}",
            "{\"hard\": 1.5}",
            "{\"hard\": \"3\"}",
        ] {
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
            validate_tool_budget_config(
                Some(&v("{\"hard\": 5, \"block\": [\" \"]}")),
                "toolBudget"
            ),
            Err("toolBudget.block must contain non-empty tool names.".to_string())
        );
        assert_eq!(
            validate_tool_budget_config(
                Some(&v("{\"hard\": 5, \"block\": \"all\"}")),
                "toolBudget"
            ),
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
            Some(&v(
                "{\"hard\": 2, \"block\": [\" bash \", \"read\", \"bash\"]}",
            )),
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
        let budget =
            validate_tool_budget_config(Some(&v("{\"hard\": 2, \"block\": \"*\"}")), "toolBudget")
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

    /// SUBA-118 — `isToolBudgetBlockedMessage` (`tool-budget.ts:76-84`): the message
    /// [`tool_budget_blocked_message`] rendered for THIS budget and THIS tool is recognized, and
    /// nothing else is.
    ///
    /// RED without the recognizer: the parent has no evidence its child was stopped by its budget, so
    /// `plan_abort_recovery`'s `tool_budget_exhausted` rung (`abort-recovery.ts:111`) can never fire
    /// and such a child is resumed where upstream settles.
    #[test]
    fn a_hard_block_message_is_recognized_only_for_its_own_budget_and_tool() {
        let budget = validate_tool_budget_config(Some(&v("{\"hard\": 3}")), "toolBudget")
            .expect("valid")
            .expect("some");
        let rendered = tool_budget_blocked_message(&budget, "read", 4);
        assert!(is_tool_budget_blocked_message(
            &budget,
            &rendered,
            Some("read")
        ));
        // Upstream trims the result text before matching (`:80`).
        assert!(is_tool_budget_blocked_message(
            &budget,
            &format!("  {rendered}\n"),
            Some(" read ")
        ));

        // A different TOOL than the message names.
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &rendered,
            Some("grep")
        ));
        // No tool name at all — upstream's `if (!tool) return false` (`:78`).
        assert!(!is_tool_budget_blocked_message(&budget, &rendered, None));
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &rendered,
            Some("   ")
        ));
        // A message rendered for a DIFFERENT hard limit: `Number(hard) === budget.hard` (`:83`).
        let other = validate_tool_budget_config(Some(&v("{\"hard\": 9}")), "toolBudget")
            .expect("valid")
            .expect("some");
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &tool_budget_blocked_message(&other, "read", 10),
            Some("read")
        ));
        // A count that does not EXCEED the hard limit is not a block (`:83`).
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &tool_budget_blocked_message(&budget, "read", 3),
            Some("read")
        ));
        // The whole point of the anchored pattern: ordinary tool output that merely QUOTES the
        // phrase is rejected (upstream's comment at `:70-75`).
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &format!("here is what the docs say:\n{rendered}"),
            Some("read")
        ));
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &format!("{rendered} — and then it kept going"),
            Some("read")
        ));
        // The soft nudge is a different message and is never a block.
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &tool_budget_soft_nudge(&budget, 2),
            Some("read")
        ));
    }

    #[test]
    fn messages_match_upstream_text_including_pluralization() {
        let budget =
            validate_tool_budget_config(Some(&v("{\"hard\": 3, \"soft\": 1}")), "toolBudget")
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
        assert_eq!(
            decode_tool_budget_env(Some(&encoded), HardMinimum::One),
            Ok(Some(budget))
        );
        assert_eq!(encode_tool_budget_env(None), None);
        assert_eq!(decode_tool_budget_env(None, HardMinimum::One), Ok(None));
        assert_eq!(
            decode_tool_budget_env(Some("   "), HardMinimum::One),
            Ok(None)
        );
    }

    #[test]
    fn a_star_block_survives_the_env_round_trip_as_the_star_literal() {
        let budget =
            validate_tool_budget_config(Some(&v("{\"hard\": 1, \"block\": \"*\"}")), "toolBudget")
                .expect("valid")
                .expect("some");
        let encoded = encode_tool_budget_env(Some(&budget)).expect("encodes");
        assert!(encoded.contains("\"block\":\"*\""), "encoded: {encoded}");
        assert_eq!(
            decode_tool_budget_env(Some(&encoded), HardMinimum::One),
            Ok(Some(budget))
        );
    }

    #[test]
    fn a_malformed_env_payload_is_an_error_not_a_panic() {
        assert!(decode_tool_budget_env(Some("{not json"), HardMinimum::One).is_err());
        assert_eq!(
            decode_tool_budget_env(Some("{\"hard\": 0}"), HardMinimum::One),
            Err(format!("{TOOL_BUDGET_ENV}.hard must be an integer >= 1."))
        );
    }

    /// CFG-067 / pi `validateToolBudgetConfig(raw, label, { minimumHard: 0 })`
    /// (`tool-budget.ts:16-24` @v0.64.0): the SAME `hard: 0` payload is rejected by default and
    /// accepted once the parent authorised zero — and the floor is interpolated into the message.
    /// Before this port `decode_tool_budget_env` had no authorisation input at all, so a zero
    /// budget was indistinguishable from a malformed one (the assertion just above was the only
    /// behaviour).
    #[test]
    fn a_zero_hard_budget_is_rejected_unless_the_parent_authorised_it() {
        let zero = v("{\"hard\": 0}");
        assert_eq!(
            validate_tool_budget_config_with(Some(&zero), "toolBudget", HardMinimum::One),
            Err("toolBudget.hard must be an integer >= 1.".to_string())
        );
        let accepted =
            validate_tool_budget_config_with(Some(&zero), "toolBudget", HardMinimum::Zero)
                .expect("authorised zero is valid")
                .expect("some");
        assert_eq!(accepted.hard, 0);
        assert_eq!(
            decode_tool_budget_env(Some("{\"hard\": 0}"), HardMinimum::Zero),
            Ok(Some(accepted))
        );
        // A negative `hard` is still rejected under the lowered floor, with the floor in the text.
        assert_eq!(
            validate_tool_budget_config_with(
                Some(&v("{\"hard\": -1}")),
                "toolBudget",
                HardMinimum::Zero
            ),
            Err("toolBudget.hard must be an integer >= 0.".to_string())
        );
    }

    /// pi `process.env[TOOL_BUDGET_ZERO_AUTH_ENV] === "1"` (`subagent-prompt-runtime.ts:693`):
    /// exact equality — `"true"`, `" 1"` and unset all leave the floor at one.
    #[test]
    fn zero_authorisation_requires_the_exact_string_one() {
        let with = |value: Option<&'static str>| {
            HardMinimum::from_env(&move |key| {
                (key == TOOL_BUDGET_ZERO_AUTH_ENV)
                    .then(|| value.map(str::to_string))
                    .flatten()
            })
        };
        assert_eq!(with(Some("1")), HardMinimum::Zero);
        assert_eq!(with(Some("true")), HardMinimum::One);
        assert_eq!(with(Some(" 1")), HardMinimum::One);
        assert_eq!(with(None), HardMinimum::One);
    }

    /// An authorised zero budget means "no tool calls at all" for the blocked tools: the FIRST
    /// browsing call is refused (`nextToolCount 1 > hard 0`, `tool-budget.ts:57-60`), while a
    /// non-blocked tool still passes under the default block list.
    #[test]
    fn a_zero_budget_blocks_the_first_browsing_call() {
        let budget = decode_tool_budget_env(Some("{\"hard\": 0}"), HardMinimum::Zero)
            .expect("valid")
            .expect("some");
        assert!(should_block_tool_for_budget(&budget, "read", 1));
        assert!(!should_block_tool_for_budget(&budget, "bash", 1));
    }

    /// SUBA-132 — pi `isToolBudgetBlockedMessage` (`tool-budget.ts:76-94` @v0.71.0): only the
    /// runtime's own whole-message block for THIS budget and tool counts.
    #[test]
    fn only_this_runs_own_whole_blocked_message_is_a_budget_block() {
        let budget = decode_tool_budget_env(Some("{\"hard\": 2}"), HardMinimum::One)
            .expect("valid")
            .expect("some");
        let message = tool_budget_blocked_message(&budget, "read", 3);
        assert!(is_tool_budget_blocked_message(
            &budget,
            &message,
            Some("read")
        ));
        assert!(is_tool_budget_blocked_message(
            &budget,
            &format!("  {message}\n"),
            Some(" read ")
        ));
        // Another tool's name, a missing name, another run's hard limit, a count within the limit,
        // or the phrase embedded in ordinary output are all rejected.
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &message,
            Some("grep")
        ));
        assert!(!is_tool_budget_blocked_message(&budget, &message, None));
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &message,
            Some("  ")
        ));
        let other = decode_tool_budget_env(Some("{\"hard\": 5}"), HardMinimum::One)
            .expect("valid")
            .expect("some");
        assert!(!is_tool_budget_blocked_message(
            &other,
            &message,
            Some("read")
        ));
        let within = message.replace("after 3 tool calls", "after 2 tool calls");
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &within,
            Some("read")
        ));
        assert!(!is_tool_budget_blocked_message(
            &budget,
            &format!("grep output: {message}"),
            Some("read")
        ));
    }
}
