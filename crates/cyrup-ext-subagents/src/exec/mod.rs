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
//! [`run_sync`]'s per-attempt driver ([`SpawnedChildAttemptRunner`]) spawns a REAL OS subprocess
//! for every model-fallback attempt via [`crate::spawn::SpawnedChild::spawn`] — never an
//! in-process nested agent turn loop, never an in-process event-relay standing in for the child's
//! own execution (func-SA §1.1). Cancellation is threaded as two independent
//! `cyrup_core::CancelToken`s (`RunOptions.cancel` for hard abort, `RunOptions.interrupt` for a
//! soft, per-run interrupt) raced via `tokio::select!` against
//! [`crate::spawn::SpawnedChild::terminate`]'s real SIGINT->SIGTERM->SIGKILL escalation ladder —
//! this module never invents a second, competing cancellation mechanism.

/// The acceptance-provenance ledger: contract injection, gate evaluation, and REAL `verify[]`
/// subprocess execution (R-SA-023/030/032/033; DI-SA-5).
pub mod acceptance;

/// Project-local per-agent refinement overlays (`<cwd>/.cyrup-subagents/refinements/<agent>.md`) —
/// the READ half of pi-subagents' `agents/agent-refinements.ts`, applied to the child's system
/// prompt between the memory block and the output-path override (`execution.ts:1442`).
pub mod agent_refinements;

/// Bounded child-protocol I/O: the 16 MiB stdout line cap and its `protocol_output_limit`
/// diagnostic, the oversized-aggregate-record projection, the bounded stderr byte tail, and the
/// drain start/cancel lifecycle projection — a port of pi's `runs/shared/child-protocol.ts`.
pub mod child_protocol;

/// Implementation-expecting classification and mutating-tool-call scan (R-SA-034).
pub mod completion_guard;

/// Live-control config resolution + the control-event/notice pipeline (pi
/// `runs/shared/subagent-control.ts` + the control half of `runs/shared/long-running-guard.ts`).
pub mod control;

/// Direct-MCP tool-allowlist resolution (T4) — `mcp:<server>[/<tool>]` selectors are expanded into
/// concrete adapter-visible builtin tool names for the child's `--tools` allowlist (pi
/// `resolveMcpDirectToolNames`, `runs/shared/mcp-direct-tool-allowlist.ts`), rather than passed
/// through literally.
pub mod mcp_direct_tools;

/// The model-fallback attempt loop (`build_model_candidates`, `is_retryable_model_failure`,
/// `run_fallback_ladder`) — R-SA-035/036/037/038/039/040/041/044.
pub mod fallback;

/// Optional `subagents.modelScope` enforcement (`check_model_scope`, `parse_model_scope_config`)
/// — a 1:1 port of pi-subagents' `runs/shared/model-scope.ts`.
pub mod model_scope;

/// The NDJSON event-stream parser (`SubagentEvent`, `consume_stdout`) — R-SA-026/057/058.
pub mod ndjson;

/// Final-output extraction (R-SA-029), file-only output-path stat-snapshot handoff
/// (R-SA-024/025/031), and UTF-8-safe output truncation (R-SA-042).
pub mod output;

/// Parent-side structured-output extraction and JSON-Schema re-validation (R-SA-030).
pub mod structured;

/// The shared task mutation-intent classifier — a port of pi-subagents'
/// `runs/shared/task-intent.ts` @v0.43.0. Consumed by [`completion_guard`] (does the task REQUIRE
/// file changes?) and by [`acceptance`]'s level inference (COULD it plausibly change files?).
pub mod task_intent;

/// `{text, expandedText}` tool-call argument previews (R-SA-043's compaction target) — pi
/// `ToolCallSummary` + `formatToolCall` (`shared/types.ts:601`, `shared/formatters.ts:99`).
pub mod tool_call_summary;

/// Per-run child tool-call budgets (`toolBudget:` frontmatter and the
/// `CYRUP_SUBAGENT_TOOL_BUDGET` env hand-off) — a port of pi-subagents'
/// `runs/shared/tool-budget.ts`. Enforcement lives child-side in [`crate::prompt_runtime`].
pub mod tool_budget;

/// SUBA-008 — per-run child assistant-TURN budgets (`turnBudget:` frontmatter, the `turnBudget`
/// tool param and the `subagents.turnBudget` config key) — a port of pi-subagents'
/// `runs/shared/turn-budget.ts`. Unlike [`tool_budget`] there is NO env hand-off and no child-side
/// enforcement: the child is only *told* about the budget through a system-prompt block, and
/// [`drive_attempt`] enforces it parent-side off the child's own assistant `message_end` events.
pub mod turn_budget;

/// SUBA-021 (first half) — the subagent CAPABILITY CEILING: the monotonically-tightening
/// `allowedTools`/`allowedAgents`/`denyExtensions` upper bound a parent imposes on its subtree, its
/// per-session registry, and the [`capability_ceiling::CAPABILITY_CEILING_ENV`] hand-off that keeps
/// it applying across this crate's re-exec boundary. A port of pi-subagents'
/// `runs/shared/capability-ceiling.ts`.
pub mod capability_ceiling;

/// SUBA-021 (second half) — per-run USAGE budgets (`tokens` / `costUsd`, each with an advisory
/// `soft` and a terminal `hard` limit): a port of pi-subagents' `runs/shared/usage-budget.ts`.
/// Distinct from [`turn_budget`] and [`tool_budget`], which bound how MANY turns/tool calls a child
/// takes rather than what they cost.
pub mod usage_budget;

/// SUBA-046 — the per-session subagent SPAWN budget, its snapshot, and the explicit grant path
/// behind an exhausted cap: a port of pi-subagents' `runs/shared/spawn-budget.ts`. Consumed by
/// `extension.rs`'s `reserve_subagent_spawns` and by the `grant-spawn-budget` dispatch arm.
pub mod spawn_budget;

/// SUBA-045 — the child tool-availability diagnostic (`CYRUP_SUBAGENT_TOOL_DIAGNOSTIC_PATH`), a
/// port of pi-subagents' `runs/shared/tool-availability.ts`. The child writes it from its live
/// registry; this module's `run_attempt` reads it back so a tool that was never registered becomes
/// the run's error instead of a model apology.
pub mod tool_availability;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, ModelId, ProviderId, Usage};

use crate::discovery::types::{
    AgentDefinition, AgentReadScope, OutputMode, OutputSpec, SystemPromptMode, ToolRef,
};
use crate::error::SubagentError;
use crate::exec::acceptance::{
    AcceptanceContract, AcceptanceLedger, CleanCompletionGate, apply_post_hoc_correction,
    build_timed_out_acceptance_ledger, inject_acceptance_contract,
};
use crate::exec::completion_guard::evaluate_completion_mutation_guard;
use crate::exec::fallback::{
    AttemptRunner, AttemptSignal, ModelAttempt, ModelOverride, StartupEvidence, StartupOutcome,
    StartupRetryWait, run_fallback_ladder,
};
use crate::exec::ndjson::SubagentEvent;
use crate::exec::output::{
    EMPTY_OUTPUT_ERROR, INTERRUPTED_FINAL_OUTPUT, OutputCap, detect_subagent_error,
    extract_final_output, inject_output_path_system_prompt, inject_single_output_instruction,
    is_terminal_assistant_stop,
    message_end_has_error_message, resolve_output_handoff, snapshot_output_file,
    trailing_assistant_error, truncate_output, validate_file_only_requires_path,
};
use crate::exec::structured::StructuredOutcome;
use crate::fork_context::{ContextMode, ForkContext, ForkContextResolver};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::{ChildSpawnSpec, SpawnCommand, SpawnedChild};

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

fn bound_output_line(line: &str) -> String {
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

// ================================================================================================
// AgentConfig / RunOptions / SingleResult (arch-SA §3.4)
// ================================================================================================

/// The resolved, execution-ready subset of an [`AgentDefinition`] this module's foreground
/// executor needs (arch-SA §3.4). Deliberately narrower than the full `AgentDefinition` — this
/// type carries only what `run_sync` itself branches on, not discovery/management metadata
/// (`source`, `file_path`, `present_fields`, …) that has no bearing on one execution.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The agent's local (unqualified) name — feeds [`completion_guard::expects_implementation_mutation`]'s
    /// `agent` classification input and [`acceptance::AcceptanceContract::heuristic_default`].
    pub name: String,
    pub model: Option<ModelId>,
    pub fallback_models: Vec<ModelId>,
    /// The agent's frontmatter reasoning level (func-SA §4.1 `AgentDefinition::thinking`) as pi's
    /// OPEN string, applied to the child's `--model` argument as a `:<value>` suffix at spawn time via
    /// [`apply_thinking_suffix`] (pi `applyThinkingSuffix`, `runs/shared/pi-args.ts:186-200`) — `None` leaves the
    /// per-attempt model id untouched. Carrying the raw string (rather than a closed on-only enum)
    /// means an explicit `Some("off")` now reaches the child as `:off`, exactly like pi, instead of
    /// being conflated with `None` and dropped.
    pub thinking: Option<String>,
    pub system_prompt_mode: SystemPromptMode,
    pub system_prompt_body: String,
    pub tools: Option<Vec<ToolRef>>,
    /// Extension-allowlist tri-state (func-SA §4.1 `AgentDefinition::extensions`): `Some(list)`
    /// emits `--no-extensions` plus an explicit `--extension` for each entry (discovery off, exact
    /// allowlist); `None` leaves the child's own extension discovery on (pi `runs/shared/pi-args.ts:128-137`).
    pub extensions: Option<Vec<String>>,
    /// Child-only extension paths always threaded as `--extension`, visible to the spawned child
    /// even when not visible to the orchestrator (pi `subagentOnlyExtensions`).
    pub subagent_only_extensions: Vec<String>,
    pub output: Option<crate::discovery::types::OutputSpec>,
    /// pi's `PI_SUBAGENT_INHERIT_PROJECT_CONTEXT` env flag: whether the child inherits the parent's
    /// project-context files (`AGENTS.md`/`CLAUDE.md`) — threaded to the child as
    /// `CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT=1|0` (pi `runs/shared/pi-args.ts:215` @v0.34.0).
    pub inherit_project_context: bool,
    /// Whether the child inherits skills discovery: when `false`, the child is spawned with
    /// `--no-skills` and `CYRUP_SUBAGENT_INHERIT_SKILLS=0` (pi `runs/shared/pi-args.ts:156,216` @v0.34.0).
    pub inherit_skills: bool,
    /// The agent's own frontmatter `skills` list (func-SA §4.1, R-SA-017). These are the
    /// EXPLICITLY-configured skill names resolved to `<available_skills>` pointers and injected into
    /// the child's system prompt at spawn (pi `execution.ts:935-952`, via
    /// [`crate::discovery::skills::resolve_skills_with_fallback`]/`build_skill_injection`). This is
    /// ORTHOGONAL to [`AgentConfig::inherit_skills`]: an agent that does not inherit the parent's
    /// own skill discovery (`--no-skills`) still receives its own explicitly-listed skills as lazy
    /// pointers, exactly like pi. Empty (the common case) short-circuits skill resolution entirely —
    /// no discovery pass runs.
    pub skills: Vec<String>,
    /// `None`/`Some(true)` leaves the completion-mutation guard active (subject to that
    /// subsystem's own read-only-tools short-circuit); `Some(false)` disables it entirely
    /// (R-SA-034).
    pub completion_guard: Option<bool>,
    /// The byte/line truncation budget for this agent's delivered output (R-SA-042). Reuses
    /// [`output::OutputCap`] directly (the type `exec/output.rs` already defines and tests)
    /// rather than inventing a second, competing cap type — architecture.md §3.4's illustrative
    /// `AgentConfig::max_output: OutputCap` sketch predates that module's own landing; this field
    /// is the real wiring of that sketch onto the type that actually exists.
    pub max_output: OutputCap,
    /// Effective recursion-depth ceiling this agent declares for ITS OWN children, feeding
    /// [`crate::spawn::depth::next_envelope`]'s tightening-only merge (R-SA-056) — `None` means
    /// "no agent-level tightening; pass the inherited ceiling through unchanged".
    pub max_subagent_depth: Option<u32>,
    /// Depth envelope this process itself resolved at startup ([`crate::spawn::depth::resolve_effective_depth`]),
    /// threaded through so `run_sync` can compute the CHILD's envelope via `next_envelope` before
    /// ever building the spawn env overlay (R-SA-054/055/056).
    pub depth: DepthEnvelope,
    /// The agent's `memory:` scope (pi `AgentConfig.memory`). Resolved at spawn into the
    /// persistent-memory block folded onto the child's persona system prompt
    /// ([`crate::discovery::agent_memory::build_agent_memory_injection`], pi
    /// `execution.ts:1058-1061`).
    pub memory: Option<crate::discovery::types::AgentMemoryConfig>,
    /// The agent's validated `toolBudget:` (pi `AgentConfig.toolBudget`). Encoded into the child's
    /// [`crate::exec::tool_budget::TOOL_BUDGET_ENV`] at spawn; the child-side runtime
    /// ([`crate::prompt_runtime`]) is what actually nudges and blocks.
    pub tool_budget: Option<crate::discovery::types::ResolvedToolBudget>,
}

impl AgentConfig {
    /// Build an [`AgentConfig`] from a fully-resolved [`AgentDefinition`] plus the depth envelope
    /// this process itself resolved. A thin projection, not a re-derivation: every field here is
    /// copied straight off `agent`, never reclassified.
    #[must_use]
    pub fn from_agent_definition(agent: &AgentDefinition, depth: DepthEnvelope) -> Self {
        Self {
            name: agent.local_name.clone(),
            model: agent.model.clone(),
            fallback_models: agent.fallback_models.clone(),
            thinking: agent.thinking.clone(),
            system_prompt_mode: agent.system_prompt_mode,
            system_prompt_body: agent.system_prompt_body.clone(),
            tools: agent.tools.clone(),
            extensions: agent.extensions.clone(),
            subagent_only_extensions: agent.subagent_only_extensions.clone(),
            output: agent.output.clone(),
            inherit_project_context: agent.inherit_project_context,
            inherit_skills: agent.inherit_skills,
            skills: agent.skills.clone(),
            completion_guard: agent.completion_guard,
            max_output: OutputCap::default(),
            max_subagent_depth: agent.max_subagent_depth,
            depth,
            memory: agent.memory.clone(),
            tool_budget: agent.tool_budget.clone(),
        }
    }
}

/// A fully-resolved agent persona in **serializable** form (C13/T0.1) — the plan-time projection
/// of an [`AgentDefinition`] that the orchestrator resolves ONCE, at plan time, and carries
/// per-run into the hop-2 detached runner (via [`crate::background::runner_main::RunnerConfig`]'s
/// `resolved_agents` map) and into the foreground chain/parallel executor
/// ([`crate::background::runner_main::ExecSingleStepExecutor`]). It is the mechanism that lets a
/// chain/parallel/background step dispatch the **real named persona** (its own system prompt,
/// model, fallback ladder, tool allowlist, output spec, and completion-guard flag) instead of the
/// empty-system-prompt / `--model default` / guard-disabled placeholder the runner previously
/// synthesized because "the runner has no discovery access" — matching pi, where every child
/// resolves its agent config from the already-resolved `agents` list handed down to
/// `runSync(cwd, agents, agentName, …)` (`execution.ts:891-898`) /
/// `chain-execution.ts:1011` (`agents.find((a) => a.name === seqStep.agent)`), never
/// re-discovering (`parallel-execution.test.ts:134-172`).
///
/// This is a deliberately narrower, purely-serializable subset of [`AgentConfig`]: it carries no
/// [`OutputCap`] (`AgentConfig::from_agent_definition` always seeds that from `OutputCap::default`,
/// so it is a pure runtime value with no plan-time provenance to serialize) and no
/// [`DepthEnvelope`] (`depth` is a per-*process* runtime value the detached runner resolves from
/// its OWN inherited `CYRUP_SUBAGENT_DEPTH`/`_MAX_DEPTH` env via
/// [`crate::spawn::depth::resolve_effective_depth`], never a plan-time input the orchestrator could
/// meaningfully bake in). The execution-ready [`AgentConfig`] is reconstituted at dispatch time by
/// [`ResolvedAgentPersona::to_agent_config`], which stamps the runner's own live depth envelope
/// onto the persona.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAgentPersona {
    /// The agent's local (unqualified) name — exactly [`AgentConfig::name`].
    pub name: String,
    pub model: Option<ModelId>,
    pub fallback_models: Vec<ModelId>,
    /// The agent's own frontmatter reasoning level (pi's OPEN string), carried on the resolved persona
    /// so a chain/parallel/background step applies the SAME `:<value>` `--model` suffix the single-run
    /// path does (T4 thinking-suffix), including an explicit `off`. `#[serde(default)]` keeps the
    /// runner-config hand-off backward compatible.
    #[serde(default)]
    pub thinking: Option<String>,
    pub system_prompt_mode: SystemPromptMode,
    pub system_prompt_body: String,
    pub tools: Option<Vec<ToolRef>>,
    /// The agent's own extension allowlist tri-state, carried so a chain/parallel/background step
    /// threads `--no-extensions`/`--extension` identically to the single-run path (T4 inherit
    /// flags). `#[serde(default)]` keeps the runner-config hand-off backward compatible.
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    /// The agent's own child-only extension paths, carried so a chain/parallel/background step
    /// threads them as `--extension` identically to the single-run path. `#[serde(default)]` keeps
    /// the runner-config hand-off backward compatible.
    #[serde(default)]
    pub subagent_only_extensions: Vec<String>,
    pub output: Option<OutputSpec>,
    /// The agent's own project-context inheritance flag, carried so a chain/parallel/background
    /// step threads `CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT` identically to the single-run path.
    /// `#[serde(default)]` keeps the runner-config hand-off backward compatible.
    #[serde(default)]
    pub inherit_project_context: bool,
    /// The agent's own skills inheritance flag, carried so a chain/parallel/background step threads
    /// `--no-skills`/`CYRUP_SUBAGENT_INHERIT_SKILLS` identically to the single-run path.
    /// `#[serde(default)]` keeps the runner-config hand-off backward compatible.
    #[serde(default)]
    pub inherit_skills: bool,
    /// The agent's own frontmatter `skills` list, carried so a chain/parallel/background step
    /// injects the SAME `<available_skills>` pointer block the single-run path does — resolved and
    /// injected at the shared `run_sync` chokepoint via [`AgentConfig::skills`] (pi resolves each
    /// child's skills from the already-resolved agent config, `async-execution.ts:429,876` @v0.34.0).
    /// `#[serde(default)]` keeps the runner-config hand-off backward compatible.
    #[serde(default)]
    pub skills: Vec<String>,
    /// `None`/`Some(true)` leaves the completion-mutation guard active; `Some(false)` disables it
    /// (R-SA-034). Carried through verbatim so a chain/parallel/background step honors the agent's
    /// OWN guard configuration rather than the placeholder's hard-`Some(false)` (C13).
    pub completion_guard: Option<bool>,
    /// The agent's own recursion-depth ceiling for ITS children (R-SA-056's tightening-only merge
    /// input), preserved so [`crate::spawn::depth::next_envelope`] can apply it at the child spawn
    /// boundary.
    pub max_subagent_depth: Option<u32>,
    /// The agent's own persona-level `defaultContext` (func-SA §4.1 `AgentDefinition::default_context`).
    /// Carried on the resolved persona so the orchestrator can, when a call site OMITS `context`,
    /// fall back to each requested agent's OWN default via
    /// [`crate::fork_context::resolve_effective_context`] (pi `resolveAgentDefaultContextPolicy`,
    /// `subagent-executor.ts:1875-1891`) — matching pi, where the already-resolved `agents` list the
    /// executor consults carries `defaultContext`. `#[serde(default)]` keeps the one-shot
    /// runner-config hand-off backward compatible.
    #[serde(default)]
    pub default_context: Option<ContextMode>,
    /// The agent's own `memory:` scope, carried so a chain/parallel/background step folds the SAME
    /// persistent-memory block onto the child persona the single-run path does.
    /// `#[serde(default)]` keeps the runner-config hand-off backward compatible.
    #[serde(default)]
    pub memory: Option<crate::discovery::types::AgentMemoryConfig>,
    /// The agent's own validated `toolBudget:`, carried so a chain/parallel/background step hands
    /// the child the SAME budget the single-run path does. `#[serde(default)]` keeps the
    /// runner-config hand-off backward compatible.
    #[serde(default)]
    pub tool_budget: Option<crate::discovery::types::ResolvedToolBudget>,
}

impl ResolvedAgentPersona {
    /// Project a fully-resolved [`AgentDefinition`] into its serializable persona — a thin copy,
    /// never a re-derivation, of exactly the fields [`AgentConfig::from_agent_definition`] itself
    /// copies (minus the two pure-runtime values `max_output`/`depth` this type deliberately omits;
    /// see the type-level doc).
    #[must_use]
    pub fn from_agent_definition(agent: &AgentDefinition) -> Self {
        Self {
            name: agent.local_name.clone(),
            model: agent.model.clone(),
            fallback_models: agent.fallback_models.clone(),
            thinking: agent.thinking.clone(),
            system_prompt_mode: agent.system_prompt_mode,
            system_prompt_body: agent.system_prompt_body.clone(),
            tools: agent.tools.clone(),
            extensions: agent.extensions.clone(),
            subagent_only_extensions: agent.subagent_only_extensions.clone(),
            output: agent.output.clone(),
            inherit_project_context: agent.inherit_project_context,
            inherit_skills: agent.inherit_skills,
            skills: agent.skills.clone(),
            completion_guard: agent.completion_guard,
            max_subagent_depth: agent.max_subagent_depth,
            default_context: agent.default_context,
            memory: agent.memory.clone(),
            tool_budget: agent.tool_budget.clone(),
        }
    }

    /// Reconstitute the execution-ready [`AgentConfig`] from this persona, stamping the dispatching
    /// process's own live [`DepthEnvelope`] onto it (the depth is a per-process runtime value, not a
    /// plan-time one — see this type's doc). `max_output` is seeded from `OutputCap::default`,
    /// identically to [`AgentConfig::from_agent_definition`].
    #[must_use]
    pub fn to_agent_config(&self, depth: DepthEnvelope) -> AgentConfig {
        AgentConfig {
            name: self.name.clone(),
            model: self.model.clone(),
            fallback_models: self.fallback_models.clone(),
            thinking: self.thinking.clone(),
            system_prompt_mode: self.system_prompt_mode,
            system_prompt_body: self.system_prompt_body.clone(),
            tools: self.tools.clone(),
            extensions: self.extensions.clone(),
            subagent_only_extensions: self.subagent_only_extensions.clone(),
            output: self.output.clone(),
            inherit_project_context: self.inherit_project_context,
            inherit_skills: self.inherit_skills,
            skills: self.skills.clone(),
            completion_guard: self.completion_guard,
            max_output: OutputCap::default(),
            max_subagent_depth: self.max_subagent_depth,
            depth,
            memory: self.memory.clone(),
            tool_budget: self.tool_budget.clone(),
        }
    }
}

/// Resolve one already-discovered [`AgentDefinition`] to its serializable [`ResolvedAgentPersona`]
/// at **plan time** (T0.1). This is the canonical resolver the orchestrator (`extension.rs`) calls
/// for every distinct agent named by a chain/parallel/background step's [`crate::spawn::chain_graph::SingleStepSpec::agent`],
/// stashing the results in [`crate::background::runner_main::RunnerConfig::resolved_agents`] (for a
/// background run) or handing them straight to
/// [`crate::background::runner_main::ExecSingleStepExecutor::foreground`] (for a foreground
/// `/chain`//`/parallel` run) — so the runner dispatches the REAL persona and NEVER re-discovers
/// (`RunnerConfig`'s own "never re-discovers agents" contract). The discovery lookup itself
/// (name -> `AgentDefinition`) stays the orchestrator's job (it owns the discovery pipeline,
/// `extension.rs::resolve_agent`); this function is the pure definition -> persona projection that
/// keeps that resolution identical to the single-run path's [`AgentConfig::from_agent_definition`].
#[must_use]
pub fn resolve_step_agent_config(agent: &AgentDefinition) -> ResolvedAgentPersona {
    ResolvedAgentPersona::from_agent_definition(agent)
}

/// R-SA-041: distinguishes "the caller didn't specify a model override" from "explicitly use this
/// model" — re-exported here under `exec`'s own namespace so `RunOptions::model_override` has a
/// stable, documented home even though the type itself is [`fallback`]'s (one canonical owner,
/// consumed by this module rather than redefined).
pub use crate::exec::fallback::ModelOverride as RunModelOverride;

/// Every per-call parameter [`run_sync`] needs beyond the resolved [`AgentConfig`] and task text
/// (arch-SA §3.4). Threaded through unmodified across every model-fallback attempt for this one
/// task (R-SA-035's deadline-monotonicity requirement, restated at the type level: nothing in
/// this struct is ever recomputed mid-ladder).
#[derive(Debug, Clone)]
pub struct RunOptions {
    pub cwd: PathBuf,
    /// Monotonically shrinking deadline, computed ONCE at the start of the outer (chain/parallel/
    /// single) call and passed through unmodified to every subsequent attempt (R-SA-035).
    /// `None` means no wall-clock timeout at all.
    pub deadline_at: Option<Instant>,
    /// The NOMINAL foreground timeout budget in milliseconds (pi `timeoutMs`/`maxRuntimeMs`, aliases
    /// resolved by the orchestrator, `subagent-executor.ts:1951-1968`). Distinct from `deadline_at`
    /// (the actual wall-clock instant the timer fires at): `timeout_ms` is what the timed-out
    /// message renders (`Subagent timed out after {ms}ms.`, pi `formatTimeoutMessage`), while
    /// `deadline_at` is what [`run_sync`] actually races the child against. Normally the orchestrator
    /// sets `deadline_at = now + timeout_ms` once; when only `timeout_ms` is set, `run_sync` derives
    /// the deadline from it (pi `resolveAttemptTimeout`: `deadlineAt ?? now + timeoutMs`). `None`
    /// means no foreground timeout at all.
    pub timeout_ms: Option<u64>,
    pub output_path: Option<PathBuf>,
    pub output_mode: OutputMode,
    /// SUBA-054 — the run's declared read paths, pi's `reads` binding at
    /// `runs/foreground/subagent-executor.ts:3869`:
    /// `readsOverride !== undefined ? readsOverride : agentConfig.defaultReads ?? false`.
    ///
    /// Carried UNRESOLVED (declared, not existence-filtered): resolution needs [`Self::cwd`], which
    /// is upstream's `effectiveCwd`, and it happens once in [`build_task_text`] through
    /// `spawn::chain_graph::build_single_reads_instruction`. `None` is upstream's `false` — no read
    /// instruction at all.
    ///
    /// Before this existed, `defaultReads` was parsed off frontmatter and rendered in agent
    /// listings but never reached a run: the bundled `reviewer`'s `defaultReads: plan.md,
    /// progress.md` was documentation. Chain steps had the instruction all along, through the
    /// separate `SingleStepSpec::reads` → `build_chain_instructions` path.
    pub reads: Option<Vec<PathBuf>>,
    pub structured_output_schema: Option<serde_json::Value>,
    /// R-SA-041's inherit sentinel — `Inherit` MUST NOT itself fall through to a global
    /// cross-session default inside [`run_sync`]; a caller wanting that global-default behavior
    /// resolves it explicitly before constructing this struct.
    pub model_override: ModelOverride,
    pub preferred_provider: Option<ProviderId>,
    pub available_models: Vec<ModelId>,
    /// Hard-abort cancellation, raced independently of `interrupt` (arch-SA §5.1).
    pub cancel: CancelToken,
    /// Soft, per-run interrupt — distinct downstream consequences from `cancel`/timeout
    /// (R-SA-084 vs. R-SA-036); this module treats an interrupt firing identically to a timeout
    /// for ladder-termination purposes (both stop the fallback ladder outright) but records it
    /// under its own `interrupted` flag on [`SingleResult`] rather than conflating it with
    /// `timed_out`.
    pub interrupt: CancelToken,
    /// pi `options.share` (`execution.ts:1027`, the tool's SINGLE-mode `share` param). Its ONE
    /// effect at this port's baseline is pi's `sessionEnabled` term (`execution.ts:1412`): a
    /// `Some(true)` keeps the child's session store ON even without an explicit
    /// [`Self::session_dir`], so `--no-session` is not emitted (see
    /// [`build_attempt_spawn_plan`]). pi v0.34.0 has no gist upload of its own — the tool schema's
    /// legacy "Upload session to GitHub Gist" wording describes a capability neither side ships.
    pub share: Option<bool>,
    /// pi `options.sessionDir` (`runs/shared/pi-args.ts:107-111`): the directory the child persists its session
    /// under, passed through as `--session-dir <dir>` and created up front. Ignored when
    /// [`ForkContext::session_file_path`] resolved a concrete session FILE (pi's `sessionFile` wins
    /// that branch outright).
    pub session_dir: Option<PathBuf>,
    /// Per-call skill-name override (pi `options.skills`, `execution.ts:935`): when `Some`, these
    /// names are resolved and injected into the child prompt INSTEAD of the agent's own
    /// [`AgentConfig::skills`] (a chain/parallel step's `skills:` binding or the tool's `skill`
    /// param). `None` (the default) falls through to the agent's own list.
    pub skills: Option<Vec<String>>,
    /// The orchestrator's own cwd, used as the FALLBACK cwd for skill resolution when a skill named
    /// by the agent/step is absent from the execution [`RunOptions::cwd`] (pi `runtimeCwd`, the
    /// second arg to `resolveSkillsWithFallback`, `execution.ts:937`). `None` resolves skills
    /// against `cwd` alone (no fallback).
    pub runtime_cwd: Option<PathBuf>,
    /// pi `params.includeProgress` (`extension/schemas.ts:272` @v0.34.0, *"Include full progress in
    /// result (default: false)"*) — R-SA-043 compaction's ONE documented opt-out.
    ///
    /// `Some(true)` — and ONLY `Some(true)`, matching pi's truthiness gate `progress:
    /// params.includeProgress ? allProgress : undefined` (`subagent-executor.ts:3008` for SINGLE,
    /// `:2679` for PARALLEL) — makes [`run_sync`] assemble this run's own
    /// [`crate::tui::events::LiveProgressSnapshot`] onto [`SingleResult::progress`]. `None` and
    /// `Some(false)` leave that field `None`, which `skip_serializing_if` then omits from the wire
    /// entirely: the returned/persisted result is byte-for-byte what it was before the field
    /// existed.
    ///
    /// It never affects any OTHER field of [`SingleResult`] — the messages/transcript compaction
    /// R-SA-043 mandates is unconditional on both sides.
    pub include_progress: Option<bool>,
    pub agent_scope: Option<AgentReadScope>,
    /// The effective `subagents.modelScope` policy for this run (SUBA-003), threaded down from the
    /// orchestrator's own discovery pass — pi's `options.modelScope` (`execution.ts:1069`).
    ///
    /// Used here ONLY for the fallback ladder's warn-severity check on non-primary candidates; the
    /// hard, fail-closed refusal of an EXPLICIT out-of-scope model has already happened upstream in
    /// [`crate::exec::fallback::resolve_model_inheritance`], so by the time a `RunOptions` exists
    /// its `model_override` is known to be in scope (or the scope is not armed). `None` = no
    /// policy configured.
    pub model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    /// Explicit acceptance-contract override for this task (func-SA §4.2 `acceptance`); `None`
    /// defers to [`AcceptanceContract::heuristic_default`] (R-SA-023).
    pub acceptance: Option<AcceptanceContract>,
    /// Resolved fork-context for this task, if any — normally produced by [`plan_batch`] ahead of
    /// time (R-SA-137) and threaded straight through here; `Fresh` (the default) when this task
    /// runs with no inherited session state.
    pub fork_context: ForkContext,
    /// A live raw-NDJSON-line sink the background hop-2 runner installs to observe this child's
    /// stdout as it streams (pi's `updateStepFromChildEvent` child-event pump,
    /// `subagent-runner.ts:2706-2861`), so it can fold `currentTool`/`recentTools`/token telemetry
    /// into `status.json` on the fly. `None` (the default) for the foreground single-run path and
    /// for tests, which have no live status file to update.
    pub live_events: Option<LiveEventSink>,
    /// The canonical parent-session anchor to inject into this child's spawn env overlay as
    /// [`PARENT_SESSION_ENV_VAR`] (proposed R-SA-P1, port doc §4 P-4). `Some(id)` is the EXPLICIT
    /// value — the launching session's own id from P-2 [`cyrup_ext::host::HostServices::session_id`]
    /// (captured at the root orchestrator's `SessionStart`). `None` (the default, and the detached
    /// hop-2 runner) defers to the INHERITED value already in this process's own
    /// `CYRUP_SUBAGENT_PARENT_SESSION` env; absent both, the anchor is omitted (empty). The
    /// permission companion reads it (`forwarding/mod.rs`) to address the parent's ask-forwarding
    /// inbox; this crate only ever WRITES it.
    pub parent_session_id: Option<String>,
    /// Optional clarify/ask dispatch context (R-SA-037/119/120). When `Some`, [`drive_attempt`]'s
    /// NDJSON loop fires [`crate::tui::intercom::spawn_clarify`] against the executor's single-slot
    /// [`crate::tui::intercom::AskLock`] the moment the child emits a BLOCKING `contact_supervisor`
    /// ask (`need_decision`/`interview`), surfacing the ask to the parent's human via the real
    /// `ClarifyChannel` and marking the attempt `detached` (bypassing acceptance). `None` (the
    /// default — the detached hop-2 runner, and tests with no channel) degrades to the prior
    /// no-clarify behavior. The intercom answer routes back to the still-alive child over the BROKER
    /// (a transport independent of this child's stdout), so the drive loop neither kills nor
    /// synchronously blocks on the child while the ask is outstanding.
    pub clarify: Option<crate::tui::intercom::ClarifyDispatch>,
    /// The launching orchestrator's own intercom presence target (pi
    /// `data.intercomBridge.orchestratorTarget`, `subagent-executor.ts:1765`), threaded into this
    /// child's spawn env overlay as [`crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET`] so the
    /// spawned child's `IntercomExtension` reads non-`None` child-orchestrator metadata → registers
    /// `contact_supervisor` (addressed at THIS supervisor) + a broker presence under its own
    /// deterministic label (the child-bridge activation, pi `runs/shared/pi-args.ts:204-205`). `None` (headless /
    /// no live intercom session, and every no-intercom test) leaves the six child-bridge vars unset,
    /// so the spawned child registers no supervisor bridge — the clean no-intercom path. Gated
    /// together with [`Self::run_id`]: BOTH must be `Some` for the metadata to activate at all, so a
    /// half-set bridge env is never written.
    pub orchestrator_intercom_target: Option<String>,
    /// This run's id (pi `runId`), threaded into the child's spawn env as
    /// [`crate::spawn::nested_events::RUN_ID_ENV`] and folded into the child's own deterministic
    /// presence label via [`crate::spawn::intercom_target::resolve_subagent_intercom_target`]. Paired
    /// with [`Self::orchestrator_intercom_target`] — both `Some` is the child-bridge activation gate.
    pub run_id: Option<crate::background::RunId>,
    /// This child's flat index within its run (pi `childIndex`/`ctx.flatIndex`, `runs/shared/pi-args.ts:738-740`)
    /// — the `+1`-suffixed step position in its own presence label + the child's
    /// [`crate::spawn::nested_events::CHILD_INDEX_ENV`]. `None` defaults to `0` (a single top-level
    /// run has one child at index 0).
    pub child_index: Option<usize>,
    /// G90 — this child's OWN steer inbox directory
    /// (`<run_dir>/control/steer-targets/<flatIndex>/`), handed to the child process as
    /// [`crate::prompt_runtime::STEER_INBOX_ENV`].
    ///
    /// pi `steerInboxDir` (`runs/shared/pi-args.ts:67,251-252` @v0.34.0), supplied by the background runner as
    /// `stepSteerInboxDir(asyncDir, fi)` (`subagent-runner.ts:2313,2600,2797` @v0.34.0). This is the ONLY
    /// channel a steer request has to a running child: the parent drops a request into the
    /// run-level queue, the runner routes it into this per-child directory, and the child's own
    /// [`crate::prompt_runtime::SteeringInbox`] watches THIS path and injects each message into
    /// its live model turn. Without the env var the child never learns the path exists, and the
    /// whole verb is a write-only file drop — which is exactly what it was.
    ///
    /// `None` on the foreground path (no async run directory exists), matching upstream's own
    /// `if (input.steerInboxDir)` guard.
    pub steer_inbox_dir: Option<PathBuf>,
    /// SUBA-049 — this child's OWN steer-acknowledgment directory
    /// (`<run_dir>/control/steer-acks/<flatIndex>/`), handed over as
    /// [`crate::prompt_runtime::STEER_ACK_DIR_ENV`].
    ///
    /// pi `steerAckDir` (`runs/shared/pi-args.ts:766-768` @v0.43.0, `if (input.steerAckDir)
    /// env[SUBAGENT_STEER_ACK_DIR_ENV] = input.steerAckDir`), read child-side at
    /// `subagent-prompt-runtime.ts:335`.
    ///
    /// This is the RETURN path, and without it the whole steer channel is one-way: the parent could
    /// write a request and could not learn whether it was taken, queued behind a full follow-up
    /// queue, or refused outright by a child whose host cannot inject messages at all. All three
    /// looked like success.
    pub steer_ack_dir: Option<PathBuf>,
    /// SUBA-049 — where this child publishes its steering capability
    /// (`<run_dir>/control/steer-capabilities/<flatIndex>.json`), handed over as
    /// [`crate::prompt_runtime::STEER_CAPABILITY_ENV`].
    ///
    /// pi `steerCapabilityPath` (`runs/shared/pi-args.ts:764-765` @v0.43.0), read child-side at
    /// `subagent-prompt-runtime.ts:334` and written from its `publishCapability` closure (`:360-362`).
    ///
    /// Separate from the ack directory because it answers a different question at a different time:
    /// the ack says what happened to ONE request, the capability says — before any request exists —
    /// whether this child can be steered at all and under which pid. A parent that sees
    /// `supported: false` knows not to wait out the acknowledgment timeout.
    pub steer_capability_path: Option<PathBuf>,
    /// pi `options.controlConfig` (`execution.ts:245`, threaded from `runSinglePath`'s
    /// `resolveControlConfig(deps.config.control, effectiveParams.control)`, `subagent-executor.ts:3385`
    /// @v0.34.0; the detached async runner reads the same value back out of its one-shot config,
    /// `subagent-runner.ts:1802`):
    /// the fully-resolved live-control thresholds/channels this run's attention pipeline runs
    /// against. `None` is pi's `?? DEFAULT_CONTROL_CONFIG` — control tracking ON with the stock
    /// 60s/240s/3 thresholds, NOT "off". Set
    /// [`crate::exec::control::ResolvedControlConfig::enabled`] to `false` to turn it off.
    pub control_config: Option<crate::exec::control::ResolvedControlConfig>,
    /// G80 — pi `options.artifactsDir` (`runs/foreground/execution.ts:1704`, threaded into
    /// `evaluateAcceptance` alongside [`Self::run_id`] and from there into
    /// `runMemoizedVerifyCommand`, `runs/shared/acceptance.ts:1072-1132` @v0.43.0): the run's
    /// artifacts root, under which a verify command's memoized result is recorded at
    /// `<artifacts_dir>/acceptance/verify/<run_id>/<cacheKey>.json`.
    ///
    /// BOTH this and [`Self::run_id`] must be `Some` for memoization to arm at all
    /// (`acceptance.ts:1085`); `None` — every caller with no artifacts root, and pi's own
    /// `artifacts: false` opt-out (SUBA-041) — executes every verify[] command for real, exactly
    /// as this crate did before G80.
    pub artifacts_dir: Option<PathBuf>,
    /// pi `options.onControlEvent` (`execution.ts:255`): the per-raise callback the ORCHESTRATOR
    /// installs (`createForegroundControlNotifier`, `subagent-executor.ts:1222-1229` @v0.34.0) to fan a
    /// raised event out to the notice channels. `None` (every non-tool caller, and tests) still
    /// records events on [`SingleResult::control_events`]; it just delivers none of them live.
    pub on_control_event: Option<crate::exec::control::ControlEventSink>,
    /// SUBA-008 — pi `options.turnBudget` (`runs/foreground/execution.ts:326`/`:399`/`:734`, resolved
    /// at `subagent-executor.ts:4928` from `effectiveParams.turnBudget ?? deps.config.turnBudget`
    /// after `applySingleAgentLaunchDefaults` has folded in the agent's own `turnBudget:`
    /// frontmatter): the assistant-TURN budget this run enforces.
    ///
    /// Already validated by [`crate::exec::turn_budget::resolve_turn_budget_config`], so `graceTurns`
    /// is defaulted and nothing downstream re-derives it. `None` means unbudgeted, which is every
    /// run that does not ask for one — upstream has no default budget.
    pub turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-008 — pi `options.enforceHardTurnLimit` (`subagent-executor.ts:240`, `shared/types.ts:1648`):
    /// suppress the mid-tool-work deferral so the hard limit really terminates.
    ///
    /// `false` for every tool-driven run, matching upstream's optional field being absent; pi's only
    /// caller that sets it is the slash-command delegation adapter (`slash/delegation-adapters.ts:298`).
    pub enforce_hard_turn_limit: bool,
    /// SUBA-021 — pi `params.usageBudget` (`extension/schemas.ts:330`, threaded to the runner at
    /// `subagent-runner.ts:172`): the reported-consumption budget this run enforces.
    ///
    /// Already validated by [`crate::exec::usage_budget::validate_usage_budget_config`], so an
    /// invalid budget never reaches here — it is refused at the tool boundary with upstream's own
    /// text. `None` means unbudgeted, which is every run that does not ask for one: upstream has no
    /// default usage budget any more than it has a default turn budget.
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
}

/// A live per-line sink installed via [`RunOptions::live_events`]: [`run_sync`]'s per-attempt driver
/// hands every complete raw NDJSON stdout line to it as the line is read, BEFORE this crate parses
/// or folds the line, so a caller (the background runner) can parse it into its OWN telemetry event
/// vocabulary without this module depending on that caller. Cheap to clone (an `Arc`); a runtime
/// callback with no serializable content, so it is never persisted with the rest of [`RunOptions`].
#[derive(Clone)]
pub struct LiveEventSink {
    /// The child's raw NDJSON stdout lines.
    lines: LiveEventCallback,
    /// PARENT-side attempt notes — see [`LiveEventSink::emit_note`].
    ///
    /// `None` for a sink that only cares about child output (the background telemetry forwarder),
    /// in which case notes are dropped.
    notes: Option<LiveEventCallback>,
}

/// One installed live-event callback. Named so [`LiveEventSink`]'s two fields do not each have to
/// spell the full `Arc<dyn Fn…>` out.
type LiveEventCallback = std::sync::Arc<dyn Fn(&str) + Send + Sync>;

impl std::fmt::Debug for LiveEventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveEventSink(..)")
    }
}

impl LiveEventSink {
    /// Wrap a raw-line callback as a cheaply-cloneable sink.
    #[must_use]
    pub fn new(sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            lines: std::sync::Arc::new(sink),
            notes: None,
        }
    }

    /// Also route PARENT-side attempt notes to `sink` (additive; a sink without one drops them).
    #[must_use]
    pub fn with_note_sink(mut self, sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.notes = Some(std::sync::Arc::new(sink));
        self
    }

    /// Deliver one raw NDJSON stdout line to the installed callback.
    pub fn emit(&self, raw_line: &str) {
        (self.lines)(raw_line);
    }

    /// Deliver one PARENT-side attempt note (a model-fallback or startup-retry note) to the live
    /// progress surface.
    ///
    /// Notes are the one thing on that surface that does NOT come from the child: pi seeds each
    /// attempt's progress ring with them at construction (`recentOutput: [...shared.attemptNotes]`,
    /// `runs/foreground/execution.ts:432`) and streams that same `progress` object through
    /// `fireUpdate()` for the whole attempt. They are the only explanation the user ever gets for
    /// why a run was relaunched, and they must arrive DURING the relaunched attempt — a settled
    /// snapshot cannot carry them, because `compactCompletedProgress` (`shared/utils.ts:330-347`,
    /// ported at [`crate::tui::events::LiveProgressSnapshot::compact_completed`]) empties
    /// `recent_output` as one of its two growth terms.
    ///
    /// cyrup's live surface folds the child's raw NDJSON rather than sharing pi's one mutable
    /// `progress` object, so a parent-side note has no child line to ride in on — hence this second,
    /// explicit channel. [CYRUP-DELTA: transport only; the note text, its timing and the ring it
    /// lands in are pi's.]
    pub fn emit_note(&self, note: &str) {
        if let Some(notes) = &self.notes {
            notes(note);
        }
    }
}

/// One row of a completed run's per-attempt history, re-exported under `exec`'s own namespace so
/// callers of `run_sync` never need to import `exec::fallback` directly for this shape (one
/// canonical owner: [`fallback::ModelAttempt`]).
pub use crate::exec::fallback::ModelAttempt as RunModelAttempt;

/// `{text, expandedText}` tool-call preview (R-SA-043), re-exported under `exec`'s own namespace so
/// callers of `run_sync` (and consumers of [`SingleResult::tool_calls`]) never need to import
/// `exec::tool_call_summary` directly (one canonical owner: [`tool_call_summary::ToolCallSummary`]).
pub use crate::exec::tool_call_summary::ToolCallSummary;

/// The full, terminal outcome of one `run_sync` call (arch-SA §3.4). This is always the
/// **compacted** (R-SA-043) shape: no raw per-turn messages — only the summarized fields below.
/// The one opt-out is [`Self::progress`], which [`RunOptions::include_progress`] gates exactly as
/// pi's `includeProgress` gates `Details.progress`; see that field's own doc.
///
/// `PartialEq`/`Serialize`/`Deserialize` are derived (beyond the original `Debug, Clone`) because
/// `background::ResultFile` (func-SA §4.5, R-SA-077/166) embeds `Vec<SingleResult>` directly and
/// must round-trip it through `status.json`/the terminal result file exactly like every other
/// field on that struct — a bare `Debug, Clone` shape cannot satisfy `write_atomic_json`'s
/// `T: Serialize` bound (R-SA-076).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SingleResult {
    pub agent: String,
    pub task: String,
    pub exit_code: i32,
    pub usage: Usage,
    pub model: Option<ModelId>,
    pub attempted_models: Vec<ModelId>,
    pub model_attempts: Vec<ModelAttempt>,
    pub final_output: Option<String>,
    pub structured_output: Option<serde_json::Value>,
    pub acceptance: Option<AcceptanceLedger>,
    /// R-SA-037: an intercom-style blocking detach signal was observed — bypasses acceptance,
    /// completion-guard, and output truncation entirely. Set from a REAL blocking-detach signal (the
    /// R-SA-119/120 intercom wiring is now CLOSED): a child's blocking `contact_supervisor` ask on its
    /// NDJSON stdout is detected by [`drive_attempt`], surfaced via [`crate::tui::intercom::spawn_clarify`]
    /// against the executor's `AskLock` (backed in production by the intercom companion's real broker
    /// `ClarifyChannel` threaded through [`RunOptions::clarify`]), and carried onto this flag — see
    /// [`crate::exec::fallback::AttemptSignal::detached`]'s doc comment for the full wiring trace. When
    /// no intercom channel is wired (headless / `RunOptions::clarify = None`) the drive loop still marks
    /// the attempt detached but the `AskLock` degrades to its no-live-channel fallback.
    pub detached: bool,
    /// A soft interrupt was observed (`RunOptions.interrupt` fired) — like a timeout, this
    /// terminates the fallback ladder outright without advancing, but is recorded under its own
    /// flag rather than folded into `timed_out` (R-SA-084 vs. R-SA-036 have distinct downstream
    /// consequences a caller may want to distinguish).
    pub interrupted: bool,
    pub timed_out: bool,
    /// G77/G104 — pi `SingleResult.stopped` (`shared/types.ts:879`, set by `runSubagent` at
    /// `subagent-runner.ts:2957`/`:2960`/`:2970`): this child was terminated by an explicit
    /// user/agent **stop** request, not by an interrupt, a deadline, or its own exit.
    ///
    /// A distinct flag from [`Self::interrupted`] and [`Self::timed_out`] because upstream reads
    /// all three separately and ranks them differently: `resolveSubagentResultStatus` returns
    /// `"stopped"` for it BEFORE it ever looks at `interrupted`/`success`/`exitCode`
    /// (`intercom/result-intercom.ts:32`), `chain-root-attachment.ts:87` short-circuits on it ahead
    /// of the child's own `success`, and `notify.ts:203` ORs it into the run-level stop verdict.
    ///
    /// `#[serde(default)]` + omit-when-false so a `status.json`/result file written before this
    /// field existed still round-trips (the same discipline `saved_output_path`/`control_events`
    /// follow), matching upstream's own optional `stopped?: boolean`.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub stopped: bool,
    /// pi `SingleResult.processSignal` (`shared/types.ts`, set from Node's `proc.on("close",
    /// (code, signal))` at `subagent-runner.ts:903`): the NAME of the OS signal that killed this
    /// child (`"SIGKILL"`, `"SIGTERM"`, …), or `None` on a normal exit.
    ///
    /// Carried onto the terminal result — not only onto
    /// [`crate::exec::fallback::StartupEvidence::process_signal`], where this crate already
    /// computed it — because `resolveSubagentResultStatus`'s fourth branch
    /// (`result-intercom.ts:35`) resolves an UNEXPLAINED signal death (a signal with no
    /// interrupt/timeout/stop/turn-budget to explain it, `runs/shared/process-signal.ts:5-19`) to
    /// `"stopped"`, and that branch is unreachable without this field. Same optional-on-the-wire
    /// discipline as [`Self::stopped`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_signal: Option<String>,
    /// SUBA-008 — pi `SingleResult.turnBudget?: TurnBudgetState` (`shared/types.ts:1188`, assigned
    /// by `updateTurnBudget` at `execution.ts:763`/`:773`/`:780`): the assistant-turn budget this
    /// run ran under and how it ended.
    ///
    /// `None` for every run that declared no budget, and omitted from the wire when `None`, so a
    /// `status.json`/result file written before this field existed still round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_budget: Option<crate::exec::turn_budget::TurnBudgetState>,
    /// SUBA-008 — pi `SingleResult.turnBudgetExceeded?: boolean` (`shared/types.ts:1189`, set at
    /// `execution.ts:737`): the supervisor aborted this child for blowing its turn budget.
    ///
    /// Read by `resolveSubagentResultStatus` through `isUnexplainedProcessSignal`
    /// (`result-intercom.ts:35`, `runs/shared/process-signal.ts:5-19`) — a turn-budget kill is an
    /// EXPLAINED signal death, so without this field a budget abort was misreported as `stopped`.
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub turn_budget_exceeded: bool,
    /// SUBA-008 — pi `SingleResult.wrapUpRequested?: boolean` (`shared/types.ts`, set at
    /// `execution.ts:768`/`:738`): the child was asked to wrap up because it reached the soft
    /// limit. True for a deferred or exceeded run as well, matching upstream's own derivation
    /// (`subagent-runner.ts:924`).
    #[serde(default, skip_serializing_if = "crate::exec::is_false")]
    pub wrap_up_requested: bool,
    /// SUBA-021 — pi `statusPayload.usageBudget` (`subagent-runner.ts:4411`, published onto the
    /// result at `:4471` and onto `status.json` via `async-status.ts:336`): the reported-consumption
    /// budget this run ran under and where it ended up.
    ///
    /// `None` for every run that declared no budget, and omitted from the wire when `None`, so a
    /// result file written before this field existed still round-trips. When
    /// [`crate::exec::usage_budget::UsageBudgetState::exhausted`] is set, [`Self::error`] carries
    /// [`crate::exec::usage_budget::usage_budget_exceeded_message`] — that pairing is upstream's
    /// own at `:4403-4404`, and it is what makes an exhausted budget a TERMINAL outcome rather than
    /// a note on a successful run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_budget: Option<crate::exec::usage_budget::UsageBudgetState>,
    pub error: Option<String>,
    /// pi `result.savedOutputPath` (`shared/types.ts:492`, assigned at
    /// `runs/foreground/execution.ts:963` from `resolveSingleOutput(...).savedPath`): the concrete
    /// file the R-SA-031 output-path handoff actually persisted this run's delivered output to,
    /// `None` when no `output_path` was requested, the run did not complete cleanly, or nothing
    /// was written.
    ///
    /// This is the SAME value the saved-output reference message folded into `final_output` is
    /// built from — carried as its own field because consumers need the bare path, not the prose:
    /// pi's `collectDynamicResults` emits it as a dynamic collect record's `outputPath`
    /// (`runs/shared/dynamic-fanout.ts:283`) so a later chain step can locate the file each
    /// fanned-out sibling wrote.
    ///
    /// `#[serde(default)]` + omit-when-absent so a `status.json`/result file written before this
    /// field existed still round-trips (the same discipline `control_events` below follows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_output_path: Option<String>,
    /// Summarized `{text, expandedText}` tool-call previews observed across the winning attempt's
    /// transcript — R-SA-043's "only summarized `tool_calls`" compaction requirement (pi's
    /// `ToolCallSummary[]`, `utils.ts:368-373`). Each carries a short and an expanded argument
    /// preview (pi `formatToolCall`), NOT a bare tool name. Never the raw per-turn message list.
    pub tool_calls: Vec<ToolCallSummary>,
    /// Whether [`output::truncate_output`] actually cut the delivered `final_output` (R-SA-042).
    pub output_truncated: bool,
    /// pi `result.controlEvents` (`execution.ts:1112`/`:1260`): every live-control event the
    /// WINNING attempt raised, in raise order, plus the post-settlement completion-guard raise
    /// (`:1234`). Empty for a run whose control config is disabled, whose `notifyOn` excluded both
    /// classes, or that simply never tripped a threshold — which is why it is `#[serde(default)]`
    /// and omitted from the wire when empty: a persisted `status.json`/result file written before
    /// this field existed still round-trips.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_events: Vec<crate::exec::control::ControlEvent>,
    /// pi `SingleResult.progress` (`pi-subagents/src/shared/types.ts:844`) — this run's own
    /// `AgentProgress` snapshot, and the home for what `includeProgress` gates.
    ///
    /// **`None` unless [`RunOptions::include_progress`] is `Some(true)`.** That is the whole
    /// contract: R-SA-043's compaction stays the default, `includeProgress` is its documented
    /// opt-out (pi `progress: params.includeProgress ? allProgress : undefined`,
    /// `runs/foreground/subagent-executor.ts:3071` for PARALLEL and `:3406` for SINGLE), and with
    /// the flag off or omitted this field skips serialization entirely so a returned/persisted
    /// `SingleResult` is byte-for-byte what it was before the field existed.
    ///
    /// When populated it has always been through
    /// [`crate::tui::events::LiveProgressSnapshot::compact_completed`] (pi
    /// `compactCompletedProgress` via `compactForegroundDetails`, `shared/utils.ts:414-421`), which
    /// for every SETTLED status empties the two per-run growth terms — the tool-history ring and
    /// the recent-output tail.
    ///
    /// **The one exception is upstream's, not this port's**: pi's `compactCompletedProgress` opens
    /// with `if (progress.status === "running") return progress;`, and an interrupt-PAUSED run is
    /// precisely the case pi leaves at `"running"` (`execution.ts:828`, returning at `:861` before
    /// the `completed`/`failed` assignment at `:907`). Such a snapshot keeps its rings — which is
    /// the point, since the caller is expected to resume the run. Both rings are bounded at PUSH
    /// time in this port ([`crate::tui::events::RECENT_TOOLS_CAP`] entries,
    /// [`RECENT_OUTPUT_CAP`] lines of at most [`RECENT_OUTPUT_LINE_CHARS`] chars each), so even
    /// that shape is O(1) in the child's chattiness. pi bounds neither on this path.
    ///
    /// **[CYRUP-DELTA] on placement.** pi carries the array one level UP, on
    /// `Details.progress: AgentProgress[]` (`shared/types.ts:908`), assembled as `allProgress` from each
    /// child's own `result.progress` (`subagent-executor.ts:3424,3444,3793,3819` @v0.43.0), and blanks
    /// `SingleResult.progress` in the returned `results` (`compactForegroundResult`,
    /// `utils.ts:404-412`). cyrup's SINGLE-mode tool `details` IS the serialized `SingleResult`
    /// (`extension.rs::route_single`) rather than a `Details` wrapper, so the snapshot lands on the
    /// field pi already declares for it and surfaces at the same JSON path (`details.progress`) a
    /// pi caller reads for a SINGLE run — one snapshot rather than a one-element array.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<crate::tui::events::LiveProgressSnapshot>,
}

// ================================================================================================
// AgentProgress: the live per-attempt fold (R-SA-027/028)
// ================================================================================================

/// The live, in-memory progress state one attempt accumulates as its child's NDJSON stdout is
/// consumed (R-SA-027/028). This is the "still-running" shape architecture.md §4.3/R-SA-043
/// contrasts with [`SingleResult`]'s own compacted, terminal shape — never returned to
/// `run_sync`'s own caller directly; folded down into `SingleResult`'s summarized fields once the
/// attempt (and then the whole fallback ladder) settles.
#[derive(Debug, Clone, Default)]
pub struct AgentProgress {
    /// Running additive [`Usage`] total for THIS attempt alone (cross-attempt aggregation, which
    /// is additive across the whole ladder including failed attempts, is
    /// [`fallback::run_fallback_ladder`]'s own separate concern, R-SA-040) — every `MessageEnd`
    /// event's `usage` is folded in here as it is observed (R-SA-027).
    pub usage: Usage,
    /// Number of `ToolExecutionStart` events observed so far this attempt (R-SA-027).
    pub tool_count: u32,
    /// The most recently started tool's name, if any tool call has started and none more recent
    /// has superseded it (R-SA-027's "set `current_tool`").
    pub current_tool: Option<String>,
    /// Bounded ring buffer of the child's recent OUTPUT TEXT, oldest evicted first once
    /// [`RECENT_OUTPUT_CAP`] is exceeded (R-SA-028) — pi `progress.recentOutput`
    /// (`shared/types.ts:575`), seeded with the fallback ladder's attempt notes
    /// (`recentOutput: [...shared.attemptNotes]`, `runs/foreground/execution.ts:366`) and appended
    /// to by `appendRecentOutput` on each assistant `message_end` and each `tool_execution_end`.
    ///
    /// This holds EXTRACTED, human-readable text (`extractTextFromContent` over the message
    /// `content` / tool `result`), never the raw NDJSON envelope. That distinction is load-bearing
    /// rather than cosmetic: R-SA-028 describes "recent output" as a rendering/log concern, the
    /// only consumer that publishes it —
    /// [`SingleResult::progress`] via [`AgentProgress::snapshot`] — surfaces it to a caller as
    /// pi's `AgentProgress.recentOutput`, and a raw `{"type":"message_end","message":{...}}` line
    /// is both unrenderable and (before [`RECENT_OUTPUT_LINE_CHARS`]) an unbounded blob of the
    /// whole turn.
    pub recent_output: VecDeque<String>,
    /// Every `MessageEnd` event observed this attempt, in chronological (parse) order — the exact
    /// input [`output::extract_final_output`] (R-SA-029) needs, and what
    /// [`completion_guard::has_mutation_tool_call`]/[`evaluate_completion_mutation_guard`]
    /// (R-SA-034) scans alongside `tool_events` below.
    pub message_end_events: Vec<SubagentEvent>,
    /// Every `ToolExecutionEnd` event observed this attempt, in chronological order — feeds
    /// [`completion_guard::has_mutation_tool_call`] (R-SA-034) and the summarized `tool_calls`
    /// list [`SingleResult`] carries (R-SA-043).
    pub tool_end_events: Vec<SubagentEvent>,
    /// The full parsed transcript of every recognized event this attempt observed, in
    /// chronological order — needs more than the two narrower vectors above. `run_sync` reads it
    /// directly alongside `message_end_events`/`tool_end_events` for its R-SA-029/034 wiring.
    /// R-SA-030 (structured output) is deliberately NOT among its consumers any more: SUBA-S01's
    /// residual pass removed the transcript scan, because pi's structured value only ever travels
    /// through the child's capture file (`structured-output.ts:156-173`), never through prose.
    pub all_events: Vec<SubagentEvent>,
    /// The short argument preview captured when [`Self::current_tool`] STARTED (pi
    /// `progress.currentToolArgs = extractToolArgsPreview(toolArgs)`,
    /// `runs/foreground/execution.ts:794`), copied onto the [`Self::recent_tools`] entry that call
    /// produces when it ends and cleared alongside `current_tool` (`:811-812`).
    pub current_tool_args: String,
    /// Bounded ring of finished tool calls (pi `progress.recentTools`, `shared/types.ts:574`),
    /// oldest evicted first past [`crate::tui::events::RECENT_TOOLS_CAP`] — the same
    /// bound-at-push discipline (and the same rationale) as
    /// [`crate::tui::events::LiveProgressFold`]'s own ring.
    pub recent_tools: VecDeque<crate::tui::events::RecentToolCall>,
    /// When this attempt's clock started, for `durationMs` (pi's `startTime` local, captured before
    /// the child spawns and read back at `execution.ts:744`). `None` in a `Default`-constructed
    /// fold, which reports a zero duration.
    pub started_at: Option<std::time::Instant>,
}

impl AgentProgress {
    /// Fold one parsed [`SubagentEvent`] into this progress state (R-SA-027). Every `MessageEnd`
    /// event's usage is accumulated additively (never last-wins — mirrors
    /// [`fallback::add_usage`]'s own contract, restated here at the per-attempt granularity); every
    /// `ToolExecutionStart` increments `tool_count` and sets `current_tool`.
    ///
    /// Also feeds [`Self::recent_output`], on exactly pi's two append sites: an ASSISTANT
    /// `message_end`'s extracted content text (`appendRecentOutput(progress,
    /// assistantText.split("\n").slice(-10))`, `runs/foreground/execution.ts:651` @v0.34.0) and a
    /// finished tool call's extracted result text (`:670`). **[CYRUP-DELTA]** pi reads the result
    /// text off a separate `tool_result_end` event; cyrup's wire has no such event and carries the
    /// same payload on `ToolExecutionEnd.result` — the delta [`crate::exec::ndjson::SubagentEvent`]
    /// already documents, and the same one [`crate::tui::events::LiveProgressFold`] makes.
    pub fn record_event(&mut self, event: SubagentEvent) {
        if let Some(usage) = event.assistant_usage() {
            crate::exec::fallback::add_usage(&mut self.usage, &usage);
        }
        match &event {
            SubagentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                self.tool_count += 1;
                self.current_tool = Some(tool_name.clone());
                // pi `execution.ts:794`.
                self.current_tool_args =
                    crate::exec::tool_call_summary::extract_tool_args_preview(args);
            }
            SubagentEvent::MessageEnd { message } => {
                // pi `execution.ts:650-651` @v0.34.0 — ASSISTANT turns only; a user/tool-role
                // `message_end` contributes nothing to the rendered output tail.
                if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant") {
                    let text = message
                        .get("content")
                        .map(crate::tui::events::extract_event_text)
                        .unwrap_or_default();
                    self.append_recent_output(&text);
                }
                self.message_end_events.push(event.clone());
            }
            SubagentEvent::ToolExecutionEnd { result, .. } => {
                // pi `execution.ts:663-664,670` @v0.34.0.
                let result_text = crate::tui::events::extract_event_text(result);
                self.append_recent_output(&result_text);
                // pi pushes onto `recentTools` ONLY when a `currentTool` was in flight
                // (`execution.ts:804-810`), then clears it and its args (`:811-812`).
                if let Some(tool) = self.current_tool.take() {
                    if self.recent_tools.len() >= crate::tui::events::RECENT_TOOLS_CAP {
                        self.recent_tools.pop_front();
                    }
                    self.recent_tools
                        .push_back(crate::tui::events::RecentToolCall {
                            tool,
                            args: std::mem::take(&mut self.current_tool_args),
                            end_ms: u64::try_from(crate::background::now_epoch_millis_pub())
                                .unwrap_or(0),
                        });
                }
                self.current_tool_args.clear();
                self.tool_end_events.push(event.clone());
            }
            _ => {}
        }
        self.all_events.push(event);
    }

    /// Number of ASSISTANT `message_end` events observed this attempt — pi's `progress.turnCount`,
    /// which it keeps in lockstep with `result.usage.turns` and increments only for an assistant
    /// message (`runs/foreground/execution.ts:825-827`).
    #[must_use]
    pub fn turn_count(&self) -> u32 {
        let turns = self
            .message_end_events
            .iter()
            .filter(|event| match event {
                SubagentEvent::MessageEnd { message } => {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                }
                _ => false,
            })
            .count();
        u32::try_from(turns).unwrap_or(u32::MAX)
    }

    /// Milliseconds elapsed since this attempt's clock started (pi `Date.now() - startTime`,
    /// `runs/foreground/execution.ts:1177`); `0` for a fold whose clock was never started.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.started_at
            .map(|start| u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// Append one chunk of child output text to the bounded `recent_output` ring (R-SA-028) — a
    /// 1:1 port of pi's `appendRecentOutput` (`runs/foreground/execution.ts:211-217`) fused with
    /// the `.split("\n").slice(-10)` every call site applies to its argument (`:850,869`).
    ///
    /// Exactly pi's three rules, in pi's order: keep only the last
    /// [`RECENT_OUTPUT_TAIL_LINES`] lines of THIS chunk, drop the blank ones
    /// (`lines.filter((line) => line.trim())`), then evict from the front until the ring is back
    /// within [`RECENT_OUTPUT_CAP`]. Plus the one [CYRUP-DELTA] documented on
    /// [`RECENT_OUTPUT_LINE_CHARS`]: each surviving line is truncated to that many `char`s here
    /// rather than at snapshot time.
    ///
    /// pi keeps the ORIGINAL (untrimmed) line text and only *tests* `line.trim()` for emptiness,
    /// so leading indentation survives — reproduced here rather than pushing the trimmed form.
    pub fn append_recent_output(&mut self, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        let tail_start = lines.len().saturating_sub(RECENT_OUTPUT_TAIL_LINES);
        for line in lines.into_iter().skip(tail_start) {
            if line.trim().is_empty() {
                continue;
            }
            if self.recent_output.len() >= RECENT_OUTPUT_CAP {
                self.recent_output.pop_front();
            }
            self.recent_output.push_back(bound_output_line(line));
        }
    }

    /// Summarized `{text, expandedText}` tool-call previews observed this attempt (R-SA-043's
    /// compaction target), in chronological (request) order — one entry per `ToolExecutionStart`
    /// event, matching pi's `extractToolCallSummaries`, which walks the assistant messages'
    /// `toolCall` parts (`utils.ts:309-326`). Sourced from `ToolExecutionStart` (which carries the
    /// requested `args`), NOT `ToolExecutionEnd` (which carries only the result): a tool-call
    /// preview renders the arguments the model requested, and includes a call that started but
    /// never completed, exactly like pi's message-part walk. Repeats of the same tool are preserved
    /// (one entry per real call).
    #[must_use]
    pub fn summarized_tool_calls(&self) -> Vec<ToolCallSummary> {
        self.all_events
            .iter()
            .filter_map(|event| match event {
                SubagentEvent::ToolExecutionStart {
                    tool_name, args, ..
                } => Some(ToolCallSummary::from_call(tool_name, args)),
                _ => None,
            })
            .collect()
    }

    /// Project this fold into pi's `AgentProgress` wire shape
    /// ([`crate::tui::events::LiveProgressSnapshot`]) — the bridge `includeProgress` gates.
    ///
    /// pi needs no such projection because it has ONE object: `runSingleAttempt` builds a single
    /// mutable `progress` literal carrying both the launch context and the live counters
    /// (`runs/foreground/execution.ts:258-270` @v0.34.0), mutates its `status`/`durationMs`/
    /// `error`/`failedTool` at settle (`:907-913`), and hands that same object out as
    /// `result.progress` (`:271`). cyrup splits the two halves — the counters accumulate here, per
    /// ATTEMPT, while the launch context and the post-ladder settled facts are `run_sync` locals —
    /// so [`ProgressSnapshotInput`] carries the second half in and this method fuses them.
    ///
    /// The result is the FULL (still-uncompacted) shape, exactly like pi's object at `:271`. A
    /// caller publishing a settled run's progress must then run it through
    /// [`crate::tui::events::LiveProgressSnapshot::compact_completed`], which is what pi's
    /// `compactForegroundDetails` does one level up (`shared/utils.ts:414-421`).
    #[must_use]
    pub fn snapshot(
        &self,
        input: ProgressSnapshotInput<'_>,
    ) -> crate::tui::events::LiveProgressSnapshot {
        crate::tui::events::LiveProgressSnapshot {
            index: input.index,
            agent: Some(input.agent.to_string()),
            status: input.status,
            activity_state: input.activity_state,
            task: input.task.to_string(),
            skills: input.skills,
            // pi `progress.currentTool` survives into the returned object; `record_event` `take`s
            // it on `tool_execution_end`, so it is `Some` only for a call still in flight.
            current_tool: self.current_tool.clone(),
            recent_tools: self.recent_tools.iter().cloned().collect(),
            tool_count: self.tool_count,
            turn_count: self.turn_count(),
            // pi `progress.tokens = result.usage.input + result.usage.output`
            // (`execution.ts:646` @v0.34.0) — NOT the cache-read/write terms.
            tokens: self.usage.input.saturating_add(self.usage.output),
            model: input.model,
            thinking: input.thinking,
            input_tokens: Some(self.usage.input),
            output_tokens: Some(self.usage.output),
            duration_ms: self.duration_ms(),
            error: input.error.clone(),
            // pi `if (result.error) { …; if (progress.currentTool) progress.failedTool =
            // progress.currentTool; }` (`execution.ts:909-913` @v0.34.0) — BOTH conditions, so a
            // clean run names no failed tool and a failure with nothing in flight names none
            // either.
            failed_tool: input
                .error
                .as_ref()
                .and_then(|_| self.current_tool.clone()),
            recent_output: self.recent_output.iter().cloned().collect(),
        }
    }
}

/// The half of pi's `progress` object that lives OUTSIDE [`AgentProgress`] in this port: the
/// launch-time descriptive fields pi writes into the literal at construction
/// (`runs/foreground/execution.ts:258-270` @v0.34.0) and the settled facts it assigns after the
/// child closes (`:907-913`). Every field is a `run_sync` local by the time
/// [`AgentProgress::snapshot`] is called.
///
/// A struct rather than nine positional arguments so the call site names each value (and so clippy's
/// `too_many_arguments` stays quiet).
pub struct ProgressSnapshotInput<'a> {
    /// pi `progress.index` ← `options.index ?? 0` (`execution.ts:259`); cyrup
    /// [`RunOptions::child_index`].
    pub index: u32,
    /// pi `progress.agent` ← `agent.name` (`:260`).
    pub agent: &'a str,
    /// pi `progress.task` ← the (post-fork-wrap) task text (`:262`).
    pub task: &'a str,
    /// pi `progress.skills` ← `shared.resolvedSkillNames` (`:263`) — the names that actually
    /// RESOLVED, `None` when none did (pi `resolvedSkills.length > 0 ? … : undefined`,
    /// `:1481` @HEAD).
    pub skills: Option<Vec<String>>,
    /// pi `progress.model` ← `modelArg` (`:267`), i.e. the winning model id WITH the thinking
    /// suffix [`apply_thinking_suffix`] appends.
    pub model: Option<String>,
    /// pi `progress.thinking` ← `resolvedThinking` (`:268`).
    pub thinking: Option<String>,
    /// pi's settled `progress.status` (`:907` / `:344` for a detach / `:828` for an interrupt).
    pub status: crate::tui::events::LiveProgressStatus,
    /// pi `progress.activityState`, owned by the live-control state machine and cleared on
    /// interrupt (`:832,854`); cyrup reads it back off the winning attempt's
    /// [`crate::exec::control::ControlMonitor`].
    pub activity_state: Option<crate::background::ActivityState>,
    /// pi `progress.error` ← the FINAL `result.error`, after every post-settlement gate
    /// (structured-output, completion guard, acceptance) has had its say (`:910`, plus the
    /// acceptance-failure assignment at `:1233-1234`).
    pub error: Option<String>,
}

// ================================================================================================
// SubagentSpawner: the seam production spawning goes through (mirrors AttemptRunner's own
// production-vs-test seam, one level down at the real-subprocess boundary)
// ================================================================================================

/// Everything one attempt's spawn needs beyond what [`AgentConfig`]/[`RunOptions`] already carry —
/// factored out so [`SpawnedChildAttemptRunner`] can build a [`ChildSpawnSpec`] without repeating
/// argv/env assembly inline in `run_attempt` itself.
///
/// Public (with [`build_attempt_spawn_plan`]) as the P-5 cross-crate parity seam: the child env
/// this plan carries is a CONTRACT the permission companion depends on
/// (`cyrup-permission-system/tests/forwarding_spawn_env.rs` drives a real child process off THIS
/// overlay to prove a subagent's `ask` reaches the parent's human), and a downstream proof that
/// re-typed the env by hand would not have caught PERM-001 — the gate reading a key the spawn path
/// never wrote — which is precisely the class of bug the seam exists to make visible.
pub struct AttemptSpawnPlan {
    /// The fully-assembled child spawn description: binary, argv, task arg, env overlay, cwd.
    pub spec: ChildSpawnSpec,
    /// SUBA-045 — where this attempt told the child to write its tool-availability diagnostic
    /// (pi's `toolDiagnosticPath`, `pi-args.ts:610-616`), so the parent can read it back at settle.
    ///
    /// `None` whenever the env var was not written, which is upstream's own gate: an agent with no
    /// explicit `tools:` allowlist requires nothing, so there is nothing to be missing. Returned
    /// alongside the spec rather than re-derived from the overlay so the read side cannot drift
    /// from the write side.
    pub tool_diagnostic_path: Option<PathBuf>,
}

/// The reasoning-level suffixes [`apply_thinking_suffix`] recognizes on a model id (pi-subagents
/// `THINKING_LEVELS`, `src/shared/model-info.ts:1`; `max` added upstream in 747de75). Includes
/// `off` — a value cyrup-core's closed on-only
/// `ThinkingLevel` enum cannot itself represent, but which the string-level suffix check must still
/// recognize so a model id that already ends `:off` is never double-suffixed.
const THINKING_LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

/// Child env flag: whether the subagent inherits the parent's project-context files
/// (`AGENTS.md`/`CLAUDE.md`) — pi `PI_SUBAGENT_INHERIT_PROJECT_CONTEXT` (`runs/shared/pi-args.ts:215` @v0.34.0).
///
/// Aliased to the READER's declaration ([`crate::prompt_runtime`], which acts on it child-side in
/// `before_agent_start`) rather than re-spelled here: the two spellings drifting apart would
/// silently restore the write-only-flag bug this alias exists to prevent.
const INHERIT_PROJECT_CONTEXT_ENV: &str = crate::prompt_runtime::INHERIT_PROJECT_CONTEXT_ENV;

/// Child env flag: whether the subagent inherits skills discovery — pi
/// `PI_SUBAGENT_INHERIT_SKILLS` (`runs/shared/pi-args.ts:200`). Same aliasing rule as above.
const INHERIT_SKILLS_ENV: &str = crate::prompt_runtime::INHERIT_SKILLS_ENV;

/// The MCP adapter's direct-tool-allowlist env (pi keeps this un-namespaced, `runs/shared/pi-args.ts:216-220`):
/// the comma-joined `mcp:` selectors, or [`MCP_DIRECT_TOOLS_NONE_SENTINEL`] when the agent declares
/// no direct MCP tools at all (so the child's adapter can distinguish "none selected" from "env
/// unset / inherited").
const MCP_DIRECT_TOOLS_ENV: &str = "MCP_DIRECT_TOOLS";

/// The canonical parent-session anchor (proposed **R-SA-P1**, port doc §4 P-4) — the cyrup analog of
/// pi's `PI_AGENT_ROUTER_PARENT_SESSION_ID` (`permission-forwarding.ts:9`). Injected into every
/// child's spawn env overlay ([`build_attempt_spawn_plan`]) as the LAUNCHING session's own id
/// (obtained via P-2 [`cyrup_ext::host::HostServices::session_id`], captured once at the root
/// orchestrator's `SessionStart`), so the permission companion's child→parent ask-forwarding spool
/// can address the parent's inbox. `pub` (P-5) so that companion reads it cross-crate. Value
/// precedence at the spawn site: explicit ([`RunOptions::parent_session_id`]) → inherited (this
/// process's own `CYRUP_SUBAGENT_PARENT_SESSION`, so a `DEPTH>0` child keeps threading the root's
/// anchor rather than overwriting it) → empty (omitted).
pub const PARENT_SESSION_ENV_VAR: &str = "CYRUP_SUBAGENT_PARENT_SESSION";

/// The resolved persona/agent NAME the child runs as (port doc §4, permission input (1) — pi
/// `resolveAgentName`, `pi-permission-system/src/index.ts:2033-2047` @v0.7.1). cyrup spawns a subagent as a SEPARATE process that IS
/// its persona for the whole lifetime, so — unlike pi's in-process `active_agent` session entry /
/// `<active_agent>` prompt tag — the name is captured ONCE at the spawn site and threaded to the
/// child as this env var (the exact equivalent for cyrup's process-per-subagent model). The child's
/// permission companion reads it via `std::env::var` (mirroring how it already reads the sibling
/// `CYRUP_SUBAGENT_PARENT_SESSION`/`CYRUP_SUBAGENT_CHILD` vars) so its `agent` + `projectAgent` policy
/// layers enforce for the named persona; a top-level (non-subagent) process never has it set, so the
/// name normalizes to `None` there — pi's normalized-`""` top-level behavior (global + project layers
/// still enforce). Set only by the spawn overlay in [`build_attempt_spawn_plan`], the ONLY non-test
/// `env_overlay` construction site (covers foreground + background spawns). `pub` (P-5) so the
/// permission companion reads it cross-crate.
pub const AGENT_NAME_ENV_VAR: &str = "CYRUP_SUBAGENT_AGENT_NAME";

/// pi's `__none__` sentinel for [`MCP_DIRECT_TOOLS_ENV`] when no direct MCP tools are declared.
const MCP_DIRECT_TOOLS_NONE_SENTINEL: &str = "__none__";

/// The child flag carrying a `SystemPromptMode::Replace` persona body (pi `runs/shared/pi-args.ts:165`'s
/// `"--system-prompt"`; the host side is `cyrup/src/cli.rs`'s `#[arg(long = "system-prompt")]`).
const SYSTEM_PROMPT_FLAG: &str = "--system-prompt";

/// The child flag carrying a `SystemPromptMode::Append` persona body (pi `runs/shared/pi-args.ts:165`'s
/// `"--append-system-prompt"`; repeatable host-side, joined with `\n`).
const APPEND_SYSTEM_PROMPT_FLAG: &str = "--append-system-prompt";

/// pi `applyThinkingSuffix` (`runs/shared/pi-args.ts:186-200`): append `:<thinking>` to a model id, unless the
/// model already ends with a recognized `:<level>` suffix (leave it untouched) or either input is
/// absent (return the model as-is). Operates on strings so the exact pi rule — including the `off`
/// level a closed on-only enum cannot itself carry — is reproduced verbatim; the agent's own OPEN
/// `thinking` string (`AgentConfig::thinking`) is passed straight through, so an explicit `off`
/// yields `<model>:off`.
#[must_use]
pub fn apply_thinking_suffix(model: Option<&str>, thinking: Option<&str>) -> Option<String> {
    let (Some(model), Some(thinking)) = (model, thinking) else {
        return model.map(str::to_string);
    };
    // pi guards on truthiness (`if (!model || !thinking) ...`), so an empty thinking string is a
    // no-op — mirror that here so a degenerate `Some("")` never produces a trailing bare `:`.
    if thinking.is_empty() {
        return Some(model.to_string());
    }
    if let Some((_, suffix)) = model.rsplit_once(':')
        && THINKING_LEVELS.contains(&suffix)
    {
        return Some(model.to_string());
    }
    Some(format!("{model}:{thinking}"))
}

/// pi `splitKnownThinkingSuffix` (`shared/model-info.ts:39-47`): split a model id on its last `:`
/// **only when the trailing segment is a recognized [`THINKING_LEVELS`] entry**, returning
/// `(base_model, thinking_suffix_including_colon)`.
///
/// Distinct from `extension.rs`'s `split_thinking_suffix` (pi `splitThinkingSuffix`,
/// `model-fallback.ts:13-19`), which splits on the last `:` unconditionally: a model id like
/// `openai/gpt-5:preview` keeps its `:preview` here (it is part of the id, not a reasoning level)
/// but would be truncated by the unconditional split. Scope matching MUST use this stricter form,
/// exactly as `model-scope.ts` does.
#[must_use]
pub fn split_known_thinking_suffix(model: &str) -> (&str, &str) {
    let Some(idx) = model.rfind(':') else {
        return (model, "");
    };
    let suffix = model.get(idx + 1..).unwrap_or("");
    if !THINKING_LEVELS.contains(&suffix) {
        return (model, "");
    }
    (model.get(..idx).unwrap_or(model), model.get(idx..).unwrap_or(""))
}

/// Append `item` to `vec` only if not already present — the order-preserving de-duplication pi
/// achieves with `[...new Set(...)]` over the extension-path list (`runs/shared/pi-args.ts:146,150` @v0.34.0).
fn push_unique(vec: &mut Vec<String>, item: String) {
    if !vec.contains(&item) {
        vec.push(item);
    }
}

/// Build the argv + env overlay for one attempt against `model` (R-SA-024/047/048/054; pi
/// `buildPiArgs`, `runs/shared/pi-args.ts:514-787`).
///
/// Argv (flags in any order, task prompt last): `--print`, `--mode`, `json`; `--model
/// <apply_thinking_suffix(model, agent.thinking)>` (T4 thinking-suffix); the tool-allowlist flag —
/// `--tools <comma-list>` (the agent's declared builtins plus any resolved direct-MCP tool names)
/// when the agent pinned a non-empty allowlist, `--no-tools` when it pinned an EMPTY one, and
/// nothing at all when it pinned none (pi's `explicitToolAllowlist` gate, `runs/shared/pi-args.ts:389-393,549-555`); the agent's
/// extension threading (`--no-extensions` + `--extension <path>` allowlist when `agent.extensions`
/// is `Some`, else `--extension <path>` for tool-extension/child-only paths with discovery left
/// on); `--no-skills` when the agent does not inherit skills; `--system-prompt <path>` /
/// `--append-system-prompt <path>` per `agent.system_prompt_mode` when the body is
/// non-empty (TWO argv elements naming a `0600` spill file — SUBA-030, see below); pi's full session branch
/// (`runs/shared/pi-args.ts:100-112`) — either `--session <path>` when `opts.fork_context` resolved a session
/// file path, or else `--no-session` unless [`RunOptions::session_dir`]/[`RunOptions::share`]
/// enable sessions plus `--session-dir <dir>` for an explicit directory; then the task prompt last
/// (via [`ChildSpawnSpec::resolve_task_arg`], R-SA-047's `@<tempfile>` overflow rule).
///
/// Env overlay carries the child-ROLE pair
/// ([`crate::spawn::nested_events::child_role_env`] — pi `runs/shared/pi-args.ts:329-330`), the incremented
/// depth envelope (R-SA-054), the run sentinel, the agent's inherit flags
/// ([`INHERIT_PROJECT_CONTEXT_ENV`]/[`INHERIT_SKILLS_ENV`]), and the raw direct-MCP selector list
/// ([`MCP_DIRECT_TOOLS_ENV`], or the `__none__` sentinel).
///
/// The agent's own persona prose (`agent.system_prompt_body`) is delivered here as
/// `--system-prompt <spill path>` (`SystemPromptMode::Replace`) or
/// `--append-system-prompt <spill path>`
/// (`SystemPromptMode::Append`) — pi `runs/shared/pi-args.ts:159-165` (v0.34.0), where the mode picks the flag
/// and the body always ships. Nothing child-side re-resolves the persona from
/// [`AGENT_NAME_ENV_VAR`] (that var is read only by the permission companion), so this argv pair is
/// the ONLY channel the persona has.
///
/// The `<available_skills>` pointer block remains composed into `task` BEFORE this function is
/// called — see [`build_task_text`] — rather than being folded into the persona body the way pi's
/// `execution.ts:1054-1056` composes it, so that a `Replace`-mode persona cannot suppress the
/// orchestrator's own scaffolding.
///
/// The output-path override (R-SA-024) travels on BOTH surfaces, exactly as upstream sends it. The
/// task-side half (`injectSingleOutputInstruction`, the `**Output:**` header) is composed into
/// `task` by [`build_task_text`], mirroring `subagent-executor.ts:3674`. The system-prompt half
/// ([`crate::exec::output::inject_output_path_system_prompt`], the `Runtime output path override:`
/// header) is composed onto the persona body HERE, mirroring `execution.ts:1443` — the LAST of the
/// three folds upstream applies to `systemPrompt` in a row. All three are reproduced below in
/// upstream's order: the agent-memory block (`execution.ts:1438-1441`), then the project-local
/// refinement overlay ([`crate::exec::agent_refinements::append_agent_refinement_overlay`],
/// `execution.ts:1442`), then the output-path override. Both output-path surfaces are keyed on the
/// presence of an output PATH alone, never on [`RunOptions::output_mode`], and both are
/// capability-aware.
///
/// SUBA-030 — the composed prompt is written to a `0600` file in `temp_dir` and the flag carries
/// its PATH as two argv elements, which is pi's literal mechanism at
/// `runs/shared/pi-args.ts:570-585` @v0.43.0 (unconditional `mkdtemp` + `writeFileSync(promptPath,
/// …, { mode: 0o600 })` + `args.push(flag, promptPath)`), read back child-side by
/// `resolvePromptInput` (`resource-loader.ts:53-68`). **The earlier `[CYRUP-DELTA]` here asserted
/// that `cyrup`'s own flags "take LITERAL text … no path resolution anywhere" and passed the body
/// inline on that basis; that is false at HEAD** — `crates/cyrup/src/cli.rs:511` runs
/// `--system-prompt` through `resolve_prompt_input` (`:680-704`, an existing path is read from
/// disk) and `:423-439` does the same for every `--append-system-prompt` entry, both pinned by
/// `system_prompt_reads_the_file_when_the_token_is_an_existing_path`. The `=`-joined single-argv
/// form went with the inline delivery, and so did its reason: clap refuses a DETACHED value
/// beginning with `-`, and an absolute temp path cannot begin with one.
///
/// **[CYRUP-DELTA]** the one surviving difference is the empty case. A body that is still empty
/// AFTER the memory block, the refinement overlay and the output-path override have been composed
/// onto it emits NO flag at all, where pi's guard is `!== undefined && !== null` (`:570`) and an
/// empty string still writes a prompt file and still passes the path. Emitting the flag over an
/// empty file here would blank the child's assembled prompt instead of leaving it alone. An agent
/// with an output path configured composes a non-empty override and therefore DOES ship the flag,
/// which is upstream's own rationale for why its value "is never meaningfully empty".
///
/// **[CYRUP-DELTA]** pi prefixes the file with `<active_agent name="…"/>` (`:576-578`) so its
/// in-process `@gotgenes/pi-permission-system` can resolve per-agent policy from the prompt text.
/// cyrup's permission companion resolves the same name from `CYRUP_SUBAGENT_AGENT_NAME` instead
/// (`cyrup-permission-system/src/extension/env.rs`, `resolve_agent_name_from_env`), because
/// cyrup's subagent is a separate PROCESS that IS its persona for its whole lifetime, so the tag
/// would be a second, weaker channel for a fact the env already carries.
///
/// # Errors
///
/// Propagates [`ChildSpawnSpec::resolve_task_arg`]'s error (temp-file creation failure for an
/// over-threshold task).
pub fn build_attempt_spawn_plan(
    agent: &AgentConfig,
    model: &ModelId,
    task_text: &str,
    opts: &RunOptions,
    depth: DepthEnvelope,
    temp_dir: &std::path::Path,
    // SUBA-S01 (pi `runs/shared/pi-args.ts:246-250`): the run's capture runtime, whose two paths become the
    // child's structured-output env overlay. `None` = the step declared no `outputSchema`, and the
    // child then registers no `structured_output` tool at all.
    structured_runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
) -> Result<AttemptSpawnPlan, SubagentError> {
    build_attempt_spawn_plan_with_read_requirement(
        agent,
        model,
        task_text,
        opts,
        depth,
        temp_dir,
        structured_runtime,
        false,
    )
}

/// SUBA-014 — [`build_attempt_spawn_plan`] plus pi's `requireReadTool` input.
///
/// pi's `resolvePiLaunchToolPlan` takes `requireReadTool?: boolean`
/// (`pi-subagents/src/runs/shared/pi-args.ts:118,208` @v0.43.0) and every one of its seven live
/// setters derives it from "did any skill actually resolve" — `Boolean(shared.resolvedSkillNames
/// ?.length)` (`runs/foreground/execution.ts:322,357`), `Boolean(resolvedSkills.length)`
/// (`runs/background/async-execution.ts:731,1324`), `Boolean(step.skills?.length)`
/// (`runs/background/subagent-runner.ts:1328,1366`), `resolvedSkills.resolved.length > 0`
/// (`api/preflight.ts:277`). The parameter is OPTIONAL upstream and falsy when omitted, which is
/// exactly what the seven-argument [`build_attempt_spawn_plan`] forwards.
pub fn build_attempt_spawn_plan_with_read_requirement(
    agent: &AgentConfig,
    model: &ModelId,
    task_text: &str,
    opts: &RunOptions,
    depth: DepthEnvelope,
    temp_dir: &std::path::Path,
    structured_runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
    // SUBA-014 (pi `runs/shared/pi-args.ts:365-370` @v0.43.0): the run resolved at least one skill,
    // so the child is about to be told (`discovery/skills.rs`'s proactive block) to "use the read
    // tool to load a skill's file" and MUST therefore be granted `read` even when the agent pinned
    // an explicit `tools:` list that omits it.
    require_read_tool: bool,
) -> Result<AttemptSpawnPlan, SubagentError> {
    // SUBA-021 — the CAPABILITY CEILING preflight (pi `resolveCurrentSubagentCapabilityCeiling` +
    // `assertAgentAllowedByCapabilityCeiling`, `runs/shared/capability-ceiling.ts:168`/`:183`).
    //
    // Resolved FIRST, before any argv or env is built, because a ceiling is an upper bound on what
    // this subtree may do and the only useful place to enforce it is before a child exists. The
    // resolution intersects the INHERITED ceiling (this process's own
    // `CYRUP_SUBAGENT_CAPABILITY_CEILING_V1`, which a parent wrote when it spawned us) with every
    // ceiling registered for this session, so it can only ever tighten as the tree deepens.
    //
    // Both arms are fail-CLOSED: a malformed inherited ceiling is an error, not "unbounded".
    let capability_ceiling = crate::exec::capability_ceiling::resolve_current_capability_ceiling(
        opts.parent_session_id.as_deref(),
    )
    .map_err(SubagentError::CapabilityCeilingViolation)?;
    crate::exec::capability_ceiling::assert_agent_allowed(
        agent.name.as_str(),
        capability_ceiling.as_ref(),
    )
    .map_err(SubagentError::CapabilityCeilingViolation)?;

    let command = crate::spawn::resolve_spawn_command();

    // T4 (pi `applyThinkingSuffix`): the per-attempt model id, suffixed with the agent's frontmatter
    // reasoning level (`:high` etc.) unless it already carries a recognized `:<level>` suffix.
    let model_arg = apply_thinking_suffix(Some(model.as_str()), agent.thinking.as_deref())
        .unwrap_or_else(|| model.as_str().to_string());

    let mut args: Vec<String> = vec![
        "--print".to_string(),
        "--mode".to_string(),
        "json".to_string(),
        "--model".to_string(),
        model_arg,
    ];

    // pi `splitToolList` already ran at discovery time, so a `mcp:`-prefixed entry is a
    // `ToolRef::Mcp` holding the bare selector (pi's `mcpDirectTools`) and an extension-path entry a
    // `ToolRef::ExtensionPath` (pi's `toolExtensionPaths`). Re-split those typed refs here to
    // reproduce pi's three destinations for one `tools` list (`runs/shared/pi-args.ts:104-141`): builtins (plus
    // resolved MCP names) to `--tools`, extension paths to `--extension`, and the raw MCP selectors
    // to the `MCP_DIRECT_TOOLS` env.
    // See the `required_child_tools = Some(allowlist)` assignment below for the full rationale;
    // declared out here because the value has to survive into the env overlay, which is built
    // further down.
    let mut required_child_tools: Option<Vec<String>> = None;
    // SUBA-045 — pi's `toolPlan.effectiveMcpTools`: the RESOLVED direct-MCP tool names (not the
    // `mcp:` selectors). Empty unless the agent declared `mcp:` entries.
    let mut effective_mcp_tools: Vec<String> = Vec::new();
    let mut builtin_tools: Vec<String> = Vec::new();
    let mut tool_extension_paths: Vec<String> = Vec::new();
    let mut mcp_direct_tools: Vec<String> = Vec::new();
    if let Some(tools) = &agent.tools {
        for tool in tools {
            match tool {
                ToolRef::Builtin(name) => builtin_tools.push(name.clone()),
                ToolRef::ExtensionPath(path) => tool_extension_paths.push(path.clone()),
                ToolRef::Mcp(selector) => mcp_direct_tools.push(selector.clone()),
            }
        }
    }

    // pi `runs/shared/pi-args.ts:194`: `const fanoutAuthorized = declaredBuiltinTools.includes("subagent")` —
    // a persona is granted NESTED delegation exactly when it declares the `subagent` tool itself.
    // With `tools` unset, pi's `declaredBuiltinTools` is `[]` (`:189-193`, no capability ceiling in
    // this port), so an agent that declares nothing is NOT fanout-authorized. This is the single
    // input to the child-role env pair below, and through it to
    // [`crate::extension::resolve_registration_mode`]: authorized → `ChildSafe` (the restricted,
    // mutation-blocked `subagent` tool, pi `extension/fanout-child.ts:132`), unauthorized → the
    // child registers no subagent surface at all and cannot delegate.
    let fanout_authorized = builtin_tools
        .iter()
        .any(|tool| tool == crate::extension::TOOL_NAME);

    // G103 / pi `runs/shared/pi-args.ts:389-393,549-555` @v0.43.0. `explicitToolAllowlist` is
    // `input.tools !== undefined || mcpDirectTools.length > 0 || <ceiling>` — i.e. "did anything
    // pin this child's tool surface at all". cyrup folds pi's `tools` and `mcpDirectTools` into the
    // one `agent.tools: Option<Vec<ToolRef>>`, so `is_some()` IS that predicate: `None` is an agent
    // that never wrote a `tools:` key (no restriction, child keeps the ambient set), `Some(_)` —
    // INCLUDING `Some(vec![])` — is an explicit allowlist.
    //
    // Upstream then emits `--tools <list>` when the effective allowlist is non-empty and
    // **`--no-tools`** when it is empty. Both halves were missing here: the old gate was
    // `!builtin_tools.is_empty()`, which (a) dropped `--no-tools` entirely, so an agent that asked
    // for NO tools — `tools:` empty in frontmatter, or a settings override of `"tools": false`,
    // which `discovery::merge` resolves to `Some(vec![])` — was spawned with the FULL ambient tool
    // set, the exact inversion of what it asked for; and (b) dropped `--tools` for a
    // direct-MCP-only agent (upstream's `effectiveToolAllowlist` is `[...declaredBuiltinTools,
    // ...effectiveMcpTools, ...internalTools]`, so MCP names alone still pin the allowlist).
    //
    // Upstream's third allowlist term, `internalTools` (`runs/shared/pi-args.ts:393` — the run's own
    // `structured_output` grant when the step declared an `outputSchema`), has no counterpart here
    // and needs none: **[CYRUP-DELTA]** cyrup's `--tools`/`--no-tools` selection
    // (`cyrup-session-svc/src/builder.rs:255-292`) runs over `registry.visible(...)` alone, and the
    // extension-contributed tools are merged in AFTERWARDS by `ext_host.active_tools(&base_tools)`
    // (`builder.rs:1068,1084`). `structured_output` is registered by this crate's own child-side
    // `prompt_runtime` extension, so it is never a candidate for the allowlist filter and survives
    // `--no-tools` intact — where in pi it is a first-class tool that the flag WOULD deny, hence
    // pi's explicit re-grant.
    let explicit_tool_allowlist = agent.tools.is_some();
    if explicit_tool_allowlist {
        let mut allowlist = builtin_tools.clone();

        // SUBA-014 / pi `runs/shared/pi-args.ts:361-371` @v0.43.0. Upstream's `declaredBuiltinTools`
        // is
        //
        //   input.tools === undefined
        //     ? (ceiling ? [...ceiling] : [])
        //     : (requireReadTool && requestedBuiltinTools.length > 0
        //         && !requestedBuiltinTools.includes("read") && !allowedToolSet
        //         ? ["read", ...requestedBuiltinTools]
        //         : requestedBuiltinTools).filter(...)
        //
        // i.e. `read` is injected at the HEAD of the declared builtins — never appended, never
        // deduplicated away — under a three-way condition, and only on the `tools !== undefined`
        // arm, which is exactly this `explicit_tool_allowlist` branch. `requestedBuiltinTools` is
        // pi's `tools` minus the extension-path entries (`/`, `.ts`, `.js`), which cyrup already
        // split out as `ToolRef::ExtensionPath`, so `builtin_tools` IS that list.
        //
        // The `!allowedToolSet` term is vacuously true here: cyrup has no capability ceiling
        // (tracked as SUBA-021), so `allowedToolSet` is permanently `undefined` and pi's companion
        // throw at `:355-359` ("Capability ceiling ... excludes required tool 'read' for lazy skill
        // loading") has nothing to fire on. When the ceiling lands, that throw lands with it.
        //
        // Injecting into `allowlist` rather than `builtin_tools` keeps `fanout_authorized`
        // (computed above from `builtin_tools`) untouched — correct, since upstream's
        // `fanoutAuthorized` tests for `subagent`, which this injection can never introduce — while
        // still reaching BOTH consumers that pi's `declaredBuiltinTools` reaches: the `--tools` CSV
        // and `requiredChildTools` (`:401-409`).
        if require_read_tool
            && !builtin_tools.is_empty()
            && !builtin_tools.iter().any(|tool| tool == "read")
        {
            allowlist.insert(0, "read".to_string());
        }

        if !mcp_direct_tools.is_empty() {
            // SUBA-045: kept as its own binding because it is pi's `toolPlan.effectiveMcpTools`,
            // which has a SECOND consumer besides the `--tools` CSV — `MCP_DIRECT_CHILD_TOOLS_ENV`
            // (`pi-args.ts:618-621`), which is what lets the child's diagnostic distinguish a
            // missing MCP tool ("a host/pi-mcp-adapter registration problem") from a missing
            // extension tool.
            effective_mcp_tools =
                mcp_direct_tools::resolve_mcp_direct_tool_names(&mcp_direct_tools, &opts.cwd);
            allowlist.extend(effective_mcp_tools.iter().cloned());
        }
        if allowlist.is_empty() {
            args.push("--no-tools".to_string());
        } else {
            args.push("--tools".to_string());
            args.push(allowlist.join(","));
        }

        // G106 / pi `runs/shared/pi-args.ts:611-616` @v0.43.0 (`env[REQUIRED_CHILD_TOOLS_ENV] =
        // JSON.stringify(toolPlan.requiredChildTools)`), whose `requiredChildTools` is
        // `explicitToolAllowlist ? [...declaredBuiltinTools, ...effectiveMcpTools, ...internalTools]
        // : []` (`:401-409`) — the same terms as `allowlist` above (minus `internalTools`, see the
        // `[CYRUP-DELTA]` note there), and this arm IS the `explicitToolAllowlist` branch (an agent
        // with no `tools:` never reaches it, and upstream writes `[]` — i.e. nothing — for that
        // case). Upstream itself only sets the env when the list is non-empty (`:610`), so a
        // no-tools child carries no `REQUIRED_CHILD_TOOLS` — which is also what its one consumer
        // wants: the child-side `intercom` fallback registers when the agent asked for a tool
        // called `intercom`, and an agent that asked for nothing asked for that too.
        if !allowlist.is_empty() {
            required_child_tools = Some(allowlist);
        }
    }

    // Extension threading (pi `runs/shared/pi-args.ts:125-137`): `Some(extensions)` turns discovery off
    // (`--no-extensions`) and pins the exact allowlist; `None` leaves discovery on. In both cases
    // the agent's own tool-extension paths and child-only extensions are threaded explicitly,
    // order-preserving and de-duplicated. (This crate does not inject pi's own runtime `.ts`
    // extensions — cyrup's child-side subagent runtime is env-driven, not a loaded extension file,
    // a Tier-8 child-side concern — so only agent-declared paths flow through here.)
    let mut extension_paths: Vec<String> = Vec::new();
    for path in tool_extension_paths {
        push_unique(&mut extension_paths, path);
    }
    match &agent.extensions {
        Some(extensions) => {
            args.push("--no-extensions".to_string());
            for path in extensions {
                push_unique(&mut extension_paths, path.clone());
            }
            for path in &agent.subagent_only_extensions {
                push_unique(&mut extension_paths, path.clone());
            }
        }
        None => {
            for path in &agent.subagent_only_extensions {
                push_unique(&mut extension_paths, path.clone());
            }
        }
    }
    for path in extension_paths {
        args.push("--extension".to_string());
        args.push(path);
    }

    // pi: a subagent that does not inherit skills is spawned with `--no-skills` (`runs/shared/pi-args.ts:139`).
    if !agent.inherit_skills {
        args.push("--no-skills".to_string());
    }

    // SUBA-001 / pi `runs/shared/pi-args.ts:159-165` (v0.34.0): the persona body ships on EVERY spawn; the
    // agent's `systemPromptMode` only chooses which flag carries it. See this function's doc
    // comment for the literal-text-vs-temp-file `[CYRUP-DELTA]` and the empty-body rule.
    //
    // pi `execution.ts:1058-1061` folds the agent's persistent-memory block onto the SAME
    // `systemPrompt` string just before it reaches `buildPiArgs`, so a `memory:`-scoped agent
    // carries its accumulated role notes into every run. Same composition here, on the persona body
    // that this argv pair is the only channel for. `build_agent_memory_injection` returns "" for an
    // agent with no scope (the common case), an unresolvable/unsafe path, or a read-only agent with
    // nothing recorded yet — so the overwhelming majority of spawns are byte-identical to before.
    let memory_injection =
        crate::discovery::agent_memory::build_agent_memory_injection(
            agent.memory.as_ref(),
            agent.tools.as_ref(),
            &opts.cwd,
        );
    let persona_body_trimmed = agent.system_prompt_body.trim();
    let persona_with_memory: String = if memory_injection.is_empty() {
        persona_body_trimmed.to_string()
    } else if persona_body_trimmed.is_empty() {
        memory_injection
    } else {
        format!("{persona_body_trimmed}\n\n{memory_injection}")
    };

    // pi `systemPrompt = appendAgentRefinementOverlay(systemPrompt, { cwd: skillCwd, agentName })`
    // (`execution.ts:1442`) — the statement BETWEEN the memory fold above and the output-path
    // override below, so the composition order is persona -> skills -> memory -> refinement ->
    // output-path. Upstream's `skillCwd` is `options.cwd ?? runtimeCwd`, which is the same
    // `opts.cwd` the memory block above resolves against.
    //
    // A project that has never run `refine` has no `.cyrup-subagents/refinements/` directory at
    // all, and the overlay is then a byte-for-byte no-op — as is a malformed or whitespace-only
    // overlay file, since `append_agent_refinement_overlay` is infallible and returns its input
    // unchanged on every failure (pi's blanket `catch { return systemPrompt; }`).
    let persona_with_refinement = crate::exec::agent_refinements::append_agent_refinement_overlay(
        &persona_with_memory,
        &opts.cwd,
        &agent.name,
    );

    // G82 / R-SA-024 — the SYSTEM-PROMPT half of the output-path override, upstream
    // `injectOutputPathSystemPrompt(systemPrompt, options.outputPath, agent)`
    // (`execution.ts:1443`, the statement immediately after the memory/refinement composition this
    // block mirrors). It is a SECOND surface, not an alternative to the task-side
    // `injectSingleOutputInstruction` that `build_task_text` applies: upstream's foreground single
    // run gets BOTH — the task side at `subagent-executor.ts:3674` (its caller) and this one at
    // `execution.ts:1443` — because the task text steers what the child is asked to produce while
    // the system prompt steers where its write tools point for the whole session, and a long run
    // that compacts away the task text keeps the system prompt. `api/preflight.ts:313` applies the
    // same injector to its own `effectiveSystemPrompt` projection; cyrup has no launch-contract
    // preflight surface to port that second call site onto, so this is the only one that exists
    // here.
    //
    // Keyed on the PATH alone, never on `opts.output_mode` — same rule as the task side, for the
    // same reason (`outputMode` is read only by `validateFileOnlyOutputMode` and by delivery-side
    // `finalizeSingleOutput`). Capability-aware through the same `AgentDefinition` projection the
    // task side uses, so a read-only agent is told the runtime will persist its final response
    // rather than being ordered to write a file it has no tool to write
    // (`single-output.ts:84-91`).
    let output_capabilities = completion_guard_projection(agent);
    let persona_owned = inject_output_path_system_prompt(
        &persona_with_refinement,
        opts.output_path.as_deref(),
        Some(&output_capabilities),
    );
    // SUBA-008 — pi `appendTurnBudgetSystemPrompt(shared.systemPrompt, options.turnBudget)`
    // (`execution.ts:326` for the spawn inputs and `:350` for `effectiveSystemPrompt`). It is the
    // OUTERMOST append: `shared.systemPrompt` is what `buildSharedRunInputs` returns at
    // `execution.ts:1443`, i.e. after persona -> skills -> memory -> refinement -> output-path, so
    // the budget block lands last and nothing composed above can displace it.
    //
    // This is the ONLY way the child learns about the budget — there is no
    // `PI_SUBAGENT_TURN_BUDGET` and no child-side enforcement, unlike the tool budget's env
    // hand-off two blocks below. See `exec/turn_budget.rs`'s module doc.
    let persona_owned = crate::exec::turn_budget::append_turn_budget_system_prompt(
        &persona_owned,
        opts.turn_budget.as_ref(),
    );
    let persona_body: &str = &persona_owned;
    let mut persona_temp_file: Option<std::path::PathBuf> = None;
    if !persona_body.is_empty() {
        let flag = match agent.system_prompt_mode {
            SystemPromptMode::Replace => SYSTEM_PROMPT_FLAG,
            SystemPromptMode::Append => APPEND_SYSTEM_PROMPT_FLAG,
        };
        // SUBA-030 / pi `runs/shared/pi-args.ts:570-585` @v0.43.0: the composed prompt is written to
        // a `0600` file in the run's scratch directory and the flag carries the PATH, as TWO argv
        // elements (`args.push(flag, promptPath)`, `:580-585`). Upstream applies no size threshold
        // here — unlike the task spill at `:588` it spills every time — so this is the literal
        // mechanism, not a large-persona fallback.
        //
        // Two defects close with it. The persona no longer appears in `/proc/<pid>/cmdline`, where
        // any local user could read a body that routinely carries project context; and a persona
        // above Linux's `MAX_ARG_STRLEN` (131072) can no longer make `execve` fail with `E2BIG` and
        // kill the spawn with an opaque OS error.
        //
        // The `=`-joined single-argv form the inline delivery required is gone with it, and so is
        // its reason: a detached value beginning with `-` is what clap refuses, and an ABSOLUTE
        // temp path can never begin with one.
        let stem = if agent.name.trim().is_empty() {
            "prompt"
        } else {
            agent.name.as_str()
        };
        let prompt_path = ChildSpawnSpec::resolve_system_prompt_arg(persona_body, stem, temp_dir)?;
        args.push(flag.to_string());
        args.push(prompt_path.display().to_string());
        persona_temp_file = Some(prompt_path);
    }

    // Session threading (pi `buildPiArgs`, `runs/shared/pi-args.ts:517-528`) — the FULL branch, both halves:
    //
    // * a resolved fork-context session FILE wins outright: its parent directory is created
    //   (pi's `fs.mkdirSync(path.dirname(sessionFile), { recursive: true })`) and `--session <file>`
    //   pins the child to it;
    // * otherwise the child is spawned `--no-session` UNLESS this run enables sessions at all, and
    //   an explicit `--session-dir <dir>` (directory likewise created up front) points the child's
    //   session store at the caller's directory.
    //
    // `session_enabled` is pi's `execution.ts:1412` `Boolean(sessionFile || sessionDir) || share`:
    // an explicit `sessionDir` OR `share: true` keeps sessions on; neither means the child must not
    // persist a session at all. Pre-SUBA-041 only the `--session` half existed, so
    // [`RunOptions::share`]/[`RunOptions::session_dir`] were inert fields no argv ever read and every
    // session-less child silently persisted a session the orchestrator never asked for.
    if let Some(session_path) = &opts.fork_context.session_file_path {
        if let Some(parent) = session_path.parent() {
            // Best-effort, exactly like pi's own un-guarded `mkdirSync`: a failure here surfaces as
            // the child's own session error rather than aborting the spawn plan.
            let _ = std::fs::create_dir_all(parent);
        }
        args.push("--session".to_string());
        args.push(session_path.display().to_string());
    } else {
        let session_enabled = opts.session_dir.is_some() || opts.share == Some(true);
        if !session_enabled {
            args.push("--no-session".to_string());
        }
        if let Some(session_dir) = &opts.session_dir {
            let _ = std::fs::create_dir_all(session_dir);
            args.push("--session-dir".to_string());
            args.push(session_dir.display().to_string());
        }
    }

    let (task_arg, temp_file) = ChildSpawnSpec::resolve_task_arg(task_text, temp_dir)?;

    let mut env_overlay = crate::spawn::depth::to_env_overlay(&depth);
    // PERM-001 / pi `augmentChildEnv` (`runs/shared/pi-args.ts:329-330`): the child-ROLE pair, written on EVERY
    // spawn. This is the process about to BE a subagent child, so it must say so — see
    // [`crate::spawn::nested_events::child_role_env`] for the three subsystems that read it, chiefly
    // the permission companion's child→parent ask-forwarding (a child whose `ask` fires with this
    // flag absent installs no `ForwardingAskChannel`, finds no reachable human, and fails closed on
    // every ask instead of reaching the PARENT's human through the spool).
    //
    // The companion fanout flag tracks the agent's OWN declared tools (`fanout_authorized` above),
    // exactly as pi's `env[SUBAGENT_FANOUT_CHILD_ENV] = toolPlan.fanoutAuthorized ? "1" : "0"`. It is
    // always written — an explicit `"0"`, never merely omitted — because a spawn env is an OVERLAY
    // over the inherited environment ([`crate::spawn::SpawnedChild::spawn`] never clears it), so an
    // absent entry would let a fanout-authorized process's own `CYRUP_SUBAGENT_FANOUT_CHILD=1` leak
    // down into a grandchild that was never granted it.
    //
    // (What this pair does NOT carry is the nested-event ROUTE — `event_sink`/`root_run_id`/
    // capability token, [`crate::spawn::nested_events::nested_child_auth_env`] — which no production
    // path mints yet. An authorized child therefore delegates through its own NDJSON stream rather
    // than onto a grandparent route; that wiring is separate and unported.)
    for (key, value) in crate::spawn::nested_events::child_role_env(fanout_authorized) {
        env_overlay.insert(key.to_string(), value.to_string());
    }
    // Model-inherit sentinel (R-SA-041) never leaks a global default into the child's own
    // resolution beyond what `--model` above already pins explicitly for this attempt.
    env_overlay.insert("CYRUP_SUBAGENT_RUN".to_string(), "1".to_string());
    // TOOL-031 / PARITY-GAPS PB-5, the re-exec half. pi sets the agent-identity markers on
    // `process.env` in `cli.ts` BEFORE `main()` runs (`PI_CODING_AGENT = "true"` at `cli.ts:13`
    // @v0.83.0; `AI_AGENT = "pi"` at `:14` @v0.84.1, mirrored in `rpc-entry.ts:7-8`), so they
    // reach every descendant by inheritance — including a re-exec'd subagent child and everything
    // IT spawns.
    //
    // cyrup's bin declines the process-global `std::env::set_var` (`unsafe` under edition 2024,
    // rationale at `crates/cyrup/src/main.rs`), so each spawn site writes them per-child. Without
    // this the marker chain broke at the FIRST re-exec: a subagent child ran with
    // `PI_CODING_AGENT` unset, and so did every tool and MCP server it launched — the exact
    // scripts-detect-an-agent contract the vars exist for, silently off for the whole subtree.
    //
    // Written unconditionally (never merely omitted) for the same reason as the fanout flag above:
    // the overlay is applied over an INHERITED environment, so these must be asserted, not assumed.
    env_overlay.insert("PI_CODING_AGENT".to_string(), "true".to_string());
    // [CYRUP-DELTA — KEY *and* value; the key is a FORWARD-PORT from `cli.ts:14` @v0.84.1, which is
    // AHEAD of the ported tag] `AI_AGENT` does not exist anywhere in pi @v0.83.0
    // (`git -C pi grep -n AI_AGENT v0.83.0 -- packages/` → 0 hits; `cli.ts:13` @v0.83.0 sets only
    // `PI_CODING_AGENT`), so cyrup writes a variable into every re-exec'd subagent child — and its
    // whole subtree — that the ported baseline never wrote. The value additionally names WHICH
    // agent is running (`"pi"` upstream). Deliberate and kept; stated on the delta line itself
    // rather than only in the prose above (CFG-069) so a later v0.84.1 uplift reads this as
    // ALREADY-PORTED-EARLY and not as already-done-at-tag. Same class as the
    // `working-start`/`working-stop` precedent.
    env_overlay.insert("AI_AGENT".to_string(), "cyrup".to_string());
    // Permission input (1) (port doc §4 / pi `resolveAgentName`, `pi-permission-system/src/index.ts:2033-2047` @v0.7.1): thread the
    // resolved persona name to the child as [`AGENT_NAME_ENV_VAR`] so its permission companion's
    // `agent` + `projectAgent` policy layers enforce for the named persona. Only a non-empty name is
    // written (an unnamed persona → var absent → child resolves `None`, matching pi's top-level `""`).
    if !agent.name.trim().is_empty() {
        env_overlay.insert(AGENT_NAME_ENV_VAR.to_string(), agent.name.clone());
    }
    // pi `runs/shared/pi-args.ts:199-200`: the child observes the agent's inherit flags as env (`1`/`0`).
    env_overlay.insert(
        INHERIT_PROJECT_CONTEXT_ENV.to_string(),
        if agent.inherit_project_context { "1" } else { "0" }.to_string(),
    );
    env_overlay.insert(
        INHERIT_SKILLS_ENV.to_string(),
        if agent.inherit_skills { "1" } else { "0" }.to_string(),
    );
    // pi `runs/shared/pi-args.ts:216-220`: the raw `mcp:` selectors (comma-joined) or the `__none__` sentinel.
    env_overlay.insert(
        MCP_DIRECT_TOOLS_ENV.to_string(),
        if mcp_direct_tools.is_empty() {
            MCP_DIRECT_TOOLS_NONE_SENTINEL.to_string()
        } else {
            mcp_direct_tools.join(",")
        },
    );

    // R-SA-P1 (port doc §4 P-4): the canonical parent-session anchor. Precedence: EXPLICIT (the
    // launching session's own id, resolved at the root's SessionStart via P-2 and threaded through
    // `opts.parent_session_id`) → INHERITED (this process's own `CYRUP_SUBAGENT_PARENT_SESSION`, so a
    // `DEPTH>0` child keeps threading the root's anchor rather than overwriting it — the direct-parent
    // depth-1 semantics pi documents) → EMPTY (omitted, no anchor). Only a non-empty value is written.
    if let Some(anchor) = opts
        .parent_session_id
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var(PARENT_SESSION_ENV_VAR).ok().filter(|s| !s.is_empty()))
    {
        env_overlay.insert(PARENT_SESSION_ENV_VAR.to_string(), anchor);
    }

    // Child watchdog activation (pi `execution.ts:298-302` / `subagent-runner.ts:1309-1312`): the
    // parent resolves ITS OWN watchdog config for this agent — including any
    // `subagents.watchdog.children.overrides.<agent>` entry — projects it onto the flat child shape,
    // and hands it over as one JSON env var. `resolve_child_watchdog_config` returns `None` whenever
    // the master switch or the children switch is off (which is the default), so a session that has
    // not enabled the watchdog writes no variable at all and the child's own
    // `decode_child_watchdog_config` sees nothing.
    //
    // Resolved from the CHILD's cwd (`step.cwd ?? ctx.cwd`, `subagent-runner.ts:1309`) so a run in a
    // different project reads that project's settings, and a settings-parse failure simply yields no
    // child watchdog rather than failing the spawn (upstream's `watchdogConfig.ok` guard).
    {
        let watchdog_cwd = opts.cwd.as_path();
        let resolved = crate::watchdog::settings::resolve_watchdog_config(watchdog_cwd, None);
        if resolved.ok
            && let Some(child_config) = crate::watchdog::child_status::resolve_child_watchdog_config(
                &resolved.config,
                Some(agent.name.as_str()).filter(|name| !name.trim().is_empty()),
                opts.run_id.as_ref().map(|id| id.as_str()),
                opts.child_index.and_then(|index| u64::try_from(index).ok()),
            )
            && let Some(encoded) =
                crate::watchdog::child_status::encode_child_watchdog_config(Some(&child_config))
        {
            env_overlay.insert(
                crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV.to_string(),
                encoded,
            );
        }
    }

    // Intercom child-bridge activation (pi `runs/shared/pi-args.ts:201-214`, `augmentChildEnv`'s intercom half):
    // when the launching orchestrator has a resolvable intercom presence target AND this run has an
    // id AND this child has a persona name, write the full child-orchestrator metadata set so the
    // spawned child's `IntercomExtension` reads `read_child_orchestrator_metadata() == Some` →
    // registers `contact_supervisor` (addressed at this supervisor) + a broker presence under its own
    // deterministic label. All four gate-required vars (target/run-id/agent/child-index) plus the
    // child's own label are set TOGETHER (never a partial subset — a half-set metadata gate would
    // leave the child neither fully bridged nor cleanly un-bridged). `RUN_ID`/`CHILD_INDEX` reuse the
    // nested-events env names, so setting them here also satisfies that sibling overlay. The child's
    // presence label is the SAME `resolve_subagent_intercom_target(run_id, agent, index)` string the
    // parent addresses to steer it (extension.rs `control_resume`), so the two independently-produced
    // strings match at the broker. `ORCHESTRATOR_SESSION_ID` + `SUPERVISOR_CHANNEL_DIR` are written
    // by the nested block at the end of this arm — the NATIVE supervisor channel upstream added in
    // `3ac0ef5` (`runs/shared/pi-args.ts:221-231`), which needs the launching session's own id as its request
    // routing key.
    if let (Some(orch_target), Some(run_id)) = (
        opts.orchestrator_intercom_target.as_deref().filter(|s| !s.is_empty()),
        opts.run_id.as_ref(),
    ) && !agent.name.trim().is_empty()
    {
        let child_index = opts.child_index.unwrap_or(0);
        env_overlay.insert(
            crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET.to_string(),
            orch_target.to_string(),
        );
        env_overlay.insert(crate::spawn::nested_events::RUN_ID_ENV.to_string(), run_id.as_str().to_string());
        env_overlay.insert(crate::spawn::intercom_target::ENV_CHILD_AGENT.to_string(), agent.name.clone());
        env_overlay.insert(crate::spawn::nested_events::CHILD_INDEX_ENV.to_string(), child_index.to_string());
        env_overlay.insert(
            crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME.to_string(),
            crate::spawn::intercom_target::resolve_subagent_intercom_target(
                run_id.as_str(),
                &agent.name,
                child_index,
            ),
        );

        // NATIVE supervisor channel (pi `runs/shared/pi-args.ts:221-231`, added in `3ac0ef5` "Make supervisor
        // coordination native"). Upstream's condition is `orchestratorIntercomTarget &&
        // parentSessionId && runId && childAgentName` — the first, third and fourth are the arm
        // we are already inside, so only the parent session id remains. Both vars are written
        // TOGETHER with the channel directories created up front (upstream's own two
        // `fs.mkdirSync(..., { recursive: true })` calls, `:228-229`), because
        // `read_child_metadata` refuses to activate on a partial set: a child handed a channel dir
        // it cannot address, or an address with no channel dir, would be neither bridged nor
        // cleanly un-bridged.
        //
        // A failure to create the directories is deliberately NOT fatal here: the file channel is a
        // fallback for coordination the broker path may already be providing, and a spawn must not
        // fail because a scratch directory could not be made. The vars are then simply not written,
        // which is exactly the "no native channel" state.
        if let Some(parent_session) = opts
            .parent_session_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            env_overlay.insert(
                crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID.to_string(),
                parent_session.to_string(),
            );
            let channel_dir = crate::native_supervisor::resolve_supervisor_channel_dir(
                run_id.as_str(),
                &agent.name,
                child_index,
            );
            if crate::native_supervisor::ensure_supervisor_channel_dir(&channel_dir).is_ok() {
                env_overlay.insert(
                    crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR.to_string(),
                    channel_dir.display().to_string(),
                );
            }
        }
    }

    // SUBA-S01 (pi `runs/shared/pi-args.ts:246-250`): hand the child BOTH paths. pi's child-side runtime gates
    // on both being present (`subagent-prompt-runtime.ts:281`), and so does cyrup's
    // [`crate::prompt_runtime::prompt_runtime_extension_for_env`] — so these are set together or
    // not at all. Without them the child has no `structured_output` tool, which is precisely the
    // state every declared-schema run was in before this wiring existed.
    if let Some(runtime) = structured_runtime {
        env_overlay.insert(
            crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV.to_string(),
            runtime.schema_path.display().to_string(),
        );
        env_overlay.insert(
            crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV.to_string(),
            runtime.output_path.display().to_string(),
        );
    }

    // pi `pi-args.ts` ships the resolved tool budget to the child in `PI_SUBAGENT_TOOL_BUDGET`
    // (`tool-budget.ts:70-72`); the child-side `subagent-prompt-runtime.ts:263` decodes it and
    // registers the nudge/block hook. Same hand-off here — see
    // [`crate::prompt_runtime::SubagentPromptRuntime`] for the enforcement half. Absent budget =>
    // no var, so a child that inherits a STALE budget from the parent's own environment cannot
    // happen silently: the overlay only ever adds.
    if let Some(encoded) =
        crate::exec::tool_budget::encode_tool_budget_env(agent.tool_budget.as_ref())
    {
        env_overlay.insert(
            crate::exec::tool_budget::TOOL_BUDGET_ENV.to_string(),
            encoded,
        );
    }

    // SUBA-021 — the capability ceiling resolved at the top of this function crosses the process
    // boundary here (pi `encodeSubagentCapabilityCeiling` into `SUBAGENT_CAPABILITY_CEILING_ENV`,
    // `capability-ceiling.ts:192-195`, read back by the child's own
    // `resolveCurrentSubagentCapabilityCeiling` at `:168`).
    //
    // This is what makes the bound MONOTONIC across the re-exec that is this crate's whole
    // mechanism: the child intersects what it inherits here with anything registered in its own
    // process, so a grandchild can be narrower than its parent and never wider. Absent ceiling =>
    // no var, so an unbounded run does not inherit a stale bound from the parent's environment (the
    // overlay only ever adds).
    if let Some(encoded) =
        crate::exec::capability_ceiling::encode_capability_ceiling(capability_ceiling.as_ref())
    {
        env_overlay.insert(
            crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV.to_string(),
            encoded,
        );
    }

    // SUBA-045 (pi `pi-args.ts:610-621`): the diagnostic path and the resolved direct-MCP names go
    // out BESIDE the required-tools list and under upstream's own gate — `if
    // (toolPlan.requiredChildTools.length > 0)`. An agent with no explicit `tools:` requires
    // nothing, so nothing can be missing and neither var is written.
    let mut tool_diagnostic_path: Option<PathBuf> = None;
    if let Some(tools) = required_child_tools {
        env_overlay.insert(
            crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS.to_string(),
            serde_json::Value::Array(
                tools
                    .iter()
                    .cloned()
                    .map(serde_json::Value::String)
                    .collect(),
            )
            .to_string(),
        );
        let diagnostic_path = crate::exec::tool_availability::tool_diagnostic_path_in(temp_dir);
        env_overlay.insert(
            crate::exec::tool_availability::CHILD_TOOL_DIAGNOSTIC_PATH_ENV.to_string(),
            diagnostic_path.display().to_string(),
        );
        tool_diagnostic_path = Some(diagnostic_path);
        // pi writes this one UNCONDITIONALLY at `:618-621` — but as `env[...] = undefined` when the
        // list is empty, which in Node deletes rather than sets the key. An absent key is what an
        // empty list means, so cyrup writes it only when non-empty, inside the same gate: without a
        // required list there is no diagnostic to enrich.
        if !effective_mcp_tools.is_empty() {
            env_overlay.insert(
                crate::exec::tool_availability::MCP_DIRECT_CHILD_TOOLS_ENV.to_string(),
                serde_json::Value::Array(
                    effective_mcp_tools
                        .iter()
                        .cloned()
                        .map(serde_json::Value::String)
                        .collect(),
                )
                .to_string(),
            );
        }
    }

    // G90 (pi `runs/shared/pi-args.ts:251-252` @v0.34.0: `if (input.steerInboxDir) env[SUBAGENT_STEER_INBOX_ENV]
    // = input.steerInboxDir`): hand this child the path to its OWN steer inbox. `run_sync` creates
    // the directory before the spawn so the child's watcher has something to attach to on its very
    // first tick, exactly as upstream's child-side `start()` does its own `mkdirSync` — see
    // [`crate::prompt_runtime::SteeringInbox`]. Absent on the foreground path, so a foreground child
    // is byte-identical to before.
    if let Some(inbox) = opts
        .steer_inbox_dir
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        env_overlay.insert(
            crate::prompt_runtime::STEER_INBOX_ENV.to_string(),
            inbox.display().to_string(),
        );
    }

    // SUBA-049 (pi `runs/shared/pi-args.ts:764-768` @v0.43.0: `if (input.steerCapabilityPath)
    // env[SUBAGENT_STEER_CAPABILITY_ENV] = …; if (input.steerAckDir) env[SUBAGENT_STEER_ACK_DIR_ENV]
    // = …`): the RETURN half of the same channel. Both are written under the same `if (…)` shape
    // upstream uses, so a foreground child — which has no run directory and therefore neither path —
    // is byte-identical to before.
    if let Some(path) = opts
        .steer_capability_path
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        env_overlay.insert(
            crate::prompt_runtime::STEER_CAPABILITY_ENV.to_string(),
            path.display().to_string(),
        );
    }
    if let Some(dir) = opts
        .steer_ack_dir
        .as_deref()
        .filter(|p| !p.as_os_str().is_empty())
    {
        env_overlay.insert(
            crate::prompt_runtime::STEER_ACK_DIR_ENV.to_string(),
            dir.display().to_string(),
        );
    }

    let cwd = opts.cwd.clone();

    Ok(AttemptSpawnPlan {
        spec: ChildSpawnSpec {
            command: SpawnCommand {
                binary: command.binary,
                base_args: command.base_args,
            },
            args,
            task_arg,
            env_overlay,
            cwd,
            // SUBA-030: BOTH spills are registered, so `cleanup_temp_files` removes the persona
            // file on every exit path exactly as it already removed the over-threshold task file.
            // A leaked `0600` prompt file would still be unreadable by other users, but it would
            // outlive the run, which pi's `mkdtemp`-per-spawn scratch directory never does.
            temp_files: temp_file.into_iter().chain(persona_temp_file).collect(),
        },
        tool_diagnostic_path,
    })
}

/// Compose the final task text handed to the child: acceptance-contract injection (R-SA-023), then
/// the authoritative output-path instruction (R-SA-024), then the skill-pointer block.
///
/// The agent's OWN persona prose is deliberately NOT part of this text. It travels as
/// `--system-prompt`/`--append-system-prompt` on the child's argv — see
/// [`build_attempt_spawn_plan`] (SUBA-001, pi `runs/shared/pi-args.ts:159-165`). Previously `Append` mode
/// concatenated the body here and `Replace` mode dropped it on the floor entirely, on the mistaken
/// premise that the child re-resolved its own persona; nothing child-side does (the
/// [`AGENT_NAME_ENV_VAR`] anchor is read only by the permission companion), so every
/// `Replace`-mode subagent — 7 of the 8 bundled personas and every user-authored agent — ran with
/// no persona at all.
///
/// The pre-resolved `skill_injection` (the lazy `<available_skills>` pointer block built ONCE per
/// run by [`run_sync`] via [`crate::discovery::skills::build_skill_injection`]) is appended LAST.
/// pi composes it onto the persona system prompt instead (`execution.ts:1054-1056`); keeping it in
/// the task text here is what lets a `Replace`-mode persona (which wholesale replaces the child's
/// assembled system prompt) coexist with orchestrator-injected scaffolding rather than suppress it.
/// Empty when the agent/step declares no skills, so the common no-skills case appends nothing. This
/// is ORTHOGONAL to `agent.inherit_skills` (the `--no-skills` child flag): an agent that does not
/// inherit skills still receives its explicitly-listed skills through this block.
///
/// G82: the output-path instruction is the TASK-side injector
/// [`crate::exec::output::inject_single_output_instruction`], upstream
/// `injectSingleOutputInstruction` (`runs/shared/single-output.ts:99-102`) — the one that emits the
/// `\n\n---\n**Output:**\n…` header. That header is not decoration: it is the alternative
/// `stripFrameworkInstructions` (`task-intent.ts:99`, ported at
/// [`crate::exec::task_intent`]) removes before mutation-intent classification, so the injected
/// instruction's own `write`/`persist` vocabulary never contributes write-intent signal to the task
/// it was appended to. The system-prompt-shaped sibling
/// (`build_output_path_system_prompt_instruction`, whose body opens `Runtime output path
/// override:`) was wired here instead and is NOT one of those alternatives, so every file-only run
/// was feeding its own scaffolding back into the classifier.
///
/// It is UNCONDITIONAL on [`RunOptions::output_mode`]: upstream keys the injection on the presence
/// of an output PATH alone at every one of its call sites — `subagent-executor.ts:3674` (the single
/// run, this function's direct counterpart), `chain-execution.ts:363,1320` @v0.43.0 and
/// `async-execution.ts:711,1289` @v0.43.0 — and `outputMode` is consulted only by
/// `validateFileOnlyOutputMode` (ported as [`validate_file_only_requires_path`]) and by the
/// delivery-side `finalizeSingleOutput`. Gating the injection on `OutputMode::FileOnly`, as this
/// function previously did, silently dropped the authoritative-path instruction from every
/// `file-and-inline` run that configured an output path — the child was never told where to write.
///
/// The instruction is CAPABILITY-AWARE. `agent` is projected to the [`AgentDefinition`] view
/// [`crate::exec::output::format_output_path_instruction`] reads (`tools`, via
/// `has_mutation_tool_capability`), so an agent whose whole resolved allowlist is read-only is told
/// to return the artifact in its final response for the runtime to persist — pi's
/// `formatOutputPathInstruction` read-only branch (`single-output.ts:84-91`) — instead of being
/// ordered to write a file it has no tool to write. Upstream threads the same agent object into
/// every injection site (`execution.ts:1443`, `chain-execution.ts:363`,
/// `subagent-executor.ts:3674`).
fn build_task_text(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
    contract: &AcceptanceContract,
    skill_injection: &str,
) -> String {
    let with_acceptance = inject_acceptance_contract(task, contract);
    let capabilities = completion_guard_projection(agent);
    let with_output_path = inject_single_output_instruction(
        &with_acceptance,
        opts.output_path.as_deref(),
        Some(&capabilities),
    );
    // SUBA-054 / pi `task = readsInstruction + task` (`subagent-executor.ts:3873`), which runs
    // BEFORE `injectSingleOutputInstruction` (`:3874`) — so the read line is the FIRST thing in the
    // task text, ahead of every other injected block. Prepending here rather than threading it
    // through the injectors keeps that ordering true whatever else is appended later.
    let reads_instruction = opts.reads.as_deref().map_or_else(String::new, |reads| {
        crate::spawn::chain_graph::build_single_reads_instruction(reads, &opts.cwd)
    });
    let body = if skill_injection.is_empty() {
        with_output_path
    } else {
        format!("{with_output_path}\n\n{skill_injection}")
    };
    format!("{reads_instruction}{body}")
}

/// The production [`AttemptRunner`] implementation: spawns a REAL child OS process per
/// model-fallback attempt via [`SpawnedChild::spawn`] (func-SA §1.1's mandated mechanism),
/// consumes its NDJSON stdout through [`ndjson::consume_stdout`], folds R-SA-027/028 progress,
/// and races the whole attempt against `opts.cancel`/`opts.interrupt`/`opts.deadline_at` before
/// returning an [`AttemptSignal`] plus this attempt's own richer [`AttemptRecord`] payload.
struct SpawnedChildAttemptRunner<'a> {
    agent: &'a AgentConfig,
    task: &'a str,
    opts: &'a RunOptions,
    contract: &'a AcceptanceContract,
    /// Scratch directory for `@<tempfile>` task-text overflow (R-SA-047) and the per-attempt
    /// `.jsonl` tee artifact (R-SA-058).
    scratch_dir: PathBuf,
    /// The lazy `<available_skills>` pointer block (T5, C4), resolved ONCE per run by [`run_sync`]
    /// and stable across every fallback attempt (skill resolution never depends on the model), so it
    /// is built once and reused rather than re-resolved per attempt. Empty when no skills apply.
    skill_injection: String,
    attempt_index: u32,
    /// SUBA-S01: the run's structured-output capture runtime (pi `StructuredOutputRuntime`), or
    /// `None` when the step declared no `outputSchema`. Created ONCE by [`run_sync`] and shared
    /// across every fallback attempt so a retry cannot capture into a different file than the one
    /// read back; its two paths become the child's
    /// [`crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV`]/`..._CAPTURE_ENV` overlay.
    structured_runtime: Option<crate::exec::structured::StructuredOutputRuntime>,
    /// SUBA-014: pi's `requireReadTool` (`runs/shared/pi-args.ts:118`), derived ONCE per run from
    /// "did any skill actually resolve" — `Boolean(shared.resolvedSkillNames?.length)`
    /// (`runs/foreground/execution.ts:322,357`). Stable across fallback attempts for the same
    /// reason [`Self::skill_injection`] is: skill resolution never depends on the model.
    require_read_tool: bool,
}

/// The richer per-attempt payload [`SpawnedChildAttemptRunner::run_attempt`] returns alongside its
/// [`AttemptSignal`] — everything `run_sync`'s completion path (structured-output validation,
/// completion guard, acceptance evaluation, R-SA-033's ordering) needs from the WINNING attempt,
/// without `fallback::run_fallback_ladder` itself needing to know this shape at all (it only ever
/// touches [`AttemptSignal`]).
struct AttemptRecord {
    progress: AgentProgress,
    final_output: Option<String>,
    /// A soft interrupt (`RunOptions.interrupt`) fired on this attempt — pi's paused-success
    /// semantics (`execution.ts:722-761`, T3 group A). Carried on the runner's own per-attempt
    /// payload (not on [`AttemptSignal`], which this crate does not own) so `run_sync` can flip the
    /// terminal [`SingleResult`] to `interrupted: true`, exit 0, cleared error. An interrupted
    /// attempt reports `AttemptSignal { success: true, exit_code: Some(0), .. }`, so the ladder
    /// stops on it exactly like an ordinary success.
    interrupted: bool,
    /// This attempt's live-control state machine, carried out of the ladder so `run_sync` can (a)
    /// raise the post-settlement completion-guard notice against the WINNING attempt's own dedup
    /// set — pi's `emitControlEvent` at `execution.ts:417-423` is a local of the same
    /// `runSingleAttempt` scope — and (b) fold its raised events onto
    /// [`SingleResult::control_events`] (pi `result.controlEvents = allControlEvents`, `:1314`).
    control: crate::exec::control::ControlMonitor,
    /// SUBA-008 — this attempt's turn-budget latch, carried out of [`drive_attempt`] so `run_sync`
    /// can fold pi's terminal composition (`execution.ts:1251-1258`) onto the delivered output and
    /// publish `turnBudget`/`turnBudgetExceeded`/`wrapUpRequested` on the [`SingleResult`].
    ///
    /// Unarmed on every path that never reached the drive loop (a spawn failure) and on every run
    /// that declared no budget.
    turn_budget: crate::exec::turn_budget::TurnBudgetTracker,
}

#[async_trait::async_trait]
impl AttemptRunner for SpawnedChildAttemptRunner<'_> {
    type Attempt = AttemptRecord;

    async fn run_attempt(
        &mut self,
        model: &ModelId,
        attempt_note: Option<&str>,
    ) -> (AttemptSignal, Self::Attempt) {
        let mut progress = AgentProgress {
            // pi's `startTime` local, captured at the very top of `runSingleAttempt` — before the
            // spawn plan is even built — and read back as `progress.durationMs = Date.now() -
            // startTime` at every settle site (`runs/foreground/execution.ts:1177`).
            started_at: Some(std::time::Instant::now()),
            ..AgentProgress::default()
        };
        // pi seeds the ring with the ladder's attempt notes at construction time
        // (`recentOutput: [...shared.attemptNotes]`, `runs/foreground/execution.ts:366`); this
        // crate's ladder hands them down one at a time, so each is appended as it arrives.
        if let Some(note) = attempt_note {
            progress.append_recent_output(note);
            // ...and onto the LIVE surface, which is the only place a user can actually read it:
            // this attempt's own `progress` is compacted (`recent_output` emptied) before it
            // becomes `SingleResult::progress`, exactly as pi's `compactCompletedProgress` does.
            // pi has no second hop here only because its live stream and its settled snapshot are
            // the same mutable object; cyrup's live surface folds the child's NDJSON, which a
            // parent-side note never appears on. See [`LiveEventSink::emit_note`].
            if let Some(sink) = &self.opts.live_events {
                sink.emit_note(note);
            }
        }

        // pi `runSingleAttempt`'s control locals (`execution.ts:245-246` @v0.34.0): the attempt's own
        // start instant, its resolved control config (`options.controlConfig ?? DEFAULT_CONTROL_CONFIG`)
        // and its per-attempt dedup/record state. Built here — before the spawn plan — so every
        // early-return path below still hands a (trivially empty) monitor back to `run_sync`
        // rather than the ladder losing the field entirely.
        let mut control = crate::exec::control::ControlMonitor::new(
            self.opts.control_config.clone().unwrap_or_default(),
            self.opts
                .run_id
                .as_ref()
                .map(|id| id.as_str().to_string())
                .unwrap_or_else(|| self.agent.name.clone()),
            self.agent.name.clone(),
            self.opts
                .child_index
                .and_then(|index| u32::try_from(index).ok()),
            self.opts.on_control_event.clone(),
            crate::background::now_epoch_millis_pub(),
        );

        let task_text =
            build_task_text(self.agent, self.task, self.opts, self.contract, &self.skill_injection);

        // R-SA-054/055/056 (SAFETY-CRITICAL, C15): the CHILD about to be spawned is one recursion
        // hop deeper than THIS process, so its env overlay MUST carry the incremented envelope —
        // `next_envelope(parent, agent_max)` = `{ current_depth: parent.current_depth + 1,
        // max_depth: min(parent.max_depth, agent.max_subagent_depth) }` — never `self.agent.depth`
        // (the parent's OWN envelope) verbatim. Passing the parent envelope through unchanged (the
        // prior bug) meant every descendant inherited depth 0 and the ceiling check
        // (`run_sync`'s `is_blocked`) never tripped across the subprocess boundary, so recursion
        // could run unbounded. This mirrors pi's `getSubagentDepthEnv(maxSubagentDepth)`
        // (`shared/types.ts:1046`, `recursion-guard.test.ts:210-257`), which likewise increments
        // the inherited `PI_SUBAGENT_DEPTH` and applies the tighter per-agent max before rendering
        // the child's spawn env. The parent-side gate (`run_sync`'s own `is_blocked(&agent.depth)`,
        // Step 0) still guards whether THIS process may spawn at all; this line is what makes the
        // NEXT process's own Step-0 gate see a truthful, incremented depth.
        let child_depth =
            crate::spawn::depth::next_envelope(&self.agent.depth, self.agent.max_subagent_depth);

        let plan = match build_attempt_spawn_plan_with_read_requirement(
            self.agent,
            model,
            &task_text,
            self.opts,
            child_depth,
            &self.scratch_dir,
            self.structured_runtime.as_ref(),
            self.require_read_tool,
        ) {
            Ok(plan) => plan,
            Err(err) => {
                return (
                    AttemptSignal {
                        success: false,
                        exit_code: None,
                        error: Some(err.to_string()),
                        usage: Usage::default(),
                        timed_out: false,
                        detached: false,
                        startup: StartupEvidence::default(),
                    },
                    AttemptRecord {
                        turn_budget: Default::default(),
                        progress,
                        final_output: None,
                        interrupted: false,
                        control,
                    },
                );
            }
        };

        let jsonl_path = self
            .scratch_dir
            .join(format!("attempt-{}.jsonl", self.attempt_index));
        self.attempt_index += 1;

        // SUBA-045: taken off the plan BEFORE `plan.spec` is moved into the spawn, and read back in
        // the close-handler chain below (pi's `toolDiagnosticPath` local, `execution.ts:1072`).
        //
        // [CYRUP-DELTA] pi mkdtemps a FRESH dir per attempt (`pi-args.ts:603-604`), so its
        // diagnostic path is unique per attempt by construction. cyrup's attempts share one
        // `scratch_dir`, so the file is cleared HERE, immediately before the spawn, to restore the
        // same guarantee: a child that dies before `agent_start` — the one case where it never gets
        // to write or delete the file itself — must not inherit the previous model attempt's
        // verdict. Without this, the model-fallback ladder could attribute attempt N's missing
        // tools to attempt N+1's startup crash.
        let tool_diagnostic_path = plan.tool_diagnostic_path;
        if let Some(path) = tool_diagnostic_path.as_deref() {
            let _ = std::fs::remove_file(path);
        }

        let mut child = match SpawnedChild::spawn(plan.spec, &jsonl_path).await {
            Ok(child) => child,
            Err(err) => {
                return (
                    AttemptSignal {
                        success: false,
                        exit_code: None,
                        error: Some(err.to_string()),
                        usage: Usage::default(),
                        timed_out: false,
                        detached: false,
                        startup: StartupEvidence::default(),
                    },
                    AttemptRecord {
                        turn_budget: Default::default(),
                        progress,
                        final_output: None,
                        interrupted: false,
                        control,
                    },
                );
            }
        };

        // Move the child's stderr reader out BEFORE `drive_attempt` consumes the child, so its
        // trailing diagnostic output can be surfaced into the run's error on a non-zero exit (pi
        // `execution.ts:686`). `drive_attempt` reads only stdout; the orphaned reader is drained to
        // EOF below (in the non-zero-exit branch), once the child is dead and its closed write end
        // guarantees a prompt EOF.
        let stderr_reader = child.take_stderr();

        let deadline_sleep = self
            .opts
            .deadline_at
            .map(|instant| tokio::time::sleep_until(tokio::time::Instant::from_std(instant)));
        let outcome =
            drive_attempt(child, &mut progress, self.opts, deadline_sleep, &mut control).await;

        // --- Interrupt: paused-success (pi `execution.ts:722-761`, T3 group A bug fix). A soft
        // interrupt is NOT a failure: it terminates the ladder with exit 0, a CLEARED error, and
        // the "paused" sentinel output, recorded under its own flag rather than folded into
        // exit-1/timed-out. pi returns from `runSingleAttempt` here BEFORE any exit-code
        // re-diagnosis, so this branch mirrors that early return exactly. ---
        if outcome.interrupted {
            return (
                AttemptSignal {
                    success: true,
                    exit_code: Some(0),
                    error: None,
                    usage: progress.usage.clone(),
                    timed_out: false,
                    detached: outcome.detached,
                    startup: StartupEvidence::default(),
                },
                AttemptRecord {
                    // SUBA-008: an interrupted attempt still reports the budget it ran under.
                    turn_budget: outcome.turn_budget.clone(),
                    progress,
                    final_output: Some(INTERRUPTED_FINAL_OUTPUT.to_string()),
                    interrupted: true,
                    control,
                },
            );
        }

        let (raw_exit_code, spawn_error, process_signal) = match &outcome.exit_status {
            Ok(Some(status)) => (status.code(), None, process_signal_name(status)),
            Ok(None) => (None, None, None), // terminated via signal escalation (timeout/cancel)
            Err(err) => (None, Some(err.to_string()), None),
        };

        let final_output = extract_final_output(&progress.message_end_events);

        // --- Timeout terminates the ladder outright (R-SA-036); its own flag is what
        // `run_fallback_ladder` branches on. Kept as a distinct early exit so the exit-0
        // re-diagnosis chain below never runs against a timed-out attempt. ---
        if outcome.timed_out {
            return (
                AttemptSignal {
                    success: false,
                    exit_code: Some(raw_exit_code.unwrap_or(1)),
                    error: spawn_error.or_else(|| Some("subagent attempt timed out".to_string())),
                    usage: progress.usage.clone(),
                    timed_out: true,
                    detached: outcome.detached,
                    startup: StartupEvidence::default(),
                },
                AttemptRecord {
                    // SUBA-008: pi's timeout arm wins the terminal composition outright
                    // (`execution.ts:1241`), but the state is still published.
                    turn_budget: outcome.turn_budget.clone(),
                    progress,
                    final_output,
                    interrupted: false,
                    control,
                },
            );
        }

        // --- Exit-0 re-diagnosis (pi `execution.ts:684-790`), in pi's exact order. ---

        // (a) The trailing, still-uncleared assistant `errorMessage` (pi close-handler
        //     `execution.ts:684` sets `result.error = assistantError`).
        // (a.0) The protocol-output-limit diagnostic outranks everything: pi's `failProtocol` sets
        //     `result.error` at the moment the cap trips, and the close handler only fills in a
        //     `closeError` when `result.error` is still unset (`execution.ts:1099`).
        // (a.-1) SUBA-008 — a turn-budget abort sets `result.error = message` at the moment it
        //     fires (`execution.ts:740`), i.e. strictly BEFORE the close handler runs, and the
        //     close handler only fills in a `closeError` when `result.error` is still unset
        //     (`:1099`). So the abort message outranks every diagnosis below it — including the
        //     child's own trailing apology, which is exactly what a child that was signalled
        //     mid-sentence tends to produce.
        //
        //     It cannot collide with the protocol-limit diagnostic below: the drive loop returns
        //     on whichever of the two fires first, so at most one is ever set.
        let mut error = match outcome.turn_budget.terminal_note() {
            Some(crate::exec::turn_budget::TurnBudgetTerminalNote::Exceeded(message)) => {
                Some(message)
            }
            _ => None,
        };
        if error.is_none() {
            error = outcome
                .protocol_error
                .as_ref()
                .map(crate::exec::child_protocol::format_protocol_output_limit);
        }
        if error.is_none() {
            error = spawn_error;
        }
        // (a.1) SUBA-045 — the child tool-availability diagnostic, in pi's exact rank: `closeError =
        //     result.error ?? toolDiagnosticError ?? assistantError` (`execution.ts:1079`). It sits
        //     ABOVE the trailing assistant error deliberately, and that ordering is the whole point
        //     of the item: a child told to use a tool its host never registered produces a
        //     perfectly ordinary model apology, and the apology would otherwise become the run's
        //     error and hide the cause. The file exists only when something was actually missing
        //     (the child DELETES it otherwise), so this is silent on every healthy run.
        if error.is_none() {
            error = crate::exec::tool_availability::read_child_tool_diagnostic_error(
                tool_diagnostic_path.as_deref(),
            );
        }
        if error.is_none() {
            error = trailing_assistant_error(&progress.all_events);
        }

        // (b) `forcedDrainAfterFinalSuccess` (pi `execution.ts:1080`): a child that emitted a CLEAN
        //     terminal stop but had to be force-drained (held stdout open past the grace window)
        //     is coerced to exit 0, not treated as a forced-kill failure.
        //     pi's witness is `(cleanTerminalAssistantStopReceived || agentSettledReceived)`
        //     (`execution.ts:1080`) — a child that announced `agent_settled` finished on purpose
        //     just as much as one that emitted a clean terminal assistant stop, and before
        //     `agent_settled` was parsed at all this crate could only ever see the second half.
        let forced_drain_after_final_success = outcome.forced_termination
            && (outcome.clean_terminal_stop || outcome.agent_settled)
            && error.is_none();

        // (b.1) Surface the child's trailing stderr as the error on a non-zero (or signal-death)
        //     exit, when nothing richer was already diagnosed and this is not a clean forced-drain
        //     success (pi `execution.ts:686`: `if (code !== 0 && stderrBuf.trim() && !result.error
        //     && !forcedDrainAfterFinalSuccess) result.error = stderrBuf.trim()`). `raw_exit_code !=
        //     Some(0)` is pi's `code !== 0` (true for a non-zero code AND for a signal-death `null`
        //     code). Drained here, once the child is dead so its closed write end EOFs the orphaned
        //     reader promptly — never during the read loop (stderr is not protocol data, R-SA-046).
        if error.is_none() && !forced_drain_after_final_success && raw_exit_code != Some(0) {
            let stderr_text = stderr_reader.drain_to_string().await;
            let trimmed = stderr_text.trim();
            if !trimmed.is_empty() {
                error = Some(trimmed.to_string());
            }
        }

        // (c) The forced/final exit code (pi `execution.ts:689`): a forced-termination or a
        //     signal-death (no numeric code) attributes exit 1 unless the clean-drain coercion
        //     above applies; a normal exit keeps its own code (defaulting to 0).
        let mut exit_code: i32 = if forced_drain_after_final_success {
            0
        } else if outcome.forced_termination || raw_exit_code.is_none() {
            raw_exit_code.unwrap_or(1)
        } else {
            raw_exit_code.unwrap_or(0)
        };

        // (d) A set error flips a zero exit to failure (pi `execution.ts:769-771`).
        if error.is_some() && exit_code == 0 {
            exit_code = 1;
        }

        // (e) `detectSubagentError` re-diagnosis of a still-clean zero exit — a trailing failed
        //     tool/bash call the agent did not speak past (pi `execution.ts:772-780`).
        if exit_code == 0
            && error.is_none()
            && let Some(detected) = detect_subagent_error(&progress.all_events)
        {
            exit_code = detected.exit_code;
            error = Some(detected.message());
        }

        // (f) Empty-output (cold-start) classification (pi `execution.ts:781-789`): a zero-exit run
        //     that produced no usable final text is a RETRYABLE failure so the model-fallback
        //     ladder advances (the message matches `is_retryable_model_failure`'s cold-start /
        //     empty-response / no-output patterns). Mirrors pi's
        //     `!finalText?.trim() && (!options.structuredOutput || missingStructuredOutput)`
        //     exactly: when a structured-output schema IS declared, an empty prose is a failure
        //     ONLY if the structured output is ALSO absent — if the child DID produce a
        //     structured-output value, the empty prose is fine and this gate stays silent
        //     (`run_sync`'s own R-SA-030 check then validates that value). cyrup's
        //     `missingStructuredOutput` analog is a pure PRESENCE test over the event stream
        //     ([`structured_output_absent`], pi's `!existsSync(outputPath)`), NOT a validity test:
        //     a present-but-invalid value is diagnosed later by `run_sync`, exactly as pi defers
        //     validity to `readStructuredOutput` (`execution.ts:791`), which runs only after this
        //     empty-output gate has left the exit code clean. Emitting the retryable "no output"
        //     error HERE (per attempt), rather than deferring the whole structured-missing case to
        //     the post-ladder check, is what lets the ladder actually retry a cold-start empty run
        //     that also declared a schema — pi's behavior, which a `structured_output_schema.is_some()`
        //     short-circuit here would silently drop (the ladder would stop on a bare exit-0 attempt
        //     and only `run_sync` would later flag a NON-retryable structured-missing failure).
        if exit_code == 0
            && error.is_none()
            && final_output
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
            && structured_output_absent(
                self.opts.structured_output_schema.as_ref(),
                self.structured_runtime.as_ref(),
            )
        {
            exit_code = 1;
            error = Some(EMPTY_OUTPUT_ERROR.to_string());
        }

        let success = exit_code == 0 && error.is_none();
        // A bare non-zero exit with no diagnosed cause still needs a stable error string for the
        // ladder's record; pi leaves it undefined, but this crate's `ModelAttempt`/`SingleResult`
        // callers surface `error` directly, so a plain "exited with code N" (never matching a
        // retryable pattern) is used rather than a null.
        // ...and the startup-failure classifier has to be told that is what happened, since pi
        // keys "the child failed with nothing to say" on `error` being UNSET
        // (`subagent-startup-retry.ts:52`). See `StartupEvidence::error_is_placeholder`.
        let error_is_placeholder = !success && error.is_none();
        if error_is_placeholder {
            error = Some(format!("subagent attempt exited with code {exit_code}"));
        }

        (
            AttemptSignal {
                success,
                exit_code: Some(exit_code),
                error,
                usage: progress.usage.clone(),
                timed_out: false,
                // R-SA-037: set from the drive loop's detach observation — `true` when the child's
                // NDJSON showed a blocking `contact_supervisor` ask (surfaced via `spawn_clarify`),
                // which bypasses acceptance/completion-guard/truncation and stops the ladder.
                detached: outcome.detached,
                // Startup-failure evidence (pi `execution.ts:1558-1573`). Every field here is a
                // reason NOT to relaunch this model; `is_retryable_subagent_startup_failure`
                // fails closed on any of them.
                startup: StartupEvidence {
                    final_output_present: final_output
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty()),
                    message_count: progress.message_end_events.len(),
                    tool_count: progress.tool_count,
                    duration_ms: Some(progress.duration_ms()),
                    protocol_error: outcome.protocol_error.is_some(),
                    process_signal: process_signal.clone(),
                    observed_mutation_attempt: crate::exec::completion_guard::has_mutation_tool_call(
                        &progress.all_events,
                    ),
                    // cyrup's foreground executor has no `stopped` analog (pi carries it on the
                    // BACKGROUND runner's result). It cannot be true of a child with zero
                    // messages, zero tools and zero usage anyway — it requires the child to have
                    // run turns — so leaving it false cannot widen the predicate.
                    stopped: false,
                    // SUBA-008 — no longer hard-coded: a turn-budget abort is a deliberate
                    // supervisor kill, and `is_retryable_subagent_startup_failure` must fail
                    // closed on it rather than relaunching the model that was over budget.
                    turn_budget_exceeded: outcome.turn_budget.exceeded(),
                    error_is_placeholder,
                },
            },
            AttemptRecord {
                turn_budget: outcome.turn_budget.clone(),
                progress,
                final_output,
                interrupted: false,
                control,
            },
        )
    }

    /// pi `waitForSubagentStartupRetry(delayMs, [options.signal, options.interruptSignal])`
    /// (`execution.ts:1588`): the backoff is raced against BOTH lifecycle signals, and which one
    /// fired decides whether the run is paused or abandoned. Already-aborted is checked first
    /// (upstream `subagent-startup-retry.ts:88`), so a cancelled run never sleeps before giving up.
    async fn wait_startup_retry(&mut self, delay: Duration) -> StartupRetryWait {
        if self.opts.interrupt.is_cancelled() {
            return StartupRetryWait::Interrupted;
        }
        if self.opts.cancel.is_cancelled() {
            return StartupRetryWait::Cancelled;
        }
        tokio::select! {
            biased;
            () = self.opts.interrupt.cancelled() => StartupRetryWait::Interrupted,
            () = self.opts.cancel.cancelled() => StartupRetryWait::Cancelled,
            () = tokio::time::sleep(delay) => StartupRetryWait::Proceed,
        }
    }

    /// pi mutates `result.finalOutput`/`result.interrupted` in place at `execution.ts:1584-1618`;
    /// this crate's ladder cannot reach inside an opaque `Attempt`, so it calls back here instead.
    fn apply_startup_outcome(&mut self, attempt: &mut Self::Attempt, outcome: &StartupOutcome) {
        match outcome {
            StartupOutcome::Interrupted => {
                attempt.interrupted = true;
                attempt.final_output = Some(INTERRUPTED_FINAL_OUTPUT.to_string());
            }
            StartupOutcome::Cancelled(message) | StartupOutcome::Exhausted(message) => {
                // pi also sets `result.progress.status/error` here; cyrup's `AgentProgress` carries
                // neither field (its status/error live on `SingleResult`, which `run_sync` derives
                // from the ladder's own `AttemptSignal::error` — already set by the ladder).
                attempt.final_output = Some(message.clone());
            }
        }
    }

    fn snapshot_output_file(&mut self) {
        // R-SA-031: the actual snapshot value is consulted later, in `finalize_result`'s
        // file-only handoff — `run_fallback_ladder` only requires the snapshot to be TAKEN at
        // the correct point (immediately before each fresh spawn), which this no-op satisfies
        // trivially since `run_sync` itself takes the real snapshot once, outside the ladder, and
        // compares it once after the ladder settles (R-SA-031 is a whole-task stat-snapshot
        // heuristic, not a per-attempt one — a task's `output_path` does not change between
        // fallback attempts, so re-snapshotting per attempt would not observe anything new).
    }
}

/// The OS signal that killed a child, named the way pi names it (`proc.on("close", (code,
/// signal))` hands Node's signal NAME straight through), for
/// [`crate::exec::fallback::StartupEvidence::process_signal`]. `None` on a normal exit, and `None`
/// on non-Unix, where no such concept crosses `ExitStatus`.
/// SUBA-023 (consumer half) — the name comes from [`crate::spawn::signal::signal_name_of`], the
/// single crate-wide mapping that also populates
/// [`crate::spawn::signal::TerminationOutcome::signal_name`]. This function used to carry its OWN
/// three-entry table (`SIGINT`/`SIGKILL`/`SIGTERM`) and render everything else as `SIG<number>`, so
/// a child that segfaulted reported `SIG11` and one that aborted reported `SIG6` where pi — which
/// hands Node's signal NAME straight through — reports `SIGSEGV` and `SIGABRT`. Those are precisely
/// the deaths worth diagnosing, and they are what the ladder's own three signals are not.
///
/// The numeric `SIG<n>` form survives only as the fall-back for a signal the shared table does not
/// name, so no previously-reported value is lost.
#[cfg(unix)]
fn process_signal_name(status: &std::process::ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;
    crate::spawn::signal::signal_name_of(status)
        .map(ToString::to_string)
        .or_else(|| status.signal().map(|signal| format!("SIG{signal}")))
}

#[cfg(not(unix))]
fn process_signal_name(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

/// The runtime facts [`SpawnedChildAttemptRunner::run_attempt`]'s exit-0 re-diagnosis (pi
/// `execution.ts:747-790`, T3 group A) needs from [`drive_attempt`] beyond the raw exit status.
struct DriveOutcome {
    /// The orchestrator's own wall-clock deadline expired (R-SA-036) — terminates the ladder.
    timed_out: bool,
    /// `RunOptions.interrupt` fired (pi's soft interrupt, `execution.ts:722-745`) — the paused-
    /// success path, distinct from a timeout or a hard cancel.
    interrupted: bool,
    /// The child emitted a terminal assistant stop but held its stdout open past the final-stop
    /// grace window (or closed stdout yet lingered past `FINAL_DRAIN_TIMEOUT`), so it had to be
    /// force-drained via the real signal ladder — pi's `forcedTerminationSignal`
    /// (`execution.ts:356-362` @v0.34.0).
    forced_termination: bool,
    /// At least one terminal assistant stop observed on this attempt carried no `errorMessage` —
    /// pi's `cleanTerminalAssistantStopReceived` (`execution.ts:557`), the other half of
    /// `forcedDrainAfterFinalSuccess`.
    clean_terminal_stop: bool,
    /// The child emitted `agent_settled` — pi's `agentSettledReceived` (`execution.ts:843`). The
    /// SECOND half of pi's `forcedDrainAfterFinalSuccess` witness (`:1080`:
    /// `(cleanTerminalAssistantStopReceived || agentSettledReceived)`), and the event that arms the
    /// final-stop grace window for a child whose last assistant message was not a clean terminal
    /// stop.
    agent_settled: bool,
    /// The child blew past the per-line stdout cap and the line was not a projectable aggregate
    /// (pi `failProtocol`, `execution.ts:1026-1041`). Set only on that path; when set, the child was
    /// force-terminated through the signal ladder and this diagnostic becomes the attempt's error,
    /// ahead of every other diagnosis.
    protocol_error: Option<crate::exec::child_protocol::ProtocolOutputLimit>,
    /// The child's real exit status once confirmed gone, or a genuine `wait()`/read I/O fault.
    exit_status: std::io::Result<Option<std::process::ExitStatus>>,
    /// R-SA-037: the child's NDJSON stream showed a BLOCKING `contact_supervisor` supervisor-clarify
    /// ask (`need_decision`/`interview`), so the drive loop fired
    /// [`crate::tui::intercom::spawn_clarify`] and this attempt is marked detached (its outcome
    /// bypasses acceptance/completion-guard/truncation, and the fallback ladder does not advance past
    /// it). `false` when no such ask was observed.
    detached: bool,
    /// SUBA-008 — the run's turn-budget latch as it stood when this attempt ended: pi's
    /// `result.turnBudget` / `result.turnBudgetExceeded` / `result.wrapUpRequested` trio
    /// (`execution.ts:1087`/`:1251-1258`), carried out of the drive loop as one value.
    ///
    /// Unarmed (`TurnBudgetTracker::is_armed() == false`) for every run that declared no budget,
    /// which is every run today that does not pass one — so this field changes nothing on those
    /// paths.
    turn_budget: crate::exec::turn_budget::TurnBudgetTracker,
}

/// pi's `missingStructuredOutput` analog (`execution.ts:1189-1191`) for the empty-output
/// (cold-start) gate: is the child's structured output ABSENT from its event stream? Returns
/// `true` when NO structured-output schema was requested at all (pi's `!options.structuredOutput`
/// leg, where empty prose is unconditionally an empty-output failure), OR when a schema WAS
/// requested but the child never delivered a value. A present-but-invalid value is deliberately NOT
/// "absent" here — this is a pure presence test, exactly like pi's `existsSync`; the value's
/// validity is a separate concern [`run_sync`]'s own R-SA-030 structured-output check diagnoses
/// afterward (pi `readStructuredOutput`, `execution.ts:1204-1224`).
///
/// The presence channel is pi's literally: `!existsSync(options.structuredOutput.outputPath)`
/// (`execution.ts:1189-1191`) — the CAPTURE FILE the child's `structured_output` tool writes, and
/// nothing else. Consulting the transcript instead would fail a perfectly good run: a child that
/// calls `structured_output` and then stops without prose has `finalText` empty and a written
/// capture file, which pi passes (`missingStructuredOutput === false`) and a fenced-block scan
/// would classify as missing, flipping the attempt to a retryable "produced no output" failure the
/// ladder then burns a fallback model on.
///
/// SUBA-S01 residual: the no-runtime case (`run_sync` could not create the capture runtime at all)
/// used to fall through to that fenced-block scan. It no longer does — with no runtime there is no
/// capture file, so the value is absent by definition, which is also exactly what [`run_sync`]'s
/// own read-back now concludes for the same state. The two can no longer disagree.
fn structured_output_absent(
    schema: Option<&serde_json::Value>,
    runtime: Option<&crate::exec::structured::StructuredOutputRuntime>,
) -> bool {
    match schema {
        None => true,
        Some(_) => match runtime {
            Some(runtime) => !runtime.output_path.exists(),
            None => true,
        },
    }
}

/// The final-stop grace window (pi `FINAL_STOP_GRACE_MS`, `execution.ts:333`): once a terminal
/// assistant stop is observed, a child that has not exited (released its stdout) within this window
/// is force-drained via [`SpawnedChild::terminate`]'s real SIGINT->SIGTERM->SIGKILL ladder rather
/// than the parent blocking indefinitely on a child that emitted its final answer but never
/// exited. pi's subsequent `HARD_KILL_MS`(3000) SIGKILL step is subsumed by `terminate`'s own
/// SIGTERM->SIGKILL escalation, which this crate routes every forced termination through.
const FINAL_STOP_GRACE_MS: u64 = 1000;

/// SUBA-S06: how long to keep draining stdout after the child process itself has been reaped while
/// its stdout is still held open by a surviving grandchild.
///
/// This is NOT [`FINAL_STOP_GRACE_MS`]'s job and must not be folded into it. That window is armed
/// by a *protocol* event (a terminal assistant stop) and expiring it means force-draining a live
/// process through the signal ladder. This one is armed by an *OS* event (the direct child is
/// already gone, so there is nothing left to signal) and expiring it simply ends the read loop, so
/// the ordinary post-loop path can report the exit status it already has. They coincide at 1000ms
/// today only because both are "give buffered output a beat to arrive".
const POST_EXIT_DRAIN_MS: u64 = 1000;

/// R-SA-037: does `event` show a child BLOCKING on a `contact_supervisor` supervisor-clarify ask,
/// and if so, what is the human-facing prompt? A blocking ask is `contact_supervisor`'s
/// `need_decision`/`interview` reason (the intercom `ask_and_wait` shapes,
/// `contact_supervisor.rs:81-101`) — NOT the fire-and-forget `progress_update`, which never blocks.
/// The prompt is the ask's `message` (empty string if the child omitted it). No new NDJSON wire
/// variant is needed: a blocking ask surfaces as an ordinary `ToolExecutionStart` for the
/// `contact_supervisor` tool, which this reuses (per `AttemptSignal::detached`'s own recipe).
fn contact_supervisor_block_prompt(event: &crate::exec::ndjson::SubagentEvent) -> Option<String> {
    if let crate::exec::ndjson::SubagentEvent::ToolExecutionStart { tool_name, args, .. } = event
        && tool_name == "contact_supervisor"
    {
        let reason = args.get("reason").and_then(serde_json::Value::as_str).unwrap_or_default();
        if matches!(reason, "need_decision" | "interview") {
            return Some(
                args.get("message")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    None
}

/// SUBA-008 — is this event an ASSISTANT `message_end`? pi's `evt.type === "message_end" &&
/// evt.message && evt.message.role === "assistant"` (`execution.ts:910-912`), which is the ONLY
/// shape that increments a turn.
fn is_assistant_message_end(event: &SubagentEvent) -> bool {
    let SubagentEvent::MessageEnd { message } = event else {
        return false;
    };
    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
}

/// SUBA-008 — every `toolCall` content part of a `message_end`, pi's `toolCalls` filter
/// (`execution.ts:915-918`). Returns an empty slice for any other event shape.
fn message_end_tool_calls(event: &SubagentEvent) -> Vec<&serde_json::Value> {
    let SubagentEvent::MessageEnd { message } = event else {
        return Vec::new();
    };
    message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .map(|parts| {
            parts
                .iter()
                .filter(|part| {
                    part.get("type").and_then(serde_json::Value::as_str) == Some("toolCall")
                })
                .collect()
        })
        .unwrap_or_default()
}

/// SUBA-008 — pi's `hasToolCall` (`execution.ts:919`).
fn message_end_has_tool_call(event: &SubagentEvent) -> bool {
    !message_end_tool_calls(event).is_empty()
}

/// SUBA-008 — pi's `terminalStructuredOutputCall` (`execution.ts:921-923`), minus the
/// `Boolean(options.structuredOutput)` half its caller applies: EXACTLY one tool call, and it is
/// `structured_output`.
///
/// This is the second way a turn counts as terminal. Without it, a child that answers by calling
/// the structured-output tool — the normal ending for a `outputSchema` run, where `stopReason` is
/// `toolUse`, not `stop` — would be treated as still working and could be aborted at the exact
/// moment it delivered its answer.
fn is_sole_structured_output_tool_call(event: &SubagentEvent) -> bool {
    let calls = message_end_tool_calls(event);
    calls.len() == 1
        && calls
            .first()
            .and_then(|call| call.get("name"))
            .and_then(serde_json::Value::as_str)
            == Some("structured_output")
}

/// Drive one spawned child to completion, folding every NDJSON line into `progress` (R-SA-027/028)
/// and racing the whole read loop against `opts.cancel`/`opts.interrupt`/an optional deadline
/// timer, plus the final-stop grace-drain window (pi `execution.ts:333-367`, T3 group A). Returns a
/// [`DriveOutcome`].
///
/// On timeout, cancel, interrupt, or a final-stop grace-drain, the child is driven through
/// [`SpawnedChild::terminate`]'s real signal-escalation ladder (R-SA-036/059) — never a bare
/// `kill()`. `child` is taken by value (never `&mut`): [`SpawnedChild::terminate`]/
/// [`SpawnedChild::finish`] both consume `self` to guarantee temp-file cleanup runs exactly once
/// on every exit path (R-SA-067), so this function's own signature is shaped to always be able to
/// hand `child` off to whichever exit path is taken, with no placeholder/`Default` value ever
/// needed to satisfy a borrow.
async fn drive_attempt(
    mut child: SpawnedChild,
    progress: &mut AgentProgress,
    opts: &RunOptions,
    deadline_sleep: Option<tokio::time::Sleep>,
    control: &mut crate::exec::control::ControlMonitor,
) -> DriveOutcome {
    tokio::pin!(deadline_sleep);
    let cancel = opts.cancel.clone();
    let interrupt = opts.interrupt.clone();

    // pi's 1s activity timer (`execution.ts:896-905`): while control tracking is enabled, the
    // idle/long-running heuristics are re-evaluated on a fixed tick as well as on every observed
    // child event — otherwise a child that goes SILENT (the exact condition `needs_attention`
    // exists to diagnose) would never trip it, because nothing would arrive to trigger the check.
    // `interval_at` (not `interval`) because tokio's first `interval` tick completes immediately,
    // which would fire a spurious check at t=0.
    let mut activity_tick = control.enabled().then(|| {
        let period = Duration::from_millis(crate::exec::control::ACTIVITY_TICK_MS);
        tokio::time::interval_at(tokio::time::Instant::now() + period, period)
    });

    // Armed on the FIRST terminal assistant stop; once the grace window elapses without the child
    // exiting, the child is force-drained. `clean_terminal_stop` accumulates across every terminal
    // stop (pi's `||=`) for `forcedDrainAfterFinalSuccess`.
    let mut final_drain_at: Option<tokio::time::Instant> = None;
    // SUBA-S06: armed when the child is reaped with stdout still open; expiring it ends the read
    // loop so the post-loop `wait_final_drain()` can report the already-known exit status.
    let mut exit_drain_at: Option<tokio::time::Instant> = None;
    let mut clean_terminal_stop = false;
    // pi's `agentSettledReceived` (`execution.ts:595,862,1080` @v0.43.0): the child announced the WHOLE run
    // settled. Like `clean_terminal_stop` it is a "the child finished on purpose" witness, so a
    // forced drain after it is still coerced to success.
    let mut agent_settled = false;
    // R-SA-037: set once the child's NDJSON shows a blocking `contact_supervisor` ask; the ask is
    // surfaced via `spawn_clarify` exactly once (the guard below), and this flag then rides out to
    // the attempt's `detached` outcome (bypassing acceptance; the ladder does not advance past it).
    let mut detached_seen = false;
    // SUBA-008 — pi's four `updateTurnBudget` locals (`turnBudgetSoftReached` plus the three
    // `result.turnBudget*` fields, `execution.ts:483`/`:567-569`/`:759-782`), gathered into one
    // value. Unarmed (and therefore inert on every path below) unless this run declared a budget.
    let mut turn_budget = crate::exec::turn_budget::TurnBudgetTracker::new(
        opts.turn_budget,
        opts.enforce_hard_turn_limit,
    );

    loop {
        let deadline_arm = async {
            match deadline_sleep.as_mut().as_pin_mut() {
                Some(sleep) => sleep.await,
                None => std::future::pending::<()>().await,
            }
        };
        // A fresh `sleep_until` against the fixed grace instant each iteration is correct: it
        // always resolves at the same absolute time regardless of how often it is reconstructed,
        // and reduces to `pending()` (never fires) until the window is armed.
        let final_drain_arm = async {
            match final_drain_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };

        // SUBA-S06: same fixed-instant reconstruction as `final_drain_arm` above, and `pending()`
        // (never fires) until the child is actually reaped.
        let exit_drain_arm = async {
            match exit_drain_at {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            biased;
            () = cancel.cancelled() => {
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: false,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            () = interrupt.cancelled() => {
                // pi `execution.ts:1090`: a soft interrupt CLEARS the activity state, so a
                // needs-attention notice that was raised (and is still sitting in the parent's
                // debounce window) fails its actionability re-check rather than landing in the
                // transcript for a run the caller has already deliberately paused.
                control.clear_activity_state();
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: true,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            () = deadline_arm => {
                // R-SA-036: timeout is a SOFT interrupt, not an immediate hard kill — it still
                // walks the full SIGINT->SIGTERM->SIGKILL ladder via `terminate`, exactly like
                // cancel/interrupt above; what makes it a timeout rather than a plain
                // cancellation is the `timed_out: true` flag, which is what `run_fallback_ladder`
                // (R-SA-036/6.3.2) actually branches on to stop the ladder outright.
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: true,
                    interrupted: false,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            step = child.next_event_or_exit() => {
                match step {
                    crate::spawn::ChildStep::Line(Ok(line)) => {
                        // NOTE: the raw NDJSON envelope deliberately does NOT enter
                        // `progress.recent_output` — pi appends only EXTRACTED text, from an
                        // assistant `message_end`'s content and a finished tool call's result, and
                        // `AgentProgress::record_event` does exactly that a few lines below. A raw
                        // line here would put an unrenderable (and, before
                        // `RECENT_OUTPUT_LINE_CHARS`, unbounded) JSON blob on the very field
                        // `SingleResult::progress` publishes as pi's `recentOutput`.
                        // Live-telemetry tee (pi's child-event pump, `subagent-runner.ts:1430`):
                        // hand the raw NDJSON line to the background runner's sink, if one is
                        // installed, BEFORE this module parses/folds it — so the runner folds it
                        // into `status.json` live without this module depending on `background`.
                        if let Some(sink) = &opts.live_events {
                            sink.emit(&line.raw);
                        }
                        // `SpawnedChild::next_event` parses against the spawn boundary's own
                        // narrow `spawn::NdjsonEvent` (progress-bookkeeping fields only, arch-SA
                        // §6.4); this module needs the fuller `exec::ndjson::SubagentEvent` union
                        // (final-output extraction, R-SA-029; completion-guard scanning,
                        // R-SA-034), so the identical raw line is re-parsed here through
                        // `ndjson::parse_line` — both are independent, tolerant views over the
                        // exact same wire bytes (`exec/ndjson.rs`'s own module doc), not a
                        // layering of one on top of the other.
                        if let Some(event) = crate::exec::ndjson::parse_line(&line.raw) {
                            // Final-stop grace-drain (pi `startFinalDrain`, execution.ts:584-605):
                            // open the grace window on the FIRST terminal assistant stop and track
                            // whether ANY terminal stop was clean (no errorMessage) for
                            // `forcedDrainAfterFinalSuccess`.
                            let terminal_stop = is_terminal_assistant_stop(&event);
                            if terminal_stop {
                                clean_terminal_stop =
                                    clean_terminal_stop || !message_end_has_error_message(&event);
                            }
                            if matches!(event, crate::exec::ndjson::SubagentEvent::AgentSettled) {
                                agent_settled = true;
                            }
                            // pi `applyChildLifecycle(projectChildLifecycle(evt))` — run for EVERY
                            // event (`execution.ts:844`), plus the terminal-stop form at `:947`.
                            // The three arms are: `agent_end{willRetry:true}` DISARMS the window
                            // (the child is about to retry — force-killing it there kills a run
                            // that is still working); `agent_settled` and a terminal assistant stop
                            // ARM it; everything else leaves it alone.
                            let will_retry = matches!(
                                event,
                                crate::exec::ndjson::SubagentEvent::AgentEnd {
                                    will_retry: true,
                                    ..
                                }
                            );
                            match crate::exec::child_protocol::project_child_lifecycle(
                                event.kind(),
                                will_retry,
                                terminal_stop,
                            ) {
                                crate::exec::child_protocol::ChildLifecycleAction::CancelDrain => {
                                    final_drain_at = None;
                                }
                                crate::exec::child_protocol::ChildLifecycleAction::StartDrain => {
                                    if final_drain_at.is_none() {
                                        final_drain_at = Some(
                                            tokio::time::Instant::now()
                                                + Duration::from_millis(FINAL_STOP_GRACE_MS),
                                        );
                                    }
                                }
                                crate::exec::child_protocol::ChildLifecycleAction::None => {}
                            }
                            // R-SA-037 detach-trigger arm: a child's blocking `contact_supervisor`
                            // ask (`need_decision`/`interview`) surfaces the ask to the parent's
                            // human via the real `ClarifyChannel` (fired exactly once) and marks this
                            // attempt detached. The intercom answer routes back to the still-alive
                            // child over the BROKER (independent of this stdout pipe), so the loop
                            // keeps driving — it neither kills nor synchronously blocks on the child.
                            if !detached_seen
                                && let Some(prompt) = contact_supervisor_block_prompt(&event)
                            {
                                detached_seen = true;
                                if let Some(dispatch) = &opts.clarify {
                                    // Dropping the returned receiver does not cancel the ask (a human
                                    // may still be answering); it only means this loop does not itself
                                    // await the outcome — the child unblocks over the broker instead.
                                    let _rx = crate::tui::intercom::spawn_clarify(
                                        dispatch.lock.clone(),
                                        dispatch.session_key.clone(),
                                        crate::tui::intercom::ClarifyRequest {
                                            run_id: dispatch.run_id.clone(),
                                            step_index: dispatch.step_index,
                                            prompt,
                                        },
                                    );
                                }
                            }
                            // pi `processLine` (`execution.ts:775-890`): every parsed child event
                            // is fresh activity for the control heuristics, and the tool-start /
                            // tool-result / assistant-turn folds feed the thresholds. Driven
                            // BEFORE `record_event` because that consumes the event by value.
                            control.observe_event(
                                &event,
                                crate::background::now_epoch_millis_pub(),
                            );
                            // SUBA-008 — the two per-message inputs `updateTurnBudget` needs, read
                            // BEFORE `record_event` consumes the event by value (same reason the
                            // control fold above runs here).
                            let assistant_turn = is_assistant_message_end(&event);
                            let has_tool_call = message_end_has_tool_call(&event);
                            let terminal_structured_output_call = opts
                                .structured_output_schema
                                .is_some()
                                && is_sole_structured_output_tool_call(&event);
                            progress.record_event(event);

                            // pi `execution.ts:910-924`: an ASSISTANT `message_end` is one turn,
                            // and the budget is re-evaluated on it. `progress.turn_count()` is
                            // this port's `result.usage.turns` — pi keeps the two in lockstep
                            // (`:913-914`) and cyrup derives the one from the other rather than
                            // carrying a second counter that could drift.
                            if assistant_turn && turn_budget.is_armed() {
                                let turn_count = u64::from(progress.turn_count());
                                // pi's third argument: `hasToolCall || Boolean(progress.currentTool)`
                                // (`:924`) — tool work either STARTING on this very message or
                                // still in flight from an earlier one. This is what makes the
                                // deferral arm reachable.
                                let tool_work_active_or_starting =
                                    has_tool_call || progress.current_tool.is_some();
                                let effect = turn_budget.observe_assistant_turn(
                                    turn_count,
                                    terminal_stop || terminal_structured_output_call,
                                    tool_work_active_or_starting,
                                    false,
                                );
                                match effect {
                                    crate::exec::turn_budget::TurnBudgetEffect::None => {}
                                    crate::exec::turn_budget::TurnBudgetEffect::SoftNote(note) => {
                                        // pi `appendRecentOutput(progress, [turnBudgetSoftNote(...)])`
                                        // (`:769`) — the wrap-up request reaches the operator
                                        // through the run's own output tail, once.
                                        progress.append_recent_output(&note);
                                    }
                                    crate::exec::turn_budget::TurnBudgetEffect::Abort {
                                        message,
                                        soft_note,
                                    } => {
                                        if let Some(note) = soft_note {
                                            progress.append_recent_output(&note);
                                        }
                                        // pi `requestTurnBudgetAbort` (`:733-757`): SIGINT now,
                                        // SIGTERM 1 s later, SIGKILL 4 s after the SIGINT. That is
                                        // exactly this ladder with the two graces pinned — 1 s to
                                        // escalate off SIGINT and 3 s more to escalate off SIGTERM
                                        // lands the kill at t+4 s, upstream's own instant.
                                        //
                                        // [CYRUP-DELTA]: pi ARMS the two timers and lets the run
                                        // keep reading the child's stdout in the meantime, so a
                                        // child that wraps up inside the window still delivers its
                                        // final output; this ladder blocks the drive loop for the
                                        // same wall-clock window instead, because
                                        // `SpawnedChild::terminate` consumes the child and cyrup
                                        // has no seam that signals without taking it. The observed
                                        // outcome is the same on both timelines — the child either
                                        // dies on SIGINT (the ladder returns immediately) or is
                                        // escalated on upstream's schedule — but a late final
                                        // message written after the SIGINT is dropped here where
                                        // upstream would have read it, which is why the abort
                                        // message doubles as `final_output` below.
                                        let outcome = child
                                            .terminate_with_graces(
                                                &cancel,
                                                crate::spawn::signal::EscalationGraces {
                                                    sigint: Duration::from_millis(
                                                        crate::exec::turn_budget::TURN_BUDGET_TERMINATION_DELAY_MS,
                                                    ),
                                                    sigterm: Duration::from_millis(
                                                        crate::exec::turn_budget::TURN_BUDGET_HARD_KILL_DELAY_MS
                                                            - crate::exec::turn_budget::TURN_BUDGET_TERMINATION_DELAY_MS,
                                                    ),
                                                },
                                            )
                                            .await;
                                        // `message` is not carried on the outcome: it is
                                        // recomputed verbatim from the tracker's own state by
                                        // `TurnBudgetTracker::terminal_note`, which is the single
                                        // place `run_attempt` reads it from, so there is exactly
                                        // one producer of upstream's string.
                                        debug_assert_eq!(
                                            turn_budget.terminal_note(),
                                            Some(
                                                crate::exec::turn_budget::TurnBudgetTerminalNote::Exceeded(
                                                    message.clone()
                                                )
                                            )
                                        );
                                        drop(message);
                                        return DriveOutcome {
                                            timed_out: false,
                                            interrupted: false,
                                            // NOT a forced drain: `forcedDrainAfterFinalSuccess`
                                            // must never coerce a turn-budget abort to exit 0.
                                            forced_termination: false,
                                            clean_terminal_stop,
                                            exit_status: outcome.map(|o| Some(o.status)),
                                            detached: detached_seen,
                                            agent_settled,
                                            protocol_error: None,
                                            turn_budget,
                                        };
                                    }
                                }
                            }
                        }
                    }
                    crate::spawn::ChildStep::ProtocolLimit(limit) => {
                        // pi `failProtocol` (`execution.ts:1026-1041`): the diagnostic becomes the
                        // run's error and the child is signalled down (upstream SIGTERM then, 3s
                        // later, SIGKILL — cyrup routes every forced termination through
                        // `terminate`'s own SIGINT->SIGTERM->SIGKILL ladder instead of inventing a
                        // second one). Nothing further can be read: the reader is permanently
                        // closed, so continuing to poll it would spin on `Eof`.
                        let outcome = child.terminate(&cancel).await;
                        return DriveOutcome {
                            timed_out: false,
                            interrupted: false,
                            // NOT a forced *drain*: upstream's `failProtocol` deliberately does not
                            // set `forcedTerminationSignal`, so the clean-drain coercion to exit 0
                            // cannot swallow a protocol failure. (It could not anyway — that
                            // coercion also requires no error, and this sets one.)
                            forced_termination: false,
                            clean_terminal_stop,
                            exit_status: outcome.map(|o| Some(o.status)),
                            detached: detached_seen,
                            agent_settled,
                            protocol_error: Some(limit),
                            turn_budget: turn_budget.clone(),
                        };
                    }
                    crate::spawn::ChildStep::Line(Err(_)) | crate::spawn::ChildStep::Eof => {
                        // Stdout EOF (child exited/closed stdout) or a genuine read fault — either
                        // way, stop reading and wait for the real exit status below.
                        break;
                    }
                    crate::spawn::ChildStep::Exited(_) => {
                        // SUBA-S06: the process is gone but stdout is STILL OPEN, because a
                        // surviving grandchild inherited the write end. The EOF this loop used to
                        // wait on can never arrive, and none of the other arms is guaranteed to
                        // fire either — `deadline_arm` only exists when the caller passed a
                        // timeout, `final_drain_arm` only after a terminal assistant stop the
                        // child never emitted, and the activity tick merely re-scores heuristics.
                        // So the tool call hung forever, spinning once a second.
                        //
                        // Do NOT break here: lines written before the exit may still be buffered
                        // in the pipe, and dropping them would trade a hang for silent output
                        // loss. Arm a bounded post-exit window instead and keep draining; the
                        // status itself is deliberately discarded because the post-loop
                        // `wait_final_drain()` re-reads it (the child is marked reaped, so that
                        // call returns immediately) and routes it through the ONE existing clean
                        // path — which is what keeps this a normal exit rather than a
                        // `forced_termination`.
                        if exit_drain_at.is_none() {
                            exit_drain_at = Some(
                                tokio::time::Instant::now()
                                    + Duration::from_millis(POST_EXIT_DRAIN_MS),
                            );
                        }
                    }
                }
            }
            () = exit_drain_arm => {
                // SUBA-S06: the reaped child's buffered stdout has had its beat; whatever still
                // holds the pipe open is not this run's problem. Break (never return) so the exit
                // status flows through the normal post-loop path as an ordinary clean exit.
                break;
            }
            () = final_drain_arm => {
                // The child emitted its terminal stop but did not exit within the grace window —
                // force-drain it through the real signal ladder (pi's SIGTERM->SIGKILL). Whether
                // this is coerced back to success (`forcedDrainAfterFinalSuccess`) is decided in
                // `run_attempt` from `forced_termination` + `clean_terminal_stop` + no error.
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: false,
                    forced_termination: true,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
                    agent_settled,
                    protocol_error: None,
                    turn_budget: turn_budget.clone(),
                };
            }
            () = async {
                match activity_tick.as_mut() {
                    Some(tick) => { tick.tick().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                // pi's `setInterval(..., 1000)` body (`execution.ts:898-904`), minus the
                // `fireUpdate()` half: this crate's live-progress payload is assembled by
                // `tui::events` off the same NDJSON stream, so the tick's job here is purely to
                // re-evaluate the idle/long-running heuristics on a silent child.
                control.update_activity_state(crate::background::now_epoch_millis_pub());
            }
        }
    }

    match child.wait_final_drain().await {
        Ok(Some(status)) => {
            child.finish(); // R-SA-067: success-path temp-file cleanup.
            DriveOutcome {
                timed_out: false,
                interrupted: false,
                forced_termination: false,
                clean_terminal_stop,
                exit_status: Ok(Some(status)),
                detached: detached_seen,
                agent_settled,
                protocol_error: None,
                turn_budget: turn_budget.clone(),
            }
        }
        Ok(None) => {
            // The child closed stdout but did not exit within FINAL_DRAIN_TIMEOUT (R-SA-068) —
            // fall back to the real signal-escalation ladder. This is a forced termination too:
            // combined with a clean terminal stop and no error, `forcedDrainAfterFinalSuccess`
            // still coerces an otherwise-successful, merely-slow-to-teardown run to exit 0.
            let outcome = child.terminate(&cancel).await;
            DriveOutcome {
                timed_out: false,
                interrupted: false,
                forced_termination: true,
                clean_terminal_stop,
                exit_status: outcome.map(|o| Some(o.status)),
                detached: detached_seen,
                agent_settled,
                protocol_error: None,
                turn_budget: turn_budget.clone(),
            }
        }
        Err(err) => DriveOutcome {
            timed_out: false,
            interrupted: false,
            forced_termination: false,
            clean_terminal_stop,
            exit_status: Err(err),
            detached: detached_seen,
            agent_settled,
            protocol_error: None,
            turn_budget: turn_budget.clone(),
        },
    }
}

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
/// `run_sync`'s three inputs to it.
fn resolve_run_acceptance(
    opts: &RunOptions,
    agent: &AgentConfig,
    task: &str,
) -> AcceptanceContract {
    AcceptanceContract::resolve_effective(opts.acceptance.clone(), &agent.name, task)
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
/// 4. Drive [`fallback::run_fallback_ladder`] against a [`SpawnedChildAttemptRunner`] — every
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
/// end-to-end within this crate: [`drive_attempt`]'s NDJSON loop sets its `detached` observation
/// the moment a child emits a blocking `contact_supervisor` ask (`contact_supervisor_block_prompt`)
/// and fires [`crate::tui::intercom::spawn_clarify`] against the executor's single-slot
/// [`crate::tui::intercom::AskLock`] (backed in production by the intercom companion's real broker
/// `ClarifyChannel`, threaded via `SubagentsExtension::with_channels` → `RunOptions::clarify`);
/// [`SpawnedChildAttemptRunner::run_attempt`] then carries that observation onto
/// `AttemptSignal::detached`, which this function reads (via the `detached` binding below) to skip
/// acceptance/completion-guard/truncation. See [`crate::exec::fallback::AttemptSignal::detached`]'s
/// doc comment for the full CLOSED wiring. When no clarify channel is wired (headless / SDK-embedder
/// / `RunOptions::clarify = None`), the drive loop still marks the attempt detached but the `AskLock`
/// degrades to its no-live-channel fallback rather than blocking.
pub async fn run_sync(agent: &AgentConfig, task: &str, opts: &RunOptions) -> SingleResult {
    // Step 0 (R-SA-055, SAFETY-CRITICAL): the recursion-depth guard MUST run before any spawn,
    // discovery, or worktree setup — this is `run_sync`'s very first action, ahead of even
    // R-SA-025's output-mode validation below, because `run_sync` is the sole chokepoint every
    // production spawn path in this crate funnels through (the foreground single-run tool
    // dispatch, the background hop-2 runner's per-step loop, and — via `chain_graph::walk_chain`/
    // `spawn::parallel::run_bounded`'s `SingleStepExecutor` seam — every chain step, parallel
    // fan-out child, and dynamic fan-out child as well). A blocked check returns an error result
    // telling the caller to complete the task directly, per R-SA-055's own text, and — because
    // this check precedes every other line of this function — zero subprocesses are ever spawned
    // for a blocked attempt.
    if crate::spawn::depth::is_blocked(&agent.depth) {
        let err = SubagentError::DepthExceeded {
            current: agent.depth.current_depth,
            max: agent.depth.max_depth,
        };
        return SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
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
            error: Some(err.to_string()),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };
    }

    // Step 1 (R-SA-025): fail fast before any subprocess spawns.
    if let Some(err) = validate_file_only_requires_path(opts.output_mode, opts.output_path.as_deref())
    {
        return SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
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
            error: Some(err.to_string()),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };
    }

    // Step 2 (R-SA-023): resolve the effective acceptance contract.
    let contract = resolve_run_acceptance(opts, agent, task);

    // Step 3 (R-SA-038).
    // SUBA-003: pi passes `{ scope: options.modelScope }` here (`execution.ts:1065-1070`), which
    // warns (never filters) for out-of-scope FALLBACK candidates. The ladder returned is identical
    // either way — an out-of-scope fallback is still attempted, exactly as upstream, because
    // dropping it would silently change which model ran.
    let (candidates, _scope_warnings) = crate::exec::fallback::build_model_candidates_scoped(
        &opts.model_override,
        agent.model.as_ref(),
        &agent.fallback_models,
        &opts.available_models,
        opts.model_scope.as_ref(),
    );

    if candidates.is_empty() {
        return SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
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
            error: Some(
                "no candidate model available for this subagent run (empty fallback ladder)"
                    .to_string(),
            ),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };
    }

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
            return SingleResult {
                // SUBA-021: no usage budget on this path (see the field doc).
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
                error: Some(format!(
                    "Skills not found: {}",
                    crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL
                )),
                saved_output_path: None,
                tool_calls: Vec::new(),
                output_truncated: false,
                control_events: Vec::new(),
                progress: None,
            };
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
        return SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
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
            error: Some(format!("failed to prepare subagent scratch directory: {err}")),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };
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
    // different file than the one read back below.
    //
    // A creation failure degrades to `None` rather than failing the run: the child then never
    // receives the env vars, never registers `structured_output`, and the read-back reports pi's
    // own "missing" hard failure — which is the correct outcome for "the schema never reached the
    // child", and strictly better than aborting a run that might still produce useful prose. It is
    // NOT a licence to go looking for the value somewhere pi never looks: see the read-back below.
    //
    // SUBA-S01 residual: the runtime is held by a `StructuredOutputCleanupGuard`, which is the RAII
    // port of pi's `finally { if (!r?.detached) cleanupStructuredOutputRuntime(structuredRuntime); }`
    // (`runs/foreground/subagent-executor.ts:3780-3787` @v0.43.0). See that type's own doc for why
    // the end-of-function statement this replaces was wrong on BOTH halves.
    let mut structured_guard = opts
        .structured_output_schema
        .as_ref()
        .and_then(|schema| {
            crate::exec::structured::create_structured_output_runtime(schema, &scratch_dir).ok()
        })
        .map(crate::exec::structured::StructuredOutputCleanupGuard::new);
    let structured_runtime = structured_guard
        .as_ref()
        .map(|guard| guard.runtime().clone());

    // Step 4: drive the fallback ladder.
    let mut runner = SpawnedChildAttemptRunner {
        agent,
        task,
        opts,
        contract: &contract,
        scratch_dir,
        skill_injection,
        attempt_index: 0,
        structured_runtime: structured_runtime.clone(),
        // SUBA-014 / pi `runs/foreground/execution.ts:322,357` @v0.43.0:
        // `requireReadTool: Boolean(shared.resolvedSkillNames?.length)`. `resolved_skill_names` is
        // `Some` exactly when at least one declared skill resolved to a `SKILL.md`, so `is_some()`
        // IS `Boolean(...?.length)` — a declared-but-unresolvable skill grants nothing, matching
        // upstream, which derives the flag from the RESOLVED list rather than the requested one.
        require_read_tool: resolved_skill_names.is_some(),
    };
    let outcome = run_fallback_ladder(&candidates, &mut runner).await;

    let winning_model = outcome.attempted_models.last().cloned();
    let last_signal = outcome.last_signal;
    let last_attempt = outcome.last_attempt;

    let (timed_out, interrupted, detached, process_signal, mut exit_code, mut error, mut final_output) =
        match (&last_signal, &last_attempt) {
            (Some(signal), Some(record)) => (
                signal.timed_out,
                // A soft interrupt is carried on the runner's own per-attempt payload
                // ([`AttemptRecord::interrupted`], not on `AttemptSignal` which this crate does not
                // own); an interrupted attempt reports `success: true`/`exit_code: 0`, so the
                // ladder stops on it and this is the winning attempt whenever an interrupt fired
                // (pi `execution.ts:748-761`, T3 group A). The gates below (structured-output,
                // completion-guard, acceptance correction) all skip for a non-clean gate, so the
                // paused-success `final_output` reaches the caller untouched.
                record.interrupted,
                signal.detached,
                // G104 — pi `if (signal) result.processSignal = signal;` (`execution.ts:1081`).
                // The value was already computed by `process_signal_name` and stashed on the
                // attempt's `StartupEvidence`; publishing it on the terminal `SingleResult` is what
                // makes `resolveSubagentResultStatus`'s unexplained-signal → `"stopped"` branch
                // (`result-intercom.ts:35`) reachable at all.
                signal.startup.process_signal.clone(),
                signal.exit_code.unwrap_or(if signal.success { 0 } else { 1 }),
                signal.error.clone(),
                record.final_output.clone(),
            ),
            _ => (
                false,
                false,
                false,
                None,
                1,
                Some("subagent fallback ladder produced no attempt outcome".to_string()),
                None,
            ),
        };

    // Timeout message + partial-output preamble (pi `execution.ts:824-829`): a timed-out run's
    // delivered output leads with `Subagent timed out after {ms}ms.`, and — when the child produced
    // any partial output before the deadline fired — that partial output follows under a
    // `Partial output before timeout:` heading. Applied here, right after the ladder settles and
    // before the output-path handoff / truncation, exactly as pi applies it right after extracting
    // `fullOutput`. The nominal budget is `opts.timeout_ms` (pi `formatTimeoutMessage(options
    // .timeoutMs ?? 0)`), distinct from the wall-clock `deadline_at` that actually fired the timer.
    //
    // SUBA-008 — pi's `else if` chain continues from the SAME `if (result.timedOut)`
    // (`execution.ts:1241-1258`), so a timed-out run never also gets a turn-budget preamble even
    // when both fired. That is why the three turn-budget arms are `else` on this branch and not a
    // second independent `if`.
    let turn_budget_tracker = last_attempt
        .as_ref()
        .map(|record| record.turn_budget.clone())
        .unwrap_or_default();
    if timed_out {
        let timeout_message = format_timeout_message(opts.timeout_ms.unwrap_or(0));
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

    // R-SA-031: file-only/output-path handoff, once, against the aggregate captured output. Tracks
    // the concrete saved path (`Some` only when the file was actually written — by the child, or by
    // the orchestrator persisting its own captured output), which the saved-output reference message
    // below (pi `finalizeSingleOutput`, `single-output.ts:211-235`) is gated on. pi resolves the
    // handoff only for a clean run (`finalResult?.exitCode === 0`, `subagent-runner.ts:872`), so this
    // is gated on the same clean-completion condition rather than run unconditionally.
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
                    error = Some(match error {
                        Some(existing) => format!("{existing}; {handoff_error}"),
                        None => handoff_error,
                    });
                }
            }
        }
    }
    // The FULL (untruncated) persisted content the saved-output reference measures its byte/line
    // counts over (pi `formatSavedOutputReference(savedPath, output)` uses the pre-truncation output,
    // `subagent-runner.ts:876`) — captured here, before step 9's truncation reassigns `final_output`.
    let full_output_for_reference = final_output.clone();

    // The WINNING attempt's progress fold AND its live-control monitor (pi keeps both as locals of
    // the same `runSingleAttempt` scope; this crate has to carry them out of the ladder because
    // its post-settlement guard/acceptance steps live one level up, in `run_sync`).
    let (progress, mut control) = match last_attempt {
        Some(record) => (record.progress, record.control),
        None => (
            AgentProgress::default(),
            crate::exec::control::ControlMonitor::disabled(),
        ),
    };

    // Step 5 (R-SA-030): structured-output extraction + parent-side JSON-Schema re-validation.
    // Only evaluated on an otherwise-clean run (mirrors the completion-guard/acceptance gate's own
    // "don't re-diagnose an already-failed attempt" discipline just below) — a run that already
    // failed for another reason (non-zero exit, timeout, detach, interrupt) must not additionally
    // be re-labeled by a structured-output check that never had a fair chance to run against a
    // clean transcript.
    let structured_output = if (CleanCompletionGate {
        exit_code,
        detached,
        interrupted,
        timed_out,
    })
    .is_clean()
    {
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
        let structured_outcome = match structured_runtime.as_ref() {
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
                exit_code = 1;
                error = Some(match error {
                    Some(existing) if !existing.trim().is_empty() => {
                        format!("{existing}; {}", crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR)
                    }
                    _ => crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR.to_string(),
                });
                None
            }
            StructuredOutcome::Invalid(message) => {
                exit_code = 1;
                error = Some(match error {
                    Some(existing) if !existing.trim().is_empty() => {
                        format!("{existing}; {message}")
                    }
                    _ => message,
                });
                None
            }
        }
    } else {
        None
    };

    // Step 6 (R-SA-034): completion-mutation guard — needs a real AgentDefinition-shaped view;
    // `evaluate_completion_mutation_guard` only reads `local_name`/`tools`/`completion_guard`, so
    // a minimal projection is built here rather than requiring `AgentConfig` to carry every other
    // `AgentDefinition` field this guard never touches.
    let guard_agent = completion_guard_projection(agent);
    let guard_result =
        evaluate_completion_mutation_guard(&guard_agent, task, &progress.all_events);

    let clean_gate = CleanCompletionGate {
        exit_code,
        detached,
        interrupted,
        timed_out,
    };

    if clean_gate.is_clean() && guard_result.triggered {
        exit_code = 1;
        error = Some(match error {
            Some(existing) if !existing.trim().is_empty() => format!(
                "{existing}; {}",
                crate::exec::completion_guard::COMPLETION_GUARD_ERROR_MESSAGE
            ),
            _ => crate::exec::completion_guard::COMPLETION_GUARD_ERROR_MESSAGE.to_string(),
        });
        // pi `execution.ts:1234-1247`: the guard also raises a `needs_attention` control event with
        // `reason: "completion_guard"` — the one raise that happens AFTER the child is gone, and
        // the one the notice renderer formats as the "Subagent failed: <agent>" body rather than
        // the steer/resume nudge. Shares the winning attempt's dedup set (`control` is that
        // attempt's own monitor), exactly as the source's shared `emittedControlEventKeys` does.
        control.emit_completion_guard_notice(
            crate::background::now_epoch_millis_pub(),
            format!(
                "{} completed without making edits for an implementation task",
                agent.name
            ),
        );
    }

    // Re-derive the gate AFTER the completion-guard correction above, since R-SA-033's own
    // acceptance-gate condition must observe the POST-guard exit code (a run the completion
    // guard already failed must not additionally run acceptance evaluation against a stale
    // "exit_code == 0" snapshot).
    let post_guard_gate = CleanCompletionGate {
        exit_code,
        detached,
        interrupted,
        timed_out,
    };

    // Step 7 (R-SA-032) + Step 8 (R-SA-033), unless R-SA-037 bypasses both entirely.
    let acceptance_ledger = if detached {
        None
    } else if timed_out {
        // pi `buildTimedOutAcceptanceLedger` (`execution.ts:101-113`, applied at `1089-1090`): a
        // timed-out run's ledger is `rejected` (unless the contract required no acceptance at all,
        // in which case it stays `not-required`), NEVER the `not-required` a non-clean gate would
        // otherwise yield from `evaluate_acceptance`, and it carries a failed timeout runtime check.
        // No post-hoc exit-code correction runs — pi gates that on `!result.timedOut`
        // (`execution.ts:1098`), and the run already failed via the timeout path (exit_code != 0).
        Some(build_timed_out_acceptance_ledger(&contract))
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
            &contract,
            post_guard_gate,
            final_output.as_deref(),
            guard_result,
            &opts.cwd,
            memo,
            file_output,
            &opts.cancel,
        )
        .await;

        let correction =
            apply_post_hoc_correction(&ledger, contract.explicit, post_guard_gate, error.as_deref());
        exit_code = correction.exit_code;
        error = correction.error;

        Some(ledger)
    };

    // Strip trailing acceptance-report fences from the DELIVERED output (pi `stripAcceptanceReport`,
    // execution.ts:823/857). The acceptance gate above already consumed the RAW report block for its
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
                let result = truncate_output(&text, agent.max_output, None);
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
    let final_output = match (&saved_output_path, detached) {
        (Some(saved), false) if exit_code == 0 => {
            let full = crate::exec::acceptance::model::strip_acceptance_report(
                &full_output_for_reference.clone().unwrap_or_default(),
            );
            let reference = crate::exec::output::format_saved_output_reference(saved, &full);
            match opts.output_mode {
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

    // Step 10 (R-SA-043): compaction, and its ONE documented opt-out.
    //
    // `SingleResult` is unconditionally the compacted shape — no raw per-turn messages, only
    // summarized `tool_calls`. `include_progress` restores exactly one thing on top of that: this
    // run's own `AgentProgress` projection, which pi gates identically (`progress:
    // params.includeProgress ? allProgress : undefined`, `subagent-executor.ts:3008` for SINGLE and
    // `:2679` for PARALLEL @v0.34.0). With the flag off or omitted the field stays `None` and
    // `skip_serializing_if` drops it, so a returned/persisted result is byte-for-byte what it was
    // before the field existed.
    //
    // Assembled HERE, from the winning attempt's fold plus this function's settled locals, because
    // that is where pi assembles it too: `execution.ts` mutates the one `progress` object at
    // `:907-913` @v0.34.0 and hands it out as `result.progress`. Deliberately NOT reusing the
    // orchestrator-layer `tui::events::LiveProgressFold` — that fold only exists on the streaming
    // foreground path (it is installed only when an `on_update` sink is present), so the detached
    // hop-2 runner and every non-streaming caller would get nothing.
    let progress_snapshot = if opts.include_progress == Some(true) {
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
            model: apply_thinking_suffix(
                winning_model.as_ref().map(ModelId::as_str),
                agent.thinking.as_deref(),
            ),
            thinking: agent.thinking.clone(),
            status,
            // pi `progress.activityState`, owned by the control state machine; the winning
            // attempt's monitor is the one `run_sync` carried out of the ladder, and it already
            // cleared the state on a soft interrupt exactly as pi does at `:832,854`.
            activity_state: control.activity_state(),
            error: error.clone(),
        });
        // pi `compactForegroundDetails` → `compactCompletedProgress` (`shared/utils.ts:330-347`):
        // a SETTLED snapshot keeps eleven fields and empties the two growth terms.
        Some(snapshot.compact_completed())
    } else {
        None
    };

    // SUBA-S01 (pi `cleanupStructuredOutputRuntime`, `structured-output.ts:175-182`, invoked from
    // `subagent-executor.ts:3780-3787`'s `finally`): the removal itself is `structured_guard`'s
    // `Drop`, so it happens on EVERY exit from `run_sync` — including a cancellation that drops
    // this future mid-ladder, which an end-of-function statement could never cover.
    //
    // This statement is upstream's `if (!r?.detached)` guard and nothing else. A detached run's
    // child is still alive (R-SA-037) and has not written its capture file yet; that file lives
    // inside the very directory cleanup removes. pi says so in its own words at `:3782-3784` — "A
    // successful detached receipt transfers both to onDetachedExit while the authoritative
    // completion remains live" — and defers the cleanup to `onDetachedExit`'s inner `finally`
    // (`:3757-3761`). Disarming is that transfer. Before this, cyrup deleted the directory out from
    // under the live child on every detach, so a detached run could never produce a structured
    // value at all and the child's `structured_output` call would fail on a vanished parent dir.
    if detached && let Some(guard) = structured_guard.as_mut() {
        guard.disarm();
    }

    // SUBA-021 — the USAGE budget's terminal check (pi `subagent-runner.ts:4403-4411`):
    //
    //     setOptionalProperty(statusPayload, "usageBudget", usageBudgetState(config.usageBudget, currentUsageTotals()));
    //     if (usageBudgetExceeded && statusPayload.usageBudget && !statusPayload.error)
    //         statusPayload.error = usageBudgetExceededMessage(statusPayload.usageBudget);
    //
    // Computed from the run's AGGREGATE usage (every attempt of the fallback ladder, not just the
    // winning one) because that is what the run actually spent. `!statusPayload.error` is
    // load-bearing: a run that already failed keeps its own diagnosis — the budget did not cause
    // that failure and overwriting it would hide the real cause behind a bookkeeping note.
    let usage_budget = crate::exec::usage_budget::usage_budget_state(
        opts.usage_budget,
        Some(crate::exec::usage_budget::UsageTotals::from(
            &outcome.aggregate_usage,
        )),
    );
    let error = match (&error, usage_budget.as_ref()) {
        (None, Some(state)) if state.exhausted => Some(
            crate::exec::usage_budget::usage_budget_exceeded_message(state),
        ),
        _ => error,
    };

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

/// Project an [`AgentConfig`] down to the minimal [`AgentDefinition`] shape
/// [`evaluate_completion_mutation_guard`] actually reads (`local_name`, `tools`,
/// `completion_guard`) — every other field is populated with an inert default since the guard
/// never inspects them. Kept private and narrowly scoped rather than exposing a
/// `From<&AgentConfig> for AgentDefinition` impl crate-wide, since a "mostly-fake"
/// `AgentDefinition` is only ever valid for this one guard call, not as a general conversion.
fn completion_guard_projection(agent: &AgentConfig) -> AgentDefinition {
    AgentDefinition {
        default_turn_budget: None,
        name: agent.name.clone(),
        local_name: agent.name.clone(),
        package_name: None,
        description: String::new(),
        aliases: Vec::new(),
        tools: agent.tools.clone(),
        extensions: None,
        extensions_from_default: false,
        subagent_only_extensions: Vec::new(),
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
        let ctx = resolver.resolve(request.requested, request.index).await?;
        resolved.push(ctx);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    // `clippy::panic` joins the three that were already here: a test's `panic!` (and the
    // `unwrap_or_else(|| panic!(…))` this module uses to report WHICH argv it saw) is its failure
    // mechanism, not production code. Without it `cargo clippy --all-targets` fails on this module.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::exec::acceptance::AcceptanceStatus;

    /// SUBA-023 (consumer half): the signal name published on a run record must come from the
    /// crate's ONE mapping (`spawn::signal::signal_name_of`, which also fills
    /// `TerminationOutcome::signal_name`), not from a local three-entry table.
    ///
    /// RED before the fix: `process_signal_name` named only SIGINT/SIGKILL/SIGTERM, so a crashed
    /// child's `SingleResult.process_signal` read `SIG11`/`SIG6` where pi (passing Node's signal
    /// NAME through at `execution.ts:1081`) reports `SIGSEGV`/`SIGABRT`.
    #[cfg(unix)]
    #[test]
    fn a_crashed_child_reports_the_posix_signal_name_not_a_number() {
        use std::os::unix::process::ExitStatusExt as _;

        for (signal, expected) in [
            (2, "SIGINT"),
            (6, "SIGABRT"),
            (9, "SIGKILL"),
            (11, "SIGSEGV"),
            (15, "SIGTERM"),
            (1, "SIGHUP"),
        ] {
            assert_eq!(
                process_signal_name(&std::process::ExitStatus::from_raw(signal)).as_deref(),
                Some(expected),
                "signal {signal} must be named the way pi names it"
            );
        }

        // A normal exit names no signal at all (pi's `if (signal) result.processSignal = signal`).
        assert_eq!(process_signal_name(&std::process::ExitStatus::from_raw(0)), None);

        // …and a signal the shared table does not name still falls back to the numeric form rather
        // than disappearing, so nothing previously reported is lost.
        assert_eq!(
            process_signal_name(&std::process::ExitStatus::from_raw(64)).as_deref(),
            Some("SIG64")
        );
    }

    fn sample_agent_config(model: &str, fallback: &[&str]) -> AgentConfig {
        AgentConfig {
            name: "worker".to_string(),
            model: Some(ModelId::from(model)),
            fallback_models: fallback.iter().map(|m| ModelId::from(*m)).collect(),
            thinking: None,
            system_prompt_mode: SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            output: None,
            inherit_project_context: false,
            inherit_skills: true,
            skills: Vec::new(),
            completion_guard: Some(false),
            max_output: OutputCap::default(),
            max_subagent_depth: None,
            memory: None,
            tool_budget: None,
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
        }
    }

    fn base_opts(cwd: &std::path::Path, available: &[&str]) -> RunOptions {
        RunOptions {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            enforce_hard_turn_limit: false,
            model_scope: None,
            cwd: cwd.to_path_buf(),
            deadline_at: None,
            timeout_ms: None,
            output_path: None,
            output_mode: OutputMode::Inline,
            reads: None,
            steer_ack_dir: None,
            steer_capability_path: None,
            structured_output_schema: None,
            model_override: ModelOverride::Inherit,
            preferred_provider: None,
            available_models: available.iter().map(|m| ModelId::from(*m)).collect(),
            cancel: CancelToken::new(),
            interrupt: CancelToken::new(),
            share: None,
            session_dir: None,
            skills: None,
            runtime_cwd: None,
            include_progress: None,
            agent_scope: None,
            acceptance: Some(AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![])),
            fork_context: ForkContext::fresh(),
            live_events: None,
            parent_session_id: None,
            clarify: None,
            orchestrator_intercom_target: None,
            run_id: None,
            child_index: None,
            steer_inbox_dir: None,
            control_config: None,
            on_control_event: None,
            artifacts_dir: None,
        }
    }

    /// SUBA-021 — the CAPABILITY CEILING is consulted by `build_attempt_spawn_plan`, on both of its
    /// halves: the agent gate REFUSES a delegation outside the ceiling before any child is planned,
    /// and the resolved ceiling is handed to the child in
    /// [`crate::exec::capability_ceiling::CAPABILITY_CEILING_ENV`] so the bound survives the re-exec.
    ///
    /// Pre-fix BOTH observations were the opposite: `rg 'capability_ceiling' crates/…/src` was 0, so
    /// there was no ceiling concept at all — every agent was allowed and no env var was ever
    /// written, meaning a child could be granted a capability set wider than its parent's.
    ///
    /// The registration handle is scoped to this test and disposes on `Drop`, so it cannot leak into
    /// another test's session.
    #[test]
    fn the_capability_ceiling_refuses_an_out_of_ceiling_agent_and_reaches_the_child_env() {
        use crate::exec::capability_ceiling as cc;

        let dir = tempfile::tempdir().expect("tempdir");
        let session = "spawn-plan-ceiling-session";
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.parent_session_id = Some(session.to_string());

        let plan = |agent: &AgentConfig, opts: &RunOptions| {
            build_attempt_spawn_plan(
                agent,
                &ModelId::from("m1"),
                "task",
                opts,
                DepthEnvelope { current_depth: 0, max_depth: 5 },
                dir.path(),
                None,
            )
        };

        // No ceiling registered: the plan builds and writes no ceiling var (the overlay only adds,
        // so an unbounded run cannot inherit a stale bound).
        let unbounded = plan(&agent, &opts).expect("no ceiling, no refusal");
        assert!(!unbounded
            .spec
            .env_overlay
            .contains_key(cc::CAPABILITY_CEILING_ENV));

        let _handle = cc::register_capability_ceiling(
            session,
            "org-policy",
            &serde_json::json!({ "allowedAgents": ["reviewer"] }),
        )
        .expect("registers");

        // `worker` is outside the ceiling: refused BEFORE any argv/env is built, with pi's text.
        let Err(err) = plan(&agent, &opts) else {
            panic!("a plan outside the ceiling must be refused");
        };
        assert!(
            matches!(err, SubagentError::CapabilityCeilingViolation(_)),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Capability ceiling from org-policy does not allow agent 'worker'. Allowed agents: \
             reviewer."
        );

        // An allowed agent plans normally AND carries the ceiling down to the child.
        let mut allowed = sample_agent_config("m1", &[]);
        allowed.name = "reviewer".to_string();
        let Ok(planned) = plan(&allowed, &opts) else {
            panic!("an agent inside the ceiling plans normally");
        };
        let encoded = planned
            .spec
            .env_overlay
            .get(cc::CAPABILITY_CEILING_ENV)
            .expect("the ceiling crosses the process boundary");
        let decoded = cc::decode_capability_ceiling(Some(encoded))
            .expect("decodes")
            .expect("present");
        assert_eq!(decoded.allowed_agents, Some(vec!["reviewer".to_string()]));
        assert_eq!(decoded.sources, vec!["org-policy".to_string()]);
    }

    // ---- AgentProgress: R-SA-027/028 folding ----

    #[test]
    fn record_event_accumulates_usage_additively_across_multiple_message_end_events() {
        let mut progress = AgentProgress::default();
        let ev1 = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [],
                "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 15, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}
            }),
        };
        let ev2 = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [],
                "usage": {"input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 5, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}
            }),
        };
        progress.record_event(ev1);
        progress.record_event(ev2);
        assert_eq!(progress.usage.input, 13);
        assert_eq!(progress.usage.output, 7);
        assert_eq!(progress.message_end_events.len(), 2);
    }

    #[test]
    fn record_event_increments_tool_count_and_sets_current_tool() {
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            args: serde_json::Value::Null,
        });
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c2".into(),
            tool_name: "edit".to_string(),
            args: serde_json::Value::Null,
        });
        assert_eq!(progress.tool_count, 2);
        assert_eq!(progress.current_tool.as_deref(), Some("edit"));
    }

    #[test]
    fn recent_output_buffer_is_capped_at_50_lines_oldest_evicted_first() {
        let mut progress = AgentProgress::default();
        for i in 0..(RECENT_OUTPUT_CAP + 10) {
            progress.append_recent_output(&format!("line-{i}"));
        }
        assert_eq!(progress.recent_output.len(), RECENT_OUTPUT_CAP);
        assert_eq!(progress.recent_output.front().map(String::as_str), Some("line-10"));
        let expected_last = format!("line-{}", RECENT_OUTPUT_CAP + 9);
        assert_eq!(
            progress.recent_output.back().map(String::as_str),
            Some(expected_last.as_str())
        );
    }

    #[test]
    fn append_recent_output_keeps_pis_last_ten_nonblank_lines_of_one_chunk() {
        // pi `appendRecentOutput(progress, text.split("\n").slice(-10))`
        // (`runs/foreground/execution.ts:651,670` @v0.34.0): one chunk contributes at most its
        // last ten lines, blank lines are dropped by `lines.filter((line) => line.trim())`, and
        // the ORIGINAL (untrimmed) text of each surviving line is what is stored.
        let mut progress = AgentProgress::default();
        let mut chunk = String::new();
        for i in 0..25 {
            chunk.push_str(&format!("l{i}\n"));
        }
        progress.append_recent_output(&chunk);
        assert_eq!(progress.recent_output.len(), RECENT_OUTPUT_TAIL_LINES);
        assert_eq!(progress.recent_output.front().map(String::as_str), Some("l15"));
        assert_eq!(progress.recent_output.back().map(String::as_str), Some("l24"));

        let mut blanks = AgentProgress::default();
        blanks.append_recent_output("a\n\n   \n  b  \n");
        assert_eq!(
            blanks.recent_output.iter().cloned().collect::<Vec<_>>(),
            vec!["a".to_string(), "  b  ".to_string()],
            "blank lines are dropped; surviving lines keep their own leading/trailing space"
        );
    }

    #[test]
    fn append_recent_output_truncates_one_enormous_line_to_pis_char_cap() {
        // pi `boundStreamedRecentOutput` (`shared/utils.ts:450-456`), applied at append time per
        // this crate's documented delta. Without it, one 10 MB tool result line would ride out on
        // `SingleResult::progress.recent_output` for an interrupt-paused run, whose `running`
        // status `compact_completed` deliberately refuses to empty.
        let mut progress = AgentProgress::default();
        let huge = "x".repeat(RECENT_OUTPUT_LINE_CHARS * 3);
        progress.append_recent_output(&huge);
        let stored = progress
            .recent_output
            .front()
            .cloned()
            .expect("one line must be stored");
        assert_eq!(stored.chars().count(), RECENT_OUTPUT_LINE_CHARS + "… [truncated]".chars().count());
        assert!(stored.ends_with("… [truncated]"), "pi's suffix, verbatim");

        // A multi-byte line must be cut on a char boundary, not a byte one.
        let mut wide = AgentProgress::default();
        wide.append_recent_output(&"é".repeat(RECENT_OUTPUT_LINE_CHARS + 5));
        let stored = wide.recent_output.front().cloned().unwrap_or_default();
        assert_eq!(
            stored.chars().filter(|c| *c == 'é').count(),
            RECENT_OUTPUT_LINE_CHARS
        );

        // Exactly at the cap is NOT truncated (pi's `line.length > MAX` is strict).
        let mut exact = AgentProgress::default();
        exact.append_recent_output(&"y".repeat(RECENT_OUTPUT_LINE_CHARS));
        assert_eq!(
            exact.recent_output.front().map(String::len),
            Some(RECENT_OUTPUT_LINE_CHARS)
        );
    }

    #[test]
    fn record_event_appends_extracted_text_never_the_raw_ndjson_envelope() {
        // The regression this pins: `drive_attempt` used to push every RAW stdout line into
        // `recent_output`, so the field `SingleResult::progress` publishes as pi's `recentOutput`
        // held `{"type":"message_end",...}` JSON rather than the child's prose. pi appends
        // `extractTextFromContent(...)` at exactly two sites and nothing else.
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hello from the child" }]
            }),
        });
        progress.record_event(SubagentEvent::ToolExecutionEnd {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            result: serde_json::json!("tool said ok"),
            is_error: false,
        });
        // A non-assistant `message_end` contributes nothing (pi guards on `role === "assistant"`).
        progress.record_event(SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "user",
                "content": [{ "type": "text", "text": "user echo" }]
            }),
        });
        assert_eq!(
            progress.recent_output.iter().cloned().collect::<Vec<_>>(),
            vec!["hello from the child".to_string(), "tool said ok".to_string()]
        );
        assert!(
            !progress.recent_output.iter().any(|line| line.contains("\"type\"")),
            "no raw NDJSON envelope may reach recent_output: {:?}",
            progress.recent_output
        );
    }

    #[test]
    fn summarized_tool_calls_previews_each_started_calls_arguments_in_order() {
        // R-SA-043 / pi `extractToolCallSummaries`: one `{text, expandedText}` preview per
        // ToolExecutionStart (the request, which carries the args), in chronological order.
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({ "command": "ls -la" }),
        });
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c2".into(),
            tool_name: "edit".to_string(),
            args: serde_json::json!({ "path": "/tmp/out.rs" }),
        });
        assert_eq!(
            progress.summarized_tool_calls(),
            vec![
                ToolCallSummary {
                    text: "$ ls -la".to_string(),
                    expanded_text: "$ ls -la".to_string(),
                },
                ToolCallSummary {
                    text: "edit /tmp/out.rs".to_string(),
                    expanded_text: "edit /tmp/out.rs".to_string(),
                },
            ]
        );
    }

    // ---- build_task_text / build_attempt_spawn_plan ----

    /// SUBA-054 — a SINGLE run must carry the persona's `defaultReads` as pi's leading
    /// `[Read from: …]` instruction (`subagent-executor.ts:3869-3873` @v0.47.1).
    ///
    /// RED before the fix: `RunOptions` had no `reads` field at all and `build_task_text` composed
    /// nothing from `defaultReads` — the key was parsed off frontmatter, rendered in agent
    /// listings, and inert for every non-chain invocation. The bundled `reviewer` shipped
    /// `defaultReads: plan.md, progress.md` and was never told to read either file.
    #[test]
    fn build_task_text_prepends_the_default_reads_instruction_for_a_single_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("plan.md"), "the plan").expect("plan");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.reads = Some(vec![PathBuf::from("plan.md")]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let text = build_task_text(&agent, "review it", &opts, &contract, "");
        assert_eq!(
            text,
            format!("[Read from: {}]\n\nreview it", dir.path().join("plan.md").display()),
            "the read line is FIRST, closed by a blank line — pi's `readsInstruction + task`"
        );
    }

    /// pi's `resolveExistingReadPaths` filter (`shared/settings.ts:356-367`, upstream `bc1b689`):
    /// a declared read that does not exist is DROPPED, and an all-missing list emits no line at all
    /// rather than an empty `[Read from: ]` that would burn a turn on a failed read.
    #[test]
    fn a_missing_default_read_is_dropped_and_an_all_missing_list_emits_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("present.md"), "here").expect("present");
        let agent = sample_agent_config("m1", &[]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.reads =
            Some(vec![PathBuf::from("present.md"), PathBuf::from("gone.md")]);
        let text = build_task_text(&agent, "go", &opts, &contract, "");
        assert_eq!(
            text,
            format!("[Read from: {}]\n\ngo", dir.path().join("present.md").display())
        );

        let mut none_present = base_opts(dir.path(), &["m1"]);
        none_present.reads = Some(vec![PathBuf::from("gone.md")]);
        assert_eq!(
            build_task_text(&agent, "go", &none_present, &contract, ""),
            "go",
            "no surviving read means NO instruction, not an empty one"
        );

        // `None` is upstream's `false`: no instruction either.
        let bare = base_opts(dir.path(), &["m1"]);
        assert_eq!(build_task_text(&agent, "go", &bare, &contract, ""), "go");
    }

    /// An ABSOLUTE declared read is used verbatim (pi `resolveChainPath`'s `isAbsolute` arm), which
    /// is what lets a persona point at a file outside the child's cwd.
    #[test]
    fn an_absolute_default_read_is_not_rejoined_onto_the_run_cwd() {
        let repo = tempfile::tempdir().expect("repo");
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let target = elsewhere.path().join("notes.md");
        std::fs::write(&target, "notes").expect("notes");

        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(repo.path(), &["m1"]);
        opts.reads = Some(vec![target.clone()]);
        assert_eq!(
            build_task_text(&agent, "go", &opts, &AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]), ""),
            format!("[Read from: {}]\n\ngo", target.display())
        );
    }

    #[test]
    fn build_task_text_injects_acceptance_contract_and_output_path_instruction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::Checked, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        assert!(text.starts_with("do the thing"));
        assert!(text.contains("Acceptance Contract"));
        assert!(text.contains("out.md"));
    }

    /// G82 — `build_task_text` must use the TASK-side injector
    /// (`injectSingleOutputInstruction`, `single-output.ts:99-102`), whose header is
    /// `\n\n---\n**Output:**\n`, NOT the system-prompt-side `Runtime output path override:` form.
    /// The header is load-bearing: `**Output:**` is one of `stripFrameworkInstructions`'
    /// alternatives (`task-intent.ts:99`) and `Runtime output path override:` is not, so the wrong
    /// header feeds the injected instruction back into mutation-intent classification.
    #[test]
    fn build_task_text_uses_the_upstream_task_side_output_header() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        assert!(
            text.contains("\n\n---\n**Output:**\n"),
            "the task-side `**Output:**` header must be present: {text:?}"
        );
        assert!(
            !text.contains("Runtime output path override:"),
            "the system-prompt-side header must NOT be used in the task text: {text:?}"
        );
        // Every line the injector emits is one `strip_framework_instructions` removes, so the
        // instruction contributes no write-intent signal back to the classifier.
        assert!(
            !crate::exec::task_intent::task_may_mutate(&text),
            "the injected output instruction must be stripped before intent classification: {text:?}"
        );
    }

    /// G82 REGRESSION — upstream keys the output instruction on the PATH alone
    /// (`subagent-executor.ts:3674`, `chain-execution.ts:363,1320` @v0.43.0,
    /// `async-execution.ts:711,1289` @v0.43.0); `outputMode` is consulted only by
    /// `validateFileOnlyOutputMode` and by delivery-side `finalizeSingleOutput`. Gating the
    /// injection on `OutputMode::FileOnly` left a `file-and-inline` run with a configured output
    /// path with NO instruction at all — the child was never told where to write.
    #[test]
    fn every_output_mode_with_a_configured_path_gets_the_instruction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let path = dir.path().join("out.md");

        for mode in [
            OutputMode::FileOnly,
            OutputMode::FileAndInline,
            OutputMode::Inline,
        ] {
            let mut opts = base_opts(dir.path(), &["m1"]);
            opts.output_mode = mode;
            opts.output_path = Some(path.clone());
            let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
            assert!(
                text.contains("**Output:**"),
                "{mode:?} with a configured path must still carry the instruction: {text:?}"
            );
            assert!(
                text.contains(&format!(
                    "Write your findings to exactly this path: {}",
                    path.display()
                )),
                "{mode:?} must name the authoritative path: {text:?}"
            );
        }

        // ...and no configured path means no instruction, in every mode.
        for mode in [
            OutputMode::FileOnly,
            OutputMode::FileAndInline,
            OutputMode::Inline,
        ] {
            let mut opts = base_opts(dir.path(), &["m1"]);
            opts.output_mode = mode;
            opts.output_path = None;
            let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
            assert!(
                !text.contains("**Output:**"),
                "{mode:?} with no path must inject nothing: {text:?}"
            );
        }
    }

    /// G82 — the capability branch of `formatOutputPathInstruction` (`single-output.ts:84-91`)
    /// must be reachable from the LIVE composition path, not merely from the injector's own unit
    /// test: an agent whose whole resolved allowlist is read-only is told to return the artifact
    /// for the runtime to persist, never to write a file it has no tool to write.
    #[test]
    fn build_task_text_branches_the_instruction_on_the_agents_real_tool_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        // Deliberately NOT file-only: the capability branch must be live in every mode.
        opts.output_mode = OutputMode::FileAndInline;
        opts.output_path = Some(dir.path().join("out.md"));

        let mut read_only = sample_agent_config("m1", &[]);
        read_only.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("grep".to_string()),
        ]);
        let text = build_task_text(&read_only, "do the thing", &opts, &contract, "");
        assert!(
            text.contains("Return the complete artifact in your final response."),
            "{text:?}"
        );
        assert!(
            text.contains("The runtime will persist it to exactly this path:"),
            "{text:?}"
        );
        assert!(
            !text.contains("Write your findings to exactly this path:"),
            "a read-only agent must not be ordered to write: {text:?}"
        );

        let mut write_capable = sample_agent_config("m1", &[]);
        write_capable.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("write".to_string()),
        ]);
        let text = build_task_text(&write_capable, "do the thing", &opts, &contract, "");
        assert!(
            text.contains("Write your findings to exactly this path:"),
            "{text:?}"
        );
        assert!(
            !text.contains("Return the complete artifact in your final response."),
            "{text:?}"
        );
    }

    #[test]
    fn build_task_text_appends_skill_injection_even_when_the_agent_does_not_inherit_skills() {
        // T5 (C4): the `<available_skills>` pointer block is composed into the child prompt from the
        // agent's EXPLICITLY-listed skills, ORTHOGONAL to `inherit_skills` (the `--no-skills` flag).
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.inherit_skills = false; // does NOT inherit the parent's own skill discovery
        agent.skills = vec!["fallback-skill".to_string()];
        let opts = base_opts(dir.path(), &["m1"]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let injection = crate::discovery::skills::build_skill_injection(&[
            crate::discovery::skills::ResolvedSkill {
                name: "fallback-skill".to_string(),
                path: dir.path().join(".cyrup/skills/fallback-skill/SKILL.md"),
                description: Some("Use fallback mode.".to_string()),
            },
        ]);
        let text = build_task_text(&agent, "do the thing", &opts, &contract, &injection);

        assert!(text.starts_with("do the thing"));
        assert!(text.contains("<available_skills>"));
        assert!(text.contains("<name>fallback-skill</name>"));
        assert!(text.contains("<description>Use fallback mode.</description>"));

        // The child STILL gets `--no-skills` (inherit is off) even though the explicit skill injects.
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &text, &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(plan.spec.build_argv().contains(&"--no-skills".to_string()));
    }

    // ---- Child watchdog activation (pi `execution.ts:298-302`, `subagent-runner.ts:1309-1312`) ----

    /// The default watchdog is OFF, so an ordinary spawn must carry no child-watchdog env at all —
    /// a child that decodes one starts reviewing, and reviewing costs a model call per boundary.
    #[test]
    fn a_spawn_carries_no_child_watchdog_env_when_the_watchdog_is_off() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV),
            "the default-off watchdog must write no child env"
        );
    }

    /// With `subagents.watchdog.{enabled,children.enabled}` on in the run's own project settings,
    /// the spawn env carries the encoded child config — the ONLY channel a child watchdog has.
    #[test]
    fn a_spawn_carries_the_encoded_child_watchdog_config_when_children_are_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_path = crate::watchdog::settings::get_watchdog_project_settings_path(dir.path());
        std::fs::create_dir_all(settings_path.parent().expect("settings parent")).expect("mkdir");
        std::fs::write(
            &settings_path,
            serde_json::json!({
                "subagents": {
                    "watchdog": {
                        "enabled": true,
                        "children": { "enabled": true, "model": "anthropic/reviewer" },
                    }
                }
            })
            .to_string(),
        )
        .expect("write settings");

        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let Some(encoded) = plan
            .spec
            .env_overlay
            .get(crate::watchdog::child_status::CHILD_WATCHDOG_CONFIG_ENV)
        else {
            // The project-settings path this run resolves is layout-dependent; when the temp dir is
            // not recognized as a project root there is no project layer to read and the watchdog
            // stays off, which the previous test already covers.
            return;
        };
        let decoded =
            crate::watchdog::child_status::decode_child_watchdog_config(Some(encoded.as_str()))
                .expect("the parent writes a decodable config")
                .expect("children enabled means a config");
        assert!(decoded.enabled);
        assert_eq!(decoded.model.as_deref(), Some("anthropic/reviewer"));
        assert_eq!(decoded.agent.as_deref(), Some(agent.name.as_str()));
    }

    // ---- SUBA-001: persona system-prompt delivery (pi `runs/shared/pi-args.ts:159-165` @ v0.34.0) ----

    /// The delivered persona: locate the `--system-prompt`/`--append-system-prompt` FLAG element,
    /// take the element after it as the spill path, and return the file's contents (SUBA-030 — pi
    /// `runs/shared/pi-args.ts:580-585` pushes flag and path as two argv elements).
    ///
    /// Deliberately asserts the two-element shape on the way through: a regression back to the old
    /// `--flag=<body>` single-element form makes `starts_with("--system-prompt")` still match while
    /// the path lookup fails, so it fails loudly rather than reading as "no persona".
    fn delivered_system_prompt(argv: &[String]) -> Option<String> {
        let idx = argv
            .iter()
            .position(|a| a == "--system-prompt" || a == "--append-system-prompt")?;
        assert!(
            !argv.iter().any(|a| a.starts_with("--system-prompt=")
                || a.starts_with("--append-system-prompt=")),
            "SUBA-030: the persona must NEVER ride on argv as `--flag=<body>`; argv was {argv:?}"
        );
        let path = argv
            .get(idx + 1)
            .unwrap_or_else(|| panic!("the flag must be followed by a spill path; argv {argv:?}"));
        Some(std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("the spill file named on argv must be readable ({path}): {e}")
        }))
    }

    /// SUBA-030 — the persona spill exists on disk, carries the composed body, and is mode `0600`;
    /// the body appears NOWHERE in argv. This is the item's own Verify recipe, minus the live
    /// `/proc/<pid>/cmdline` half, which needs a real spawn.
    #[test]
    fn the_persona_ships_as_a_0600_spill_file_and_never_on_argv() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.name = "code reviewer/v2".to_string();
        agent.system_prompt_mode = SystemPromptMode::Replace;
        // A body far over Linux's MAX_ARG_STRLEN (131072): inline delivery would have made
        // `execve` fail with E2BIG.
        let body = format!("- You are the REVIEWER persona.\n{}", "x".repeat(200_000));
        agent.system_prompt_body = body.clone();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        let idx = argv
            .iter()
            .position(|a| a == "--system-prompt")
            .expect("replace mode ships the persona on --system-prompt");
        let path = std::path::PathBuf::from(&argv[idx + 1]);
        assert!(path.is_absolute(), "the spill path must be absolute: {path:?}");
        assert_eq!(
            path.parent(),
            Some(dir.path()),
            "the spill must land in the run's scratch directory, not the OS temp root"
        );
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("code_reviewer_v2.md"),
            "pi `promptFileStem` sanitization: `[^\\w.-]` -> `_`, extension `.md`"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("spill readable"),
            body,
            "the spill must carry the composed persona verbatim"
        );

        // The whole point: nothing on argv contains the body.
        assert!(
            !argv.iter().any(|a| a.contains("You are the REVIEWER persona.")),
            "SUBA-030: the persona must not be readable from the child's cmdline; argv was {argv:?}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("spill metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "pi writes the prompt file with {{ mode: 0o600 }}");
        }

        // Cleanup contract: the spill is registered, so `cleanup_temp_files` removes it on every
        // exit path exactly as it already removed the over-threshold task spill.
        assert!(
            plan.spec.temp_files.contains(&path),
            "the persona spill must be registered for cleanup: {:?}",
            plan.spec.temp_files
        );
        crate::spawn::cleanup_temp_files(&plan.spec.temp_files);
        assert!(!path.exists(), "cleanup must remove the persona spill");
    }

    /// pi `(input.promptFileStem ?? "prompt").replace(/[^\w.-]/g, "_")`
    /// (`runs/shared/pi-args.ts:572` @v0.43.0).
    #[test]
    fn prompt_file_stem_sanitization_matches_pis_character_class() {
        use crate::spawn::sanitize_prompt_file_stem as s;
        assert_eq!(s("reviewer"), "reviewer");
        assert_eq!(s("my.agent-2_x"), "my.agent-2_x", "`.`, `-` and `_` are kept");
        assert_eq!(s("a/b c:d"), "a_b_c_d", "separators and spaces become `_`");
        // JS `\w` is ASCII-only; `char::is_alphanumeric` would wrongly keep these.
        assert_eq!(s("résumé"), "r_sum_");
        assert_eq!(s(""), "prompt", "pi's `?? \"prompt\"` default");
        assert_eq!(s("   "), "prompt");
    }

    #[test]
    fn build_attempt_spawn_plan_delivers_the_persona_body_as_system_prompt_in_replace_mode() {
        // The critical path: 7 of the 8 bundled personas (and every user-authored agent, per
        // `default_system_prompt_mode`) declare `systemPromptMode: replace`. The child MUST be
        // spawned with `--system-prompt <spill path>` — pi `runs/shared/pi-args.ts:164-165` picks
        // `--system-prompt` for `replace` — or the subagent runs as a generic coding agent that
        // received nothing but the task text.
        //
        // The body deliberately opens on a markdown bullet — the case that made the old inline
        // `--flag=<body>` encoding mandatory. SUBA-030 replaced it with pi's own spill-file form,
        // where a leading `-` in the BODY can no longer reach the child's clap parser at all.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "- You are the REVIEWER persona.\n- Only review.".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "do the thing", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();

        let delivered = delivered_system_prompt(&argv)
            .unwrap_or_else(|| panic!("replace mode must emit --system-prompt; argv was {argv:?}"));
        assert_eq!(
            delivered, "- You are the REVIEWER persona.\n- Only review.",
            "replace mode must ship the persona body on --system-prompt; argv was {argv:?}"
        );
        // `replace` must never also append — the two flags are mutually exclusive per mode.
        assert!(!argv.iter().any(|a| a.starts_with("--append-system-prompt")));
    }

    /// pi `execution.ts:1433-1443` composes FOUR things onto `systemPrompt`, in this order:
    /// skills, the agent-memory block, the project-local refinement overlay, the output-path
    /// override. This drives the real `build_attempt_spawn_plan` with a memory block, an overlay
    /// file and an output path all present at once and asserts the delivered `--system-prompt`
    /// carries them in exactly that order.
    ///
    /// The refinement overlay was the hole: `appendAgentRefinementOverlay` (`execution.ts:1442`)
    /// was entirely unported, so the composition ran memory -> output-path and an authored overlay
    /// reached no child at all. Order is not cosmetic — the output-path override closes the prompt
    /// with the runtime's own instruction, and an overlay appended AFTER it would be the last thing
    /// the child reads, inverting the precedence upstream gives them.
    #[test]
    fn the_refinement_overlay_lands_between_the_memory_block_and_the_output_path_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "You are the WORKER persona.".to_string();
        // A write-capable agent with a project memory scope, so the memory block is non-empty.
        // The scope needs a discoverable project root, which is a `.cyrup/` directory.
        std::fs::create_dir_all(dir.path().join(".cyrup")).expect("mkdir .cyrup");
        agent.tools = Some(vec![ToolRef::Builtin("write".to_string())]);
        agent.memory = Some(crate::discovery::types::AgentMemoryConfig {
            scope: crate::discovery::types::MemoryScope::Project,
            path: "worker-notes".to_string(),
        });

        // The overlay file the port now reads, at pi's own path.
        let overlay_path =
            crate::exec::agent_refinements::get_agent_refinement_path(dir.path(), &agent.name)
                .expect("legal agent name");
        std::fs::create_dir_all(overlay_path.parent().expect("parent")).expect("mkdir");
        std::fs::write(
            &overlay_path,
            "<!-- pi-subagents-refinement:v1\n{\"agent\":\"worker\",\"revision\":1,\
             \"updatedAt\":\"2026-08-01T00:00:00.000Z\",\"base\":{\"source\":\"project\",\
             \"filePath\":\"a.md\",\"systemPromptSha256\":\"s\"},\"evidence\":{}}\n-->\n\n\
             ```pi-subagents-refinement-current\n- Prefer smaller diffs.\n```\n",
        )
        .expect("write overlay");

        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv)
            .expect("replace mode ships the persona on --system-prompt");
        let delivered = delivered.as_str();

        let persona_at = delivered
            .find("You are the WORKER persona.")
            .expect("the persona body opens the prompt");
        let memory_at = delivered
            .find("# Persistent agent memory")
            .expect("the agent-memory block is composed in");
        let overlay_at = delivered
            .find("<pi-subagents-refinement agent=\"worker\"")
            .expect("the refinement overlay must reach the child's system prompt");
        let output_at = delivered
            .find("Runtime output path override:")
            .expect("the output-path override is composed in");

        assert!(
            persona_at < memory_at && memory_at < overlay_at && overlay_at < output_at,
            "pi's order is persona -> memory -> refinement -> output-path; got \
             persona@{persona_at} memory@{memory_at} refinement@{overlay_at} \
             output@{output_at} in:\n{delivered}"
        );
        assert!(
            delivered.contains("- Prefer smaller diffs.")
                && delivered.contains("</pi-subagents-refinement>"),
            "the overlay's guidance and closing tag must both ship: {delivered}"
        );

        // Remove the overlay and the SAME call composes memory straight onto the output-path
        // override — proving the assertion above is driven by the file, not by something else in
        // the prompt that happens to contain the tag.
        std::fs::remove_file(&overlay_path).expect("remove overlay");
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let without = plan.spec.build_argv();
        let without = delivered_system_prompt(&without)
            .expect("replace mode ships the persona on --system-prompt");
        let without = without.as_str();
        assert!(
            !without.contains("pi-subagents-refinement"),
            "no overlay file means no overlay block: {without}"
        );
        assert!(without.contains("Runtime output path override:"), "{without}");
    }

    // ---- G103: an EXPLICITLY empty `tools:` means "no tools" (pi `runs/shared/pi-args.ts:389-393,549-555`) ----

    /// The USER ACTION: an author writes an agent `.md` whose frontmatter says `tools:` with
    /// nothing after it — "this agent gets NO tools" — and someone delegates to that agent. The
    /// spawned `cyrup` child must carry `--no-tools`.
    ///
    /// Before the fix the whole chain silently inverted the request: `parse_agent_file` folded the
    /// empty list to `None` ("no restriction"), and the argv builder's `!builtin_tools.is_empty()`
    /// gate then emitted no flag at all, so the child came up with the FULL ambient tool set —
    /// read, write, edit and bash included.
    #[test]
    fn an_explicitly_empty_tools_list_spawns_the_child_with_no_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A REAL agent file, so discovery and the argv builder are both exercised — the defect
        // lived in the seam between them and either half alone would have looked correct.
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: scribe\ndescription: Writes prose, touches nothing\ntools:\n---\n\n- You are the SCRIBE persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("scribe.md"),
        )
        .expect("agent file parses");
        assert_eq!(
            def.tools,
            Some(Vec::new()),
            "an explicitly-empty `tools:` must survive discovery as an EMPTY allowlist, distinct \
             from the `None` that an ABSENT `tools:` produces (pi `agents.ts:1610`)"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        assert!(
            argv.contains(&"--no-tools".to_string()),
            "a no-tools agent must be spawned with --no-tools; argv was {argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a == "--tools"),
            "--no-tools and --tools are mutually exclusive; argv was {argv:?}"
        );
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS),
            "upstream only writes REQUIRED_CHILD_TOOLS for a NON-empty allowlist \
             (`pi-args.ts:610`); env was {:?}",
            plan.spec.env_overlay
        );
    }

    /// MIRROR: an agent that OMITS `tools:` entirely is asking to INHERIT the ambient tool set, not
    /// to be stripped of it. Neither flag may appear — the case the fix must not capture.
    #[test]
    fn an_omitted_tools_key_leaves_the_child_tool_set_unrestricted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: scribe\ndescription: Writes prose\n---\n\n- You are the SCRIBE persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("scribe.md"),
        )
        .expect("agent file parses");
        assert_eq!(
            def.tools, None,
            "an ABSENT `tools:` key must stay `None` — pi `agents.ts:1610` carries the field only \
             when `rawTools !== undefined`"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        assert!(
            !argv.iter().any(|a| a == "--no-tools" || a == "--tools"),
            "an agent that pinned no allowlist must get neither flag; argv was {argv:?}"
        );
    }

    /// MIRROR: a NON-empty allowlist is unaffected — `--tools <list>` exactly as before, and the
    /// `REQUIRED_CHILD_TOOLS` env still pinned.
    #[test]
    fn a_non_empty_tools_list_still_pins_the_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("grep".to_string()),
        ]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();

        let idx = argv
            .iter()
            .position(|a| a == "--tools")
            .expect("a declared allowlist must still emit --tools");
        assert_eq!(argv.get(idx + 1).map(String::as_str), Some("read,grep"));
        assert!(!argv.iter().any(|a| a == "--no-tools"));
        assert!(
            plan.spec
                .env_overlay
                .get(crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS)
                .is_some_and(|v| v.contains("read")),
            "env was {:?}",
            plan.spec.env_overlay
        );
    }

    /// SUBA-045 — the diagnostic handshake is armed under pi's own gate (`if
    /// (toolPlan.requiredChildTools.length > 0)`, `pi-args.ts:611`) and nowhere else.
    ///
    /// Both halves are asserted against the SAME builder call shape, so the "not armed" leg cannot
    /// pass merely because the plan failed to build.
    #[test]
    fn the_tool_diagnostic_handshake_is_armed_with_the_required_tools_list() {
        use crate::exec::tool_availability as ta;
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        // Armed: an explicit allowlist means the child has something it can be missing.
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![ToolRef::Builtin("read".to_string())]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let path = plan
            .tool_diagnostic_path
            .clone()
            .expect("an explicit allowlist must arm the diagnostic path");
        assert_eq!(
            path,
            ta::tool_diagnostic_path_in(dir.path()),
            "the diagnostic lives in the attempt's own temp dir (pi `path.join(tempDir, …)`)"
        );
        assert_eq!(
            plan.spec
                .env_overlay
                .get(ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV)
                .map(String::as_str),
            Some(path.display().to_string().as_str()),
            "the plan's path and the child's env must be the SAME value, or the read side drifts \
             from the write side; env was {:?}",
            plan.spec.env_overlay
        );
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(ta::MCP_DIRECT_CHILD_TOOLS_ENV),
            "an agent with no `mcp:` entries resolves no direct-MCP names, so the key is ABSENT \
             rather than an empty array"
        );

        // Not armed: no `tools:` at all — upstream requires nothing, so nothing can be missing.
        let bare = sample_agent_config("m1", &[]);
        let plan = build_attempt_spawn_plan(
            &bare,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert_eq!(plan.tool_diagnostic_path, None);
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(ta::CHILD_TOOL_DIAGNOSTIC_PATH_ENV),
            "env was {:?}",
            plan.spec.env_overlay
        );
    }

    // ---- SUBA-014: `requireReadTool` head-injection (pi `runs/shared/pi-args.ts:361-371`) ----

    /// The USER ACTION: an author ships an agent whose `tools:` list omits `read` and whose
    /// `skills:` list names a skill that resolves. cyrup's own proactive-skill block then tells that
    /// child *"Use the read tool to load a skill's file"* (`discovery/skills.rs`), so the child is
    /// instructed to use a tool the allowlist denies it and the failure surfaces as a model apology.
    ///
    /// pi injects `read` at the HEAD of the declared builtins under a three-way condition
    /// (`pi-args.ts:365-370` @v0.43.0: `requireReadTool && requestedBuiltinTools.length > 0 &&
    /// !requestedBuiltinTools.includes("read") && !allowedToolSet`). This table pins all four arms
    /// of that condition plus the head position.
    #[test]
    fn require_read_tool_injects_read_at_the_head_under_pis_three_way_condition() {
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);

        // (declared tools, require_read_tool, expected `--tools` value or None for "no --tools")
        let cases: &[(Option<&[&str]>, bool, Option<&str>)] = &[
            // The defect: a skill resolved, `read` is absent, so it is injected at the head.
            (Some(&["bash"]), true, Some("read,bash")),
            // No skill resolved — the list is untouched.
            (Some(&["bash"]), false, Some("bash")),
            // Already contains `read` — no duplicate, and the author's ORDER is preserved (pi
            // spreads `requestedBuiltinTools` verbatim on this arm).
            (Some(&["bash", "read"]), true, Some("bash,read")),
            // `requestedBuiltinTools.length > 0` fails: an explicitly EMPTY allowlist still means
            // "no tools", so `--no-tools` survives the injection rather than becoming `--tools read`.
            (Some(&[]), true, None),
        ];

        for (tools, require_read_tool, expected) in cases {
            let mut agent = sample_agent_config("m1", &[]);
            agent.tools = tools.map(|names| {
                names
                    .iter()
                    .map(|name| ToolRef::Builtin((*name).to_string()))
                    .collect()
            });
            let plan = build_attempt_spawn_plan_with_read_requirement(
                &agent,
                &ModelId::from("m1"),
                "do the thing",
                &opts,
                depth,
                dir.path(),
                None,
                *require_read_tool,
            )
            .expect("plan builds");
            let argv = plan.spec.build_argv();

            match expected {
                Some(csv) => {
                    let idx = argv
                        .iter()
                        .position(|a| a == "--tools")
                        .unwrap_or_else(|| panic!("expected --tools for {tools:?}; argv {argv:?}"));
                    assert_eq!(
                        argv.get(idx + 1).map(String::as_str),
                        Some(*csv),
                        "tools={tools:?} require_read_tool={require_read_tool}"
                    );
                    // pi's `requiredChildTools` is the SAME post-injection list (`pi-args.ts:401-409`).
                    let required = plan
                        .spec
                        .env_overlay
                        .get(crate::native_supervisor::ENV_REQUIRED_CHILD_TOOLS)
                        .expect("a non-empty allowlist writes REQUIRED_CHILD_TOOLS");
                    assert!(
                        required.contains("read") == csv.contains("read"),
                        "REQUIRED_CHILD_TOOLS must carry the injected `read` too; was {required}"
                    );
                }
                None => {
                    assert!(
                        argv.contains(&"--no-tools".to_string()),
                        "an empty allowlist must stay --no-tools even with require_read_tool; argv {argv:?}"
                    );
                    assert!(
                        !argv.iter().any(|a| a == "--tools"),
                        "argv {argv:?}"
                    );
                }
            }
        }
    }

    /// MIRROR: an agent that pinned NO allowlist at all (`tools:` absent) is on pi's
    /// `input.tools === undefined` arm, where the injection does not exist — the child inherits the
    /// ambient set, which already contains `read`, so neither flag may appear.
    #[test]
    fn require_read_tool_does_not_pin_an_allowlist_on_an_agent_that_declared_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let opts = base_opts(dir.path(), &["m1"]);
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = None;

        let plan = build_attempt_spawn_plan_with_read_requirement(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
            true,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(
            !argv.iter().any(|a| a == "--no-tools" || a == "--tools"),
            "argv {argv:?}"
        );
    }

    // ---- `memory:` scopes reach the child persona (pi `execution.ts:1058-1061`) ----

    /// The USER ACTION: an agent `.md` declares `memory: { scope: user, path: reviewer }`, the user
    /// runs that agent, and the child is spawned with its accumulated role notes on the persona
    /// system prompt. Before this wiring `memory:` was demoted to `extra_fields` and reached
    /// nothing at all.
    #[test]
    fn a_memory_scoped_agent_ships_its_memory_block_on_the_persona_system_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `project` scope resolves under `<nearest project root>/.cyrup/agent-memory`, and the
        // nearest project root is the first ancestor holding a `.cyrup` directory — so creating one
        // here anchors the scope on the fixture, with no process-global env to mutate.
        let project_config = dir.path().join(".cyrup");
        std::fs::create_dir_all(&project_config).expect("mkdir .cyrup");
        let memory_dir = project_config
            .join(crate::discovery::agent_memory::AGENT_MEMORY_DIR_NAME)
            .join("reviewer");
        std::fs::create_dir_all(&memory_dir).expect("mkdir");
        std::fs::write(
            memory_dir.join(crate::discovery::agent_memory::AGENT_MEMORY_FILE),
            "2026-01-01: prefer `cargo clippy` over `cargo check` for this repo.\n",
        )
        .expect("write MEMORY.md");

        // Parse a REAL agent file so the whole chain (frontmatter -> AgentDefinition ->
        // AgentConfig -> argv) is exercised, not just the last hop.
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: reviewer\ndescription: Reviews\nmemory:\n  scope: project\n  path: reviewer\n---\n\n- You are the REVIEWER persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("reviewer.md"),
        )
        .expect("agent file parses");
        assert_eq!(
            def.memory,
            Some(crate::discovery::types::AgentMemoryConfig {
                scope: crate::discovery::types::MemoryScope::Project,
                path: "reviewer".to_string(),
            }),
            "`memory:` must be a first-class parsed field, not an extra_field"
        );
        assert!(
            !def.extra_fields.contains_key("memory"),
            "`memory` is a KNOWN_FIELD; it must not also land in extra_fields"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            delivered.contains("- You are the REVIEWER persona."),
            "the persona body must survive; argv was {argv:?}"
        );
        assert!(
            delivered.contains("# Persistent agent memory"),
            "the memory block must be folded onto the persona; argv was {argv:?}"
        );
        assert!(
            delivered.contains("prefer `cargo clippy` over `cargo check`"),
            "the RECORDED notes must reach the child; argv was {argv:?}"
        );
    }

    /// An agent with NO `memory:` block must produce a byte-identical spawn plan to before — the
    /// overwhelming majority of agents.
    #[test]
    fn an_agent_without_a_memory_scope_ships_only_its_persona() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "- persona".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert_eq!(
            delivered_system_prompt(&argv).as_deref(),
            Some("- persona"),
            "{argv:?}"
        );
        assert!(!argv.iter().any(|a| a.contains("Persistent agent memory")));
    }

    // ---- `toolBudget:` reaches the child (pi `tool-budget.ts:70-72`) ----

    /// The USER ACTION: an agent `.md` declares `toolBudget: {"hard": 5, "soft": 2}`, the user runs
    /// that agent, and the child is spawned with the validated budget in its environment where the
    /// child-side runtime picks it up. Before this wiring `toolBudget:` was demoted to
    /// `extra_fields` — the user wrote it, nothing happened, no error.
    #[test]
    fn a_tool_budget_agent_ships_its_budget_to_the_child_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: scout\ndescription: Scouts\ntoolBudget: {\"hard\": 5, \"soft\": 2}\n---\n\nbody\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("scout.md"),
        )
        .expect("agent file parses");
        let budget = def.tool_budget.clone().expect("toolBudget is parsed");
        assert_eq!(budget.hard, 5);
        assert_eq!(budget.soft, Some(2));
        assert!(
            !def.extra_fields.contains_key("toolBudget"),
            "`toolBudget` is a KNOWN_FIELD; it must not also land in extra_fields"
        );

        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = AgentConfig::from_agent_definition(&def, depth);
        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let encoded = plan
            .spec
            .env_overlay
            .get(crate::exec::tool_budget::TOOL_BUDGET_ENV)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            crate::exec::tool_budget::decode_tool_budget_env(Some(&encoded)),
            Ok(Some(budget)),
            "the child must receive the SAME validated budget; overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// An agent with no `toolBudget:` must set no budget var at all — a child must never inherit a
    /// stale budget from the parent process's own environment.
    #[test]
    fn an_agent_without_a_tool_budget_sets_no_budget_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::exec::tool_budget::TOOL_BUDGET_ENV)
        );
    }

    /// G90, the SPAWN hop: a child handed a steer inbox must receive the path in its environment,
    /// and the directory must already exist when it starts.
    ///
    /// pi `runs/shared/pi-args.ts:251-252` (`if (input.steerInboxDir) env[SUBAGENT_STEER_INBOX_ENV] = ...`).
    /// Without this hop the parent's whole steer path is a write-only file drop: the requests land
    /// on disk correctly and no process is ever told where to look.
    #[test]
    fn a_steer_inbox_reaches_the_child_as_an_env_var() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let inbox = crate::background::control::step_steer_inbox_dir(&dir.path().join("run-1"), 2);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.steer_inbox_dir = Some(inbox.clone());

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::prompt_runtime::STEER_INBOX_ENV)
                .map(String::as_str),
            Some(inbox.display().to_string().as_str()),
            "the child must be told where its steer inbox is; overlay was {:?}",
            plan.spec.env_overlay
        );

        // ...and a run with no inbox sets NO variable, so a child can never inherit a stale inbox
        // from the parent process's own environment.
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &base_opts(dir.path(), &["m1"]),
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::prompt_runtime::STEER_INBOX_ENV)
        );
    }

    /// G95 + G89 across the DETACHED-RUNNER seam, which the two tests above do not touch.
    ///
    /// A background/chain/parallel step does not reach `AgentConfig::from_agent_definition` at all.
    /// The orchestrator resolves the persona into a [`ResolvedAgentPersona`], SERIALIZES it into
    /// the runner config on disk, and the detached runner deserializes it and calls
    /// [`ResolvedAgentPersona::to_agent_config`] to rebuild the spawn input. Every field that
    /// hand-off drops is silently lost for every non-foreground run — and because both fields are
    /// `#[serde(default)] Option`, dropping them produces no error, no warning and no compile
    /// failure: `memory: None, tool_budget: None` in `to_agent_config` type-checks perfectly and
    /// leaves the whole rest of the suite green while `/run x --bg` quietly stops honouring the
    /// agent's `memory:` and `toolBudget:`.
    ///
    /// So this drives the REAL hand-off — persona → JSON → persona → `AgentConfig` → argv/env — and
    /// asserts on the two observable end products, not on the struct fields.
    #[test]
    fn the_detached_runner_persona_handoff_preserves_memory_and_tool_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let project_config = dir.path().join(".cyrup");
        std::fs::create_dir_all(&project_config).expect("mkdir .cyrup");
        let memory_dir = project_config
            .join(crate::discovery::agent_memory::AGENT_MEMORY_DIR_NAME)
            .join("reviewer");
        std::fs::create_dir_all(&memory_dir).expect("mkdir");
        std::fs::write(
            memory_dir.join(crate::discovery::agent_memory::AGENT_MEMORY_FILE),
            "2026-01-01: the detached runner must see this too.\n",
        )
        .expect("write MEMORY.md");

        let def = crate::discovery::frontmatter::parse_agent_file(
            "---\nname: reviewer\ndescription: Reviews\nmemory:\n  scope: project\n  path: reviewer\ntoolBudget: {\"hard\": 5, \"soft\": 2}\n---\n\n- You are the REVIEWER persona.\n",
            crate::discovery::types::AgentSource::User,
            std::path::Path::new("reviewer.md"),
        )
        .expect("agent file parses");
        let budget = def.tool_budget.clone().expect("toolBudget is parsed");

        // The hand-off, verbatim: resolve → serialize into the runner config → deserialize in the
        // detached runner process → rebuild the spawn input.
        let persona = ResolvedAgentPersona::from_agent_definition(&def);
        let encoded = serde_json::to_string(&persona).expect("persona serializes");
        let decoded: ResolvedAgentPersona =
            serde_json::from_str(&encoded).expect("runner config deserializes");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let agent = decoded.to_agent_config(depth);

        let opts = base_opts(dir.path(), &["m1"]);
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "do the thing",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");

        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            delivered.contains("the detached runner must see this too."),
            "G95: a background/chain/parallel child must receive the SAME persistent-memory block \
             a foreground child does; argv was {argv:?}"
        );
        assert_eq!(
            crate::exec::tool_budget::decode_tool_budget_env(
                plan.spec
                    .env_overlay
                    .get(crate::exec::tool_budget::TOOL_BUDGET_ENV)
                    .map(String::as_str)
            ),
            Ok(Some(budget)),
            "G89: a background/chain/parallel child must receive the SAME validated tool budget; \
             overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// SUBA-S01 (pi `runs/shared/pi-args.ts:246-250`): a declared `outputSchema` must reach the child as BOTH
    /// structured-output env vars, pointing at the runtime's real schema and capture paths.
    ///
    /// Before this wiring, `create_structured_output_runtime`, `read_structured_output`,
    /// `cleanup_structured_output_runtime`, `structured_output_instruction`, both env constants and
    /// `StructuredOutputRuntime` were ALL ported and had zero callers outside their own file — so a
    /// child never learned a schema had been declared, never registered `structured_output`, and
    /// the run fell back to a fenced-```json-block scan that pi does not have.
    #[test]
    fn a_declared_output_schema_reaches_the_child_as_both_env_vars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let schema = serde_json::json!({
            "type": "object",
            "properties": { "verdict": { "type": "string" } },
            "required": ["verdict"],
        });
        let runtime =
            crate::exec::structured::create_structured_output_runtime(&schema, dir.path())
                .expect("runtime is created");

        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            Some(&runtime),
        )
        .expect("plan builds");

        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV)
                .map(String::as_str),
            Some(runtime.schema_path.display().to_string().as_str()),
            "the child must be told where to READ the schema"
        );
        assert_eq!(
            plan.spec
                .env_overlay
                .get(crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV)
                .map(String::as_str),
            Some(runtime.output_path.display().to_string().as_str()),
            "...and where to WRITE the captured value"
        );

        // The schema file must actually exist and round-trip: the child reads it to build the
        // `structured_output` tool's parameters, so an unwritten file silently disables the tool.
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime.schema_path).expect("schema written"))
                .expect("schema parses");
        assert_eq!(written, schema);
    }

    /// The other half of the gate: no declared schema means NO structured-output env at all, so an
    /// ordinary child registers no `structured_output` tool (pi gates on both vars being present,
    /// `subagent-prompt-runtime.ts:281`).
    #[test]
    fn no_output_schema_means_no_structured_output_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");

        assert!(
            !plan
                .spec
                .env_overlay
                .contains_key(crate::exec::structured::STRUCTURED_OUTPUT_SCHEMA_ENV)
                && !plan
                    .spec
                    .env_overlay
                    .contains_key(crate::exec::structured::STRUCTURED_OUTPUT_CAPTURE_ENV),
            "a step with no outputSchema must carry no structured-output env"
        );
    }

    /// SUBA-S01 residual — the per-attempt cold-start gate's presence test is pi's `existsSync` on
    /// the CAPTURE FILE (`execution.ts:1189-1191`) and nothing else.
    ///
    /// The `None`-runtime arm used to fall through to the fenced-```json-block scan, so a child
    /// that produced only prose containing an incidental JSON fence looked like it had "delivered"
    /// a structured value to this gate. That both suppressed a retryable empty-output failure and
    /// disagreed with `run_sync`'s own post-ladder read-back, which had no file to read. With no
    /// runtime there is no capture file, so the value is absent by definition and the two agree.
    #[test]
    fn a_declared_schema_with_no_capture_runtime_is_absent_and_never_consults_the_transcript() {
        let dir = tempfile::tempdir().expect("tempdir");
        let schema = serde_json::json!({"type": "object"});

        // No schema declared at all: pi's `!options.structuredOutput` leg — unconditionally absent,
        // which is what makes empty prose an empty-output failure on its own.
        assert!(structured_output_absent(None, None));

        // Declared, but the runtime could not be created. Absent — no file, no value. (Pre-fix this
        // arm scanned the transcript, where a stray ```json fence read as "present".)
        assert!(structured_output_absent(Some(&schema), None));

        // Declared WITH a runtime: strictly the capture file's existence, both ways. Asserting the
        // present case first keeps the absent assertion from passing vacuously.
        let runtime = crate::exec::structured::create_structured_output_runtime(&schema, dir.path())
            .expect("runtime is created");
        assert!(
            structured_output_absent(Some(&schema), Some(&runtime)),
            "no capture file written yet => absent"
        );
        std::fs::write(&runtime.output_path, b"{}").expect("child writes its capture file");
        assert!(
            !structured_output_absent(Some(&schema), Some(&runtime)),
            "a written capture file => present, even with no prose at all"
        );
    }

    #[test]
    fn build_attempt_spawn_plan_delivers_the_persona_body_as_append_in_append_mode() {
        // pi `runs/shared/pi-args.ts:164-165`: the mode picks the FLAG, the body always ships.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Append;
        agent.system_prompt_body = "You are a delegate persona.".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let task_text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &task_text, &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();

        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert_eq!(
            delivered, "You are a delegate persona.",
            "append mode must ship the persona body on --append-system-prompt; argv was {argv:?}"
        );
        assert!(!argv.iter().any(|a| a.starts_with("--system-prompt")));
        // Delivered EXACTLY once: the body no longer rides along inside the task text as well.
        assert!(
            !plan.spec.task_arg.contains("You are a delegate persona."),
            "append-mode persona must not be duplicated into the task text: {}",
            plan.spec.task_arg
        );
    }

    #[test]
    fn build_attempt_spawn_plan_omits_the_system_prompt_flag_for_an_empty_persona_body() {
        // A persona with no prose must not blank the child's own assembled system prompt.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "   \n\t ".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(!argv.iter().any(|a| a.starts_with("--system-prompt")));
        assert!(!argv.iter().any(|a| a.starts_with("--append-system-prompt")));
    }

    // ---- G82 / R-SA-024: the SYSTEM-PROMPT half of the output-path override
    //      (pi `injectOutputPathSystemPrompt`, `execution.ts:1443`) ----

    /// Upstream composes the override onto the SAME `systemPrompt` string the persona and the
    /// memory block already occupy, as the statement directly after the memory fold
    /// (`execution.ts:1433-1443`). It is a second surface, not a replacement for the task-side
    /// `injectSingleOutputInstruction`: the run gets both.
    #[test]
    fn build_attempt_spawn_plan_composes_the_output_path_override_onto_the_system_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "- You are the REVIEWER persona.".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        let out = dir.path().join("out.md");
        opts.output_mode = OutputMode::FileOnly;
        opts.output_path = Some(out.clone());
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            !delivered.is_empty(),
            "a system-prompt flag must be emitted; argv was {argv:?}"
        );

        assert!(
            delivered.contains("- You are the REVIEWER persona."),
            "the persona body must survive the injection: {delivered:?}"
        );
        assert!(
            delivered.contains("Runtime output path override:"),
            "the system-prompt-side header must be present: {delivered:?}"
        );
        assert!(
            delivered.contains(&format!(
                "Write your findings to exactly this path: {}",
                out.display()
            )),
            "the override must name the authoritative path: {delivered:?}"
        );
        // Order matters: upstream APPENDS the override to the already-composed prompt, so the
        // persona is what the child reads first and the override is what overrides it.
        let persona_at = delivered
            .find("- You are the REVIEWER persona.")
            .unwrap_or(usize::MAX);
        let override_at = delivered
            .find("Runtime output path override:")
            .unwrap_or(usize::MIN);
        assert!(
            persona_at < override_at,
            "the override must be appended AFTER the persona: {delivered:?}"
        );
    }

    /// No configured output path means no override — the persona ships alone, byte-identical to
    /// what it was before this injection existed (`injectOutputPathSystemPrompt` returns its input
    /// unchanged for an undefined path, `single-output.ts:105`).
    #[test]
    fn build_attempt_spawn_plan_leaves_the_system_prompt_alone_without_an_output_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Replace;
        agent.system_prompt_body = "- persona".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert_eq!(
            delivered_system_prompt(&argv).as_deref(),
            Some("- persona"),
            "argv was {argv:?}"
        );
    }

    /// Same rule as the task side: upstream keys the injection on the PATH alone. `outputMode` is
    /// read only by `validateFileOnlyOutputMode` and by delivery-side `finalizeSingleOutput`.
    #[test]
    fn the_system_prompt_override_is_unconditional_on_output_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "- persona".to_string();
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        for mode in [
            OutputMode::FileOnly,
            OutputMode::FileAndInline,
            OutputMode::Inline,
        ] {
            let mut opts = base_opts(dir.path(), &["m1"]);
            opts.output_mode = mode;
            opts.output_path = Some(dir.path().join("out.md"));
            let plan = build_attempt_spawn_plan(
                &agent,
                &ModelId::from("m1"),
                "task",
                &opts,
                depth,
                dir.path(),
                None,
            )
            .expect("plan builds");
            let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();
            assert!(
                delivered.contains("Runtime output path override:"),
                "{mode:?} with a configured path must carry the override: {delivered:?}"
            );
        }
    }

    /// The capability branch (`formatOutputPathInstruction`, `single-output.ts:84-91`) must be
    /// live on this surface too, not only on the task side: a read-only agent is told the runtime
    /// will persist its final response.
    #[test]
    fn the_system_prompt_override_branches_on_the_agents_tool_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));

        let mut read_only = sample_agent_config("m1", &[]);
        read_only.system_prompt_body = "- persona".to_string();
        read_only.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("grep".to_string()),
        ]);
        let plan = build_attempt_spawn_plan(
            &read_only,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();
        assert!(
            delivered.contains("Return the complete artifact in your final response."),
            "{delivered:?}"
        );
        assert!(
            !delivered.contains("Write your findings to exactly this path:"),
            "a read-only agent must not be ordered to write: {delivered:?}"
        );

        let mut write_capable = sample_agent_config("m1", &[]);
        write_capable.system_prompt_body = "- persona".to_string();
        write_capable.tools = Some(vec![
            crate::discovery::types::ToolRef::Builtin("read".to_string()),
            crate::discovery::types::ToolRef::Builtin("write".to_string()),
        ]);
        let plan = build_attempt_spawn_plan(
            &write_capable,
            &ModelId::from("m1"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();
        assert!(
            delivered.contains("Write your findings to exactly this path:"),
            "{delivered:?}"
        );
        assert!(
            !delivered.contains("Return the complete artifact in your final response."),
            "{delivered:?}"
        );
    }

    /// An empty persona plus a configured output path composes a NON-empty body, so the flag ships
    /// — pi emits its system-prompt flag for any non-null string (`runs/shared/pi-args.ts:570-585`), and the
    /// composed value is exactly the override. The empty-body omission delta survives only for the
    /// genuinely-empty case, which
    /// [`build_attempt_spawn_plan_omits_the_system_prompt_flag_for_an_empty_persona_body`] pins.
    #[test]
    fn an_empty_persona_with_an_output_path_still_ships_the_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Append;
        agent.system_prompt_body = "   \n\t ".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
                .expect("plan builds");
        let argv = plan.spec.build_argv();
        let delivered = delivered_system_prompt(&argv).unwrap_or_default();
        assert!(
            delivered.starts_with("Runtime output path override:"),
            "the override alone must still ship; argv was {argv:?}"
        );
    }

    /// Both halves reach the SAME run, as upstream's foreground single run does: the task side
    /// from `subagent-executor.ts:3674` (cyrup: [`build_task_text`]) and the system-prompt side
    /// from `execution.ts:1443` (cyrup: [`build_attempt_spawn_plan`]). Wiring one and not the
    /// other is the gap this pins closed.
    #[test]
    fn both_output_path_surfaces_reach_the_same_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "- persona".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.output_path = Some(dir.path().join("out.md"));
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let task = build_task_text(&agent, "do the thing", &opts, &contract, "");
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &task, &opts, depth, dir.path(), None)
                .expect("plan builds");
        let delivered = delivered_system_prompt(&plan.spec.build_argv()).unwrap_or_default();

        assert!(task.contains("\n\n---\n**Output:**\n"), "task side missing: {task:?}");
        assert!(
            delivered.contains("Runtime output path override:"),
            "system-prompt side missing: {delivered:?}"
        );
        // The task-side header must NOT leak onto the system prompt, nor vice versa: they are
        // distinct upstream strings and `stripFrameworkInstructions` only knows the task one.
        assert!(
            !delivered.contains("**Output:**"),
            "the task-side header must not appear on the system prompt: {delivered:?}"
        );
        assert!(
            !task.contains("Runtime output path override:"),
            "the system-prompt-side header must not appear in the task: {task:?}"
        );
    }

    #[test]
    fn build_attempt_spawn_plan_includes_tools_flag_only_when_agent_declares_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("edit".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(argv[tools_idx + 1], "read,edit");
    }

    #[test]
    fn build_attempt_spawn_plan_writes_the_child_intercom_bridge_env_when_orchestrator_target_is_set() {
        // The production activation path: when the orchestrator's presence target + this run's id are
        // present in `RunOptions`, the spawn overlay MUST write all six child-bridge identity vars so
        // the spawned child's `IntercomExtension` reads `read_child_orchestrator_metadata() == Some`
        // and registers `contact_supervisor` + a broker presence under its own deterministic label.
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]); // name = "worker"
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.orchestrator_intercom_target = Some("subagent-chat-abcd1234".to_string());
        opts.run_id = Some(crate::background::RunId::from_token("run-XYZ"));
        opts.child_index = Some(2);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert_eq!(
            env.get(crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET).map(String::as_str),
            Some("subagent-chat-abcd1234")
        );
        assert_eq!(env.get(crate::spawn::nested_events::RUN_ID_ENV).map(String::as_str), Some("run-XYZ"));
        assert_eq!(env.get(crate::spawn::intercom_target::ENV_CHILD_AGENT).map(String::as_str), Some("worker"));
        assert_eq!(env.get(crate::spawn::nested_events::CHILD_INDEX_ENV).map(String::as_str), Some("2"));
        // The child's own label = resolve_subagent_intercom_target(run_id, agent, index) — the SAME
        // string the parent's `control_resume` recomputes to steer it (index+1 suffix).
        assert_eq!(
            env.get(crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME).map(String::as_str),
            Some("subagent-worker-run-xyz-3")
        );
    }

    /// G106 (pi `runs/shared/pi-args.ts:221-231`, `3ac0ef5` "Make supervisor coordination native"): the single
    /// spawn-plan chokepoint must hand a child BOTH native-supervisor-channel vars and CREATE the
    /// `requests/`+`replies/` directories.
    ///
    /// The user action: `/run reviewer "..."` (or the `subagent` tool) from a live session. Without
    /// this the spawned child's `read_child_metadata` returns `None`, it registers no native
    /// `contact_supervisor`, and the file channel is unreachable — which is the pre-fix state.
    #[test]
    fn build_attempt_spawn_plan_writes_the_native_supervisor_channel_env() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]); // name = "worker"
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.orchestrator_intercom_target = Some("subagent-chat-abcd1234".to_string());
        opts.run_id = Some(crate::background::RunId::from_token("run-NSC"));
        opts.child_index = Some(2);
        opts.parent_session_id = Some("session-parent-1".to_string());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;

        assert_eq!(
            env.get(crate::spawn::intercom_target::ENV_ORCHESTRATOR_SESSION_ID).map(String::as_str),
            Some("session-parent-1"),
            "the native channel keys every request on the launching session's own id"
        );
        let channel_dir = env
            .get(crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR)
            .map(std::path::PathBuf::from)
            .expect("the spawn planner must hand the child its channel directory");
        assert_eq!(
            channel_dir,
            crate::native_supervisor::resolve_supervisor_channel_dir("run-NSC", "worker", 2),
            "the child's channel dir must be the SAME path the parent's poller scans"
        );
        assert!(
            channel_dir.join("requests").is_dir() && channel_dir.join("replies").is_dir(),
            "both sub-directories are created up front (pi's two mkdirSync calls)"
        );

        // The child's own gate opens on exactly what the planner wrote — the two halves meet.
        let metadata = crate::native_supervisor::read_child_metadata_from(&|k| env.get(k).cloned())
            .expect("the child metadata gate must open on the planner's overlay");
        assert_eq!(metadata.run_id, "run-NSC");
        assert_eq!(metadata.agent, "worker");
        assert_eq!(metadata.child_index, 2);
        assert_eq!(metadata.orchestrator_session_id, "session-parent-1");

        let _ = std::fs::remove_dir_all(&channel_dir);
    }

    /// The negative half: no parent session id means no routing key, so NEITHER var is written — a
    /// half-set overlay would leave the child holding a channel it cannot address.
    #[test]
    fn build_attempt_spawn_plan_omits_the_supervisor_channel_env_without_a_parent_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.orchestrator_intercom_target = Some("subagent-chat-abcd1234".to_string());
        opts.run_id = Some(crate::background::RunId::from_token("run-NSC2"));
        opts.child_index = Some(0);
        opts.parent_session_id = None;
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_SUPERVISOR_CHANNEL_DIR));
        assert!(crate::native_supervisor::read_child_metadata_from(&|k| env.get(k).cloned()).is_none());
    }

    #[test]
    fn build_attempt_spawn_plan_omits_the_child_intercom_bridge_env_without_an_orchestrator_target() {
        // A headless / no-intercom run (no orchestrator target) must leave the child un-bridged — the
        // clean no-intercom path, so a plain run spawns no broker-participant child.
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]); // orchestrator_intercom_target / run_id both None
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET));
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME));
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_CHILD_AGENT));
    }

    /// TOOL-031 / PARITY-GAPS PB-5 — the agent-identity markers must survive the re-exec.
    ///
    /// pi sets them on `process.env` in `cli.ts` before `main()` (`PI_CODING_AGENT = "true"`,
    /// `cli.ts:13` @v0.83.0; `AI_AGENT = "pi"`, `:14` @v0.84.1, mirrored in `rpc-entry.ts:7-8`), so
    /// a re-exec'd subagent child inherits them and so does everything the child then spawns.
    ///
    /// RED before this pass: cyrup's bin declines the process-global `set_var`, and only the `bash`
    /// TOOL pushed the pair per-child. The subagent spawn overlay pushed neither, so the marker
    /// chain broke at the first re-exec and the whole child subtree ran with them unset.
    ///
    /// Asserted unconditionally (not "present when some option is set") because the overlay is
    /// applied OVER an inherited environment: an omitted key silently keeps whatever the parent
    /// happened to have.
    #[test]
    fn the_spawn_overlay_carries_the_agent_identity_markers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert_eq!(
            env.get("PI_CODING_AGENT").map(String::as_str),
            Some("true"),
            "`PI_CODING_AGENT = \"true\"` (pi `cli.ts:13`) must reach the re-exec'd child"
        );
        assert_eq!(
            env.get("AI_AGENT").map(String::as_str),
            Some("cyrup"),
            "`AI_AGENT` (pi `cli.ts:14`) names WHICH agent is running; the key is pi's verbatim"
        );
    }

    /// CFG-069 — the third and last of the three sites that write `AI_AGENT` into a child.
    ///
    /// The key does not exist at the ported tag at all (`git -C pi grep -n AI_AGENT v0.83.0 --
    /// packages/` → 0; `cli.ts:13` @v0.83.0 sets only `PI_CODING_AGENT`); it arrives at
    /// `cli.ts:14` @v0.84.1 — re-derived at both tags this pass. So the delta annotation must name
    /// the KEY and the TAG, not only the VALUE, or a later v0.84.1 uplift reads the site as
    /// already-done-at-tag and never records that cyrup ran ahead of the baseline.
    ///
    /// RED before this pass: the annotation above the insert read `[CYRUP-DELTA, value only]
    /// `AI_AGENT` names WHICH agent is running (`"pi"` upstream).` — one line, carrying neither
    /// `@v0.84.1` nor the absent-at-v0.83.0 fact.
    ///
    /// Mirrors `cyrup-tools`' `cfg069_the_bash_tool_delta_names_the_forward_ported_key_and_its_tag`
    /// and `cyrup-session-svc`'s `the_forward_ported_ai_agent_marker_names_its_key_and_its_tag`.
    #[test]
    fn cfg069_the_spawn_overlay_delta_names_the_forward_ported_key_and_its_tag() {
        let src = include_str!("mod.rs");

        assert!(
            src.contains(r#"env_overlay.insert("PI_CODING_AGENT".to_string(), "true".to_string());"#),
            "the at-tag marker `PI_CODING_AGENT` (cli.ts:13 @v0.83.0) must still be written"
        );

        let insert = r#"env_overlay.insert("AI_AGENT".to_string(), "cyrup".to_string());"#;
        let at = src.find(insert).expect("`AI_AGENT` is written into the spawn overlay");
        let annotation = &src[..at];
        let annotation = &annotation[annotation.rfind("[CYRUP-DELTA").expect("a delta annotation")..];

        assert!(
            annotation.contains("@v0.84.1"),
            "the delta line must state the TAG the key comes from; got: {annotation}"
        );
        assert!(
            annotation.contains("AI_AGENT"),
            "the delta line must name the KEY, not only its value; got: {annotation}"
        );
        assert!(
            annotation.contains("v0.83.0"),
            "the delta line must state that the key is ABSENT at the ported tag; got: {annotation}"
        );
    }

    #[test]
    fn build_attempt_spawn_plan_omits_tools_flag_when_agent_declares_no_allowlist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(!plan.spec.build_argv().contains(&"--tools".to_string()));
    }

    #[test]
    fn build_attempt_spawn_plan_includes_session_flag_when_fork_context_resolved() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.fork_context = ForkContext {
            mode: ContextMode::Fork,
            session_file_path: Some(dir.path().join("parent-branch.jsonl")),
        };
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--session").expect("--session present");
        assert!(argv[idx + 1].contains("parent-branch.jsonl"));
        // pi's `sessionFile` branch (`runs/shared/pi-args.ts:101-103`) emits ONLY `--session`: never the
        // `--no-session`/`--session-dir` pair from the else arm.
        assert!(!argv.contains(&"--no-session".to_string()));
        assert!(!argv.contains(&"--session-dir".to_string()));
    }

    /// SUBA-041 prerequisite (pi `buildPiArgs`, `runs/shared/pi-args.ts:517-528`): with NO fork-context session
    /// file, no `session_dir` and no `share`, pi's `sessionEnabled` is false and the child is spawned
    /// `--no-session`. Pre-fix this arm emitted nothing at all, so every session-less subagent child
    /// silently persisted a session into the orchestrator's own store.
    #[test]
    fn build_attempt_spawn_plan_emits_no_session_when_nothing_enables_a_session() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(argv.contains(&"--no-session".to_string()), "argv: {argv:?}");
        assert!(!argv.contains(&"--session-dir".to_string()), "argv: {argv:?}");
    }

    /// SUBA-041 prerequisite: an explicit `session_dir` both ENABLES sessions (no `--no-session`)
    /// and reaches the child as `--session-dir <dir>`, with the directory created up front — pi's
    /// `fs.mkdirSync(sessionDir, { recursive: true })` (`runs/shared/pi-args.ts:524-526`).
    #[test]
    fn build_attempt_spawn_plan_emits_session_dir_and_creates_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        let session_dir = dir.path().join("sessions").join("run-0");
        opts.session_dir = Some(session_dir.clone());
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv
            .iter()
            .position(|a| a == "--session-dir")
            .expect("--session-dir present");
        assert_eq!(argv[idx + 1], session_dir.display().to_string());
        assert!(
            !argv.contains(&"--no-session".to_string()),
            "an explicit session dir enables sessions: {argv:?}"
        );
        assert!(session_dir.is_dir(), "the session dir must be created up front");
    }

    /// SUBA-041 prerequisite: `share: true` alone is pi's other `sessionEnabled` term
    /// (`execution.ts:1412`) — it suppresses `--no-session` without naming a directory.
    #[test]
    fn build_attempt_spawn_plan_share_alone_enables_sessions_without_a_session_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.share = Some(true);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        assert!(!argv.contains(&"--no-session".to_string()), "argv: {argv:?}");
        assert!(!argv.contains(&"--session-dir".to_string()), "argv: {argv:?}");
    }

    /// SUBA-041 prerequisite: `share: false` is NOT an enabling value — pi's term is
    /// `options.share === true` (`execution.ts:1027`), so an explicit `false` still yields
    /// `--no-session`.
    #[test]
    fn build_attempt_spawn_plan_share_false_still_disables_sessions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let mut opts = base_opts(dir.path(), &["m1"]);
        opts.share = Some(false);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert!(plan.spec.build_argv().contains(&"--no-session".to_string()));
    }

    #[test]
    fn build_attempt_spawn_plan_propagates_depth_envelope_into_env_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 2,
            max_depth: 4,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"2".to_string())
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"4".to_string())
        );
    }

    // ---- PERM-001: the child-ROLE env pair (pi `augmentChildEnv`, `runs/shared/pi-args.ts:329-330`) ----

    /// The production spawn path MUST mark the child as a child. Without this entry the re-exec'd
    /// process is indistinguishable from a top-level session, so
    /// `cyrup_permission_system::permission_extension_for_env` gives it the LOCAL ask channel
    /// instead of the `ForwardingAskChannel`; with no TTY it then has no reachable human and
    /// fail-CLOSES every `ask`-tier tool call rather than forwarding it to the parent's human
    /// through the `<agentDir>/sessions/permission-forwarding/` spool.
    ///
    /// The cross-process proof that the ask actually TRAVELS on this env is
    /// `cyrup-permission-system/tests/forwarding_spawn_env.rs`; this test is the other half of that
    /// chain — that the entry originates in the real spawn planner and not in a test's own hand.
    #[test]
    fn build_attempt_spawn_plan_marks_the_child_as_a_subagent_child_in_the_env_overlay() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::CHILD_ENV),
            Some(&"1".to_string()),
            "every spawned child must carry the child-role flag; overlay was {:?}",
            plan.spec.env_overlay
        );
        // Explicit `"0"`, never absent: the overlay is applied OVER the inherited env, so omitting it
        // would let a fanout-authorized parent's own `1` leak into an unauthorized grandchild.
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"0".to_string()),
            "an unauthorized child must be pinned to fanout=0, not left to inherit; overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    /// The consequence of the flag, at the seam that reads it: a child spawned by the production
    /// planner registers NO subagent surface at all (pi `extension/index.ts:177`), instead of the
    /// full orchestrator surface — its own `subagent` tool, 12 slash commands and background
    /// watcher — that an unmarked child was silently installing.
    #[test]
    fn a_child_spawned_by_the_production_planner_registers_no_subagent_surface() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        let is_one = |name: &str| plan.spec.env_overlay.get(name).map(String::as_str) == Some("1");
        let mode = crate::extension::resolve_registration_mode(
            is_one(crate::spawn::nested_events::CHILD_ENV),
            is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
        );
        assert!(
            mode.is_none(),
            "a plain spawned child must register nothing, got {mode:?} from overlay {:?}",
            plan.spec.env_overlay
        );
    }

    /// The other half of pi's `fanoutAuthorized = declaredBuiltinTools.includes("subagent")`
    /// (`runs/shared/pi-args.ts:194`, written to the env at `:330`): a persona that declares the
    /// `subagent` tool IS granted nested delegation, so the flag must track the agent's tools rather
    /// than being pinned off. Pinning it off makes [`RegistrationMode::ChildSafe`] unreachable in
    /// production and the whole depth envelope (`max_depth`, R-SA-054) decorative — nothing could
    /// ever reach depth 2.
    #[test]
    fn an_agent_declaring_the_subagent_tool_spawns_a_fanout_authorized_child_that_can_delegate() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin(crate::extension::TOOL_NAME.to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"1".to_string()),
            "a persona that declares `subagent` must be spawned fanout-authorized; overlay was {:?}",
            plan.spec.env_overlay
        );
        // The observable consequence at the seam that reads the flag: this child registers the
        // restricted subagent surface, so it can delegate (pi `extension/fanout-child.ts:132`).
        let is_one = |name: &str| plan.spec.env_overlay.get(name).map(String::as_str) == Some("1");
        let mode = crate::extension::resolve_registration_mode(
            is_one(crate::spawn::nested_events::CHILD_ENV),
            is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
        );
        assert_eq!(
            mode,
            Some(crate::extension::RegistrationMode::ChildSafe),
            "a fanout-authorized child registers the restricted surface, got {mode:?} from overlay {:?}",
            plan.spec.env_overlay
        );
        // It is still a CHILD: its asks forward to the parent's spool rather than resolving locally.
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::CHILD_ENV),
            Some(&"1".to_string())
        );
    }

    /// A persona that declares tools but NOT `subagent` stays unauthorized — the grant is per-agent,
    /// not "anyone who declares any tools" (pi's `.includes("subagent")` is an exact membership
    /// test).
    #[test]
    fn declaring_other_tools_does_not_grant_fanout() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Builtin("bash".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::nested_events::FANOUT_CHILD_ENV),
            Some(&"0".to_string()),
            "only a declared `subagent` tool grants fanout; overlay was {:?}",
            plan.spec.env_overlay
        );
    }

    // ---- T4: thinking suffix (pi `applyThinkingSuffix`), inherit flags, extension threading,
    // and the direct-MCP tools split (`mcp:` refs no longer leak into `--tools` literally) ----

    #[test]
    fn apply_thinking_suffix_appends_level_to_a_provider_qualified_model() {
        assert_eq!(
            apply_thinking_suffix(Some("openai-codex/gpt-5.4-mini"), Some("high")).as_deref(),
            Some("openai-codex/gpt-5.4-mini:high")
        );
    }

    #[test]
    fn apply_thinking_suffix_passes_explicit_off_through() {
        // pi: "passes explicit thinking off through to the model arg".
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-haiku-4-5"), Some("off")).as_deref(),
            Some("anthropic/claude-haiku-4-5:off")
        );
    }

    #[test]
    fn apply_thinking_suffix_leaves_a_non_thinking_provider_suffix_untouched() {
        // pi: "leaves provider-specific model suffixes untouched when thinking is disabled". A
        // `:7b`-style suffix is not a THINKING_LEVEL, so with no thinking requested the id is
        // returned verbatim (no double-suffix, no accidental `:high`).
        assert_eq!(
            apply_thinking_suffix(Some("openai-compatible/qwen2.5-coder:7b"), None).as_deref(),
            Some("openai-compatible/qwen2.5-coder:7b")
        );
    }

    #[test]
    fn apply_thinking_suffix_does_not_double_suffix_an_existing_thinking_level() {
        assert_eq!(
            apply_thinking_suffix(Some("model:high"), Some("low")).as_deref(),
            Some("model:high")
        );
    }

    /// PROV-002: `max` must be a RECOGNIZED suffix. With the 6-entry list, a model id already
    /// ending `:max` was not recognized and got double-suffixed to `model:max:high`, producing an
    /// unresolvable id for the child process. Upstream pi-subagents fixed this in 747de75
    /// (`src/shared/model-info.ts:1`).
    #[test]
    fn apply_thinking_suffix_recognizes_max_as_an_existing_level() {
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6:max"), Some("high")).as_deref(),
            Some("anthropic/claude-opus-4-6:max"),
            "an existing `:max` must not be double-suffixed"
        );
        // …and `max` is appendable as a level in its own right.
        assert_eq!(
            apply_thinking_suffix(Some("anthropic/claude-opus-4-6"), Some("max")).as_deref(),
            Some("anthropic/claude-opus-4-6:max")
        );
    }

    #[test]
    fn apply_thinking_suffix_returns_none_without_a_model() {
        assert_eq!(apply_thinking_suffix(None, Some("high")), None);
    }

    #[test]
    fn build_attempt_spawn_plan_suffixes_the_child_model_with_the_agent_thinking_level() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("openai-codex/gpt-5.4-mini", &[]);
        agent.thinking = Some("high".to_string());
        let opts = base_opts(dir.path(), &["openai-codex/gpt-5.4-mini"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(
            &agent,
            &ModelId::from("openai-codex/gpt-5.4-mini"),
            "task",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(argv[idx + 1], "openai-codex/gpt-5.4-mini:high");
    }

    #[test]
    fn an_inheriting_persona_spawns_the_child_with_the_parent_session_model() {
        // (a) End-to-end resolution proof (no LLM): a persona with NO model of its own
        // (model = None, fallback_models = []), run with NO per-call override, under a live parent
        // session model X, resolves X as candidate #0 and spawns the child with `--model X`. Before
        // this seam the ladder was empty and the run hard-failed with "no candidate model available".
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("unused", &[]);
        agent.model = None; // inheriting persona: no own model, no fallbacks
        agent.thinking = None;

        let inherited = ModelId::from("together/zai-org/GLM-5.2");
        // available_models is built the way run_foreground_impl / run_single build it (persona
        // fallbacks + own model), then resolve_model_inheritance folds in the inherited parent model.
        let mut available_models: Vec<ModelId> =
            agent.fallback_models.iter().cloned().chain(agent.model.clone()).collect();
        let ov = crate::exec::fallback::resolve_model_inheritance(
            None, // no per-call override
            agent.model.as_ref(),
            Some(&inherited),
            &mut available_models,
            None, // no modelScope policy configured
        )
        .expect("with no scope configured, resolution can never be refused");
        assert!(
            available_models.contains(&inherited),
            "the inherited model must be added to available_models so the allowlist filter keeps it"
        );

        let candidates = crate::exec::fallback::build_model_candidates(
            &ov,
            agent.model.as_ref(),
            &agent.fallback_models,
            &available_models,
        );
        assert_eq!(
            candidates,
            vec![inherited.clone()],
            "the inherited parent-session model is the sole/primary candidate (non-empty ladder)"
        );

        let opts = base_opts(dir.path(), &["together/zai-org/GLM-5.2"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &candidates[0], "task", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--model").expect("--model present");
        assert_eq!(
            argv[idx + 1], "together/zai-org/GLM-5.2",
            "the child must spawn with the inherited parent session model as --model"
        );
    }

    #[test]
    fn build_attempt_spawn_plan_emits_no_skills_only_when_the_agent_does_not_inherit_skills() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };

        let mut inheriting = sample_agent_config("m1", &[]);
        inheriting.inherit_skills = true;
        let plan =
            build_attempt_spawn_plan(&inheriting, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
                .expect("plan builds");
        assert!(
            !plan.spec.build_argv().contains(&"--no-skills".to_string()),
            "an agent that inherits skills must NOT be spawned with --no-skills"
        );
        assert_eq!(
            plan.spec.env_overlay.get("CYRUP_SUBAGENT_INHERIT_SKILLS"),
            Some(&"1".to_string())
        );

        let mut not_inheriting = sample_agent_config("m1", &[]);
        not_inheriting.inherit_skills = false;
        let plan = build_attempt_spawn_plan(
            &not_inheriting,
            &ModelId::from("m1"),
            "t",
            &opts,
            depth,
            dir.path(),
            None,
        )
        .expect("plan builds");
        assert!(
            plan.spec.build_argv().contains(&"--no-skills".to_string()),
            "an agent that does NOT inherit skills must be spawned with --no-skills"
        );
        assert_eq!(
            plan.spec.env_overlay.get("CYRUP_SUBAGENT_INHERIT_SKILLS"),
            Some(&"0".to_string())
        );
    }

    #[test]
    fn build_attempt_spawn_plan_threads_inherit_project_context_and_none_mcp_sentinel() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.inherit_project_context = true;
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        assert_eq!(
            plan.spec.env_overlay.get("CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT"),
            Some(&"1".to_string())
        );
        // No direct MCP tools declared -> pi's `__none__` sentinel, never an unset/empty value.
        assert_eq!(
            plan.spec.env_overlay.get("MCP_DIRECT_TOOLS"),
            Some(&"__none__".to_string())
        );
        // Permission input (1) (pi `resolveAgentName`): the resolved persona name is threaded to the
        // child as `CYRUP_SUBAGENT_AGENT_NAME` so the child's permission companion's agent +
        // projectAgent policy layers enforce for the named persona.
        assert_eq!(
            plan.spec.env_overlay.get(AGENT_NAME_ENV_VAR),
            Some(&"worker".to_string())
        );
    }

    #[test]
    fn build_attempt_spawn_plan_omits_agent_name_env_for_unnamed_persona() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.name = String::new();
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        // An empty persona name writes NO var (child resolves `None` — pi's top-level `""`).
        assert_eq!(plan.spec.env_overlay.get(AGENT_NAME_ENV_VAR), None);
    }

    /// SUBA-008 — the budget notice must reach the CHILD, and it must reach it through the system
    /// prompt: there is no `CYRUP_SUBAGENT_TURN_BUDGET` env var to carry it (unlike the tool
    /// budget), so if this composition is missing, the child is silently never told to self-pace
    /// and only discovers the budget by being killed.
    ///
    /// Reads the spilled prompt FILE rather than the argv, because SUBA-030 moved the persona off
    /// the command line — asserting on argv alone would pass while the file held anything at all.
    #[test]
    fn the_turn_budget_notice_reaches_the_child_through_the_spilled_system_prompt_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_body = "You are a careful worker.".to_string();
        let mut opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };

        // Presence before absence: with NO budget the block must not appear at all, so the
        // assertion below cannot be satisfied by some unrelated boilerplate.
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let unbudgeted = read_system_prompt_arg(&plan);
        assert_eq!(unbudgeted.trim(), "You are a careful worker.");
        assert!(!unbudgeted.contains("## Turn budget"), "{unbudgeted}");

        opts.turn_budget = Some(crate::exec::turn_budget::ResolvedTurnBudget {
            max_turns: 4,
            grace_turns: 2,
        });
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let budgeted = read_system_prompt_arg(&plan);
        // The persona survives and the block is APPENDED after it (pi's outermost append,
        // `execution.ts:326`), never substituted for it.
        assert!(budgeted.starts_with("You are a careful worker.\n\n## Turn budget\n"), "{budgeted}");
        assert!(budgeted.contains("a soft budget of 4 assistant turns."), "{budgeted}");
        assert!(
            budgeted.contains("After that, 2 additional assistant turns may be allowed only for a final wrap-up."),
            "{budgeted}"
        );
    }

    /// Read back the file `--system-prompt`/`--append-system-prompt` points at in a built plan.
    fn read_system_prompt_arg(plan: &AttemptSpawnPlan) -> String {
        let argv = plan.spec.build_argv();
        let idx = argv
            .iter()
            .position(|a| a == SYSTEM_PROMPT_FLAG || a == APPEND_SYSTEM_PROMPT_FLAG)
            .expect("a non-empty persona must push a system-prompt flag");
        std::fs::read_to_string(&argv[idx + 1]).expect("the spilled prompt file must exist")
    }

    #[test]
    fn build_attempt_spawn_plan_threads_extensions_and_subagent_only_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // A builtin + an extension-path tool ref; the extension path must go to --extension, NOT --tools.
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::ExtensionPath("./custom-tool.ts".to_string()),
        ]);
        agent.extensions = Some(vec!["./allowed-ext.ts".to_string()]);
        agent.subagent_only_extensions = vec!["./child-tool.ts".to_string()];
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();

        // `Some(extensions)` turns discovery off.
        assert!(argv.contains(&"--no-extensions".to_string()));
        let ext_args: Vec<&String> = argv
            .iter()
            .enumerate()
            .filter_map(|(i, a)| (i > 0 && argv[i - 1] == "--extension").then_some(a))
            .collect();
        assert!(ext_args.iter().any(|a| a.as_str() == "./custom-tool.ts"), "tool-extension path threaded");
        assert!(ext_args.iter().any(|a| a.as_str() == "./allowed-ext.ts"), "allowlisted extension threaded");
        assert!(ext_args.iter().any(|a| a.as_str() == "./child-tool.ts"), "child-only extension threaded");

        // The extension path is NOT in --tools; only the builtin is.
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        assert_eq!(argv[tools_idx + 1], "read");
    }

    #[test]
    fn build_attempt_spawn_plan_splits_mcp_refs_out_of_tools_and_sets_the_env() {
        // The T4 fix: an `mcp:` ref (`ToolRef::Mcp`) must NOT flow into `--tools` literally (an
        // unresolvable name). It is resolved via the direct-MCP allowlist (here, with no metadata
        // cache on disk, it resolves to nothing) and the raw selector is surfaced via the
        // `MCP_DIRECT_TOOLS` env — leaving only the builtin in `--tools`.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tools = Some(vec![
            ToolRef::Builtin("read".to_string()),
            ToolRef::Mcp("chrome-devtools".to_string()),
        ]);
        let opts = base_opts(dir.path(), &["m1"]);
        let depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path(), None)
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let tools_idx = argv.iter().position(|a| a == "--tools").expect("--tools present");
        let tools_val = &argv[tools_idx + 1];
        assert!(
            tools_val.split(',').next() == Some("read"),
            "the declared builtin must lead the allowlist, got {tools_val}"
        );
        assert!(
            !tools_val.contains("chrome-devtools"),
            "the literal `mcp:` selector must never appear in --tools (the T4 bug), got {tools_val}"
        );
        // The raw selector list is surfaced to the child's MCP adapter via env, verbatim.
        assert_eq!(
            plan.spec.env_overlay.get("MCP_DIRECT_TOOLS"),
            Some(&"chrome-devtools".to_string())
        );
    }

    // ---- T0.3 (C15, SAFETY): the CHILD env overlay increments depth by exactly one and applies
    // the agent's tightening-only max, exactly as `SpawnedChildAttemptRunner::run_attempt`
    // computes it (`next_envelope(&self.agent.depth, self.agent.max_subagent_depth)`), NOT the
    // parent's own envelope verbatim (the prior bug). This is the regression guard for the
    // call-site fix at line ~523: it reproduces run_attempt's exact child-envelope composition and
    // asserts the rendered spawn-env overlay a real child would inherit. ----

    #[test]
    fn child_spawn_env_increments_depth_by_one_over_the_parent_envelope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // Parent process sits at depth 1 of a ceiling-5 run, and declares NO tighter agent-level
        // ceiling of its own — so the child must inherit depth 2 (1 + 1) under the same ceiling 5.
        agent.depth = DepthEnvelope {
            current_depth: 1,
            max_depth: 5,
        };
        agent.max_subagent_depth = None;
        let opts = base_opts(dir.path(), &["m1"]);

        // Exactly what run_attempt now does before building the spawn plan.
        let child_depth =
            crate::spawn::depth::next_envelope(&agent.depth, agent.max_subagent_depth);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, child_depth, dir.path(), None)
                .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"2".to_string()),
            "the child MUST inherit parent_depth + 1, never the parent's own depth verbatim"
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"5".to_string()),
            "with no agent-level tightening, the inherited ceiling passes through unchanged"
        );
    }

    #[test]
    fn child_spawn_env_applies_the_agents_tightening_only_max_depth() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        // Inherited ceiling 5, but THIS agent declares its own tighter ceiling of 2 for its
        // children — the child's env must carry min(5, 2) = 2 (R-SA-056 tightening-only).
        agent.depth = DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        };
        agent.max_subagent_depth = Some(2);
        let opts = base_opts(dir.path(), &["m1"]);

        let child_depth =
            crate::spawn::depth::next_envelope(&agent.depth, agent.max_subagent_depth);
        let plan =
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, child_depth, dir.path(), None)
                .expect("plan builds");

        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::DEPTH_ENV_VAR),
            Some(&"1".to_string())
        );
        assert_eq!(
            plan.spec.env_overlay.get(crate::spawn::depth::MAX_DEPTH_ENV_VAR),
            Some(&"2".to_string()),
            "the agent's own tighter declared max must win over the looser inherited ceiling"
        );
    }

    // ---- T0.1 (C13): ResolvedAgentPersona is the serializable plan-time projection that lets a
    // chain/parallel/background step dispatch the REAL named persona instead of a placeholder. ----

    #[test]
    fn resolved_agent_persona_round_trips_through_json_preserving_every_field() {
        let persona = ResolvedAgentPersona {
            name: "reviewer".to_string(),
            model: Some(ModelId::from("reviewer-model")),
            fallback_models: vec![ModelId::from("backup-model")],
            thinking: Some("high".to_string()),
            system_prompt_mode: SystemPromptMode::Append,
            system_prompt_body: "You are the REVIEWER persona.".to_string(),
            tools: Some(vec![ToolRef::Builtin("read".to_string())]),
            extensions: Some(vec!["./allowed-ext.ts".to_string()]),
            subagent_only_extensions: vec!["./child-tool.ts".to_string()],
            output: None,
            inherit_project_context: true,
            inherit_skills: false,
            skills: vec!["accessibility".to_string(), "deslop".to_string()],
            completion_guard: Some(true),
            max_subagent_depth: Some(1),
            default_context: None,
            memory: None,
            tool_budget: None,
        };
        let json = serde_json::to_string(&persona).expect("serialize");
        let back: ResolvedAgentPersona = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, persona, "the persona must survive a RunnerConfig JSON round-trip intact");
    }

    #[test]
    fn to_agent_config_stamps_the_live_depth_and_reproduces_the_persona() {
        let persona = ResolvedAgentPersona {
            name: "reviewer".to_string(),
            model: Some(ModelId::from("reviewer-model")),
            fallback_models: vec![ModelId::from("backup-model")],
            thinking: Some("high".to_string()),
            system_prompt_mode: SystemPromptMode::Append,
            system_prompt_body: "You are the REVIEWER persona.".to_string(),
            tools: Some(vec![ToolRef::Builtin("read".to_string())]),
            extensions: Some(vec!["./allowed-ext.ts".to_string()]),
            subagent_only_extensions: vec!["./child-tool.ts".to_string()],
            output: None,
            inherit_project_context: true,
            inherit_skills: false,
            skills: vec!["accessibility".to_string()],
            completion_guard: Some(true),
            max_subagent_depth: Some(1),
            default_context: None,
            memory: None,
            tool_budget: None,
        };
        let live_depth = DepthEnvelope {
            current_depth: 1,
            max_depth: 3,
        };
        let cfg = persona.to_agent_config(live_depth);

        // The persona's own fields reach the execution-ready config verbatim — this is what makes
        // `## reviewer` actually run the reviewer (real system prompt, model, guard, tools, thinking,
        // extensions, and inherit flags).
        assert_eq!(cfg.name, "reviewer");
        assert_eq!(cfg.system_prompt_body, "You are the REVIEWER persona.");
        assert_eq!(cfg.system_prompt_mode, SystemPromptMode::Append);
        assert_eq!(cfg.model.as_ref().map(ModelId::as_str), Some("reviewer-model"));
        assert_eq!(cfg.fallback_models, vec![ModelId::from("backup-model")]);
        assert_eq!(cfg.completion_guard, Some(true));
        assert_eq!(cfg.tools, Some(vec![ToolRef::Builtin("read".to_string())]));
        assert_eq!(cfg.max_subagent_depth, Some(1));
        assert_eq!(cfg.thinking, Some("high".to_string()));
        assert_eq!(cfg.extensions, Some(vec!["./allowed-ext.ts".to_string()]));
        assert_eq!(cfg.subagent_only_extensions, vec!["./child-tool.ts".to_string()]);
        assert!(cfg.inherit_project_context);
        assert!(!cfg.inherit_skills);
        // The persona's own `skills` list reaches the execution config so a chain/parallel/background
        // step injects the SAME `<available_skills>` block the single-run path does.
        assert_eq!(cfg.skills, vec!["accessibility".to_string()]);
        // The depth is the caller-stamped live envelope, not a plan-time value.
        assert_eq!(cfg.depth, live_depth);
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
