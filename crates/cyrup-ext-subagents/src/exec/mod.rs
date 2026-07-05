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

/// Implementation-expecting classification and mutating-tool-call scan (R-SA-034).
pub mod completion_guard;

/// Direct-MCP tool-allowlist resolution (T4) — `mcp:<server>[/<tool>]` selectors are expanded into
/// concrete adapter-visible builtin tool names for the child's `--tools` allowlist (pi
/// `resolveMcpDirectToolNames`, `runs/shared/mcp-direct-tool-allowlist.ts`), rather than passed
/// through literally.
pub mod mcp_direct_tools;

/// The model-fallback attempt loop (`build_model_candidates`, `is_retryable_model_failure`,
/// `run_fallback_ladder`) — R-SA-035/036/037/038/039/040/041/044.
pub mod fallback;

/// The NDJSON event-stream parser (`SubagentEvent`, `consume_stdout`) — R-SA-026/057/058.
pub mod ndjson;

/// Final-output extraction (R-SA-029), file-only output-path stat-snapshot handoff
/// (R-SA-024/025/031), and UTF-8-safe output truncation (R-SA-042).
pub mod output;

/// Parent-side structured-output extraction and JSON-Schema re-validation (R-SA-030).
pub mod structured;

/// `{text, expandedText}` tool-call argument previews (R-SA-043's compaction target) — pi
/// `ToolCallSummary` + `formatToolCall` (`shared/types.ts:225`, `shared/formatters.ts:99`).
pub mod tool_call_summary;

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
    build_timed_out_acceptance_ledger, evaluate_acceptance, inject_acceptance_contract,
};
use crate::exec::completion_guard::evaluate_completion_mutation_guard;
use crate::exec::fallback::{
    AttemptRunner, AttemptSignal, ModelAttempt, ModelOverride, build_model_candidates,
    run_fallback_ladder,
};
use crate::exec::ndjson::SubagentEvent;
use crate::exec::output::{
    EMPTY_OUTPUT_ERROR, INTERRUPTED_FINAL_OUTPUT, OutputCap,
    build_output_path_system_prompt_instruction, detect_subagent_error, extract_final_output,
    is_terminal_assistant_stop, message_end_has_error_message, resolve_output_handoff,
    snapshot_output_file, trailing_assistant_error, truncate_output,
    validate_file_only_requires_path,
};
use crate::exec::structured::{StructuredOutcome, resolve_structured_output};
use crate::fork_context::{ContextMode, ForkContext, ForkContextResolver};
use crate::spawn::depth::DepthEnvelope;
use crate::spawn::{ChildSpawnSpec, SpawnCommand, SpawnedChild};

/// R-SA-028 (MUST) — bounded recent-output buffer cap: `recent_output` in a live progress
/// snapshot MUST be capped at 50 lines (oldest evicted first) while the run is active.
pub const RECENT_OUTPUT_CAP: usize = 50;

/// The exact message a timed-out run leads its delivered output with, and the text of the timeout
/// error — a 1:1 port of pi's `formatTimeoutMessage` (`execution.ts:87-89`). `ms` is the NOMINAL
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
    /// [`apply_thinking_suffix`] (pi `applyThinkingSuffix`, `pi-args.ts:76-81`) — `None` leaves the
    /// per-attempt model id untouched. Carrying the raw string (rather than a closed on-only enum)
    /// means an explicit `Some("off")` now reaches the child as `:off`, exactly like pi, instead of
    /// being conflated with `None` and dropped.
    pub thinking: Option<String>,
    pub system_prompt_mode: SystemPromptMode,
    pub system_prompt_body: String,
    pub tools: Option<Vec<ToolRef>>,
    /// Extension-allowlist tri-state (func-SA §4.1 `AgentDefinition::extensions`): `Some(list)`
    /// emits `--no-extensions` plus an explicit `--extension` for each entry (discovery off, exact
    /// allowlist); `None` leaves the child's own extension discovery on (pi `pi-args.ts:128-137`).
    pub extensions: Option<Vec<String>>,
    /// Child-only extension paths always threaded as `--extension`, visible to the spawned child
    /// even when not visible to the orchestrator (pi `subagentOnlyExtensions`).
    pub subagent_only_extensions: Vec<String>,
    pub output: Option<crate::discovery::types::OutputSpec>,
    /// pi's `PI_SUBAGENT_INHERIT_PROJECT_CONTEXT` env flag: whether the child inherits the parent's
    /// project-context files (`AGENTS.md`/`CLAUDE.md`) — threaded to the child as
    /// `CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT=1|0` (pi `pi-args.ts:199`).
    pub inherit_project_context: bool,
    /// Whether the child inherits skills discovery: when `false`, the child is spawned with
    /// `--no-skills` and `CYRUP_SUBAGENT_INHERIT_SKILLS=0` (pi `pi-args.ts:139-141,200`).
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
    /// child's skills from the already-resolved agent config, `async-execution.ts:404-409,818-822`).
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
    /// `subagent-executor.ts:1280-1293`) — matching pi, where the already-resolved `agents` list the
    /// executor consults carries `defaultContext`. `#[serde(default)]` keeps the one-shot
    /// runner-config hand-off backward compatible.
    #[serde(default)]
    pub default_context: Option<ContextMode>,
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
    /// resolved by the orchestrator, `subagent-executor.ts:1327-1341`). Distinct from `deadline_at`
    /// (the actual wall-clock instant the timer fires at): `timeout_ms` is what the timed-out
    /// message renders (`Subagent timed out after {ms}ms.`, pi `formatTimeoutMessage`), while
    /// `deadline_at` is what [`run_sync`] actually races the child against. Normally the orchestrator
    /// sets `deadline_at = now + timeout_ms` once; when only `timeout_ms` is set, `run_sync` derives
    /// the deadline from it (pi `resolveAttemptTimeout`: `deadlineAt ?? now + timeoutMs`). `None`
    /// means no foreground timeout at all.
    pub timeout_ms: Option<u64>,
    pub output_path: Option<PathBuf>,
    pub output_mode: OutputMode,
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
    pub share: Option<bool>,
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
    /// R-SA-043: when `Some(false)`, even a still-running progress snapshot omits per-turn
    /// detail; `None`/`Some(true)` is the default fuller shape. `run_sync`'s own return value
    /// (always a terminal, compacted [`SingleResult`]) is unaffected either way — see
    /// [`SingleResult`]'s own doc comment for exactly what compaction means for a *completed*
    /// result vs. a live callback snapshot.
    pub include_progress: Option<bool>,
    pub agent_scope: Option<AgentReadScope>,
    /// Explicit acceptance-contract override for this task (func-SA §4.2 `acceptance`); `None`
    /// defers to [`AcceptanceContract::heuristic_default`] (R-SA-023).
    pub acceptance: Option<AcceptanceContract>,
    /// Resolved fork-context for this task, if any — normally produced by [`plan_batch`] ahead of
    /// time (R-SA-137) and threaded straight through here; `Fresh` (the default) when this task
    /// runs with no inherited session state.
    pub fork_context: ForkContext,
    /// A live raw-NDJSON-line sink the background hop-2 runner installs to observe this child's
    /// stdout as it streams (pi's `updateStepFromChildEvent` child-event pump,
    /// `subagent-runner.ts:1430-1517`), so it can fold `currentTool`/`recentTools`/token telemetry
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
    /// deterministic label (the child-bridge activation, pi `pi-args.ts:204-205`). `None` (headless /
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
    /// This child's flat index within its run (pi `childIndex`/`ctx.flatIndex`, `pi-args.ts:213-214`)
    /// — the `+1`-suffixed step position in its own presence label + the child's
    /// [`crate::spawn::nested_events::CHILD_INDEX_ENV`]. `None` defaults to `0` (a single top-level
    /// run has one child at index 0).
    pub child_index: Option<usize>,
}

/// A live per-line sink installed via [`RunOptions::live_events`]: [`run_sync`]'s per-attempt driver
/// hands every complete raw NDJSON stdout line to it as the line is read, BEFORE this crate parses
/// or folds the line, so a caller (the background runner) can parse it into its OWN telemetry event
/// vocabulary without this module depending on that caller. Cheap to clone (an `Arc`); a runtime
/// callback with no serializable content, so it is never persisted with the rest of [`RunOptions`].
#[derive(Clone)]
pub struct LiveEventSink(std::sync::Arc<dyn Fn(&str) + Send + Sync>);

impl std::fmt::Debug for LiveEventSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LiveEventSink(..)")
    }
}

impl LiveEventSink {
    /// Wrap a raw-line callback as a cheaply-cloneable sink.
    #[must_use]
    pub fn new(sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(sink))
    }

    /// Deliver one raw NDJSON stdout line to the installed callback.
    pub fn emit(&self, raw_line: &str) {
        (self.0)(raw_line);
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
/// **compacted** (R-SA-043) shape: no raw per-turn messages, no live `progress` object — only the
/// summarized fields below. A still-running progress snapshot used for live update callbacks
/// (`RunOptions.include_progress`-gated, §4.3) is a materially different, richer shape this crate
/// does not construct in this module (that belongs to `tui/` once it exists); `SingleResult` is
/// exclusively the terminal return value.
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
    pub error: Option<String>,
    /// Summarized `{text, expandedText}` tool-call previews observed across the winning attempt's
    /// transcript — R-SA-043's "only summarized `tool_calls`" compaction requirement (pi's
    /// `ToolCallSummary[]`, `utils.ts:368-373`). Each carries a short and an expanded argument
    /// preview (pi `formatToolCall`), NOT a bare tool name. Never the raw per-turn message list.
    pub tool_calls: Vec<ToolCallSummary>,
    /// Whether [`output::truncate_output`] actually cut the delivered `final_output` (R-SA-042).
    pub output_truncated: bool,
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
    /// Bounded ring buffer of recent raw NDJSON lines, oldest evicted first once
    /// [`RECENT_OUTPUT_CAP`] is exceeded (R-SA-028). Kept as raw text (not parsed events) since
    /// R-SA-028's own text speaks of "recent output" as a rendering/log concern, not a
    /// re-parseable event queue.
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
    /// chronological order — feeds [`structured::resolve_structured_output`] (R-SA-030), which
    /// needs more than the two narrower vectors above; `run_sync` also reads this directly for
    /// that R-SA-030 wiring, alongside `message_end_events`/`tool_end_events` for its own
    /// R-SA-029/034 wiring.
    pub all_events: Vec<SubagentEvent>,
}

impl AgentProgress {
    /// Fold one parsed [`SubagentEvent`] into this progress state (R-SA-027). Every `MessageEnd`
    /// event's usage is accumulated additively (never last-wins — mirrors
    /// [`fallback::add_usage`]'s own contract, restated here at the per-attempt granularity); every
    /// `ToolExecutionStart` increments `tool_count` and sets `current_tool`.
    pub fn record_event(&mut self, event: SubagentEvent) {
        if let Some(usage) = event.assistant_usage() {
            crate::exec::fallback::add_usage(&mut self.usage, &usage);
        }
        match &event {
            SubagentEvent::ToolExecutionStart { tool_name, .. } => {
                self.tool_count += 1;
                self.current_tool = Some(tool_name.clone());
            }
            SubagentEvent::MessageEnd { .. } => {
                self.message_end_events.push(event.clone());
            }
            SubagentEvent::ToolExecutionEnd { .. } => {
                self.tool_end_events.push(event.clone());
            }
            _ => {}
        }
        self.all_events.push(event);
    }

    /// Push one raw NDJSON line into the bounded `recent_output` ring buffer (R-SA-028): capped
    /// at [`RECENT_OUTPUT_CAP`] lines, oldest evicted first.
    pub fn record_raw_line(&mut self, line: &str) {
        if self.recent_output.len() >= RECENT_OUTPUT_CAP {
            self.recent_output.pop_front();
        }
        self.recent_output.push_back(line.to_string());
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
}

// ================================================================================================
// SubagentSpawner: the seam production spawning goes through (mirrors AttemptRunner's own
// production-vs-test seam, one level down at the real-subprocess boundary)
// ================================================================================================

/// Everything one attempt's spawn needs beyond what [`AgentConfig`]/[`RunOptions`] already carry —
/// factored out so [`SpawnedChildAttemptRunner`] can build a [`ChildSpawnSpec`] without repeating
/// argv/env assembly inline in `run_attempt` itself.
struct AttemptSpawnPlan {
    spec: ChildSpawnSpec,
}

/// The reasoning-level suffixes [`apply_thinking_suffix`] recognizes on a model id (pi
/// `THINKING_LEVELS`, `pi-args.ts:10`). Includes `off` — a value cyrup-core's closed on-only
/// `ThinkingLevel` enum cannot itself represent, but which the string-level suffix check must still
/// recognize so a model id that already ends `:off` is never double-suffixed.
const THINKING_LEVELS: [&str; 6] = ["off", "minimal", "low", "medium", "high", "xhigh"];

/// Child env flag: whether the subagent inherits the parent's project-context files
/// (`AGENTS.md`/`CLAUDE.md`) — pi `PI_SUBAGENT_INHERIT_PROJECT_CONTEXT` (`pi-args.ts:199`).
const INHERIT_PROJECT_CONTEXT_ENV: &str = "CYRUP_SUBAGENT_INHERIT_PROJECT_CONTEXT";

/// Child env flag: whether the subagent inherits skills discovery — pi
/// `PI_SUBAGENT_INHERIT_SKILLS` (`pi-args.ts:200`).
const INHERIT_SKILLS_ENV: &str = "CYRUP_SUBAGENT_INHERIT_SKILLS";

/// The MCP adapter's direct-tool-allowlist env (pi keeps this un-namespaced, `pi-args.ts:216-220`):
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
/// `resolveAgentName`, `index.ts:2033-2047`). cyrup spawns a subagent as a SEPARATE process that IS
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

/// pi `applyThinkingSuffix` (`pi-args.ts:76-81`): append `:<thinking>` to a model id, unless the
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

/// Append `item` to `vec` only if not already present — the order-preserving de-duplication pi
/// achieves with `[...new Set(...)]` over the extension-path list (`pi-args.ts:130,134`).
fn push_unique(vec: &mut Vec<String>, item: String) {
    if !vec.contains(&item) {
        vec.push(item);
    }
}

/// Build the argv + env overlay for one attempt against `model` (R-SA-024/047/048/054; pi
/// `buildPiArgs`, `pi-args.ts:83-229`).
///
/// Argv (flags in any order, task prompt last): `--print`, `--mode`, `json`; `--model
/// <apply_thinking_suffix(model, agent.thinking)>` (T4 thinking-suffix); an optional `--tools
/// <comma-list>` — the agent's declared builtins plus any resolved direct-MCP tool names — present
/// only when at least one builtin is declared (pi's `builtinTools.length > 0` gate); the agent's
/// extension threading (`--no-extensions` + `--extension <path>` allowlist when `agent.extensions`
/// is `Some`, else `--extension <path>` for tool-extension/child-only paths with discovery left
/// on); `--no-skills` when the agent does not inherit skills; an optional `--session <path>` (when
/// `opts.fork_context` resolved a session file path); then the task prompt last (via
/// [`ChildSpawnSpec::resolve_task_arg`], R-SA-047's `@<tempfile>` overflow rule).
///
/// Env overlay carries the incremented depth envelope (R-SA-054), the run sentinel, the agent's
/// inherit flags ([`INHERIT_PROJECT_CONTEXT_ENV`]/[`INHERIT_SKILLS_ENV`]), and the raw direct-MCP
/// selector list ([`MCP_DIRECT_TOOLS_ENV`], or the `__none__` sentinel).
///
/// System prompt steering for `output_mode == FileOnly` (R-SA-024's system-prompt half) is
/// applied to `task` BEFORE this function is called — see [`build_task_text`] — since this crate's
/// spawn contract carries the system prompt as part of the composed task/system text handed to
/// the child rather than a separate `--system-prompt` argv flag for subagent runs specifically
/// (mirroring `agent.system_prompt_mode`'s own task-text-composition role, R-SA-024's own
/// wording: "steered at generation time... not merely conveyed via argv").
///
/// # Errors
///
/// Propagates [`ChildSpawnSpec::resolve_task_arg`]'s error (temp-file creation failure for an
/// over-threshold task).
fn build_attempt_spawn_plan(
    agent: &AgentConfig,
    model: &ModelId,
    task_text: &str,
    opts: &RunOptions,
    depth: DepthEnvelope,
    temp_dir: &std::path::Path,
) -> Result<AttemptSpawnPlan, SubagentError> {
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
    // reproduce pi's three destinations for one `tools` list (`pi-args.ts:104-141`): builtins (plus
    // resolved MCP names) to `--tools`, extension paths to `--extension`, and the raw MCP selectors
    // to the `MCP_DIRECT_TOOLS` env.
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

    // pi: `--tools` is emitted ONLY when at least one builtin is declared; a direct-MCP-only agent
    // gets no `--tools` at all (`pi-args.ts:117-123`). The resolved direct-MCP tool names — the
    // whole point of the T4 fix (`mcp:` refs previously flowed to `--tools` literally, an
    // unresolvable name) — are appended after the builtins.
    if !builtin_tools.is_empty() {
        let mut allowlist = builtin_tools.clone();
        if !mcp_direct_tools.is_empty() {
            allowlist.extend(mcp_direct_tools::resolve_mcp_direct_tool_names(
                &mcp_direct_tools,
                &opts.cwd,
            ));
        }
        args.push("--tools".to_string());
        args.push(allowlist.join(","));
    }

    // Extension threading (pi `pi-args.ts:125-137`): `Some(extensions)` turns discovery off
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

    // pi: a subagent that does not inherit skills is spawned with `--no-skills` (`pi-args.ts:139`).
    if !agent.inherit_skills {
        args.push("--no-skills".to_string());
    }

    if let Some(session_path) = &opts.fork_context.session_file_path {
        args.push("--session".to_string());
        args.push(session_path.display().to_string());
    }

    let (task_arg, temp_file) = ChildSpawnSpec::resolve_task_arg(task_text, temp_dir)?;

    let mut env_overlay = crate::spawn::depth::to_env_overlay(&depth);
    // Model-inherit sentinel (R-SA-041) never leaks a global default into the child's own
    // resolution beyond what `--model` above already pins explicitly for this attempt.
    env_overlay.insert("CYRUP_SUBAGENT_RUN".to_string(), "1".to_string());
    // Permission input (1) (port doc §4 / pi `resolveAgentName`, `index.ts:2033-2047`): thread the
    // resolved persona name to the child as [`AGENT_NAME_ENV_VAR`] so its permission companion's
    // `agent` + `projectAgent` policy layers enforce for the named persona. Only a non-empty name is
    // written (an unnamed persona → var absent → child resolves `None`, matching pi's top-level `""`).
    if !agent.name.trim().is_empty() {
        env_overlay.insert(AGENT_NAME_ENV_VAR.to_string(), agent.name.clone());
    }
    // pi `pi-args.ts:199-200`: the child observes the agent's inherit flags as env (`1`/`0`).
    env_overlay.insert(
        INHERIT_PROJECT_CONTEXT_ENV.to_string(),
        if agent.inherit_project_context { "1" } else { "0" }.to_string(),
    );
    env_overlay.insert(
        INHERIT_SKILLS_ENV.to_string(),
        if agent.inherit_skills { "1" } else { "0" }.to_string(),
    );
    // pi `pi-args.ts:216-220`: the raw `mcp:` selectors (comma-joined) or the `__none__` sentinel.
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

    // Intercom child-bridge activation (pi `pi-args.ts:201-214`, `augmentChildEnv`'s intercom half):
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
    // strings match at the broker. `ORCHESTRATOR_SESSION_ID` is deliberately NOT set (pi's own
    // `pi-args.ts` never sets it — the child resolves the supervisor by the presence NAME in
    // `ORCHESTRATOR_TARGET`, which the broker resolves by name; a stable-id env is only wired when a
    // broker-resolvable id exists, which a freshly-connected orchestrator does not have).
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
            temp_files: temp_file.into_iter().collect(),
        },
    })
}

/// Compose the final task text handed to the child: acceptance-contract injection (R-SA-023) then
/// output-path system-prompt steering (R-SA-024's file-only half) then, when
/// `agent.system_prompt_mode == Append`, the agent's own system-prompt body appended after both
/// (mirroring [`crate::discovery::types::SystemPromptMode::Append`]'s documented role: this
/// agent's own frontmatter prose combines with orchestrator-injected scaffolding rather than
/// replacing it). `Replace` mode leaves the agent's own `system_prompt_body` for the spawned
/// child's own system-prompt resolution to apply independently (out of this module's scope — this
/// function only ever touches the TASK text, never the child's actual `--system-prompt`
/// invocation, which this crate does not set at all, letting the child's own agent-persona
/// resolution own that).
///
/// The pre-resolved `skill_injection` (the lazy `<available_skills>` pointer block built ONCE per
/// run by [`run_sync`] via [`crate::discovery::skills::build_skill_injection`]) is appended LAST,
/// after any Append-mode system-prompt body — matching pi, where the skill injection is composed
/// onto the end of the child's system prompt (`execution.ts:949-952`). It is composed into the task
/// text here (rather than a separate `--system-prompt` flag) because this crate carries the
/// orchestrator-injected scaffolding as part of the task/system text handed to the child (see this
/// function's own note above about `agent.system_prompt_mode`). Empty when the agent/step declares
/// no skills, so the common no-skills case appends nothing. This is ORTHOGONAL to
/// `agent.inherit_skills` (the `--no-skills` child flag): an agent that does not inherit skills
/// still receives its explicitly-listed skills through this block.
fn build_task_text(
    agent: &AgentConfig,
    task: &str,
    opts: &RunOptions,
    contract: &AcceptanceContract,
    skill_injection: &str,
) -> String {
    let with_acceptance = inject_acceptance_contract(task, contract);
    let with_output_path = match opts.output_mode {
        OutputMode::FileOnly => {
            let path = opts.output_path.as_deref();
            match build_output_path_system_prompt_instruction(path) {
                Some(instruction) => format!("{with_acceptance}\n\n{instruction}"),
                None => with_acceptance,
            }
        }
        OutputMode::FileAndInline | OutputMode::Inline => with_acceptance,
    };
    let with_system_prompt = match agent.system_prompt_mode {
        SystemPromptMode::Append if !agent.system_prompt_body.is_empty() => {
            format!("{with_output_path}\n\n{}", agent.system_prompt_body)
        }
        SystemPromptMode::Append | SystemPromptMode::Replace => with_output_path,
    };
    if skill_injection.is_empty() {
        with_system_prompt
    } else {
        format!("{with_system_prompt}\n\n{skill_injection}")
    }
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
}

#[async_trait::async_trait]
impl AttemptRunner for SpawnedChildAttemptRunner<'_> {
    type Attempt = AttemptRecord;

    async fn run_attempt(
        &mut self,
        model: &ModelId,
        attempt_note: Option<&str>,
    ) -> (AttemptSignal, Self::Attempt) {
        let mut progress = AgentProgress::default();
        if let Some(note) = attempt_note {
            progress.record_raw_line(note);
        }

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

        let plan = match build_attempt_spawn_plan(
            self.agent,
            model,
            &task_text,
            self.opts,
            child_depth,
            &self.scratch_dir,
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
                    },
                    AttemptRecord {
                        progress,
                        final_output: None,
                        interrupted: false,
                    },
                );
            }
        };

        let jsonl_path = self
            .scratch_dir
            .join(format!("attempt-{}.jsonl", self.attempt_index));
        self.attempt_index += 1;

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
                    },
                    AttemptRecord {
                        progress,
                        final_output: None,
                        interrupted: false,
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
        let outcome = drive_attempt(child, &mut progress, self.opts, deadline_sleep).await;

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
                },
                AttemptRecord {
                    progress,
                    final_output: Some(INTERRUPTED_FINAL_OUTPUT.to_string()),
                    interrupted: true,
                },
            );
        }

        let (raw_exit_code, spawn_error) = match outcome.exit_status {
            Ok(Some(status)) => (status.code(), None),
            Ok(None) => (None, None), // terminated via signal escalation (timeout/cancel)
            Err(err) => (None, Some(err.to_string())),
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
                },
                AttemptRecord {
                    progress,
                    final_output,
                    interrupted: false,
                },
            );
        }

        // --- Exit-0 re-diagnosis (pi `execution.ts:684-790`), in pi's exact order. ---

        // (a) The trailing, still-uncleared assistant `errorMessage` (pi close-handler
        //     `execution.ts:684` sets `result.error = assistantError`).
        let mut error = spawn_error;
        if error.is_none() {
            error = trailing_assistant_error(&progress.all_events);
        }

        // (b) `forcedDrainAfterFinalSuccess` (pi `execution.ts:685`): a child that emitted a CLEAN
        //     terminal stop but had to be force-drained (held stdout open past the grace window)
        //     is coerced to exit 0, not treated as a forced-kill failure.
        let forced_drain_after_final_success =
            outcome.forced_termination && outcome.clean_terminal_stop && error.is_none();

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
                &progress.all_events,
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
        if !success && error.is_none() {
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
            },
            AttemptRecord {
                progress,
                final_output,
                interrupted: false,
            },
        )
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
    /// (`execution.ts:336,356-362`).
    forced_termination: bool,
    /// At least one terminal assistant stop observed on this attempt carried no `errorMessage` —
    /// pi's `cleanTerminalAssistantStopReceived` (`execution.ts:580`), the other half of
    /// `forcedDrainAfterFinalSuccess`.
    clean_terminal_stop: bool,
    /// The child's real exit status once confirmed gone, or a genuine `wait()`/read I/O fault.
    exit_status: std::io::Result<Option<std::process::ExitStatus>>,
    /// R-SA-037: the child's NDJSON stream showed a BLOCKING `contact_supervisor` supervisor-clarify
    /// ask (`need_decision`/`interview`), so the drive loop fired
    /// [`crate::tui::intercom::spawn_clarify`] and this attempt is marked detached (its outcome
    /// bypasses acceptance/completion-guard/truncation, and the fallback ladder does not advance past
    /// it). `false` when no such ask was observed.
    detached: bool,
}

/// pi's `missingStructuredOutput` analog (`execution.ts:783-785`) for the empty-output
/// (cold-start) gate: is the child's structured output ABSENT from its event stream? Returns
/// `true` when NO structured-output schema was requested at all (pi's `!options.structuredOutput`
/// leg, where empty prose is unconditionally an empty-output failure), OR when a schema WAS
/// requested but no structured-output value is present in the transcript
/// ([`StructuredOutcome::Missing`], pi's `!existsSync(outputPath)`). A present-but-invalid value is
/// deliberately NOT "absent" here — this is a pure presence test, exactly like pi's `existsSync`;
/// the value's validity is a separate concern [`run_sync`]'s own R-SA-030 structured-output check
/// diagnoses afterward (pi `readStructuredOutput`, `execution.ts:791`). Reuses [`structured`]'s own
/// public [`resolve_structured_output`] rather than reimplementing extraction, so this crate keeps a
/// single owner of "what counts as a present structured output".
fn structured_output_absent(schema: Option<&serde_json::Value>, events: &[SubagentEvent]) -> bool {
    match schema {
        None => true,
        Some(schema) => matches!(
            resolve_structured_output(Some(schema), events),
            StructuredOutcome::Missing
        ),
    }
}

/// The final-stop grace window (pi `FINAL_STOP_GRACE_MS`, `execution.ts:333`): once a terminal
/// assistant stop is observed, a child that has not exited (released its stdout) within this window
/// is force-drained via [`SpawnedChild::terminate`]'s real SIGINT->SIGTERM->SIGKILL ladder rather
/// than the parent blocking indefinitely on a child that emitted its final answer but never
/// exited. pi's subsequent `HARD_KILL_MS`(3000) SIGKILL step is subsumed by `terminate`'s own
/// SIGTERM->SIGKILL escalation, which this crate routes every forced termination through.
const FINAL_STOP_GRACE_MS: u64 = 1000;

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
) -> DriveOutcome {
    tokio::pin!(deadline_sleep);
    let cancel = opts.cancel.clone();
    let interrupt = opts.interrupt.clone();

    // Armed on the FIRST terminal assistant stop; once the grace window elapses without the child
    // exiting, the child is force-drained. `clean_terminal_stop` accumulates across every terminal
    // stop (pi's `||=`) for `forcedDrainAfterFinalSuccess`.
    let mut final_drain_at: Option<tokio::time::Instant> = None;
    let mut clean_terminal_stop = false;
    // R-SA-037: set once the child's NDJSON shows a blocking `contact_supervisor` ask; the ask is
    // surfaced via `spawn_clarify` exactly once (the guard below), and this flag then rides out to
    // the attempt's `detached` outcome (bypassing acceptance; the ladder does not advance past it).
    let mut detached_seen = false;

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
                };
            }
            () = interrupt.cancelled() => {
                let outcome = child.terminate(&cancel).await;
                return DriveOutcome {
                    timed_out: false,
                    interrupted: true,
                    forced_termination: false,
                    clean_terminal_stop,
                    exit_status: outcome.map(|o| Some(o.status)),
                    detached: detached_seen,
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
                };
            }
            next = child.next_event() => {
                match next {
                    Some(Ok(line)) => {
                        progress.record_raw_line(&line.raw);
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
                            // Final-stop grace-drain (pi `startFinalDrain`, execution.ts:350-367):
                            // open the grace window on the FIRST terminal assistant stop and track
                            // whether ANY terminal stop was clean (no errorMessage) for
                            // `forcedDrainAfterFinalSuccess`.
                            if is_terminal_assistant_stop(&event) {
                                clean_terminal_stop =
                                    clean_terminal_stop || !message_end_has_error_message(&event);
                                if final_drain_at.is_none() {
                                    final_drain_at = Some(
                                        tokio::time::Instant::now()
                                            + Duration::from_millis(FINAL_STOP_GRACE_MS),
                                    );
                                }
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
                            progress.record_event(event);
                        }
                    }
                    Some(Err(_)) | None => {
                        // Stdout EOF (child exited/closed stdout) or a genuine read fault — either
                        // way, stop reading and wait for the real exit status below.
                        break;
                    }
                }
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
                };
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
            }
        }
        Err(err) => DriveOutcome {
            timed_out: false,
            interrupted: false,
            forced_termination: false,
            clean_terminal_stop,
            exit_status: Err(err),
            detached: detached_seen,
        },
    }
}

// ================================================================================================
// run_sync: the model-fallback attempt loop, wired end to end (arch-SA §6.3.2)
// ================================================================================================

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
/// 2. Resolve the effective acceptance contract (explicit `opts.acceptance`, or
///    [`AcceptanceContract::heuristic_default`], R-SA-023).
/// 3. R-SA-038: build the model-fallback candidate ladder.
/// 4. Drive [`fallback::run_fallback_ladder`] against a [`SpawnedChildAttemptRunner`] — every
///    candidate model gets a FRESH real child OS process (R-SA-039); R-SA-036 (timeout)/R-SA-037
///    (detach) both terminate the ladder outright without advancing, exactly as
///    `run_fallback_ladder` itself already enforces (this module supplies the signal, not the
///    ladder-control logic, which stays [`fallback`]'s sole responsibility).
/// 5. R-SA-030: structured-output extraction + parent-side JSON-Schema re-validation, via
///    [`structured::resolve_structured_output`] (arch-SA §12 item 13's resolved crate choice,
///    `jsonschema`). Only evaluated when the run is otherwise clean (exit 0, not detached/
///    interrupted/timed-out) — mirrors R-SA-032/033's own "don't re-diagnose an already-failed
///    attempt" gate. If `opts.structured_output_schema` is `None`, this step is a no-op
///    (`SingleResult::structured_output` stays `None`). If a schema IS declared: an extracted value
///    that validates populates `SingleResult::structured_output`; an extracted value that fails
///    validation, or no value at all when no plain-text fallback was produced either, forces
///    `exit_code = 1` with a validation-error `error` message — never silently downgraded, per
///    R-SA-030's "MUST also fail the run" text.
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
            error: Some(err.to_string()),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // Step 1 (R-SA-025): fail fast before any subprocess spawns.
    if let Some(err) = validate_file_only_requires_path(opts.output_mode, opts.output_path.as_deref())
    {
        return SingleResult {
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
            error: Some(err.to_string()),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // Step 2 (R-SA-023): resolve the effective acceptance contract.
    let contract = opts
        .acceptance
        .clone()
        .unwrap_or_else(|| AcceptanceContract::heuristic_default(&agent.name, task));

    // Step 3 (R-SA-038).
    let candidates = build_model_candidates(
        &opts.model_override,
        agent.model.as_ref(),
        &agent.fallback_models,
        &opts.available_models,
    );

    if candidates.is_empty() {
        return SingleResult {
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
            error: Some(
                "no candidate model available for this subagent run (empty fallback ladder)"
                    .to_string(),
            ),
            tool_calls: Vec::new(),
            output_truncated: false,
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
                error: Some(format!(
                    "Skills not found: {}",
                    crate::discovery::skills::SUBAGENT_ORCHESTRATION_SKILL
                )),
                tool_calls: Vec::new(),
                output_truncated: false,
            };
        }
        crate::discovery::skills::build_skill_injection(&resolution.resolved)
    };

    // R-SA-031: snapshot the output file's state ONCE, before the ladder starts (a task's
    // `output_path` is stable across fallback attempts — see `SpawnedChildAttemptRunner::
    // snapshot_output_file`'s own doc note for why re-snapshotting per attempt is unnecessary).
    let output_snapshot = snapshot_output_file(opts.output_path.as_deref());

    let scratch_dir = opts.cwd.join(".cyrup-subagent-scratch");
    if let Err(err) = std::fs::create_dir_all(&scratch_dir) {
        return SingleResult {
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
            error: Some(format!("failed to prepare subagent scratch directory: {err}")),
            tool_calls: Vec::new(),
            output_truncated: false,
        };
    }

    // Step 4: drive the fallback ladder.
    let mut runner = SpawnedChildAttemptRunner {
        agent,
        task,
        opts,
        contract: &contract,
        scratch_dir,
        skill_injection,
        attempt_index: 0,
    };
    let outcome = run_fallback_ladder(&candidates, &mut runner).await;

    let winning_model = outcome.attempted_models.last().cloned();
    let last_signal = outcome.last_signal;
    let last_attempt = outcome.last_attempt;

    let (timed_out, interrupted, detached, mut exit_code, mut error, mut final_output) =
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
                signal.exit_code.unwrap_or(if signal.success { 0 } else { 1 }),
                signal.error.clone(),
                record.final_output.clone(),
            ),
            _ => (
                false,
                false,
                false,
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
    if timed_out {
        let timeout_message = format_timeout_message(opts.timeout_ms.unwrap_or(0));
        let partial = final_output.clone().unwrap_or_default();
        final_output = Some(if partial.trim().is_empty() {
            timeout_message
        } else {
            format!("{timeout_message}\n\nPartial output before timeout:\n{partial}")
        });
    }

    // R-SA-031: file-only/output-path handoff, once, against the aggregate captured output. Tracks
    // the concrete saved path (`Some` only when the file was actually written — by the child, or by
    // the orchestrator persisting its own captured output), which the saved-output reference message
    // below (pi `finalizeSingleOutput`, `single-output.ts:156-180`) is gated on. pi resolves the
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

    let progress = last_attempt.map(|record| record.progress).unwrap_or_default();

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
        match resolve_structured_output(opts.structured_output_schema.as_ref(), &progress.all_events)
        {
            StructuredOutcome::NotRequested => None,
            StructuredOutcome::Valid(value) => Some(value),
            StructuredOutcome::Missing => {
                // pi `readStructuredOutput` (structured-output.ts:55-58, execution.ts:791-805): a
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
        let ledger = evaluate_acceptance(
            &contract,
            post_guard_gate,
            final_output.as_deref(),
            guard_result,
            &opts.cwd,
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

    // Saved-output reference (pi `finalizeSingleOutput`, `single-output.ts:156-180`): once a clean
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

    // Step 10 (R-SA-043): compaction. `SingleResult` itself is the compacted shape; the
    // `include_progress` flag governs only a LIVE snapshot this function never constructs (no
    // live-callback path exists in this phase), so it has no further effect here beyond being
    // threaded through `RunOptions` for a future phase's live-progress plumbing to read.
    let _ = opts.include_progress;

    SingleResult {
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
        error,
        tool_calls: progress.summarized_tool_calls(),
        output_truncated,
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
        name: agent.name.clone(),
        local_name: agent.name.clone(),
        package_name: None,
        description: String::new(),
        tools: agent.tools.clone(),
        extensions: None,
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::AcceptanceStatus;

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
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
        }
    }

    fn base_opts(cwd: &std::path::Path, available: &[&str]) -> RunOptions {
        RunOptions {
            cwd: cwd.to_path_buf(),
            deadline_at: None,
            timeout_ms: None,
            output_path: None,
            output_mode: OutputMode::Inline,
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
        }
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
            progress.record_raw_line(&format!("line-{i}"));
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), &text, &opts, depth, dir.path())
            .expect("plan builds");
        assert!(plan.spec.build_argv().contains(&"--no-skills".to_string()));
    }

    #[test]
    fn build_task_text_appends_system_prompt_body_in_append_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.system_prompt_mode = SystemPromptMode::Append;
        agent.system_prompt_body = "You are a delegate persona.".to_string();
        let opts = base_opts(dir.path(), &["m1"]);
        let contract = AcceptanceContract::explicit(AcceptanceStatus::NotRequired, vec![]);

        let text = build_task_text(&agent, "do the thing", &opts, &contract, "");
        assert!(text.contains("You are a delegate persona."));
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
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

    #[test]
    fn build_attempt_spawn_plan_omits_the_child_intercom_bridge_env_without_an_orchestrator_target() {
        // A headless / no-intercom run (no orchestrator target) must leave the child un-bridged — the
        // clean no-intercom path, so a plain run spawns no broker-participant child.
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = sample_agent_config("m1", &[]);
        let opts = base_opts(dir.path(), &["m1"]); // orchestrator_intercom_target / run_id both None
        let depth = DepthEnvelope { current_depth: 0, max_depth: 5 };
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
            .expect("plan builds");
        let env = &plan.spec.env_overlay;
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_ORCHESTRATOR_TARGET));
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_INTERCOM_SESSION_NAME));
        assert!(!env.contains_key(crate::spawn::intercom_target::ENV_CHILD_AGENT));
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
            .expect("plan builds");
        let argv = plan.spec.build_argv();
        let idx = argv.iter().position(|a| a == "--session").expect("--session present");
        assert!(argv[idx + 1].contains("parent-branch.jsonl"));
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, depth, dir.path())
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
        );
        assert!(
            available_models.contains(&inherited),
            "the inherited model must be added to available_models so the allowlist filter keeps it"
        );

        let candidates = build_model_candidates(
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
        let plan = build_attempt_spawn_plan(&agent, &candidates[0], "task", &opts, depth, dir.path())
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
            build_attempt_spawn_plan(&inheriting, &ModelId::from("m1"), "t", &opts, depth, dir.path())
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path())
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path())
            .expect("plan builds");
        // An empty persona name writes NO var (child resolves `None` — pi's top-level `""`).
        assert_eq!(plan.spec.env_overlay.get(AGENT_NAME_ENV_VAR), None);
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path())
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
        let plan = build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "t", &opts, depth, dir.path())
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
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, child_depth, dir.path())
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
            build_attempt_spawn_plan(&agent, &ModelId::from("m1"), "task", &opts, child_depth, dir.path())
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
