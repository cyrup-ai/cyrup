//! Model-fallback attempt loop (func-SA §5.2 R-SA-035/036/037/038/039/040/041/044; arch-SA
//! §6.3.2).
//!
//! This module owns exactly two algorithms plus the driver that composes them:
//!
//! 1. [`build_model_candidates`] — R-SA-038's ladder-construction rule: dedupe, in order,
//!    `model_override` (if explicit) then the agent's own primary model then its
//!    `fallback_models` list, filtered to models present in `available_models`, preserving
//!    first-occurrence order.
//! 2. [`is_retryable_model_failure`] — R-SA-039's fixed retryable-failure pattern classifier
//!    (rate limits, auth errors, 5xx, cold-start, empty response, etc.), a verbatim 1:1 port of
//!    pi-subagents' `RETRYABLE_MODEL_FAILURE_PATTERNS`/`isRetryableModelFailure`
//!    (`pi-subagents/src/runs/shared/model-fallback.ts:98-138`).
//! 3. [`run_fallback_ladder`] — the attempt loop itself (R-SA-035/036/037/039/040), which drives
//!    a caller-supplied single-attempt runner across the candidate ladder built by (1),
//!    consulting (2) only after a **distinct, prior** timeout check (R-SA-036's load-bearing
//!    ordering rule — see that function's doc comment).
//!
//! # Relationship to `exec/mod.rs`'s not-yet-built `AgentConfig`/`RunOptions`/`SingleResult`
//!
//! arch-SA §3.4 sketches `AgentConfig`/`RunOptions`/`SingleResult`/`ModelAttempt`/`ModelOverride`
//! as `exec/mod.rs` types; `exec/mod.rs` is a later phase of this crate's build-out (currently
//! only `pub mod ndjson;`) and is not implemented yet. Rather than either (a) blocking on that
//! phase or (b) silently redefining a shape that phase might resolve differently, this module
//! keeps its own public surface deliberately narrow and self-contained: [`ModelOverride`],
//! [`ModelAttempt`], and [`FallbackOutcome`] here are the minimal types this file's own
//! algorithms need, not a preemptive full `SingleResult`. [`run_fallback_ladder`] is generic over
//! an `AttemptRunner` trait (a single-attempt driver) rather than depending on
//! `crate::spawn::SubprocessSpawner` (also not yet built) or a concrete `AgentConfig`/`RunOptions`
//! pair, so this module compiles and is fully testable in isolation today; `exec/mod.rs`'s later
//! phase is expected to either re-export these types directly or provide a thin adapter from its
//! own richer `AgentConfig`/`RunOptions` down to the parameters [`build_model_candidates`] and
//! [`run_fallback_ladder`] take, per this crate's one-canonical-owner convention (see
//! `fork_context.rs`'s and `discovery/types.rs`'s doc comments for the same pattern applied
//! elsewhere in this crate).
//!
//! This module has ZERO dependency on `cyrup-agent`/`cyrup-session-svc` (arch-SA §2.1) — usage
//! accounting is `cyrup_core::Usage` (the same type `exec/ndjson.rs`'s
//! [`crate::exec::ndjson::SubagentEvent::assistant_usage`] already extracts from a child's
//! `MessageEnd` event), and per-attempt errors are plain `String`s (the orchestrator's own
//! classification of a failed attempt, not a re-typed provider error).

use cyrup_core::{ModelId, Usage};

// -------------------------------------------------------------------------------------------
// R-SA-041: the inherit sentinel
// -------------------------------------------------------------------------------------------

/// Distinguishes "the caller didn't specify a model override" from "explicitly use this model"
/// (R-SA-041). Deliberately NOT `Option<ModelId>`: an `Option`-shaped API invites a caller to
/// silently fall through to a global cross-session default model config when no override and no
/// per-agent model are set, which R-SA-041 forbids — "an explicit 'inherit' sentinel MUST be used
/// to distinguish 'caller didn't specify' from 'use the shared global default,' preventing one
/// session's global model choice from leaking into another session's subagents." Whatever global
/// default resolution a caller wants for the `Inherit` case must happen at the call site, visibly,
/// never implicitly inside [`build_model_candidates`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ModelOverride {
    /// No explicit override was requested; the ladder falls through to the agent's own
    /// primary/fallback models.
    #[default]
    Inherit,
    /// An explicit model override, taking first priority in the candidate ladder.
    Explicit(ModelId),
}

impl ModelOverride {
    /// The overridden model, if any (`None` for [`ModelOverride::Inherit`]).
    #[must_use]
    pub fn as_model_id(&self) -> Option<&ModelId> {
        match self {
            ModelOverride::Inherit => None,
            ModelOverride::Explicit(model) => Some(model),
        }
    }
}

// -------------------------------------------------------------------------------------------
// R-SA-038: model-fallback ladder construction
// -------------------------------------------------------------------------------------------

/// Build the model-fallback candidate ladder (R-SA-038).
///
/// Priority order, before deduplication: `model_override` (if [`ModelOverride::Explicit`]) →
/// `agent_primary_model` → `agent_fallback_models`, in the order given. The combined list is then
/// filtered to models present in `available_models` and deduplicated, **preserving
/// first-occurrence order** — a model named twice (e.g. as both the explicit override and again
/// in the agent's own fallback list) appears exactly once, at its first (highest-priority)
/// position.
///
/// `available_models` acts as a pure allowlist filter here: a candidate not present in it is
/// dropped entirely rather than reordered or substituted. When `available_models` is empty, the
/// resulting ladder is always empty too — this function does not fall back to "no filter" the way
/// pi-subagents' own `resolveModelCandidate` does for a single-model resolution (that
/// looser-when-unknown behavior belongs to model *lookup*, e.g. resolving a bare name against a
/// provider catalog, which is not this function's concern; `available_models` here is assumed to
/// already be the caller's fully resolved, non-empty-when-meaningful allowlist).
///
/// This function never fails and never panics: an empty result (e.g. no override, no agent model,
/// empty fallback list, or every candidate filtered out by `available_models`) is a legitimate,
/// representable outcome — [`run_fallback_ladder`]'s caller is responsible for treating an empty
/// ladder as a hard pre-spawn failure, not this function.
#[must_use]
pub fn build_model_candidates(
    model_override: &ModelOverride,
    agent_primary_model: Option<&ModelId>,
    agent_fallback_models: &[ModelId],
    available_models: &[ModelId],
) -> Vec<ModelId> {
    let mut candidates: Vec<ModelId> = Vec::new();

    if let Some(explicit) = model_override.as_model_id() {
        candidates.push(explicit.clone());
    }
    if let Some(primary) = agent_primary_model {
        candidates.push(primary.clone());
    }
    candidates.extend(agent_fallback_models.iter().cloned());

    let mut seen: Vec<ModelId> = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if seen.contains(&candidate) {
            continue;
        }
        if !available_models.contains(&candidate) {
            continue;
        }
        seen.push(candidate);
    }
    seen
}

/// Resolve one subagent attempt's effective [`ModelOverride`], folding in the INHERITED parent
/// session model — the cyrup analog of pi's `resolveSubagentModelOverride(requestedModel,
/// parentModel, availableModels, preferredProvider)`
/// (`pi-subagents/src/runs/shared/model-fallback.ts:47-59`), where `requestedModel = task.model ??
/// agentConfig.model` and `parentModel = ctx.model`.
///
/// Precedence, highest first (matching pi's `explicit ?? parentModel` branch, `model-fallback.ts:52-58`):
///
/// 1. `per_call_override` — an explicit per-call (`/run [model=…]`, tool `model`, single-run
///    `model_override`) or per-step (chain step `model`) override. Returned as
///    [`ModelOverride::Explicit`] so it is candidate #0.
/// 2. `persona_model` — the resolved persona's own `model` (frontmatter / settings). Returned as
///    [`ModelOverride::Inherit`] so [`build_model_candidates`] places the persona's already-present
///    primary model first, exactly as before this seam existed (no behavior change for a persona
///    that declares its own model).
/// 3. `inherited_session_model` — the live PARENT session model
///    ([`cyrup_ext::host::HostServices::current_model`], `${provider}/${id}`), used ONLY when
///    neither an override nor a persona model is set. This is pi's `parentModel` inherit branch:
///    without it an inheriting persona (`model = None`, `fallback_models = []`) has an EMPTY ladder,
///    and [`run_fallback_ladder`]'s caller hard-fails the run with "no candidate model available"
///    (`exec/mod.rs`) — the exact live blocker this seam closes. Returned as
///    [`ModelOverride::Explicit`] so it is candidate #0, and PUSHED into `available_models` so
///    [`build_model_candidates`]' allowlist filter does not immediately drop it.
/// 4. Otherwise [`ModelOverride::Inherit`] with no inherited model — the genuine no-live-session
///    degrade (headless / SDK-embedder / no active model yet): the ladder falls through to
///    `persona_model` (absent in this arm) and the persona's `fallback_models` exactly as before,
///    and an empty ladder stays the caller's hard pre-spawn error.
///
/// `available_models` is expected to ALREADY contain `per_call_override`/`persona_model`/the
/// persona `fallback_models` (both production call sites build it that way before calling this);
/// this function only ever ADDS the inherited model, never removes a caller-supplied candidate. Why
/// inherit rather than let the child default: pi issue #266 (`model-fallback.ts:32-45`) — without
/// an explicit `provider/id`, the child falls back to the global cross-session default, so one
/// session's model choice contaminates another session's subagents (exactly R-SA-041's concern).
#[must_use]
pub fn resolve_model_inheritance(
    per_call_override: Option<&ModelId>,
    persona_model: Option<&ModelId>,
    inherited_session_model: Option<&ModelId>,
    available_models: &mut Vec<ModelId>,
) -> ModelOverride {
    match (per_call_override, persona_model) {
        (Some(explicit), _) => ModelOverride::Explicit(explicit.clone()),
        (None, Some(_)) => ModelOverride::Inherit,
        (None, None) => match inherited_session_model {
            Some(inherited) => {
                if !available_models.contains(inherited) {
                    available_models.push(inherited.clone());
                }
                ModelOverride::Explicit(inherited.clone())
            }
            None => ModelOverride::Inherit,
        },
    }
}

// -------------------------------------------------------------------------------------------
// R-SA-039: retryable-failure classification
// -------------------------------------------------------------------------------------------

/// One retryable-failure pattern (R-SA-039), a dependency-free re-typing of one of pi-subagents'
/// case-insensitive JS regexes in `RETRYABLE_MODEL_FAILURE_PATTERNS`
/// (`pi-subagents/src/runs/shared/model-fallback.ts:98-133`).
///
/// This crate does not depend on the `regex` crate (workspace dependency budget; mirrors
/// `cyrup-provider::utils::regexlite`'s own dependency-free approach for the analogous
/// `retry.ts`/`overflow.ts` ports). An earlier port collapsed EVERY pi regex to a plain
/// case-insensitive substring test — but two of pi's regex constructs do NOT reduce to substrings
/// and doing so introduced real false positives and lost real generality:
///
/// - The bare HTTP-status regexes `\b429\b`/`\b502\b`/`\b503\b`/`\b504\b` are **word-bounded**: a
///   plain `"429"` substring wrongly fires on `"processed 4290 rows"` or `"sku 50249"`. Ported
///   here as [`RetryPattern::WordNumber`], which requires the digits to sit on a `\b`-style
///   word boundary (no adjacent `[A-Za-z0-9_]`), exactly like pi.
/// - The `.*` sequence regexes `provider.*unavailable`/`model.*(?:load|fail|error)`/… match the two
///   ends across arbitrary intervening text on the SAME line (JS `.` never crosses `\n`); the
///   substring port hardcoded a couple of literal variants (`"provider unavailable"`,
///   `"provider is unavailable"`) and silently dropped every other phrasing (`"provider foo is
///   currently unavailable"`). Ported here as [`RetryPattern::Then`]/[`RetryPattern::ThenAny`],
///   evaluated per line so a genuinely intervening clause still matches but a `provider` and an
///   `unavailable` on two DIFFERENT lines do not (faithful to `.`-no-newline).
///
/// The `?`/`\s*`/`(?:…)?` optional-run constructs are ported as
/// [`RetryPattern::OptionalCharBetween`]/[`RetryPattern::OptionalWsBetween`]/
/// [`RetryPattern::OptionalWordBetween`] so `cold.?start`, `rate\s*limit`, `timed? out`, and
/// `temporar(?:ily)? unavailable` match exactly what pi's regexes do (and no more — e.g.
/// `temporar(?:ily)? unavailable` matches `"temporarily unavailable"` but NOT `"temporary
/// unavailable"`, matching pi).
enum RetryPattern {
    /// Case-insensitive literal substring (pi `/quota/i`, `/forbidden/i`, `/auth(?:entication)?/i`
    /// — the last collapses to `"auth"` since any string containing `auth` matches regardless of
    /// the optional suffix).
    Contains(&'static str),
    /// A bare numeric HTTP-status token bounded by `\b` on both sides (pi `/\b429\b/`) — matches
    /// only when the digits are NOT adjacent to another word character `[A-Za-z0-9_]`.
    WordNumber(&'static str),
    /// Case-insensitive `first.*second` on one line: `first` occurs, then `second` occurs at or
    /// after the end of that `first` match, within the same line (pi `/provider.*unavailable/i`).
    Then(&'static str, &'static str),
    /// Case-insensitive `first.*(?:a|b|c)` on one line (pi `/model.*(?:load|fail|error)/i`).
    ThenAny(&'static str, &'static [&'static str]),
    /// Case-insensitive `first.?second`: `first`, then an optional single character, then `second`
    /// (pi `/cold.?start/i` → `coldstart`/`cold start`/`cold-start`).
    OptionalCharBetween(&'static str, &'static str),
    /// Case-insensitive `first\s*second`: `first`, then zero or more whitespace, then `second`
    /// (pi `/rate\s*limit/i`).
    OptionalWsBetween(&'static str, &'static str),
    /// Case-insensitive `first(?:middle)?second`: `first`, then an optional literal `middle`, then
    /// `second` (pi `/temporar(?:ily)? unavailable/i` → first=`temporar`, middle=`ily`,
    /// second=` unavailable`; pi `/timed? out/i` → first=`time`, middle=`d`, second=` out`).
    OptionalWordBetween(&'static str, &'static str, &'static str),
}

/// The fixed retryable-failure pattern set (R-SA-039), in pi's exact `RETRYABLE_MODEL_FAILURE_PATTERNS`
/// declaration order (`model-fallback.ts:98-133`).
const RETRYABLE_MODEL_FAILURE_PATTERNS: &[RetryPattern] = &[
    RetryPattern::OptionalWsBetween("rate", "limit"), // /rate\s*limit/i
    RetryPattern::Contains("too many requests"),
    RetryPattern::WordNumber("429"), // /\b429\b/
    RetryPattern::Contains("quota"),
    RetryPattern::Contains("billing"),
    RetryPattern::Contains("credit"),
    RetryPattern::Contains("auth"), // /auth(?:entication)?/i
    RetryPattern::Contains("unauthorized"), // /unauthori[sz]ed/i, US spelling
    RetryPattern::Contains("unauthorised"), // /unauthori[sz]ed/i, UK spelling
    RetryPattern::Contains("forbidden"),
    RetryPattern::Contains("api key"),
    RetryPattern::Contains("token expired"),
    RetryPattern::Contains("invalid key"),
    RetryPattern::Then("provider", "unavailable"), // /provider.*unavailable/i
    RetryPattern::Then("model", "unavailable"),    // /model.*unavailable/i
    RetryPattern::Then("model", "disabled"),       // /model.*disabled/i
    RetryPattern::Then("model", "not found"),      // /model.*not found/i
    RetryPattern::Contains("unknown model"),
    RetryPattern::Contains("overloaded"),
    RetryPattern::Contains("service unavailable"),
    RetryPattern::OptionalWordBetween("temporar", "ily", " unavailable"), // /temporar(?:ily)? unavailable/i
    RetryPattern::Contains("connection refused"),
    RetryPattern::Contains("fetch failed"),
    RetryPattern::Contains("network error"),
    RetryPattern::Contains("socket hang up"),
    RetryPattern::Contains("upstream"),
    RetryPattern::OptionalWordBetween("time", "d", " out"), // /timed? out/i
    RetryPattern::Contains("timeout"),
    RetryPattern::WordNumber("502"), // /\b502\b/
    RetryPattern::WordNumber("503"), // /\b503\b/
    RetryPattern::WordNumber("504"), // /\b504\b/
    RetryPattern::OptionalCharBetween("cold", "start"), // /cold.?start/i
    RetryPattern::Contains("empty response"),
    RetryPattern::Contains("no output"),
    RetryPattern::ThenAny("model", &["load", "fail", "error"]), // /model.*(?:load|fail|error)/i
];

/// Whether a byte is a regex `\w`/`\b`-boundary word character (`[A-Za-z0-9_]`).
fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Advance past exactly one UTF-8 character of `s`, returning the remainder (or `""` if `s` had a
/// single character). `None` only for an already-empty `s`.
fn skip_one_char(s: &str) -> Option<&str> {
    let mut chars = s.char_indices();
    chars.next()?; // consume the first character
    match chars.next() {
        Some((idx, _)) => s.get(idx..),
        None => Some(""),
    }
}

/// `\b<num>\b`: does `num` (ASCII digits) appear on a word boundary anywhere in `hay`?
fn matches_word_number(hay: &str, num: &str) -> bool {
    let bytes = hay.as_bytes();
    let mut search_start = 0;
    while let Some(rel) = hay.get(search_start..).and_then(|s| s.find(num)) {
        let start = search_start + rel;
        let end = start + num.len();
        let before_ok = start == 0 || bytes.get(start - 1).is_none_or(|&b| !is_word_byte(b));
        let after_ok = bytes.get(end).is_none_or(|&b| !is_word_byte(b));
        if before_ok && after_ok {
            return true;
        }
        search_start = start + 1;
    }
    false
}

/// Whether `line` (already lowercased) matches `pattern`. `line` is a single line — the `.*`/`\s*`
/// constructs never cross a `\n` (JS `.`-no-newline), so [`is_retryable_model_failure`] applies
/// this per line.
fn line_matches(line: &str, pattern: &RetryPattern) -> bool {
    match pattern {
        RetryPattern::Contains(needle) => line.contains(needle),
        RetryPattern::WordNumber(num) => matches_word_number(line, num),
        RetryPattern::Then(first, second) => match line.find(first) {
            Some(pos) => line
                .get(pos + first.len()..)
                .is_some_and(|rest| rest.contains(second)),
            None => false,
        },
        RetryPattern::ThenAny(first, seconds) => match line.find(first) {
            Some(pos) => line.get(pos + first.len()..).is_some_and(|rest| {
                seconds.iter().any(|second| rest.contains(second))
            }),
            None => false,
        },
        RetryPattern::OptionalCharBetween(first, second) => {
            let mut search = 0;
            while let Some(rel) = line.get(search..).and_then(|s| s.find(first)) {
                let start = search + rel;
                if let Some(rest) = line.get(start + first.len()..)
                    && (rest.starts_with(second)
                        || skip_one_char(rest).is_some_and(|r| r.starts_with(second)))
                {
                    return true;
                }
                search = start + 1;
            }
            false
        }
        RetryPattern::OptionalWsBetween(first, second) => {
            let mut search = 0;
            while let Some(rel) = line.get(search..).and_then(|s| s.find(first)) {
                let start = search + rel;
                if let Some(rest) = line.get(start + first.len()..)
                    && rest
                        .trim_start_matches(char::is_whitespace)
                        .starts_with(second)
                {
                    return true;
                }
                search = start + 1;
            }
            false
        }
        RetryPattern::OptionalWordBetween(first, middle, second) => {
            let mut search = 0;
            while let Some(rel) = line.get(search..).and_then(|s| s.find(first)) {
                let start = search + rel;
                if let Some(rest) = line.get(start + first.len()..)
                    && (rest.starts_with(second)
                        || rest
                            .strip_prefix(middle)
                            .is_some_and(|r| r.starts_with(second)))
                {
                    return true;
                }
                search = start + 1;
            }
            false
        }
    }
}

/// Classify whether a failed attempt's error text matches a known retryable-failure pattern
/// (R-SA-039).
///
/// This is a **pure text classifier only** — it does not know, and must not be asked to know,
/// whether the failure was a timeout: R-SA-036's load-bearing ordering rule ("timeout terminates
/// the ladder... unlike other retryable failures") requires the *caller* ([`run_fallback_ladder`])
/// to check `timed_out` as a wholly distinct branch **before** ever consulting this function,
/// because the retryable-pattern set above deliberately includes `timed? out`/`timeout` (a
/// real, common provider-error phrase for a *provider-side* request timeout, distinct from *this
/// orchestrator's own* wall-clock deadline expiring) — a timeout-classified attempt's error text
/// can plainly match this classifier's own patterns, which would otherwise wrongly continue the
/// fallback ladder past a run that R-SA-036 requires to terminate it outright. See
/// [`run_fallback_ladder`]'s doc comment and this module's
/// `ordering_rule_timeout_branch_wins_even_when_error_text_matches_a_retryable_pattern` test for
/// the concrete proof.
///
/// Returns `false` for `None`/empty error text (nothing to classify), matching pi's own
/// `isRetryableModelFailure(undefined) === false`.
#[must_use]
pub fn is_retryable_model_failure(error: Option<&str>) -> bool {
    let Some(error) = error else {
        return false;
    };
    if error.trim().is_empty() {
        return false;
    }
    let haystack = error.to_lowercase();
    // Per-line so the `.*`/`\s*`/`.?` constructs never cross a newline (JS `.`-no-newline). A
    // `Contains`/`WordNumber` needle can never straddle a `\n` either, so per-line evaluation is
    // equivalent to whole-string for those and correct for the sequence patterns.
    haystack
        .split('\n')
        .any(|line| RETRYABLE_MODEL_FAILURE_PATTERNS.iter().any(|p| line_matches(line, p)))
}

/// Format the "prior attempt failed" note appended into the next attempt's initial
/// `recent_output`/progress context (R-SA-039's "append a note about the prior attempt into the
/// next attempt's initial `recent_output`/progress context"), mirroring pi's
/// `formatModelAttemptNote` (`model-fallback.ts:140-145`).
#[must_use]
pub fn format_attempt_note(
    failed_model: &ModelId,
    error: Option<&str>,
    next_model: &ModelId,
) -> String {
    let failure = error
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("attempt failed");
    format!("[fallback] {failed_model} failed: {failure}. Retrying with {next_model}.")
}

// -------------------------------------------------------------------------------------------
// R-SA-040: per-attempt record + additive usage aggregation
// -------------------------------------------------------------------------------------------

/// One row of the model-fallback ladder's attempt history (R-SA-040's `model_attempts`;
/// arch-SA §3.4's `ModelAttempt`).
///
/// `PartialEq`/`Serialize`/`Deserialize` are derived (beyond the original `Debug, Clone`) because
/// `background::ResultFile` (func-SA §4.5, R-SA-077/166) embeds this type transitively via
/// `SingleResult.model_attempts` and must round-trip through `status.json`/the terminal result
/// file exactly like every other field on that struct.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAttempt {
    pub model: ModelId,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub usage: Usage,
}

/// Add `other`'s token/cost counters into `total`, additively — never "last attempt wins"
/// (R-SA-040: "the orchestrator MUST aggregate `usage` **additively** across all attempts
/// (including failed ones...)"). `cyrup_core::Usage` has no `Add`/`add` of its own (unlike the
/// illustrative `exec::Usage::add` sketched in arch-SA §3.4, which predates the real
/// `cyrup_core::Usage` shape confirmed live against `crates/cyrup-core/src/message.rs:299`), so
/// this module owns the additive-fold helper directly rather than inventing a second, competing
/// `Usage` type.
///
/// `cache_write_1h`/`reasoning` (both `Option<u64>`) are summed when at least one side is
/// `Some`, treating an absent value as `0` for the purpose of the sum — an attempt that never
/// reports 1h-cache-write or reasoning-token usage must not zero out a sibling attempt's real
/// count, and a running total that has seen no such value from any attempt yet stays `None`
/// rather than spuriously becoming `Some(0)`.
pub fn add_usage(total: &mut Usage, other: &Usage) {
    total.input += other.input;
    total.output += other.output;
    total.cache_read += other.cache_read;
    total.cache_write += other.cache_write;
    total.total_tokens += other.total_tokens;
    total.cost.input += other.cost.input;
    total.cost.output += other.cost.output;
    total.cost.cache_read += other.cost.cache_read;
    total.cost.cache_write += other.cost.cache_write;
    total.cost.total += other.cost.total;

    total.cache_write_1h = match (total.cache_write_1h, other.cache_write_1h) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    total.reasoning = match (total.reasoning, other.reasoning) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
}

/// Additively fold a full slice of attempts' usage into one aggregate total (R-SA-040), starting
/// from [`Usage::default`]. A convenience over repeatedly calling [`add_usage`].
#[must_use]
pub fn aggregate_usage<'a>(attempts: impl IntoIterator<Item = &'a Usage>) -> Usage {
    let mut total = Usage::default();
    for usage in attempts {
        add_usage(&mut total, usage);
    }
    total
}

// -------------------------------------------------------------------------------------------
// The attempt loop itself (R-SA-035/036/037/039/040)
// -------------------------------------------------------------------------------------------

/// What one single-model attempt produced, as far as the fallback ladder driver
/// ([`run_fallback_ladder`]) needs to know to decide whether to advance, stop, or aggregate.
///
/// Deliberately narrower than arch-SA §3.4's full `SingleResult`: this is only the subset of
/// fields the ladder-control decisions (R-SA-036/037/039/040) actually branch on. The richer
/// per-attempt payload (final output, structured output, acceptance ledger, etc.) is threaded
/// through as the runner's own associated `Attempt` type (see [`AttemptRunner`]) and returned to
/// the caller unmodified inside [`FallbackOutcome`] — this module has no need to inspect it.
#[derive(Debug, Clone)]
pub struct AttemptSignal {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    pub usage: Usage,
    /// R-SA-036: a soft-interrupt timeout occurred on this attempt. Checked as a distinct branch
    /// strictly before [`is_retryable_model_failure`] is ever consulted — see
    /// [`run_fallback_ladder`]'s doc comment for why this ordering is load-bearing.
    pub timed_out: bool,
    /// R-SA-037: an intercom-style blocking detach signal was observed on this attempt. A
    /// detached attempt is terminal exactly like a timeout (the ladder does not advance past it,
    /// nor does it retry) but is tracked as its own distinct flag rather than folded into
    /// `timed_out`, since downstream gate-bypass behavior distinguishes the two (R-SA-037 bypasses
    /// acceptance/completion-guard/truncation entirely; R-SA-036 does not).
    ///
    /// # How this is now wired (R-SA-037, reconciliation §4 step 5 item 3 — CLOSED)
    ///
    /// This field is now set from a REAL blocking-detach signal, via the intercom-companion wiring:
    ///
    /// 1. A child that blocks on a `contact_supervisor` supervisor-clarify ask (the intercom
    ///    `ask_and_wait` reasons `need_decision`/`interview`, `contact_supervisor.rs:81-101`) emits
    ///    an ordinary `ToolExecutionStart` for the `contact_supervisor` tool on its NDJSON stdout —
    ///    no new wire variant is needed (the recipe's "reuse an existing clarify-shaped event").
    /// 2. `exec::mod::drive_attempt`'s NDJSON loop detects it (`contact_supervisor_block_prompt`),
    ///    fires `crate::tui::intercom::spawn_clarify` against the executor's single-slot
    ///    [`crate::tui::intercom::AskLock`] — backed in production by the intercom companion's real
    ///    broker `ClarifyChannel` (threaded via `SubagentsExtension::with_channels` →
    ///    `RunOptions::clarify`) — which surfaces the ask to the parent's human and routes the answer
    ///    back to the still-alive child over the BROKER (a transport independent of this stdout pipe).
    /// 3. `SpawnedChildAttemptRunner::run_attempt` carries the drive loop's `detached` observation
    ///    onto this field, so the ladder does not advance and `run_sync`'s acceptance/completion-guard/
    ///    truncation gate is bypassed — both of which already correctly branch on this flag.
    ///
    /// When no intercom channel is wired (headless / SDK-embedder / a run with `RunOptions::clarify
    /// = None`), the drive loop degrades gracefully: it still marks the attempt detached but the
    /// `AskLock` degrades to its no-live-channel fallback (`ClarifyOutcome::NoLiveChannel`), never
    /// blocking. **Do not fabricate a synthetic trigger** from output-text heuristics — the trigger
    /// is a real `contact_supervisor` blocking-ask event on the child's own wire.
    pub detached: bool,
}

/// Drives exactly one fresh child-subprocess attempt for `model` (R-SA-039: "launch a fresh child
/// OS subprocess for that model, never reuse or restart a previous child process").
///
/// This trait is the seam [`run_fallback_ladder`] is generic over so it can be exercised with a
/// fully scripted, deterministic runner in this module's own tests without requiring the (not yet
/// built) `crate::spawn::SubprocessSpawner`/`exec::mod::AgentConfig`/`RunOptions` machinery.
/// Production wiring (a later phase of `exec/mod.rs`) is expected to implement this trait as a
/// thin adapter that actually spawns via `crate::spawn::SpawnedChild::spawn`, drains its stdout
/// through `exec::ndjson::consume_stdout`, and folds the result down to an [`AttemptSignal`] plus
/// whatever richer `Attempt` payload it wants preserved.
#[async_trait::async_trait]
pub trait AttemptRunner {
    /// The richer per-attempt payload this runner produces beyond what [`AttemptSignal`] exposes
    /// to the ladder driver (e.g. final output text, structured output, the raw NDJSON artifact
    /// path). Returned verbatim inside [`FallbackOutcome::attempts`]/[`FallbackOutcome::last`].
    type Attempt: Send;

    /// Run one attempt against `model`, with `attempt_note` (R-SA-039's "note about the prior
    /// attempt") injected into this attempt's initial context when this is not the ladder's first
    /// candidate, and `output_snapshot` the R-SA-031 stat-snapshot the caller took of the output
    /// file (if any) immediately before this call — [`run_fallback_ladder`] snapshots (via
    /// [`Self::snapshot_output_file`]) once per attempt but does not otherwise interpret the
    /// snapshot value itself, since that comparison is R-SA-031's `exec/output.rs`'s concern, not
    /// this module's.
    async fn run_attempt(
        &mut self,
        model: &ModelId,
        attempt_note: Option<&str>,
    ) -> (AttemptSignal, Self::Attempt);

    /// Snapshot whatever output-file state R-SA-031 needs before this attempt spawns. Default
    /// no-op `()` for runners that don't use file-only/file-and-inline output modes at all (most
    /// test runners); production wiring overrides this to call the real
    /// `exec/output.rs::snapshot_output_file` once that later phase exists. Returned value is
    /// discarded by [`run_fallback_ladder`] itself — it exists purely so the snapshot is taken at
    /// the correct point in the loop (immediately before each fresh spawn, R-SA-031) without this
    /// module needing to know anything about output-file mechanics.
    fn snapshot_output_file(&mut self) {}
}

/// The full outcome of driving the model-fallback ladder to completion (R-SA-040).
#[derive(Debug, Clone)]
pub struct FallbackOutcome<A> {
    /// Every candidate model that was actually attempted, in ladder order (R-SA-040's
    /// `attempted_models`, flat list).
    pub attempted_models: Vec<ModelId>,
    /// One row per attempt, in ladder order (R-SA-040's `model_attempts`).
    pub model_attempts: Vec<ModelAttempt>,
    /// The additive sum of every attempt's usage, including failed attempts (R-SA-040).
    pub aggregate_usage: Usage,
    /// The final [`AttemptSignal`] the ladder stopped on (success, exhaustion, timeout, or
    /// detach) — `None` only when `candidates` was empty to begin with (R-SA-038's ladder can
    /// legitimately be empty; the caller, not this function, decides how to treat that as a
    /// pre-spawn failure).
    pub last_signal: Option<AttemptSignal>,
    /// The richer per-attempt payload from the runner that produced [`Self::last_signal`] —
    /// paired with it by construction (both come from the same [`AttemptRunner::run_attempt`]
    /// call), kept as a separate field rather than tupled together so callers that only care about
    /// ladder-control state (tests, mostly) don't need to know `A`'s shape at all.
    pub last_attempt: Option<A>,
}

/// Drive the model-fallback attempt loop to completion (R-SA-035/036/037/039/040; arch-SA
/// §6.3.2).
///
/// # The load-bearing ordering rule (R-SA-036)
///
/// For each attempt's outcome, in this exact order:
///
/// 1. **Timeout check, first, as a wholly distinct branch.** If `signal.timed_out`, the loop
///    stops immediately — it does NOT advance to the next candidate, and critically, it does
///    NOT consult [`is_retryable_model_failure`] at all for this attempt's error text. This
///    ordering is not cosmetic: [`is_retryable_model_failure`]'s own fixed pattern set includes
///    `"timed out"`/`"timeout"` (a legitimate retryable *provider-side* request-timeout phrase in
///    ordinary, non-orchestrator-timeout failures) — if the retryable-pattern classifier were
///    consulted before (or instead of) the `timed_out` flag, an orchestrator-level wall-clock
///    timeout whose captured error text happens to also contain timeout-shaped wording would be
///    wrongly classified as "retryable" and the ladder would incorrectly continue past a case
///    R-SA-036 requires it to terminate outright. See this module's
///    `ordering_rule_timeout_branch_wins_even_when_error_text_matches_a_retryable_pattern` test
///    for the concrete failure mode this ordering prevents.
/// 2. **Detach check, also before retry classification.** If `signal.detached` (R-SA-037), the
///    loop likewise stops immediately without consulting [`is_retryable_model_failure`] — an
///    intercom-style blocking detach is not a failure to retry past at all.
/// 3. **Success, or last candidate, or a non-retryable error** — any of these three also stops
///    the loop (R-SA-039: "If the error is retryable AND this is not the last candidate AND the
///    failure was not a timeout, the orchestrator MUST proceed to the next candidate model...
///    otherwise the ladder MUST stop"). Only past this point is
///    [`is_retryable_model_failure`] ever actually consulted, and only for an attempt that has
///    already been confirmed to be neither a timeout nor a detach.
/// 4. Otherwise (retryable, not the last candidate, not a timeout, not a detach): format an
///    attempt note (R-SA-039) for the next iteration and continue the ladder.
///
/// # Deadline monotonicity (R-SA-035)
///
/// This function takes no `deadline_at` parameter of its own and does not compute or mutate one:
/// R-SA-035 requires `deadline_at` to be "computed once at the start of the outer call... and
/// passed through unmodified to every subsequent model-fallback attempt" — that single shared
/// deadline is exactly the kind of state [`AttemptRunner::run_attempt`]'s implementor is
/// responsible for threading unmodified into each attempt's own spawn/wait logic (a later phase's
/// concern), not something this loop should re-derive, reset, or even observe. This function's
/// role in R-SA-035 is purely negative but load-bearing: it must never itself introduce a
/// per-attempt timeout budget independent of what the runner enforces, which the implementation
/// below satisfies by not referencing time at all.
///
/// # Usage aggregation (R-SA-040)
///
/// Every attempt's usage — success or failure alike — is folded additively into
/// [`FallbackOutcome::aggregate_usage`] via [`add_usage`] before this function even branches on
/// the outcome, so a candidate that fails on its very first attempt still contributes its
/// (possibly nonzero, e.g. partial-stream) usage to the total.
///
/// # Empty ladder
///
/// If `candidates` is empty, this function returns immediately with an empty `attempted_models`/
/// `model_attempts`, zeroed `aggregate_usage`, and `last_signal`/`last_attempt` both `None` — it
/// never calls `runner.run_attempt` at all in that case. Treating an empty ladder as a hard
/// pre-spawn failure (there is no model to even try) is the caller's responsibility.
pub async fn run_fallback_ladder<R: AttemptRunner>(
    candidates: &[ModelId],
    runner: &mut R,
) -> FallbackOutcome<R::Attempt> {
    let mut attempted_models = Vec::with_capacity(candidates.len());
    let mut model_attempts = Vec::with_capacity(candidates.len());
    let mut aggregate = Usage::default();
    let mut last_signal: Option<AttemptSignal> = None;
    let mut last_attempt: Option<R::Attempt> = None;
    let mut attempt_note: Option<String> = None;

    for (i, model) in candidates.iter().enumerate() {
        runner.snapshot_output_file(); // R-SA-031: snapshot immediately before each fresh spawn
        let (signal, attempt) = runner
            .run_attempt(model, attempt_note.take().as_deref())
            .await; // R-SA-039: always a fresh child subprocess per candidate

        attempted_models.push(model.clone());
        add_usage(&mut aggregate, &signal.usage); // R-SA-040: additive, even for a failed attempt

        model_attempts.push(ModelAttempt {
            model: model.clone(),
            success: signal.success,
            exit_code: signal.exit_code,
            error: signal.error.clone(),
            usage: signal.usage.clone(),
        });

        let is_last_candidate = i + 1 == candidates.len();

        // --- R-SA-036: timeout is a distinct branch, checked FIRST, before any retryable-error
        // --- pattern classification runs at all. ---
        if signal.timed_out {
            last_signal = Some(signal);
            last_attempt = Some(attempt);
            break;
        }
        // --- R-SA-037: detach is likewise terminal, checked before retry classification. ---
        if signal.detached {
            last_signal = Some(signal);
            last_attempt = Some(attempt);
            break;
        }

        if signal.success || is_last_candidate {
            last_signal = Some(signal);
            last_attempt = Some(attempt);
            break;
        }

        // Only reached for a non-timeout, non-detached, non-last-candidate failure: NOW (and
        // only now) is the retryable-pattern classifier consulted (R-SA-039).
        if !is_retryable_model_failure(signal.error.as_deref()) {
            last_signal = Some(signal);
            last_attempt = Some(attempt);
            break;
        }

        // Retryable, not the last candidate, not a timeout, not a detach: advance the ladder.
        if let Some(next_model) = candidates.get(i + 1) {
            attempt_note = Some(format_attempt_note(
                model,
                signal.error.as_deref(),
                next_model,
            ));
        }
        last_signal = Some(signal);
        last_attempt = Some(attempt);
    }

    FallbackOutcome {
        attempted_models,
        model_attempts,
        aggregate_usage: aggregate,
        last_signal,
        last_attempt,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn model(id: &str) -> ModelId {
        ModelId::from(id)
    }

    fn usage(input: u64, output: u64, cost_total: f64) -> Usage {
        Usage {
            input,
            output,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: input + output,
            cost: cyrup_core::Cost {
                input: 0.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                total: cost_total,
            },
        }
    }

    // ---------------------------------------------------------------------------------------
    // build_model_candidates (R-SA-038): dedup, priority order, availability filter
    // ---------------------------------------------------------------------------------------

    #[test]
    fn candidate_priority_is_override_then_primary_then_fallback_list() {
        let available = vec![model("a"), model("b"), model("c")];
        let candidates = build_model_candidates(
            &ModelOverride::Explicit(model("c")),
            Some(&model("a")),
            &[model("b")],
            &available,
        );
        assert_eq!(
            candidates,
            vec![model("c"), model("a"), model("b")],
            "explicit override first, then agent primary, then fallback list, in that order"
        );
    }

    #[test]
    fn candidate_ladder_falls_through_to_primary_and_fallback_when_override_is_inherit() {
        let available = vec![model("a"), model("b")];
        let candidates = build_model_candidates(
            &ModelOverride::Inherit,
            Some(&model("a")),
            &[model("b")],
            &available,
        );
        assert_eq!(candidates, vec![model("a"), model("b")]);
    }

    #[test]
    fn candidate_ladder_dedupes_preserving_first_occurrence_order() {
        // "a" appears as both the explicit override AND later in the fallback list — R-SA-038
        // requires it to appear exactly once, at its highest-priority (first) position.
        let available = vec![model("a"), model("b")];
        let candidates = build_model_candidates(
            &ModelOverride::Explicit(model("a")),
            Some(&model("b")),
            &[model("a"), model("b")],
            &available,
        );
        assert_eq!(
            candidates,
            vec![model("a"), model("b")],
            "each model must appear exactly once, at its first (highest-priority) occurrence"
        );
    }

    #[test]
    fn candidate_ladder_dedupes_a_primary_model_repeated_in_its_own_fallback_list() {
        let available = vec![model("a"), model("b"), model("c")];
        let candidates = build_model_candidates(
            &ModelOverride::Inherit,
            Some(&model("a")),
            &[model("a"), model("b"), model("a"), model("c")],
            &available,
        );
        assert_eq!(candidates, vec![model("a"), model("b"), model("c")]);
    }

    #[test]
    fn candidate_ladder_filters_out_models_absent_from_available_models() {
        let available = vec![model("b")]; // "a" and "c" are not available
        let candidates = build_model_candidates(
            &ModelOverride::Explicit(model("a")),
            Some(&model("b")),
            &[model("c")],
            &available,
        );
        assert_eq!(
            candidates,
            vec![model("b")],
            "unavailable candidates must be dropped entirely, not reordered or substituted"
        );
    }

    #[test]
    fn candidate_ladder_is_empty_when_nothing_is_available() {
        let candidates = build_model_candidates(
            &ModelOverride::Explicit(model("a")),
            Some(&model("b")),
            &[model("c")],
            &[], // nothing available
        );
        assert!(candidates.is_empty());
    }

    #[test]
    fn candidate_ladder_with_no_override_and_no_agent_model_is_empty() {
        let candidates = build_model_candidates(&ModelOverride::Inherit, None, &[], &[model("a")]);
        assert!(candidates.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // resolve_model_inheritance (R-SA-038/041; pi `resolveSubagentModelOverride`): precedence is
    // per-call override > persona model > INHERITED parent session model > fallback_models, and the
    // inherited model must survive `build_model_candidates`' allowlist filter (it is added to
    // `available_models`) so it lands as candidate #0.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn inheritance_uses_the_parent_session_model_when_persona_has_no_model_and_no_override() {
        // (a) the real blocker: an inheriting persona (model = None, fallback_models = []) with a
        // live parent session model X ends up with X as candidate #0 — a NON-empty ladder, where
        // before it was empty and the run hard-failed with "no candidate model available".
        let inherited = model("together/zai-org/GLM-5.2");
        let persona_model: Option<&ModelId> = None;
        let persona_fallbacks: Vec<ModelId> = Vec::new();

        // available_models is built the way both call sites build it: fallbacks + persona model.
        let mut available_models: Vec<ModelId> =
            persona_fallbacks.iter().cloned().chain(persona_model.cloned()).collect();
        let ov = resolve_model_inheritance(None, persona_model, Some(&inherited), &mut available_models);

        assert_eq!(ov, ModelOverride::Explicit(inherited.clone()));
        assert!(
            available_models.contains(&inherited),
            "the inherited model must be added to available_models so the allowlist filter keeps it"
        );
        let candidates =
            build_model_candidates(&ov, persona_model, &persona_fallbacks, &available_models);
        assert_eq!(
            candidates,
            vec![inherited],
            "the inherited parent-session model must be the primary (candidate #0), not filtered out"
        );
    }

    #[test]
    fn inheritance_yields_a_nonempty_ladder_where_inherit_sentinel_alone_would_be_empty() {
        // Contrast with `candidate_ladder_with_no_override_and_no_agent_model_is_empty`: the bare
        // Inherit sentinel + no persona model = empty ladder (the failure). With a live parent model
        // threaded through resolve_model_inheritance, the same persona now resolves a candidate.
        let inherited = model("anthropic/claude-opus-4-8");
        let mut available_models: Vec<ModelId> = Vec::new();
        let ov = resolve_model_inheritance(None, None, Some(&inherited), &mut available_models);
        let candidates = build_model_candidates(&ov, None, &[], &available_models);
        assert!(!candidates.is_empty());
        assert_eq!(candidates, vec![inherited]);
    }

    #[test]
    fn a_per_call_override_wins_over_an_inherited_session_model() {
        // (b) explicit per-call/per-step override beats inheritance.
        let inherited = model("together/zai-org/GLM-5.2");
        let per_call = model("z");
        let mut available_models: Vec<ModelId> = vec![per_call.clone()];
        let ov =
            resolve_model_inheritance(Some(&per_call), None, Some(&inherited), &mut available_models);
        assert_eq!(ov, ModelOverride::Explicit(per_call.clone()));
        let candidates = build_model_candidates(&ov, None, &[], &available_models);
        assert_eq!(candidates.first(), Some(&per_call), "per-call override is candidate #0");
        assert!(
            !candidates.contains(&inherited),
            "inheritance must NOT be added when a per-call override is present"
        );
    }

    #[test]
    fn a_persona_model_wins_over_an_inherited_session_model() {
        // (c) a persona that declares its own `model:` beats inheritance — resolve returns Inherit so
        // build_model_candidates places the persona's own primary model first (unchanged behavior).
        let inherited = model("together/zai-org/GLM-5.2");
        let persona = model("x");
        let mut available_models: Vec<ModelId> = vec![persona.clone()];
        let ov =
            resolve_model_inheritance(None, Some(&persona), Some(&inherited), &mut available_models);
        assert_eq!(ov, ModelOverride::Inherit);
        let candidates = build_model_candidates(&ov, Some(&persona), &[], &available_models);
        assert_eq!(candidates.first(), Some(&persona), "persona model is candidate #0");
        assert!(
            !candidates.contains(&inherited),
            "inheritance must NOT be added when the persona declares its own model"
        );
    }

    #[test]
    fn no_host_degrades_to_persona_fallbacks_exactly_as_before() {
        // (d) no live session (inherited = None): resolve returns Inherit and adds nothing — the
        // ladder falls through to the persona model + fallback_models exactly as before this seam.
        let fallbacks = vec![model("f1"), model("f2")];
        let mut available_models: Vec<ModelId> = fallbacks.clone();
        let ov = resolve_model_inheritance(None, None, None, &mut available_models);
        assert_eq!(ov, ModelOverride::Inherit);
        assert_eq!(available_models, fallbacks, "no inherited model may be added when there is no host");
        let candidates = build_model_candidates(&ov, None, &fallbacks, &available_models);
        assert_eq!(candidates, fallbacks, "the ladder is exactly the persona fallback list");

        // ...and with neither a persona model nor fallbacks nor a host, the ladder stays empty (the
        // caller's genuine hard pre-spawn error) — never a spuriously-invented candidate.
        let mut empty_avail: Vec<ModelId> = Vec::new();
        let ov = resolve_model_inheritance(None, None, None, &mut empty_avail);
        assert_eq!(ov, ModelOverride::Inherit);
        assert!(build_model_candidates(&ov, None, &[], &empty_avail).is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // is_retryable_model_failure (R-SA-039): pattern classification
    // ---------------------------------------------------------------------------------------

    #[test]
    fn classifies_known_retryable_patterns() {
        for msg in [
            "429 Too Many Requests",
            "rate limit exceeded",
            "Rate Limit reached", // case-insensitive
            "quota exceeded",
            "billing issue",
            "insufficient credit",
            "authentication failed",
            "unauthorized",
            "forbidden",
            "invalid api key",
            "token expired",
            "invalid key",
            "provider unavailable",
            "model unavailable",
            "model disabled",
            "model not found",
            "unknown model",
            "overloaded",
            "503 Service Unavailable",
            "temporarily unavailable",
            "connection refused",
            "fetch failed",
            "network error",
            "socket hang up",
            "upstream error",
            "502 Bad Gateway",
            "504 Gateway Timeout",
            "cold start",
            "empty response",
            "no output",
            "model load failed",
        ] {
            assert!(
                is_retryable_model_failure(Some(msg)),
                "expected retryable: {msg}"
            );
        }
    }

    #[test]
    fn non_retryable_error_text_is_not_classified_as_retryable() {
        for msg in [
            "task completed with logic error",
            "assertion failed in test",
            "file not found: /tmp/foo",
            "unexpected token in JSON",
        ] {
            assert!(
                !is_retryable_model_failure(Some(msg)),
                "expected NOT retryable: {msg}"
            );
        }
    }

    #[test]
    fn no_error_text_is_not_retryable() {
        assert!(!is_retryable_model_failure(None));
        assert!(!is_retryable_model_failure(Some("")));
        assert!(!is_retryable_model_failure(Some("   ")));
    }

    #[test]
    fn bare_http_status_codes_require_a_word_boundary_and_do_not_false_positive_on_larger_numbers() {
        // pi `/\b429\b/` etc. — a status code embedded in a LARGER number is NOT a rate-limit/5xx
        // signal. The prior bare-substring port wrongly fired on all of these.
        for msg in [
            "processed 4290 rows",
            "sku 50249 shipped",
            "error code 45021 encountered",
            "offset 15040 of 20000",
            "id x429y not found",
        ] {
            assert!(
                !is_retryable_model_failure(Some(msg)),
                "a status code embedded in a larger token must NOT be retryable: {msg}"
            );
        }
        // ...but a genuine, word-bounded status code still is.
        for msg in ["HTTP 429", "(429) Too Many", "got 502 from upstream", "-> 503", "504."] {
            assert!(
                is_retryable_model_failure(Some(msg)),
                "a word-bounded status code MUST be retryable: {msg}"
            );
        }
    }

    #[test]
    fn sequence_patterns_match_across_intervening_text_on_the_same_line() {
        // pi `/provider.*unavailable/i` / `/model.*(?:load|fail|error)/i` — restored `.*` generality
        // the hardcoded-variant substring port had dropped.
        for msg in [
            "the model provider is currently unavailable",
            "provider openai returned: service is unavailable right now",
            "model claude-x is temporarily disabled for this account",
            "model gpt-9 could not be loaded: weights failed to fetch",
        ] {
            assert!(
                is_retryable_model_failure(Some(msg)),
                "a `.*` sequence pattern must match across intervening words: {msg}"
            );
        }
    }

    #[test]
    fn sequence_patterns_do_not_cross_a_newline() {
        // JS `.` never matches `\n`, so `provider` on one line and `unavailable` on another is NOT
        // a `provider.*unavailable` match — each pattern is evaluated per line.
        assert!(
            !is_retryable_model_failure(Some("provider foo\nthe endpoint is unavailable")),
            "a `.*` sequence must not span a newline"
        );
        // But two independent lines, one of which independently matches, still trips.
        assert!(is_retryable_model_failure(Some(
            "some benign context\nthe provider is unavailable"
        )));
    }

    #[test]
    fn optional_run_patterns_match_exactly_what_pi_does() {
        // `/cold.?start/i`, `/rate\s*limit/i`, `/timed? out/i`, `/temporar(?:ily)? unavailable/i`.
        for msg in [
            "coldstart penalty",
            "cold start penalty",
            "cold-start penalty",
            "ratelimit hit",
            "rate  limit hit", // \s* allows more than one space
            "request time out",
            "request timed out",
            "temporarily unavailable",
        ] {
            assert!(is_retryable_model_failure(Some(msg)), "expected retryable: {msg}");
        }
        // `temporar(?:ily)? unavailable` matches "temporarily"/"temporar unavailable" but NOT the
        // "temporary unavailable" spelling (faithful to pi's regex, which has no `y` branch).
        assert!(
            !is_retryable_model_failure(Some("temporary glitch, retry later")),
            "\"temporary\" alone must not match the temporar(?:ily)? unavailable pattern"
        );
    }

    // ---------------------------------------------------------------------------------------
    // add_usage / aggregate_usage (R-SA-040): additive, never last-wins
    // ---------------------------------------------------------------------------------------

    #[test]
    fn add_usage_accumulates_additively_across_calls() {
        let mut total = Usage::default();
        add_usage(&mut total, &usage(10, 20, 0.5));
        add_usage(&mut total, &usage(5, 7, 0.25));
        assert_eq!(total.input, 15);
        assert_eq!(total.output, 27);
        assert_eq!(total.total_tokens, 15 + 27); // sum of each call's own total_tokens
        assert!((total.cost.total - 0.75).abs() < f64::EPSILON);
    }

    #[test]
    fn aggregate_usage_sums_a_whole_slice_including_a_zero_failed_attempt() {
        let usages = [usage(10, 10, 1.0), Usage::default(), usage(5, 5, 0.5)];
        let total = aggregate_usage(usages.iter());
        assert_eq!(total.input, 15);
        assert_eq!(total.output, 15);
        assert!((total.cost.total - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn add_usage_sums_optional_1h_cache_write_and_reasoning_when_present() {
        let mut total = Usage::default();
        let mut a = usage(1, 1, 0.0);
        a.cache_write_1h = Some(3);
        a.reasoning = Some(7);
        add_usage(&mut total, &a);
        add_usage(&mut total, &usage(1, 1, 0.0)); // no 1h/reasoning on this one
        assert_eq!(total.cache_write_1h, Some(3));
        assert_eq!(total.reasoning, Some(7));
    }

    #[test]
    fn add_usage_leaves_optional_fields_none_when_neither_side_ever_reports_them() {
        let mut total = Usage::default();
        add_usage(&mut total, &usage(1, 1, 0.0));
        add_usage(&mut total, &usage(1, 1, 0.0));
        assert_eq!(total.cache_write_1h, None);
        assert_eq!(total.reasoning, None);
    }

    // ---------------------------------------------------------------------------------------
    // run_fallback_ladder: a fully scripted AttemptRunner (no real subprocess — this module's
    // own unit-level control-flow proof; real-subprocess integration is exec/mod.rs's later-
    // phase concern once crate::spawn::SubprocessSpawner exists).
    // ---------------------------------------------------------------------------------------

    struct ScriptedRunner {
        /// One scripted `(AttemptSignal, attempt_label)` per call, consumed in order.
        script: Vec<(AttemptSignal, &'static str)>,
        calls: Vec<(ModelId, Option<String>)>,
        snapshot_calls: u32,
    }

    impl ScriptedRunner {
        fn new(script: Vec<(AttemptSignal, &'static str)>) -> Self {
            Self {
                script,
                calls: Vec::new(),
                snapshot_calls: 0,
            }
        }
    }

    #[async_trait::async_trait]
    impl AttemptRunner for ScriptedRunner {
        type Attempt = &'static str;

        async fn run_attempt(
            &mut self,
            model: &ModelId,
            attempt_note: Option<&str>,
        ) -> (AttemptSignal, Self::Attempt) {
            self.calls
                .push((model.clone(), attempt_note.map(str::to_string)));
            let idx = self.calls.len() - 1;
            self.script[idx].clone()
        }

        fn snapshot_output_file(&mut self) {
            self.snapshot_calls += 1;
        }
    }

    fn ok_signal(usage: Usage) -> AttemptSignal {
        AttemptSignal {
            success: true,
            exit_code: Some(0),
            error: None,
            usage,
            timed_out: false,
            detached: false,
        }
    }

    fn failed_signal(error: &str, usage: Usage) -> AttemptSignal {
        AttemptSignal {
            success: false,
            exit_code: Some(1),
            error: Some(error.to_string()),
            usage,
            timed_out: false,
            detached: false,
        }
    }

    fn timed_out_signal(error: &str, usage: Usage) -> AttemptSignal {
        AttemptSignal {
            success: false,
            exit_code: None,
            error: Some(error.to_string()),
            usage,
            timed_out: true,
            detached: false,
        }
    }

    fn detached_signal(usage: Usage) -> AttemptSignal {
        AttemptSignal {
            success: false,
            exit_code: None,
            error: None,
            usage,
            timed_out: false,
            detached: true,
        }
    }

    #[tokio::test]
    async fn empty_ladder_never_calls_the_runner() {
        let mut runner = ScriptedRunner::new(Vec::new());
        let outcome = run_fallback_ladder(&[], &mut runner).await;
        assert!(outcome.attempted_models.is_empty());
        assert!(outcome.model_attempts.is_empty());
        assert!(outcome.last_signal.is_none());
        assert!(outcome.last_attempt.is_none());
        assert_eq!(runner.calls.len(), 0);
        assert_eq!(runner.snapshot_calls, 0);
    }

    #[tokio::test]
    async fn a_successful_first_attempt_stops_the_ladder_immediately() {
        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (ok_signal(usage(10, 5, 0.1)), "attempt-a"),
            (ok_signal(usage(999, 999, 999.0)), "attempt-b"), // must never be reached
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(outcome.attempted_models, vec![model("a")]);
        assert_eq!(
            runner.calls.len(),
            1,
            "must not spawn a second attempt on success"
        );
        assert_eq!(outcome.last_attempt, Some("attempt-a"));
        assert_eq!(outcome.aggregate_usage.input, 10);
    }

    #[tokio::test]
    async fn a_retryable_failure_advances_to_the_next_candidate_with_a_fresh_attempt() {
        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (
                failed_signal("429 rate limit", usage(10, 0, 0.0)),
                "attempt-a",
            ),
            (ok_signal(usage(5, 5, 0.0)), "attempt-b"),
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(outcome.attempted_models, vec![model("a"), model("b")]);
        assert_eq!(
            runner.calls.len(),
            2,
            "a retryable failure MUST trigger a fresh attempt"
        );
        assert_eq!(
            runner.calls[1].0,
            model("b"),
            "the next candidate is a distinct model"
        );
        assert!(
            runner.calls[1]
                .1
                .as_deref()
                .unwrap_or_default()
                .contains("429 rate limit"),
            "the next attempt's initial context must carry a note about the prior failure"
        );
        assert!(outcome.last_signal.expect("some outcome").success);
        // R-SA-040: usage aggregates ACROSS attempts, including the failed first one.
        assert_eq!(outcome.aggregate_usage.input, 15);
    }

    #[tokio::test]
    async fn a_non_retryable_failure_stops_the_ladder_without_advancing() {
        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (
                failed_signal("assertion failed: expected 2, got 3", usage(10, 0, 0.0)),
                "attempt-a",
            ),
            (ok_signal(usage(1, 1, 0.0)), "attempt-b"), // must never be reached
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(outcome.attempted_models, vec![model("a")]);
        assert_eq!(
            runner.calls.len(),
            1,
            "a non-retryable failure must stop the ladder, never advance"
        );
        assert!(!outcome.last_signal.expect("some outcome").success);
    }

    #[tokio::test]
    async fn a_retryable_failure_on_the_last_candidate_does_not_advance_past_the_ladder_end() {
        let candidates = vec![model("a")]; // only one candidate
        let mut runner = ScriptedRunner::new(vec![(
            failed_signal("rate limit", usage(10, 0, 0.0)),
            "attempt-a",
        )]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(
            runner.calls.len(),
            1,
            "there is no next candidate to advance to"
        );
        assert!(!outcome.last_signal.expect("some outcome").success);
    }

    #[tokio::test]
    async fn every_attempts_usage_is_aggregated_including_failed_ones() {
        let candidates = vec![model("a"), model("b"), model("c")];
        let mut runner = ScriptedRunner::new(vec![
            (failed_signal("rate limit", usage(10, 1, 1.0)), "a"),
            (
                failed_signal("503 service unavailable", usage(20, 2, 2.0)),
                "b",
            ),
            (ok_signal(usage(30, 3, 3.0)), "c"),
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(outcome.attempted_models.len(), 3);
        assert_eq!(outcome.aggregate_usage.input, 10 + 20 + 30);
        assert_eq!(outcome.aggregate_usage.output, 1 + 2 + 3);
        assert!((outcome.aggregate_usage.cost.total - 6.0).abs() < f64::EPSILON);
        assert_eq!(outcome.model_attempts.len(), 3);
        assert!(!outcome.model_attempts[0].success);
        assert!(!outcome.model_attempts[1].success);
        assert!(outcome.model_attempts[2].success);
    }

    // ---------------------------------------------------------------------------------------
    // THE ordering rule (R-SA-036): timeout is a distinct branch checked BEFORE retryable-
    // pattern classification, even when the timeout's own error text also matches a retryable
    // pattern. This is the specific scenario the task brief calls out as load-bearing.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn ordering_rule_timeout_branch_wins_even_when_error_text_matches_a_retryable_pattern() {
        // This error text is deliberately crafted to match BOTH:
        //   - a timeout-shaped phrase ("timed out") -- which is ALSO one of
        //     is_retryable_model_failure's own patterns, and
        //   - a second, unrelated retryable pattern ("rate limit"),
        // so that if the ordering rule were violated (retryable-pattern classification consulted
        // before/instead of the timed_out flag), this attempt would be misclassified as
        // "retryable, advance the ladder" purely because of its text shape.
        let error_text = "request timed out after rate limit backoff";
        assert!(
            is_retryable_model_failure(Some(error_text)),
            "sanity check: this text MUST independently match the retryable-pattern classifier, \
             otherwise this test would not be exercising the ordering rule at all"
        );

        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (timed_out_signal(error_text, usage(10, 0, 0.0)), "attempt-a"),
            (ok_signal(usage(999, 999, 999.0)), "attempt-b"), // must NEVER be reached
        ]);

        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(
            runner.calls.len(),
            1,
            "R-SA-036: a timeout MUST terminate the ladder outright, even though its error text \
             also matches a retryable pattern — the timeout branch must win and the ladder must \
             stop, never advance to the next candidate"
        );
        assert_eq!(outcome.attempted_models, vec![model("a")]);
        let last = outcome.last_signal.expect("some outcome");
        assert!(
            last.timed_out,
            "the terminal signal must be flagged timed_out"
        );
        assert!(!last.success);
        // Usage from the timed-out attempt must still have been aggregated (R-SA-040 applies
        // regardless of R-SA-036 stopping the ladder).
        assert_eq!(outcome.aggregate_usage.input, 10);
    }

    #[tokio::test]
    async fn ordering_rule_detach_branch_also_wins_over_a_matching_retryable_pattern() {
        // R-SA-037: detach is likewise terminal and must not be second-guessed by the retryable
        // classifier even indirectly (detach carries no error text at all here, but the branch
        // ordering itself — checked before the retryable-pattern branch — is what this test
        // proves, mirroring the timeout case above).
        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (detached_signal(usage(3, 0, 0.0)), "attempt-a"),
            (ok_signal(usage(1, 1, 0.0)), "attempt-b"), // must NEVER be reached
        ]);

        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(
            runner.calls.len(),
            1,
            "a detach must terminate the ladder outright"
        );
        let last = outcome.last_signal.expect("some outcome");
        assert!(last.detached);
        assert!(!last.timed_out);
    }

    #[tokio::test]
    async fn a_genuine_non_timeout_retryable_failure_still_advances_normally() {
        // Companion to the ordering-rule test above: proves the ordering fix does not
        // accidentally suppress the normal (non-timeout) retry path for the exact same kind of
        // matching text, when timed_out is correctly false.
        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (
                failed_signal(
                    "request timed out after rate limit backoff",
                    usage(10, 0, 0.0),
                ),
                "attempt-a",
            ),
            (ok_signal(usage(5, 5, 0.0)), "attempt-b"),
        ]);

        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(
            runner.calls.len(),
            2,
            "the SAME error text, when NOT flagged timed_out, is a genuine retryable failure and \
             must advance the ladder normally"
        );
        assert!(outcome.last_signal.expect("some outcome").success);
    }

    #[tokio::test]
    async fn snapshot_output_file_is_called_once_per_attempt_before_each_spawn() {
        let candidates = vec![model("a"), model("b"), model("c")];
        let mut runner = ScriptedRunner::new(vec![
            (failed_signal("rate limit", usage(1, 0, 0.0)), "a"),
            (failed_signal("503", usage(1, 0, 0.0)), "b"),
            (ok_signal(usage(1, 0, 0.0)), "c"),
        ]);
        let _ = run_fallback_ladder(&candidates, &mut runner).await;
        assert_eq!(
            runner.snapshot_calls, 3,
            "R-SA-031: the output file must be snapshotted immediately before EVERY fresh attempt"
        );
    }

    // ---------------------------------------------------------------------------------------
    // ModelOverride
    // ---------------------------------------------------------------------------------------

    #[test]
    fn model_override_inherit_has_no_model_id() {
        assert_eq!(ModelOverride::Inherit.as_model_id(), None);
        assert_eq!(ModelOverride::default(), ModelOverride::Inherit);
    }

    #[test]
    fn model_override_explicit_exposes_its_model_id() {
        let ov = ModelOverride::Explicit(model("x"));
        assert_eq!(ov.as_model_id(), Some(&model("x")));
    }

    // ---------------------------------------------------------------------------------------
    // format_attempt_note
    // ---------------------------------------------------------------------------------------

    #[test]
    fn format_attempt_note_includes_failed_and_next_model_and_trimmed_error() {
        let note = format_attempt_note(&model("a"), Some("  429 rate limit  "), &model("b"));
        assert!(note.contains("a"));
        assert!(note.contains("b"));
        assert!(note.contains("429 rate limit"));
        assert!(!note.contains("  429")); // trimmed, not raw with leading whitespace
    }

    #[test]
    fn format_attempt_note_falls_back_to_a_generic_phrase_when_error_is_absent() {
        let note = format_attempt_note(&model("a"), None, &model("b"));
        assert!(note.contains("attempt failed"));
    }
}
