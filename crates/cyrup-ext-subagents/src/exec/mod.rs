//! Foreground single-run execution: prompt/argv construction, NDJSON consumption, final-output
//! extraction, acceptance-gate evaluation, completion-mutation guard, model-fallback retry
//! ladder (func-SA §5.2; arch-SA §6.3).
//!
//! This is the integration module: it owns `run_sync`/`RunOptions`/`AgentConfig`/`SingleResult`
//! (arch-SA §3.4) and `plan_batch` (arch-SA §6.6's eager whole-batch fork-context resolution),
//! wiring together every sibling module in this subtree:
//!
//! - [`ndjson`] — the `SubagentEvent` tagged union and `consume_stdout`, the sole NDJSON parser
//!   this module folds progress/usage state from (R-SA-026/057/058).
//! - [`output`] — final-output extraction, file-only output-path handoff, UTF-8-safe truncation
//!   (R-SA-024/025/029/031/042).
//! - [`structured`] — structured-output extraction from the child's event stream + parent-side
//!   JSON-Schema re-validation via the `jsonschema` crate (R-SA-030).
//! - [`completion_guard`] — implementation-expecting classification + mutating-tool-call scan
//!   (R-SA-034).
//! - [`fallback`] — the model-fallback ladder-construction/retry-classification/usage-aggregation
//!   algorithms (R-SA-035..041/044); this module supplies the `AttemptRunner` implementation that
//!   actually spawns a real child OS process per attempt.
//! - [`acceptance`] — the acceptance-provenance ledger: contract injection, gate evaluation, and
//!   REAL `verify[]` subprocess execution (R-SA-023/030/032/033).
//! - [`crate::fork_context::ForkContextResolver`] — [`plan_batch`] resolves every batch step's
//!   fork-context up front, before any child process for that batch is spawned (R-SA-137, arch-SA
//!   §6.6's eager-whole-batch-validation rule).
//!
//! # The mandated mechanism, concretely, in this file
//!
//! [`run_sync`]'s per-attempt driver ([`crate::exec::attempt_runner::SpawnedChildAttemptRunner`]) spawns a REAL OS subprocess
//! for every model-fallback attempt via [`crate::spawn::SpawnedChild::spawn`] — never an
//! in-process nested agent turn loop, never an in-process event-relay standing in for the child's
//! own execution (func-SA §1.1). Cancellation is threaded as two independent
//! `cyrup_core::CancelToken`s (`RunOptions.cancel` for hard abort, `RunOptions.interrupt` for a
//! soft, per-run interrupt) raced via `tokio::select!` against
//! [`crate::spawn::SpawnedChild::terminate`]'s real SIGINT->SIGTERM->SIGKILL escalation ladder —
//! this module never invents a second, competing cancellation mechanism.

pub mod acceptance;
pub mod agent_refinements;
pub mod child_protocol;
pub mod completion_guard;
pub mod control;
pub mod mcp_direct_tools;
pub mod fallback;
pub mod model_scope;
pub mod ndjson;
pub mod output;
pub mod structured;
pub mod task_intent;
pub mod tool_call_summary;
pub mod tool_budget;
pub mod turn_budget;
pub mod capability_ceiling;
/// SUBA-078 — the `subagents.maxThinking` reasoning-level ceiling.
pub mod thinking_ceiling;
pub mod permissions;
pub mod usage_budget;
pub mod spawn_budget;
pub mod tool_availability;

/// The static, execution-ready "what to run and how" input surface: [`AgentConfig`],
/// [`ResolvedAgentPersona`], [`resolve_step_agent_config`], [`RunOptions`], [`LiveEventSink`]
/// (arch-SA §3.4's input-contract half; [`run_result`] is the output-contract half).
pub mod agent_config;

/// The concrete production [`fallback::AttemptRunner`] implementor — spawns one real OS child
/// process per model-fallback attempt (the process-spawning third of the former "SubagentSpawner"
/// section; [`spawn_plan`] and [`drive_attempt`] are the other two).
pub mod attempt_runner;

/// Given one already-spawned child's stdout NDJSON event stream, fold it into
/// progress/output/acceptance state until the attempt settles (the per-attempt drive-loop third of
/// the former "SubagentSpawner" section).
pub mod drive_attempt;

/// The live per-attempt progress fold (R-SA-027/028): [`AgentProgress`], [`ProgressSnapshotInput`].
pub mod progress;

/// The output contract of one [`run_sync`] call: [`SingleResult`] (arch-SA §3.4's output-contract
/// half; [`agent_config`] is the input-contract half).
pub mod run_result;

/// Pure computation of *what to spawn* for one model-fallback attempt: argv/env/system-prompt
/// assembly, zero process handles, zero I/O (the spawn-plan-construction third of the former
/// "SubagentSpawner" section).
pub mod spawn_plan;

/// Fixtures shared by more than one `exec` submodule's tests (helper constructors only —
/// matches this crate's own `acceptance/model/testsupport.rs` / `acceptance/lattice/testsupport.rs`
/// convention).
#[cfg(test)]
pub(crate) mod testsupport;

// Re-exported at `exec::` root (below) so the public API this module presented before the split —
// `exec::AgentConfig`, `exec::RunOptions`, `exec::SingleResult`, `exec::AgentProgress`,
// `exec::build_attempt_spawn_plan`, `exec::ToolCallSummary`, etc. — is unchanged for every existing
// caller (in-crate and, per `cyrup-it`'s integration tests, cross-crate).
pub use agent_config::*;
use attempt_runner::{AttemptRecord, SpawnedChildAttemptRunner};
pub use progress::*;
pub use run_result::*;
pub use spawn_plan::*;

use std::path::PathBuf;

use cyrup_core::{ModelId, Usage};

use crate::discovery::types::{
    AgentDefinition, OutputMode,
};
use crate::error::SubagentError;
use crate::exec::acceptance::{
    AcceptanceContract, CleanCompletionGate, apply_post_hoc_correction,
    build_timed_out_acceptance_ledger,
};
use crate::exec::completion_guard::{CompletionMutationGuardResult, evaluate_completion_mutation_guard};
use crate::exec::fallback::{AttemptSignal, run_fallback_ladder};
use crate::exec::output::{
    resolve_output_handoff, snapshot_output_file, truncate_output, validate_file_only_requires_path,
};
use crate::exec::structured::StructuredOutcome;
use crate::fork_context::{ContextMode, ForkContext, ForkContextResolver};

/// R-SA-028 (MUST) — bounded recent-output buffer cap: `recent_output` in a live progress
/// snapshot MUST be capped at 50 lines (oldest evicted first) while the run is active. Identical to
/// pi's own `if (progress.recentOutput.length > 50) splice(...)` window
/// (`runs/foreground/execution.ts:115-120`).
pub const RECENT_OUTPUT_CAP: usize = 50;

/// How many trailing lines of ONE chunk of child text enter [`AgentProgress::recent_output`] —
/// pi's `.split("\n").slice(-10)` at both append sites (`runs/foreground/execution.ts:651,670`
/// @v0.34.0). A single enormous assistant turn therefore contributes at most ten
/// lines to the ring, before [`RECENT_OUTPUT_CAP`] even applies.
pub const RECENT_OUTPUT_TAIL_LINES: usize = 10;

/// Hard per-line character cap applied as each line enters [`AgentProgress::recent_output`] — pi's
/// `MAX_STREAMED_OUTPUT_LINE_CHARS` (`pi-subagents/src/shared/utils.ts:442`, applied by
/// `boundStreamedRecentOutput` at `:450-456`), whose own doc comment is *"Cap per-line length of
/// recent output so one long line can't inflate a snapshot."*
///
/// **Version note**: this constant does NOT exist at the ported v0.34.0 baseline — it arrived
/// upstream with `boundStreamedRecentTools`/`MAX_STREAMED_RECENT_TOOLS`, which
/// [`crate::tui::events::RECENT_TOOLS_CAP`] already adopts for the same reason. Adopting the
/// sibling bound keeps the two halves of one upstream guard from being half-ported.
///
/// **[CYRUP-DELTA] ×2.**
/// 1. pi applies the bound only when SNAPSHOTTING for the streamed wire (`snapshotProgress`,
///    `execution.ts:230-237`), leaving the live array's lines unbounded in length. This fold
///    truncates at append time instead — the identical bounded lines on every snapshot, with an
///    in-memory ring that is O(1) in line width too. That closes the one growth term a
///    settled-but-`running` snapshot (pi's interrupt-paused shape, which `compactCompletedProgress`
///    deliberately refuses to compact) would otherwise still carry: 50 lines × unbounded width.
/// 2. pi's `line.slice(0, N)` counts UTF-16 code units; this counts `char`s, because a byte slice
///    at an arbitrary offset can split a UTF-8 sequence (and the crate denies `indexing_slicing`).
///    The suffix `… [truncated]` is pi's, verbatim.
pub const RECENT_OUTPUT_LINE_CHARS: usize = 2000;

/// pi `boundStreamedRecentOutput`'s per-line arm (`shared/utils.ts:450-456`), applied at append
/// time per [`RECENT_OUTPUT_LINE_CHARS`]'s delta note: a line longer than the cap becomes its first
/// `RECENT_OUTPUT_LINE_CHARS` `char`s followed by pi's verbatim `… [truncated]` suffix; anything
/// within the cap is returned unchanged.
#[must_use]
/// `serde(skip_serializing_if)` predicate for an optional-on-the-wire `bool` that upstream declares
/// as `foo?: boolean` and only ever writes when true (e.g. [`SingleResult::stopped`]). A free
/// function because `skip_serializing_if` is handed a `&bool`, which `std::ops::Not::not` (taking
/// `self: bool`) cannot accept.
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

pub(crate) fn bound_output_line(line: &str) -> String {
    // `chars().count()` rather than `len()`: the cap is a CHARACTER cap (pi's UTF-16-code-unit
    // `slice`), and a byte length would truncate multi-byte text far too eagerly.
    if line.chars().count() <= RECENT_OUTPUT_LINE_CHARS {
        return line.to_string();
    }
    let mut out: String = line.chars().take(RECENT_OUTPUT_LINE_CHARS).collect();
    out.push_str("… [truncated]");
    out
}

/// The exact message a timed-out run leads its delivered output with, and the text of the timeout
/// error — a 1:1 port of pi's `formatTimeoutMessage` (`execution.ts:169-171`). `ms` is the NOMINAL
/// timeout budget ([`RunOptions::timeout_ms`], pi `options.timeoutMs ?? 0`), not the elapsed time.
#[must_use]
pub fn format_timeout_message(ms: u64) -> String {
    format!("Subagent timed out after {ms}ms.")
}

/// pi `DEFAULT_FOREGROUND_TIMEOUT_MS` (`runs/foreground/subagent-executor.ts:2656` @v0.57.0) — the
/// wall-clock backstop every FOREGROUND launch gets when neither the caller, the agent's own
/// `timeoutMs:` frontmatter, nor `subagents.timeoutMs` set one. Without it a child whose bash tool
/// blocks forever hangs the orchestrator's turn open-endedly with no signal at all.
///
/// Deliberately a SEPARATE constant from [`crate::background::DEFAULT_ASYNC_CHILD_TIMEOUT_MS`]
/// rather than an alias of it. The two coincide today — upstream calls the async one "same generous
/// default as foreground" — but upstream keeps them independent, and the two paths apply them at
/// different seams: this one at the tool/slash dispatch, the async one inside
/// `extension/executor/background.rs`'s own `unwrap_or`.
pub const DEFAULT_FOREGROUND_TIMEOUT_MS: u64 = 30 * 60 * 1000;

// ================================================================================================
// run_sync: the model-fallback attempt loop, wired end to end (arch-SA §6.3.2)
// ================================================================================================

/// [`run_sync`]'s step 2 (R-SA-023): the effective acceptance contract for this run.
///
/// A named function rather than an inline expression because it is the SINGLE seam at which an
/// explicit, caller-supplied contract meets the heuristically-inferred one, and the rule joining
/// them is upstream's, not this crate's: pi `resolveEffectiveAcceptance` takes
/// `max(explicitLevel, inferred.level)` by rank (`runs/shared/acceptance.ts:277-281` @v0.34.0),
/// so an explicit level may only RAISE the inferred floor. This seam used to read
/// `opts.acceptance.clone().unwrap_or_else(|| AcceptanceContract::heuristic_default(...))` —
/// explicit and inferred were mutually exclusive, so `acceptance: "attested"` on a write-capable
/// task ran a weaker gate than the same policy does under pi, silently. The combination rule
/// itself lives on [`AcceptanceContract::resolve_effective`]; this function only supplies
/// `run_sync`'s inputs to it.
///
/// SUBA-082 — two of those inputs now come off the agent itself:
///
/// * `agent.acceptance_role` is threaded into the inferred half exactly as pi passes
///   `acceptanceRole: agent.acceptanceRole` to `resolveEffectiveAcceptance`
///   (`runs/foreground/execution.ts:1834` @v0.64.0), so a declared `read-only`/`writer` replaces
///   the agent-NAME guess on every dispatch path that reaches `run_sync` (single, chain/parallel
///   step, background hop-2 — the persona carries it across the process boundary).
/// * `agent.default_acceptance` — the agent file's `acceptance:` launch default — is NOT applied
///   here. pi applies it in `applySingleAgentLaunchDefaults` (`subagent-executor.ts:2690-2692`
///   @v0.64.0), which bails for any `chain`/`tasks` launch; `run_sync` is this crate's shared
///   chokepoint for chain/parallel steps too, so the default is folded into the single-agent
///   call's own params upstream of here (`extension/tool/routing.rs::route_single`), where it
///   reaches `opts.acceptance` through the ordinary explicit-policy lowering. Applying it at this
///   seam would leak an agent-level default into chain/parallel steps, which pi keeps as
///   task/step configuration (`docs/agents.md:326` @v0.64.0).
fn resolve_run_acceptance(
    opts: &RunOptions,
    agent: &AgentConfig,
    task: &str,
) -> AcceptanceContract {
    AcceptanceContract::resolve_effective_for_role(
        opts.acceptance.clone(),
        &agent.name,
        agent.acceptance_role,
        task,
    )
}

/// Run one subagent task to completion, synchronously, against `agent`/`opts` (func-SA §5.2;
/// arch-SA §6.3.2).
///
/// # Pipeline (strict order, per R-SA-033's own ordering restated at the top level)
///
/// 0. R-SA-055 (SAFETY-CRITICAL): the recursion-depth guard ([`crate::spawn::depth::is_blocked`]
///    against `agent.depth`) runs FIRST, before anything else in this function — including
///    R-SA-025's own output-mode validation immediately below. `run_sync` is the sole real spawn
///    chokepoint in this crate (every production caller — the foreground tool dispatch, the
///    background runner's step loop, and every chain/parallel/dynamic fan-out child reached via
///    `chain_graph::walk_chain`/`spawn::parallel::run_bounded`'s `SingleStepExecutor` seam —
///    funnels through this one function before ever touching `SpawnedChild::spawn`), so gating
///    here is what makes the depth ceiling actually bind at runtime rather than merely existing as
///    a unit-tested-in-isolation predicate. A blocked attempt returns
///    [`SubagentError::DepthExceeded`]'s message as `SingleResult::error` with `exit_code: 1` and
///    spawns nothing.
/// 1. R-SA-025: file-only output mode requires an output path — fail fast, before any subprocess
///    is spawned, if violated.
/// 2. Resolve the effective acceptance contract — `max(explicit opts.acceptance,
///    [`AcceptanceContract::heuristic_default`])` via [`resolve_run_acceptance`], R-SA-023.
/// 3. R-SA-038: build the model-fallback candidate ladder.
/// 4. Drive [`fallback::run_fallback_ladder`] against a [`crate::exec::attempt_runner::SpawnedChildAttemptRunner`] — every
///    candidate model gets a FRESH real child OS process (R-SA-039); R-SA-036 (timeout)/R-SA-037
///    (detach) both terminate the ladder outright without advancing, exactly as
///    `run_fallback_ladder` itself already enforces (this module supplies the signal, not the
///    ladder-control logic, which stays [`fallback`]'s sole responsibility).
/// 5. R-SA-030: structured-output CAPTURE-FILE read-back + parent-side JSON-Schema re-validation,
///    via [`structured::read_structured_output`] and [`structured::validate_structured_output`]
///    (arch-SA §12 item 13's resolved crate choice, `jsonschema`). Only evaluated when the run is
///    otherwise clean (exit 0, not detached/interrupted/timed-out) — mirrors R-SA-032/033's own
///    "don't re-diagnose an already-failed attempt" gate. If `opts.structured_output_schema` is
///    `None`, this step is a no-op (`SingleResult::structured_output` stays `None`). If a schema IS
///    declared: a captured value that validates populates `SingleResult::structured_output`; a
///    captured value that fails validation, or no captured value at all, forces `exit_code = 1`
///    with an error message — never silently downgraded, per R-SA-030's "MUST also fail the run"
///    text, and never satisfied by prose, per pi's "EVEN WHEN prose was produced" rule.
/// 6. R-SA-034: completion-mutation guard, via [`completion_guard::evaluate_completion_mutation_guard`].
/// 7. R-SA-032: acceptance-gate evaluation, gated on `exit_code == 0 && !detached && !interrupted
///    && !timed_out` (R-SA-033's own gate condition), via [`acceptance::evaluate_acceptance`].
/// 8. R-SA-033: post-hoc exit-code correction, via [`acceptance::apply_post_hoc_correction`].
/// 9. R-SA-042: UTF-8-safe output truncation, via [`output::truncate_output`].
/// 10. R-SA-043: result compaction — `SingleResult` itself IS the compacted shape (no raw
///     per-turn messages, no live `progress` object); `SingleResult::tool_calls` carries only the
///     summarized tool-name list.
///
/// R-SA-037 (intercom detach bypasses acceptance/completion-guard/truncation entirely) is WIRED
/// end-to-end within this crate: [`crate::exec::drive_attempt::drive_attempt`]'s NDJSON loop sets its `detached` observation
/// the moment a child emits a blocking `contact_supervisor` ask (`contact_supervisor_block_prompt`)
/// and fires [`crate::tui::intercom::spawn_clarify`] against the executor's single-slot
/// [`crate::tui::intercom::AskLock`] (backed in production by the intercom companion's real broker
/// `ClarifyChannel`, threaded via `SubagentsExtension::with_channels` → `RunOptions::clarify`);
/// [`crate::exec::fallback::AttemptRunner::run_attempt`] then carries that observation onto
/// `AttemptSignal::detached`, which this function reads (via the `detached` binding below) to skip
/// acceptance/completion-guard/truncation. See [`crate::exec::fallback::AttemptSignal::detached`]'s
/// doc comment for the full CLOSED wiring. When no clarify channel is wired (headless / SDK-embedder
/// / `RunOptions::clarify = None`), the drive loop still marks the attempt detached but the `AskLock`
/// degrades to its no-live-channel fallback rather than blocking.
pub async fn run_sync(agent: &AgentConfig, task: &str, opts: &RunOptions) -> SingleResult {
    if let Some(failure) = depth_guard_failure(agent, task) {
        return failure;
    }

    // Step 0b (SUBA-074) — REFUSE a runner this crate cannot honour, before the model-fallback
    // ladder rather than inside it. `build_attempt_spawn_plan`'s errors are per-ATTEMPT
    // (`exec/attempt_runner.rs:351` hands an `Err` to `attempt_setup_failure`, `:534-556`, which
    // yields `AttemptSignal { success: false, … }` and the ladder tries the next model), so a
    // runner refusal raised there would fire once per candidate model and end in a misleading
    // "all models failed". This is a property of the RUN, so it belongs here beside the depth
    // guard, which has exactly the same shape.
    //
    // Without this, an agent declaring an external profile — which upstream FORBIDS from declaring
    // `tools:`, so it declares none, which this crate reads as "no allowlist restriction" — spawns
    // as a native child with the full builtin tool surface.
    if let Some(reason) = agent
        .runner
        .as_ref()
        .and_then(crate::runner::AgentRunnerConfig::refusal_reason)
    {
        return pre_spawn_failure(agent, task, reason);
    }

    // Step 1 (R-SA-025): fail fast before any subprocess spawns.
    if let Some(err) = validate_file_only_requires_path(opts.output_mode, opts.output_path.as_deref())
    {
        return pre_spawn_failure(agent, task, err.to_string());
    }

    // Step 2 (R-SA-023): resolve the effective acceptance contract.
    let contract = resolve_run_acceptance(opts, agent, task);

    let candidates = resolve_model_candidates(agent, opts);
    if candidates.is_empty() {
        let error =
            "no candidate model available for this subagent run (empty fallback ladder)".to_string();
        return pre_spawn_failure(agent, task, error);
    }

    // Step 2b (SUBA-078) — the `subagents.maxThinking` ceiling, folded once and then asserted over
    // the WHOLE ladder (pi `execution.ts:1711-1714` then `:1845-1851` @v0.57.0).
    //
    // Folding intersects the discovered/caller ceiling with whatever THIS process inherited through
    // `CYRUP_SUBAGENT_THINKING_CEILING`, taking the lowest — which is what makes the bound monotonic
    // as the tree deepens. It is also how the detached hop-2 runner gets its ceiling at all: that
    // path re-reads no settings and passes `RunOptions::thinking_ceiling = None`, so the env is its
    // only source.
    //
    // The assertion sweeps EVERY candidate before the ladder starts and refuses the whole run if
    // any one exceeds — upstream's shape, and deliberately NOT the per-attempt skip or the
    // warn-only treatment `model_scope` gives an out-of-scope fallback (`exec/model_scope.rs`). A
    // ceiling has no warn tier: silently running a shallower model, or silently dropping the rung
    // the operator's bound excluded, would both hide the misconfiguration.
    // Fail-CLOSED on BOTH steps: a malformed inherited ceiling, and an unrankable level in the
    // fold itself, are each an error rather than "unbounded".
    let folded = crate::exec::thinking_ceiling::inherited_thinking_ceiling().and_then(|inherited| {
        crate::exec::thinking_ceiling::intersect_thinking_ceilings(&[
            opts.thinking_ceiling.as_deref(),
            inherited.as_deref(),
        ])
    });
    let thinking_ceiling = match folded {
        Ok(ceiling) => ceiling,
        Err(error) => return pre_spawn_failure(agent, task, error),
    };
    if let Some(ceiling) = thinking_ceiling.as_deref() {
        for candidate in &candidates {
            let model_arg = crate::exec::spawn_plan::apply_thinking_suffix(
                Some(candidate.as_str()),
                agent.thinking.as_deref(),
                false,
            );
            if let Err(error) = crate::exec::thinking_ceiling::assert_thinking_within_ceiling(
                model_arg.as_deref(),
                agent.thinking.as_deref(),
                Some(ceiling),
                Some(agent.name.as_str()),
                opts.run_id.as_ref().map(crate::background::RunId::as_str),
            ) {
                return pre_spawn_failure(agent, task, error);
            }
        }
    }

    let setup = match prepare_ladder(agent, task, opts).await {
        Ok(setup) => setup,
        Err(failure) => return *failure,
    };
    let mut structured_guard = setup.structured_guard;
    let structured_runtime = structured_guard.as_ref().map(|guard| guard.runtime().clone());

    let outcome = drive_fallback_ladder(
        agent,
        task,
        opts,
        &contract,
        &candidates,
        setup.scratch_dir,
        setup.skill_injection,
        structured_runtime.clone(),
        setup.resolved_skill_names.is_some(),
    )
    .await;

    let winning_model = outcome.attempted_models.last().cloned();
    let last_signal = outcome.last_signal;
    let last_attempt = outcome.last_attempt;

    let SettledAttempt {
        timed_out,
        interrupted,
        detached,
        process_signal,
        exit_code,
        mut error,
        final_output,
    } = SettledAttempt::from_ladder(last_signal.as_ref(), last_attempt.as_ref());

    let turn_budget_tracker = last_attempt
        .as_ref()
        .map(|record| record.turn_budget.clone())
        .unwrap_or_default();
    let final_output =
        apply_terminal_preamble(final_output, timed_out, opts.timeout_ms, &turn_budget_tracker);

    let (final_output, full_output_for_reference, saved_output_path) =
        resolve_saved_output(opts, exit_code, final_output, setup.output_snapshot, &mut error);

    let (progress, mut control) = winning_attempt_state(last_attempt);

    let mut gates = GateState { exit_code, error, detached, interrupted, timed_out };
    let structured_output = gates.apply_structured_output(structured_runtime.as_ref(), opts);
    let guard_result = gates.apply_completion_guard(agent, task, &progress, &mut control);
    let acceptance_ledger = gates
        .apply_acceptance(&contract, &progress, opts, final_output.as_deref(), guard_result)
        .await;
    let GateState { exit_code, error, .. } = gates;

    let (final_output, output_truncated) = finalize_delivered_output(
        final_output,
        full_output_for_reference,
        saved_output_path.as_ref(),
        detached,
        exit_code,
        agent.max_output,
        opts.output_mode,
    );

    let progress_snapshot = build_progress_snapshot(
        &progress,
        opts,
        agent,
        task,
        setup.resolved_skill_names,
        winning_model.as_ref(),
        &control,
        detached,
        interrupted,
        exit_code,
        error.clone(),
    );

    disarm_structured_guard_on_detach(detached, structured_guard.as_mut());

    let (usage_budget, error) =
        resolve_terminal_usage_budget(opts, &outcome.aggregate_usage, error);

    SingleResult {
        usage_budget,
        // SUBA-008 — pi `result.turnBudget` / `result.turnBudgetExceeded` / `result.wrapUpRequested`
        // (`execution.ts:1087`), published from the WINNING attempt's own latch. `None`/`false` for
        // every run that declared no budget.
        turn_budget: turn_budget_tracker.state(),
        turn_budget_exceeded: turn_budget_tracker.exceeded(),
        wrap_up_requested: turn_budget_tracker.wrap_up_requested(),
        agent: agent.name.clone(),
        task: task.to_string(),
        exit_code,
        usage: outcome.aggregate_usage,
        model: winning_model,
        attempted_models: outcome.attempted_models,
        model_attempts: outcome.model_attempts,
        final_output,
        structured_output,
        acceptance: acceptance_ledger,
        detached,
        interrupted,
        timed_out,
        // G77/G104: pi's FOREGROUND executor never sets `result.stopped` — it only ever READS it
        // (`execution.ts:1086`/`:1571`/`:1689`), because the stop verb is a background-run control
        // request consumed by the detached runner (`subagent-runner.ts:2955-2984`), and a
        // foreground child has no control inbox. `false` here is therefore faithful, not a stub;
        // the live producers are `background/runner_main.rs`'s stop arm and the stale-run
        // reconciler.
        stopped: false,
        process_signal,
        error,
        // pi `result.savedOutputPath = resolvedOutput.savedPath` (`execution.ts:963`) — the SAME
        // path the saved-output reference message above was built from, published as its own field
        // so callers that need the bare location (dynamic-fanout collect records) do not have to
        // re-parse it out of `final_output`.
        saved_output_path: saved_output_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        tool_calls: progress.summarized_tool_calls(),
        output_truncated,
        progress: progress_snapshot,
        // pi `result.controlEvents = allControlEvents.length ? allControlEvents : undefined`
        // (`execution.ts:1260`) — an empty Vec is this crate's `undefined` (it serializes away).
        control_events: control.into_events(),
    }
}

/// Step 0 (R-SA-055, SAFETY-CRITICAL): the recursion-depth guard MUST run before any spawn,
/// discovery, or worktree setup — this is `run_sync`'s very first action, ahead of even
/// R-SA-025's output-mode validation in `run_sync`'s own step 1, because `run_sync` is the sole
/// chokepoint every production spawn path in this crate funnels through (the foreground single-run
/// tool dispatch, the background hop-2 runner's per-step loop, and — via `chain_graph::walk_chain`/
/// `spawn::parallel::run_bounded`'s `SingleStepExecutor` seam — every chain step, parallel
/// fan-out child, and dynamic fan-out child as well). A blocked check returns an error result
/// telling the caller to complete the task directly, per R-SA-055's own text, and — because
/// this check precedes every other line of `run_sync` — zero subprocesses are ever spawned
/// for a blocked attempt.
fn depth_guard_failure(agent: &AgentConfig, task: &str) -> Option<SingleResult> {
    if !crate::spawn::depth::is_blocked(&agent.depth) {
        return None;
    }
    let err = SubagentError::DepthExceeded {
        current: agent.depth.current_depth,
        max: agent.depth.max_depth,
    };
    Some(pre_spawn_failure(agent, task, err.to_string()))
}

/// Step 3 (R-SA-038).
///
/// SUBA-003: pi passes `{ scope: options.modelScope }` here (`execution.ts:1065-1070`), which
/// warns (never filters) for out-of-scope FALLBACK candidates. The ladder returned is identical
/// either way — an out-of-scope fallback is still attempted, exactly as upstream, because
/// dropping it would silently change which model ran.
fn resolve_model_candidates(agent: &AgentConfig, opts: &RunOptions) -> Vec<ModelId> {
    let (candidates, _scope_warnings) = crate::exec::fallback::build_model_candidates_scoped(
        &opts.model_override,
        agent.model.as_ref(),
        &agent.fallback_models,
        &opts.available_models,
        opts.model_scope.as_ref(),
    );
    candidates
}

/// Step 4: drive the model-fallback ladder, spawning one REAL child OS process per attempt via
/// [`SpawnedChildAttemptRunner`], and hand back the settled [`fallback::FallbackOutcome`].
///
/// Nine parameters is over clippy's threshold; they are `run_sync`'s own pre-ladder state handed
/// through verbatim, exactly like `evaluate_acceptance_with_cancel`'s own allow in
/// `acceptance/lattice/gate.rs`.
#[allow(clippy::too_many_arguments)]
async fn drive_fallback_ladder<'a>(
    agent: &'a AgentConfig,
    task: &'a str,
    opts: &'a RunOptions,
    contract: &'a AcceptanceContract,
    candidates: &[ModelId],
    scratch_dir: PathBuf,
    skill_injection: String,
    structured_runtime: Option<crate::exec::structured::StructuredOutputRuntime>,
    require_read_tool: bool,
) -> fallback::FallbackOutcome<AttemptRecord> {
    let mut runner = SpawnedChildAttemptRunner {
        agent,
        task,
        opts,
        contract,
        scratch_dir,
        skill_injection,
        attempt_index: 0,
        structured_runtime,
        // SUBA-014 / pi `runs/foreground/execution.ts:322,357` @v0.43.0:
        // `requireReadTool: Boolean(shared.resolvedSkillNames?.length)`. `resolved_skill_names` is
        // `Some` exactly when at least one declared skill resolved to a `SKILL.md`, so `is_some()`
        // IS `Boolean(...?.length)` — a declared-but-unresolvable skill grants nothing, matching
        // upstream, which derives the flag from the RESOLVED list rather than the requested one.
        require_read_tool,
    };
    run_fallback_ladder(candidates, &mut runner).await
}

/// The WINNING attempt's progress fold AND its live-control monitor (pi keeps both as locals of
/// the same `runSingleAttempt` scope; this crate has to carry them out of the ladder because
/// its post-settlement guard/acceptance steps live one level up, in `run_sync`).
fn winning_attempt_state(
    last_attempt: Option<AttemptRecord>,
) -> (AgentProgress, crate::exec::control::ControlMonitor) {
    match last_attempt {
        Some(record) => (record.progress, record.control),
        None => (
            AgentProgress::default(),
            crate::exec::control::ControlMonitor::disabled(),
        ),
    }
}

/// SUBA-S01 (pi `cleanupStructuredOutputRuntime`, `structured-output.ts:175-182`, invoked from
/// `subagent-executor.ts:3780-3787`'s `finally`): the removal itself is `structured_guard`'s
/// `Drop`, so it happens on EVERY exit from `run_sync` — including a cancellation that drops
/// this future mid-ladder, which an end-of-function statement could never cover.
///
/// This function is upstream's `if (!r?.detached)` guard and nothing else. A detached run's
/// child is still alive (R-SA-037) and has not written its capture file yet; that file lives
/// inside the very directory cleanup removes. pi says so in its own words at `:3782-3784` — "A
/// successful detached receipt transfers both to onDetachedExit while the authoritative
/// completion remains live" — and defers the cleanup to `onDetachedExit`'s inner `finally`
/// (`:3757-3761`). Disarming is that transfer. Before this, cyrup deleted the directory out from
/// under the live child on every detach, so a detached run could never produce a structured
/// value at all and the child's `structured_output` call would fail on a vanished parent dir.
fn disarm_structured_guard_on_detach(
    detached: bool,
    structured_guard: Option<&mut crate::exec::structured::StructuredOutputCleanupGuard>,
) {
    if detached && let Some(guard) = structured_guard {
        guard.disarm();
    }
}

/// SUBA-021 — the USAGE budget's terminal check (pi `subagent-runner.ts:4403-4411`):
///
/// ```text
/// setOptionalProperty(statusPayload, "usageBudget", usageBudgetState(config.usageBudget, currentUsageTotals()));
/// if (usageBudgetExceeded && statusPayload.usageBudget && !statusPayload.error)
///     statusPayload.error = usageBudgetExceededMessage(statusPayload.usageBudget);
/// ```
///
/// Computed from the run's AGGREGATE usage (every attempt of the fallback ladder, not just the
/// winning one) because that is what the run actually spent. `!statusPayload.error` is
/// load-bearing: a run that already failed keeps its own diagnosis — the budget did not cause
/// that failure and overwriting it would hide the real cause behind a bookkeeping note.
fn resolve_terminal_usage_budget(
    opts: &RunOptions,
    aggregate_usage: &Usage,
    error: Option<String>,
) -> (Option<crate::exec::usage_budget::UsageBudgetState>, Option<String>) {
    let usage_budget = crate::exec::usage_budget::usage_budget_state(
        opts.usage_budget,
        Some(crate::exec::usage_budget::UsageTotals::from(aggregate_usage)),
    );
    let error = match (&error, usage_budget.as_ref()) {
        (None, Some(state)) if state.exhausted => Some(
            crate::exec::usage_budget::usage_budget_exceeded_message(state),
        ),
        _ => error,
    };
    (usage_budget, error)
}

/// The [`SingleResult`] shape every pre-spawn failure in [`run_sync`] returns: exit 1, no usage,
/// no attempts, no artifacts — only the diagnosis.
///
/// SUBA-021: no usage budget on this path (see the field doc) — nothing was spent because
/// nothing was spawned.
fn pre_spawn_failure(agent: &AgentConfig, task: &str, error: String) -> SingleResult {
    SingleResult {
        usage_budget: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        agent: agent.name.clone(),
        task: task.to_string(),
        exit_code: 1,
        usage: Usage::default(),
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        final_output: None,
        structured_output: None,
        acceptance: None,
        detached: false,
        interrupted: false,
        timed_out: false,
        stopped: false,
        process_signal: None,
        error: Some(error),
        saved_output_path: None,
        tool_calls: Vec::new(),
        output_truncated: false,
        control_events: Vec::new(),
        progress: None,
    }
}

/// Everything [`prepare_ladder`] resolves before the fallback ladder starts, and that the
/// ladder (or `run_sync`'s completion path) then reads back.
struct LadderSetup {
    /// The `<available_skills>` block composed into every attempt's child prompt.
    skill_injection: String,
    /// The names that actually RESOLVED to a `SKILL.md` — carried alongside
    /// [`Self::skill_injection`] rather than being load-bearing by declaration order, because it
    /// outlives the injection string it is computed with.
    resolved_skill_names: Option<Vec<String>>,
    /// The output file's pre-ladder state (R-SA-031).
    output_snapshot: Option<crate::exec::output::OutputFileSnapshot>,
    /// This run's private scratch directory.
    scratch_dir: PathBuf,
    /// The structured-output capture runtime, RAII-scoped (SUBA-S01).
    structured_guard: Option<crate::exec::structured::StructuredOutputCleanupGuard>,
}

/// [`run_sync`]'s pre-ladder setup phase: resolve skills, snapshot the output file, make the
/// scratch directory, make the steer inbox/ack/capability directories, create the
/// structured-output capture runtime.
///
/// # Errors
///
/// `Err` carries the already-shaped pre-spawn failure result ([`pre_spawn_failure`]) for the two
/// setup steps that abort the run outright: an explicitly-requested-but-missing orchestration
/// skill, and an unusable scratch directory.
async fn prepare_ladder(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
) -> Result<LadderSetup, Box<SingleResult>> {
    // T5 (C4) — skill association: resolve the agent's (or the call-site's) configured skills to
    // lazy `<available_skills>` pointers ONCE, before the ladder starts, and compose them into every
    // attempt's child prompt (pi `execution.ts:935-952`). The names are `opts.skills ?? agent.skills`
    // (pi's `options.skills ?? agent.skills ?? []`); an empty list short-circuits discovery entirely
    // (the common case), so a run with no configured skills pays no discovery cost and injects
    // nothing. This is ORTHOGONAL to `agent.inherit_skills` — the `--no-skills` child flag governs
    // whether the child runs its OWN skill discovery, while THIS block always injects the explicitly
    // configured skills. Resolution is stable across model-fallback attempts (it never depends on the
    // model), so it is done here, not per attempt.
    let skill_names = opts.skills.clone().unwrap_or_else(|| agent.skills.clone());
    // pi `shared.resolvedSkillNames` (`runs/foreground/execution.ts:1481` @HEAD): the names that
    // actually RESOLVED to a `SKILL.md`, or `undefined` when none did — the value
    // `progress.skills` is seeded from (`:263`). Hoisted out of the `else` arm below because it
    // outlives the injection string it is computed alongside.
    let mut resolved_skill_names: Option<Vec<String>> = None;
    let skill_injection = if skill_names.is_empty() {
        String::new()
    } else {
        let resolution = crate::discovery::skills::resolve_skills_with_fallback(
            &skill_names,
            &opts.cwd,
            opts.runtime_cwd.as_deref(),
        )
        .await;
        // pi `execution.ts:938-946`: an EXPLICIT request for the orchestration skill (always
        // missing) is a hard failure, spawning nothing.
        let orchestration_requested = skill_names
            .iter()
            .any(|s| s.trim() == crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL);
        let orchestration_missing = resolution
            .missing
            .iter()
            .any(|m| m == crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL);
        if orchestration_requested && orchestration_missing {
            return Err(Box::new(pre_spawn_failure(
                agent,
                task,
                format!(
                    "Skills not found: {}",
                    crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL
                ),
            )));
        }
        resolved_skill_names = (!resolution.resolved.is_empty())
            .then(|| resolution.resolved.iter().map(|s| s.name.clone()).collect());
        crate::discovery::skills::build_skill_injection(&resolution.resolved)
    };

    // R-SA-031: snapshot the output file's state ONCE, before the ladder starts (a task's
    // `output_path` is stable across fallback attempts — see `SpawnedChildAttemptRunner::
    // snapshot_output_file`'s own doc note for why re-snapshotting per attempt is unnecessary).
    let output_snapshot = snapshot_output_file(opts.output_path.as_deref());

    let scratch_dir = opts.cwd.join(".cyrup-subagent-scratch");
    if let Err(err) = std::fs::create_dir_all(&scratch_dir) {
        return Err(Box::new(pre_spawn_failure(
            agent,
            task,
            format!("failed to prepare subagent scratch directory: {err}"),
        )));
    }

    // G90: create this child's steer inbox BEFORE the spawn. The child's own watcher does its own
    // `mkdir` on start (pi `subagent-prompt-runtime.ts:226`), but the RUNNER may route an accepted
    // steer request into the directory before the child has finished booting, and a request written
    // into a directory that is then created underneath it would be lost. Creating it here — at the
    // single point that also hands the path over — makes the directory exist for the whole of the
    // child's life. A failure is deliberately NOT fatal: steering is an optional live channel, and a
    // run must not fail to start because a control subdirectory could not be made.
    if let Some(inbox) = opts.steer_inbox_dir.as_deref() {
        let _ = std::fs::create_dir_all(inbox);
    }

    // SUBA-049: same reasoning for the ack directory, one hop earlier. The child creates it itself
    // before its first write, but the PARENT polls it while waiting for the acknowledgment — and
    // `consume_steer_acks` treats an unreadable directory as "no acks yet", which is
    // indistinguishable from "the child has not answered". Creating it here makes the empty-and-
    // waiting state a real, observable empty directory from the moment the child is spawned.
    if let Some(acks) = opts.steer_ack_dir.as_deref() {
        let _ = std::fs::create_dir_all(acks);
    }
    if let Some(parent) = opts.steer_capability_path.as_deref().and_then(std::path::Path::parent) {
        let _ = std::fs::create_dir_all(parent);
    }

    // SUBA-S01 (pi `chain-execution.ts:301` / `async-execution.ts:498`): when the step declares an
    // `outputSchema`, create the capture runtime ONCE per run — not per attempt — and write the
    // schema to a private file the child reads. Every fallback attempt shares it, exactly as pi
    // shares one runtime across a step's execution, so a retry cannot silently capture into a
    // different file than the one [`GateState::apply_structured_output`] reads back.
    //
    // A creation failure degrades to `None` rather than failing the run: the child then never
    // receives the env vars, never registers `structured_output`, and the read-back reports pi's
    // own "missing" hard failure — which is the correct outcome for "the schema never reached the
    // child", and strictly better than aborting a run that might still produce useful prose. It is
    // NOT a licence to go looking for the value somewhere pi never looks: see the read-back in
    // [`GateState::apply_structured_output`].
    //
    // SUBA-S01 residual: the runtime is held by a `StructuredOutputCleanupGuard`, which is the RAII
    // port of pi's `finally { if (!r?.detached) cleanupStructuredOutputRuntime(structuredRuntime); }`
    // (`runs/foreground/subagent-executor.ts:3780-3787` @v0.43.0). See that type's own doc for why
    // the end-of-function statement this replaces was wrong on BOTH halves.
    let structured_guard = opts
        .structured_output_schema
        .as_ref()
        .and_then(|schema| {
            crate::exec::structured::create_structured_output_runtime(schema, &scratch_dir).ok()
        })
        .map(crate::exec::structured::StructuredOutputCleanupGuard::new);

    Ok(LadderSetup {
        skill_injection,
        resolved_skill_names,
        output_snapshot,
        scratch_dir,
        structured_guard,
    })
}

/// The settled ladder outcome, destructured once into named fields instead of a seven-element
/// positional tuple whose meaning was pure position.
struct SettledAttempt {
    timed_out: bool,
    /// A soft interrupt is carried on the runner's own per-attempt payload
    /// ([`AttemptRecord::interrupted`], not on `AttemptSignal` which this crate does not
    /// own); an interrupted attempt reports `success: true`/`exit_code: 0`, so the
    /// ladder stops on it and this is the winning attempt whenever an interrupt fired
    /// (pi `execution.ts:748-761`, T3 group A). The [`GateState`] gates (structured-output,
    /// completion-guard, acceptance correction) all skip for a non-clean gate, so the
    /// paused-success `final_output` reaches the caller untouched.
    interrupted: bool,
    detached: bool,
    /// G104 — pi `if (signal) result.processSignal = signal;` (`execution.ts:1081`).
    /// The value was already computed by `process_signal_name` and stashed on the
    /// attempt's `StartupEvidence`; publishing it on the terminal `SingleResult` is what
    /// makes `resolveSubagentResultStatus`'s unexplained-signal → `"stopped"` branch
    /// (`result-intercom.ts:35`) reachable at all.
    process_signal: Option<String>,
    exit_code: i32,
    error: Option<String>,
    final_output: Option<String>,
}

impl SettledAttempt {
    /// Fold the winning attempt's [`AttemptSignal`] and [`AttemptRecord`] into the seven values
    /// `run_sync`'s completion path reads. A ladder that produced no attempt at all settles as a
    /// plain exit-1 failure with its own diagnosis.
    fn from_ladder(signal: Option<&AttemptSignal>, record: Option<&AttemptRecord>) -> Self {
        match (signal, record) {
            (Some(signal), Some(record)) => Self {
                timed_out: signal.timed_out,
                interrupted: record.interrupted,
                detached: signal.detached,
                process_signal: signal.startup.process_signal.clone(),
                exit_code: signal.exit_code.unwrap_or(if signal.success { 0 } else { 1 }),
                error: signal.error.clone(),
                final_output: record.final_output.clone(),
            },
            _ => Self {
                timed_out: false,
                interrupted: false,
                detached: false,
                process_signal: None,
                exit_code: 1,
                error: Some("subagent fallback ladder produced no attempt outcome".to_string()),
                final_output: None,
            },
        }
    }
}

/// Timeout message + partial-output preamble (pi `execution.ts:824-829`): a timed-out run's
/// delivered output leads with `Subagent timed out after {ms}ms.`, and — when the child produced
/// any partial output before the deadline fired — that partial output follows under a
/// `Partial output before timeout:` heading. Applied here, right after the ladder settles and
/// before the output-path handoff / truncation, exactly as pi applies it right after extracting
/// `fullOutput`. The nominal budget is `opts.timeout_ms` (pi `formatTimeoutMessage(options
/// .timeoutMs ?? 0)`), distinct from the wall-clock `deadline_at` that actually fired the timer.
///
/// SUBA-008 — pi's `else if` chain continues from the SAME `if (result.timedOut)`
/// (`execution.ts:1241-1258`), so a timed-out run never also gets a turn-budget preamble even
/// when both fired. That is why the three turn-budget arms are `else` on this branch and not a
/// second independent `if`.
fn apply_terminal_preamble(
    mut final_output: Option<String>,
    timed_out: bool,
    timeout_ms: Option<u64>,
    turn_budget_tracker: &crate::exec::turn_budget::TurnBudgetTracker,
) -> Option<String> {
    if timed_out {
        let timeout_message = format_timeout_message(timeout_ms.unwrap_or(0));
        let partial = final_output.clone().unwrap_or_default();
        final_output = Some(if partial.trim().is_empty() {
            timeout_message
        } else {
            format!("{timeout_message}\n\nPartial output before timeout:\n{partial}")
        });
    } else if let Some(note) = turn_budget_tracker.terminal_note() {
        let body = final_output.clone().unwrap_or_default();
        final_output = Some(match note {
            // pi `formatTurnBudgetOutput(turnBudgetExceededMessage(...), fullOutput)` (`:1252`) —
            // message first, whatever the child managed under a "Partial output" heading.
            crate::exec::turn_budget::TurnBudgetTerminalNote::Exceeded(message) => {
                crate::exec::turn_budget::format_turn_budget_output(&message, &body)
            }
            // pi `fullOutput.trim() ? `${note}\n\n${fullOutput}` : note` (`:1255`/`:1258`) — the
            // note leads, and the child's real answer follows it intact.
            crate::exec::turn_budget::TurnBudgetTerminalNote::Note(note) => {
                crate::exec::turn_budget::prepend_turn_budget_note(&body, &note)
            }
        });
    }
    final_output
}

/// R-SA-031: file-only/output-path handoff, once, against the aggregate captured output. Tracks
/// the concrete saved path (`Some` only when the file was actually written — by the child, or by
/// the orchestrator persisting its own captured output), which the saved-output reference message
/// in [`finalize_delivered_output`] (pi `finalizeSingleOutput`, `single-output.ts:211-235`) is
/// gated on. pi resolves the
/// handoff only for a clean run (`finalResult?.exitCode === 0`, `subagent-runner.ts:872`), so this
/// is gated on the same clean-completion condition rather than run unconditionally.
///
/// Returns the possibly-replaced delivered output, the FULL (untruncated) copy of it the
/// saved-output reference measures its byte/line counts over, and the concrete saved path; any
/// handoff error is folded into `error`.
fn resolve_saved_output(
    opts: &RunOptions,
    exit_code: i32,
    mut final_output: Option<String>,
    output_snapshot: Option<crate::exec::output::OutputFileSnapshot>,
    error: &mut Option<String>,
) -> (Option<String>, Option<String>, Option<PathBuf>) {
    let mut saved_output_path: Option<PathBuf> = None;
    if let Some(output_path) = opts.output_path.as_ref()
        && exit_code == 0
    {
        let captured = final_output.clone().unwrap_or_default();
        match resolve_output_handoff(output_path, &captured, output_snapshot) {
            crate::exec::output::OutputHandoff::ChildWrote { content } => {
                final_output = Some(content);
                saved_output_path = Some(output_path.clone());
            }
            crate::exec::output::OutputHandoff::OrchestratorWrote {
                written,
                error: handoff_error,
            } => {
                if written {
                    saved_output_path = Some(output_path.clone());
                }
                if let Some(handoff_error) = handoff_error {
                    let merged = match error.take() {
                        Some(existing) => format!("{existing}; {handoff_error}"),
                        None => handoff_error,
                    };
                    *error = Some(merged);
                }
            }
        }
    }
    // The FULL (untruncated) persisted content the saved-output reference measures its byte/line
    // counts over (pi `formatSavedOutputReference(savedPath, output)` uses the pre-truncation output,
    // `subagent-runner.ts:876`) — captured here, before step 9's truncation reassigns `final_output`.
    let full_output_for_reference = final_output.clone();
    (final_output, full_output_for_reference, saved_output_path)
}

/// The two values [`run_sync`]'s three post-settlement gates mutate — `exit_code` and `error` —
/// carried together with the three settled observations those gates read, so that every gate
/// derives its [`CleanCompletionGate`] from the SAME, always-current state.
///
/// The re-derivation this makes structural is load-bearing: R-SA-033's acceptance-gate condition
/// must observe the POST-completion-guard exit code (a run the completion guard already failed
/// must not additionally run acceptance evaluation against a stale `exit_code == 0` snapshot).
struct GateState {
    exit_code: i32,
    error: Option<String>,
    detached: bool,
    interrupted: bool,
    timed_out: bool,
}

impl GateState {
    /// The clean-completion gate as of RIGHT NOW — re-derived per call, never cached, so a gate
    /// that runs after an earlier gate's correction sees the corrected `exit_code`.
    fn gate(&self) -> CleanCompletionGate {
        CleanCompletionGate {
            exit_code: self.exit_code,
            detached: self.detached,
            interrupted: self.interrupted,
            timed_out: self.timed_out,
        }
    }

    /// Append `message` to the run's diagnosis, `; `-joined behind whatever a previous gate (or
    /// the attempt itself) already reported — an empty/blank existing error is replaced, not
    /// prefixed.
    fn push_error(&mut self, message: String) {
        self.error = Some(match self.error.take() {
            Some(existing) if !existing.trim().is_empty() => format!("{existing}; {message}"),
            _ => message,
        });
    }

    /// Step 5 (R-SA-030): structured-output extraction + parent-side JSON-Schema re-validation.
    /// Only evaluated on an otherwise-clean run (mirrors the completion-guard/acceptance gate's own
    /// "don't re-diagnose an already-failed attempt" discipline in the two gates that follow) — a run that already
    /// failed for another reason (non-zero exit, timeout, detach, interrupt) must not additionally
    /// be re-labeled by a structured-output check that never had a fair chance to run against a
    /// clean transcript.
    fn apply_structured_output(
        &mut self,
        structured_runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
        opts: &RunOptions,
    ) -> Option<serde_json::Value> {
        if self.gate().is_clean() {
            // SUBA-S01: read the FILE the child's `structured_output` tool wrote (pi
            // `readStructuredOutput`, `structured-output.ts:156-173`). The capture file is the ONLY
            // channel — pi has no other, and neither does this port any more.
            //
            // The `None` arm used to fall back to `resolve_structured_output`, a cyrup-original scan
            // that accepted the newest fenced ```json block in the child's prose. That is exactly what
            // the "EVEN WHEN prose was produced" rule below says must NOT satisfy a declared schema,
            // and it was not merely lenient: a coincidental fence could VALIDATE against the caller's
            // schema and become the run's structured result, silently feeding a wrong answer into a
            // chain's output bindings. A schema that was declared but whose capture runtime could not
            // be created is therefore `Missing` — no file, no value — which is the same hard failure
            // upstream produces when the child never called the tool.
            let structured_outcome = match structured_runtime {
                Some(runtime) => match crate::exec::structured::read_structured_output(runtime) {
                    Ok(value) => StructuredOutcome::Valid(value),
                    Err(message)
                        if message == crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR =>
                    {
                        StructuredOutcome::Missing
                    }
                    Err(message) => StructuredOutcome::Invalid(message),
                },
                None if opts.structured_output_schema.is_some() => StructuredOutcome::Missing,
                None => StructuredOutcome::NotRequested,
            };
            match structured_outcome {
                StructuredOutcome::NotRequested => None,
                StructuredOutcome::Valid(value) => Some(value),
                StructuredOutcome::Missing => {
                    // pi `readStructuredOutput` (structured-output.ts:156-173, execution.ts:1212-1216): a
                    // declared `outputSchema` with no captured `structured_output` value is a HARD
                    // failure — EVEN WHEN the child produced prose. pi runs its structured-output check
                    // on every clean exit and fails on the missing value unconditionally; prose is never
                    // an exemption. (An empty-prose + missing-structured attempt never reaches here: the
                    // per-attempt cold-start gate already failed it retryably via `structured_output_absent`,
                    // so a clean gate at this point implies prose WAS produced — exactly the "even with
                    // prose" case this must still reject.)
                    self.exit_code = 1;
                    self.push_error(
                        crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR.to_string(),
                    );
                    None
                }
                StructuredOutcome::Invalid(message) => {
                    self.exit_code = 1;
                    self.push_error(message);
                    None
                }
            }
        } else {
            None
        }
    }

    /// Step 6 (R-SA-034): completion-mutation guard — needs a real AgentDefinition-shaped view;
    /// `evaluate_completion_mutation_guard` only reads `local_name`/`tools`/`completion_guard`, so
    /// a minimal projection is built here rather than requiring `AgentConfig` to carry every other
    /// `AgentDefinition` field this guard never touches.
    ///
    /// Returns the guard's verdict, which the acceptance gate below consumes as an acceptance
    /// report source.
    fn apply_completion_guard(
        &mut self,
        agent: &AgentConfig,
        task: &str,
        progress: &AgentProgress,
        control: &mut crate::exec::control::ControlMonitor,
    ) -> CompletionMutationGuardResult {
        let guard_agent = completion_guard_projection(agent);
        let guard_result =
            evaluate_completion_mutation_guard(&guard_agent, task, &progress.all_events);

        if self.gate().is_clean() && guard_result.triggered {
            self.exit_code = 1;
            self.push_error(
                crate::exec::completion_guard::COMPLETION_GUARD_ERROR_MESSAGE.to_string(),
            );
            // pi `execution.ts:1234-1247`: the guard also raises a `needs_attention` control event with
            // `reason: "completion_guard"` — the one raise that happens AFTER the child is gone, and
            // the one the notice renderer formats as the "Subagent failed: <agent>" body rather than
            // the steer/resume nudge. Shares the winning attempt's dedup set (`control` is that
            // attempt's own monitor), exactly as the source's shared `emittedControlEventKeys` does.
            control.emit_completion_guard_notice(
                crate::time::now_epoch_millis(),
                format!(
                    "{} completed without making edits for an implementation task",
                    agent.name
                ),
            );
        }

        guard_result
    }

    /// Step 7 (R-SA-032) + Step 8 (R-SA-033), unless R-SA-037 bypasses both entirely.
    ///
    /// The gate is re-derived HERE, after [`Self::apply_completion_guard`]'s correction, since
    /// R-SA-033's own acceptance-gate condition must observe the POST-guard exit code (a run the
    /// completion guard already failed must not additionally run acceptance evaluation against a
    /// stale "exit_code == 0" snapshot).
    async fn apply_acceptance(
        &mut self,
        contract: &AcceptanceContract,
        progress: &AgentProgress,
        opts: &RunOptions,
        final_output: Option<&str>,
        guard_result: CompletionMutationGuardResult,
    ) -> Option<acceptance::AcceptanceLedger> {
        let post_guard_gate = self.gate();

        if self.detached {
            None
        } else if self.timed_out {
            // pi `buildTimedOutAcceptanceLedger` (`execution.ts:101-113`, applied at `1089-1090`): a
            // timed-out run's ledger is `rejected` (unless the contract required no acceptance at all,
            // in which case it stays `not-required`), NEVER the `not-required` a non-clean gate would
            // otherwise yield from `evaluate_acceptance`, and it carries a failed timeout runtime check.
            // No post-hoc exit-code correction runs — pi gates that on `!result.timedOut`
            // (`execution.ts:1098`), and the run already failed via the timeout path (exit_code != 0).
            Some(build_timed_out_acceptance_ledger(contract))
        } else {
            // G82 — pi `execution.ts:1680-1682`:
            //   const childWrittenOutput = options.outputPath
            //       ? extractChildWrittenOutput(result.messages, options.outputPath, options.cwd ?? runtimeCwd)
            //       : undefined;
            // Authorship taken from the CHILD'S OWN successful `write` calls, never from disk, so a
            // sibling run writing the same path cannot have its content misattributed here (#420).
            // Fed to the acceptance gate as an admissible acceptance-report source — the PRIMARY one
            // in `outputMode: "file-only"`, where the artifact, not the receipt prose, is the answer.
            let child_written_output = crate::exec::output::extract_child_written_output(
                &progress.all_events,
                opts.output_path.as_deref(),
                &opts.cwd,
            );
            // `fileOutput: childWrittenOutput !== undefined && options.outputPath ? {...} : undefined`
            // (`execution.ts:1699-1701`).
            let file_output = match (child_written_output.as_deref(), opts.output_path.as_deref()) {
                (Some(content), Some(path)) => Some(acceptance::AcceptanceFileOutput {
                    content,
                    path,
                    authoritative: matches!(opts.output_mode, OutputMode::FileOnly),
                }),
                _ => None,
            };
            // G80 — pi `evaluateAcceptance({ …, artifactsDir: options.artifactsDir, runId:
            // options.runId })` (`runs/foreground/execution.ts:1704-1705` @v0.43.0). Both must be
            // present for `runMemoizedVerifyCommand` to consult/record a memo (`acceptance.ts:1085`).
            let memo = match (opts.artifacts_dir.as_deref(), opts.run_id.as_ref()) {
                (Some(artifacts_dir), Some(run_id)) => Some(acceptance::model::VerifyMemoContext {
                    artifacts_dir,
                    run_id: run_id.as_str(),
                }),
                _ => None,
            };
            // SUBA-028 / pi `evaluateAcceptance({ …, signal: options.signal })`
            // (`runs/foreground/execution.ts:1704-1706` @v0.43.0). THIS is the call the item was about:
            // without the token, cancelling a run (Ctrl-C, orchestrator cancel, parent timeout) left
            // acceptance verification running, so the caller waited out a full per-command `timeoutMs`
            // — once per remaining command — after asking to stop.
            let ledger = acceptance::evaluate_acceptance_with_cancel(
                contract,
                post_guard_gate,
                final_output,
                guard_result,
                &opts.cwd,
                memo,
                file_output,
                &opts.cancel,
            )
            .await;

            let correction = apply_post_hoc_correction(
                &ledger,
                contract.explicit,
                post_guard_gate,
                self.error.as_deref(),
            );
            self.exit_code = correction.exit_code;
            self.error = correction.error;

            Some(ledger)
        }
    }
}

/// The delivered-output tail: strip acceptance-report fences, apply R-SA-042 truncation, then
/// append (or, in `file-only` mode, substitute) the saved-output reference message. All three
/// steps are skipped for a detached result (R-SA-037).
fn finalize_delivered_output(
    mut final_output: Option<String>,
    full_output_for_reference: Option<String>,
    saved_output_path: Option<&PathBuf>,
    detached: bool,
    exit_code: i32,
    max_output: crate::exec::output::OutputCap,
    output_mode: OutputMode,
) -> (Option<String>, bool) {
    // Strip trailing acceptance-report fences from the DELIVERED output (pi `stripAcceptanceReport`,
    // execution.ts:823/857). [`GateState::apply_acceptance`] already consumed the RAW report block for its
    // provenance evaluation (`evaluate_acceptance` receives the unstripped `final_output`); the
    // human/LLM caller must be shown the answer prose, never the machine report JSON that was
    // previously delivered verbatim. Skipped for a detached result (R-SA-037 bypasses output
    // post-processing entirely, exactly like the truncation step below).
    if !detached {
        final_output = final_output
            .as_deref()
            .map(crate::exec::acceptance::model::strip_acceptance_report);
    }

    // Step 9 (R-SA-042), skipped entirely for a detached result (R-SA-037).
    let (final_output, output_truncated) = if detached {
        (final_output, false)
    } else {
        match final_output {
            Some(text) => {
                let result = truncate_output(&text, max_output, None);
                (Some(result.text), result.truncated)
            }
            None => (None, false),
        }
    };

    // Saved-output reference (pi `finalizeSingleOutput`, `single-output.ts:211-235`): once a clean
    // run wrote its `output` file, the delivered output either gains a trailing
    // `Output saved to: <path> (<size>, <n> lines). Read this file if needed.` line (inline /
    // file-and-inline modes) or is REPLACED entirely by that reference message (file-only mode) — so
    // an LLM caller/terminal user sees where the artifact landed rather than a wall of inlined
    // content it can re-read on demand. The byte/line counts are measured over the FULL,
    // pre-truncation persisted content, with acceptance-report fences stripped — matching pi, which
    // measures `formatSavedOutputReference(savedPath, stripAcceptanceReport(resolvedOutput.fullOutput))`
    // (execution.ts:857-861).
    let final_output = match (saved_output_path, detached) {
        (Some(saved), false) if exit_code == 0 => {
            let full = crate::exec::acceptance::model::strip_acceptance_report(
                &full_output_for_reference.clone().unwrap_or_default(),
            );
            let reference = crate::exec::output::format_saved_output_reference(saved, &full);
            match output_mode {
                OutputMode::FileOnly => Some(reference.message),
                OutputMode::Inline | OutputMode::FileAndInline => Some(match final_output {
                    Some(text) if !text.is_empty() => {
                        format!("{text}\n\n{}", reference.message)
                    }
                    _ => reference.message,
                }),
            }
        }
        _ => final_output,
    };

    (final_output, output_truncated)
}

/// Step 10 (R-SA-043): compaction, and its ONE documented opt-out.
///
/// `SingleResult` is unconditionally the compacted shape — no raw per-turn messages, only
/// summarized `tool_calls`. `include_progress` restores exactly one thing on top of that: this
/// run's own `AgentProgress` projection, which pi gates identically (`progress:
/// params.includeProgress ? allProgress : undefined`, `subagent-executor.ts:3008` for SINGLE and
/// `:2679` for PARALLEL @v0.34.0). With the flag off or omitted the field stays `None` and
/// `skip_serializing_if` drops it, so a returned/persisted result is byte-for-byte what it was
/// before the field existed.
///
/// Assembled HERE, from the winning attempt's fold plus `run_sync`'s settled locals, because
/// that is where pi assembles it too: `execution.ts` mutates the one `progress` object at
/// `:907-913` @v0.34.0 and hands it out as `result.progress`. Deliberately NOT reusing the
/// orchestrator-layer `tui::events::LiveProgressFold` — that fold only exists on the streaming
/// foreground path (it is installed only when an `on_update` sink is present), so the detached
/// hop-2 runner and every non-streaming caller would get nothing.
// Eleven parameters is over clippy's threshold; this is `run_sync`'s own settled state handed
// through verbatim, exactly like `evaluate_acceptance_with_cancel`'s own allow in
// `acceptance/lattice/gate.rs`.
#[allow(clippy::too_many_arguments)]
fn build_progress_snapshot(
    progress: &AgentProgress,
    opts: &RunOptions,
    agent: &AgentConfig,
    task: &str,
    resolved_skill_names: Option<Vec<String>>,
    winning_model: Option<&ModelId>,
    control: &crate::exec::control::ControlMonitor,
    detached: bool,
    interrupted: bool,
    exit_code: i32,
    error: Option<String>,
) -> Option<crate::tui::events::LiveProgressSnapshot> {
    if opts.include_progress == Some(true) {
        // pi's settled `progress.status`. Order matters: a detach short-circuits at
        // `execution.ts:344` and an interrupt returns early at `:861` with the status pi set at
        // `:828` — neither ever reaches the `exitCode === 0 ? "completed" : "failed"` assignment at
        // `:907`. Leaving an interrupt-paused run as `Running` is therefore upstream's own shape,
        // and it is load-bearing: `compact_completed` refuses to compact a `running` snapshot
        // (pi `compactCompletedProgress`'s first line), which is exactly what lets the caller who
        // will `resume` this run still see its live detail.
        let status = if detached {
            crate::tui::events::LiveProgressStatus::Detached
        } else if interrupted {
            crate::tui::events::LiveProgressStatus::Running
        } else if exit_code == 0 {
            crate::tui::events::LiveProgressStatus::Complete
        } else {
            crate::tui::events::LiveProgressStatus::Failed
        };
        let snapshot = progress.snapshot(ProgressSnapshotInput {
            index: u32::try_from(opts.child_index.unwrap_or(0)).unwrap_or(u32::MAX),
            agent: &agent.name,
            task,
            skills: resolved_skill_names,
            // pi `progress.model = modelArg` (`execution.ts:267` @v0.34.0) — the id the child was
            // actually launched with, thinking suffix included, not the bare ladder entry.
            //
            // SUBA-075: "actually launched with" is why the fork thinking-override has to be
            // applied HERE too, on exactly the terms `build_attempt_spawn_plan` applied it. Reading
            // only `agent.thinking` would report `:high` for a child whose argv said `:off`, and
            // this snapshot is what the TUI and the run record show.
            model: apply_thinking_suffix(
                winning_model.map(ModelId::as_str),
                opts.fork_context
                    .thinking_override
                    .as_deref()
                    .or(agent.thinking.as_deref()),
                opts.fork_context.thinking_override.is_some(),
            ),
            thinking: agent.thinking.clone(),
            status,
            // pi `progress.activityState`, owned by the control state machine; the winning
            // attempt's monitor is the one `run_sync` carried out of the ladder, and it already
            // cleared the state on a soft interrupt exactly as pi does at `:832,854`.
            activity_state: control.activity_state(),
            error,
        });
        // pi `compactForegroundDetails` → `compactCompletedProgress` (`shared/utils.ts:330-347`):
        // a SETTLED snapshot keeps eleven fields and empties the two growth terms.
        Some(snapshot.compact_completed())
    } else {
        None
    }
}

/// Project an [`AgentConfig`] down to the minimal [`AgentDefinition`] shape
/// [`evaluate_completion_mutation_guard`] actually reads (`local_name`, `tools`,
/// `completion_guard`) — every other field is populated with an inert default since the guard
/// never inspects them. Kept private and narrowly scoped rather than exposing a
/// `From<&AgentConfig> for AgentDefinition` impl crate-wide, since a "mostly-fake"
/// `AgentDefinition` is only ever valid for this one guard call, not as a general conversion.
pub(crate) fn completion_guard_projection(agent: &AgentConfig) -> AgentDefinition {
    AgentDefinition {
        default_turn_budget: None,
        default_acceptance: agent.default_acceptance.clone(),
        acceptance_role: agent.acceptance_role,
        permission_rules: None,
        runner: None,
        name: agent.name.clone(),
        local_name: agent.name.clone(),
        package_name: None,
        description: String::new(),
        aliases: Vec::new(),
        tools: agent.tools.clone(),
        extensions: None,
        extensions_from_default: false,
        subagent_only_extensions: Vec::new(),
        exclude_tools: None,
        allow_nested_subagents: None,
        model: agent.model.clone(),
        fallback_models: agent.fallback_models.clone(),
        thinking: None,
        system_prompt_mode: agent.system_prompt_mode,
        inherit_project_context: false,
        inherit_skills: false,
        skills: Vec::new(),
        default_reads: None,
        default_progress: None,
        output: agent.output.clone(),
        completion_guard: agent.completion_guard,
        interactive: None,
        max_subagent_depth: agent.max_subagent_depth,
        default_context: None,
        default_async: None,
        default_timeout_ms: None,
        memory: None,
        tool_budget: None,
        disabled: None,
        system_prompt_body: agent.system_prompt_body.clone(),
        source: crate::discovery::types::AgentSource::User,
        file_path: PathBuf::new(),
        present_fields: std::collections::HashSet::new(),
        extra_fields: std::collections::BTreeMap::new(),
        override_info: None,
        model_source: None,
    }
}

// ================================================================================================
// plan_batch: eager whole-batch fork-context resolution (arch-SA §6.6, R-SA-137)
// ================================================================================================

/// One batch step's fork-context request, as [`plan_batch`] needs it: an index (for
/// [`ForkContextResolver`]'s own per-index caching) and the requested [`ContextMode`].
#[derive(Debug, Clone, Copy)]
pub struct BatchForkRequest {
    pub index: u32,
    pub requested: ContextMode,
}

/// R-SA-137 (MUST) — eagerly resolve EVERY step's [`ForkContext`] in `requests`, before spawning
/// ANY child process for the batch, via [`ForkContextResolver::resolve`] — the sole owner of
/// fork-context logic in this crate (arch-SA §6.6; this function never re-derives any part of
/// that algorithm, it only sequences calls into it).
///
/// If ANY resolution errors, the WHOLE batch aborts immediately — this function returns that
/// first error without attempting any further request, and (by construction: this function
/// spawns nothing itself) zero subprocesses have been spawned for this batch at the point of
/// failure. Implementing this lazily (validating step N's fork only when execution reaches step
/// N) would violate the fail-fast intent R-SA-137 requires; `plan_batch` exists specifically so a
/// caller (a later phase's chain/parallel dispatch in `exec/`, or the background hand-off's
/// one-shot runner-config construction, arch-SA §6.5) can call this ONCE, up front, for a whole
/// batch and only proceed to spawning if every resolution in `requests` succeeded.
///
/// On success, returns one [`ForkContext`] per request, in the SAME order as `requests` — a
/// caller zips this back against its own step list by position, mirroring R-SA-051's
/// position-preserving-regardless-of-completion-order discipline (restated here at plan time
/// rather than execution time, since fork-context resolution for a `Fresh` step is synchronous
/// and effectively instantaneous, so there is no meaningful "completion order" to preserve beyond
/// simply awaiting each request in the order given).
///
/// # Errors
///
/// Propagates the first [`SubagentError`] any individual [`ForkContextResolver::resolve`] call
/// returns (`ForkRequiresLeaf`/`ForkRequiresPersistedParent`/`ForkFailed`) — never falls back to
/// [`ContextMode::Fresh`] for a request that explicitly asked for [`ContextMode::Fork`]
/// (R-SA-137/DI-SA-2's fail-hard rule, restated at the batch level).
pub async fn plan_batch(
    resolver: &ForkContextResolver,
    requests: &[BatchForkRequest],
) -> Result<Vec<ForkContext>, SubagentError> {
    let mut resolved = Vec::with_capacity(requests.len());
    for request in requests {
        // SUBA-075: upstream's `forceThinkingOffForIndex?.(index) ?? true` fallback. A
        // [`BatchForkRequest`] names a step's context mode and index only — it carries no model
        // ladder — so the batch planner cannot answer the gate and takes the conservative arm.
        let ctx = resolver.resolve(request.requested, request.index, true).await?;
        resolved.push(ctx);
    }
    Ok(resolved)
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
    use crate::exec::acceptance::AcceptanceStatus;
    use crate::exec::testsupport::{base_opts, sample_agent_config};
    use crate::spawn::depth::DepthEnvelope;


    // ---- SUBA-075: the progress snapshot reports the model the child REALLY launched with ----

    /// pi sets `progress.model = modelArg` (`runs/foreground/execution.ts:267`), and `modelArg` is
    /// the id the override was already applied to. So this snapshot has to resolve the fork
    /// thinking-override on exactly the terms `build_attempt_spawn_plan` resolved it on: reading
    /// `agent.thinking` alone would report `:high` for a child whose argv said `:off`, and this
    /// snapshot is what the TUI and the run record show.
    ///
    /// Taken on an INTERRUPTED run on purpose: a settled snapshot is compacted, and
    /// `compact_completed` drops `model` outright (pi's literal has no such key), so a completed
    /// run could not observe this at all.
    #[test]
    fn the_progress_snapshot_model_carries_a_fork_thinking_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.thinking = Some("high".to_string());
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.include_progress = Some(true);
        let progress = crate::exec::progress::AgentProgress::default();
        let control = crate::exec::control::ControlMonitor::disabled();
        let winning = ModelId::from("m1");

        let snapshot = build_progress_snapshot(
            &progress, &opts, &agent, "task", None, Some(&winning), &control, false, true, 0, None,
        )
        .expect("progress was requested");
        assert_eq!(
            snapshot.model.as_deref(),
            Some("m1:high"),
            "precondition: with no override the persona's own level is what gets reported"
        );

        opts.fork_context = crate::fork_context::ForkContext {
            mode: crate::fork_context::ContextMode::Fork,
            session_file_path: Some(dir.path().join("branch.jsonl")),
            thinking_override: Some("off".to_string()),
        };
        let snapshot = build_progress_snapshot(
            &progress, &opts, &agent, "task", None, Some(&winning), &control, false, true, 0, None,
        )
        .expect("progress was requested");
        assert_eq!(
            snapshot.model.as_deref(),
            Some("m1:off"),
            "the reported model must agree with the argv the child was actually launched with"
        );
    }

    // ---- run_sync step 2: the effective contract is max(explicit, inferred) (R-SA-023) ----

    /// The seam itself, not just the rule it delegates to: `run_sync` must combine
    /// `opts.acceptance` with the inferred contract rather than let it replace it. Pre-fix this
    /// step read `opts.acceptance.clone().unwrap_or_else(|| heuristic_default(..))`, so the
    /// explicit `attested` below would have reached the gate verbatim — weaker than the `checked`
    /// pi resolves for the same policy on the same task
    /// (`runs/shared/acceptance.ts:277-281` @v0.34.0).
    #[test]
    fn run_sync_resolves_an_explicit_acceptance_level_as_a_floor_over_the_inferred_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);

        // A wire-lowered `acceptance: "attested"` (a floor, never a disable).
        opts.acceptance = Some(AcceptanceContract::explicit_floor(
            AcceptanceStatus::Attested,
            vec![],
        ));
        let contract = resolve_run_acceptance(&opts, &agent, "Implement the fix");
        assert_eq!(
            contract.required_level,
            AcceptanceStatus::Checked,
            "the inferred `checked` floor must win over the explicit `attested`"
        );
        assert!(contract.explicit, "R-SA-033's correction stays armed");

        // No explicit policy at all: pi's `auto` — the inferred contract, unchanged.
        opts.acceptance = None;
        assert_eq!(
            resolve_run_acceptance(&opts, &agent, "Implement the fix").required_level,
            AcceptanceStatus::Checked
        );

        // An in-Rust `NotRequired` contract still disables the gate outright.
        opts.acceptance = Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            vec![],
        ));
        assert!(resolve_run_acceptance(&opts, &agent, "Implement the fix").is_no_op());
    }


    // ---- SUBA-082: the agent's DECLARED role reaches the inferred floor at this seam ----

    /// `resolveEffectiveAcceptance({ …, acceptanceRole: agent.acceptanceRole, … })`
    /// (`runs/foreground/execution.ts:1834` @v0.64.0): the same agent name, task and (absent)
    /// explicit policy resolve to a DIFFERENT floor once the agent config carries a role. A
    /// `reviewer` that declares `writer` is a writer; a `worker` that declares `read-only` on
    /// neutral wording is not.
    #[test]
    fn run_sync_threads_the_agents_declared_acceptance_role_into_the_inferred_floor() {
        use crate::exec::acceptance::model::AcceptanceRole;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut opts = base_opts(dir.path(), &["m1"]);
        // `base_opts` disarms the gate for the spawn tests; this seam is about the INFERRED half.
        opts.acceptance = None;

        let mut reviewer = sample_agent_config("m1", &[]);
        reviewer.name = "reviewer".to_string();
        assert_eq!(
            resolve_run_acceptance(&opts, &reviewer, "Handle the authentication flow").required_level,
            AcceptanceStatus::Attested,
            "control: the NAME alternation still decides when no role is declared"
        );
        reviewer.acceptance_role = Some(AcceptanceRole::Writer);
        assert_eq!(
            resolve_run_acceptance(&opts, &reviewer, "Handle the authentication flow").required_level,
            AcceptanceStatus::Checked,
            "a declared `writer` role replaces the reviewer-name guess"
        );

        let mut worker = sample_agent_config("m1", &[]);
        worker.acceptance_role = Some(AcceptanceRole::ReadOnly);
        assert_eq!(
            resolve_run_acceptance(&opts, &worker, "Explore the authentication flow").required_level,
            AcceptanceStatus::Attested,
            "a declared `read-only` role replaces the worker-name guess"
        );
        assert_eq!(
            resolve_run_acceptance(&opts, &worker, "Implement the authentication fix").required_level,
            AcceptanceStatus::Checked,
            "explicit task mutation intent still wins over a declared read-only role"
        );
    }

    // ---- SUBA-074: an unsupported runner refuses the RUN, before the model ladder ----

    /// A declared `external-cli` profile must refuse the launch outright rather than spawn a
    /// full-capability native child. The refusal is a property of the RUN, so it fires ONCE,
    /// before any model is attempted — not once per candidate in the fallback ladder.
    #[tokio::test]
    async fn run_sync_refuses_an_unsupported_runner_once_before_any_model_attempt() {
        use crate::runner::{AgentRunnerConfig, ExternalCliRunner};

        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &["m2", "m3"]);
        agent.runner = Some(AgentRunnerConfig::ExternalCli(ExternalCliRunner {
            adapter: Some("claude-code".to_string()),
            command: "claude".to_string(),
            args: Vec::new(),
            prompt_delivery_stdin: false,
            capabilities: None,
        }));
        let opts = base_opts(dir.path(), &["m1", "m2", "m3"]);

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1, "an unsupported runner must fail the run: {result:?}");
        let error = result.error.as_deref().unwrap_or_default();
        assert!(error.contains("runner.type='external-cli'"), "{error}");
        assert!(error.contains("full-capability native child"), "{error}");
        assert!(
            result.attempted_models.is_empty(),
            "the refusal precedes the ladder, so NO model may be attempted; got {:?}",
            result.attempted_models
        );
    }

    /// `runner: {"type":"pi"}` is the native child, so it is indistinguishable from declaring no
    /// runner at all — it must NOT be refused.
    #[tokio::test]
    async fn run_sync_does_not_refuse_a_pi_runner() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.runner = Some(crate::runner::AgentRunnerConfig::Pi);
        let opts = base_opts(dir.path(), &["m1"]);

        let result = run_sync(&agent, "do something", &opts).await;

        assert!(
            !result.error.as_deref().unwrap_or_default().contains("not yet supported by cyrup"),
            "a `pi` runner is the native child and must never hit the SUBA-074 refusal: {result:?}"
        );
    }

    // ---- run_sync: depth guard runs first, before anything else (R-SA-055, SAFETY-CRITICAL) ----

    #[tokio::test]
    async fn run_sync_rejects_a_blocked_depth_envelope_before_any_spawn_setup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // current_depth == max_depth: is_blocked() must be true (R-SA-055's own `>=` semantics,
        // not merely `>`).
        agent.depth = DepthEnvelope {
            current_depth: 3,
            max_depth: 3,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1, "a blocked depth attempt must report failure: {result:?}");
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("depth limit exceeded"),
            "expected a DepthExceeded-shaped error message, got: {:?}",
            result.error
        );
        assert!(result.attempted_models.is_empty(), "no model attempt may ever be made");
        assert!(result.model_attempts.is_empty());
        assert_eq!(result.usage, Usage::default(), "no usage can have accrued");
        // The load-bearing proof that this rejection happens BEFORE any spawn setup: `run_sync`'s
        // scratch-directory creation (the very first filesystem side effect any subsequent spawn
        // attempt would need) must never have run at all.
        assert!(
            !dir.path().join(".cyrup-subagent-scratch").exists(),
            "the depth guard must reject before the spawn-scratch directory is ever created"
        );
    }


    #[tokio::test]
    async fn run_sync_rejects_when_depth_has_defensively_exceeded_the_ceiling() {
        // current_depth > max_depth (should never occur given each hop only increments by one past
        // a checked gate, but the guard must still be a safe `>=`, matching
        // `spawn::depth::is_blocked`'s own defense-in-depth comparison).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.depth = DepthEnvelope {
            current_depth: 9,
            max_depth: 2,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1);
        assert!(!dir.path().join(".cyrup-subagent-scratch").exists());
    }


    #[tokio::test]
    async fn run_sync_proceeds_normally_when_strictly_below_the_depth_ceiling() {
        // The negative case: a non-blocked envelope must NOT be rejected by the depth guard —
        // proven by observing this attempt fails for the ordinary, UNRELATED "no candidate model"
        // reason (this test supplies no available models), never a DepthExceeded message, so the
        // depth guard is proven to be neither a false-positive gate nor accidentally bypassed by a
        // change to this function's own step ordering.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.model = None;
        agent.depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &[]); // no available models: ladder is empty downstream

        let result = run_sync(&agent, "do something", &opts).await;

        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("no candidate model"),
            "a non-blocked depth must fall through to the NEXT gate (empty ladder), not be \
             rejected by the depth guard itself, got: {:?}",
            result.error
        );
    }


    // ---- run_sync: pre-spawn fail-fast (R-SA-025) ----

    #[tokio::test]
    async fn run_sync_fails_fast_on_file_only_mode_without_output_path_before_any_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = None;

        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("output path")
        );
        // No scratch dir should have been created since this fails before any spawn setup.
        assert!(!dir.path().join(".cyrup-subagent-scratch").exists());
    }


    #[tokio::test]
    async fn run_sync_fails_with_empty_ladder_when_no_model_is_resolvable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.model = None;
        let opts = base_opts(dir.path(), &[]); // nothing available
        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.attempted_models.is_empty());
    }


    // ---- SUBA-078: the maxThinking ceiling refuses the RUN, before any child spawns ----

    /// pi `execution.ts:1845-1851` @v0.57.0. A level above the ceiling REFUSES — it is not clamped
    /// to the ceiling and the run is not attempted at a lower level, because silently running
    /// shallower than the agent asked would hide the misconfiguration.
    #[tokio::test]
    async fn a_thinking_level_above_the_ceiling_refuses_the_run_before_any_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.thinking = Some("xhigh".to_string());
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.thinking_ceiling = Some("low".to_string());

        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(
            result.attempted_models.is_empty(),
            "the refusal must precede the ladder — no model may be attempted: {result:?}"
        );
        let error = result.error.as_deref().unwrap_or_default();
        assert!(
            error.contains("Thinking level 'xhigh' exceeds configured maximum 'low'")
                && error.contains("for agent 'worker'"),
            "pi's verbatim message, naming the agent: {error}"
        );
    }

    /// The sweep covers the WHOLE ladder, not just the primary: a fallback rung that would exceed
    /// refuses the entire run rather than being skipped. Deliberately unlike `model_scope`, which
    /// only WARNS for an out-of-scope fallback (`exec/model_scope.rs`) — a ceiling has no warn tier.
    #[tokio::test]
    async fn a_fallback_candidate_above_the_ceiling_refuses_the_whole_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        // The PRIMARY is clean; only the fallback carries a level above the bound.
        let agent = sample_agent_config("m1", &["m2:xhigh"]);
        let mut opts = base_opts(dir.path(), &["m1", "m2:xhigh"]);
        opts.thinking_ceiling = Some("low".to_string());

        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(
            result.attempted_models.is_empty(),
            "an offending FALLBACK must refuse before the ladder starts, not be skipped: {result:?}"
        );
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("exceeds configured maximum 'low'"),
            "{result:?}"
        );
    }

    /// SUBA-078 rework: an unrankable ceiling REFUSES the run rather than running it unbounded.
    ///
    /// `RunOptions::thinking_ceiling` is a public `Option<String>` on a public module, so an
    /// embedder can set a level this crate cannot rank. Before the fix the fold silently dropped
    /// it, the assert saw no ceiling, and the run proceeded with no bound at all — a configured
    /// ceiling vanishing without a word, which is the one outcome the whole module exists to
    /// prevent.
    #[tokio::test]
    async fn an_unrankable_ceiling_refuses_the_run_rather_than_running_it_unbounded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.thinking = Some("xhigh".to_string());
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.thinking_ceiling = Some("garbage".to_string());

        let result = run_sync(&agent, "do something", &opts).await;
        assert_eq!(result.exit_code, 1);
        assert!(
            result.attempted_models.is_empty(),
            "an unusable bound must refuse before the ladder, not be ignored: {result:?}"
        );
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Invalid thinking level comparison;"),
            "{result:?}"
        );
    }

    /// Fail-CLOSED at the run seam too: an unreadable inherited ceiling refuses rather than
    /// degrading to "unbounded".
    #[tokio::test]
    async fn a_ceiling_at_or_below_the_bound_does_not_refuse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.thinking = Some("low".to_string());
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.thinking_ceiling = Some("low".to_string());

        let result = run_sync(&agent, "do something", &opts).await;
        assert!(
            !result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("exceeds configured maximum"),
            "a level AT the ceiling is within it: {result:?}"
        );
    }

    // ---- plan_batch: eager whole-batch fork-context resolution (R-SA-137) ----

    #[tokio::test]
    async fn plan_batch_resolves_every_fresh_request_in_order() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/plan-batch-test");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        let manager = cyrup_session::SessionManager::in_memory(&cwd, cyrup_session::NewSessionOpts::default())
            .expect("create in-memory session");
        let manager = std::sync::Arc::new(tokio::sync::Mutex::new(manager));
        let resolver = ForkContextResolver::new(manager, layout);

        let requests = vec![
            BatchForkRequest {
                index: 0,
                requested: ContextMode::Fresh,
            },
            BatchForkRequest {
                index: 1,
                requested: ContextMode::Fresh,
            },
        ];
        let resolved = plan_batch(&resolver, &requests).await.expect("resolves");
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().all(|ctx| ctx.mode == ContextMode::Fresh));
    }


    #[tokio::test]
    async fn plan_batch_aborts_whole_batch_on_first_fork_failure_zero_side_effects() {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = PathBuf::from("/proj/plan-batch-abort-test");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        // Unpersisted in-memory session: any Fork request must fail hard (R-SA-137/DI-SA-2).
        let manager = cyrup_session::SessionManager::in_memory(&cwd, cyrup_session::NewSessionOpts::default())
            .expect("create in-memory session");
        let manager = std::sync::Arc::new(tokio::sync::Mutex::new(manager));
        let resolver = ForkContextResolver::new(manager, layout);

        let requests = vec![
            BatchForkRequest {
                index: 0,
                requested: ContextMode::Fresh,
            },
            BatchForkRequest {
                index: 1,
                requested: ContextMode::Fork, // must fail: unpersisted parent
            },
            BatchForkRequest {
                index: 2,
                requested: ContextMode::Fresh,
            },
        ];
        let err = plan_batch(&resolver, &requests)
            .await
            .expect_err("must abort on the second request's failure");
        assert!(matches!(
            err,
            SubagentError::ForkRequiresPersistedParent | SubagentError::ForkRequiresLeaf
        ));

        // No filesystem state created anywhere under root — proof zero subprocess/session-branch
        // side effects occurred beyond the failed resolution itself.
        let any_files = std::fs::read_dir(root.path())
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        assert!(!any_files);
    }

}
