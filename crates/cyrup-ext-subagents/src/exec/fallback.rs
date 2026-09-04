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
//!    (`pi-subagents/src/runs/shared/model-fallback.ts:278-329`), and its per-ATTEMPT wrapper
//!    [`is_retryable_model_failure_attempt`] (`isRetryableModelFailureAttempt`,
//!    `model-fallback.ts:530-537` @v0.64.0, SUBA-089), which additionally refuses to re-dispatch
//!    an attempt that already ran tools or whose recorded messages do not corroborate the error.
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

use cyrup_core::{ModelId, ProviderId, Usage};

use crate::exec::model_scope::{
    ModelScopeConfig, ModelScopeSeverity, ModelScopeViolation, ModelSource, check_model_scope,
    warn_violation,
};

// -------------------------------------------------------------------------------------------
// R-SA-041: the inherit sentinel
// -------------------------------------------------------------------------------------------

/// pi's `INHERIT_MODEL` sentinel (`runs/shared/model-fallback.ts:22`). A requested model set to
/// the literal string `"inherit"` — like an empty/whitespace-only one, and like pi's `false` —
/// means "no model was requested", NOT "run a model literally named `inherit`". It is never a real
/// model id and must never reach `--model` on a child's argv.
///
/// Canonically owned here, next to [`resolve_model_inheritance`] (the launch-path resolver);
/// `extension.rs`'s `models` report formatter imports it rather than re-declaring the literal.
pub const INHERIT_MODEL_SENTINEL: &str = "inherit";

/// pi's `const explicit = trimmed && trimmed !== INHERIT_MODEL ? trimmed : undefined`
/// (`model-fallback.ts:203-204`): the requested model as a REAL model id, or `None` when the
/// request is absent, blank, or the [`INHERIT_MODEL_SENTINEL`].
pub(crate) fn real_requested_model(requested: Option<&ModelId>) -> Option<&ModelId> {
    let requested = requested?;
    let trimmed = requested.as_str().trim();
    (!trimmed.is_empty() && trimmed != INHERIT_MODEL_SENTINEL).then_some(requested)
}

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
/// **`preferred_provider` (SUBA-088)** — pi `buildModelCandidates`'s 4th parameter
/// (`runs/shared/model-fallback.ts:412-418` @v0.64.0), which every caller passes as
/// `agent.modelProvider ?? options.preferredModelProvider` (`runs/foreground/execution.ts:1885`,
/// `runs/background/async-execution.ts:930`) — the agent's own `subagents.defaultProvider` stamp
/// first, else the PARENT session's provider. Upstream threads it into
/// `resolveSubagentModelCandidate` (`:207-218`), where a BARE id (no `provider/` prefix) that
/// several registry providers offer resolves to the preferred provider's `fullId`. This crate's
/// launch path holds no registry here — `available_models` is the persona's own list, and the
/// bare id is forwarded verbatim as `--model <id>` for the CHILD to resolve — so the preference is
/// applied the only way it can reach the child: each surviving bare candidate is QUALIFIED to
/// `{provider}/{id}` by [`qualify_model_candidate`] before it is deduplicated. A candidate that
/// already names a provider is never rewritten (upstream's "never switches providers for a
/// qualified query"), and `None` leaves every id exactly as it was. The allowlist is consulted on
/// BOTH spellings so a persona-derived list (bare) and an inherited parent model (qualified) each
/// keep matching.
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
    preferred_provider: Option<&ProviderId>,
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
        let qualified = qualify_model_candidate(&candidate, preferred_provider);
        if seen.contains(&qualified) {
            continue;
        }
        if !available_models.contains(&candidate) && !available_models.contains(&qualified) {
            continue;
        }
        seen.push(qualified);
    }
    seen
}

/// SUBA-088: the provider half of a `provider/id` model id, or `None` for a bare id (or one whose
/// halves are not both non-empty — pi `normalizeParentModel`'s rule, `model-fallback.ts:33-39`,
/// under which `""`, `"anthropic"`, `"/sonnet"` and `"anthropic/"` all yield no parent model).
/// This is how the PARENT session's provider is derived for `preferredModelProvider` (pi
/// `currentProvider = parentModel?.provider`, `subagent-executor.ts:3648,3825` @v0.64.0): cyrup's
/// [`ModelId`] carries the joined `"{provider}/{id}"`, so the provider is split back off it.
#[must_use]
pub fn provider_of(model: &ModelId) -> Option<ProviderId> {
    model
        .as_str()
        .split_once('/')
        .filter(|(provider, id)| !provider.is_empty() && !id.is_empty())
        .map(|(provider, _)| ProviderId::from(provider))
}

/// SUBA-088: qualify a BARE model id with `preferred_provider` — `gpt-5` under `openai-codex`
/// becomes `openai-codex/gpt-5`; `gpt-5:high` keeps its thinking suffix
/// (`openai-codex/gpt-5:high`). An id that already carries a `/` is returned unchanged (upstream
/// treats a qualified query as pinned to its provider and never rewrites it), and so is every id
/// when no provider is preferred. A pure decision over its two inputs; the launch shell applies it.
///
/// Documented inference, not upstream text: pi only treats a `/` prefix as a provider when that
/// prefix is a REGISTERED provider (`splitQualifiedModelQuery`, `model-fallback.ts:95-113`), so a
/// Hugging Face `owner/name` id with no registered `owner` would still be a bare id there. With no
/// registry in hand this function treats any `/` as a provider prefix — the same convention the
/// fork-thinking predicate and the `models` report already use for the parent model.
#[must_use]
pub fn qualify_model_candidate(
    model: &ModelId,
    preferred_provider: Option<&ProviderId>,
) -> ModelId {
    let raw = model.as_str();
    match preferred_provider {
        Some(provider) if !raw.contains('/') && !raw.trim().is_empty() => {
            ModelId::from(format!("{}/{raw}", provider.as_str()))
        }
        _ => model.clone(),
    }
}

/// [`build_model_candidates`] plus `subagents.modelScope` enforcement over the ladder's
/// **non-primary** entries — pi `buildModelCandidates`'s `if (index > 0 && options?.scope?.enforce)`
/// arm (`model-fallback.ts:253-276`).
///
/// Only entries AFTER the first raw candidate are checked here, and only ever at `warn` severity,
/// because upstream splits the work exactly that way: candidate #0 is the already-resolved primary
/// model, whose scope check (including the hard-error `explicit` case) belongs to
/// [`resolve_model_inheritance`] / `resolveSubagentModelOverride`; everything after it is inherited
/// agent config (`fallbackModels`), which warns rather than erroring so an existing agent file with
/// an out-of-scope fallback keeps working.
///
/// A warn NEVER removes or substitutes a candidate — the returned ladder is byte-identical to
/// [`build_model_candidates`]'. Filtering here would be a silent downgrade: the run would quietly
/// proceed on a different model than configured with nothing surfaced to the caller.
///
/// Returns the ladder plus every violation observed, so a caller (and a test) can see the warnings
/// rather than having to scrape a log.
#[must_use]
pub fn build_model_candidates_scoped(
    model_override: &ModelOverride,
    agent_primary_model: Option<&ModelId>,
    agent_fallback_models: &[ModelId],
    available_models: &[ModelId],
    preferred_provider: Option<&ProviderId>,
    scope: Option<&ModelScopeConfig>,
) -> (Vec<ModelId>, Vec<ModelScopeViolation>) {
    let candidates = build_model_candidates(
        model_override,
        agent_primary_model,
        agent_fallback_models,
        available_models,
        preferred_provider,
    );
    let mut violations = Vec::new();
    if scope.is_some_and(ModelScopeConfig::is_armed) {
        // pi indexes into the RAW `[primaryModel, ...fallbackModels]` list and skips index 0; the
        // deduped/filtered ladder preserves that first-occurrence ordering, so skipping the first
        // surviving candidate is the same set.
        for candidate in candidates.iter().skip(1) {
            if let Some(violation) =
                check_model_scope(Some(candidate.as_str()), scope, ModelSource::Inherited)
            {
                warn_violation(&violation);
                violations.push(violation);
            }
        }
    }
    (candidates, violations)
}

/// Resolve one subagent attempt's effective [`ModelOverride`], folding in the INHERITED parent
/// session model — the cyrup analog of pi's `resolveSubagentModelOverride(requestedModel,
/// parentModel, availableModels, preferredProvider)`
/// (`pi-subagents/src/runs/shared/model-fallback.ts:196-220`), where `requestedModel = task.model ??
/// agentConfig.model` and `parentModel = ctx.model`.
///
/// As of pi v0.43.0 this is the TWO-STAGE `resolveEffectiveSubagentModel`
/// (`model-fallback.ts:222-245`), not the single-shot `resolveSubagentModelOverride` it wraps.
/// Stage 1 resolves `explicitModel ?? agentModel`; when that yields nothing *and* an explicit
/// per-call model was supplied, stage 2 re-resolves the agent's own model alone. The case that
/// makes the second stage load-bearing: a caller that passes the `"inherit"` sentinel (or a blank
/// `model`) in a headless session with no live parent model. Stage 1 reduces the request to "the
/// parent model", finds none, and returns nothing — without stage 2 the agent's own `model:` is
/// silently shadowed by the caller's non-request and the ladder falls through to the fallback
/// list (or hard-fails empty).
///
/// Precedence, highest first (matching pi's `explicit ?? parentModel` branch, `model-fallback.ts:52-58`):
///
/// 1. `per_call_override` — an explicit per-call (`/run [model=…]`, tool `model`, single-run
///    `model_override`) or per-step (chain step `model`) override, **when it names a real model**.
///    Returned as [`ModelOverride::Explicit`] so it is candidate #0. A blank or
///    [`INHERIT_MODEL_SENTINEL`] value is a request to inherit, never a model id: it skips to
///    branch 3 and then to the stage-2 retry against branch 2.
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
/// # `subagents.modelScope` enforcement (fail-closed)
///
/// This is pi's `resolveSubagentModelOverride` scope gate (`model-fallback.ts:200-212`), and it is
/// the ONE place a model can be REFUSED. `scope` comes from `subagents.modelScope` (project wins
/// over user, [`crate::discovery::types::LayeredOverrideSettings::model_scope`]); `None` — or a
/// config that is not [`ModelScopeConfig::is_armed`] — leaves behavior exactly as it was.
///
/// The resolved model for each branch is checked with the branch's own [`ModelSource`]:
///
/// - branch 1 (`per_call_override`) is [`ModelSource::Explicit`] — a violation is
///   [`ModelScopeSeverity::Error`] and is returned as `Err`, aborting the run before any child
///   process is spawned. The caller surfaces it verbatim; there is deliberately no
///   "pick the nearest allowed model instead" path, because a silent downgrade would run a
///   different model than the caller asked for and hide the policy violation.
/// - branches 2 and 3 (persona `model:`, inherited parent-session model) are
///   [`ModelSource::Inherited`] — a violation only warns (pi's documented back-compat allowance)
///   and the model is still used unchanged.
///
/// # Errors
///
/// Returns the [`ModelScopeViolation`] when an EXPLICIT caller-supplied model falls outside an
/// armed scope. `available_models` is left untouched on that path (nothing was pushed yet), so a
/// caller that retries with a different model sees an unpolluted allowlist.
pub fn resolve_model_inheritance(
    per_call_override: Option<&ModelId>,
    persona_model: Option<&ModelId>,
    inherited_session_model: Option<&ModelId>,
    available_models: &mut Vec<ModelId>,
    scope: Option<&ModelScopeConfig>,
) -> Result<ModelOverride, ModelScopeViolation> {
    // A blank or `"inherit"` model id is a REQUEST, not a candidate. Purge it from the allowlist
    // before anything else: `build_model_candidates` re-derives the ladder from
    // `agent_primary_model`/`agent_fallback_models` independently of this function's return value,
    // so a persona whose frontmatter says `model: inherit` would otherwise still be filtered *in*
    // as candidate #0 and spawn a child with `--model inherit`.
    available_models.retain(|model| real_requested_model(Some(model)).is_some());

    let explicit_present = per_call_override.is_some();

    // --- Stage 1 (pi `resolveEffectiveSubagentModel`, `model-fallback.ts:230-236`) ---
    //
    // The request is `explicitModel ?? agentModel` — JS nullish coalescing over PRESENCE, so a
    // present-but-sentinel per-call `model` does NOT fall through to the persona's model here; it
    // falls through to the parent session model, which is exactly what "inherit" asks for.
    let stage1_request = per_call_override.or(persona_model);
    match real_requested_model(stage1_request) {
        Some(requested) => {
            // pi `:212`: the source is the caller's only when the request itself was a real,
            // explicit model — an explicit request that reduced to the parent model is always
            // `"inherited"` (handled in the `None` arm below).
            let source = if explicit_present {
                ModelSource::Explicit
            } else {
                ModelSource::Inherited
            };
            if let Some(violation) = check_model_scope(Some(requested.as_str()), scope, source) {
                // Fail closed: an explicitly requested out-of-scope model refuses the run.
                if violation.severity == ModelScopeSeverity::Error {
                    return Err(violation);
                }
                warn_violation(&violation);
            }
            if explicit_present {
                return Ok(ModelOverride::Explicit(requested.clone()));
            }
            // The request WAS the persona's own model: keep returning `Inherit` so
            // `build_model_candidates` seats it via `agent_primary_model`, byte-identically to
            // before this two-stage shape existed.
            return Ok(ModelOverride::Inherit);
        }
        None => {
            // Absent / blank / `"inherit"`: pi resolves to `${parentModel.provider}/${id}`
            // (`model-fallback.ts:207`), always at `ModelSource::Inherited` (`:212`).
            if let Some(inherited) = inherited_session_model {
                if let Some(violation) =
                    check_model_scope(Some(inherited.as_str()), scope, ModelSource::Inherited)
                {
                    warn_violation(&violation);
                }
                if !available_models.contains(inherited) {
                    available_models.push(inherited.clone());
                }
                return Ok(ModelOverride::Explicit(inherited.clone()));
            }
        }
    }

    // --- Stage 2 (pi `:237-244`) ---
    //
    // `if (resolved || explicitModel === undefined) return resolved;` — an explicit request that
    // resolved to NOTHING (a `"inherit"`/blank `model` with no live parent session) must not
    // shadow the agent's own configured model. Re-resolve against the persona model alone, at
    // `"inherited"` source so an out-of-scope agent default warns rather than refusing the run.
    if explicit_present && let Some(persona) = real_requested_model(persona_model) {
        if let Some(violation) =
            check_model_scope(Some(persona.as_str()), scope, ModelSource::Inherited)
        {
            warn_violation(&violation);
        }
        if !available_models.contains(persona) {
            available_models.push(persona.clone());
        }
        return Ok(ModelOverride::Explicit(persona.clone()));
    }
    Ok(ModelOverride::Inherit)
}

// -------------------------------------------------------------------------------------------
// R-SA-039: retryable-failure classification
// -------------------------------------------------------------------------------------------

/// One retryable-failure pattern (R-SA-039), a dependency-free re-typing of one of pi-subagents'
/// case-insensitive JS regexes in `RETRYABLE_MODEL_FAILURE_PATTERNS`
/// (`pi-subagents/src/runs/shared/model-fallback.ts:278-314`).
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
    /// Case-insensitive `first\s+(?:a|b|c)`: `first`, then AT LEAST one whitespace character,
    /// then any of the alternatives (pi `/connection\s+(?:error|reset|closed|aborted)/i`, added
    /// by `d8d1408d` inside v0.47.1..v0.57.0 — SUBA-089). Distinct from
    /// [`Self::OptionalWsBetween`]: `connectionreset` is NOT a match.
    WsThenAny(&'static str, &'static [&'static str]),
    /// Case-insensitive `first(?:middle)?second`: `first`, then an optional literal `middle`, then
    /// `second` (pi `/temporar(?:ily)? unavailable/i` → first=`temporar`, middle=`ily`,
    /// second=` unavailable`; pi `/timed? out/i` → first=`time`, middle=`d`, second=` out`).
    OptionalWordBetween(&'static str, &'static str, &'static str),
}

/// The fixed retryable-failure pattern set (R-SA-039), in pi's exact `RETRYABLE_MODEL_FAILURE_PATTERNS`
/// declaration order (`model-fallback.ts:278-314`).
const RETRYABLE_MODEL_FAILURE_PATTERNS: &[RetryPattern] = &[
    RetryPattern::OptionalWsBetween("rate", "limit"), // /rate\s*limit/i
    RetryPattern::Contains("too many requests"),
    RetryPattern::WordNumber("429"), // /\b429\b/
    RetryPattern::Contains("quota"),
    RetryPattern::Contains("billing"),
    RetryPattern::Contains("credit"),
    RetryPattern::Contains("auth"),         // /auth(?:entication)?/i
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
    // SUBA-089 (`d8d1408d`, v0.57.0 `model-fallback.ts:428`; v0.64.0 `:496`): a dropped provider
    // connection is a provider failure. The same commit narrowed the ladder gate to
    // `isRetryableModelFailureAttempt` so this broader text never re-runs a child that already
    // did work — the two halves ship together.
    RetryPattern::WsThenAny("connection", &["error", "reset", "closed", "aborted"]), // /connection\s+(?:error|reset|closed|aborted)/i
    RetryPattern::Contains("connection refused"),
    RetryPattern::Contains("fetch failed"),
    RetryPattern::Contains("network error"),
    RetryPattern::Contains("socket hang up"),
    // Added upstream at v0.43.0 (`model-fallback.ts:303`): a stream that ended without a
    // `finish_reason` is a truncated provider response, not a task failure.
    RetryPattern::Contains("stream ended without finish_reason"),
    RetryPattern::Contains("upstream"),
    RetryPattern::OptionalWordBetween("time", "d", " out"), // /timed? out/i
    RetryPattern::Contains("timeout"),
    RetryPattern::WordNumber("502"),                    // /\b502\b/
    RetryPattern::WordNumber("503"),                    // /\b503\b/
    RetryPattern::WordNumber("504"),                    // /\b504\b/
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
            Some(pos) => line
                .get(pos + first.len()..)
                .is_some_and(|rest| seconds.iter().any(|second| rest.contains(second))),
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
        RetryPattern::WsThenAny(first, alternatives) => {
            let mut search = 0;
            while let Some(rel) = line.get(search..).and_then(|s| s.find(first)) {
                let start = search + rel;
                if let Some(rest) = line.get(start + first.len()..) {
                    let after_ws = rest.trim_start_matches(char::is_whitespace);
                    // `\s+`: the whitespace run must be non-empty.
                    if after_ws.len() < rest.len()
                        && alternatives.iter().any(|alt| after_ws.starts_with(alt))
                    {
                        return true;
                    }
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

/// pi `TOOL_FAILURE_PREFIX` (`model-fallback.ts:316-323`), hand-rolled for the same reason the
/// rest of this module is: `^[\w.:@/-]+ failed (?:\(exit \d+\):|with exit code \d+)(?:\s|$)`.
///
/// Those two shapes are produced by exactly one thing in this crate — [`crate::exec::output::
/// DetectedSubagentError::message`], the exit-0 re-diagnosis of a trailing failed tool/bash call
/// (`output.rs`, pi `execution.ts:776-778`). They describe a TOOL that failed inside the child's
/// task, never the provider or the model, however network-flavoured the tool's own output reads.
/// The overlap is not hypothetical: `FATAL_BASH_PATTERNS` (`output.rs`) and
/// [`RETRYABLE_MODEL_FAILURE_PATTERNS`] literally share `"connection refused"` and `"timeout"`, so
/// `bash failed (exit 1): curl: (7) Failed to connect ... Connection refused` was classified
/// retryable and re-ran the child's WHOLE task on the next model in the ladder — which cannot fix
/// a tool failure and costs a full extra run. Tool names include namespaced forms
/// (`mcp.server/write`), hence the `.`/`:`/`@`/`/`/`-` members of the leading character class.
///
/// Case-insensitive like pi's `/i`; anchored at the start of the trimmed error like pi's `^`.
fn is_tool_failure_prefix(error: &str) -> bool {
    let trimmed = error.trim_start();
    // `[\w.:@/-]+` — all ASCII, so the byte count is a valid char boundary.
    let name_len = trimmed
        .bytes()
        .take_while(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'@' | b'/' | b'-')
        })
        .count();
    if name_len == 0 {
        return false;
    }
    let Some(rest) = trimmed
        .get(name_len..)
        .and_then(|r| strip_prefix_ci(r, " failed "))
    else {
        return false;
    };
    let tail = if let Some(after) = strip_prefix_ci(rest, "(exit ") {
        // `\(exit \d+\):`
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        match after.get(digits..).and_then(|r| r.strip_prefix("):")) {
            Some(tail) => tail,
            None => return false,
        }
    } else if let Some(after) = strip_prefix_ci(rest, "with exit code ") {
        // `with exit code \d+`
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return false;
        }
        match after.get(digits..) {
            Some(tail) => tail,
            None => return false,
        }
    } else {
        return false;
    };
    // `(?:\s|$)`
    tail.is_empty() || tail.starts_with(char::is_whitespace)
}

/// ASCII-case-insensitive [`str::strip_prefix`].
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    let rest = s.get(prefix.len()..)?;
    head.eq_ignore_ascii_case(prefix).then_some(rest)
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
    // pi `:326`: the tool-failure guard runs FIRST and short-circuits the whole pattern set.
    if is_tool_failure_prefix(error.trim()) {
        return false;
    }
    let haystack = error.to_lowercase();
    // Per-line so the `.*`/`\s*`/`.?` constructs never cross a newline (JS `.`-no-newline). A
    // `Contains`/`WordNumber` needle can never straddle a `\n` either, so per-line evaluation is
    // equivalent to whole-string for those and correct for the sequence patterns.
    haystack.split('\n').any(|line| {
        RETRYABLE_MODEL_FAILURE_PATTERNS
            .iter()
            .any(|p| line_matches(line, p))
    })
}

/// The prefix/suffix of pi's second empty-output sentinel — `Subagent produced no output after
/// terminal assistant stopReason "<reason>".` (`formatEmptyTerminalAssistantResponseError`,
/// `shared/utils.ts:472` @v0.64.0). Cyrup's own empty-output diagnosis emits only the cold-start
/// form ([`crate::exec::output::EMPTY_OUTPUT_ERROR`]); this form is recognised so the predicate
/// matches upstream's `/^Subagent produced no output after terminal assistant stopReason
/// "[^"]+"\.$/` (`model-fallback.ts:533` @v0.64.0) the day cyrup produces it too.
const EMPTY_OUTPUT_AFTER_STOP_REASON_PREFIX: &str =
    "Subagent produced no output after terminal assistant stopReason \"";
const EMPTY_OUTPUT_AFTER_STOP_REASON_SUFFIX: &str = "\".";

/// `model-fallback.ts:533` @v0.64.0 — is `error` one of the two empty-output sentinels? Exact and
/// untrimmed, like upstream's `===` and its anchored regex (`[^"]+` is any non-empty run of
/// non-quote characters, newlines included; `$` is end of input).
fn is_empty_output_sentinel(error: &str) -> bool {
    if error == crate::exec::output::EMPTY_OUTPUT_ERROR {
        return true;
    }
    error
        .strip_prefix(EMPTY_OUTPUT_AFTER_STOP_REASON_PREFIX)
        .and_then(|rest| rest.strip_suffix(EMPTY_OUTPUT_AFTER_STOP_REASON_SUFFIX))
        .is_some_and(|stop_reason| !stop_reason.is_empty() && !stop_reason.contains('"'))
}

/// SUBA-089 — the per-ATTEMPT retry decision the ladder actually gates on
/// (`isRetryableModelFailureAttempt({error, messages, toolCount})`, `model-fallback.ts:530-537`
/// @v0.64.0; introduced by `d8d1408d` inside v0.47.1..v0.57.0, where the gate had been the bare
/// [`is_retryable_model_failure`], `v0.47.1:execution.ts:1633`). Clause for clause, in upstream's
/// order:
///
/// 1. `!isRetryableModelFailure(error)` → `false` — the text is not a model/provider failure.
/// 2. `toolCount > 0` → `false` — the child already ran tools. **This is the load-bearing half**:
///    a child that made ten edits and then hit `connection reset` must NOT be re-run from scratch
///    on the next model, duplicating its side effects and its token spend.
/// 3. The error is one of the two empty-output sentinels → `true` regardless of messages (a
///    cold-start/empty response is retryable even when the transcript holds an assistant turn).
/// 4. `toolCount === 0 && messages.length === 0` → `true` — nothing ran; the error can only be
///    the provider's.
/// 5. Otherwise `true` only if some recorded message's own `errorMessage` (trimmed) equals the
///    error (trimmed): the transcript corroborates that the provider failed. Raw process stderr
///    that merely *reads* retryable after real activity (upstream's test case, `test/unit/
///    model-fallback.test.ts:342` @v0.64.0) stops the ladder.
///
/// Pure over the signal ([`AttemptSignal::error`], [`StartupEvidence::tool_count`],
/// [`StartupEvidence::message_count`], [`AttemptSignal::message_errors`]); the ladder's
/// timeout/detach/success/startup arms run before it, exactly as before (see
/// [`run_fallback_ladder`]).
#[must_use]
pub fn is_retryable_model_failure_attempt(signal: &AttemptSignal) -> bool {
    let error = signal.error.as_deref();
    if !is_retryable_model_failure(error) {
        return false;
    }
    let Some(error) = error else {
        return false;
    };
    if signal.startup.tool_count > 0 {
        return false;
    }
    if is_empty_output_sentinel(error) {
        return true;
    }
    if signal.startup.tool_count == 0 && signal.startup.message_count == 0 {
        return true;
    }
    let error = error.trim();
    !error.is_empty()
        && signal
            .message_errors
            .iter()
            .any(|message_error| message_error.trim() == error)
}

/// Format the "prior attempt failed" note appended into the next attempt's initial
/// `recent_output`/progress context (R-SA-039's "append a note about the prior attempt into the
/// next attempt's initial `recent_output`/progress context"), mirroring pi's
/// `formatModelAttemptNote` (`model-fallback.ts:331-336`).
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
    /// SUBA-089 — the `errorMessage` of every message the child emitted this attempt, in order
    /// (pi `messageError(message)` over `result.messages`/`run.messages`, `model-fallback.ts:524-528`
    /// @v0.64.0; every `message_end` message regardless of role, `execution.ts:1122,1190`). Read
    /// by [`is_retryable_model_failure_attempt`]'s last clause: a retryable-looking error that the
    /// child's own transcript never reported is raw process stderr after real activity, and is
    /// NOT re-dispatched on another model.
    ///
    /// Deliberately on the signal rather than in [`StartupEvidence`]: that struct's contract is
    /// "every field is a reason NOT to relaunch", and this list is the opposite — corroborating
    /// evidence FOR advancing the ladder. Empty when no message carried one (pi's `messages?.some`
    /// over an empty list is `false`).
    pub message_errors: Vec<String>,
    /// Everything [`is_retryable_subagent_startup_failure`] needs beyond the fields above, to tell
    /// "the child never started" apart from "the child started and failed". See
    /// [`StartupEvidence`].
    pub startup: StartupEvidence,
}

// -------------------------------------------------------------------------------------------
// Startup retry: a child that dies before doing ANYTHING is relaunched on the SAME model
// (a port of pi-subagents' `runs/shared/subagent-startup-retry.ts`, v0.43.0, 104 lines)
// -------------------------------------------------------------------------------------------

/// Backoff before each relaunch of a child that exited before any model or tool activity
/// (`subagent-startup-retry.ts:9`, verbatim). Its length also fixes the attempt budget: 3 delays =
/// up to 4 launches of the same model.
///
/// Short and bounded on purpose (upstream's own comment): long enough for a concurrent startup lock
/// to clear, short enough not to amplify a persistently broken binary into a multi-second stall on
/// every candidate in the ladder.
pub const SUBAGENT_STARTUP_RETRY_DELAYS_MS: [u64; 3] = [250, 750, 1500];

/// A genuine startup race fails well before a model request could possibly complete
/// (`subagent-startup-retry.ts:12`). A zero-activity failure that took LONGER than this is not a
/// startup failure — something was happening — and is not retried.
pub const MAX_SUBAGENT_STARTUP_FAILURE_DURATION_MS: u64 = 2000;

/// pi `formatProcessSignalError` (`runs/shared/process-signal.ts:1-3`). Ported here rather than in
/// a `process_signal` module of its own because this predicate is currently its only consumer —
/// the rest of `process-signal.ts` (`isUnexplainedProcessSignal`) is a separate, still-unported
/// gap.
#[must_use]
pub fn format_process_signal_error(signal: &str) -> String {
    format!("Subagent process terminated by signal {signal}.")
}

/// The per-attempt facts the startup-failure classifier reads that [`AttemptSignal`]'s other fields
/// do not already carry (`SubagentStartupFailureEvidence`, `subagent-startup-retry.ts:16-32`).
///
/// Every field is a REASON NOT TO RETRY. The classifier fails closed: anything that looks like the
/// child actually did something — a message, a tool call, any usage, a produced output, a protocol
/// violation, an unexplained signal, a lifecycle interruption — disqualifies the retry. That
/// asymmetry is the whole point: retrying a child that genuinely ran and failed would double the
/// cost and hide the failure, while NOT retrying a child that never started masks a transient
/// launch race behind a permanent-looking error.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupEvidence {
    /// The attempt produced some non-blank final output (`finalOutput`).
    pub final_output_present: bool,
    /// How many assistant/tool messages the child emitted (`messages.length`).
    pub message_count: usize,
    /// How many tool calls the child started (`progressSummary.toolCount`).
    pub tool_count: u32,
    /// The attempt's wall-clock duration. `None` means "unknown", which upstream models as
    /// `Number.POSITIVE_INFINITY` — never within the startup window, so never retryable.
    pub duration_ms: Option<u64>,
    /// The attempt failed with a `protocol_output_limit` (`protocolError`). A child that produced
    /// 16 MiB of output on one line unquestionably started.
    pub protocol_error: bool,
    /// The OS signal that killed the child, if any (`processSignal`). Only `SIGKILL` is tolerated
    /// (upstream `:60`) — that is what this crate's own escalation ladder ends with, so it does not
    /// prove the child misbehaved; any OTHER signal is evidence something external acted on it.
    pub process_signal: Option<String>,
    /// The completion guard saw a mutation attempt (`observedMutationAttempt`).
    pub observed_mutation_attempt: bool,
    /// The run was stopped by an explicit user/agent stop (`stopped`).
    pub stopped: bool,
    /// The run hit its turn budget (`turnBudgetExceeded`).
    pub turn_budget_exceeded: bool,
    /// [CYRUP-DELTA, load-bearing] The attempt's `error` is cyrup's own bare-non-zero-exit
    /// PLACEHOLDER (`subagent attempt exited with code N`), not a diagnosed failure.
    ///
    /// pi leaves `result.error` `undefined` for a non-zero exit it could not explain, and
    /// `isRetryableSubagentStartupFailure` keys on exactly that absence (`:52`
    /// `!evidence.error?.trim()`). cyrup cannot leave it unset — `SingleResult`/`ModelAttempt`
    /// callers surface `error` directly — so `run_attempt` fills a stable placeholder string.
    /// Without this flag that placeholder is non-blank error TEXT, the predicate's silence test
    /// fails for every failed attempt, and the whole startup retry is dead code that can never
    /// fire. This flag is how cyrup spells pi's `undefined`.
    pub error_is_placeholder: bool,
}

/// Classify a failed attempt as an unexplained ZERO-ACTIVITY child startup failure — the only
/// failure this crate relaunches on the SAME model (`isRetryableSubagentStartupFailure`,
/// `subagent-startup-retry.ts:48-67`, term for term).
///
/// The boundary this draws is the whole feature: get it too loose and a legitimately failing model
/// is launched four times over; too tight and a concurrent-startup race surfaces as a hard failure
/// the ladder then burns a fallback model on. So: a non-zero numeric exit code, no error text at
/// all (or exactly this crate's own SIGKILL text), no output, no messages, no tools, no usage, a
/// duration inside [`MAX_SUBAGENT_STARTUP_FAILURE_DURATION_MS`], no protocol error, no signal other
/// than SIGKILL, and none of the lifecycle flags set.
#[must_use]
pub fn is_retryable_subagent_startup_failure(signal: &AttemptSignal) -> bool {
    let evidence = &signal.startup;
    let sigkill_error = format_process_signal_error("SIGKILL");
    let error_is_silent = evidence.error_is_placeholder
        || match signal.error.as_deref() {
            None => true,
            Some(error) => error.trim().is_empty() || error == sigkill_error,
        };
    signal.exit_code.is_some_and(|code| code != 0)
        && error_is_silent
        && !evidence.final_output_present
        && evidence.message_count == 0
        && evidence.tool_count == 0
        && !has_usage(&signal.usage)
        && evidence
            .duration_ms
            .is_some_and(|duration| duration <= MAX_SUBAGENT_STARTUP_FAILURE_DURATION_MS)
        && !evidence.protocol_error
        && evidence
            .process_signal
            .as_deref()
            .is_none_or(|signal| signal == "SIGKILL")
        && !evidence.observed_mutation_attempt
        && !signal.detached
        && !signal.timed_out
        && !evidence.stopped
        && !evidence.turn_budget_exceeded
}

/// pi `hasUsage` (`subagent-startup-retry.ts:34-41`): did the attempt account for ANY tokens or
/// cost? cyrup's [`Usage`] has no `turns` counter (pi carries one on its own aggregate); every other
/// field maps one to one, and `total_tokens` is checked as well since a provider may report it
/// without a per-direction split.
fn has_usage(usage: &Usage) -> bool {
    usage.input != 0
        || usage.output != 0
        || usage.cache_read != 0
        || usage.cache_write != 0
        || usage.total_tokens != 0
        || usage.cost.total != 0.0
}

/// The note recorded against the failed attempt and injected into the relaunched child's context
/// (`formatSubagentStartupRetryNote`, `subagent-startup-retry.ts:69-76`, verbatim text).
#[must_use]
pub fn format_subagent_startup_retry_note(
    model: &str,
    attempt: usize,
    max_attempts: usize,
    delay_ms: u64,
) -> String {
    format!(
        "[startup-retry] {model} exited before model or tool activity (attempt \
         {attempt}/{max_attempts}). Retrying the same model in {delay_ms}ms."
    )
}

/// The terminal error once every startup attempt is spent
/// (`formatSubagentStartupRetryExhaustedError`, `subagent-startup-retry.ts:78-83`, verbatim text).
#[must_use]
pub fn format_subagent_startup_retry_exhausted_error(model: &str, attempts: usize) -> String {
    format!(
        "Subagent failed to start after {attempts} attempts on {model}; no model, tool, output, \
         usage, or diagnostic activity was observed. This may be a concurrent Pi startup race. \
         Retry the run or temporarily lower subagent concurrency."
    )
}

/// The outcome of waiting out a startup-retry backoff (`waitForSubagentStartupRetry`,
/// `subagent-startup-retry.ts:86-104`), split three ways because upstream's caller branches on
/// WHICH signal aborted the wait (`execution.ts:1583-1600`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupRetryWait {
    /// The delay elapsed undisturbed — relaunch.
    Proceed,
    /// A soft interrupt fired during the backoff: the run is PAUSED, not failed (exit 0, cleared
    /// error, the paused sentinel output).
    Interrupted,
    /// A hard cancel fired during the backoff: the run is abandoned with a cancellation error.
    Cancelled,
}

/// How the ladder resolved a startup-retry sequence, handed to
/// [`AttemptRunner::apply_startup_outcome`] so the runner can stamp its own richer per-attempt
/// payload (which the ladder cannot see inside).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOutcome {
    /// A soft interrupt landed during a backoff (`execution.ts:1584-1592`).
    Interrupted,
    /// A hard cancel landed during a backoff (`execution.ts:1593-1599`).
    Cancelled(String),
    /// Every startup attempt was spent (`execution.ts:1606-1618`).
    Exhausted(String),
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
    /// path). Returned verbatim inside [`FallbackOutcome::last_attempt`].
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

    /// Wait out one startup-retry backoff, aborting early if a lifecycle signal fires
    /// (`waitForSubagentStartupRetry`, `subagent-startup-retry.ts:86-104`).
    ///
    /// The default sleeps and proceeds — correct for a runner with no cancellation channel at all
    /// (test runners). The production runner overrides it to race the delay against
    /// `RunOptions.cancel`/`RunOptions.interrupt`, because a backoff that ignores them would hold a
    /// cancelled run open for up to 1.5s AND then relaunch a child into it.
    async fn wait_startup_retry(&mut self, delay: std::time::Duration) -> StartupRetryWait {
        tokio::time::sleep(delay).await;
        StartupRetryWait::Proceed
    }

    /// Stamp a startup-retry resolution onto this runner's own per-attempt payload — pi mutates
    /// `result.finalOutput`/`result.interrupted`/`result.progress` in place at
    /// `execution.ts:1584-1618`, which the ladder cannot do here because `Attempt` is opaque to it.
    /// Default no-op.
    fn apply_startup_outcome(&mut self, _attempt: &mut Self::Attempt, _outcome: &StartupOutcome) {}
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
///    [`is_retryable_model_failure_attempt`] ever actually consulted, and only for an attempt
///    that has already been confirmed to be neither a timeout nor a detach. SUBA-089: it is the
///    per-attempt form, not the bare text classifier — an attempt that already ran tools, or
///    whose transcript does not corroborate a retryable-looking error, stops the ladder here
///    (pi `execution.ts:2144,2151` and `background/subagent-runner.ts:2090,2097` @v0.64.0; cyrup's
///    background runner reaches this same loop through `exec::run_sync`).
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
pub async fn run_fallback_ladder<R: AttemptRunner + Send>(
    candidates: &[ModelId],
    runner: &mut R,
) -> FallbackOutcome<R::Attempt> {
    let mut attempted_models = Vec::with_capacity(candidates.len());
    let mut model_attempts = Vec::with_capacity(candidates.len());
    let mut aggregate = Usage::default();
    let mut last_signal: Option<AttemptSignal> = None;
    let mut last_attempt: Option<R::Attempt> = None;
    let mut attempt_note: Option<String> = None;

    'ladder: for (i, model) in candidates.iter().enumerate() {
        // pi `for (let startupAttemptIndex = 0; ; startupAttemptIndex++)` (`execution.ts:1518`):
        // the SAME model may be relaunched, without advancing the ladder, when the child died
        // before doing anything at all.
        let mut startup_attempt_index = 0usize;
        loop {
            runner.snapshot_output_file(); // R-SA-031: snapshot immediately before each fresh spawn
            let (mut signal, mut attempt) = runner
                .run_attempt(model, attempt_note.take().as_deref())
                .await; // R-SA-039: always a fresh child subprocess per candidate

            // pi records the candidate ONCE per model (`execution.ts:1536-1539`) — a startup
            // relaunch is the same rung of the ladder, not a new one.
            if startup_attempt_index == 0 {
                attempted_models.push(model.clone());
            }
            add_usage(&mut aggregate, &signal.usage); // R-SA-040: additive, even for a failed attempt

            // ...but every LAUNCH gets its own row (`modelAttempts.push` is inside the inner loop),
            // so a run that relaunched three times shows three rows and the retry notes that
            // explain them.
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
                break 'ladder;
            }
            // --- R-SA-037: detach is likewise terminal, checked before retry classification. ---
            if signal.detached {
                last_signal = Some(signal);
                last_attempt = Some(attempt);
                break 'ladder;
            }

            if signal.success {
                last_signal = Some(signal);
                last_attempt = Some(attempt);
                break 'ladder;
            }

            // --- Startup retry (pi `execution.ts:1558-1619`), evaluated BEFORE the model-fallback
            // --- decision and before the last-candidate stop: a child that never started says
            // --- nothing about the MODEL, so advancing the ladder (or giving up on the last rung)
            // --- would spend a fallback model on what is usually a concurrent-launch race. ---
            let startup_failure = is_retryable_subagent_startup_failure(&signal);
            let retry_delay_ms = SUBAGENT_STARTUP_RETRY_DELAYS_MS
                .get(startup_attempt_index)
                .copied();
            if let (true, Some(delay_ms)) = (startup_failure, retry_delay_ms) {
                let note = format_subagent_startup_retry_note(
                    model.as_str(),
                    startup_attempt_index + 1,
                    SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1,
                    delay_ms,
                );
                match runner
                    .wait_startup_retry(std::time::Duration::from_millis(delay_ms))
                    .await
                {
                    StartupRetryWait::Proceed => {}
                    StartupRetryWait::Interrupted => {
                        // pi `:1584-1592`: a soft interrupt during the backoff is a PAUSE, not a
                        // failure — exit 0, cleared error, paused sentinel output.
                        signal.success = true;
                        signal.exit_code = Some(0);
                        signal.error = None;
                        runner.apply_startup_outcome(&mut attempt, &StartupOutcome::Interrupted);
                        last_signal = Some(signal);
                        last_attempt = Some(attempt);
                        break 'ladder;
                    }
                    StartupRetryWait::Cancelled => {
                        let cancelled =
                            "Subagent startup retry cancelled before relaunch.".to_string();
                        signal.error = Some(cancelled.clone());
                        if let Some(row) = model_attempts.last_mut() {
                            row.error = Some(cancelled.clone());
                        }
                        runner.apply_startup_outcome(
                            &mut attempt,
                            &StartupOutcome::Cancelled(cancelled),
                        );
                        last_signal = Some(signal);
                        last_attempt = Some(attempt);
                        break 'ladder;
                    }
                }
                // pi `:1602-1604`: the note replaces this launch's error on its own row and is
                // injected into the relaunched child's context.
                if let Some(row) = model_attempts.last_mut() {
                    row.error = Some(note.clone());
                }
                attempt_note = Some(note);
                // `signal`/`attempt` are deliberately dropped here rather than parked in
                // `last_signal`/`last_attempt`: `continue` always runs another attempt for the same
                // model, which overwrites both before anything can read them (pi's `lastResult` is
                // likewise overwritten on the next pass). Assigning them would be dead.
                startup_attempt_index += 1;
                continue;
            }
            if startup_failure {
                // Every launch spent, still zero activity (pi `:1606-1618`).
                let exhausted = format_subagent_startup_retry_exhausted_error(
                    model.as_str(),
                    startup_attempt_index + 1,
                );
                signal.error = Some(exhausted.clone());
                if let Some(row) = model_attempts.last_mut() {
                    row.error = Some(exhausted.clone());
                }
                runner.apply_startup_outcome(&mut attempt, &StartupOutcome::Exhausted(exhausted));
                last_signal = Some(signal);
                last_attempt = Some(attempt);
                break 'ladder;
            }

            if is_last_candidate {
                last_signal = Some(signal);
                last_attempt = Some(attempt);
                break 'ladder;
            }

            // Only reached for a non-timeout, non-detached, non-last-candidate failure: NOW (and
            // only now) is the retryable-pattern classifier consulted (R-SA-039) — in its
            // per-attempt form (SUBA-089), so a child that already ran tools is never re-run.
            if !is_retryable_model_failure_attempt(&signal) {
                last_signal = Some(signal);
                last_attempt = Some(attempt);
                break 'ladder;
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
            break;
        }
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
        );
        assert!(candidates.is_empty());
    }

    #[test]
    fn candidate_ladder_with_no_override_and_no_agent_model_is_empty() {
        let candidates =
            build_model_candidates(&ModelOverride::Inherit, None, &[], &[model("a")], None);
        assert!(candidates.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-088: preferred provider qualifies BARE candidates (pi `buildModelCandidates`'s 4th
    // parameter -> `resolveSubagentModelCandidate(model, available, preferredProvider)`)
    // ---------------------------------------------------------------------------------------

    /// pi `execution.ts:1885` `agent.modelProvider ?? options.preferredModelProvider`: the agent's
    /// own provider is what the caller passes, and a bare id lands on the child as
    /// `provider/id`. Fails before SUBA-088: the ladder had no provider input and forwarded `gpt-5`
    /// verbatim.
    #[test]
    fn bare_candidate_is_qualified_by_the_preferred_provider() {
        let available = vec![model("gpt-5"), model("gpt-5-mini:high")];
        let provider = ProviderId::from("openai-codex");
        let candidates = build_model_candidates(
            &ModelOverride::Inherit,
            Some(&model("gpt-5")),
            &[model("gpt-5-mini:high")],
            &available,
            Some(&provider),
        );
        assert_eq!(
            candidates,
            vec![
                model("openai-codex/gpt-5"),
                model("openai-codex/gpt-5-mini:high")
            ],
            "every bare id is qualified; a thinking suffix rides along"
        );
    }

    /// A qualified id is pinned to its provider (upstream never switches providers for a qualified
    /// query), and with no preference nothing is rewritten — the pre-SUBA-088 ladder byte for byte.
    #[test]
    fn qualified_candidates_and_no_preference_are_left_untouched() {
        let available = vec![model("anthropic/claude-opus-4-6"), model("gpt-5")];
        let provider = ProviderId::from("openai-codex");
        let candidates = build_model_candidates(
            &ModelOverride::Inherit,
            Some(&model("anthropic/claude-opus-4-6")),
            &[model("gpt-5")],
            &available,
            Some(&provider),
        );
        assert_eq!(
            candidates,
            vec![
                model("anthropic/claude-opus-4-6"),
                model("openai-codex/gpt-5")
            ]
        );
        let untouched = build_model_candidates(
            &ModelOverride::Inherit,
            Some(&model("anthropic/claude-opus-4-6")),
            &[model("gpt-5")],
            &available,
            None,
        );
        assert_eq!(
            untouched,
            vec![model("anthropic/claude-opus-4-6"), model("gpt-5")]
        );
    }

    /// Dedup runs on the QUALIFIED spelling (pi's `seen` set holds the normalized id), so an
    /// explicit `openai-codex/gpt-5` override and the persona's bare `gpt-5` are one rung; and the
    /// allowlist accepts either spelling so an inherited (qualified) parent model still passes.
    #[test]
    fn qualification_dedups_against_the_qualified_spelling_and_keeps_qualified_allowlist_entries() {
        let provider = ProviderId::from("openai-codex");
        let available = vec![model("gpt-5"), model("openai-codex/gpt-5")];
        let candidates = build_model_candidates(
            &ModelOverride::Explicit(model("openai-codex/gpt-5")),
            Some(&model("gpt-5")),
            &[],
            &available,
            Some(&provider),
        );
        assert_eq!(candidates, vec![model("openai-codex/gpt-5")]);
    }

    /// `provider_of` is pi `normalizeParentModel`'s two-non-empty-halves rule applied to the joined
    /// `provider/id` form; `qualify_model_candidate` never touches a blank or qualified id.
    #[test]
    fn provider_of_and_qualify_follow_the_parent_model_rules() {
        assert_eq!(
            provider_of(&model("anthropic/claude-opus-4-6")),
            Some(ProviderId::from("anthropic"))
        );
        assert_eq!(provider_of(&model("gpt-5")), None);
        assert_eq!(provider_of(&model("/sonnet")), None);
        assert_eq!(provider_of(&model("anthropic/")), None);
        let provider = ProviderId::from("groq");
        assert_eq!(
            qualify_model_candidate(&model("llama-4"), Some(&provider)),
            model("groq/llama-4")
        );
        assert_eq!(
            qualify_model_candidate(&model("groq/llama-4"), Some(&provider)),
            model("groq/llama-4")
        );
        assert_eq!(
            qualify_model_candidate(&model(""), Some(&provider)),
            model("")
        );
        assert_eq!(
            qualify_model_candidate(&model("llama-4"), None),
            model("llama-4")
        );
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
        let mut available_models: Vec<ModelId> = persona_fallbacks
            .iter()
            .cloned()
            .chain(persona_model.cloned())
            .collect();
        let ov = resolve_model_inheritance(
            None,
            persona_model,
            Some(&inherited),
            &mut available_models,
            None,
        )
        .expect("no scope configured, so resolution cannot be refused");

        assert_eq!(ov, ModelOverride::Explicit(inherited.clone()));
        assert!(
            available_models.contains(&inherited),
            "the inherited model must be added to available_models so the allowlist filter keeps it"
        );
        let candidates = build_model_candidates(
            &ov,
            persona_model,
            &persona_fallbacks,
            &available_models,
            None,
        );
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
        let ov =
            resolve_model_inheritance(None, None, Some(&inherited), &mut available_models, None)
                .expect("no scope configured");
        let candidates = build_model_candidates(&ov, None, &[], &available_models, None);
        assert!(!candidates.is_empty());
        assert_eq!(candidates, vec![inherited]);
    }

    #[test]
    fn a_per_call_override_wins_over_an_inherited_session_model() {
        // (b) explicit per-call/per-step override beats inheritance.
        let inherited = model("together/zai-org/GLM-5.2");
        let per_call = model("z");
        let mut available_models: Vec<ModelId> = vec![per_call.clone()];
        let ov = resolve_model_inheritance(
            Some(&per_call),
            None,
            Some(&inherited),
            &mut available_models,
            None,
        )
        .expect("no scope configured");
        assert_eq!(ov, ModelOverride::Explicit(per_call.clone()));
        let candidates = build_model_candidates(&ov, None, &[], &available_models, None);
        assert_eq!(
            candidates.first(),
            Some(&per_call),
            "per-call override is candidate #0"
        );
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
        let ov = resolve_model_inheritance(
            None,
            Some(&persona),
            Some(&inherited),
            &mut available_models,
            None,
        )
        .expect("no scope configured");
        assert_eq!(ov, ModelOverride::Inherit);
        let candidates = build_model_candidates(&ov, Some(&persona), &[], &available_models, None);
        assert_eq!(
            candidates.first(),
            Some(&persona),
            "persona model is candidate #0"
        );
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
        let ov = resolve_model_inheritance(None, None, None, &mut available_models, None)
            .expect("no scope configured");
        assert_eq!(ov, ModelOverride::Inherit);
        assert_eq!(
            available_models, fallbacks,
            "no inherited model may be added when there is no host"
        );
        let candidates = build_model_candidates(&ov, None, &fallbacks, &available_models, None);
        assert_eq!(
            candidates, fallbacks,
            "the ladder is exactly the persona fallback list"
        );

        // ...and with neither a persona model nor fallbacks nor a host, the ladder stays empty (the
        // caller's genuine hard pre-spawn error) — never a spuriously-invented candidate.
        let mut empty_avail: Vec<ModelId> = Vec::new();
        let ov = resolve_model_inheritance(None, None, None, &mut empty_avail, None)
            .expect("no scope configured");
        assert_eq!(ov, ModelOverride::Inherit);
        assert!(build_model_candidates(&ov, None, &[], &empty_avail, None).is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-003: modelScope enforcement over the ladder (pi `model-fallback.ts:200-237`)
    // ---------------------------------------------------------------------------------------

    fn armed_scope(patterns: &[&str]) -> crate::exec::model_scope::ModelScopeConfig {
        crate::exec::model_scope::ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(patterns.iter().map(|p| (*p).to_string()).collect()),
        }
    }

    /// An out-of-scope FALLBACK entry warns but is NOT removed — pi's `index > 0` arm. Removing it
    /// would be the silent downgrade this feature exists to prevent: the run would quietly attempt
    /// a different model than the agent declared, with nothing surfaced anywhere.
    #[test]
    fn out_of_scope_fallback_candidates_warn_without_changing_the_ladder() {
        let primary = model("anthropic/claude-opus-4");
        let fallbacks = vec![
            model("openai/gpt-5-nano"),
            model("anthropic/claude-haiku-4"),
        ];
        let available: Vec<ModelId> = std::iter::once(primary.clone())
            .chain(fallbacks.iter().cloned())
            .collect();
        let scope = armed_scope(&["anthropic/*"]);

        let unpoliced = build_model_candidates(
            &ModelOverride::Inherit,
            Some(&primary),
            &fallbacks,
            &available,
            None,
        );
        let (policed, violations) = build_model_candidates_scoped(
            &ModelOverride::Inherit,
            Some(&primary),
            &fallbacks,
            &available,
            None,
            Some(&scope),
        );

        assert_eq!(
            policed, unpoliced,
            "enforcement must never rewrite or shorten the ladder"
        );
        assert_eq!(
            violations.len(),
            1,
            "exactly the one out-of-scope fallback: {violations:?}"
        );
        assert_eq!(violations[0].model, "openai/gpt-5-nano");
        assert_eq!(
            violations[0].severity,
            crate::exec::model_scope::ModelScopeSeverity::Warn,
            "an inherited fallback warns; only an EXPLICIT model is a hard error"
        );
    }

    /// Candidate #0 is NOT re-checked here: its scope decision (including the hard-error explicit
    /// case) belongs to `resolve_model_inheritance`, exactly as pi splits the two.
    #[test]
    fn the_primary_candidate_is_not_double_reported_by_the_ladder_check() {
        let primary = model("openai/gpt-5-nano");
        let available = vec![primary.clone()];
        let (candidates, violations) = build_model_candidates_scoped(
            &ModelOverride::Inherit,
            Some(&primary),
            &[],
            &available,
            None,
            Some(&armed_scope(&["anthropic/*"])),
        );
        assert_eq!(candidates, vec![primary]);
        assert!(
            violations.is_empty(),
            "index 0 is the other seam's business: {violations:?}"
        );
    }

    /// The fail-closed half, at the decision boundary: an EXPLICIT out-of-scope model resolves to
    /// no model at all, while an inherited one (persona `model:` / parent session) still resolves.
    #[test]
    fn an_explicit_out_of_scope_model_is_refused_while_an_inherited_one_only_warns() {
        let scope = armed_scope(&["anthropic/*"]);
        let out = model("openai/gpt-5-nano");

        let mut avail = vec![out.clone()];
        let refused = resolve_model_inheritance(Some(&out), None, None, &mut avail, Some(&scope));
        let violation = refused.expect_err("an explicit out-of-scope model must be refused");
        assert_eq!(
            violation.severity,
            crate::exec::model_scope::ModelScopeSeverity::Error
        );
        assert_eq!(
            violation.message,
            "Model 'openai/gpt-5-nano' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/*."
        );

        // Persona-declared model: warn only, resolution proceeds (pi's back-compat allowance).
        let mut avail = vec![out.clone()];
        assert_eq!(
            resolve_model_inheritance(None, Some(&out), None, &mut avail, Some(&scope)),
            Ok(ModelOverride::Inherit),
            "an inherited persona model warns but still runs"
        );

        // Parent-session inheritance: likewise warn-only.
        let mut avail: Vec<ModelId> = Vec::new();
        assert_eq!(
            resolve_model_inheritance(None, None, Some(&out), &mut avail, Some(&scope)),
            Ok(ModelOverride::Explicit(out.clone())),
            "an inherited session model warns but still runs"
        );
        assert_eq!(avail, vec![out]);
    }

    /// An IN-scope explicit model is unaffected — enforcement gates, it does not obstruct.
    #[test]
    fn an_in_scope_explicit_model_passes_the_gate_unchanged() {
        let allowed = model("anthropic/claude-opus-4");
        let mut avail = vec![allowed.clone()];
        assert_eq!(
            resolve_model_inheritance(
                Some(&allowed),
                None,
                None,
                &mut avail,
                Some(&armed_scope(&["anthropic/*"])),
            ),
            Ok(ModelOverride::Explicit(allowed))
        );
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
            // `d8d1408d` / `test/unit/model-fallback.test.ts:203-205` @v0.64.0 (SUBA-089)
            "Connection error",
            "APIConnectionError: Connection closed.",
            "Connection reset by peer",
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
    fn bare_http_status_codes_require_a_word_boundary_and_do_not_false_positive_on_larger_numbers()
    {
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
        for msg in [
            "HTTP 429",
            "(429) Too Many",
            "got 502 from upstream",
            "-> 503",
            "504.",
        ] {
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
            assert!(
                is_retryable_model_failure(Some(msg)),
                "expected retryable: {msg}"
            );
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
            message_errors: Vec::new(),
            startup: StartupEvidence::default(),
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
            message_errors: Vec::new(),
            startup: StartupEvidence::default(),
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
            message_errors: Vec::new(),
            startup: StartupEvidence::default(),
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
            message_errors: Vec::new(),
            startup: StartupEvidence::default(),
        }
    }

    // ---------------------------------------------------------------------------------------
    // G88a: the `TOOL_FAILURE_PREFIX` guard (pi `model-fallback.ts:316-326`)
    // ---------------------------------------------------------------------------------------

    /// The exact strings `exec::output::DetectedSubagentError::message` produces. Their DETAILS
    /// deliberately carry text that `RETRYABLE_MODEL_FAILURE_PATTERNS` independently matches —
    /// `FATAL_BASH_PATTERNS` and the retryable set literally share `"connection refused"` and
    /// `"timeout"` — which is exactly how a failed tool used to be mistaken for a failed model.
    #[test]
    fn a_tool_failure_prefix_is_never_a_retryable_model_failure() {
        for msg in [
            "bash failed (exit 7): curl: (7) Failed to connect to api.test port 443: Connection refused",
            "bash failed (exit 124): timeout: sending signal TERM to command",
            "tool failed with exit code 1",
            "mcp.server/write failed (exit 2): quota exceeded on the remote store",
            "edit_file failed with exit code 3 after a network error",
            "  bash failed (exit 1): fetch failed  ",
        ] {
            assert!(
                !is_retryable_model_failure(Some(msg)),
                "a tool failure must NOT re-run the whole task on another model: {msg}"
            );
        }
    }

    /// The guard is a PREFIX guard, anchored and shaped. Provider errors that merely contain the
    /// word "failed", and tool-shaped text that does not actually match pi's regex, stay
    /// classified by the pattern set alone.
    #[test]
    fn the_tool_failure_guard_does_not_swallow_genuine_provider_failures() {
        for msg in [
            // No `(exit N):` / `with exit code N` clause at all.
            "provider request failed: 503 service unavailable",
            // The clause exists but not at the START of the error.
            "the model was overloaded and then bash failed (exit 1): boom",
            // `(exit N)` without the trailing colon pi requires.
            "bash failed (exit 1) rate limit",
            // A space inside the tool name breaks the leading character class.
            "some tool failed with exit code 1 rate limit",
            // No digits where pi requires `\\d+`.
            "bash failed (exit N): rate limit",
        ] {
            assert!(
                is_retryable_model_failure(Some(msg)),
                "expected still-retryable: {msg}"
            );
        }
    }

    /// Added to the retryable set upstream at v0.43.0 (`model-fallback.ts:303`).
    #[test]
    fn a_truncated_stream_is_retryable() {
        assert!(is_retryable_model_failure(Some(
            "Stream ended without finish_reason"
        )));
    }

    /// The LADDER-level proof, not just the classifier's: a first candidate that fails with a
    /// tool-failure message must stop the ladder outright — no second model, no second full run
    /// of the child's task.
    #[tokio::test]
    async fn a_tool_failure_stops_the_ladder_instead_of_re_running_the_task_on_another_model() {
        let candidates = vec![model("a"), model("b")];
        let mut runner = ScriptedRunner::new(vec![
            (
                failed_signal(
                    "bash failed (exit 7): curl: (7) Failed to connect: Connection refused",
                    usage(10, 0, 0.0),
                ),
                "attempt-a",
            ),
            (ok_signal(usage(999, 999, 999.0)), "attempt-b"), // must never be reached
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(outcome.attempted_models, vec![model("a")]);
        assert_eq!(
            runner.calls.len(),
            1,
            "a failed TOOL must not spend a second model attempt"
        );
        assert_eq!(outcome.aggregate_usage.input, 10);
    }

    // ---------------------------------------------------------------------------------------
    // G88b: two-stage explicit -> inherited resolution
    // (pi `resolveEffectiveSubagentModel`, `model-fallback.ts:222-245`)
    // ---------------------------------------------------------------------------------------

    /// The `subagent` tool / `/run model=…` accepts pi's `"inherit"` sentinel. With no live parent
    /// session model to inherit (headless, SDK embedder, background runner started before a model
    /// was bound), stage 1 resolves to nothing — and stage 2 must fall back to the AGENT's own
    /// `model:` rather than letting the caller's non-request shadow it.
    #[test]
    fn an_inherit_sentinel_with_no_parent_session_falls_back_to_the_agents_own_model() {
        let persona = model("anthropic/claude-sonnet-5");
        let mut available = vec![persona.clone(), ModelId::from(INHERIT_MODEL_SENTINEL)];

        let resolved = resolve_model_inheritance(
            Some(&ModelId::from(INHERIT_MODEL_SENTINEL)),
            Some(&persona),
            None, // no live parent session model
            &mut available,
            None,
        )
        .expect("no scope configured");

        assert_eq!(resolved, ModelOverride::Explicit(persona.clone()));
        assert!(
            !available.contains(&ModelId::from(INHERIT_MODEL_SENTINEL)),
            "the sentinel is a request, never an allowlisted model"
        );
        assert_eq!(
            build_model_candidates(&resolved, Some(&persona), &[], &available, None),
            vec![persona],
            "the child is spawned with `--model <the agent's model>`, never `--model inherit`"
        );
    }

    /// With a live parent session model, the same sentinel resolves to the PARENT model, which
    /// outranks the agent's own — that is what "inherit" asks for (pi `:207`, stage 1).
    #[test]
    fn an_inherit_sentinel_prefers_the_parent_session_model_over_the_agents_own() {
        let persona = model("anthropic/claude-sonnet-5");
        let parent = model("together/zai-org/GLM-5.2");
        let mut available = vec![persona.clone()];

        let resolved = resolve_model_inheritance(
            Some(&ModelId::from("  inherit  ")),
            Some(&persona),
            Some(&parent),
            &mut available,
            None,
        )
        .expect("no scope configured");

        assert_eq!(resolved, ModelOverride::Explicit(parent.clone()));
        assert_eq!(
            build_model_candidates(&resolved, Some(&persona), &[], &available, None),
            vec![parent, persona],
            "the parent model leads; the agent's own model stays as the next rung"
        );
    }

    /// A blank `model` is pi's `trimmed &&` falsy case — identical to the sentinel, and identical
    /// to omitting the parameter.
    #[test]
    fn a_blank_per_call_model_is_treated_as_no_request_at_all() {
        let persona = model("anthropic/claude-sonnet-5");
        let mut available = vec![persona.clone(), ModelId::from("   ")];

        let resolved = resolve_model_inheritance(
            Some(&ModelId::from("   ")),
            Some(&persona),
            None,
            &mut available,
            None,
        )
        .expect("no scope configured");

        assert_eq!(resolved, ModelOverride::Explicit(persona.clone()));
        assert_eq!(
            build_model_candidates(&resolved, Some(&persona), &[], &available, None),
            vec![persona]
        );
    }

    /// A PERSONA whose frontmatter says `model: inherit` must not reach the ladder either — the
    /// ladder is rebuilt from `agent_primary_model` independently of this function's return value,
    /// so the sentinel has to be purged from the allowlist for the filter to drop it.
    #[test]
    fn a_persona_model_of_inherit_never_reaches_the_child_argv() {
        let sentinel = ModelId::from(INHERIT_MODEL_SENTINEL);
        let parent = model("together/zai-org/GLM-5.2");
        let fallback = model("openai/gpt-5.4-mini");
        let mut available = vec![sentinel.clone(), fallback.clone()];

        let resolved = resolve_model_inheritance(
            None, // no per-call override
            Some(&sentinel),
            Some(&parent),
            &mut available,
            None,
        )
        .expect("no scope configured");

        assert_eq!(resolved, ModelOverride::Explicit(parent.clone()));
        let ladder = build_model_candidates(
            &resolved,
            Some(&sentinel),
            std::slice::from_ref(&fallback),
            &available,
            None,
        );
        assert_eq!(ladder, vec![parent, fallback]);
        assert!(
            !ladder.contains(&sentinel),
            "`inherit` is not a model id and must never be spawned"
        );
    }

    /// Stage 1's source rule (pi `:212`): a request that reduced to "the parent model" is always
    /// `"inherited"`, so an armed `modelScope` WARNS instead of refusing the run — even though the
    /// caller did pass a `model` parameter.
    #[test]
    fn an_inherit_sentinel_under_an_armed_scope_warns_rather_than_refusing() {
        let scope = ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(vec!["anthropic/*".to_string()]),
        };
        let parent = model("together/zai-org/GLM-5.2"); // outside the allow list
        let mut available = Vec::new();

        let resolved = resolve_model_inheritance(
            Some(&ModelId::from(INHERIT_MODEL_SENTINEL)),
            None,
            Some(&parent),
            &mut available,
            Some(&scope),
        )
        .expect("an inherited model only ever warns");
        assert_eq!(resolved, ModelOverride::Explicit(parent));
    }

    /// The pre-existing contract this two-stage shape must not disturb: a REAL explicit model is
    /// still candidate #0 and is still refused outright when an armed scope excludes it.
    #[test]
    fn a_real_explicit_model_is_unchanged_by_the_two_stage_shape() {
        let explicit = model("openai/gpt-5.4");
        let persona = model("anthropic/claude-sonnet-5");
        let mut available = vec![explicit.clone(), persona.clone()];
        assert_eq!(
            resolve_model_inheritance(
                Some(&explicit),
                Some(&persona),
                Some(&model("together/x")),
                &mut available,
                None,
            )
            .expect("no scope configured"),
            ModelOverride::Explicit(explicit.clone())
        );

        let scope = ModelScopeConfig {
            enforce: Some(true),
            strict: None,
            allow: Some(vec!["anthropic/*".to_string()]),
        };
        assert!(
            resolve_model_inheritance(
                Some(&explicit),
                Some(&persona),
                None,
                &mut available,
                Some(&scope),
            )
            .is_err(),
            "an out-of-scope EXPLICIT model still fails closed"
        );
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

    // ---------------------------------------------------------------------------------------
    // is_retryable_subagent_startup_failure (G74) — the boundary, term by term
    // ---------------------------------------------------------------------------------------

    /// The canonical "the child never started" shape: a bare non-zero exit, cyrup's placeholder
    /// error, nothing else.
    fn startup_failure_signal() -> AttemptSignal {
        AttemptSignal {
            success: false,
            exit_code: Some(1),
            error: Some("subagent attempt exited with code 1".to_string()),
            usage: Usage::default(),
            timed_out: false,
            detached: false,
            message_errors: Vec::new(),
            startup: StartupEvidence {
                duration_ms: Some(40),
                error_is_placeholder: true,
                ..StartupEvidence::default()
            },
        }
    }

    #[test]
    fn a_bare_zero_activity_non_zero_exit_is_a_startup_failure() {
        assert!(is_retryable_subagent_startup_failure(
            &startup_failure_signal()
        ));
    }

    /// Every field is a REASON NOT TO RETRY: each of these mutations alone must disqualify it.
    /// This is the boundary the whole feature turns on — too loose and a legitimately failing model
    /// is launched four times over.
    #[test]
    fn any_single_piece_of_evidence_disqualifies_the_startup_retry() {
        type Mutate = Box<dyn Fn(&mut AttemptSignal)>;
        let cases: Vec<(&str, Mutate)> = vec![
            (
                "a clean exit",
                Box::new(|s: &mut AttemptSignal| s.exit_code = Some(0)),
            ),
            (
                "no exit code at all",
                Box::new(|s: &mut AttemptSignal| s.exit_code = None),
            ),
            (
                "a real diagnostic",
                Box::new(|s: &mut AttemptSignal| {
                    s.error = Some("provider returned 500".to_string());
                    s.startup.error_is_placeholder = false;
                }),
            ),
            (
                "produced output",
                Box::new(|s: &mut AttemptSignal| s.startup.final_output_present = true),
            ),
            (
                "emitted a message",
                Box::new(|s: &mut AttemptSignal| s.startup.message_count = 1),
            ),
            (
                "ran a tool",
                Box::new(|s: &mut AttemptSignal| s.startup.tool_count = 1),
            ),
            (
                "burned tokens",
                Box::new(|s: &mut AttemptSignal| s.usage.input = 1),
            ),
            (
                "an unknown duration",
                Box::new(|s: &mut AttemptSignal| s.startup.duration_ms = None),
            ),
            (
                "a duration past the startup window",
                Box::new(|s: &mut AttemptSignal| {
                    s.startup.duration_ms = Some(MAX_SUBAGENT_STARTUP_FAILURE_DURATION_MS + 1);
                }),
            ),
            (
                "a protocol violation",
                Box::new(|s: &mut AttemptSignal| s.startup.protocol_error = true),
            ),
            (
                "a signal other than SIGKILL",
                Box::new(|s: &mut AttemptSignal| {
                    s.startup.process_signal = Some("SIGSEGV".to_string())
                }),
            ),
            (
                "a mutation attempt",
                Box::new(|s: &mut AttemptSignal| s.startup.observed_mutation_attempt = true),
            ),
            (
                "a detach",
                Box::new(|s: &mut AttemptSignal| s.detached = true),
            ),
            (
                "a timeout",
                Box::new(|s: &mut AttemptSignal| s.timed_out = true),
            ),
            (
                "an explicit stop",
                Box::new(|s: &mut AttemptSignal| s.startup.stopped = true),
            ),
            (
                "an exhausted turn budget",
                Box::new(|s: &mut AttemptSignal| s.startup.turn_budget_exceeded = true),
            ),
        ];
        for (label, mutate) in cases {
            let mut signal = startup_failure_signal();
            mutate(&mut signal);
            assert!(
                !is_retryable_subagent_startup_failure(&signal),
                "{label} must disqualify the startup retry"
            );
        }
    }

    /// SIGKILL is the ONE tolerated signal (`subagent-startup-retry.ts:52,60`): it is what this
    /// crate's own escalation ladder ends with, so it is not evidence the child misbehaved. Both
    /// spellings upstream tolerates — the signal name and the error text it formats to — must pass.
    #[test]
    fn sigkill_alone_does_not_disqualify_the_startup_retry() {
        let mut signal = startup_failure_signal();
        signal.startup.process_signal = Some("SIGKILL".to_string());
        signal.startup.error_is_placeholder = false;
        signal.error = Some(format_process_signal_error("SIGKILL"));
        assert!(is_retryable_subagent_startup_failure(&signal));
    }

    /// The retry budget is the delay table plus the first launch — 4 launches, never 3 or 5.
    #[test]
    fn the_startup_retry_budget_matches_upstreams_delay_table() {
        assert_eq!(SUBAGENT_STARTUP_RETRY_DELAYS_MS, [250, 750, 1500]);
        let note = format_subagent_startup_retry_note(
            "m1",
            1,
            SUBAGENT_STARTUP_RETRY_DELAYS_MS.len() + 1,
            250,
        );
        assert_eq!(
            note,
            "[startup-retry] m1 exited before model or tool activity (attempt 1/4). Retrying the \
             same model in 250ms."
        );
    }

    /// SUBA-089 — `/connection\s+(?:error|reset|closed|aborted)/i` (`d8d1408d`): `\s+` needs at
    /// least one whitespace character, any amount is fine, and the alternative must follow the
    /// run directly; `connection refused` stays its own separate pattern.
    #[test]
    fn a_dropped_provider_connection_is_retryable_but_only_across_real_whitespace() {
        for msg in [
            "connection   aborted",
            "Provider: CONNECTION\tCLOSED unexpectedly",
            "first connection ok, second connection error",
        ] {
            assert!(is_retryable_model_failure(Some(msg)), "{msg}");
        }
        for msg in [
            "connectionreset",
            "connection was reset",
            "the connection is fine",
        ] {
            assert!(!is_retryable_model_failure(Some(msg)), "{msg}");
        }
    }

    // ---------------------------------------------------------------------------------------
    // SUBA-089: is_retryable_model_failure_attempt (pi `isRetryableModelFailureAttempt`,
    // `model-fallback.ts:530-537` @v0.64.0) — the per-attempt gate the ladder consults
    // ---------------------------------------------------------------------------------------

    /// A failed attempt with explicit activity evidence — the four inputs the upstream predicate
    /// reads (`{error, messages, toolCount}`; `messages.length` and each `errorMessage`).
    fn attempt_signal(
        error: &str,
        message_count: usize,
        tool_count: u32,
        message_errors: &[&str],
    ) -> AttemptSignal {
        let mut signal = failed_signal(error, Usage::default());
        signal.startup.message_count = message_count;
        signal.startup.tool_count = tool_count;
        signal.message_errors = message_errors.iter().map(|s| (*s).to_string()).collect();
        signal
    }

    /// Upstream's own four cases, verbatim (`test/unit/model-fallback.test.ts:342-345` @v0.64.0,
    /// "does not retry raw process stderr after child activity").
    #[test]
    fn attempt_predicate_matches_upstreams_stderr_after_activity_cases() {
        let error = "APIConnectionError: Connection closed.";
        // :342 — a message ran but none reported this error: raw stderr after activity, stop.
        assert!(!is_retryable_model_failure_attempt(&attempt_signal(
            error,
            1,
            0,
            &[]
        )));
        // :343 — the transcript corroborates the error: advance.
        assert!(is_retryable_model_failure_attempt(&attempt_signal(
            error,
            1,
            0,
            &[error]
        )));
        // :344 — nothing ran at all: advance.
        assert!(is_retryable_model_failure_attempt(&attempt_signal(
            error,
            0,
            0,
            &[]
        )));
        // :345 — corroborated, but a tool already ran: stop.
        assert!(!is_retryable_model_failure_attempt(&attempt_signal(
            error,
            1,
            1,
            &[error]
        )));
    }

    /// `:532` — `toolCount > 0` refuses BEFORE either sentinel or the no-activity clause can say
    /// yes: even the cold-start sentinel does not re-dispatch a child that ran a tool.
    #[test]
    fn attempt_predicate_never_advances_once_a_tool_ran() {
        for error in [
            crate::exec::output::EMPTY_OUTPUT_ERROR,
            "Subagent produced no output after terminal assistant stopReason \"length\".",
            "429 rate limit",
            "connection reset by peer",
        ] {
            assert!(
                !is_retryable_model_failure_attempt(&attempt_signal(error, 0, 3, &[error])),
                "{error}"
            );
        }
    }

    /// `:533` — both empty-output sentinels advance even when the transcript holds messages
    /// that never reported them (the v0.64.0 form is the second regex; the v0.57.0 gate knew
    /// only the first literal).
    #[test]
    fn attempt_predicate_empty_output_sentinels_advance_despite_messages() {
        assert!(is_retryable_model_failure_attempt(&attempt_signal(
            crate::exec::output::EMPTY_OUTPUT_ERROR,
            2,
            0,
            &[]
        )));
        assert!(is_retryable_model_failure_attempt(&attempt_signal(
            "Subagent produced no output after terminal assistant stopReason \"length\".",
            2,
            0,
            &[]
        )));
        // The regex is anchored and exact: no trailing period, an empty reason, or a quote
        // inside the reason is not the sentinel — and with messages present and no
        // corroborating `errorMessage`, such a string stops the ladder.
        for not_sentinel in [
            "Subagent produced no output after terminal assistant stopReason \"length\"",
            "Subagent produced no output after terminal assistant stopReason \"\".",
            "Subagent produced no output after terminal assistant stopReason \"a\"b\".",
            " Subagent produced no output (possible model cold-start or empty response).",
        ] {
            assert!(
                !is_retryable_model_failure_attempt(&attempt_signal(not_sentinel, 2, 0, &[])),
                "{not_sentinel}"
            );
        }
    }

    /// `:535-536` — the correlation clause trims both sides and ignores role/order; a different
    /// `errorMessage` does not corroborate.
    #[test]
    fn attempt_predicate_correlates_a_trimmed_message_error_message() {
        assert!(is_retryable_model_failure_attempt(&attempt_signal(
            "  overloaded  ",
            3,
            0,
            &["unrelated", "overloaded\n"]
        )));
        assert!(!is_retryable_model_failure_attempt(&attempt_signal(
            "overloaded",
            3,
            0,
            &["service unavailable"]
        )));
    }

    /// `:531` — a non-retryable text is refused before any activity evidence is read, and an
    /// absent error is never retryable (pi `isRetryableModelFailure(undefined) === false`).
    #[test]
    fn attempt_predicate_still_requires_a_retryable_text() {
        assert!(!is_retryable_model_failure_attempt(&attempt_signal(
            "assertion failed: expected 2, got 3",
            0,
            0,
            &[]
        )));
        let mut absent = failed_signal("x", Usage::default());
        absent.error = None;
        assert!(!is_retryable_model_failure_attempt(&absent));
    }

    /// The behaviour gap SUBA-089 filed: a child that ran ten mutating tool calls and then hit a
    /// transient provider error must NOT be re-run from scratch on the next model. RED before the
    /// port — the ladder gated on the bare text classifier and advanced to `b`.
    #[tokio::test]
    async fn retryable_error_after_tools_ran_does_not_advance_the_ladder() {
        let candidates = vec![model("a"), model("b")];
        let mut failed = failed_signal("connection reset by peer", usage(10, 0, 0.0));
        failed.startup.tool_count = 10;
        failed.startup.message_count = 12;
        failed.message_errors = vec!["connection reset by peer".to_string()];
        let mut runner = ScriptedRunner::new(vec![
            (failed, "attempt-a"),
            (ok_signal(usage(1, 1, 0.0)), "attempt-b"), // must never be reached
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;

        assert_eq!(outcome.attempted_models, vec![model("a")]);
        assert_eq!(
            runner.calls.len(),
            1,
            "a half-completed mutating run must not be re-dispatched on the next model"
        );
        let last = outcome.last_signal.expect("some outcome");
        assert!(!last.success);
        assert_eq!(last.error.as_deref(), Some("connection reset by peer"));
    }

    /// The other side of the same gate: a retryable-looking error the child's transcript never
    /// reported (raw stderr after an assistant turn) also stops the ladder, while the corroborated
    /// form still advances — so the port is the narrowed v0.64.0 gate, not a blanket refusal.
    #[tokio::test]
    async fn uncorroborated_retryable_text_after_messages_stops_but_corroborated_advances() {
        let candidates = vec![model("a"), model("b")];

        let mut uncorroborated =
            failed_signal("APIConnectionError: Connection closed.", usage(1, 0, 0.0));
        uncorroborated.startup.message_count = 1;
        let mut runner = ScriptedRunner::new(vec![
            (uncorroborated, "attempt-a"),
            (ok_signal(usage(1, 1, 0.0)), "attempt-b"),
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;
        assert_eq!(outcome.attempted_models, vec![model("a")]);
        assert_eq!(runner.calls.len(), 1);

        let mut corroborated =
            failed_signal("APIConnectionError: Connection closed.", usage(1, 0, 0.0));
        corroborated.startup.message_count = 1;
        corroborated.message_errors = vec!["APIConnectionError: Connection closed.".to_string()];
        let mut runner = ScriptedRunner::new(vec![
            (corroborated, "attempt-a"),
            (ok_signal(usage(1, 1, 0.0)), "attempt-b"),
        ]);
        let outcome = run_fallback_ladder(&candidates, &mut runner).await;
        assert_eq!(outcome.attempted_models, vec![model("a"), model("b")]);
        assert_eq!(runner.calls.len(), 2);
        assert!(outcome.last_signal.expect("some outcome").success);
    }
}
