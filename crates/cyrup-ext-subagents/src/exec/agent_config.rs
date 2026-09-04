//! The static, execution-ready "what to run and how" input surface (arch-SA §3.4):
//! [`AgentConfig`], [`ResolvedAgentPersona`], [`resolve_step_agent_config`], [`RunOptions`], and
//! [`LiveEventSink`]. Split out of `exec/mod.rs`'s own "AgentConfig / RunOptions / SingleResult"
//! section; [`crate::exec::run_result::SingleResult`] is that section's output-contract half.

use std::path::PathBuf;
use std::time::Instant;

use cyrup_core::{CancelToken, ModelId, ProviderId};

use crate::discovery::types::{
    AgentDefinition, AgentReadScope, OutputMode, OutputSpec, SystemPromptMode, ToolRef,
};
use crate::exec::acceptance::AcceptanceContract;
use crate::exec::fallback::{
    ModelOverride,
};
use crate::exec::output::OutputCap;
use crate::fork_context::{ContextMode, ForkContext};
use crate::spawn::depth::DepthEnvelope;


// ================================================================================================
// AgentConfig / RunOptions / SingleResult (arch-SA §3.4)
// ================================================================================================

/// The resolved, execution-ready subset of an [`AgentDefinition`] this module's foreground
/// executor needs (arch-SA §3.4). Deliberately narrower than the full `AgentDefinition` — this
/// type carries only what `run_sync` itself branches on, not discovery/management metadata
/// (`source`, `file_path`, `present_fields`, …) that has no bearing on one execution.
#[derive(Debug, Clone)]
pub struct AgentConfig {
    /// The agent's local (unqualified) name — feeds [`crate::exec::completion_guard::expects_implementation_mutation`]'s
    /// `agent` classification input and [`crate::exec::acceptance::AcceptanceContract::heuristic_default`].
    pub name: String,
    pub model: Option<ModelId>,
    pub fallback_models: Vec<ModelId>,
    /// The agent's frontmatter reasoning level (func-SA §4.1 `AgentDefinition::thinking`) as pi's
    /// OPEN string, applied to the child's `--model` argument as a `:<value>` suffix at spawn time via
    /// [`crate::exec::apply_thinking_suffix`] (pi `applyThinkingSuffix`,
    /// `runs/shared/pi-args.ts:238-252` @v0.57.0) — `None` leaves the
    /// per-attempt model id untouched. Carrying the raw string (rather than a closed on-only enum)
    /// means an explicit `Some("off")` now reaches the child as `:off`, exactly like pi, instead of
    /// being conflated with `None` and dropped.
    pub thinking: Option<String>,
    pub system_prompt_mode: SystemPromptMode,
    pub system_prompt_body: String,
    pub tools: Option<Vec<ToolRef>>,
    /// SUBA-092 — the agent's `excludeTools` (pi `ResolvePiLaunchToolPlanInput.excludeTools`,
    /// `runs/shared/pi-args.ts:301` @v0.64.0), flattened from
    /// [`AgentDefinition::exclude_tools`]'s `Option` exactly as pi's `input.excludeTools ?? []`
    /// (`:502`) does. Subtracted from the child's declared builtin set and direct-MCP names at
    /// spawn time, or emitted as `--exclude-tools` when nothing pins an allowlist.
    pub exclude_tools: Vec<String>,
    /// SUBA-092 — pi `ResolvePiLaunchToolPlanInput.allowNestedSubagents` (`pi-args.ts:302`): the
    /// independent nested-delegation grant folded into `fanoutAuthorized` (`:505-509`). Only
    /// `Some(true)` counts (`input.allowNestedSubagents === true`).
    pub allow_nested_subagents: Option<bool>,
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
    /// [`crate::exec::output::OutputCap`] directly (the type `exec/output.rs` already defines and tests)
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
    /// SUBA-074 — the agent's declared execution runner, carried so the pre-ladder refusal in
    /// [`crate::exec::run_sync`] fires identically on every dispatch path. `None` and
    /// `Some(AgentRunnerConfig::Pi)` both mean the native child this crate spawns.
    pub runner: Option<crate::runner::AgentRunnerConfig>,
    /// SUBA-082 — the agent's declared acceptance role
    /// ([`AgentDefinition::acceptance_role`]), read by `run_sync`'s acceptance resolution
    /// (`resolve_run_acceptance`) exactly where pi reads `agent.acceptanceRole`
    /// (`runs/foreground/execution.ts:1834` @v0.64.0). `None` = no role declared, so the
    /// agent-name alternations decide.
    pub acceptance_role: Option<crate::exec::acceptance::model::AcceptanceRole>,
    /// SUBA-082 — the agent's validated `acceptance:` launch default
    /// ([`AgentDefinition::default_acceptance`]). Carried for the same reason `runner` is
    /// (so the projection is a faithful copy of the definition), but NOT consulted by
    /// `run_sync`: pi applies it only to a SINGLE-agent launch's params
    /// (`applySingleAgentLaunchDefaults`, `subagent-executor.ts:2690-2692` @v0.64.0), which this
    /// crate does in `extension/tool/routing.rs::route_single` before `RunOptions` exist.
    pub default_acceptance: Option<serde_json::Value>,
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
            exclude_tools: agent.exclude_tools.clone().unwrap_or_default(),
            allow_nested_subagents: agent.allow_nested_subagents,
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
            runner: agent.runner.clone(),
            acceptance_role: agent.acceptance_role,
            default_acceptance: agent.default_acceptance.clone(),
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
    /// SUBA-092 — the agent's own `excludeTools`, carried so a chain/parallel/background step
    /// subtracts the SAME tools the single-run path does (pi threads `agentConfig.excludeTools`
    /// into every launch, `runs/background/async-execution.ts:948,1011,1741` @v0.64.0).
    /// `#[serde(default)]` keeps the runner-config hand-off backward compatible.
    #[serde(default)]
    pub exclude_tools: Vec<String>,
    /// SUBA-092 — the agent's own `allowNestedSubagents` grant, carried for the same reason
    /// (`async-execution.ts:949,1012,1742`). `#[serde(default)]` keeps the hand-off compatible.
    #[serde(default)]
    pub allow_nested_subagents: Option<bool>,
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
    /// SUBA-074 — the agent's declared execution runner, carried across the hop-2 process
    /// boundary so a chain/parallel/background step refuses an unsupported runner exactly as the
    /// single-run path does. `#[serde(default)]` keeps an older on-disk config deserializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<crate::runner::AgentRunnerConfig>,
    /// SUBA-082 — the agent's declared acceptance role, carried across the hop-2 process
    /// boundary so a chain/parallel/background step infers its acceptance level from the SAME
    /// role the single-run path does (pi threads `acceptanceRole: a.acceptanceRole` into every
    /// background launch, `runs/background/async-execution.ts:978,1036,1044,1122,1130,1768,1799`
    /// @v0.64.0). `#[serde(default)]` keeps an older on-disk config deserializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_role: Option<crate::exec::acceptance::model::AcceptanceRole>,
    /// SUBA-082 — the agent's validated `acceptance:` launch default, carried so the persona is a
    /// faithful copy of the definition (see [`AgentConfig::default_acceptance`] for why no
    /// chain/parallel/background step ever APPLIES it). `#[serde(default)]` keeps an older
    /// on-disk config deserializable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_acceptance: Option<serde_json::Value>,
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
            exclude_tools: agent.exclude_tools.clone().unwrap_or_default(),
            allow_nested_subagents: agent.allow_nested_subagents,
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
            runner: agent.runner.clone(),
            acceptance_role: agent.acceptance_role,
            default_acceptance: agent.default_acceptance.clone(),
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
            exclude_tools: self.exclude_tools.clone(),
            allow_nested_subagents: self.allow_nested_subagents,
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
            runner: self.runner.clone(),
            acceptance_role: self.acceptance_role,
            default_acceptance: self.default_acceptance.clone(),
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
/// stable, documented home even though the type itself is [`crate::exec::fallback`]'s (one canonical owner,
/// consumed by this module rather than redefined).
pub use crate::exec::fallback::ModelOverride as RunModelOverride;

/// Every per-call parameter [`crate::exec::run_sync`] needs beyond the resolved [`AgentConfig`] and task text
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
    /// `deadline_at` is what [`crate::exec::run_sync`] actually races the child against. Normally the orchestrator
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
    /// is upstream's `effectiveCwd`, and it happens once in [`crate::exec::build_task_text`] through
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
    /// cross-session default inside [`crate::exec::run_sync`]; a caller wanting that global-default behavior
    /// resolves it explicitly before constructing this struct.
    pub model_override: ModelOverride,
    pub preferred_provider: Option<ProviderId>,
    pub available_models: Vec<ModelId>,
    /// Hard-abort cancellation, raced independently of `interrupt` (arch-SA §5.1).
    pub cancel: CancelToken,
    /// Soft, per-run interrupt — distinct downstream consequences from `cancel`/timeout
    /// (R-SA-084 vs. R-SA-036); this module treats an interrupt firing identically to a timeout
    /// for ladder-termination purposes (both stop the fallback ladder outright) but records it
    /// under its own `interrupted` flag on [`crate::exec::SingleResult`] rather than conflating it with
    /// `timed_out`.
    pub interrupt: CancelToken,
    /// pi `options.share` (`execution.ts:1027`, the tool's SINGLE-mode `share` param). Its ONE
    /// effect at this port's baseline is pi's `sessionEnabled` term (`execution.ts:1412`): a
    /// `Some(true)` keeps the child's session store ON even without an explicit
    /// [`Self::session_dir`], so `--no-session` is not emitted (see
    /// [`crate::exec::build_attempt_spawn_plan`]). pi v0.34.0 has no gist upload of its own — the tool schema's
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
    /// `:2679` for PARALLEL) — makes [`crate::exec::run_sync`] assemble this run's own
    /// [`crate::tui::events::LiveProgressSnapshot`] onto [`crate::exec::SingleResult::progress`]. `None` and
    /// `Some(false)` leave that field `None`, which `skip_serializing_if` then omits from the wire
    /// entirely: the returned/persisted result is byte-for-byte what it was before the field
    /// existed.
    ///
    /// It never affects any OTHER field of [`crate::exec::SingleResult`] — the messages/transcript compaction
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
    /// SUBA-078 — the effective `subagents.maxThinking` ceiling for this run (pi's
    /// `options.thinkingCeiling`, `runs/foreground/execution.ts:382`), already intersected with
    /// whatever this process itself inherited. `None` = no ceiling, so the bound is off.
    ///
    /// A REFUSAL bound, not a clamp: see [`crate::error::SubagentError::ThinkingCeilingViolation`].
    pub thinking_ceiling: Option<String>,
    /// Explicit acceptance-contract override for this task (func-SA §4.2 `acceptance`); `None`
    /// defers to [`AcceptanceContract::heuristic_default`] (R-SA-023).
    pub acceptance: Option<AcceptanceContract>,
    /// Resolved fork-context for this task, if any — normally produced by [`crate::exec::plan_batch`] ahead of
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
    /// [`crate::exec::PARENT_SESSION_ENV_VAR`] (proposed R-SA-P1, port doc §4 P-4). `Some(id)` is the EXPLICIT
    /// value — the launching session's own id from P-2 [`cyrup_ext::host::HostServices::session_id`]
    /// (captured at the root orchestrator's `SessionStart`). `None` (the default, and the detached
    /// hop-2 runner) defers to the INHERITED value already in this process's own
    /// `CYRUP_SUBAGENT_PARENT_SESSION` env; absent both, the anchor is omitted (empty). The
    /// permission companion reads it (`forwarding/mod.rs`) to address the parent's ask-forwarding
    /// inbox; this crate only ever WRITES it.
    pub parent_session_id: Option<String>,
    /// Optional clarify/ask dispatch context (R-SA-037/119/120). When `Some`, [`crate::exec::drive_attempt::drive_attempt`]'s
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
    /// records events on [`crate::exec::SingleResult::control_events`]; it just delivers none of them live.
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
    /// SUBA-073 — pi `resolvePermissionRules(ctx.config?.permissions, agentConfig.permissions)`
    /// (`async-execution.ts`, `api/preflight.ts`): the fully-merged permission policy (global
    /// `config.permissions` + this agent's own frontmatter, agent winning on conflict, `allow`
    /// entries stripped) this run's child receives. Resolved once by whichever call site has both
    /// the live extension config and the resolved agent in hand — [`crate::exec::permissions::resolve_permission_rules`]
    /// — and threaded here exactly like [`Self::turn_budget`], for the same reason: it has a
    /// global-config rung that a per-agent-persona projection ([`ResolvedAgentPersona`]) cannot
    /// re-derive on its own after crossing the hop-2 detached-runner process boundary.
    ///
    /// `None` means no policy at all — the pre-existing, permanently-unreachable state this item
    /// exists to make reachable.
    pub permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
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
    /// The `cyrup` binary this run's child re-execs, injected rather than resolved from the
    /// process environment. `None` means "nothing beyond what the environment says", so
    /// [`crate::spawn::resolve_spawn_command`] answers and R-SA-045's three-tier priority is
    /// unchanged — exactly the shape `thinking_ceiling` above already uses.
    ///
    /// This crate is `#![forbid(unsafe_code)]` and the 2024 edition made `std::env::set_var`
    /// `unsafe`, so nothing here may move the process environment to point a run at a different
    /// binary. Supplying the command per-run is how a caller (an integration test wiring a
    /// scripted fixture, say) redirects ONE run without disturbing any other run in the process.
    /// [`crate::exec::spawn_plan`] also copies an injected binary into the child's
    /// `env_overlay`, so a grandchild that resolves its own command from its inherited
    /// environment still finds it.
    pub spawn_command: Option<crate::spawn::SpawnCommand>,

    /// Extra entries for the CHILD's environment — the foreground twin of
    /// `background::parent_anchor::detached_runner_env_overlay_with`.
    ///
    /// This is R2 tier 2 for the in-process spawn path: where a variable IS the mechanism (a child
    /// discovering a broker socket through `CYRUP_CODING_AGENT_DIR`, say), it belongs on the
    /// child's `Command`, never on this process. Layered onto `SpawnSpec::env_overlay`.
    ///
    /// Applied FIRST, so the crate's own identity, depth and child-role entries overwrite it. A
    /// caller may add to the child's environment; it may not rewrite the invariants that decide
    /// what the child is allowed to do.
    ///
    /// # Zero production callers, and why it keeps its place
    ///
    /// Every production spawn passes an empty map; `spawn_plan` layers the real values on top. It
    /// stays because it is R2 tier 2 in its purest form: a value that must reach a CHILD goes on
    /// the CHILD's `Command`, never on this process's environment — which is the one mechanism
    /// `std::env`'s own documentation endorses for a multi-threaded program. A test whose subject
    /// is what a child inherits has no other sound way to say it.
    ///
    /// It costs production no parameter and defaults to empty.
    pub child_env: std::collections::HashMap<String, String>,
}

/// A live per-line sink installed via [`RunOptions::live_events`]: [`crate::exec::run_sync`]'s per-attempt driver
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
/// canonical owner: [`crate::exec::fallback::ModelAttempt`]).
pub use crate::exec::fallback::ModelAttempt as RunModelAttempt;

/// `{text, expandedText}` tool-call preview (R-SA-043), re-exported under `exec`'s own namespace so
/// callers of `run_sync` (and consumers of [`crate::exec::SingleResult::tool_calls`]) never need to import
/// `exec::tool_call_summary` directly (one canonical owner: [`crate::exec::tool_call_summary::ToolCallSummary`]).
pub use crate::exec::tool_call_summary::ToolCallSummary;

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;


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
            exclude_tools: vec!["bash".to_string()],
            allow_nested_subagents: Some(true),
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
            runner: Some(crate::runner::AgentRunnerConfig::ExternalCli(
                crate::runner::ExternalCliRunner {
                    adapter: Some("claude-code".to_string()),
                    command: "claude".to_string(),
                    args: Vec::new(),
                    prompt_delivery_stdin: false,
                    capabilities: None,
                },
            )),
            acceptance_role: None,
            default_acceptance: None,
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
            exclude_tools: vec!["bash".to_string()],
            allow_nested_subagents: Some(true),
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
            runner: Some(crate::runner::AgentRunnerConfig::ExternalCli(
                crate::runner::ExternalCliRunner {
                    adapter: Some("claude-code".to_string()),
                    command: "claude".to_string(),
                    args: Vec::new(),
                    prompt_delivery_stdin: false,
                    capabilities: None,
                },
            )),
            acceptance_role: None,
            default_acceptance: None,
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
        assert_eq!(cfg.runner, persona.runner);
        assert!(cfg.inherit_project_context);
        assert!(!cfg.inherit_skills);
        // The persona's own `skills` list reaches the execution config so a chain/parallel/background
        // step injects the SAME `<available_skills>` block the single-run path does.
        assert_eq!(cfg.skills, vec!["accessibility".to_string()]);
        // The depth is the caller-stamped live envelope, not a plan-time value.
        assert_eq!(cfg.depth, live_depth);
    }

}
