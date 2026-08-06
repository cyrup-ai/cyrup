//! `NativeExtension` impl: init/on_event/execute_command (arch-SA §3.2/§6.8).
//!
//! This is the crate's final integration point: it wires every already-implemented subsystem —
//! [`crate::discovery`] (resolve agent personas), [`crate::exec`]/[`crate::spawn`] (foreground OS-
//! subprocess run), [`crate::background`] (detached second-hop async run), [`crate::tui`]
//! (progress/notice folding), [`crate::registration`] (config layering, doctor, cost, profiles) —
//! into the one [`NativeExtension`] the `cyrup` binary registers (`crates/cyrup/src/main.rs`'s
//! three `with_native_extension` call sites).
//!
//! # The mandated mechanism (restated once at the seam this file owns)
//!
//! Every subagent execution this file drives is a genuine OS subprocess: the `subagent` tool's
//! foreground shape dispatches to [`crate::exec::run_sync`], which spawns a REAL child via
//! [`crate::spawn::SpawnedChild::spawn`]; the background shape dispatches to
//! [`crate::background::spawn_detached::spawn_detached_runner`], a genuine SECOND, detached OS
//! process hop that itself re-execs `cyrup __subagent-runner --config <path>`
//! (`crates/cyrup/src/subagent_runner_cmd.rs`), which in turn spawns further children through the
//! identical spawn boundary. There is no in-process nested agent turn loop anywhere in this file,
//! no in-process event-relay standing in for a child's own execution, and no extension-host
//! session-access seam beyond the one, narrow, sanctioned [`crate::fork_context`] dependency on
//! `cyrup-session` (§6.6). This file adds no new such seam.
//!
//! # Fork-context without a live session-manager handle (an honest, scoped limitation)
//!
//! [`cyrup_ext::native::NativeExtension`] instances are constructed and `init`-ed BEFORE the owning
//! session's `SessionManager` exists (`crates/cyrup-session-svc/src/builder.rs`'s `build()`
//! constructs `manager` at step 2b, well after `for ext in self.native_extensions { host
//! .load_native(ext).await?; }` would already have run if extensions were loaded that early —
//! in fact native extensions are loaded even later, at step 4b, but still driven by a
//! caller-supplied `Arc<dyn NativeExtension>` that was itself constructed by the BINARY before
//! `SessionBuilder::build()` is ever called). Per arch-SA §12 item 6/10 (confirmed against current
//! source, not assumed): no wiring exists today to inject an `AgentSessionServices`/live
//! `SessionManager` handle into [`InitApi`]/[`HostCtx`] at construction or dispatch time, and
//! building that new cross-crate seam is explicitly out of this integration task's scope (the task
//! brief is unambiguous that this crate's ONLY sanctioned session access is the direct,
//! already-built [`crate::fork_context::ForkContextResolver`] dependency on `cyrup-session` — never
//! a new extension-host session-access seam).
//!
//! This file resolves that gap the same way [`crate::fork_context`] itself is documented to work:
//! a THROWAWAY `SessionManager` handle, opened fresh per dispatch call from [`HostCtx::cwd`] via
//! [`cyrup_session::SessionManager::continue_recent`] (the identical primitive
//! `cyrup-session-svc`'s own builder uses for `SessionTarget::Continue`), scoped under this
//! extension's own `sessions` subdirectory of the resolved agent dir. This is NOT a live,
//! shared-with-the-orchestrator manager — it never mutates any in-memory state the running session
//! itself holds (R-SA-139/DI-SA-6 is satisfied trivially: there is no live in-memory state to
//! mutate, only a fresh on-disk read). If no persisted session exists yet at `cwd`,
//! `continue_recent` synthesizes an in-memory session with no leaf, and
//! [`crate::fork_context::ForkContextResolver::resolve`] correctly fails hard
//! (`ForkRequiresLeaf`/`ForkRequiresPersistedParent`) rather than silently downgrading to
//! `Fresh` — preserving DI-SA-2 exactly.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use cyrup_core::{CancelToken, ExtensionId, ModelId, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, HookOutcome, HostEvent};
use tokio::sync::Mutex as AsyncMutex;

use crate::background::atomic::write_atomic_json;
use crate::background::control::{self, AppendOutcome, InterruptOutcome, ResumeOutcome};
use crate::background::runner_main::ExecSingleStepExecutor;
use crate::background::spawn_detached::spawn_detached_runner;
use crate::background::tracker::JobTracker;
use crate::background::{run_status, RunId, RunMode, RunPaths, RunState};
use crate::discovery::types::{
    AgentDefinition, AgentModelSourceInfo, AgentReadScope, AgentSource, ChainStepConfig,
    LayeredOverrideSettings, OverrideScope,
};
use crate::discovery::{discover_agents, AgentDiscoveryConfig};
use crate::error::SubagentError;
use crate::exec::fallback::resolve_model_inheritance;
use crate::exec::model_scope::ModelScopeConfig;
use crate::exec::{AgentConfig, ResolvedAgentPersona, RunOptions, SingleResult};
use crate::fork_context::{
    resolve_effective_context, ContextMode, ForkContext, ForkContextResolver,
};
use crate::registration::doctor::{build_doctor_report, DoctorReportInput};
use crate::registration::slash_commands::{self, SlashCommandName, SLASH_COMMANDS};
use crate::registration::{
    CompanionSuggestionsConfig, CompanionSuggestionsSetting, SubagentExtensionConfig,
};
use crate::spawn::chain_graph::{
    walk_chain, ChainRunContext, GroupStepResult, OutputRegistry, ParallelGroupSpec, RunnerStep,
    SingleStepExecutor, SingleStepSpec, StepResult,
};
use crate::spawn::depth::resolve_effective_depth;
use crate::spawn::parallel::{DispatchGuard, GlobalConcurrencyLimit};

/// The literal, stable extension id every registration/log/doctor surface refers to.
const EXTENSION_ID: &str = "subagents";

/// The single LLM-visible tool name (R-SA-128). Also the name a persona lists in its own `tools:`
/// to be granted nested delegation — pi's `fanoutAuthorized = declaredBuiltinTools.includes(
/// "subagent")` (`runs/shared/pi-args.ts:194`), read by [`crate::exec::build_attempt_spawn_plan`].
pub(crate) const TOOL_NAME: &str = "subagent";

// =================================================================================================
// The SubagentExecutor: the ONE shared code path the tool and every slash command route through
// (R-SA-130). Holds no per-call state; every method takes what it needs as parameters.
// =================================================================================================

/// The shared executor both the `subagent` tool and every slash-command handler dispatch through
/// (R-SA-130: "single execution code path... both call sites are ordinary function calls into the
/// same executor type; no event-bus round-trip is required"). Owns the extension-wide, rarely-
/// mutated state ([`SubagentExtensionConfig`], the background [`JobTracker`]) that both entry
/// points need.
pub struct SubagentExecutor {
    config: Arc<AsyncMutex<SubagentExtensionConfig>>,
    tracker: Arc<JobTracker>,
    /// An EXPLICITLY-injected completion sink (a test's capturing sink, or a caller wiring its own
    /// turn-injection channel). `None` — the production default — means "derive the effective sink
    /// at install time": a live [`HostServicesCompletionSink`] when the P-1 `host_services` slot is
    /// bound (R-SA-101, the real turn-injecting sink), else the graceful-degradation
    /// [`crate::background::watch::LoggingCompletionSink`] (log + delete). Set via
    /// [`SubagentExecutor::with_completion_sink`].
    completion_sink_override: Option<Arc<dyn crate::background::watch::CompletionSink>>,
    /// The live [`crate::background::watch::CompletionWatcherHandle`] for the current session's
    /// `ResultsDir`, installed on `SessionStart` ([`SubagentExecutor::install_completion_watcher`])
    /// and retained here so the watch stays live for the session's lifetime (dropping it stops the
    /// watch). Re-installing replaces (and thereby tears down) any prior handle.
    completion_watcher: AsyncMutex<Option<crate::background::watch::CompletionWatcherHandle>>,
    /// The late-bound live capability backend (P-1, reconciliation §2 item 1). Captured by
    /// [`SubagentsExtension::set_host_services`] (which the builder calls via
    /// `load_native_with_services` BEFORE `init`), so a background task / the `SessionStart` handler
    /// / the fork-context resolver can reach the live session id/file + `inject_message` OUTSIDE any
    /// `HostCtx`. `None` (default host / SDK-embedder / headless) ⇒ every consumer degrades to its
    /// documented no-host fallback (heuristic fork-context, stderr logging sink, empty anchor).
    host_services: Arc<OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
    /// The canonical parent-session anchor (`CYRUP_SUBAGENT_PARENT_SESSION`, proposed R-SA-P1),
    /// captured ONCE from [`cyrup_ext::host::HostServices::session_id`] at the root orchestrator's
    /// `SessionStart` (depth 0). Injected into every child's spawn env overlay so the permission
    /// companion's child→parent ask-forwarding spool can address this session's inbox (port doc §4
    /// P-4). Empty/unset at `DEPTH>0` (a child never captures its own) — the spawn-site resolution
    /// then falls back to the inherited env value (explicit → inherited → empty).
    /// A plain `Mutex` (not `OnceLock`) because pi's own anchor is process-`env`-backed and
    /// therefore clearable (`delete process.env[SUBAGENT_PARENT_SESSION_ENV]`,
    /// `extension/index.ts:645`) at `session_shutdown` — [`Self::clear_parent_session_anchor`]
    /// mirrors that exactly, which a write-once `OnceLock` could not support.
    root_parent_session: Arc<std::sync::Mutex<Option<String>>>,
    /// The root orchestrator session's own NAME (`HostServices::session_name`), captured ONCE
    /// alongside [`Self::root_parent_session`] at the root `SessionStart`. Folded with the session id
    /// into this orchestrator's intercom presence target
    /// ([`crate::spawn::intercom_target::orchestrator_presence_target`]) — the address a spawned
    /// child's `contact_supervisor` relays to (pi `resolveIntercomSessionTarget`). Empty/unset when
    /// the live backend has no session name (the alias `subagent-chat-<id8>` is used instead).
    /// Cleared alongside [`Self::root_parent_session`] at `session_shutdown` (same rationale).
    root_parent_session_name: Arc<std::sync::Mutex<Option<String>>>,
    /// The live-child steer transport (R-SA-086). Defaults to
    /// [`crate::tui::intercom::NoTransportSteerChannel`] (no broker → always "not registered"); the
    /// intercom companion's broker-backed `SteerChannel` is threaded in via
    /// [`SubagentsExtension::with_channels`] → [`SubagentExecutor::with_channels`]. Consumed by
    /// [`Self::control_resume`]'s `SteerRunning` arm to DELIVER `action='resume'`'s follow-up to a
    /// still-running async child over the broker (pi `subagent-executor.ts:860-878`).
    steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    /// The out-of-band grouped-result delivery channel (R-SA-123/124/125). Defaults to
    /// [`crate::tui::intercom::NoTransportChannel`] (always "not delivered", full inline preserved);
    /// the intercom companion's broker-backed `DeliveryChannel` is threaded in via
    /// [`SubagentsExtension::with_channels`] → [`SubagentExecutor::with_channels`].
    delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
    /// The single-slot clarify/ask lock (R-SA-119/120) backed by a [`crate::tui::intercom::ClarifyChannel`].
    /// Defaults to [`crate::tui::intercom::AskLock::new_with_no_live_channel`]; the intercom companion's
    /// broker-backed `ClarifyChannel` is threaded in via [`SubagentsExtension::with_channels`]. Consumed
    /// by the exec detach-trigger arm (R-SA-037) when a child's `contact_supervisor` blocking ask fires.
    clarify: Arc<crate::tui::intercom::AskLock>,
    /// Live foreground-run control registry (pi `state.foregroundControls`, `shared/types.ts`):
    /// `targetRunId -> {interrupt, currentAgent, currentIndex}` for every foreground single run this
    /// executor currently has in flight. Populated by [`Self::run_foreground_impl`] just before
    /// driving the run and removed right after it settles, so a lookup miss means "not active" —
    /// exactly pi's "run is not active in this fanout child" guard. Consumed by
    /// [`Self::resolve_nested_control_request`] (the fanout child's nested-control inbox listener,
    /// pi `fanout-child.ts:53-128`) to service an interrupt/resume request a grandparent orchestrator
    /// addressed at a run nested inside THIS process.
    foreground_controls: Arc<std::sync::Mutex<HashMap<String, ForegroundControlEntry>>>,
    /// The per-SESSION subagent spawn budget (pi `SubagentState.subagentSpawns`,
    /// `shared/types.ts:842`: `{ sessionId: string | null; count: number }`). Charged UP FRONT by
    /// [`Self::reserve_subagent_spawns`] at every accepted execution dispatch, so a run that later
    /// fails still consumes its reservation — exactly pi's `reserveSubagentSpawns`
    /// (`runs/foreground/subagent-executor.ts:266-282`), which sets `count = used + requested`
    /// before any child is planned and never refunds. Reset when the recorded session id no longer
    /// matches the live one, and again at `SessionStart` ([`Self::reset_spawn_budget`], pi
    /// `resetSessionState`, `extension/index.ts:590`).
    spawn_budget: std::sync::Mutex<SpawnBudget>,
}

/// One session's subagent spawn budget (pi `SubagentState.subagentSpawns`, `shared/types.ts:842`).
/// `session_id` is the session the `count` was accumulated under — pi's `string | null`, so a
/// headless/unpersisted session (`None`) is a legitimate identity that still accumulates.
#[derive(Debug, Default)]
struct SpawnBudget {
    session_id: Option<String>,
    count: u32,
}

/// One live foreground run's control surface (pi `SubagentState.foregroundControls`'s per-entry
/// shape, `shared/types.ts`): the soft-interrupt token for its current attempt, plus the live
/// message-route coordinates (`currentAgent`/`currentIndex`) a nested-control "resume" request
/// resolves to the SAME [`crate::spawn::intercom_target::resolve_subagent_intercom_target`] string
/// the child registered its broker presence under at spawn.
#[derive(Clone)]
struct ForegroundControlEntry {
    /// Fires this run's soft interrupt (pi `control.interrupt?.()`); shared with the live
    /// [`RunOptions::interrupt`] token the running child's own attempt loop races against.
    interrupt: CancelToken,
    /// The run's current step agent name (pi `control.currentAgent`); `None` means no live message
    /// route exists yet (pi's "has no active child message route" guard).
    current_agent: Option<String>,
    /// The run's current step's flat child index (pi `control.currentIndex ?? 0`).
    current_index: Option<usize>,
}

/// The structured result of [`SubagentExecutor::run_or_background_graph`]: either a detached
/// background run was launched (carrying its [`RunId`]), or the graph was walked to completion in
/// the foreground and its per-step results are returned for the caller to render. Keeping this
/// structured (rather than pre-rendering a string inside the executor) is what lets the tool's
/// PARALLEL mode render pi's `N/M succeeded` summary while the slash commands render their own
/// per-step text, both over the SAME underlying walk.
pub enum GraphRunOutcome {
    /// A background run was spawned (detached hop-1); nothing waited on its completion (R-SA-074).
    Background(RunId),
    /// The graph was walked to completion in the foreground. `results`/`is_group`/`groups` are the
    /// exact triple [`render_chain_results`]/[`render_parallel_tool_summary`] consume. `run_id` is
    /// THIS run's own real, stable id (pi `runId`, `subagent-executor.ts:1087-1091`) — the same one
    /// used to derive this run's `{chain_dir}` — never a fresh id minted only for an out-of-band
    /// intercom payload/receipt (R-SA-123/124/125's "Run: {runId}" must be correlatable).
    Foreground {
        run_id: RunId,
        results: Vec<StepResult>,
        is_group: Vec<bool>,
        groups: Vec<GroupStepResult>,
    },
}

impl Default for SubagentExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// SUBA-041 — the per-call SINGLE-mode override surface pi's `runSinglePath` honors
/// (`subagent-executor.ts:2788-2791` output/outputMode/skill, `:2962` acceptance, `:2874` share,
/// `:3387-3401` artifacts/sessionDir), carried as ONE owned bundle so
/// [`ForegroundRunRequest`] stays within the field budget and every non-tool caller (the `/run`
/// slash surface, tests) can keep saying [`Default::default`] for "no overrides at all".
///
/// The values here are the RAW tool params, not resolved paths: pi resolves an `output` string
/// against `resolveSingleRunOutputBaseDir(deps, artifactsDir, runId)`
/// (`subagent-executor.ts:2203-2207,2882`), a base directory that only exists once the run id has
/// been minted and the artifacts dir computed — i.e. inside `run_foreground_impl`, not at the
/// dispatch site.
#[derive(Debug, Clone, Default)]
pub struct SingleRunOverrides {
    /// pi `params.output` (`OutputOverride`, `schemas.ts:42-48`): a path string, `false`/`"false"`
    /// to disable, or `true`/`"true"` to mean "the persona's own declared output". `None` = the
    /// param was omitted, which defers to the persona's `output:` exactly as pi's
    /// `params.output !== undefined ? params.output : agentConfig.output` does.
    pub output: Option<serde_json::Value>,
    /// pi `params.outputMode` (`schemas.ts:50-53`): `"inline"` (pi's own default) or `"file-only"`.
    pub output_mode: Option<String>,
    /// pi `params.skill` (`SkillOverride`, `schemas.ts:33-40`), already normalized through
    /// [`normalize_skill_input`]: `Some(names)` replaces the persona's own `skills:`, `Some(vec![])`
    /// is the explicit `skill: false` "no skills" form, `None` inherits the persona's list.
    pub skills: Option<Vec<String>>,
    /// pi `params.acceptance` (`AcceptanceOverride`, `schemas.ts:69-76`), already validated and
    /// lowered by [`parse_single_acceptance`]. `None` defers to
    /// [`crate::exec::acceptance::AcceptanceContract::heuristic_default`] (R-SA-023), which is
    /// exactly what pi's `acceptance: "auto"` / omitted means.
    pub acceptance: Option<crate::exec::acceptance::AcceptanceContract>,
    /// pi `params.share` (`subagent-executor.ts:3354` `shareEnabled`).
    pub share: Option<bool>,
    /// pi `params.sessionDir` (`subagent-executor.ts:3393-3401`), still the RAW string: it is
    /// tilde-expanded and `path.resolve`d, then suffixed with pi's own `<runId>/run-0` layout once
    /// the run id exists.
    pub session_dir: Option<String>,
    /// pi `params.artifacts` (`subagent-executor.ts:3387-3390`): `enabled = artifacts !== false`, so
    /// only an explicit `Some(false)` turns the artifact quadruple off.
    pub artifacts: Option<bool>,
}

/// The seven inputs one foreground single run needs, bundled into one borrowed request so
/// [`SubagentExecutor::run_foreground_streaming`] and the shared `run_foreground_impl` stay within
/// the argument-count budget (the non-streaming [`SubagentExecutor::run_foreground`] keeps its
/// original flat signature for backward compatibility and builds this internally). All fields
/// borrow for the duration of the one `run_foreground*` call they are passed to.
pub struct ForegroundRunRequest<'a> {
    /// SUBA-041: the per-call SINGLE-mode override bundle (`output`/`outputMode`/`skill`/
    /// `acceptance`/`share`/`sessionDir`/`artifacts`). [`SingleRunOverrides::default`] is
    /// "no overrides", which reproduces this entry point's pre-SUBA-041 behavior exactly.
    pub overrides: SingleRunOverrides,
    /// The task's working directory (also the discovery root for the named persona).
    pub cwd: &'a Path,
    /// The persona name to resolve and run (func-SA §5.2).
    pub agent_name: &'a str,
    /// The task text handed to the child (pi's `Task: <task>` child prompt).
    pub task: &'a str,
    /// The resolved execution-time agent-discovery scope (pi `resolveExecutionAgentScope`,
    /// `subagent-executor.ts:2973`): narrows the User-vs-Project axis when resolving `agent_name`.
    pub agent_scope: AgentReadScope,
    /// Call-site fork/fresh context; `None` defers to the persona's own `default_context`.
    pub context: Option<ContextMode>,
    /// Per-call model override (added to the availability set, R-SA-038); `None` inherits.
    pub model_override: Option<ModelId>,
    /// Foreground timeout budget in milliseconds (pi `timeoutMs`/`maxRuntimeMs`); `None` = none.
    pub timeout_ms: Option<u64>,
    /// The host's own cancellation token for this tool call (pi `execute(id, params, signal, ...)`,
    /// `extension/index.ts:498-500`), threaded straight into [`RunOptions::cancel`] so an abort of
    /// the tool call (user Esc / turn abort) drives the running child through the real
    /// SIGINT→SIGTERM→SIGKILL escalation instead of being silently dropped at this seam.
    pub cancel: CancelToken,
}

/// The already-resolved, plan-shaped inputs [`SubagentExecutor::spawn_background_steps`] takes from
/// its caller, bundled into one owned spec so that entry point stays within the argument-count
/// budget (mirroring [`ForegroundRunRequest`]'s role for the foreground path). Every field here is
/// one the ORCHESTRATOR resolves exactly once — the step graph, its run mode, the fork-context
/// session file, the plan-time persona map, and the run-wide `{task}`/`{chain_dir}` substitution
/// values — and hands verbatim to the detached hop-2 runner via `RunnerConfig`. The pieces no
/// caller can supply (the fresh [`RunId`], plus the process-config-derived concurrency / worktree /
/// depth / async-root / results-dir values read from the live `config_snapshot`) are filled in by
/// `spawn_background_steps` itself and are deliberately NOT carried here.
pub struct BackgroundStepsSpec {
    /// The already-resolved step graph to dispatch (`RunnerConfig::steps`).
    pub steps: Vec<RunnerStep>,
    /// How the detached runner drives that graph (`RunnerConfig::mode`).
    pub mode: RunMode,
    /// The fork-context session file the orchestrator resolved once (`RunnerConfig::session_file`);
    /// `None` for a run that starts no session.
    pub session_file: Option<PathBuf>,
    /// The plan-time persona map (`RunnerConfig::resolved_agents`) so hop 2 dispatches each step's
    /// REAL persona rather than re-discovering or falling back to a placeholder.
    pub resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
    /// The run-wide `{task}` value (`RunnerConfig::original_task`) every step's `{task}` resolves to.
    pub original_task: String,
    /// The dedicated per-run scratch directory `{chain_dir}` resolves to (`RunnerConfig::chain_dir`);
    /// `None` for a single top-level task that has no chain dir (`{chain_dir}` → the run cwd).
    pub chain_dir: Option<PathBuf>,
}

impl SubagentExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Arc::new(AsyncMutex::new(SubagentExtensionConfig::default())),
            tracker: Arc::new(JobTracker::new()),
            completion_sink_override: None,
            completion_watcher: AsyncMutex::new(None),
            host_services: Arc::new(OnceLock::new()),
            root_parent_session: Arc::new(std::sync::Mutex::new(None)),
            root_parent_session_name: Arc::new(std::sync::Mutex::new(None)),
            steer: Arc::new(crate::tui::intercom::NoTransportSteerChannel),
            delivery: Arc::new(crate::tui::intercom::NoTransportChannel),
            clarify: Arc::new(crate::tui::intercom::AskLock::new_with_no_live_channel()),
            foreground_controls: Arc::new(std::sync::Mutex::new(HashMap::new())),
            spawn_budget: std::sync::Mutex::new(SpawnBudget::default()),
        }
    }

    /// Construct an executor whose background-completion notifications (C6) are delivered to
    /// `sink` instead of the default graceful-degradation logging sink — the seam a host uses to
    /// route completions into a live session's turn loop (R-SA-101), and a test uses to capture
    /// them. Explicitly overriding the sink here wins over the P-1 `host_services`-derived
    /// [`HostServicesCompletionSink`] at install time (so a test's scripted sink is authoritative).
    #[must_use]
    pub fn with_completion_sink(sink: Arc<dyn crate::background::watch::CompletionSink>) -> Self {
        Self { completion_sink_override: Some(sink), ..Self::new() }
    }

    /// Late-bind the live capability backend (P-1). Called by
    /// [`SubagentsExtension::set_host_services`] (which the builder invokes via
    /// `load_native_with_services` BEFORE `init`) so the `SessionStart` handler, the fork-context
    /// resolver, and the completion watcher reach the live session id/file + `inject_message`.
    /// Idempotent (`OnceLock::set` ignores a second bind of the same session rebuild).
    pub fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        let _ = self.host_services.set(services);
    }

    /// The captured live capability backend, if the P-1 slot has been bound.
    #[must_use]
    pub fn host_services(&self) -> Option<Arc<dyn cyrup_ext::host::HostServices>> {
        self.host_services.get().cloned()
    }

    /// Thread the intercom companion's real broker-backed delivery + clarify + steer channels into
    /// this executor (item 2 of reconciliation §4 step 5), replacing the `NoTransportChannel`/no-live
    /// `AskLock`/`NoTransportSteerChannel` defaults. `delivery` closes R-SA-123/124/125 (out-of-band
    /// grouped delivery + reduced inline receipt); `clarify` (wrapped in a single-slot
    /// [`crate::tui::intercom::AskLock`], R-SA-120) closes R-SA-119/120 and backs the exec
    /// detach-trigger arm (R-SA-037); `steer` closes R-SA-086's live-child follow-up delivery (the
    /// [`Self::control_resume`] `SteerRunning` arm delivers `action='resume'` over the broker).
    #[must_use]
    pub fn with_channels(
        mut self,
        delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
        clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
        steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    ) -> Self {
        self.delivery = delivery;
        self.clarify = Arc::new(crate::tui::intercom::AskLock::new(clarify));
        self.steer = steer;
        self
    }

    /// The captured parent-session anchor (`CYRUP_SUBAGENT_PARENT_SESSION`, R-SA-P1), if the root
    /// `SessionStart` handler has resolved it from [`cyrup_ext::host::HostServices::session_id`].
    #[must_use]
    pub fn root_parent_session(&self) -> Option<String> {
        self.root_parent_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    }

    /// Reserve `requested` subagent spawns against THIS session's budget (pi `reserveSubagentSpawns`,
    /// `runs/foreground/subagent-executor.ts:266-282`), returning pi's exact over-limit text on
    /// breach and `Ok(())` otherwise.
    ///
    /// The reservation is charged UP FRONT (`count = used + requested`) and never refunded — pi
    /// deliberately bills a run at dispatch, so a fan-out that later fails still consumes its share
    /// of the session's budget. `requested == 0` is a no-op (pi's `if (input.requested <= 0) return
    /// undefined`), so a call that spawns nothing (e.g. an empty/`action` shape) never touches the
    /// counter. The comparison is pi's strict `used + requested > maxSpawns`, so a call that lands
    /// exactly ON the cap is allowed.
    ///
    /// The session identity is [`Self::root_parent_session`] — cyrup's analog of pi's
    /// `state.currentSessionId` (captured from the live `HostServices::session_id` at the root
    /// `SessionStart`). A change of session id resets the counter in place, exactly as pi's
    /// `if (state.subagentSpawns?.sessionId !== sessionId)` guard does, so a long-lived process that
    /// starts a second session starts that session with a fresh budget.
    ///
    /// # Call sites (SUBA-002)
    /// EVERY route into execution charges here, so the budget cannot be walked around by picking a
    /// different surface — upstream gets that property structurally (every slash handler funnels
    /// back through `executor.execute`, `slash/slash-commands.ts` `runSlashSubagent` ->
    /// `requestSlashRun` -> `extension/index.ts:396-401` -> `executeSubagentCollapsed`), this crate
    /// gets it by charging at each independent entry point exactly once:
    ///
    /// * the `subagent` TOOL — [`SubagentTool::execute`], after the dispatch guard and the
    ///   mode-exclusivity gate, covering its SINGLE/PARALLEL/CHAIN routes
    ///   ([`count_requested_subagent_spawns`]);
    /// * `/run` — [`SubagentsExtension::dispatch_slash`]'s `Run` arm, billed `1` for both the
    ///   foreground and the `--background` shape;
    /// * `/chain`, `/parallel`, `/run-chain` — [`SubagentsExtension::run_or_background_chain`], the
    ///   single wrapper all three share, billed over the lowered graph
    ///   ([`count_graph_requested_spawns`]).
    ///
    /// The tool path never re-enters the slash wrapper (it reaches
    /// [`Self::run_or_background_graph`] via `route_chain_mode`/`route_parallel_mode`), so no
    /// dispatch is billed twice.
    ///
    /// # Errors
    /// The over-limit notice (pi's verbatim string) when `used + requested` exceeds `max_spawns`.
    pub fn reserve_subagent_spawns(&self, requested: u32, max_spawns: u32) -> Result<(), String> {
        if requested == 0 {
            return Ok(());
        }
        let session_id = self.root_parent_session();
        let mut budget = self
            .spawn_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if budget.session_id != session_id {
            *budget = SpawnBudget { session_id, count: 0 };
        }
        let used = budget.count;
        if u64::from(used) + u64::from(requested) > u64::from(max_spawns) {
            return Err(format!(
                "Subagent spawn limit reached for this session ({used}/{max_spawns} used, \
                 {requested} requested). Complete the work directly or start a new session."
            ));
        }
        budget.count = used.saturating_add(requested);
        Ok(())
    }

    /// Reset this session's spawn budget to zero under the CURRENT session id (pi
    /// `resetSessionState`'s `state.subagentSpawns = { sessionId: state.currentSessionId, count: 0 }`,
    /// `extension/index.ts:590`). Called from the `SessionStart` handler right after the
    /// parent-session anchor is captured, so a second session on a long-lived process (SDK embedder /
    /// test harness) starts from a clean budget even when neither session had a resolvable id — the
    /// case [`Self::reserve_subagent_spawns`]' own id-change guard cannot detect on its own.
    pub fn reset_spawn_budget(&self) {
        let session_id = self.root_parent_session();
        *self
            .spawn_budget
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            SpawnBudget { session_id, count: 0 };
    }

    /// The out-of-band delivery channel (R-SA-123/124/125), for the run driver's grouped-result
    /// delivery attempt.
    #[must_use]
    pub fn delivery_channel(&self) -> Arc<dyn crate::tui::intercom::DeliveryChannel> {
        self.delivery.clone()
    }

    /// The single-slot clarify/ask lock (R-SA-119/120), for the exec detach-trigger arm (R-SA-037).
    #[must_use]
    pub fn clarify_lock(&self) -> Arc<crate::tui::intercom::AskLock> {
        self.clarify.clone()
    }

    /// Attempt out-of-band delivery of a completed grouped (parallel/chain) run's result through the
    /// executor's [`crate::tui::intercom::DeliveryChannel`] (R-SA-123/124/125), racing it against the
    /// default bounded timeout so a missing receiver never stalls the tool's own turn. Returns
    /// [`crate::tui::intercom::DeliveryOutcome::Delivered`] only when a receiver confirmed receipt —
    /// the caller may then REDUCE its inline tool payload (drop the heavy duplicated per-child
    /// outputs, R-SA-123); on any other outcome the caller keeps the full inline result (R-SA-125).
    /// With the `NoTransportChannel` default (no intercom wired) this always reports `NotDelivered`,
    /// exactly as the spec anticipates, so the inline result stays full.
    pub async fn deliver_group_out_of_band(
        &self,
        payload: crate::tui::intercom::IntercomPayload,
    ) -> crate::tui::intercom::DeliveryOutcome {
        crate::tui::intercom::deliver_with_default_timeout(self.delivery.as_ref(), payload).await
    }

    /// Capture the canonical parent-session anchor from the live session id (P-2), at the root
    /// orchestrator's `SessionStart` (depth 0). Reads [`cyrup_ext::host::HostServices::session_id`]
    /// off the bound P-1 backend; a `None`/empty id (headless / unpersisted / no live session) leaves
    /// the slot unset, so the spawn-site resolution falls through to the inherited env value.
    pub fn capture_parent_session_anchor(&self) {
        if let Some(services) = self.host_services()
            && let Some(id) = services.session_id()
            && !id.is_empty()
        {
            *self
                .root_parent_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(id);
            // Capture the session NAME too (may be absent): it feeds this orchestrator's own intercom
            // presence target (`orchestrator_presence_target(name, id)`), the address a spawned
            // child's `contact_supervisor` relays to. An absent/empty name falls through to the
            // `subagent-chat-<id8>` alias inside that resolver, so only a real name is stored here.
            if let Some(name) = services.session_name().filter(|n| !n.trim().is_empty()) {
                *self
                    .root_parent_session_name
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(name);
            }
        }
    }

    /// Clear the captured parent-session anchor (pi `delete process.env[SUBAGENT_PARENT_SESSION_ENV]`,
    /// `extension/index.ts:645`), called from `session_shutdown` so a stale id/name from the
    /// session that just ended never leaks into a subsequently-started session on this same
    /// long-lived process (e.g. an SDK embedder / test harness that starts multiple sessions
    /// against one `SubagentExecutor`). Detached background runs already spawned are wholly
    /// unaffected — this only clears THIS orchestrator's own anchor for future spawns.
    pub fn clear_parent_session_anchor(&self) {
        *self
            .root_parent_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        *self
            .root_parent_session_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// This root orchestrator's own intercom presence target — the address a spawned child's
    /// `contact_supervisor` relays to (pi `resolveIntercomSessionTarget(pi.getSessionName(),
    /// sessionManager.getSessionId())`, `subagent-executor.ts:893`). Byte-identical to the string the
    /// intercom companion registers this session's broker presence under
    /// ([`cyrup_intercom`]'s `build_registration` derives it from the SAME `HostServices`), so the
    /// two independently-produced strings match at the broker. `None` when no live session id was
    /// captured (headless / SDK-embedder) — the spawn site then writes no child-bridge env, so the
    /// child registers no supervisor bridge (the clean no-intercom path).
    #[must_use]
    pub fn orchestrator_intercom_target(&self) -> Option<String> {
        let id = self.root_parent_session()?;
        let name_guard = self
            .root_parent_session_name
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let name = name_guard.as_deref().filter(|s| !s.trim().is_empty());
        Some(crate::spawn::intercom_target::orchestrator_presence_target(name, &id))
    }

    /// The live PARENT session's current model as a `provider/id` [`ModelId`] — pi's `ctx.model`
    /// (`pi-subagents/src/runs/shared/model-fallback.ts:47-59`), the model an inheriting subagent
    /// (a persona with no `model:` of its own, run with no per-call override) resolves to. Read off
    /// the bound P-1 [`cyrup_ext::host::HostServices`] backend
    /// ([`cyrup_ext::host::HostServices::current_model`], returned by `LiveHostServices` as
    /// `"{provider}/{model}"`), the SAME live-session seam `session_id`/`session_file`/
    /// `inject_message` already reach. `None` when no live session backend is bound (headless /
    /// SDK-embedder) or it has no active model yet — the ladder then falls through to the persona's
    /// own `model`/`fallback_models` exactly as before (see [`crate::exec::fallback::resolve_model_inheritance`]).
    #[must_use]
    pub fn inherited_session_model(&self) -> Option<ModelId> {
        self.host_services().and_then(|s| s.current_model()).map(ModelId::from)
    }

    /// The effective background-completion sink to install this session (R-SA-101). Precedence:
    /// an explicitly-injected [`Self::with_completion_sink`] override (a test's scripted sink) →
    /// a live [`HostServicesCompletionSink`] when the P-1 `host_services` slot is bound (the real
    /// turn-injecting sink) → the graceful-degradation [`crate::background::watch::LoggingCompletionSink`]
    /// (stderr log + delete) when no host handle is present (the SDK-embedder / headless default).
    fn effective_completion_sink(&self) -> Arc<dyn crate::background::watch::CompletionSink> {
        if let Some(sink) = &self.completion_sink_override {
            return sink.clone();
        }
        if let Some(services) = self.host_services() {
            return Arc::new(crate::background::watch::HostServicesCompletionSink::new(services));
        }
        Arc::new(crate::background::watch::LoggingCompletionSink)
    }

    /// Install (or reinstall) the background-completion watcher (C6) over this cwd's `ResultsDir`
    /// (`notify.ts` + `result-watcher.ts`): ensure the results directory exists, attach a real
    /// filesystem watch, and drain freshly-completed runs into this executor's completion sink,
    /// deleting each result file after its notification is delivered (R-SA-099). Idempotent —
    /// reinstalling replaces (and tears down) any prior session's watcher. Best-effort: a failure to
    /// create the results dir or attach the watch degrades to "no completion notifications this
    /// session" rather than failing session start.
    pub async fn install_completion_watcher(&self, cwd: &Path) {
        let results_dir = default_results_dir(cwd);
        if crate::background::ensure_accessible_dir(&results_dir).await.is_err() {
            return;
        }
        match crate::background::watch::install_completion_watcher(
            results_dir,
            self.effective_completion_sink(),
        ) {
            Ok(handle) => {
                *self.completion_watcher.lock().await = Some(handle);
            }
            Err(_) => {
                // Degrade gracefully: no watcher this session (e.g. the results dir vanished between
                // the ensure above and the watch attach). Completions written later this session
                // simply are not surfaced until a future session re-installs the watch on start.
            }
        }
    }

    /// Tear down this session's completion watcher (pi `session_shutdown`'s `stopResultWatcher()`,
    /// `extension/index.ts:656`): drop the held [`crate::background::watch::CompletionWatcherHandle`],
    /// whose `Drop` impl aborts the drain task and releases the filesystem watch. A no-op if no
    /// watcher was ever installed (headless / a degraded install, `install_completion_watcher`'s own
    /// best-effort failure path).
    pub async fn stop_completion_watcher(&self) {
        *self.completion_watcher.lock().await = None;
    }

    /// Full session-teardown housekeeping (pi `session_shutdown`, `extension/index.ts:644-680`,
    /// minus the pieces this crate has no analog for — see `on_event`'s `SessionShutdown` arm doc
    /// for the exact mapping): stop the completion watcher, abort+clear the background job
    /// tracker's poll loop and in-memory job map, and clear the captured parent-session anchor.
    /// Detached background runs already spawned are left running to completion untouched
    /// (R-SA-071/DI-SA-8) — this only resets THIS process's own live session-scoped state.
    pub async fn teardown_session(&self) {
        self.stop_completion_watcher().await;
        self.tracker.stop_and_clear().await;
        self.clear_parent_session_anchor();
    }

    /// Current effective extension config snapshot (tier 3 of R-SA-133).
    pub async fn config_snapshot(&self) -> SubagentExtensionConfig {
        self.config.lock().await.clone()
    }

    /// The shared background-job tracker (R-SA-093), so `on_event`'s `SessionStart` handler can
    /// resume tracking any runs still recorded on disk from a prior process.
    #[must_use]
    pub fn tracker(&self) -> &Arc<JobTracker> {
        &self.tracker
    }

    // ---------------------------------------------------------------------------------------
    // Discovery config assembly (bridges HostCtx.cwd -> a real AgentDiscoveryConfig)
    // ---------------------------------------------------------------------------------------

    /// Build a real [`AgentDiscoveryConfig`] scoped to `cwd`, resolving the full pi directory
    /// topology (agents.ts:511-522,1234-1259,1279-1280): an **upward project-root search**
    /// ([`crate::discovery::find_nearest_project_root`]) so a cwd nested below the project root
    /// still finds the project's agents; the legacy `<root>/.agents` project dir plus the preferred
    /// `<root>/.cyrup/agents` dir; the primary `~/.cyrup/agents` plus the "second" `~/.agents` user
    /// dir; **separate** `.cyrup/chains` chain dirs at each scope (never the shared agents dir); the
    /// bundled builtin-persona resource root ([`builtin_agents_dir`], R-SA-020/132/134); and
    /// R-SA-003's `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` extras (prepended, lowest User-tier precedence).
    ///
    /// # Package tier (Tier-2 wire-up)
    ///
    /// The package tier is now populated by enumerating `cyrup-resources`' own persisted
    /// `packages.json` install registries (Global under `<home>/.cyrup/packages.json`, Project under
    /// `<cwd>/.cyrup/packages.json`) via [`enumerate_installed_packages`], so a package that declares
    /// an `agents = [...]` manifest entry (R-SA-020) has its personas discovered at
    /// [`crate::discovery::types::AgentSource::Package`] scope by [`crate::discovery::scan_package_agents`]
    /// and its chain files (chains-share-agents-dir) discovered at Package scope by
    /// [`crate::discovery::scan_package_chain_scopes`]. R-SA-001's four-scope precedence
    /// (package first-seen-wins, then user/project last-seen-wins) now holds over all four populated
    /// tiers rather than three.
    ///
    /// `project_root` is `cwd` (the same base the `.cyrup/agents` project dir is derived from);
    /// `global_dir` is `<home>/.cyrup`. `trusted_project` is fail-closed (`false`): a Project-scope
    /// package's `agents` manifest entries are skipped until a project-trust decision is threaded in
    /// (the same not-yet-threaded seam this file documents for the live session-manager / settings
    /// layering — cyrup-config's DI-11 trust decision has no injection point into this extension
    /// today, so this crate never silently trusts a project's installed packages). Global-scope
    /// packages are always enumerated (trust-independent, matching `cyrup-resources`' own gate).
    fn discovery_dirs_config(cwd: &Path) -> AgentDiscoveryConfig {
        let home = dirs_home();
        let global_dir = home.join(".cyrup");
        // Upward project-root search (pi `findNearestProjectRoot`, agents.ts:511-522): the nearest
        // ancestor of `cwd` holding a `.cyrup` config dir or a legacy `.agents` dir, so a cwd deep
        // inside a project still discovers that project's agents/chains. Absent any such ancestor,
        // fall back to `cwd` (pi's `findNearestProjectRoot(cwd) ?? cwd`) so package roots and the
        // project write target still resolve under `cwd/.cyrup`.
        let project_root =
            crate::discovery::find_nearest_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
        let installed_packages = enumerate_installed_packages(&global_dir, Some(&project_root));
        // Per-scope read dirs from the shared topology helpers (pi resolveNearestProject*Dirs /
        // discoverAgents userDir old+new / getUserChainDir): legacy `.agents` + preferred
        // `.cyrup/agents` for project agents; primary `.cyrup/agents` + second `~/.agents` for user
        // agents; a SEPARATE `.cyrup/chains` dir for each scope's chains (never the agents dir).
        AgentDiscoveryConfig {
            builtin_agents_dir: Some(builtin_agents_dir()),
            project_agent_dirs: crate::discovery::resolve_project_agent_read_dirs(&project_root),
            project_chain_dirs: crate::discovery::resolve_project_chain_read_dirs(&project_root),
            user_agent_dirs: crate::discovery::resolve_user_agent_read_dirs(&home),
            user_chain_dirs: crate::discovery::resolve_user_chain_read_dirs(&home),
            global_dir,
            project_root: Some(project_root),
            trusted_project: false,
            installed_packages,
            ..AgentDiscoveryConfig::default()
        }
        // R-SA-003: fold in `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS` — PREPENDED ahead of the user dirs
        // (extras are the lowest-precedence User-tier stream), so the user's own agents win.
        .with_env_extras()
    }

    /// Build a real, fully-populated [`AgentDiscoveryConfig`] scoped to `cwd`: the directory/package
    /// topology from [`discovery_dirs_config`](Self::discovery_dirs_config) PLUS the `subagents.*`
    /// settings layer read from the user (`~/.cyrup/agents/settings.json`) and project
    /// (`<cwd>/.cyrup/agents/settings.json`) `settings.json` files (C2 wiring). The two scopes are
    /// layered per R-SA-012/133 by [`crate::discovery::load_layered_subagent_settings`] (project wins
    /// over user on every scalar and per-agent override name; a project `disableBuiltins: false`
    /// re-enables what a user `true` disabled), which then drives `merge.rs`'s
    /// `defaultModel`/`disableBuiltins`/`disableThinking`/`agentOverrides` application over the merged
    /// agents.
    ///
    /// # Errors
    ///
    /// Propagates [`SubagentError::MalformedSettings`] (R-SA-009) when either scope's `settings.json`
    /// exists but cannot be read, does not parse, is not a JSON object, or carries a malformed
    /// `subagents.*` field — the malformed-settings MUST-abort contract this crate's discovery
    /// callers rely on.
    fn discovery_config(cwd: &Path) -> Result<AgentDiscoveryConfig, SubagentError> {
        let mut cfg = Self::discovery_dirs_config(cwd);
        let user_settings = dirs_home().join(".cyrup").join("agents").join("settings.json");
        let project_settings = cwd.join(".cyrup").join("agents").join("settings.json");
        // Tier 7: carry BOTH scopes UNFLATTENED (each with its own path) so `merge.rs` can resolve
        // project-beats-user at application time and record the true winning scope + path in
        // provenance (rather than a pre-flattened single scope that always looked like `Project`).
        cfg.override_settings = crate::discovery::load_layered_override_settings(
            &user_settings,
            Some(&project_settings),
        )?;
        Ok(cfg)
    }

    /// Resolve one agent by its fully-qualified runtime name (R-SA-008: exact string equality
    /// only), via the real, on-demand, re-scanned-per-call discovery pipeline (R-SA-019).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::AgentNotFound`] if no delegation-visible agent matches `name`
    /// exactly, or propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings
    /// abort).
    pub fn resolve_agent(
        &self,
        cwd: &Path,
        name: &str,
        scope: AgentReadScope,
    ) -> Result<AgentDefinition, SubagentError> {
        self.resolve_agent_with_model_scope(cwd, name, scope).map(|(agent, _)| agent)
    }

    /// [`Self::resolve_agent`] plus the effective `subagents.modelScope` policy this cwd's settings
    /// declare (SUBA-003) — pi's `discoverAgents` hands back `{ agents, modelScope }` together
    /// (`agents.ts:1446`), and an execution path needs BOTH: the persona to run, and the policy the
    /// model it runs on must satisfy. Returned as one call so the run path does not walk discovery
    /// twice (once for the agent, once for the settings) and can never see a scope read from a
    /// different point in time than the persona it is gating.
    ///
    /// # Errors
    ///
    /// Same as [`Self::resolve_agent`]: [`SubagentError::AgentNotFound`], or a discovery-time
    /// [`SubagentError::MalformedSettings`] — which now also covers a malformed `modelScope` block
    /// (R-SA-009's MUST-abort, rather than silently ignoring an unenforceable policy).
    pub fn resolve_agent_with_model_scope(
        &self,
        cwd: &Path,
        name: &str,
        scope: AgentReadScope,
    ) -> Result<(AgentDefinition, Option<ModelScopeConfig>), SubagentError> {
        let cfg = Self::discovery_config(cwd)?;
        let result = discover_agents(&cfg, Some(scope))?;
        let model_scope = result.model_scope.clone();
        let agent = result
            .agents
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| SubagentError::AgentNotFound(name.to_string()))?;
        Ok((agent, model_scope))
    }

    /// The effective `subagents.modelScope` policy for `cwd` on its own (SUBA-003), without
    /// resolving any particular agent — for the multi-agent plan paths (`/chain`, `/parallel`,
    /// background runs), which resolve their personas through
    /// [`Self::resolve_plan_personas`] and need the policy as one value covering the whole plan.
    ///
    /// Reads only the two `settings.json` layers (via [`Self::discovery_config`]), not the agent
    /// directory walk.
    ///
    /// # Errors
    ///
    /// Propagates [`SubagentError::MalformedSettings`] (R-SA-009) when either scope's settings file
    /// is unreadable/unparseable or carries a malformed `subagents.*` field — including a malformed
    /// `modelScope` block, which MUST abort rather than degrade to unenforced.
    pub fn resolve_model_scope(cwd: &Path) -> Result<Option<ModelScopeConfig>, SubagentError> {
        Ok(Self::discovery_config(cwd)?.override_settings.model_scope())
    }

    /// Plan-time persona map (T0.1's C13 root-cause seam): resolve every DISTINCT agent named across
    /// a chain/parallel/background plan to its serializable [`ResolvedAgentPersona`], keyed by the
    /// step's `agent` name. This is the orchestrator half of T0.1 that the canonical
    /// [`crate::exec::resolve_step_agent_config`] resolver's own doc describes: the discovery lookup
    /// (name -> [`AgentDefinition`]) is done HERE — `extension.rs` is the ONE place with real
    /// discovery access (`crates/cyrup/src/subagent_runner_cmd.rs`'s hop-2 runner has none, which is
    /// exactly why `background/runner_main.rs`'s `ExecSingleStepExecutor` would otherwise synthesize a
    /// placeholder `AgentConfig{system_prompt_body:"", model:"default", completion_guard:Some(false),
    /// …}`) — then each resolved definition is projected via
    /// [`crate::exec::resolve_step_agent_config`].
    ///
    /// The returned map is stashed into [`crate::background::runner_main::RunnerConfig::resolved_agents`]
    /// (for a background run) or handed straight to
    /// [`crate::background::runner_main::ExecSingleStepExecutor::foreground`] (for a foreground
    /// `/chain`//`/parallel` run), so the runner dispatches the REAL persona (its own system prompt,
    /// model + fallback ladder, completion guard, per-step depth ceiling) and NEVER re-discovers.
    /// Resolving up front also validates every referenced agent EXISTS before any child process is
    /// spawned — matching pi, which validates agent names before starting a `/chain`/`/parallel`
    /// rather than spawning a partial run that dies mid-walk.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::AgentNotFound`] if any named agent resolves to no delegation-visible
    /// agent, or propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings abort).
    pub fn resolve_plan_personas(
        &self,
        cwd: &Path,
        agent_names: impl IntoIterator<Item = String>,
        scope: AgentReadScope,
    ) -> Result<BTreeMap<String, ResolvedAgentPersona>, SubagentError> {
        let mut personas: BTreeMap<String, ResolvedAgentPersona> = BTreeMap::new();
        for name in agent_names {
            if personas.contains_key(&name) {
                continue;
            }
            let agent = self.resolve_agent(cwd, &name, scope)?;
            personas.insert(name, crate::exec::resolve_step_agent_config(&agent));
        }
        Ok(personas)
    }

    // ---------------------------------------------------------------------------------------
    // Fork-context resolution (per-call throwaway resolver, see module doc)
    // ---------------------------------------------------------------------------------------

    /// Build a fresh, throwaway [`ForkContextResolver`] scoped to `cwd`. A new `SessionManager`
    /// handle is opened once per call and discarded after use — never retained, never shared, never
    /// mutated in place beyond this one resolution.
    ///
    /// # Fork-context correctness (blocker #4, reconciliation §4 step 5 item 5)
    ///
    /// When `session_file` is `Some` — the REAL live-orchestrator session file obtained from the P-1
    /// [`cyrup_ext::host::HostServices::session_file`] backend — the fork branches from THAT exact
    /// parent session (matching pi threading the real `parentSessionId`/`sessionFile`), opened via
    /// [`cyrup_session::SessionManager::open_with_cwd`]. This replaces the
    /// [`cyrup_session::SessionManager::continue_recent`] most-recent-mtime HEURISTIC, which can
    /// silently pick the WRONG session when a cwd has multiple sessions. The heuristic remains ONLY
    /// as the fallback for `None` (no host handle — the SDK-embedder / headless path), and for the
    /// (rare) case where the supplied session file cannot be opened.
    fn fork_resolver(cwd: &Path, session_file: Option<&Path>) -> ForkContextResolver {
        let sessions_root = dirs_home().join(".cyrup").join("sessions");
        let layout = cyrup_session::SessionLayout::new(sessions_root.clone(), cwd.to_path_buf());
        // Blocker #4: prefer the real live-orchestrator session file (P-1) over the mtime heuristic.
        if let Some(path) = session_file
            && let Ok(manager) = cyrup_session::SessionManager::open_with_cwd(path, Some(cwd))
        {
            return ForkContextResolver::new(Arc::new(AsyncMutex::new(manager)), layout);
        }
        // `continue_recent` never fails in a way this resolver cannot itself handle: an absent
        // session directory yields a fresh, unpersisted, leafless in-memory session (R-SA-137's
        // fail-hard path handles that case correctly once `resolve(Fork, _)` is actually called);
        // a genuine I/O error is folded into the SAME "no resolvable session" outcome by treating
        // the resolver's underlying manager as absent — modeled here as an in-memory placeholder
        // so `ForkContextResolver::resolve` still runs its normal fail-hard checks rather than
        // this constructor itself needing to return a `Result` (every caller of this function
        // already only reaches it for a `context: "fork"` request, at which point
        // `resolve`'s own `is_persisted`/`leaf_id` checks are the authoritative fail-hard gate).
        let manager = cyrup_session::SessionManager::continue_recent(cwd, &layout)
            .or_else(|_| cyrup_session::SessionManager::in_memory(cwd, cyrup_session::NewSessionOpts::default()))
            .unwrap_or_else(|_| {
                // Even `in_memory` is documented infallible for a `None` id (see
                // `SessionManager::in_memory`'s own doc: "A `None` id is generated and never
                // fails"), so this arm is unreachable in practice; kept as a last-resort
                // in-memory fallback rather than a panic, matching this crate's no-panic policy.
                cyrup_session::SessionManager::in_memory(cwd, cyrup_session::NewSessionOpts::default())
                    .unwrap_or_else(|_| {
                        // Structurally unreachable (see above) but this crate forbids
                        // unwrap/expect/panic outside tests; the SessionManager type has no
                        // "empty" sentinel constructor, so the only remaining option that upholds
                        // both the no-panic policy and a total function signature is to retry
                        // once more with a definitely-valid cwd. Real production cwds are always
                        // valid paths by construction (HostCtx.cwd), so this loop terminates on
                        // the first or second attempt in every real scenario.
                        cyrup_session::SessionManager::in_memory(
                            Path::new("."),
                            cyrup_session::NewSessionOpts::default(),
                        )
                        .unwrap_or_else(|_| unreachable_session_manager())
                    })
            });
        ForkContextResolver::new(Arc::new(AsyncMutex::new(manager)), layout)
    }

    /// Resolve one task's requested [`ContextMode`] into a concrete [`ForkContext`] (R-SA-137,
    /// fail-hard per DI-SA-2 — never silently downgrades to `Fresh`).
    ///
    /// # Errors
    ///
    /// Propagates [`ForkContextResolver::resolve`]'s fail-hard errors.
    pub async fn resolve_context(
        &self,
        cwd: &Path,
        requested: ContextMode,
    ) -> Result<ForkContext, SubagentError> {
        // Blocker #4: branch from the REAL live-orchestrator session file (P-1), not the mtime guess.
        let session_file = self.host_services().and_then(|s| s.session_file());
        let resolver = Self::fork_resolver(cwd, session_file.as_deref());
        resolver.resolve(requested, 0).await
    }

    // ---------------------------------------------------------------------------------------
    // Foreground single-run dispatch (the tool's synchronous shape; exec::run_sync end to end)
    // ---------------------------------------------------------------------------------------

    /// Run one subagent task to completion in the foreground, synchronously (func-SA §5.2; the
    /// tool's default/`bg: false` shape). Resolves the agent via real discovery, resolves
    /// fork-context if requested, builds [`AgentConfig`]/[`RunOptions`], and drives
    /// [`crate::exec::run_sync`] — which spawns a REAL child OS process via
    /// [`crate::spawn::SpawnedChild::spawn`] (func-SA §1.1's mandated mechanism).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before agent discovery,
    /// fork-context resolution, or any spawn, so a blocked call touches none of that setup work.
    /// Otherwise returns [`SubagentError`] if the agent cannot be resolved, or fork-context
    /// resolution fails hard (R-SA-137). A subprocess-level failure (nonzero exit, timeout, …) is
    /// NOT an `Err` here — it is reported as a normal (non-`Ok`-gated) field on the returned
    /// [`SingleResult`], matching `run_sync`'s own contract. [`crate::exec::run_sync`] also
    /// independently re-checks this same guard as its own first action (defense in depth, since it
    /// is the sole chokepoint every spawn path in this crate funnels through) — the check here
    /// exists specifically to satisfy R-SA-055's stronger "before discovery" ordering, which
    /// `run_sync`'s own check alone cannot provide since discovery has already happened by the
    /// time `run_sync` is called.
    pub async fn run_foreground(
        &self,
        cwd: &Path,
        agent_name: &str,
        task: &str,
        context: Option<ContextMode>,
        model_override: Option<ModelId>,
        timeout_ms: Option<u64>,
    ) -> Result<SingleResult, SubagentError> {
        // No host `ToolCallId`/cancellation seam reaches this flat entry point's callers (the slash
        // dispatch path and this crate's own tests) — a fresh, never-cancelled token here matches
        // the pre-existing behavior for those callers exactly; the live host token is threaded
        // through [`ForegroundRunRequest::cancel`] by [`run_foreground_streaming`]'s callers instead
        // (`SubagentTool::execute` -> `route_single`).
        self.run_foreground_impl(
            ForegroundRunRequest {
                // The flat entry point (`/run`, this crate's own tests) exposes no per-call override
                // surface at all, so SUBA-041's bundle is empty here — identical to pre-SUBA-041.
                overrides: SingleRunOverrides::default(),
                cwd,
                agent_name,
                task,
                // pi's slash-command surfaces (`/run`, `/chain`, `/parallel`, `/run-chain`)
                // explicitly set `agentScope: "both"` on every dispatch they build
                // (`slash-commands.ts:997,1015,1045,1069`) — this flat entry point has no caller
                // that ever narrows the scope, so `Both` here is not a default guess but pi's own
                // explicit, always-supplied value for this exact call shape.
                agent_scope: AgentReadScope::Both,
                context,
                model_override,
                timeout_ms,
                cancel: CancelToken::new(),
            },
            None,
        )
        .await
        .map(|(result, _run_id)| result)
    }

    /// C19 (live foreground progress): the same foreground single run as [`run_foreground`], but
    /// STREAMING live progress through the host [`ToolUpdateSink`] as the child's NDJSON stdout
    /// arrives — the crate-side of pi's `onUpdate`/`fireUpdate` (`runs/foreground/execution.ts:478-499`).
    /// The tool call still blocks and still returns the same terminal [`SingleResult`]; the
    /// difference is that a still-running child no longer surfaces zero progress until completion.
    /// Each `tool_execution_start`/`tool_execution_end`/assistant `message_end` folds into a
    /// [`crate::tui::events::LiveProgressSnapshot`], is wrapped in a
    /// [`crate::tui::events::SubagentUpdatePayload`] (the `ToolUpdate.details` wire shape `cyrup-tui`
    /// renders as the inline subagent-result surface, C20), and is delivered through `on_update`.
    ///
    /// # Errors
    ///
    /// Identical to [`run_foreground`].
    ///
    /// Returns this run's own real, stable [`RunId`] alongside the result (pi `runId`,
    /// `subagent-executor.ts:1087-1091`) — the SAME id [`RunOptions::run_id`] threaded through the
    /// child's intercom-bridge registration — so a caller (`route_single`) can cite it verbatim in
    /// an out-of-band result-intercom payload/receipt (R-SA-123/124/125) rather than minting a
    /// second, disconnected id only for that message.
    pub async fn run_foreground_streaming(
        &self,
        req: ForegroundRunRequest<'_>,
        on_update: ToolUpdateSink,
    ) -> Result<(SingleResult, RunId), SubagentError> {
        self.run_foreground_impl(req, Some(on_update)).await
    }

    /// Shared body for [`run_foreground`] / [`run_foreground_streaming`]: resolves the persona +
    /// fork-context, builds the [`AgentConfig`]/[`RunOptions`], and drives [`crate::exec::run_sync`]
    /// — optionally installing a live-progress sink (`on_update = Some`, C19) that folds the child's
    /// NDJSON stream into [`crate::tui::events::SubagentUpdatePayload`] updates. Returns the run's own
    /// [`RunId`] alongside the [`SingleResult`] (see [`run_foreground_streaming`]'s doc).
    async fn run_foreground_impl(
        &self,
        req: ForegroundRunRequest<'_>,
        on_update: Option<ToolUpdateSink>,
    ) -> Result<(SingleResult, RunId), SubagentError> {
        let ForegroundRunRequest {
            overrides,
            cwd,
            agent_name,
            task,
            agent_scope,
            context,
            model_override,
            timeout_ms,
            cancel,
        } = req;
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // SUBA-003: the persona AND this cwd's effective `subagents.modelScope` policy come back
        // from ONE discovery pass, so the scope gating this run's model is the scope on disk right
        // now (pi `discoverAgents` -> `{ agents, modelScope }`, `agents.ts:1446`).
        let (agent, model_scope) =
            self.resolve_agent_with_model_scope(cwd, agent_name, agent_scope)?;
        // Fork default-mode (Tier-2, pi `resolveAgentDefaultContextPolicy`): an OMITTED call-site
        // `context` (`None`) falls back to THIS agent's own `default_context` rather than being forced
        // to `Fresh`; an explicit call-site value still wins (`resolve_effective_context`).
        let effective_context = resolve_effective_context(context, agent.default_context);
        let fork_context = self.resolve_context(cwd, effective_context).await?;
        // C19: the run's *resolved* context (R-SA-111) — captured before `fork_context` is moved
        // into `run_options` below — is what the live-progress payload's `[fork]` badge reflects.
        let resolved_context = fork_context.mode;

        let agent_config = AgentConfig::from_agent_definition(&agent, depth);
        // R-SA-038: `build_model_candidates` filters the ladder to `available_models`, so an
        // explicit `model` override (pi `slash-commands.ts:1001` `/run [model=…]`, and the tool's
        // SINGLE-mode `model`) must be ADDED to the availability set — otherwise the override is
        // silently filtered out and the child runs the agent's own default model instead of the
        // requested one. This mirrors `ExecSingleStepExecutor::run_single`, which likewise pushes
        // each step's `model` override into `available_models` before building the ladder.
        let mut available_models = agent_config
            .fallback_models
            .iter()
            .cloned()
            .chain(agent_config.model.clone())
            .collect::<Vec<_>>();
        if let Some(model) = &model_override {
            available_models.push(model.clone());
        }
        // Session-model inheritance (pi `resolveSubagentModelOverride((params.model) ?? a.model,
        // ctx.model, …)`, `subagent-executor.ts:1684`): when this run has NEITHER a per-call `model`
        // override NOR a persona `model:` of its own, inherit the live PARENT session model
        // (`HostServices::current_model`) as the primary candidate — otherwise an inheriting persona
        // has an EMPTY ladder and the run hard-fails with "no candidate model available"
        // (`exec/mod.rs`). `resolve_model_inheritance` both selects the effective override (per-call >
        // persona > inherited) and pushes the inherited id into `available_models` so it survives the
        // allowlist filter. `None` inherited (headless / no live session) degrades to the persona's
        // own `model`/`fallback_models` exactly as before.
        //
        // SUBA-003 fail-closed gate: when `subagents.modelScope.enforce` is armed and the caller
        // asked for a model no `allow` pattern matches, this returns `Err` and the run is REFUSED
        // here — before `deadline_at`, before the `RunId` is minted, and before any subprocess is
        // spawned. The violation is mapped to `SubagentError::ModelOutOfScope`, whose `Display` is
        // pi's verbatim message, so the caller (tool result / slash command) sees exactly WHY the
        // run did not happen instead of silently getting a different model's output.
        let effective_override = resolve_model_inheritance(
            model_override.as_ref(),
            agent_config.model.as_ref(),
            self.inherited_session_model().as_ref(),
            &mut available_models,
            model_scope.as_ref(),
        )
        .map_err(|violation| SubagentError::ModelOutOfScope(violation.message))?;

        // R-SA-035 / pi `resolveAttemptTimeout` (`execution.ts:91-99`): the orchestrator computes
        // the wall-clock `deadline_at` ONCE, here, from the nominal `timeout_ms` budget (pi
        // `deadlineAt ?? now + timeoutMs`), and threads BOTH down — `deadline_at` is what `run_sync`
        // races the child against; `timeout_ms` is what the timed-out message renders.
        let deadline_at =
            timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

        // The run id is minted BEFORE `run_options` so it can also identify the clarify/ask dispatch
        // context (R-SA-037/119/120) below; it doubles as the artifact-quadruple run id further down.
        let run_id = RunId::new();

        // T6 artifact quadruple config + root (pi `subagent-executor.ts:3387-3391`). Resolved HERE,
        // ahead of `run_options`, because pi derives the single-run output base directory from the
        // artifacts dir (`resolveSingleRunOutputBaseDir`, `:2203-2207`). SUBA-041: an explicit
        // `artifacts: false` turns the quadruple off — pi's `enabled: params.artifacts !== false`.
        let art_cfg = crate::artifacts::ArtifactConfig {
            enabled: overrides.artifacts != Some(false),
            ..crate::artifacts::ArtifactConfig::foreground()
        };
        let art_dir = crate::artifacts::temp_artifacts_dir(cwd);

        // SUBA-041 / pi `resolveSingleRunOutputBaseDir` (`subagent-executor.ts:2203-2207`): the
        // configured `singleRunOutputBaseDir` (tilde-expanded, `path.resolve`d) wins, else
        // `<artifactsDir>/outputs/<runId>`. This is the base a RELATIVE `output` resolves against —
        // deliberately NOT the run cwd, so a bare `report.md` never lands in the user's repo.
        let output_base_dir = match cfg.single_run_output_base_dir.as_deref() {
            Some(configured) => {
                let expanded = expand_tilde(&configured.to_string_lossy());
                resolve_against_process_cwd(&expanded).unwrap_or(expanded)
            }
            None => art_dir.join("outputs").join(run_id.as_str()),
        };
        // pi `runSinglePath` (`subagent-executor.ts:2789-2791,2882`): the persona's own `output:` is
        // the fallback for an omitted param and the referent of `output: true`; `outputMode` defaults
        // to `inline` from the PARAM alone (pi never consults the persona's own mode here).
        let output_path = resolve_single_output_path(
            normalize_single_output_override(
                overrides.output.as_ref(),
                agent
                    .output
                    .as_ref()
                    .and_then(|spec| spec.path.as_deref())
                    .and_then(Path::to_str),
            )
            .as_deref(),
            &output_base_dir,
        );
        let output_mode = parse_tool_output_mode(overrides.output_mode.as_deref())
            .unwrap_or(crate::discovery::types::OutputMode::Inline);

        // SUBA-041 / pi `subagent-executor.ts:3393-3401`: an explicit `sessionDir` is tilde-expanded
        // and `path.resolve`d and becomes the session ROOT verbatim; a configured
        // `default_session_dir` is instead scoped per run (`path.join(base, runId)`); the child's own
        // directory is then `<root>/run-0` (pi's `sessionDirForIndex(0)`).
        //
        // **[CYRUP-DELTA]** pi's third rung — `deps.getSubagentSessionRoot(parentSessionFile)`, an
        // always-present default derived from the PARENT session file — has no analog at this seam
        // (no parent-session-file plumbing reaches the extension), so with neither an explicit
        // `sessionDir` nor a configured default this stays `None` and
        // [`crate::exec::build_attempt_spawn_plan`] falls to pi's own `--no-session` branch
        // (`pi-args.ts:105-106`). The isolation outcome is the same one pi's scoped root buys: the
        // child never writes into the orchestrator's session store.
        let session_dir = overrides
            .session_dir
            .as_deref()
            .filter(|raw| !raw.is_empty())
            .map(|raw| {
                let expanded = expand_tilde(raw);
                resolve_against_process_cwd(&expanded).unwrap_or(expanded)
            })
            .or_else(|| {
                cfg.default_session_dir
                    .as_deref()
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| {
                        let expanded = expand_tilde(&path.to_string_lossy());
                        resolve_against_process_cwd(&expanded)
                            .unwrap_or(expanded)
                            .join(run_id.as_str())
                    })
            })
            .map(|root| root.join("run-0"));

        let run_options = RunOptions {
            cwd: cwd.to_path_buf(),
            deadline_at,
            timeout_ms,
            output_path,
            output_mode,
            structured_output_schema: None,
            model_override: effective_override,
            // SUBA-003: the same policy that just gated the explicit override, carried into
            // `run_sync` so the fallback ladder's own out-of-scope entries warn (pi
            // `execution.ts:1069`).
            model_scope,
            preferred_provider: None,
            available_models,
            // pi `execute(id, params, signal, ...)` threads the host's own `AbortSignal` into the
            // executor for every mode (`extension/index.ts:498-500` ->
            // `executeSubagentCollapsed:378-381`), so aborting the tool call drives the running
            // child through real SIGINT->SIGTERM->SIGKILL escalation instead of a token that can
            // never fire.
            cancel,
            interrupt: CancelToken::new(),
            // SUBA-041: pi's `shareEnabled` (`subagent-executor.ts:3354`) and `sessionDir`
            // (`:3393-3401`), both consumed by `build_attempt_spawn_plan`'s session branch.
            share: overrides.share,
            session_dir,
            // SUBA-041: the per-call `skill` override (pi `normalizeSkillInput(params.skill)`,
            // `subagent-executor.ts:2788`) replaces the agent's own `skills` list; `None` keeps the
            // pre-existing fallthrough (`run_sync` reads `opts.skills ?? agent.skills`). The
            // foreground single-run path still resolves against `cwd` alone (no distinct
            // orchestrator/runtime fallback cwd).
            skills: overrides.skills,
            runtime_cwd: None,
            include_progress: None,
            agent_scope: None,
            // SUBA-041: the per-call `acceptance` policy (pi `acceptance: params.acceptance`,
            // `subagent-executor.ts:2962`); `None` (an omitted param, or the explicit `"auto"`)
            // defers to `AcceptanceContract::heuristic_default` inside `run_sync` (R-SA-023).
            acceptance: overrides.acceptance,
            fork_context,
            live_events: None,
            // R-SA-P1: the EXPLICIT anchor — this root orchestrator session's own id, captured at
            // SessionStart via P-2. `None` when no live session id is available (headless / SDK
            // embedder), at which point the child spawn falls through to the inherited env value.
            parent_session_id: self.root_parent_session(),
            // R-SA-037/119/120: hand the executor's single-slot ask lock (backed by the intercom
            // companion's real broker `ClarifyChannel` when `with_channels` wired one, else the
            // no-live-channel degrade default) to the drive loop, so a child's blocking
            // `contact_supervisor` ask fires `spawn_clarify` and marks the attempt detached.
            clarify: Some(crate::tui::intercom::ClarifyDispatch {
                lock: self.clarify_lock(),
                session_key: self
                    .root_parent_session()
                    .unwrap_or_else(|| EXTENSION_ID.to_string()),
                run_id: run_id.clone(),
                step_index: None,
            }),
            // Intercom child-bridge activation (pi `pi-args.ts:201-214` via
            // `data.intercomBridge.orchestratorTarget`): thread THIS orchestrator's own presence
            // target + this run's id + child index 0 so the spawned child registers
            // `contact_supervisor` (addressed here) + a broker presence under
            // `resolve_subagent_intercom_target(run_id, agent, 0)`. `None` target (headless / no live
            // intercom session) leaves the child un-bridged — the clean no-intercom path.
            orchestrator_intercom_target: self.orchestrator_intercom_target(),
            run_id: Some(run_id.clone()),
            child_index: Some(0),
        };

        // pi `state.foregroundControls.set(runId, {interrupt, currentAgent, currentIndex})`
        // (`shared/types.ts` + `runs/foreground/execution.ts`): register this run's live control
        // surface BEFORE driving it, so a nested-control inbox listener polling in the SAME process
        // (a fanout child's own `foreground_controls`, `fanout-child.ts:53-128`) can resolve an
        // interrupt/resume request targeting this run's id while it is in flight. Shares the SAME
        // token `run_options.interrupt` races the running child's attempt loop against, so firing it
        // here genuinely soft-interrupts the live run rather than a disconnected flag.
        {
            let mut controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.insert(
                run_id.as_str().to_string(),
                ForegroundControlEntry {
                    interrupt: run_options.interrupt.clone(),
                    current_agent: Some(agent.name.clone()),
                    current_index: Some(0),
                },
            );
        }

        // T6 artifact quadruple (pi `runs/foreground/execution.ts:960-1074`): record this run's input
        // BEFORE spawning (so it survives a child crash), then its output/metadata/event-stream AFTER
        // the run settles. Written into the scoped-temp artifacts root for `cwd` (the Rust analog of
        // pi's `tempArtifactsDir = getArtifactsDir(null)`, `extension/index.ts:263`). Best-effort: a
        // failed artifact write never alters the `SingleResult` the caller observes. (`run_id`,
        // `art_cfg` and `art_dir` were all resolved above — `art_cfg.enabled` already honors
        // SUBA-041's `artifacts: false`, and `art_dir` doubles as the relative-output base root.)
        let art_paths =
            crate::artifacts::artifact_paths(&art_dir, run_id.as_str(), &agent.name, None);
        if art_cfg.enabled {
            let _ = crate::artifacts::ensure_artifacts_dir(&art_dir);
            if art_cfg.include_input {
                let _ = crate::artifacts::write_artifact(
                    &art_paths.input_path,
                    &format!("# Task for {}\n\n{task}", agent.name),
                );
            }
        }

        let result = drive_foreground_run_sync(
            &agent_config,
            task,
            run_options,
            &agent.name,
            resolved_context,
            on_update,
        )
        .await;

        // The run has settled (success, failure, or interrupted-terminal) — pi's foregroundControls
        // entry only exists while a run is live, so a nested-control request arriving after this
        // point must see a lookup miss ("is not active in this fanout child"), never a stale entry.
        {
            let mut controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.remove(run_id.as_str());
        }

        write_foreground_output_artifacts(&art_paths, &art_cfg, run_id.as_str(), &result);

        // R-SA-058: the per-attempt raw-stdout tee `run_sync` writes to
        // `<cwd>/.cyrup-subagent-scratch/attempt-<n>.jsonl` is this run's persisted, observable child
        // record and MUST survive the orchestrator, exactly as it does on every other spawn path in
        // this crate (the tool single/parallel/chain fan-outs and the background hop-2 runner all
        // leave it in place — it is the single observation channel the crate's integration tests read
        // back, e.g. `tool_parallel_chain_integration`'s `/run [model=…]` tee check and
        // `companions_wiring_proof`). This mirrors pi, which likewise never deletes its persisted
        // child NDJSON stream — pi only cleans the *transient* per-spawn prompt/task-overflow dir it
        // creates under `os.tmpdir()` (`pi-subagents/src/runs/shared/pi-args.ts:143-158` build it,
        // `:233-236` `cleanupTempDir` removes it, invoked from
        // `pi-subagents/src/runs/foreground/execution.ts:677`), a dir that lives OUTSIDE the working
        // tree and never holds the event stream. An earlier revision erroneously `remove_dir_all`'d
        // the whole `.cyrup-subagent-scratch` dir here, which silently discarded that tee the moment a
        // foreground `/run` completed — defeating the tee's own stated purpose and diverging from
        // every sibling path — so no such deletion is performed.

        Ok((result, run_id))
    }

    // ---------------------------------------------------------------------------------------
    // Background dispatch (the tool's `bg: true` shape; genuine second, detached OS-process hop)
    // ---------------------------------------------------------------------------------------

    /// Spawn one subagent task as a detached background run (func-SA §5.4; the tool's `bg: true`
    /// shape). Mints a [`RunId`], eagerly resolves fork-context (R-SA-137's eager whole-batch
    /// rule, degenerate single-task case), writes the one-shot `runner-config.json` handoff file
    /// (R-SA-073), and spawns hop 1 via [`spawn_detached_runner`] — a genuine SECOND, detached OS
    /// process (`cyrup __subagent-runner --config <path>`) that survives this orchestrator
    /// process's own exit (R-SA-070/071, DI-SA-8). Immediately tracks the new run
    /// ([`JobTracker::track`], R-SA-093) and returns without waiting for the run to complete
    /// (R-SA-074).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before agent discovery,
    /// fork-context resolution, run-directory creation, or the detached hop-1 spawn, so a blocked
    /// call touches none of that setup work and spawns nothing (not even the detached runner
    /// process itself). Otherwise returns [`SubagentError`] if the agent cannot be resolved,
    /// fork-context resolution fails hard, the run directory cannot be created, the one-shot
    /// config cannot be written, or the detached spawn itself fails.
    pub async fn spawn_background(
        &self,
        cwd: &Path,
        agent_name: &str,
        task: &str,
        context: Option<ContextMode>,
        model_override: Option<ModelId>,
        agent_scope: AgentReadScope,
    ) -> Result<RunId, SubagentError> {
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before agent discovery or
        // fork-context resolution below, and therefore also before `spawn_background_steps`' own
        // (correct, but too-late-for-THIS-call-site) independent re-check, since this function
        // itself performs real discovery/fork-context I/O ahead of ever delegating there.
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // R-SA-055: resolve the agent (and therefore validate it exists) before any spawn.
        // T0.1/C13: the SAME resolved definition is projected into the plan-time persona map handed
        // to the runner, so hop 2 dispatches this agent's REAL persona rather than a placeholder.
        let agent = self.resolve_agent(cwd, agent_name, agent_scope)?;
        let resolved_agents: BTreeMap<String, ResolvedAgentPersona> =
            BTreeMap::from([(agent_name.to_string(), crate::exec::resolve_step_agent_config(&agent))]);
        // Fork default-mode (Tier-2): an OMITTED call-site `context` falls back to THIS agent's own
        // `default_context` (pi `resolveAgentDefaultContextPolicy`), an explicit value still wins.
        let effective_context = resolve_effective_context(context, agent.default_context);
        // R-SA-137: eager fork-context resolution before ANY process is spawned for this batch.
        let fork_context = self.resolve_context(cwd, effective_context).await?;

        let step = SingleStepSpec {
            agent: agent_name.to_string(),
            task: task.to_string(),
            cwd: None,
            // pi `executeAsyncSingle` (`async-execution.ts:849-855`): `params.modelOverride ??
            // agent.model` reaches the detached runner's step unconditionally — a per-call
            // `model:` override on an async SINGLE run is never dropped just because the run is
            // background rather than foreground.
            model: model_override,
            tools: None,
            extensions: None,
            session_file: fork_context.session_file_path.clone(),
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: Some(effective_context),
            agent_scope: None,
        };

        self.spawn_background_steps(
            cwd,
            BackgroundStepsSpec {
                steps: vec![RunnerStep::SingleStep(step)],
                mode: RunMode::Single,
                session_file: fork_context.session_file_path,
                resolved_agents,
                // A single top-level task IS its own `{task}` value; a single run has no dedicated
                // chain scratch dir (`{chain_dir}` → the run cwd).
                original_task: task.to_string(),
                chain_dir: None,
            },
        )
        .await
    }

    /// Spawn an ARBITRARY already-resolved step list (`/chain`, `/parallel`, `/run-chain`'s `--bg`
    /// shape, R-SA-129/130) as a detached background run — the general form [`spawn_background`]
    /// itself is a thin single-step wrapper around. Mints a [`RunId`], writes the one-shot
    /// `runner-config.json` handoff file (R-SA-073), and spawns hop 1 via
    /// [`spawn_detached_runner`] exactly as [`spawn_background`] documents; the caller is
    /// responsible for having already resolved fork-context (R-SA-137's eager whole-batch rule)
    /// and for choosing `session_file` accordingly, since a multi-step chain's fork-context
    /// resolution is a per-call-site concern (a single top-level task fork-resolves once for
    /// itself; a chain fork-resolves once for its own first step) this shared helper does not
    /// itself re-derive.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055, SAFETY-CRITICAL) if this process's own
    /// recursion-depth ceiling is already reached — checked FIRST, before any run-directory
    /// creation or the detached hop-1 spawn, so a blocked call touches none of that setup work and
    /// spawns nothing (not even the detached runner process itself). Otherwise returns
    /// [`SubagentError`] if the run directory cannot be created, the one-shot config cannot be
    /// written, or the detached spawn itself fails.
    pub async fn spawn_background_steps(
        &self,
        cwd: &Path,
        spec: BackgroundStepsSpec,
    ) -> Result<RunId, SubagentError> {
        let BackgroundStepsSpec {
            steps,
            mode,
            session_file,
            resolved_agents,
            original_task,
            chain_dir,
        } = spec;
        let cfg = self.config_snapshot().await;
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before run-directory creation
        // or spawning the detached hop-1 process — since a background run is exactly as much a
        // "spawn" as a foreground one, and the resulting hop-2 runner process
        // (`background::runner_main::run`) will itself go on to spawn further real children for
        // every step in its own chain, each funneling through `exec::run_sync`'s own independent
        // re-check as defense in depth.
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        let run_id = RunId::new();

        // pi `executeAsyncChain`/`executeAsyncSingle` (`async-execution.ts:585-589,826-830`): a
        // background run started from WITHIN an already-nested run (this process inherited a nested
        // route via its own env, set by ITS OWN parent's spawn) reroutes its storage under that same
        // root's `nested-subagent-runs`/`nested` subtree, rather than becoming an indistinguishable
        // top-level run in the shared per-cwd async/results roots. A top-level (non-nested) run
        // resolves `None` here and keeps the C7 shared-roots derivation exactly as before.
        let inherited_nested_route =
            crate::spawn::nested_events::resolve_inherited_nested_route_from_env(|key| {
                std::env::var(key).ok()
            });
        let nested_address = inherited_nested_route.as_ref().and_then(|_| {
            crate::spawn::nested_events::resolve_nested_parent_address_from_env(|key| {
                std::env::var(key).ok()
            })
        });

        // C7: derive the two sibling roots ONCE from the shared source of truth and create them
        // (ensureAccessibleDir-equivalent), then pass their ABSOLUTE paths through `RunnerConfig`
        // so the detached runner writes its terminal ResultFile into the SAME `results_dir` this
        // orchestrator created and watches — never a re-derived, never-created divergent dir.
        let (async_root, results_dir) =
            resolve_background_storage_roots(cwd, inherited_nested_route.as_ref())?;
        crate::background::ensure_accessible_dir(&async_root)
            .await
            .map_err(SubagentError::Spawn)?;
        crate::background::ensure_accessible_dir(&results_dir)
            .await
            .map_err(SubagentError::Spawn)?;
        let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        crate::background::ensure_accessible_dir(&run_paths.run_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        // Captured before `steps` moves into `runner_config` below — pi's `flatAgents`/`firstAgents`
        // (`async-execution.ts:694-716,739-740`), needed only for the `subagent.nested.started`
        // event's `agent`/`agents`/`chainStepCount` fields.
        let event_agents = plan_step_agent_names(&steps);
        let event_step_count = i64::try_from(steps.len()).unwrap_or(i64::MAX);
        let event_mode_str = match mode {
            RunMode::Single => "single",
            RunMode::Parallel => "parallel",
            RunMode::Chain => "chain",
        };

        // Read before `cfg.worktree_base_dir` (a non-`Copy` `Option<PathBuf>`) is moved out of
        // `cfg` below by the struct literal — `dynamic_fanout_max_items()` takes `&self` on the
        // whole (by-then-partially-moved) `cfg`, so it must be evaluated first.
        let dynamic_fanout_max_items = cfg.dynamic_fanout_max_items();
        let runner_config = crate::background::runner_main::RunnerConfig {
            run_id: run_id.clone(),
            mode,
            steps,
            cwd: cwd.to_path_buf(),
            session_file,
            global_concurrency_limit: cfg.global_concurrency_limit as usize,
            worktree_base_dir: cfg.worktree_base_dir,
            max_subagent_depth: cfg.max_subagent_depth,
            async_root: async_root.clone(),
            results_dir: results_dir.clone(),
            // T0.1/C13: the plan-time persona map the orchestrator resolved (via
            // `resolve_plan_personas` / `exec::resolve_step_agent_config`) travels with the one-shot
            // config so the detached hop-2 runner dispatches each step's REAL persona and never
            // re-discovers or falls back to a placeholder `AgentConfig`.
            resolved_agents,
            // A (pi `originalTask`/`chainDir`): the run-wide `{task}` value + dedicated scratch chain
            // dir, resolved once by the orchestrator and serialized here so the detached runner
            // substitutes the SAME `{task}`/`{chain_dir}` the foreground path does.
            original_task,
            chain_dir,
            // Intercom child-bridge (pi `config.controlIntercomTarget`, `subagent-runner.ts:1823`):
            // this orchestrator's own presence target, resolved once here at plan time and carried
            // into the detached runner (which inherits no useful intercom env), so every step's
            // spawned child activates its `contact_supervisor` bridge addressed at this supervisor.
            // `None` (headless / no live intercom session) leaves each child un-bridged.
            orchestrator_intercom_target: self.orchestrator_intercom_target(),
            // Session-model inheritance (pi `ctx.model`): the live parent session model, resolved
            // once here at plan time and carried into the detached runner (which has no host-services
            // backend of its own), so a step whose persona declares no `model:` inherits the parent's
            // model rather than hard-failing on an empty ladder. `None` (headless / no live session)
            // leaves each inheriting step on its persona's own `model`/`fallback_models`.
            inherited_session_model: self.inherited_session_model(),
            // SUBA-003: the model-scope policy in force at authorization time, baked into the
            // one-shot config so the detached hop-2 runner enforces the SAME policy the foreground
            // path does. Without it, `subagent({..., background: true})` would be an unpoliced way
            // around an enforcing `modelScope`.
            model_scope: Self::resolve_model_scope(cwd)?,
            // Nested-route inheritance (pi `config.nestedRoute`/`config.nestedSelf`,
            // `async-execution.ts:672-678,914-920`): carried verbatim so the detached runner (were it
            // ever to relay ITS OWN descendants further, a later unit's concern) inherits the SAME
            // root route this orchestrator resolved, never re-reading env itself.
            nested_route: inherited_nested_route.clone(),
            nested_self: nested_address.clone(),
            // C16 (pi `config.chain.dynamicFanout.maxItems`): resolved once here at plan time and
            // carried into the detached runner so a background `DynamicGroup` step whose own
            // `expand.maxItems` is absent falls back to the SAME run-wide cap the foreground path
            // applies (`run_chain_foreground`), rather than always failing materialization.
            dynamic_fanout_max_items,
        };

        let cfg_path = run_paths.run_dir.join("runner-config.json");
        write_atomic_json(&cfg_path, &runner_config)
            .await
            .map_err(SubagentError::Spawn)?;

        let pid = spawn_detached_runner(
            &cfg_path,
            &run_paths.runner_stdout_log,
            &run_paths.runner_stderr_log,
        )?;

        // pi `executeAsyncChain`/`executeAsyncSingle` (`async-execution.ts:717-750,935-967`): once
        // hop 1's pid is CONFIRMED (never before — an unconfirmed spawn must not appear in the root's
        // nested registry at all), relay a `subagent.nested.started` event into the inherited route's
        // sink so the grandparent's `project_nested_events` projection can see this run without ever
        // having spawned it directly. Best-effort: a write failure is logged, never fatal to the
        // (already fully spawned) background run itself.
        if let (Some(route), Some(address)) = (&inherited_nested_route, &nested_address) {
            let now = i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
            )
            .unwrap_or(i64::MAX);
            let child = crate::spawn::nested_events::NestedRunSummary {
                id: run_id.as_str().to_string(),
                parent_run_id: address.parent_run_id.clone(),
                parent_step_index: address.parent_step_index,
                parent_agent: None,
                depth: address.depth,
                path: address.path.clone(),
                async_dir: Some(run_paths.run_dir.to_string_lossy().into_owned()),
                pid: Some(i64::from(pid)),
                session_id: None,
                session_file: None,
                intercom_target: None,
                owner_intercom_target: self.orchestrator_intercom_target(),
                // No per-step intercom-target concept is computed at this generic multi-step entry
                // point (pi's own `childIntercomTargets?.[0]`, resolved per named step) — left absent
                // rather than guessed.
                leaf_intercom_target: None,
                owner_state: Some("live".to_string()),
                control_inbox: None,
                capability_token: None,
                mode: Some(event_mode_str.to_string()),
                state: "running".to_string(),
                agent: event_agents.first().cloned(),
                agents: Some(event_agents.clone()),
                current_step: None,
                chain_step_count: Some(event_step_count),
                activity_state: None,
                last_activity_at: None,
                current_tool: None,
                current_tool_started_at: None,
                current_path: None,
                turn_count: None,
                tool_count: None,
                total_tokens: None,
                total_cost: None,
                started_at: Some(now),
                ended_at: None,
                last_update: Some(now),
                error: None,
                steps: None,
                children: None,
            };
            if let Err(err) = crate::spawn::nested_events::write_nested_event(
                route,
                &crate::spawn::nested_events::NestedEventInput {
                    event_type: "subagent.nested.started".to_string(),
                    ts: now,
                    parent_run_id: address.parent_run_id.clone(),
                    parent_step_index: address.parent_step_index,
                    child,
                },
            ) {
                tracing::warn!(error = %err, "failed to emit nested async start event");
            }
        }

        self.tracker
            .track(run_id.clone(), run_paths, Some(std::time::SystemTime::now()))
            .await;

        Ok(run_id)
    }

    // ---------------------------------------------------------------------------------------
    // Foreground chain/parallel dispatch (R-SA-130: `/chain`, `/parallel`, `/run-chain`'s
    // synchronous shape — the SAME `walk_chain`/`ExecSingleStepExecutor` machinery
    // `background::runner_main`'s hop-2 detached runner drives, reused rather than reimplemented)
    // ---------------------------------------------------------------------------------------

    /// Run an already-resolved [`RunnerStep`] list to completion in the foreground, synchronously
    /// (func-SA §5.1/§5.3; `/chain` and `/parallel`'s non-`--bg` shape). A bare `/parallel` call
    /// is represented as a ONE-element graph whose sole element is a
    /// [`RunnerStep::ParallelGroup`] — `walk_chain` dispatches that exactly like any other group
    /// step in a longer chain (R-SA-052: chain graphs and standalone parallel groups share the
    /// identical dispatch primitive, never a second parallel-only code path).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055) if this process's own recursion-depth
    /// ceiling is already reached — checked before any step is walked. Otherwise propagates
    /// [`walk_chain`]'s own errors (an unresolvable `DynamicGroup.expand` pointer, a
    /// `worktree: true` group whose setup failed, or a `worktree: true` group with no
    /// `worktree_base_dir` configured, R-SA-060..064).
    #[allow(clippy::too_many_arguments)]
    pub async fn run_chain_foreground(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        resolved_agents: BTreeMap<String, ResolvedAgentPersona>,
        original_task: String,
        chain_dir: Option<PathBuf>,
        cancel: CancelToken,
        // pi `chain-execution.ts:606`: `deadlineAt = params.deadlineAt ?? Date.now() + timeoutMs`,
        // computed ONCE here (never per step) and threaded, alongside the nominal `timeout_ms`
        // itself, into every step this walk dispatches via `ChainRunContext`. `None` (the tool gave
        // no `timeoutMs`/`maxRuntimeMs`, or this is a slash-command chain, which carries no timeout
        // param at all) means no chain-wide deadline, matching pi exactly.
        timeout_ms: Option<u64>,
    ) -> Result<(Vec<StepResult>, Vec<GroupStepResult>), SubagentError> {
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // T0.1/C13: hand the plan-time persona map straight to the foreground executor — the SAME
        // `ExecSingleStepExecutor` the hop-2 detached runner drives — so a foreground `/chain`//
        // `/parallel` step dispatches its REAL persona (never a placeholder), stamped with THIS
        // process's own live depth envelope at dispatch time (`ResolvedAgentPersona::to_agent_config`).
        // Intercom child-bridge activation for the foreground `/chain`//`/parallel` path (pi
        // `data.intercomBridge.orchestratorTarget`): mint a run id for this foreground walk and pass
        // this orchestrator's own presence target so each foreground-spawned child registers its
        // `contact_supervisor` bridge addressed at the live human orchestrator — the SAME activation
        // the background path gets via `RunnerConfig`. `None` target leaves each child un-bridged.
        let executor: Arc<dyn SingleStepExecutor> = Arc::new(ExecSingleStepExecutor::foreground(
            depth,
            Arc::new(resolved_agents),
            self.orchestrator_intercom_target(),
            Some(RunId::new()),
            // Session-model inheritance for foreground `/chain`//`/parallel` steps (pi `ctx.model`):
            // an inheriting step (no persona `model:`, no per-step override) runs the parent's live
            // model, the SAME inheritance the foreground single-run path applies.
            self.inherited_session_model(),
            // SUBA-003: the cwd's `subagents.modelScope` policy, so a foreground chain/parallel
            // step's own `model:` is policed exactly as a single run's `model` is.
            Self::resolve_model_scope(cwd)?,
        ));
        let global_limit = GlobalConcurrencyLimit::new(cfg.global_concurrency_limit.max(1) as usize);
        // R-SA-035/036 (pi `chain-execution.ts:606`): the chain-wide deadline is computed ONCE here,
        // before the walk starts, from the nominal `timeout_ms` budget the caller resolved
        // (`resolve_foreground_timeout`) — never re-derived per step, so it monotonically shrinks
        // across every step/group this walk dispatches. `None` when no timeout was requested.
        let deadline_at =
            timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
        // Read before `cfg.worktree_base_dir` (a non-`Copy` `Option<PathBuf>`) is moved out of
        // `cfg` below by the struct literal — `dynamic_fanout_max_items()` takes `&self` on the
        // whole (by-then-partially-moved) `cfg`, so it must be evaluated first.
        let dynamic_fanout_max_items = cfg.dynamic_fanout_max_items();
        let ctx = ChainRunContext {
            cwd: cwd.to_path_buf(),
            deadline_at,
            timeout_ms,
            // pi threads the host `AbortSignal` into the executor for every mode
            // (`extension/index.ts:498-500`), so an abort of the tool call must reach a
            // foreground `/chain`//`/parallel` walk's children too, not just SINGLE mode.
            cancel,
            global_limit,
            worktree_base_dir: cfg.worktree_base_dir,
            // A (pi `originalTask`/`chainDir`, `chain-execution.ts:493-497,1050`): the chain's real
            // top-level task + dedicated scratch chain dir, resolved once by the orchestrator
            // (`run_or_background_graph`) and threaded straight in, so a foreground `/chain` resolves
            // `{task}`/`{chain_dir}` to the SAME values the detached background runner does.
            original_task,
            chain_dir,
            // C16 (pi `config.chain.dynamicFanout.maxItems`): the SAME run-wide cap the background
            // path's `ChainRunContext` now also carries (via `RunnerConfig::dynamic_fanout_max_items`)
            // — a foreground `DynamicGroup` step whose own `expand.maxItems` is absent falls back to
            // this value instead of always failing materialization.
            dynamic_fanout_max_items,
        };
        let mut registry = OutputRegistry::new();
        walk_chain(&graph, &mut registry, &executor, &ctx).await
    }

    // ---------------------------------------------------------------------------------------
    // Shared chain/parallel plan execution (R-SA-130): the ONE path both the `subagent` tool's
    // `chain[]`/`tasks[]` shapes AND the `/chain`//`/parallel`//`/run-chain` slash commands funnel
    // through. Resolves every step's REAL persona at plan time (T0.1/C13), resolves fork-context
    // once for the whole batch (R-SA-137), then either walks the graph to completion in the
    // foreground or hands it to the detached hop-1 runner — never a second divergent code path.
    // ---------------------------------------------------------------------------------------

    /// Resolve personas + fork-context for `graph`, then run it foreground (walk to completion) or
    /// background (detached hop-1 runner), returning a structured [`GraphRunOutcome`] the CALLER
    /// renders (the slash commands render sequential/`N`-step text; the tool's PARALLEL mode renders
    /// pi's `N/M succeeded` summary — see `render_parallel_tool_summary`). Sharing this method is
    /// what lets the tool's `route_parallel_mode`/`route_chain_mode` reuse the identical
    /// persona-resolution + fork-context + walk machinery the slash surface already uses, rather
    /// than reimplementing it (R-SA-130).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::DepthExceeded`] (R-SA-055) when the recursion ceiling is already
    /// reached, [`SubagentError::AgentNotFound`] when any step names an unresolvable agent (fail
    /// fast at plan time, matching pi's upfront agent-name validation), or propagates fork-context /
    /// background-spawn / chain-walk errors.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_or_background_graph(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: Option<ContextMode>,
        background: bool,
        task: Option<String>,
        cancel: CancelToken,
        // pi `subagent-executor.ts:3022-3023`: a foreground-only timeout cannot be honored by a
        // detached background run. `None` for every slash-command caller (which exposes no timeout
        // param at all) and for `route_parallel_mode` (timeout wiring for bare PARALLEL is a
        // separate unit); `route_chain_mode` is the one caller that resolves a real value from the
        // tool's `timeoutMs`/`maxRuntimeMs` params.
        timeout_ms: Option<u64>,
    ) -> Result<GraphRunOutcome, SubagentError> {
        // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before persona resolution (real
        // discovery I/O) or fork-context resolution (real session I/O).
        let cfg = self.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth);
        if crate::spawn::depth::is_blocked(&depth) {
            return Err(SubagentError::DepthExceeded {
                current: depth.current_depth,
                max: depth.max_depth,
            });
        }

        // R-SA-053 (pi `chain-execution.ts:499-510`): validate EVERY chain's output bindings
        // (duplicate `as` names, malformed/unknown `{outputs.x}` references, dynamic-fanout `expand`
        // source) up front, before persona resolution, chain-dir creation, or ANY step is dispatched
        // — a tool `chain[]`/slash `/chain`//`/run-chain` graph gets the SAME upfront check a saved
        // chain file already gets at parse time (`discovery::chains::validate_chain_output_bindings`),
        // so a later-step defect fails immediately instead of only once an earlier step (which may
        // have already spawned real children and spent real tokens) reaches the bad reference.
        crate::spawn::chain_graph::validate_runner_step_output_bindings(&graph)
            .map_err(SubagentError::ChainOutputInvalid)?;

        // A (pi `originalTask`, `chain-execution.ts:493-497`): the run-wide `{task}` value — the
        // explicit call-site task if non-empty, else the graph's first step's first task. Resolved
        // ONCE here, the shared choke point, so BOTH the foreground walk and the detached background
        // runner substitute the identical `{task}`.
        let original_task = task
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| first_step_task(&graph));
        // A (pi `chainDir`, `chain-execution.ts:1050`): a dedicated per-run scratch directory under
        // the scoped chain-runs root, CREATED before dispatch so `{chain_dir}` resolves to an already-
        // existing directory on both the foreground and background paths (the detached runner only
        // substitutes the path string). Housekept by `artifacts::cleanup_old_chain_dirs`.
        //
        // This SAME id also identifies the run itself on the FOREGROUND path (pi `runId`,
        // `subagent-executor.ts:1087-1091`/`result-intercom.ts:255`): a foreground parallel/chain run
        // that attempts out-of-band intercom delivery must cite its own real run id in the payload/
        // receipt (`"Run: {runId}"`), never a second, disconnected id minted only for that message —
        // an orchestrator correlating a follow-up status/resume action against the id it just saw in
        // the receipt would otherwise find nothing. See [`GraphRunOutcome::Foreground::run_id`].
        let foreground_run_id = RunId::new();
        let chain_dir = crate::artifacts::chain_runs_dir(cwd).join(foreground_run_id.as_str());
        crate::background::ensure_accessible_dir(&chain_dir)
            .await
            .map_err(SubagentError::Spawn)?;

        // T0.1/C13: resolve every named persona up front (also the upfront agent-name validation —
        // an unresolvable agent fails here, before any child is spawned, matching pi's `/chain`//
        // `/parallel` name check).
        let resolved_agents =
            self.resolve_plan_personas(cwd, plan_step_agent_names(&graph), AgentReadScope::Both)?;
        // Fork default-mode + per-index branch (Tier-2, R-SA-137/R-SA-138, pi
        // `resolveAgentDefaultContextPolicy` + `preflightForkSessionsForStaticTasks`): resolve EACH
        // step's effective context independently (an omitted call-site `context` defers to THAT
        // step's agent's own `default_context`, never a batch-wide forced `Fresh`), then, for every
        // forking step, mint its OWN per-flat-index branch off a SINGLE shared resolver — two sibling
        // parallel tasks that both fork get two DISTINCT branch session files, not one shared branch.
        // `first_session_file` is the run-level session recorded for resume metadata only.
        // Blocker #4: branch every forking step from the REAL live-orchestrator session file (P-1),
        // not the continue_recent(cwd) mtime heuristic.
        let session_file = self.host_services().and_then(|s| s.session_file());
        let resolver = Self::fork_resolver(cwd, session_file.as_deref());
        let (graph, first_session_file) =
            apply_fork_contexts(&resolver, context, &resolved_agents, graph).await?;

        if background {
            let run_id = self
                .spawn_background_steps(
                    cwd,
                    BackgroundStepsSpec {
                        steps: graph,
                        mode,
                        session_file: first_session_file,
                        resolved_agents,
                        original_task,
                        chain_dir: Some(chain_dir),
                    },
                )
                .await?;
            Ok(GraphRunOutcome::Background(run_id))
        } else {
            // `is_group` must be computed BEFORE `graph` is moved into `run_chain_foreground` —
            // `group_results` is populated in chain order but NOT indexed by overall step position
            // (walk_chain's own doc), so a renderer needs both the graph's per-step shape and the
            // per-group child detail to zip them back together.
            let is_group: Vec<bool> = graph
                .iter()
                .map(|s| matches!(s, RunnerStep::ParallelGroup(_) | RunnerStep::DynamicGroup(_)))
                .collect();
            let (results, groups) = self
                .run_chain_foreground(
                    cwd,
                    graph,
                    resolved_agents,
                    original_task,
                    Some(chain_dir),
                    cancel,
                    timeout_ms,
                )
                .await?;
            Ok(GraphRunOutcome::Foreground {
                run_id: foreground_run_id,
                results,
                is_group,
                groups,
            })
        }
    }

    // ---------------------------------------------------------------------------------------
    // Saved-chain resolution (`/run-chain`, R-SA-129)
    // ---------------------------------------------------------------------------------------

    /// Resolve a saved chain by its fully-qualified name (R-SA-008-style exact string equality
    /// only — mirrors [`resolve_agent`]'s identical convention applied to chain names instead of
    /// agent names), via the real, on-demand, re-scanned-per-call discovery pipeline (R-SA-019).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::ChainNotFound`] if no discovered chain matches `name` exactly, or
    /// propagates a discovery-time [`SubagentError`] (R-SA-009's malformed-settings abort).
    pub fn resolve_chain(
        &self,
        cwd: &Path,
        name: &str,
    ) -> Result<crate::discovery::types::ChainDefinition, SubagentError> {
        let cfg = Self::discovery_config(cwd)?;
        let result = discover_agents(&cfg, None)?;
        // Cross-scope run precedence Project > User > Package > Builtin (pi `discoverSavedChains`
        // last-wins map, slash-commands.ts:1040) — NOT a naive first-match, which incorrectly let a
        // User chain shadow a same-named Project chain. See `discovery::resolve_chain_by_name`.
        crate::discovery::resolve_chain_by_name(&result.chains, name)
            .cloned()
            .ok_or_else(|| SubagentError::ChainNotFound(name.to_string()))
    }

    // ---------------------------------------------------------------------------------------
    // Registration surfaces: doctor / cost / profiles (delegates to already-implemented modules)
    // ---------------------------------------------------------------------------------------

    /// `/subagents-doctor` (R-SA-131; pi `buildDoctorReport`, doctor.ts:189-222): render the
    /// user-facing inventory report — a Runtime/session block, a Filesystem block naming the four
    /// scratch directories with each one's existence status, and a Discovery block with per-source
    /// agent/chain counts plus a skills inventory. This is pi's actual `/subagents-doctor` output
    /// (an inventory), distinct from [`crate::registration::doctor::DoctorRunner`]'s structured
    /// Ok/Warn/Fail check matrix (still available for programmatic diagnostics).
    ///
    /// `requested_session_dir` is pi's per-call `sessionDir` override (`paramsWithResolvedCwd.sessionDir`,
    /// `subagent-executor.ts:2828`) — an explicit value wins over the extension's own configured
    /// `default_session_dir`, which in turn wins over the literal `"not configured"` (pi
    /// `formatConfiguredSessionDir`, doctor.ts:108-116).
    pub async fn run_doctor(&self, cwd: &Path, requested_session_dir: Option<&str>) -> String {
        let roots = crate::background::run_artifact_roots(cwd);

        // pi wraps discovery in `lineFromCheck` (doctor.ts:64-70,131-153): a discovery failure (e.g.
        // R-SA-009's malformed-settings abort) must render `- agents/chains: failed — <err>` in the
        // Discovery block below, never a fabricated zero-count success — so the `Result` is
        // propagated all the way to `build_doctor_report`, never collapsed here.
        let discovery_result: Result<crate::discovery::AgentDiscoveryResult, String> =
            match Self::discovery_config(cwd) {
                Ok(discovery_config) => crate::discovery::discover_agents_all(&discovery_config)
                    .map_err(|err| err.to_string()),
                Err(err) => Err(err.to_string()),
            };

        // Session info: prefer the LIVE session manager (pi `ctx.sessionManager.getSessionFile()`/
        // `getSessionId()`, `subagent-executor.ts:2805-2813`) — the SAME live handle
        // `resolve_context` already uses (P-1) — over a per-cwd newest-mtime guess, which can name a
        // DIFFERENT session than the one the caller is actually in (another instance's newer
        // session, or a stale one). `root_parent_session` (captured once at this orchestrator's own
        // `SessionStart` from that same live `session_id()` call) is the state-held fallback pi's
        // `state.currentSessionId` plays (doctor.ts:124: `currentSessionId ?? state.currentSessionId
        // ?? "not available"`). Only when no live host is bound at all (headless/test) does this
        // degrade to the old newest-on-disk-by-mtime scan.
        let (session_file, session_id, session_error) = if let Some(services) = self.host_services()
        {
            let cached_id = self
                .root_parent_session
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            (services.session_file(), services.session_id().or(cached_id), None)
        } else {
            let sessions_dir = Self::sessions_dir(cwd);
            match crate::registration::cost::find_latest_session_file_by_mtime(&sessions_dir).await
            {
                Ok(Some(path)) => match cyrup_session::SessionManager::open(&path) {
                    Ok(manager) => (
                        Some(path),
                        Some(manager.session_id().as_str().to_string()),
                        None,
                    ),
                    Err(err) => (Some(path), None, Some(err.to_string())),
                },
                Ok(None) => (None, None, None),
                Err(err) => (None, None, Some(err.to_string())),
            }
        };

        let cfg = self.config_snapshot().await;
        let configured_session_dir = format_configured_session_dir(
            requested_session_dir,
            cfg.default_session_dir.as_deref(),
        );

        let input = DoctorReportInput {
            cwd,
            // A background/async run is a re-exec of this very binary; async is available whenever
            // the current executable path resolves (pi `isAsyncAvailable`'s cyrup analog).
            async_available: std::env::current_exe().is_ok(),
            configured_session_dir,
            current_session_file: session_file,
            current_session_id: session_id,
            session_error,
            temp_root_dir: crate::background::subagents_home(),
            async_runs_dir: roots.async_root,
            results_dir: roots.results_dir,
            chain_runs_dir: crate::artifacts::chain_runs_dir(cwd),
            discovered: discovery_result.as_ref().map_err(|err| err.as_str()),
        };
        build_doctor_report(&input)
    }

    /// The per-`cwd` session-storage directory (`<home>/.cyrup/sessions/<encoded cwd>`), the same
    /// layout [`Self::fork_resolver`] opens — factored out so `/subagents-doctor` and
    /// `/subagent-cost` locate the session transcript identically.
    fn sessions_dir(cwd: &Path) -> PathBuf {
        cyrup_session::SessionLayout::new(
            dirs_home().join(".cyrup").join("sessions"),
            cwd.to_path_buf(),
        )
        .dir()
    }

    /// `/subagent-cost` (R-SA-140; pi `buildSubagentCostReport`, slash-commands.ts:289-328): walk
    /// this session's TRANSCRIPT (not a background status file) and report the parent's own
    /// assistant-message usage plus a per-child breakdown of every subagent `toolResult` recorded in
    /// the branch — so foreground subagent usage (which never mints a background run) is visible.
    /// Reads the newest on-disk session for `cwd` (pi walks the live `ctx.sessionManager`; cyrup has
    /// no live manager threaded into this extension, so the faithful analog is the same on-disk read
    /// [`Self::fork_resolver`]/`run_doctor` already use). Delegates the actual walk + rendering to
    /// [`crate::registration::cost::build_subagent_cost_report`].
    pub async fn run_cost_report(&self, cwd: &Path) -> String {
        self.cost_report_from_sessions_dir(&Self::sessions_dir(cwd))
            .await
    }

    /// The testable core of [`Self::run_cost_report`]: given a resolved session-storage directory,
    /// open its newest `.jsonl` transcript READ-ONLY (never creating one) and render the cost report
    /// over its branch. An absent/empty session directory renders the well-formed empty-state report
    /// rather than an error.
    async fn cost_report_from_sessions_dir(&self, sessions_dir: &Path) -> String {
        let _ = self; // no executor state needed; a method purely for call-site symmetry/testability.
        match crate::registration::cost::find_latest_session_file_by_mtime(sessions_dir).await {
            Ok(Some(path)) => match cyrup_session::SessionManager::open(&path) {
                Ok(manager) => crate::registration::cost::build_subagent_cost_report(
                    manager.branch_path(None),
                ),
                Err(err) => format!(
                    "subagent-cost: could not open session {}: {err}",
                    path.display()
                ),
            },
            Ok(None) => crate::registration::cost::build_subagent_cost_report(
                std::iter::empty::<&cyrup_session::Entry>(),
            ),
            Err(err) => format!(
                "subagent-cost: could not scan session directory {}: {err}",
                sessions_dir.display()
            ),
        }
    }

    /// `/subagents-models` (pi `handleModels`, agent-management.ts:580-647; slash dispatch
    /// slash-commands.ts:1090-1111): report the RUNTIME builtin-agent -> model mapping — each
    /// discovered builtin persona's effective model + the provenance of that model — NOT a dump of
    /// the full static provider catalog. `requested_agent` filters to a single builtin (pi's
    /// single-agent form), erroring with the available-builtins list when the name is not a
    /// discovered builtin.
    ///
    /// The live PARENT session model IS now threaded into this extension — read from the bound P-1
    /// [`cyrup_ext::host::HostServices`] backend via [`Self::inherited_session_model`] (pi's
    /// `ctx.model`) — so the "Current session model" line and an inheriting persona's effective model
    /// render the REAL `provider/id` instead of "(unavailable)". A persona that declares its own
    /// `model` still shows that (frontmatter / settings override / settings default); a persona with
    /// no model shows the inherited session model when a live session is bound, and only degrades to
    /// "(unavailable)"/"(inherits current session model)" when there is genuinely no live host
    /// (headless / SDK-embedder / no active model yet).
    #[must_use]
    pub fn run_models_report(&self, cwd: &Path, requested_agent: Option<&str>) -> String {
        // The live parent session model (pi `ctx.model`) an inheriting builtin resolves to; `None`
        // when no live session backend is bound (headless / SDK-embedder) — then the display degrades
        // to "(unavailable)" exactly as before this seam existed.
        let current_model = self.inherited_session_model().map(|m| m.as_str().to_string());
        let current_model = current_model.as_deref();
        // pi `ctx.model?.provider` (agent-management.ts:588) / the `ParentModel` a `model: undefined`
        // (or the `"inherit"` sentinel) resolves to (`resolveSubagentModelOverride`,
        // model-fallback.ts:47-59): both split off the SAME live `provider/id` string.
        let preferred_provider = current_model
            .and_then(|m| m.split_once('/'))
            .map(|(provider, _)| provider);
        let parent_model = current_model.and_then(|m| m.split_once('/'));
        let available_models = registry_available_models();

        let cfg = Self::discovery_config(cwd).unwrap_or_else(|_| Self::discovery_dirs_config(cwd));
        let default_model_scope = resolve_default_model_scope(&cfg.override_settings);
        let discovered = match crate::discovery::discover_agents_all(&cfg) {
            Ok(discovered) => discovered,
            Err(err) => return format!("subagents-models: discovery failed: {err}"),
        };

        let builtin_by_name: std::collections::HashMap<&str, &AgentDefinition> = discovered
            .agents
            .iter()
            .filter(|agent| agent.source == AgentSource::Builtin)
            .map(|agent| (agent.name.as_str(), agent))
            .collect();

        // pi `params.agent?.trim()` (agent-management.ts:581): a whitespace-only/empty `agent`
        // string is JS-falsy and treated as "no agent requested" — it falls through to the
        // all-agents view below, it is NOT looked up as a builtin named "".
        let requested_agent = requested_agent.map(str::trim).filter(|s| !s.is_empty());

        if let Some(requested) = requested_agent {
            // pi's first gate (agent-management.ts:581-583) checks the STATIC `BUILTIN_AGENT_NAMES`
            // list, not whatever discovery happened to find.
            if !BUILTIN_AGENT_NAMES.contains(&requested) {
                return format!(
                    "Builtin agent '{requested}' not found. Available: {}.",
                    BUILTIN_AGENT_NAMES.join(", ")
                );
            }
            // pi's second gate (agent-management.ts:589-590): the name is a real builtin name, but
            // discovery didn't resolve it (a broken/incomplete build) — a DIFFERENT message with no
            // "Available" suffix, since the name was already validated above.
            let Some(agent) = builtin_by_name.get(requested).copied() else {
                return format!("Builtin agent '{requested}' not found.");
            };

            let requested_model_str = agent.model.as_ref().map(ModelId::as_str);
            let resolved_model = resolve_subagent_model_override(
                requested_model_str,
                parent_model,
                &available_models,
                preferred_provider,
            );
            let mut lines = vec![
                "Builtin subagent model".to_string(),
                String::new(),
                format!("Agent: {requested}"),
                "Effective model:".to_string(),
                format!("  {}", resolved_model.as_deref().unwrap_or("(unresolved)")),
                format!(
                    "Source: {}",
                    format_model_source(agent, current_model, default_model_scope)
                ),
            ];
            if let Some(override_info) = &agent.override_info {
                lines.push("Override file:".to_string());
                lines.push(format!("  {}", override_info.settings_path.display()));
            }
            // pi `agent.model && resolvedModel && agent.model !== resolvedModel`
            // (agent-management.ts:596-599): only shown when the persona declared a raw setting AND
            // it differs from the resolved model (e.g. a bare id resolved to its full `provider/id`,
            // or an explicit override that isn't the resolved value).
            if let (Some(raw), Some(resolved)) = (requested_model_str, resolved_model.as_deref())
                && raw != resolved
            {
                lines.push("Requested model setting:".to_string());
                lines.push(format!("  {raw}"));
            }
            if agent.disabled == Some(true) {
                lines.push("Disabled: true".to_string());
            }
            lines.push("Current session model:".to_string());
            lines.push(format!("  {}", current_model.unwrap_or("(unavailable)")));
            return lines.join("\n");
        }

        let mut lines = vec![
            "Builtin subagent models".to_string(),
            String::new(),
            "Current session model:".to_string(),
            format!("  {}", current_model.unwrap_or("(unavailable)")),
            String::new(),
        ];
        // pi's all-agents view walks the fixed `BUILTIN_AGENT_NAMES` list (agent-management.ts:608),
        // not whatever discovery happened to find — a name discovery didn't resolve gets its own
        // "missing" row rather than silently shrinking the report.
        for name in BUILTIN_AGENT_NAMES {
            let Some(agent) = builtin_by_name.get(name).copied() else {
                lines.push(name.to_string());
                lines.push("  model:".to_string());
                lines.push("    (builtin definition not found)".to_string());
                lines.push("  source: missing".to_string());
                lines.push(String::new());
                continue;
            };
            let requested_model_str = agent.model.as_ref().map(ModelId::as_str);
            let resolved_model = resolve_subagent_model_override(
                requested_model_str,
                parent_model,
                &available_models,
                preferred_provider,
            );
            let disabled_suffix = if agent.disabled == Some(true) {
                "; disabled"
            } else {
                ""
            };
            lines.push(name.to_string());
            lines.push("  model:".to_string());
            lines.push(format!(
                "    {}",
                resolved_model.as_deref().unwrap_or("(unresolved)")
            ));
            lines.push(format!(
                "  source: {}{disabled_suffix}",
                format_model_source(agent, current_model, default_model_scope)
            ));
            lines.push(String::new());
        }
        lines.join("\n")
    }

    /// Resume background-run tracking from disk (R-SA-093's "resume on session start" note in
    /// `on_event`'s own doc): re-discover any run directories still present under this cwd's
    /// `AsyncRoot` from a prior process and re-track them, so a restarted orchestrator does not
    /// lose visibility into still-running detached runs.
    ///
    /// Mirrors pi's `restoreActiveJobs` (`async-job-tracker.ts:405-420`) exactly: only runs whose
    /// RECONCILED state is `queued` or `running` are re-tracked — a run that has already reached a
    /// terminal state (`complete`/`failed`/`paused`) by the time this process restarts is NOT
    /// re-tracked (pi's own `listAsyncRuns({ states: ["queued", "running"] })` filter), and each
    /// restored job's `events.jsonl` byte cursor is seeded from the file's CURRENT size (pi's
    /// `restoredControlEventCursor`, ENOENT → 0) so historical control events already written before
    /// this process existed are never re-tailed. A `read_dir` failure on the `AsyncRoot` itself is
    /// logged (pi's `console.error` in the listing `catch`) rather than silently swallowed.
    pub async fn resume_tracking(&self, cwd: &Path) {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let mut entries = match tokio::fs::read_dir(&async_root).await {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    async_root = %async_root.display(),
                    "failed to restore active async jobs: could not list AsyncRoot"
                );
                return;
            }
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        async_root = %async_root.display(),
                        "failed to restore active async jobs: error reading AsyncRoot entry"
                    );
                    break;
                }
            };
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let run_id = RunId::from_token(name);
            let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

            // Only queued/running runs are restored (pi: `listAsyncRuns({ states: ["queued",
            // "running"] })`) — reconcile first so a run that is claimed-Running-but-actually-dead
            // is correctly classified as terminal (Failed) rather than spuriously re-tracked.
            let Ok(outcome) = crate::background::reconcile::reconcile_now(&paths, None).await
            else {
                continue;
            };
            if !matches!(outcome.status.state, RunState::Queued | RunState::Running) {
                continue;
            }

            // Seed the events cursor at the file's CURRENT size (pi: `restoredControlEventCursor`)
            // so this process never re-tails control events a prior process already consumed.
            let events_cursor = match tokio::fs::metadata(&paths.events).await {
                Ok(metadata) => metadata.len(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        events_path = %paths.events.display(),
                        "failed to stat events.jsonl while restoring async job; seeding cursor at 0"
                    );
                    0
                }
            };

            self.tracker
                .track_restored(run_id, paths, events_cursor)
                .await;
        }
    }

    // ---------------------------------------------------------------------------------------
    // Background control actions (C5): status / interrupt / resume / append-step
    //
    // Each method is the executor half of one `subagent` control action (pi
    // `subagent-executor.ts:2845-2912`), routing to the faithful [`crate::background::control`]
    // primitives + the [`crate::background::run_status`] report shape. They return
    // `Result<String, String>`: `Ok` is the rendered report/confirmation the caller shows as tool
    // content; `Err` is the user-facing failure message the tool surface turns into a `ToolError`
    // (cyrup's `ToolResult` has no `isError` flag, so a soft user-facing error is an `Err(text)`).
    // ---------------------------------------------------------------------------------------

    /// `action: "status"` (C5): render the no-id "list active runs" view, or a single run's full
    /// per-step report resolved by `id` (exact or unique prefix) or by `dir` — pi
    /// `subagent-executor.ts:2845-2863` + `run-status.ts:101-273`.
    ///
    /// # Errors
    ///
    /// Returns the not-found notice (or a resolution/reconciliation error message) as `Err`.
    pub async fn control_status(
        &self,
        cwd: &Path,
        id: Option<&str>,
        dir: Option<&str>,
        child_safe: bool,
    ) -> Result<String, String> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        // pi precedence (`run-status.ts:131`): a bare `id` (no `dir`) resolves by id; otherwise a
        // present `dir` resolves the directory directly; otherwise (neither) list active runs.
        match (id, dir) {
            (Some(id), None) => run_status::inspect_status_by_id(&async_root, &results_dir, id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Async run not found. Provide id or dir.".to_string()),
            (_, Some(dir)) => run_status::inspect_status_by_dir(Path::new(dir), &results_dir)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Async run not found. Provide id or dir.".to_string()),
            (None, None) => {
                // pi `run-status.ts:104-110`: the child-safe fanout tool (`deps.nested` truthy,
                // which pi sets whenever `allowMutatingManagementActions === false`) never lists
                // the cwd's active runs on a no-id status call — it hard-errors, since a fanout
                // child has no business enumerating its parent's whole async root.
                if child_safe {
                    return Err(
                        "Child-safe subagent status requires an id when no foreground run is \
                         active."
                            .to_string(),
                    );
                }
                let runs = run_status::list_active_runs(&async_root, &results_dir)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(run_status::format_run_list(&runs))
            }
        }
    }

    /// `action: "interrupt"` (C5): deliver a soft, resumable interrupt (R-SA-084 — a *pause*
    /// request, never a kill) to the target async run, or, with no id, to the most-recently-updated
    /// running run in this cwd's async root — pi `subagent-executor.ts:2871-2911`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if no interrupt-capable run is found, if the target is not Running (R-SA-079),
    /// or if the underlying delivery fails.
    pub async fn control_interrupt(&self, cwd: &Path, target: Option<&str>) -> Result<String, String> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let run_id = match target {
            Some(explicit) => explicit.to_string(),
            None => {
                // No id: interrupt the most-recently-updated running run (the list is already sorted
                // running-first, most-recent-first), mirroring pi's "defaults to the most recently
                // active controllable run" contract for interrupt.
                let runs = run_status::list_active_runs(&async_root, &results_dir)
                    .await
                    .map_err(|e| e.to_string())?;
                runs.iter()
                    .find(|run| run.status.state == RunState::Running)
                    .map(|run| run.status.run_id.as_str().to_string())
                    .ok_or_else(|| "No interrupt-capable run found in this session.".to_string())?
            }
        };
        match control::interrupt(&async_root, &results_dir, &run_id, "interrupt-action", None).await {
            Ok(InterruptOutcome::Delivered | InterruptOutcome::AlreadyPending) => {
                Ok(format!("Interrupt requested for async run {run_id}."))
            }
            Ok(InterruptOutcome::NotRunning) => Err(format!(
                "No running async run with an interrupt-capable pid was found for '{run_id}'."
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    /// `action: "resume"` (C5): the R-SA-085/086 fork — steer a still-running run's live child, or
    /// revive a terminal run from its persisted transcript — pi `subagent-executor.ts:2865`/
    /// `801-1031`. Requires a follow-up `message` (falling back to `task`) and a run `id`.
    ///
    /// The running-selection branch interrupts the live child, then DELIVERS the follow-up over the
    /// broker to that child's deterministic registered bridge target — pi
    /// `deliverSubagentIntercomMessageEvent(events, target.intercomTarget, …)`
    /// (`subagent-executor.ts:848-878`). The child WAS activated as a bridge participant at its spawn
    /// (the subagents spawn overlay writes `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET`/`_RUN_ID`/
    /// `_CHILD_AGENT`/`_CHILD_INDEX`/`_INTERCOM_SESSION_NAME`, so the child's `IntercomExtension`
    /// registered `contact_supervisor` + a broker presence under
    /// `resolve_subagent_intercom_target(run_id, agent, index)`), so this arm recovers that same
    /// target from the reconciled run status (`steps[step_index].agent` + the step index) and steers
    /// it via the [`crate::tui::intercom::SteerChannel`] threaded in by
    /// `SubagentsExtension::with_channels`. pi's "intercom target is not registered" guidance is
    /// returned ONLY as the genuine delivery-FAILED fallback (no live broker, or no registered
    /// receiver at that target) — the caller then waits for the pause and retries, hitting the
    /// terminal-revival branch. The terminal-revival branch respawns a fresh detached child seeded
    /// from the transcript, running the run's REAL resolved persona (T0.1/C13), and hard-fails (no
    /// silent fresh-session fallback) when no transcript exists.
    ///
    /// # Errors
    ///
    /// Returns `Err` for a missing message/id, the delivery-failed intercom-unregistered live-steer
    /// notice, a no-transcript revival, or any resolution/spawn failure.
    pub async fn control_resume(
        &self,
        cwd: &Path,
        target: Option<&str>,
        message: Option<&str>,
        task: Option<&str>,
        index: Option<usize>,
    ) -> Result<String, String> {
        let follow_up = message.or(task).map(str::trim).unwrap_or_default();
        if follow_up.is_empty() {
            return Err("action='resume' requires message.".to_string());
        }
        let Some(run_id) = target else {
            return Err("action='resume' requires id.".to_string());
        };
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        match control::resume(&async_root, &results_dir, run_id, index).await {
            Ok(ResumeOutcome::SteerRunning { step_index }) => {
                // pi (`subagent-executor.ts:848-878`): interrupt the live child, then DELIVER the
                // follow-up over the broker to that child's registered bridge target. Recover the
                // child's deterministic target from the reconciled run status — the resumed step's
                // REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let source_paths = RunPaths::for_run(
                    &async_root,
                    &results_dir,
                    &RunId::from_token(run_id.to_string()),
                );
                // pi `interruptLiveAsyncResumeTarget` (`background/async-resume.ts:53-56`):
                // re-reconcile and REQUIRE `status.state === "running"` with a numeric pid before
                // even attempting to interrupt — a reconciliation failure, a run that is no longer
                // Running, or a Running status with no known runner pid all abort the WHOLE resume
                // with this exact diagnostic, rather than silently falling through to steer a child
                // that was never confirmed interruptible.
                let status = match control::reconcile_before_control_op(&source_paths).await {
                    Ok(status) if status.state == RunState::Running && status.pid.is_some() => {
                        status
                    }
                    _ => {
                        return Err(format!(
                            "Async run {run_id} is live but no interrupt-capable runner pid was \
                             found."
                        ));
                    }
                };
                // Recover the child's deterministic target from the reconciled run status — the
                // resumed step's REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let (child_target, child_agent) = match status.steps.get(step_index) {
                    Some(step) => (
                        Some(crate::spawn::intercom_target::resolve_subagent_intercom_target(
                            run_id,
                            &step.agent,
                            step_index,
                        )),
                        Some(step.agent.clone()),
                    ),
                    None => (None, None),
                };
                // Interrupt the live child (genuine), matching pi's interrupt-then-deliver order
                // (`subagent-executor.ts:846-859`): a FAILED interrupt is returned as the error
                // result immediately, before any follow-up delivery is attempted — it must never be
                // silently swallowed and fall through to steering a child that may still be running
                // its prior turn.
                if let Err(e) =
                    control::interrupt(&async_root, &results_dir, run_id, "async-resume", None)
                        .await
                {
                    return Err(format!("Failed to interrupt async run {run_id}: {e}"));
                }
                // pi's follow-up header includes the resolved agent name (`subagent-executor.ts:863`:
                // `Follow-up for async run ${target.runId} (${target.agent}):`).
                let follow_up_message = match &child_agent {
                    Some(agent) => format!("Follow-up for async run {run_id} ({agent}):\n\n{follow_up}"),
                    None => format!("Follow-up for async run {run_id}:\n\n{follow_up}"),
                };
                // pi's `deliverSubagentIntercomMessageEvent` bounds EVERY caller (including this
                // live-child follow-up steer, `subagent-executor.ts:860`) to a 500ms default timeout
                // race — the caller's own turn is never blocked longer than that waiting on a
                // delivery ack (`result-intercom.ts:283-316`). Race the raw `SteerChannel::steer`
                // call against that same bound rather than awaiting it unbounded.
                let delivered = match &child_target {
                    Some(target) => {
                        crate::tui::intercom::steer_with_default_timeout(
                            self.steer.as_ref(),
                            target.clone(),
                            follow_up_message,
                        )
                        .await
                    }
                    None => false,
                };
                if delivered {
                    // pi's delivered-follow-up confirmation (`subagent-executor.ts:868-871`).
                    Ok(format!(
                        "Interrupted live async child, then delivered follow-up.\n\
                         Run: {run_id}\n\
                         Intercom target: {}",
                        child_target.unwrap_or_default()
                    ))
                } else {
                    // Delivery-FAILED fallback ONLY (no live broker, or no registered receiver at the
                    // target) — pi's exact intercom-unregistered guidance
                    // (`subagent-executor.ts:873-877`).
                    let target_line = child_target
                        .map(|t| format!("Intercom target: {t}\n"))
                        .unwrap_or_default();
                    Err(format!(
                        "Async child appears live but its intercom target is not registered.\n\
                         Run: {run_id}\n\
                         {target_line}Wait for completion, then retry action='resume'."
                    ))
                }
            }
            Ok(ResumeOutcome::RespawnFromTranscript { step_index, session_file }) => self
                .revive_from_transcript(cwd, run_id, step_index, &session_file, follow_up)
                .await
                .map_err(|e| e.to_string()),
            Err(SubagentError::ResumeNoTranscript) => Err(format!(
                "Resume unavailable: async run '{run_id}' has no persisted transcript to revive \
                 from."
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The terminal-revival spawn half of [`Self::control_resume`] (R-SA-085): read the source run's
    /// reconciled status to recover the revived step's agent, resolve its REAL persona (never a
    /// placeholder, C13), and spawn a fresh detached background single-run seeded from
    /// `session_file` (`executeAsyncSingle` in pi, `subagent-executor.ts:987`).
    async fn revive_from_transcript(
        &self,
        cwd: &Path,
        source_run_id: &str,
        step_index: usize,
        session_file: &Path,
        follow_up: &str,
    ) -> Result<String, SubagentError> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let source_paths = RunPaths::for_run(
            &async_root,
            &results_dir,
            &RunId::from_token(source_run_id.to_string()),
        );
        let status = control::reconcile_before_control_op(&source_paths).await?;
        let agent = status
            .steps
            .get(step_index)
            .map(|step| step.agent.clone())
            .ok_or_else(|| {
                SubagentError::AgentNotFound(format!("no step at index {step_index} to revive"))
            })?;
        // pi `effectiveCwd = target.cwd ?? requestCwd` (`subagent-executor.ts:890`, fed by
        // `target.cwd` = `status.cwd ?? result.cwd`, `background/async-resume.ts:373`): the revived
        // child's persona discovery AND its actual spawn cwd prefer the ORIGINAL run's own working
        // directory (persisted onto the reconciled status by `finish_run`,
        // `background/runner_main.rs`) over whatever cwd happens to be current at resume time —
        // never silently reroute a revived agent into a different directory than the one it was
        // originally invoked from.
        let effective_cwd = status.cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
        let resolved_agents =
            self.resolve_plan_personas(&effective_cwd, [agent.clone()], AgentReadScope::Both)?;
        let revived_task =
            Self::build_revived_async_task(source_run_id, &agent, session_file, follow_up);
        let step = SingleStepSpec {
            agent: agent.clone(),
            task: revived_task,
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: Some(session_file.to_path_buf()),
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: Some(ContextMode::Fork),
            agent_scope: None,
        };
        let new_id = self
            .spawn_background_steps(
                &effective_cwd,
                BackgroundStepsSpec {
                    steps: vec![RunnerStep::SingleStep(step)],
                    mode: RunMode::Single,
                    session_file: Some(session_file.to_path_buf()),
                    resolved_agents,
                    // The revival's follow-up is its `{task}`; a single revived run has no chain dir.
                    original_task: follow_up.to_string(),
                    chain_dir: None,
                },
            )
            .await?;
        // pi's confirmation (`subagent-executor.ts:1019-1029`): a source label ("foreground" /
        // "async" / "nested" — cyrub's `control::resume` only ever revives an async source today, so
        // this is always "async" here), then the intercom-target line ONLY when a real bridge is
        // wired (pi `intercomBridge.active`), matching `NoTransportSteerChannel::is_active` ==
        // `false` degrading to omitting the line entirely rather than showing a target nothing will
        // ever deliver to.
        let intercom_target_line = if self.steer.is_active() {
            let target = crate::spawn::intercom_target::resolve_subagent_intercom_target(
                new_id.as_str(),
                &agent,
                0,
            );
            format!("Intercom target: {target} (if registered)\n")
        } else {
            String::new()
        };
        Ok(format!(
            "Revived async subagent from {source_run_id}.\n\
             Revived run: {new_id}\n\
             Agent: {agent}\n\
             Session: {}\n\
             {intercom_target_line}Status if needed: subagent({{ action: \"status\", id: \"{new_id}\" }})",
            session_file.display()
        ))
    }

    /// pi `buildRevivedAsyncTask` (`background/async-resume.ts:378-391`): the revival framing wrapped
    /// AROUND the orchestrator's raw follow-up, rather than sending the follow-up verbatim as the
    /// revived child's `{task}` — the revived agent otherwise has no way to know it is being resumed
    /// from a stored transcript rather than starting fresh.
    fn build_revived_async_task(
        source_run_id: &str,
        agent: &str,
        session_file: &Path,
        follow_up: &str,
    ) -> String {
        let lines: Vec<String> = vec![
            "You are reviving a previous subagent conversation.".to_string(),
            String::new(),
            format!("Original run: {source_run_id}"),
            format!("Original agent: {agent}"),
            format!("Original session file: {}", session_file.display()),
            String::new(),
            "Use the stored session context as background. Answer the orchestrator's follow-up \
             below. Do not assume the original child process is still alive."
                .to_string(),
            String::new(),
            "Follow-up:".to_string(),
            follow_up.to_string(),
        ];
        lines.join("\n")
    }

    /// `action: "append-step"` (C5): validate and enqueue exactly one new step onto a running async
    /// chain (R-SA-094/095/096) — pi `subagent-executor.ts:2868`/`508-686`. The appended agent is
    /// resolved through real discovery first (fail-fast on an unknown agent, matching pi's
    /// `buildAsyncRunnerSteps`), then the step is enqueued via [`crate::background::control::append_step`].
    ///
    /// # Errors
    ///
    /// Returns `Err` for a missing id, a chain that is not exactly one step, an unknown agent, or a
    /// primitive-level rejection (wrong mode/state, output-name collision).
    pub async fn control_append_step(
        &self,
        cwd: &Path,
        target: Option<&str>,
        chain: &[serde_json::Value],
    ) -> Result<String, String> {
        let Some(run_id) = target else {
            return Err("action='append-step' requires id.".to_string());
        };
        if chain.len() != 1 {
            return Err("action='append-step' requires chain with exactly one step.".to_string());
        }
        let Some(step_val) = chain.first() else {
            return Err("action='append-step' requires chain with exactly one step.".to_string());
        };
        let Some(agent) = step_val.get("agent").and_then(serde_json::Value::as_str) else {
            return Err("action='append-step' chain step requires an 'agent' field.".to_string());
        };
        let task = step_val
            .get("task")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let output = step_val
            .get("output")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // pi validates every appended agent exists before enqueuing (`buildAsyncRunnerSteps` errors
        // on an unknown agent name); resolve it via real discovery for the same fail-fast behavior.
        self.resolve_agent(cwd, agent, AgentReadScope::Both)
            .map_err(|e| format!("Cannot append step to run '{run_id}': {e}"))?;
        let step = SingleStepSpec {
            agent: agent.to_string(),
            task,
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        };
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        match control::append_step(
            &async_root,
            &results_dir,
            run_id,
            vec![RunnerStep::SingleStep(step)],
        )
        .await
        {
            Ok(AppendOutcome::Enqueued { .. }) => {
                let paths = RunPaths::for_run(
                    &async_root,
                    &results_dir,
                    &RunId::from_token(run_id.to_string()),
                );
                let pending = control::count_pending_appends(&paths.append_dir)
                    .await
                    .unwrap_or(1);
                Ok(format!(
                    "Append queued for chain run {run_id}: 1 step. It becomes eligible after the \
                     chain's already-queued steps finish. Pending appends: {pending}."
                ))
            }
            Err(e) => Err(e.to_string()),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Nested-control inbox listener (T6, pi `fanout-child.ts:53-128`): serviced ONLY by a
    // `RegistrationMode::ChildSafe` process that inherited a nested route from its own parent's
    // env — a grandparent orchestrator's interrupt/resume request targeting a run nested two (or
    // more) levels deep is routed here rather than lost.
    // ---------------------------------------------------------------------------------------

    /// Start the listener (pi `startNestedControlInboxListener`, `fanout-child.ts:53-63,125-128`):
    /// resolve the inherited nested route from the process env — a resolution error is swallowed
    /// (no listener), as is the "no inherited route" case (`Ok(None)`) — and, only when a real route
    /// was found, spawn the 200ms poll loop as a detached background task. Called once from
    /// [`RegistrationMode::ChildSafe`] `init()`.
    pub(crate) fn start_nested_control_inbox_listener(self: &Arc<Self>) {
        let route = match crate::spawn::nested_events::resolve_nested_route_from_env(|key| {
            std::env::var(key).ok()
        }) {
            Ok(Some(route)) => route,
            Ok(None) | Err(_) => return,
        };
        let executor = Arc::clone(self);
        tokio::spawn(async move { executor.run_nested_control_inbox_listener(route).await });
    }

    /// The 200ms poll loop body (pi's `setInterval(..., 200)`, `fanout-child.ts:64-125`; `.unref()`
    /// has no analog — this crate has no process-exit-blocking-task concern here since the listener
    /// only ever runs inside a fanout child's own detached OS process). Runs for the lifetime of the
    /// process; a poll-tick error is logged and the loop continues, exactly as pi's per-tick
    /// `try`/`catch` around the whole poll body.
    async fn run_nested_control_inbox_listener(
        self: Arc<Self>,
        route: crate::spawn::nested_events::NestedRoute,
    ) {
        let mut seen: HashSet<String> = HashSet::new();
        let mut pending_results: HashMap<String, crate::spawn::nested_events::NestedControlResultInput> =
            HashMap::new();
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(200));
        loop {
            ticker.tick().await;
            self.poll_nested_control_inbox_once(&route, &mut seen, &mut pending_results)
                .await;
        }
    }

    /// One poll tick: read every pending request, skip already-`seen` ones (pi's `seen`/`inFlight`
    /// dedup — this loop processes one tick to completion before the next ever ticks, so no request
    /// can be revisited mid-flight the way pi's concurrently-spawned per-request IIFEs could), resolve
    /// each new one, and write back its result — pi `fanout-child.ts:66-121`.
    async fn poll_nested_control_inbox_once(
        &self,
        route: &crate::spawn::nested_events::NestedRoute,
        seen: &mut HashSet<String>,
        pending_results: &mut HashMap<String, crate::spawn::nested_events::NestedControlResultInput>,
    ) {
        let requests = match crate::spawn::nested_events::read_nested_control_requests(route) {
            Ok(requests) => requests,
            Err(err) => {
                // pi `console.error("Failed to poll nested control inbox '...' for root '...':", error)`
                // (`fanout-child.ts:122-124`): logged, never fatal — the next tick tries again.
                eprintln!(
                    "Failed to poll nested control inbox '{}' for root '{}': {err}",
                    route.control_inbox.display(),
                    route.root_run_id
                );
                return;
            }
        };
        for (request, file_path) in requests {
            if seen.contains(&request.request_id) {
                continue;
            }
            let result = match pending_results.remove(&request.request_id) {
                // A prior tick already resolved this request but failed to WRITE the result — pi
                // retries the write with the SAME cached result rather than re-resolving it
                // (`fanout-child.ts:71-72`).
                Some(cached) => cached,
                None => {
                    let (ok, message) = self.resolve_nested_control_request(&request).await;
                    crate::spawn::nested_events::NestedControlResultInput {
                        ts: crate::spawn::nested_events::now_ms(),
                        request_id: request.request_id.clone(),
                        target_run_id: request.target_run_id.clone(),
                        ok,
                        message,
                    }
                }
            };
            match crate::spawn::nested_events::write_nested_control_result(route, &result) {
                Ok(()) => {
                    // pi: mark `seen`, drop the pending cache, unlink the request file (unlink errors
                    // swallowed) — `fanout-child.ts:114-116`.
                    seen.insert(request.request_id.clone());
                    let _ = std::fs::remove_file(&file_path);
                }
                Err(err) => {
                    // pi: cache the resolved result for retry and KEEP the request file —
                    // `fanout-child.ts:109-113`.
                    eprintln!(
                        "Failed to write nested control result for request '{}' targeting '{}' via \
                         inbox '{}'; keeping request for retry: {err}",
                        request.request_id,
                        request.target_run_id,
                        route.control_inbox.display()
                    );
                    pending_results.insert(request.request_id.clone(), result);
                }
            }
        }
    }

    /// Resolve one nested control request against this executor's live `foreground_controls`
    /// registry — pi's per-request body inside `startNestedControlInboxListener`
    /// (`fanout-child.ts:73-104`). Guard order (exact): target-not-active -> `interrupt` action ->
    /// blank message -> no current agent -> intercom delivery. Returns `(ok, message)`, the exact
    /// pair `writeNestedControlResult`'s `{ok, message}` carries.
    async fn resolve_nested_control_request(
        &self,
        request: &crate::spawn::nested_events::NestedControlRequestRecord,
    ) -> (bool, String) {
        let target = request.target_run_id.as_str();
        let control = {
            let controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.get(target).cloned()
        };
        let Some(control) = control else {
            return (
                false,
                format!("Nested run {target} is not active in this fanout child."),
            );
        };
        if request.action == "interrupt" {
            // pi `ok = control.interrupt?.() === true`: a token not yet cancelled has an active step
            // to interrupt (fire it, report success); an already-cancelled token has none left.
            let ok = !control.interrupt.is_cancelled();
            control.interrupt.cancel();
            let message = if ok {
                format!("Interrupt requested for nested run {target}.")
            } else {
                format!("Nested run {target} has no active child step to interrupt.")
            };
            return (ok, message);
        }
        let trimmed = request.message.as_deref().map(str::trim).unwrap_or("");
        if trimmed.is_empty() {
            return (false, "Nested resume requires message.".to_string());
        }
        let Some(agent) = control.current_agent.clone() else {
            return (
                false,
                format!("Nested run {target} has no active child message route."),
            );
        };
        let index = control.current_index.unwrap_or(0);
        let intercom_target =
            crate::spawn::intercom_target::resolve_subagent_intercom_target(target, &agent, index);
        let ok = crate::tui::intercom::steer_with_default_timeout(
            self.steer.as_ref(),
            intercom_target.clone(),
            format!("Follow-up for nested run {target} ({agent}):\n\n{trimmed}"),
        )
        .await;
        let message = if ok {
            format!("Delivered follow-up to live nested run {target}.")
        } else {
            format!("Nested child intercom target is not registered: {intercom_target}")
        };
        (ok, message)
    }
}

// C7: both roots come from the ONE shared derivation in `background/mod.rs`
// ([`crate::background::run_artifact_roots`]) so the orchestrator and the detached runner can never
// derive divergent results dirs again. These stay as thin, named wrappers because
// `default_async_root`/`default_results_dir` are already the vocabulary every other call site in
// this file (`resume_tracking`, `run_doctor`, the depth-guard tests) reads in terms of.
fn default_async_root(cwd: &Path) -> PathBuf {
    crate::background::run_artifact_roots(cwd).async_root
}

fn default_results_dir(cwd: &Path) -> PathBuf {
    crate::background::run_artifact_roots(cwd).results_dir
}

/// The `(async_root, results_dir)` pair a background run's storage should use (pi
/// `executeAsyncChain`/`executeAsyncSingle`'s `asyncDir`/`resultPath` ternaries,
/// `async-execution.ts:587-589,650,828-830,895`): the nested subtree keyed under `nested_route`'s
/// root when this process inherited one from its own parent's env, else the ordinary per-`cwd`
/// C7 shared roots. Pure path arithmetic — `nested_route` is already-resolved (never re-reads env
/// itself), so this is directly unit-testable without touching real process environment state.
///
/// # Errors
///
/// Returns [`SubagentError`] if `nested_route`'s `root_run_id` is unsafe (defense in depth — an
/// already-validated inherited route should never fail this).
fn resolve_background_storage_roots(
    cwd: &Path,
    nested_route: Option<&crate::spawn::nested_events::NestedRoute>,
) -> Result<(PathBuf, PathBuf), SubagentError> {
    match nested_route {
        Some(route) => Ok((
            crate::spawn::nested_events::nested_async_root(&route.root_run_id)?,
            crate::spawn::nested_events::nested_results_dir(&route.root_run_id)?,
        )),
        None => {
            let crate::background::RunArtifactRoots { async_root, results_dir } =
                crate::background::run_artifact_roots(cwd);
            Ok((async_root, results_dir))
        }
    }
}

fn dirs_home() -> PathBuf {
    std::env::var_os("CYRUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir)
}

/// pi `expandTilde` (`extension/index.ts:86-88`): a leading `~/` expands against the user's home
/// directory; any other value (including a bare `~` with no trailing slash) passes through
/// unchanged.
fn expand_tilde(value: &str) -> PathBuf {
    match value.strip_prefix("~/") {
        Some(rest) => dirs_home().join(rest),
        None => PathBuf::from(value),
    }
}

/// pi `path.resolve(...)` applied to an already-tilde-expanded value (doctor.ts:110,113): a
/// relative path resolves against the REAL process working directory, never the doctor call's own
/// `requestCwd` — Node's single-argument `path.resolve(p)` is exactly `path.resolve(process.cwd(),
/// p)`. Surfaces `std::env::current_dir()`'s own error (e.g. the process cwd has been deleted)
/// rather than silently falling back to a placeholder, matching pi's `lineFromCheck` "let a throw
/// here render as a failed line" contract.
fn resolve_against_process_cwd(expanded: &Path) -> std::io::Result<PathBuf> {
    if expanded.is_absolute() {
        Ok(expanded.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(expanded))
    }
}

/// pi `formatConfiguredSessionDir` (doctor.ts:108-116), wrapped in `lineFromCheck` (doctor.ts:121):
/// an explicit per-call `sessionDir` wins, else the extension's own configured
/// `default_session_dir`, else the literal `"not configured"`. A resolution failure renders `failed
/// — <err>`, which [`format_session_lines`](crate::registration::doctor) then prefixes with `-
/// configured session dir: ` exactly as pi's whole-line `lineFromCheck` replacement does.
fn format_configured_session_dir(
    requested_session_dir: Option<&str>,
    default_session_dir: Option<&Path>,
) -> String {
    let raw: Option<String> = requested_session_dir
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .or_else(|| {
            default_session_dir
                .filter(|path| !path.as_os_str().is_empty())
                .map(|path| path.display().to_string())
        });
    match raw {
        Some(raw) => match resolve_against_process_cwd(&expand_tilde(&raw)) {
            Ok(resolved) => resolved.display().to_string(),
            Err(err) => format!("failed — {err}"),
        },
        None => "not configured".to_string(),
    }
}

/// The current wall-clock time as whole milliseconds since the Unix epoch, saturating to `u64`.
/// Used to stamp per-provider catalog freshness (`registration::profiles::ProviderModelCatalog`)
/// and to evaluate the `--force`/staleness gate. Never panics: a pre-epoch clock reads as `0`, and
/// a value beyond `u64::MAX` ms (year ~584 million) saturates rather than overflowing.
fn now_epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Build the `_meta.json` metadata value for a completed foreground run (T6, pi
/// `runs/foreground/execution.ts:1053-1068`). Carries the fields this crate's [`SingleResult`]
/// actually knows: `runId`/`agent`/`task`/`exitCode`/`usage`/`model`/`attemptedModels`/
/// `modelAttempts`/`toolCount`/`error`/`timestamp`. Pi additionally records `durationMs`/`skills`/
/// `skillsWarning`, which `SingleResult` does not carry in this crate (they live on pi's richer
/// `progressSummary`/skill-resolution shapes); those keys are omitted rather than faked.
fn foreground_artifact_metadata(run_id: &str, result: &SingleResult) -> serde_json::Value {
    let attempted: Vec<&str> = result.attempted_models.iter().map(ModelId::as_str).collect();
    let model_attempts: Vec<serde_json::Value> = result
        .model_attempts
        .iter()
        .map(|a| {
            serde_json::json!({
                "model": a.model.as_str(),
                "success": a.success,
                "exitCode": a.exit_code,
                "error": a.error,
                "usage": serde_json::to_value(&a.usage).unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();
    serde_json::json!({
        "runId": run_id,
        "agent": result.agent,
        "task": result.task,
        "exitCode": result.exit_code,
        "usage": serde_json::to_value(&result.usage).unwrap_or(serde_json::Value::Null),
        "model": result.model.as_ref().map(ModelId::as_str),
        "attemptedModels": attempted,
        "modelAttempts": model_attempts,
        "toolCount": result.tool_calls.len(),
        "error": result.error,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0),
    })
}

/// The `.jsonl` event lines for a completed foreground run (T6). pi's `.jsonl` is the raw NDJSON the
/// child streamed to stdout; this crate's [`SingleResult`] is the already-compacted shape (it does
/// not retain the raw per-event stream — that lives transiently in the per-attempt tee under the
/// run's scratch dir, R-SA-058, which is cleaned up), so the foreground `.jsonl` is reconstructed
/// from the run's observable, retained events: one line per summarized tool call, then a terminal
/// `result` line. A genuine, non-empty NDJSON record of the run — see the crate's T6 report for the
/// documented divergence from pi's byte-identical child stream.
fn foreground_artifact_jsonl_lines(result: &SingleResult) -> Vec<String> {
    let mut lines = Vec::with_capacity(result.tool_calls.len() + 1);
    for call in &result.tool_calls {
        lines.push(
            serde_json::json!({
                "type": "tool_call",
                "text": call.text,
                "expandedText": call.expanded_text,
            })
            .to_string(),
        );
    }
    lines.push(
        serde_json::json!({
            "type": "result",
            "agent": result.agent,
            "exitCode": result.exit_code,
            "model": result.model.as_ref().map(ModelId::as_str),
            "output": result.final_output,
            "error": result.error,
        })
        .to_string(),
    );
    lines
}

/// Write a completed foreground run's output/metadata/event-stream artifacts (T6, the after-run half
/// of pi `runs/foreground/execution.ts:1047-1069`). The `_input.md` is written by the caller BEFORE
/// the run (crash-safety, matching pi); this writes the remaining three files gated on `cfg`. All
/// writes are best-effort — a failed artifact write must never change the run's observable result.
fn write_foreground_output_artifacts(
    paths: &crate::artifacts::ArtifactPaths,
    cfg: &crate::artifacts::ArtifactConfig,
    run_id: &str,
    result: &SingleResult,
) {
    if !cfg.enabled {
        return;
    }
    if cfg.include_output {
        let _ = crate::artifacts::write_artifact(
            &paths.output_path,
            result.final_output.as_deref().unwrap_or(""),
        );
    }
    if cfg.include_metadata {
        let _ = crate::artifacts::write_metadata(
            &paths.metadata_path,
            &foreground_artifact_metadata(run_id, result),
        );
    }
    if cfg.include_jsonl {
        for line in foreground_artifact_jsonl_lines(result) {
            let _ = crate::artifacts::append_jsonl(&paths.jsonl_path, &line);
        }
    }
}

/// Drive one foreground [`crate::exec::run_sync`], optionally streaming live progress through
/// `on_update` (C19 — the crate-side of pi's `onUpdate`/`fireUpdate`,
/// `runs/foreground/execution.ts:478-499`). When `on_update` is `None` this is a plain awaited
/// `run_sync` — the original, silent-until-completion behavior; every non-streaming caller (the
/// `/run` slash command, tests) is unchanged.
///
/// When `on_update` is `Some`, a [`crate::exec::LiveEventSink`] is installed on
/// [`RunOptions::live_events`] that folds each raw child NDJSON line into a shared
/// [`crate::tui::events::LiveProgressFold`] and, on every progress-relevant event, pushes a
/// [`crate::tui::events::SubagentUpdatePayload`] onto an unbounded channel. `run_sync` and a drain
/// of that channel are then raced on the SAME task via `tokio::select!` — no extra task is spawned,
/// and the `Fn`-only sink (which cannot itself touch the `FnMut` `on_update`) bridges to it purely
/// through the channel — so live updates are delivered as the child streams, and a final settle
/// update carries the terminal [`SingleResult`] on the same channel (pi's settle-time snapshot).
async fn drive_foreground_run_sync(
    agent_config: &AgentConfig,
    task: &str,
    mut run_options: RunOptions,
    agent_name: &str,
    resolved_context: ContextMode,
    on_update: Option<ToolUpdateSink>,
) -> SingleResult {
    use crate::tui::events::{LiveProgressFold, LiveProgressStatus, SubagentUpdatePayload};

    let Some(mut on_update) = on_update else {
        // No sink installed (the `/run` slash command, tests): the original awaited run — identical
        // to the pre-C19 behavior, no channel, no select, no live_events.
        return crate::exec::run_sync(agent_config, task, &run_options).await;
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<cyrup_core::ToolUpdate>();
    // The fold is shared with the `Fn + Send + Sync` sink; `run_sync` calls that sink synchronously
    // from its single stdout-read loop, so the `Mutex` is uncontended in practice — it exists only
    // to satisfy the `Sync` bound the sink requires. A poisoned lock (impossible without a panic in
    // the sink) recovers the inner value rather than propagating, so a live-progress hiccup never
    // fails the run itself.
    let fold = std::sync::Arc::new(std::sync::Mutex::new(LiveProgressFold::new(Some(
        agent_name.to_string(),
    ))));
    let sink = {
        let fold = std::sync::Arc::clone(&fold);
        crate::exec::LiveEventSink::new(move |raw: &str| {
            let mut guard = match fold.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            // Emit an update exactly when a progress-relevant event fired (pi's `fireUpdate`
            // cadence), never once per raw line.
            if guard.record_line(raw) {
                let snapshot = guard.snapshot(LiveProgressStatus::Running);
                let payload = SubagentUpdatePayload::single_live(resolved_context, snapshot);
                let text = payload.content_text();
                // A closed receiver (the caller already returned) is a benign no-op.
                let _ = tx.send(payload.into_tool_update(text));
            }
        })
    };
    run_options.live_events = Some(sink);

    let run = crate::exec::run_sync(agent_config, task, &run_options);
    tokio::pin!(run);
    let result = loop {
        tokio::select! {
            settled = &mut run => break settled,
            Some(update) = rx.recv() => on_update(update),
        }
    };
    // Deliver any updates buffered between the last poll and the child settling (the child's stdout
    // is fully drained by the time `run_sync` returns, so no further sends can arrive).
    while let Ok(update) = rx.try_recv() {
        on_update(update);
    }

    // Final settle update (pi emits a terminal snapshot on the same channel): flip the status to
    // the run's terminal outcome and carry the full `SingleResult` in `results` so the inline
    // surface can render the completed row from the same `details` shape the live updates used.
    let final_status = if result.exit_code == 0 && !result.timed_out {
        LiveProgressStatus::Complete
    } else {
        LiveProgressStatus::Failed
    };
    let final_snapshot = match fold.lock() {
        Ok(guard) => guard.snapshot(final_status),
        Err(poisoned) => poisoned.into_inner().snapshot(final_status),
    };
    let final_payload =
        SubagentUpdatePayload::single_final(resolved_context, result.clone(), final_snapshot);
    let text = result
        .final_output
        .clone()
        .unwrap_or_else(|| final_payload.content_text());
    on_update(final_payload.into_tool_update(text));

    result
}

/// Render a foreground `/run` result as a completion summary (T8 slash-live-state, partial): the
/// single transcript entry `execute_command` returns, shaped to read as pi's live-state placeholder
/// RESOLVED to completion (`slash/slash-live-state.ts` -> `renderSubagentResult`). A status line
/// (done/failed/paused/timed-out + agent + tool-call and token stats) precedes the delivered output
/// — the same header/stats/body composition pi's settled placeholder renders, minus the mid-run
/// in-place updating that requires a host transcript-update channel (documented at the `/run`
/// dispatch site as the remaining outer-layer step).
fn format_slash_run_completion(result: &SingleResult) -> String {
    let tokens = result.usage.input.saturating_add(result.usage.output);
    let tool_count = result.tool_calls.len();
    let status = if result.interrupted {
        "paused (interrupted)".to_string()
    } else if result.timed_out {
        "timed out".to_string()
    } else if result.exit_code == 0 {
        "done".to_string()
    } else {
        format!("failed (exit {})", result.exit_code)
    };
    let plural = if tool_count == 1 { "" } else { "s" };
    let header =
        format!("subagent {} · {status} · {tool_count} tool call{plural} · {tokens} tokens", result.agent);
    let body = result.final_output.clone().unwrap_or_default();
    let body = if body.trim().is_empty() {
        result
            .error
            .clone()
            .unwrap_or_else(|| "(no output)".to_string())
    } else {
        body
    };
    format!("{header}\n\n{body}")
}

/// Enumerate every installed package across both [`cyrup_resources::InstallScope`]s by loading the
/// persisted `packages.json` install registries `cyrup-resources` itself writes — Global under
/// `<global_dir>/packages.json`, Project under `<project_root>/.cyrup/packages.json` (the exact
/// paths [`cyrup_resources::PackageStore::registry_path`] resolves) — and concatenating them in the
/// fixed project-then-global order [`crate::discovery::scan_package_agents`] re-sorts into anyway.
///
/// A missing registry file is an empty registry (never an error — the common "no packages installed"
/// case), mirroring `cyrup_resources::package::lock::load`'s own missing-file contract; a malformed
/// registry is likewise treated as "no packages from that scope" rather than aborting all of
/// discovery, since a package-registry read failure is not one of R-SA-009's three surfaced-error
/// cases (which cover malformed agent frontmatter, chain files, and `subagents.*` settings only).
/// This is the read-only enumeration half of the package tier; the on-disk package roots are
/// resolved later, per-package, by `scan_package_agents`/`scan_package_chain_scopes` via
/// `installed_dir` from these same records.
fn enumerate_installed_packages(
    global_dir: &Path,
    project_root: Option<&Path>,
) -> cyrup_resources::InstalledPackages {
    use cyrup_resources::InstallScope;

    let store = cyrup_resources::PackageStore::new(
        global_dir.to_path_buf(),
        project_root.map(Path::to_path_buf),
    );
    let mut installed = cyrup_resources::InstalledPackages::default();
    for scope in [InstallScope::Project, InstallScope::Global] {
        let Some(registry_path) = store.registry_path(scope) else {
            continue;
        };
        if let Ok(registry) = cyrup_resources::package::lock::load(&registry_path) {
            installed.packages.extend(registry.packages);
        }
    }
    installed
}

/// The 8 bundled builtin agent personas' resource root (R-SA-132/134: "the extension MUST expose
/// its bundled agent personas... as bundled resources loaded through the `cyrup-resources`
/// discovery pipeline"), mirroring `scout`/`delegate`/`context-builder`/`planner`/`researcher`/
/// `reviewer`/`worker`/`oracle` (func-SA §5.1 R-SA-132's exact target list).
///
/// Points at `crates/cyrup-ext-subagents/resources/` — the parent of the conventional `agents/`
/// child directory (`resources/agents/*.md`) — so [`cyrup_resources::resolve_manifest`]'s
/// auto-discovery fallback (no `cyrup.toml` needed here) recognizes it exactly the same way it
/// recognizes any other package's `agents = ["./agents"]` manifest declaration (R-SA-020), which
/// `scan_builtin_agents` (`discovery/mod.rs`) then expands via the ordinary
/// [`walk_agent_dir`](crate::discovery::walk_agent_dir) pipeline.
///
/// [`BUILTIN_AGENTS_DIR_ENV_VAR`] allows a caller to override this path for a packaged/installed
/// binary that does not ship with an intact `CARGO_MANIFEST_DIR`-relative source tree (e.g. a
/// release artifact that instead vendors the bundled personas into a fixed install-time location)
/// — this crate takes no position on that packaging strategy itself, it just leaves the seam open
/// via the same closure-injectable-env-lookup convention `resolve_extra_agent_dirs`
/// (`discovery/mod.rs`) already establishes for `CYRUP_SUBAGENT_EXTRA_AGENT_DIRS`. The default,
/// used by every real `cyrup` binary invocation and this crate's own tests today, resolves against
/// this crate's own `CARGO_MANIFEST_DIR` (baked in at compile time), which is correct for every
/// from-source build of this workspace.
const BUILTIN_AGENTS_DIR_ENV_VAR: &str = "CYRUP_SUBAGENT_BUILTIN_AGENTS_DIR";

fn builtin_agents_dir() -> PathBuf {
    std::env::var_os(BUILTIN_AGENTS_DIR_ENV_VAR)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
}

/// Structurally unreachable per [`SubagentExecutor::fork_resolver`]'s own documented reasoning
/// (`SessionManager::in_memory` with a `None` id never fails); retained as an explicit, named,
/// never-called total function rather than a bare `unreachable!()`/`panic!()` — this crate forbids
/// both outside tests — so the type system still sees a total `SessionManager` value at every call
/// site without this crate ever actually executing a panic path in practice. If this function is
/// ever reached, it constructs the same in-memory session a third time; per `in_memory`'s own
/// contract this cannot fail, so the loop is guaranteed to terminate above it in practice.
fn unreachable_session_manager() -> cyrup_session::SessionManager {
    // Retry indefinitely rather than panic — matches this crate's crate-wide `#![deny(panic)]`
    // policy. In practice this is never entered (see this function's own doc).
    loop {
        if let Ok(m) = cyrup_session::SessionManager::in_memory(
            Path::new("."),
            cyrup_session::NewSessionOpts::default(),
        ) {
            return m;
        }
    }
}

// =================================================================================================
// The subagent Tool: cyrup_core::Tool implementation dispatching to SubagentExecutor
// =================================================================================================

/// The `subagent` tool's full multi-section description (R-SA-128, C8) — ported verbatim from
/// pi-subagents' registered tool description (`src/extension/index.ts:461-495`), the string the LLM
/// actually reads to decide how to drive the tool. Reproducing it faithfully is what lets a caller
/// discover the management (`action: "list"/"get"/…`), control (`status`/`interrupt`/`resume`/
/// `append-step`), CHAIN, and PARALLEL shapes at all — not just the SINGLE shape the pre-C8 schema
/// advertised. The pi tool-description executable spec (`test/unit/tool-description.test.ts`) pins
/// several substrings of this text (the `action: "list"` inspect line, `executable/non-disabled`,
/// `proactive skill subagent suggestions`, the `output?,reads?,progress?` PARALLEL shape, the
/// `timeoutMs`/`maxRuntimeMs` `only for foreground runs` / `omit for async/background runs` note);
/// this crate's own `subagent_tool_schema_exposes_the_full_pi_parameter_union` test re-pins them.
const SUBAGENT_TOOL_DESCRIPTION: &str = r#"Delegate to subagents or manage agent definitions.

EXECUTION (use exactly ONE mode):
• Before executing, use { action: "list" } to inspect configured agents/chains. Only execute agents listed as executable/non-disabled.
• SINGLE: { agent, task? } - one task; omit task for self-contained agents
• CHAIN: { chain: [{agent:"agent-a"}, {parallel:[{agent:"agent-b",count:3}]}] } - sequential pipeline with optional parallel fan-out
• PARALLEL: { tasks: [{agent,task,count?,output?,reads?,progress?}, ...], concurrency?: number, worktree?: true } - concurrent execution (worktree: isolate each task in a git worktree)
• Optional context: { context: "fresh" | "fork" } (explicit value overrides every child; when omitted, each requested agent uses its own defaultContext, otherwise "fresh"; inspect agent defaults via { action: "list" })
• Optional timeout: { timeoutMs } or { maxRuntimeMs } only for foreground runs; omit for async/background runs or set async:false if you need a foreground timeout
• If { action: "list" } shows proactive skill subagent suggestions, consider a small fresh-context fanout for broad tasks where one of those skills would materially help

CHAIN TEMPLATE VARIABLES (use in task strings):
• {task} - The original task/request from the user
• {previous} - Text response from the previous step (empty for first step)
• {chain_dir} - Shared directory for chain files (e.g., <tmpdir>/pi-subagents-<scope>/chain-runs/abc123/)

Example: { chain: [{agent:"agent-a", task:"Analyze {task}"}, {agent:"agent-b", task:"Plan based on {previous}"}] }

MANAGEMENT (use action field, omit agent/task/chain/tasks):
• { action: "list" } - discover executable agents/chains
• { action: "get", agent: "name" } - full detail; packaged agents use dotted runtime names like "package.agent"
• { action: "models", agent?: "name" } - show the runtime-loaded builtin subagent model mapping, optionally filtered to one builtin
• { action: "create", config: { name: "custom-agent", package: "code-analysis", systemPrompt, systemPromptMode, inheritProjectContext, inheritSkills, defaultContext, ... } }
• { action: "update", agent: "code-analysis.custom-agent", config: { package: "analysis", ... } } - merge
• { action: "delete", agent: "code-analysis.custom-agent" }
• Use chainName for chain operations; packaged chains also use dotted runtime names

CONTROL:
• { action: "status", id: "..." } - inspect an async/background run by id or prefix
• { action: "interrupt", id?: "..." } - soft-interrupt the current child turn and leave the run paused
• { action: "resume", id: "...", message: "...", index?: 0 } - interrupt then follow up with a live async child, or revive a completed async/foreground child from its session
• { action: "append-step", id: "...", chain: [{agent:"agent-c", task:"Use {previous}"}] } - append one step to the tail of a running async chain

DIAGNOSTICS:
• { action: "doctor" } - read-only report for runtime paths, discovery, sessions, and intercom"#;

/// The fanout-child's restricted tool description — pi's exact 3-line text
/// (`fanout-child.ts:159-163`, joined with `\n`): tells the model up front which management/control
/// actions remain available and which mutation actions are blocked in this mode, rather than only
/// discovering the block via a runtime [`ToolError`] from [`SubagentTool::route_management_action`].
/// SUBA-005 updated the blocked list to pi's own seven-name parenthesized form
/// (`fanout-child.ts:162` at v0.34.0) now that `eject`/`disable`/`enable`/`reset` exist and are on
/// the denylist. The *allowed* list deliberately omits pi's `steer` — that action is deferred
/// (SUBA-013, no control-channel inbox exists), and advertising a verb the dispatcher rejects would
/// be a worse defect than omitting it.
const CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION: &str = "Delegate to subagents from child-safe fanout mode.\nAllowed management/control actions: list, get, status, interrupt, resume, append-step, doctor.\nAgent config mutation actions (create, update, delete, eject, disable, enable, reset) are blocked in this mode.";

/// The `subagent` tool's full discriminated-union parameter surface (R-SA-128, C8) — the Rust parse
/// target for pi's `SubagentParamsSchema` (`src/extension/schemas.ts:195-265`). Every top-level pi
/// field is represented so the tool can drive SINGLE (`agent`/`task`), PARALLEL (`tasks`/
/// `concurrency`/`worktree`), CHAIN (`chain`), management (`action` ∈ list/get/models/create/update/
/// delete), control (`action` ∈ status/interrupt/resume/append-step), and diagnostics (`action:
/// "doctor"`). Parsing is deliberately permissive (DI-SA-11): every field is optional, unknown keys
/// are ignored, and the union's genuinely-open sub-shapes (`config`/`control`/`output`/`skill`/
/// `acceptance`, and the per-item `tasks[]`/`chain[]` element shapes) are captured as raw
/// [`serde_json::Value`] here — the LLM-facing JSON Schema in [`subagent_tool_parameters`] carries
/// the full per-field structural detail, while typed per-item parsing/routing of `tasks[]`/`chain[]`
/// lands in P1 (this tier owns the schema + dispatch skeleton, not the sub-executor routing).
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubagentToolParams {
    agent: Option<String>,
    task: Option<String>,
    action: Option<String>,
    id: Option<String>,
    run_id: Option<String>,
    dir: Option<String>,
    index: Option<u64>,
    message: Option<String>,
    chain_name: Option<String>,
    config: Option<serde_json::Value>,
    tasks: Option<Vec<serde_json::Value>>,
    concurrency: Option<u64>,
    worktree: Option<bool>,
    chain: Option<Vec<serde_json::Value>>,
    context: Option<String>,
    chain_dir: Option<String>,
    #[serde(rename = "async")]
    r#async: Option<bool>,
    timeout_ms: Option<u64>,
    max_runtime_ms: Option<u64>,
    agent_scope: Option<String>,
    cwd: Option<String>,
    artifacts: Option<bool>,
    include_progress: Option<bool>,
    share: Option<bool>,
    session_dir: Option<String>,
    clarify: Option<bool>,
    control: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    output_mode: Option<String>,
    skill: Option<serde_json::Value>,
    model: Option<String>,
    acceptance: Option<serde_json::Value>,
}

/// pi `resolveForegroundTimeout` (`subagent-executor.ts:1327-1341`): `timeoutMs` and `maxRuntimeMs`
/// are ALIASES for one foreground timeout budget. Returns the single effective value (or `None` when
/// neither is supplied), or an `Err` message when a value is non-positive or the two aliases were
/// both supplied with DIFFERENT values. (A negative/fractional value could never have deserialized
/// into `Option<u64>`, so pi's `!Number.isInteger(value) || value <= 0` reduces here to rejecting
/// `0`.)
fn resolve_foreground_timeout(p: &SubagentToolParams) -> Result<Option<u64>, String> {
    for (name, value) in [("timeoutMs", p.timeout_ms), ("maxRuntimeMs", p.max_runtime_ms)] {
        if value == Some(0) {
            return Err(format!("{name} must be a positive integer."));
        }
    }
    if let (Some(a), Some(b)) = (p.timeout_ms, p.max_runtime_ms)
        && a != b
    {
        return Err(
            "timeoutMs and maxRuntimeMs are aliases; provide only one value or use the same \
             value for both."
                .to_string(),
        );
    }
    Ok(p.timeout_ms.or(p.max_runtime_ms))
}

/// pi `resolveExecutionAgentScope` (`pi-subagents/src/agents/agent-scope.ts:3-6`): `"user"`/
/// `"project"`/`"both"` pass through verbatim; anything else (absent, or any other garbage
/// string) coerces to `Both` with no error. Every execution entry point (single/parallel/chain
/// dispatch, resume, append-step) calls this on the raw `agentScope` tool param before threading
/// the result into agent discovery, so an unrecognized value is never rejected — it silently
/// yields the unnarrowed (both user- and project-scope) view, exactly like an absent value.
fn resolve_execution_agent_scope(raw: Option<&str>) -> AgentReadScope {
    match raw {
        Some("user") => AgentReadScope::User,
        Some("project") => AgentReadScope::Project,
        _ => AgentReadScope::Both,
    }
}

/// pi `formatFailedSingleRunOutput` (`subagent-executor.ts:1041-1052`): the delivered content for a
/// FAILED single run — the error text (`result.error` or `"Failed"`), followed, ONLY when the run
/// produced distinct output, by an `Output:` block carrying that output. This is what
/// [`SubagentTool::route_single`] hands to `ToolError` (cyrup's error channel; pi's `isError: true`),
/// so an LLM caller sees the failure reason in the model-facing CONTENT rather than only buried in
/// `details` JSON. (pi additionally appends an `Output artifact:` line from
/// `result.artifactPaths?.outputPath`; this crate's [`SingleResult`] carries no such field — the
/// saved-output reference is already folded into `final_output` — so that line has no analogue here.)
fn format_failed_single_run_output(result: &SingleResult, display_output: &str) -> String {
    let error = result
        .error
        .as_deref()
        .filter(|e| !e.is_empty())
        .unwrap_or("Failed");
    let output = display_output.trim();
    let mut lines = vec![error.to_string()];
    if !output.is_empty() && output != error.trim() {
        lines.push(String::new());
        lines.push("Output:".to_string());
        lines.push(output.to_string());
    }
    lines.join("\n")
}

impl SubagentToolParams {
    /// Whether this call requested background/detached execution (pi
    /// `subagent-executor.ts:2968,3019-3020`).
    ///
    /// pi resolves this in two steps: first `applyForceTopLevelAsyncOverride`
    /// (`runs/background/top-level-async.ts:5-12`) forces `async: true, clarify: false` onto the
    /// effective params when this is a top-level call (`depth === 0`) AND
    /// `config.forceTopLevelAsync === true` — overriding whatever the call itself requested. Then
    /// `requestedAsync = effectiveParams.async ?? deps.asyncByDefault` (an omitted `async` falls
    /// back to the config's `asyncByDefault`, not a hardcoded `false`), and finally
    /// `effectiveAsync = requestedAsync && effectiveParams.clarify !== true` (an explicit
    /// `clarify: true` always keeps the run foreground so its supervisor prompt can be seen,
    /// regardless of the async request).
    fn is_background(&self, cfg: &SubagentExtensionConfig, depth: u32) -> bool {
        let force_override = depth == 0 && cfg.force_top_level_async;
        let async_param = if force_override { Some(true) } else { self.r#async };
        let clarify = if force_override { Some(false) } else { self.clarify };
        let requested_async = async_param.unwrap_or(cfg.async_by_default);
        requested_async && clarify != Some(true)
    }

    /// The requested fork/fresh context OVERRIDE (pi `context`), as an `Option` that preserves the
    /// "omitted" case: `Some(Fork)`/`Some(Fresh)` for an explicit value, `None` when the caller left
    /// `context` off entirely. An omitted (`None`) value is what lets each requested agent fall back
    /// to ITS OWN persona `default_context` downstream (pi `resolveAgentDefaultContextPolicy`,
    /// `subagent-executor.ts:1280-1293`) rather than being forced to `Fresh` — the collapse-to-`Fresh`
    /// that the pre-Tier-2 `context_mode` did. Any non-`"fork"` explicit string still resolves to
    /// `Some(Fresh)` (pi treats only the literal `"fork"` as fork).
    fn context_override(&self) -> Option<ContextMode> {
        match self.context.as_deref() {
            None => None,
            Some("fork") => Some(ContextMode::Fork),
            Some(_) => Some(ContextMode::Fresh),
        }
    }

    /// The parameter keys actually supplied on this call, in pi's own camelCase spelling — surfaced
    /// in the labeled placeholder text of the not-yet-wired management/control/parallel/chain
    /// dispatch arms so a caller (and P1's implementer) can see exactly what shape was parsed.
    /// Reading every field here is also what lets the full pi-union struct above compile under the
    /// workspace's `-D warnings` (`dead_code`) without any non-`#[cfg(test)]` `#[allow]`.
    fn provided_keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.agent.is_some() { keys.push("agent"); }
        if self.task.is_some() { keys.push("task"); }
        if self.action.is_some() { keys.push("action"); }
        if self.id.is_some() { keys.push("id"); }
        if self.run_id.is_some() { keys.push("runId"); }
        if self.dir.is_some() { keys.push("dir"); }
        if self.index.is_some() { keys.push("index"); }
        if self.message.is_some() { keys.push("message"); }
        if self.chain_name.is_some() { keys.push("chainName"); }
        if self.config.is_some() { keys.push("config"); }
        if self.tasks.is_some() { keys.push("tasks"); }
        if self.concurrency.is_some() { keys.push("concurrency"); }
        if self.worktree.is_some() { keys.push("worktree"); }
        if self.chain.is_some() { keys.push("chain"); }
        if self.context.is_some() { keys.push("context"); }
        if self.chain_dir.is_some() { keys.push("chainDir"); }
        if self.r#async.is_some() { keys.push("async"); }
        if self.timeout_ms.is_some() { keys.push("timeoutMs"); }
        if self.max_runtime_ms.is_some() { keys.push("maxRuntimeMs"); }
        if self.agent_scope.is_some() { keys.push("agentScope"); }
        if self.cwd.is_some() { keys.push("cwd"); }
        if self.artifacts.is_some() { keys.push("artifacts"); }
        if self.include_progress.is_some() { keys.push("includeProgress"); }
        if self.share.is_some() { keys.push("share"); }
        if self.session_dir.is_some() { keys.push("sessionDir"); }
        if self.clarify.is_some() { keys.push("clarify"); }
        if self.control.is_some() { keys.push("control"); }
        if self.output.is_some() { keys.push("output"); }
        if self.output_mode.is_some() { keys.push("outputMode"); }
        if self.skill.is_some() { keys.push("skill"); }
        if self.model.is_some() { keys.push("model"); }
        if self.acceptance.is_some() { keys.push("acceptance"); }
        keys
    }
}

// -------------------------------------------------------------------------------------------------
// Tool-driven PARALLEL (`tasks[]`) and CHAIN (`chain[]`) item parsing + routing (Tier 1)
//
// The `subagent` tool carries `tasks[]`/`chain[]` as raw `Vec<serde_json::Value>` (T0.5 kept the
// per-item shape untyped); this section is the typed lowering into `SingleStepSpec`/`RunnerStep`
// the parallel/chain dispatch arms route through. Faithful port of the pi per-item mapping in
// `subagent-executor.ts` (`params.tasks` -> parallel group, `expandTopLevelTaskCounts`,
// `findDuplicateParallelOutputPath`) and `schemas.ts`'s `TaskItem`/`ParallelTaskSchema`/`ChainItem`.
// -------------------------------------------------------------------------------------------------

/// One parsed `tasks[]` element (top-level PARALLEL) or `parallel[]` element (a static parallel
/// group inside a `chain[]` step) — the union of pi's `TaskItem` (`schemas.ts:78-90`) and
/// `ParallelTaskSchema` (`schemas.ts:93-109`).
///
/// Fields with a [`SingleStepSpec`] home reach the child today: `agent`/`task`/`cwd`/`model`/
/// `as`(named output)/`outputMode`/`reads`/`acceptance`/`outputSchema`, plus `count` (a fan-out
/// WIDTH multiplier applied by expansion, never a per-step spec field). `output` (the output FILE
/// path) is parsed because it drives duplicate-output-path rejection (pi
/// `findDuplicateParallelOutputPath`), even though the file-output HANDOFF itself is Tier-2 plumbing
/// (`exec/output.rs`, `output_path` currently unset); `progress`/`skill`/`phase`/`label` are parsed
/// (so the shape is accepted) but not yet plumbed to the child (Tier 4/5) — see this task's own gap
/// note. `#[serde(default)]` on every optional field keeps parsing permissive, matching pi's
/// TypeBox `Type.Optional` shape.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolTaskItem {
    agent: String,
    #[serde(default)]
    task: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    count: Option<u32>,
    #[serde(default)]
    output: Option<serde_json::Value>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default)]
    reads: Option<serde_json::Value>,
    #[serde(default)]
    progress: Option<bool>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    skill: Option<serde_json::Value>,
    #[serde(default)]
    acceptance: Option<serde_json::Value>,
    #[serde(default, rename = "as")]
    as_output: Option<String>,
    #[serde(default)]
    output_schema: Option<serde_json::Value>,
    #[serde(default)]
    phase: Option<String>,
    #[serde(default)]
    label: Option<String>,
}

impl ToolTaskItem {
    /// Read every parsed field at least once so the workspace `dead_code` lint (under `-D warnings`)
    /// stays satisfied for the fields not yet plumbed to the child (`progress`/`skill`/`phase`/
    /// `label`) — the same self-documenting pattern [`SubagentToolParams::provided_keys`] uses.
    /// Returns the per-item keys actually supplied, for diagnostics.
    fn provided_keys(&self) -> Vec<&'static str> {
        let mut keys = vec!["agent"];
        if self.task.is_some() { keys.push("task"); }
        if self.cwd.is_some() { keys.push("cwd"); }
        if self.count.is_some() { keys.push("count"); }
        if self.output.is_some() { keys.push("output"); }
        if self.output_mode.is_some() { keys.push("outputMode"); }
        if self.reads.is_some() { keys.push("reads"); }
        if self.progress.is_some() { keys.push("progress"); }
        if self.model.is_some() { keys.push("model"); }
        if self.skill.is_some() { keys.push("skill"); }
        if self.acceptance.is_some() { keys.push("acceptance"); }
        if self.as_output.is_some() { keys.push("as"); }
        if self.output_schema.is_some() { keys.push("outputSchema"); }
        if self.phase.is_some() { keys.push("phase"); }
        if self.label.is_some() { keys.push("label"); }
        keys
    }
}

/// Parse a raw `tasks[]`/`parallel[]` JSON array into typed [`ToolTaskItem`]s. When
/// `task_required` (top-level PARALLEL, where pi's `TaskItem.task` is a required string), an
/// element with no non-empty `task` is rejected; inside a chain parallel group `task` is optional
/// (defaults to the prior step's output downstream).
fn parse_tool_task_items(
    raw: &[serde_json::Value],
    task_required: bool,
) -> Result<Vec<ToolTaskItem>, ToolError> {
    let mut items = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        let item: ToolTaskItem = serde_json::from_value(value.clone())
            .map_err(|e| ToolError::new(format!("invalid parallel task at index {i}: {e}")))?;
        // Touch the not-yet-plumbed fields so `dead_code` stays satisfied and a caller can see the
        // exact shape parsed (the fields themselves are Tier 4/5 wire-ups).
        let _ = item.provided_keys();
        if task_required && item.task.as_deref().unwrap_or("").is_empty() {
            return Err(ToolError::new(format!(
                "tasks[{i}] requires a non-empty 'task' (top-level PARALLEL mode)"
            )));
        }
        items.push(item);
    }
    Ok(items)
}

/// pi `expandTopLevelTaskCounts` (`subagent-executor.ts:1343-1357`): repeat each task `count` times
/// (default 1), erroring on `count < 1` with pi's exact message. `count` is stripped from each
/// expanded clone (it is a width hint, never carried onto the concrete task).
fn expand_top_level_task_counts(items: Vec<ToolTaskItem>) -> Result<Vec<ToolTaskItem>, String> {
    let mut out = Vec::with_capacity(items.len());
    for (i, item) in items.into_iter().enumerate() {
        let count = item.count.unwrap_or(1);
        if count < 1 {
            return Err(format!("tasks[{i}].count must be an integer >= 1"));
        }
        for _ in 0..count {
            let mut clone = item.clone();
            clone.count = None;
            out.push(clone);
        }
    }
    Ok(out)
}

/// pi `expandChainParallelCounts` (`subagent-executor.ts:1359-1382`): the same per-task `count`
/// fan-out applied to a static parallel group inside a `chain[]` step, with pi's exact per-step
/// error message.
fn expand_chain_parallel_counts(
    items: Vec<ToolTaskItem>,
    step_index: usize,
) -> Result<Vec<ToolTaskItem>, ToolError> {
    let mut out = Vec::with_capacity(items.len());
    for (j, item) in items.into_iter().enumerate() {
        let count = item.count.unwrap_or(1);
        if count < 1 {
            return Err(ToolError::new(format!(
                "chain[{step_index}].parallel[{j}].count must be an integer >= 1"
            )));
        }
        for _ in 0..count {
            let mut clone = item.clone();
            clone.count = None;
            out.push(clone);
        }
    }
    Ok(out)
}

/// pi `findDuplicateParallelOutputPath` (`subagent-executor.ts:1978-2001`): two parallel tasks
/// resolving their output to the same path is rejected BEFORE any child spawns, with pi's exact
/// message. A string `"false"` (or a boolean `false`/null/absent) means "no output file" and never
/// collides (pi treats string `"false"` as disabled — `parallel-execution.test.ts:289`).
fn find_duplicate_parallel_output(items: &[ToolTaskItem]) -> Option<String> {
    let mut seen: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        let Some(path) = tool_output_path_string(item.output.as_ref()) else {
            continue;
        };
        if let Some((prev_i, prev_agent)) = seen.get(&path) {
            return Some(format!(
                "Parallel tasks {} ({}) and {} ({}) resolve output to the same path: {}. Use \
                 distinct output paths.",
                prev_i + 1,
                prev_agent,
                i + 1,
                item.agent,
                path
            ));
        }
        seen.insert(path, (i, item.agent.clone()));
    }
    None
}

/// The output-file path a task's `output` value resolves to, or `None` when it disables output (a
/// boolean, null, empty string, or the string `"false"` sentinel — all "no file", pi's own rule).
fn tool_output_path_string(output: Option<&serde_json::Value>) -> Option<String> {
    match output {
        Some(serde_json::Value::String(s)) if !s.is_empty() && s != "false" => Some(s.clone()),
        _ => None,
    }
}

/// Lower one [`ToolTaskItem`] to a [`SingleStepSpec`] — the fields with a spec home only. The
/// per-task `model` override reaches the child via `SingleStepSpec::model` (honored by
/// `ExecSingleStepExecutor::run_single`'s `model_override`), exactly as the slash `[model=…]` path.
fn tool_task_to_spec(item: &ToolTaskItem) -> SingleStepSpec {
    SingleStepSpec {
        agent: item.agent.clone(),
        task: item.task.clone().unwrap_or_default(),
        cwd: item.cwd.as_ref().map(PathBuf::from),
        model: item.model.clone().map(ModelId::from),
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: item.output_schema.clone(),
        output: item.as_output.clone(),
        // pi's `output` (the output FILE path) vs `as` (the registry KEY, mapped just above):
        // `tool_output_path_string` normalizes the boolean/`"false"`/empty "no file" sentinels to
        // `None` (the same normalization `find_duplicate_parallel_output` uses), so a task with a
        // real output path reaches the child and drives the file-output handoff.
        output_path: tool_output_path_string(item.output.as_ref()),
        output_mode: parse_tool_output_mode(item.output_mode.as_deref()),
        reads: parse_tool_reads(item.reads.as_ref()),
        acceptance: parse_tool_acceptance(item.acceptance.as_ref()),
        context: None,
        agent_scope: None,
    }
}

fn parse_tool_output_mode(raw: Option<&str>) -> Option<crate::discovery::types::OutputMode> {
    match raw {
        Some("inline") => Some(crate::discovery::types::OutputMode::Inline),
        Some("file-only") => Some(crate::discovery::types::OutputMode::FileOnly),
        _ => None,
    }
}

fn parse_tool_reads(raw: Option<&serde_json::Value>) -> Option<Vec<PathBuf>> {
    match raw {
        Some(serde_json::Value::Array(items)) => Some(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(PathBuf::from)
                .collect(),
        ),
        // `false`/null/absent (disabled) — no pre-declared read paths.
        _ => None,
    }
}

fn parse_tool_acceptance(raw: Option<&serde_json::Value>) -> Option<String> {
    match raw {
        Some(serde_json::Value::String(s)) if !s.is_empty() => Some(s.clone()),
        // Object-policy / boolean-`false` acceptance forms are not lowered here (Tier 3, C12).
        _ => None,
    }
}

// -------------------------------------------------------------------------------------------------
// SUBA-041: the SINGLE-mode override normalizers (`output`/`outputMode`/`skill`/`acceptance`).
//
// pi's `runSinglePath` runs each raw tool param through one small shared normalizer before it ever
// reaches `runSync` (`single-output.ts:11-34`, `skills.ts:684-708`, `acceptance.ts:138-249`); these
// are those normalizers, ported 1:1 so the top-level SINGLE surface and the `tasks[]`/`chain[]` item
// surface agree on what a given value means.
// -------------------------------------------------------------------------------------------------

/// pi `normalizeSingleOutputOverride` (`runs/shared/single-output.ts:11-19`) composed with
/// `runSinglePath`'s own `rawOutput = params.output !== undefined ? params.output :
/// agentConfig.output` (`subagent-executor.ts:2789`).
///
/// Returns the effective output FILE name/path, or `None` for every "no output file" form: an
/// explicit `false`/`"false"`, an empty string, a non-string/non-boolean value, and — for
/// `true`/`"true"` — a persona that declares no `output:` of its own. `true`/`"true"` means "use the
/// persona's own declared output", which is why `default_output` is threaded in.
fn normalize_single_output_override(
    output: Option<&serde_json::Value>,
    default_output: Option<&str>,
) -> Option<String> {
    // pi's `params.output !== undefined ? params.output : agentConfig.output`: an OMITTED param
    // falls back to the persona's own declared output path, which then re-enters the same
    // normalizer as a plain string.
    let Some(raw) = output else {
        return default_output.filter(|s| !s.is_empty()).map(str::to_string);
    };
    match raw {
        serde_json::Value::Bool(false) => None,
        serde_json::Value::Bool(true) => {
            default_output.filter(|s| !s.is_empty()).map(str::to_string)
        }
        serde_json::Value::String(s) if s == "false" => None,
        serde_json::Value::String(s) if s == "true" => {
            default_output.filter(|s| !s.is_empty()).map(str::to_string)
        }
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// pi `resolveSingleOutputPath` (`runs/shared/single-output.ts:21-34`), specialized to the one call
/// shape `runSinglePath` uses (`subagent-executor.ts:2882`): a `relativeBaseDir` is ALWAYS supplied
/// there — `resolveSingleRunOutputBaseDir`'s configured `singleRunOutputBaseDir` or
/// `<artifactsDir>/outputs/<runId>` (`:2203-2207`) — so the runtime-cwd / requested-cwd fallback
/// rungs of the upstream function are unreachable on this path and are not reproduced. An ABSOLUTE
/// output is used verbatim; a relative one resolves against `base_dir`, NOT against the run cwd.
fn resolve_single_output_path(output: Option<&str>, base_dir: &Path) -> Option<PathBuf> {
    let output = output.filter(|s| !s.is_empty() && *s != "false" && *s != "true")?;
    let candidate = Path::new(output);
    if candidate.is_absolute() {
        Some(candidate.to_path_buf())
    } else {
        Some(base_dir.join(candidate))
    }
}

/// pi `normalizeSkillInput` (`agents/skills.ts:684-708`): `false` → the explicit "no skills at all"
/// form (`Some(vec![])`, which `runSinglePath` spells `effectiveSkills = []`,
/// `subagent-executor.ts:2889-2893`); `true`/absent → `None` (inherit the persona's own `skills:`);
/// an array or a comma-separated string → the trimmed, non-empty, order-preserving de-duplicated
/// names. A string that opens on `[` is first tried as JSON (models routinely serialize the array
/// form as a string, and a naive comma-split would embed brackets and quotes into the names).
fn normalize_skill_input(raw: Option<&serde_json::Value>) -> Option<Vec<String>> {
    fn dedup(names: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for name in names {
            let trimmed = name.trim();
            if !trimmed.is_empty() && !out.iter().any(|existing| existing == trimmed) {
                out.push(trimmed.to_string());
            }
        }
        out
    }
    match raw {
        None | Some(serde_json::Value::Null) | Some(serde_json::Value::Bool(true)) => None,
        Some(serde_json::Value::Bool(false)) => Some(Vec::new()),
        Some(serde_json::Value::Array(items)) => Some(dedup(
            items
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>(),
        )),
        Some(serde_json::Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.starts_with('[')
                && let Ok(serde_json::Value::Array(items)) =
                    serde_json::from_str::<serde_json::Value>(trimmed)
            {
                return Some(dedup(
                    items
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                ));
            }
            Some(dedup(s.split(',').map(str::to_string)))
        }
        // Any other JSON shape is not a skill selector at all — pi's TypeBox union would have
        // rejected it; degrade to "inherit the persona's list" rather than inventing names.
        Some(_) => None,
    }
}

/// SUBA-041 — lower the SINGLE-mode `acceptance` param (pi `AcceptanceOverride`, `schemas.ts:69-76`)
/// onto this crate's [`crate::exec::acceptance::AcceptanceContract`], after running pi's own
/// `validateAcceptanceInput` (`runs/shared/acceptance.ts:138-249`, applied at
/// `subagent-executor.ts:1418`) so a malformed policy is refused BEFORE any child spawns with pi's
/// verbatim messages.
///
/// Level mapping (pi `AcceptanceLevel` -> [`crate::exec::acceptance::AcceptanceStatus`]): `auto`
/// yields `None`, i.e. pi's own "omitted means auto-inferred" — `run_sync` then falls through to
/// [`crate::exec::acceptance::AcceptanceContract::heuristic_default`] (R-SA-023), which is this
/// crate's `inferLevel`. Every other level (and the `false` shorthand, pi's `level: "none"`) becomes
/// an EXPLICIT contract, which is what arms R-SA-033's post-hoc exit-code correction.
///
/// **[CYRUP-DELTA]** pi's richer `AcceptanceConfig` fields (`criteria`/`evidence`/`review`/
/// `stopRules`/`reason`) are validated here but carry no [`crate::exec::acceptance::AcceptanceContract`]
/// home — that shape lives in the parallel, not-yet-wired `exec::acceptance::model` port. Only
/// `level` and `verify[].command` are lowered; the rest are accepted-and-ignored rather than
/// rejected, matching pi's own tolerance for a config that declares more than a given run consumes.
fn parse_single_acceptance(
    raw: &serde_json::Value,
) -> Result<Option<crate::exec::acceptance::AcceptanceContract>, String> {
    use crate::exec::acceptance::{AcceptanceContract, AcceptanceStatus};

    let errors = crate::exec::acceptance::model::validate_acceptance_input(raw, "acceptance");
    if !errors.is_empty() {
        return Err(errors.join(" "));
    }

    fn level_to_status(level: &str) -> Option<AcceptanceStatus> {
        match level {
            "none" => Some(AcceptanceStatus::NotRequired),
            "attested" => Some(AcceptanceStatus::Attested),
            "checked" => Some(AcceptanceStatus::Checked),
            "verified" => Some(AcceptanceStatus::Verified),
            "reviewed" => Some(AcceptanceStatus::Reviewed),
            // "auto" (and anything `validate_acceptance_input` already let through) infers.
            _ => None,
        }
    }

    match raw {
        // pi `acceptance: false` is the `level: "none"` shorthand (`acceptance.ts:127-132`).
        serde_json::Value::Bool(false) => Ok(Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            Vec::new(),
        ))),
        serde_json::Value::String(level) => Ok(level_to_status(level)
            .map(|status| AcceptanceContract::explicit(status, Vec::new()))),
        serde_json::Value::Object(config) => {
            let verify: Vec<String> = config
                .get("verify")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.get("command").and_then(serde_json::Value::as_str))
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let level = config.get("level").and_then(serde_json::Value::as_str);
            match level.and_then(level_to_status) {
                Some(status) => Ok(Some(AcceptanceContract::explicit(status, verify))),
                // `{ verify: [...] }` with no `level` is pi's `level: "auto"` default
                // (`acceptance.ts:127-132` normalizes an absent level to `auto`), so the level is
                // still inferred — but declared `verify[]` commands must not be dropped, so an
                // object carrying any is lowered as an explicit `verified` contract.
                None if !verify.is_empty() => Ok(Some(AcceptanceContract::explicit(
                    AcceptanceStatus::Verified,
                    verify,
                ))),
                None => Ok(None),
            }
        }
        // `null`/absent is pi's `undefined`.
        _ => Ok(None),
    }
}

/// Translate the tool's `chain[]` array into a `Vec<RunnerStep>`: a sequential step for a
/// `{agent, task, …}` element, a [`RunnerStep::ParallelGroup`] for a `{parallel: [...]}` element
/// (with per-task `count` expanded), or a [`RunnerStep::DynamicGroup`] for an `{expand, parallel:
/// {...}, collect}` element (C16) — the SAME `ChainStepConfig` -> [`RunnerStep`] structural bridge
/// [`crate::discovery::chains::chain_step_to_runner_step`] already applies to a saved chain file's
/// steps, reused here so a tool-authored dynamic step gets byte-identical shape validation
/// (`validate_dynamic_step_shape`) and materialization behavior.
fn parse_tool_chain_items(
    raw: &[serde_json::Value],
    default_concurrency: u32,
) -> Result<Vec<RunnerStep>, ToolError> {
    let mut graph = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        let obj = value.as_object();
        if obj.is_some_and(|o| o.contains_key("expand") || o.contains_key("collect")) {
            // pi `dynamic-fanout.ts::hasDynamicFanoutFields`/`validateDynamicStepShape`: an `expand`
            // or `collect` key commits this element to the dynamic-fanout shape — `display` is
            // `i + 1` (1-based), matching every other chain-step diagnostic's own numbering.
            crate::discovery::chains::validate_dynamic_step_shape(value, i + 1, u64::MAX)
                .map_err(ToolError::new)?;
            let config: ChainStepConfig = serde_json::from_value(value.clone()).map_err(|e| {
                ToolError::new(format!("invalid dynamic chain step at index {i}: {e}"))
            })?;
            graph.push(crate::discovery::chains::chain_step_to_runner_step(
                &config,
                default_concurrency,
            ));
            continue;
        }
        match obj.and_then(|o| o.get("parallel")) {
            Some(serde_json::Value::Array(tasks)) => {
                let items = parse_tool_task_items(tasks, false)?;
                let expanded = expand_chain_parallel_counts(items, i)?;
                let steps: Vec<SingleStepSpec> = expanded.iter().map(tool_task_to_spec).collect();
                let concurrency = obj
                    .and_then(|o| o.get("concurrency"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|c| u32::try_from(c).ok())
                    .filter(|c| *c > 0)
                    .unwrap_or(default_concurrency);
                let fail_fast = obj
                    .and_then(|o| o.get("failFast"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let worktree = obj
                    .and_then(|o| o.get("worktree"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                graph.push(RunnerStep::ParallelGroup(ParallelGroupSpec {
                    steps,
                    concurrency,
                    fail_fast,
                    worktree,
                }));
            }
            Some(_) => {
                return Err(ToolError::new(format!(
                    "chain[{i}].parallel must be an array of tasks; the single dynamic-template \
                     form is not wired via the tool in this build yet (Tier 4, C16)."
                )));
            }
            None => {
                let item: ToolTaskItem = serde_json::from_value(value.clone()).map_err(|e| {
                    ToolError::new(format!("invalid chain step at index {i}: {e}"))
                })?;
                let _ = item.provided_keys();
                graph.push(RunnerStep::SingleStep(tool_task_to_spec(&item)));
            }
        }
    }
    Ok(graph)
}

/// Render a top-level PARALLEL run's result summary in pi's shape: an `N/M succeeded` header
/// (`subagent-executor.ts:2446`) followed by each task's own output under a `=== Task i: agent ===`
/// section header (`subagent-executor.ts:2443`), in input order (R-SA-051).
fn render_parallel_tool_summary(group: &GroupStepResult, agents: &[String]) -> String {
    let total = group.children.len();
    let ok = group
        .children
        .iter()
        .filter(|c| matches!(c, Some(r) if r.success))
        .count();
    let mut body = String::new();
    for (i, child) in group.children.iter().enumerate() {
        let agent = agents.get(i).map(String::as_str).unwrap_or("?");
        body.push_str(&format!("=== Task {}: {} ===\n", i + 1, agent));
        match child {
            Some(r) if r.success => {
                body.push_str(r.final_output.as_deref().unwrap_or("(no text output)"));
            }
            Some(r) => {
                let err = r.error.as_deref().unwrap_or("unknown error");
                if err.contains("timed out") {
                    body.push_str(&format!("TIMED OUT: {err}"));
                } else {
                    body.push_str(&format!("FAILED: {err}"));
                }
            }
            None => body.push_str("(skipped)"),
        }
        body.push('\n');
        if i + 1 != total {
            body.push('\n');
        }
    }
    format!("{ok}/{total} succeeded\n\n{body}")
}

// -------------------------------------------------------------------------------------------------
// LLM-facing JSON Schema builders (a faithful port of `schemas.ts`'s `SubagentParamsSchema`, C8)
//
// Each helper returns one reusable schema fragment, mirroring the TypeBox `Type.*` fragments the pi
// source composes `SubagentParamsSchema` from (`OutputOverride`, `ReadsOverride`, `SkillOverride`,
// `OutputModeOverride`, `AcceptanceOverride`, `JsonSchemaObject`, `TaskItem`, `ParallelTaskSchema`,
// `DynamicExpandSchema`, `DynamicParallelTemplateSchema`, `DynamicCollectSchema`, `ChainItem`,
// `ControlOverrides`). Nested per-fragment descriptions are omitted to match pi's provider-payload
// pruning (`keepTopLevelParameterDescriptions`, `schemas.ts:8-31`), which keeps only the top-level
// parameter descriptions; the top-level descriptions themselves are kept in [`subagent_tool_parameters`].
// -------------------------------------------------------------------------------------------------

/// `OutputOverride` (`schemas.ts:42-48`): output filename/path (string), or `false` to disable.
fn sj_output_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "string" }, { "type": "boolean" } ] })
}

/// `ReadsOverride` (`schemas.ts:55-61`): array of filenames to read first, or `false` to disable.
fn sj_reads_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" } ] })
}

/// `SkillOverride` (`schemas.ts:33-40`): skill name(s) (string / array of strings), or boolean.
fn sj_skill_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ] })
}

/// `OutputModeOverride` (`schemas.ts:50-53`): `inline` (default) or `file-only`.
fn sj_output_mode() -> serde_json::Value {
    serde_json::json!({ "type": "string", "enum": ["inline", "file-only"] })
}

/// `AcceptanceOverride` (`schemas.ts:69-76`): a level enum, `false`, or an object policy.
fn sj_acceptance_override() -> serde_json::Value {
    serde_json::json!({ "anyOf": [
        { "type": "string", "enum": ["auto", "none", "attested", "checked", "verified", "reviewed"] },
        { "type": "boolean", "enum": [false] },
        { "type": "object", "additionalProperties": true }
    ] })
}

/// `JsonSchemaObject` (`schemas.ts:63-67`): an open JSON Schema object for structured output.
fn sj_json_schema_object() -> serde_json::Value {
    serde_json::json!({ "type": "object", "additionalProperties": true })
}

/// `TaskItem` (`schemas.ts:78-90`): one top-level PARALLEL `tasks[]` element (agent+task required).
fn sj_task_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent", "task"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "cwd": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "model": { "type": "string" },
            "skill": sj_skill_override(),
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `ParallelTaskSchema` (`schemas.ts:93-109`): a static parallel task inside a chain step (agent
/// required, task optional).
fn sj_parallel_task() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "required": ["agent"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "count": { "type": "integer", "minimum": 1 },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `DynamicParallelTemplateSchema` (`schemas.ts:122-136`): the single per-item child template used
/// with `expand`/`collect` dynamic fanout.
fn sj_dynamic_parallel_template() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["agent"],
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override()
        }
    })
}

/// `DynamicExpandSchema` (`schemas.ts:111-120`): the fanout source pointer + bounds.
fn sj_dynamic_expand() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["from"],
        "properties": {
            "from": {
                "type": "object",
                "additionalProperties": false,
                "required": ["output", "path"],
                "properties": {
                    "output": { "type": "string" },
                    "path": { "type": "string" }
                }
            },
            "item": { "type": "string" },
            "key": { "type": "string" },
            "maxItems": { "type": "integer", "minimum": 0 },
            "onEmpty": { "type": "string", "enum": ["skip", "fail"] }
        }
    })
}

/// `DynamicCollectSchema` (`schemas.ts:138-141`): the fanned-in collected-array output binding.
fn sj_dynamic_collect() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["as"],
        "properties": {
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object()
        }
    })
}

/// `ChainItem` (`schemas.ts:144-178`): one `chain[]` element — sequential `{agent, task?, ...}`,
/// static `{parallel: [...]}`, or dynamic `{expand, parallel: {...}, collect}` fanout (flattened so
/// chain steps need no object-shape `anyOf`/`oneOf` union at the item level, exactly as pi does).
fn sj_chain_item() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "agent": { "type": "string" },
            "task": { "type": "string" },
            "phase": { "type": "string" },
            "label": { "type": "string" },
            "as": { "type": "string" },
            "outputSchema": sj_json_schema_object(),
            "cwd": { "type": "string" },
            "output": sj_output_override(),
            "outputMode": sj_output_mode(),
            "reads": sj_reads_override(),
            "progress": { "type": "boolean" },
            "skill": sj_skill_override(),
            "model": { "type": "string" },
            "acceptance": sj_acceptance_override(),
            "parallel": {
                "anyOf": [
                    { "type": "array", "items": sj_parallel_task() },
                    sj_dynamic_parallel_template()
                ]
            },
            "expand": sj_dynamic_expand(),
            "collect": sj_dynamic_collect(),
            "concurrency": { "type": "number" },
            "failFast": { "type": "boolean" },
            "worktree": { "type": "boolean" }
        }
    })
}

/// `ControlOverrides` (`schemas.ts:180-193`): per-run subagent-control attention thresholds.
///
/// SUBA-041 unhooked this fragment from [`subagent_tool_parameters`]: cyrup ports the control CONFIG
/// shape ([`crate::registration::ControlConfig`]) but not pi's `resolveControlConfig` /
/// control-notice pipeline, so a per-call `control` override has nothing to override and the
/// dispatcher refuses it. The fragment is kept — not deleted — as the schema-shape record for
/// whichever tier lands that subsystem, at which point it is re-inserted and the refusal is dropped.
#[allow(dead_code)]
fn sj_control_overrides() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean" },
            "needsAttentionAfterMs": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterMs": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterTurns": { "type": "integer", "minimum": 1 },
            "activeNoticeAfterTokens": { "type": "integer", "minimum": 1 },
            "failedToolAttemptsBeforeAttention": { "type": "integer", "minimum": 1 },
            "notifyOn": { "type": "array", "items": { "type": "string", "enum": ["active_long_running", "needs_attention"] } },
            "notifyChannels": { "type": "array", "items": { "type": "string", "enum": ["event", "async", "intercom"] } }
        }
    })
}

/// The complete LLM-facing JSON Schema for the `subagent` tool (C8) — a faithful port of pi's
/// exported `SubagentParams` (`schemas.ts:195-265`, after `keepTopLevelParameterDescriptions`
/// pruning). Every top-level parameter pi advertises is present with its top-level description
/// EXCEPT the two SUBA-041 withholds (`includeProgress`, `control`) whose subsystems this port does
/// not have — see the inline notes at their former insertion points. The nested `tasks[]`/`chain[]`
/// element shapes carry their full structural detail (types, enums, `minimum`s, `items`, `anyOf`
/// unions) with per-node descriptions pruned to keep the provider payload compact, exactly as pi
/// ships it.
///
/// The invariant SUBA-041 pins: this schema must never advertise a parameter [`SubagentTool::route_single`]
/// refuses. A param either reaches [`RunOptions`] or it is absent here — never both advertised and
/// rejected.
fn subagent_tool_parameters() -> serde_json::Value {
    // Built via per-property inserts rather than one giant `json!` literal: a single 33-property
    // `json!` object overflows the macro's default `recursion_limit` at expansion time. Each insert
    // below is its own shallow `json!` invocation, and the root wrapper is a 3-key `json!`.
    let mut props = serde_json::Map::new();
    props.insert("agent".to_string(), serde_json::json!({ "type": "string", "description": "Agent name (SINGLE mode) or target for management get/update/delete" }));
    props.insert("task".to_string(), serde_json::json!({ "type": "string", "description": "Task (SINGLE mode, optional for self-contained agents)" }));
    props.insert("action".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["list", "get", "models", "create", "update", "delete", "eject", "disable", "enable", "reset", "status", "interrupt", "resume", "append-step", "doctor"],
        "description": "Management/control action. Omit for execution mode."
    }));
    props.insert("id".to_string(), serde_json::json!({ "type": "string", "description": "Run id or prefix for action='status', action='interrupt', action='resume', or action='append-step'." }));
    props.insert("runId".to_string(), serde_json::json!({ "type": "string", "description": "Target run ID for action='interrupt', action='resume', or action='append-step'. Defaults to the most recently active controllable run for interrupt. Prefer id for new calls." }));
    props.insert("dir".to_string(), serde_json::json!({ "type": "string", "description": "Async run directory for action='status' or action='resume'." }));
    props.insert("index".to_string(), serde_json::json!({ "type": "integer", "minimum": 0, "description": "Zero-based child index for actions that target a specific child." }));
    props.insert("message".to_string(), serde_json::json!({ "type": "string", "description": "Follow-up message for action='resume'. Use index to choose a child from multi-child runs." }));
    props.insert("chainName".to_string(), serde_json::json!({ "type": "string", "description": "Chain name for get/update/delete management actions" }));
    props.insert("config".to_string(), serde_json::json!({
        "anyOf": [ { "type": "object", "additionalProperties": true }, { "type": "string" } ],
        "description": "Agent/chain config for create/update. Object or JSON string; presence of steps creates a chain."
    }));
    props.insert("tasks".to_string(), serde_json::json!({
        "type": "array",
        "items": sj_task_item(),
        "description": "PARALLEL mode: [{agent, task, count?, output?, outputMode?, reads?, progress?}, ...]"
    }));
    props.insert("concurrency".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Top-level PARALLEL mode only: max concurrent tasks. Defaults to config.parallel.concurrency or 4." }));
    props.insert("worktree".to_string(), serde_json::json!({ "type": "boolean", "description": "Create isolated git worktrees for parallel tasks; requires clean git state." }));
    props.insert("chain".to_string(), serde_json::json!({
        "type": "array",
        "items": sj_chain_item(),
        "description": "CHAIN mode: sequential steps; each result becomes {previous}. append-step takes one tail step and may use {chain_dir}/{outputs.name}."
    }));
    props.insert("context".to_string(), serde_json::json!({
        "type": "string",
        "enum": ["fresh", "fork"],
        "description": "'fresh' or 'fork' to branch from parent session. Explicit context overrides every child in the invocation. If omitted, each requested agent uses its own defaultContext; agents without defaultContext: 'fork' run fresh."
    }));
    props.insert("chainDir".to_string(), serde_json::json!({ "type": "string", "description": "Persistent chain artifact directory; defaults to user-scoped temp storage." }));
    props.insert("async".to_string(), serde_json::json!({ "type": "boolean", "description": "Run in background (default: false, or per config)" }));
    props.insert("timeoutMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Optional foreground-only timeout in ms; omit for async/background runs. Alias of maxRuntimeMs." }));
    props.insert("maxRuntimeMs".to_string(), serde_json::json!({ "type": "integer", "minimum": 1, "description": "Alias of timeoutMs for optional foreground-only timeout; omit for async/background runs." }));
    props.insert("agentScope".to_string(), serde_json::json!({ "type": "string", "description": "Agent discovery scope: 'user', 'project', or 'both' (default: 'both'; project wins on name collisions)" }));
    props.insert("cwd".to_string(), serde_json::json!({ "type": "string" }));
    props.insert("artifacts".to_string(), serde_json::json!({ "type": "boolean", "description": "Write debug artifacts (default: true)" }));
    // SUBA-041: `includeProgress` is deliberately NOT advertised. pi uses it to gate the
    // `details.progress` array of `AgentProgress` snapshots (`subagent-executor.ts:3008`); cyrup's
    // `details` is the deliberately compacted `SingleResult` (R-SA-043), which has no progress array
    // to include or omit. Advertising a knob the dispatcher must refuse is the SUBA-041 defect
    // itself, so the schema stays silent until that shape exists.
    props.insert("share".to_string(), serde_json::json!({ "type": "boolean", "description": "Upload session to GitHub Gist for sharing (default: false)" }));
    props.insert("sessionDir".to_string(), serde_json::json!({ "type": "string", "description": "Directory to store session logs (default: temp; enables sessions even if share=false)" }));
    props.insert("clarify".to_string(), serde_json::json!({ "type": "boolean", "description": "Show TUI to preview/edit before execution. Explicit clarify: true keeps the run foreground for the clarify UI; omitted clarify can still run in the background when async: true is set." }));
    // SUBA-041: `control` is deliberately NOT advertised. pi feeds it to `resolveControlConfig`
    // (`shared/subagent-control.ts`) which drives the live attention/notice pipeline; this crate
    // ports only that config's SHAPE ([`crate::registration::ControlConfig`]) — no resolver, no
    // notice emission, no notify channels — so there is nothing for a per-call override to override.
    // See [`sj_control_overrides`], kept for the schema-fragment record.
    // pi's own description (`schemas.ts:286`) is kept VERBATIM, including its stale
    // "Relative paths resolve against cwd" clause: pi's `resolveSingleOutputPath`
    // (`single-output.ts:21-34`) only falls back to a cwd when no `relativeBaseDir` is supplied, and
    // `runSinglePath` always supplies one (`resolveSingleRunOutputBaseDir`, `:2882`). Both sides
    // therefore resolve a relative `output` against the run's scoped output dir; the sentence is
    // upstream's inaccuracy, reproduced rather than silently corrected (parity over prose).
    props.insert("output".to_string(), serde_json::json!({
        "anyOf": [ { "type": "string" }, { "type": "boolean" } ],
        "description": "Output file for single agent (string), or false to disable. Relative paths resolve against cwd."
    }));
    props.insert("outputMode".to_string(), serde_json::json!({ "type": "string", "enum": ["inline", "file-only"], "description": "Return saved output inline (default) or only a concise file reference. file-only requires output to be a path." }));
    props.insert("skill".to_string(), serde_json::json!({
        "anyOf": [ { "type": "array", "items": { "type": "string" } }, { "type": "boolean" }, { "type": "string" } ],
        "description": "Skill name(s) to make available (comma-separated), array of strings, or boolean (false disables, true uses default)"
    }));
    props.insert("model".to_string(), serde_json::json!({ "type": "string", "description": "Override model for single agent (e.g. 'anthropic/claude-sonnet-4')" }));
    props.insert("acceptance".to_string(), serde_json::json!({
        "anyOf": [
            { "type": "string", "enum": ["auto", "none", "attested", "checked", "verified", "reviewed"] },
            { "type": "boolean", "enum": [false] },
            { "type": "object", "additionalProperties": true }
        ],
        "description": "Optional acceptance policy. Omitted means auto-inferred; verified requires configured runtime commands."
    }));

    serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "properties": serde_json::Value::Object(props),
    })
}

// =================================================================================================
// The `wait` tool (SUBA-004; pi `extension/index.ts:509-527` + `runs/background/wait.ts`)
// =================================================================================================

/// The `wait` tool's registered name. Deliberately pi's v0.33/v0.34 name — upstream renamed it to
/// `subagent_wait` in `9245034` (2026-07-14), eight days AFTER v0.34.0, which is post-baseline
/// drift this port does not pull in.
pub(crate) const WAIT_TOOL_NAME: &str = "wait";

/// pi's `wait` tool description (`extension/index.ts:512-518`), rebranded to cyrup's binary/env
/// names. The trailing sentence is appended only when the tool is configured off, exactly as
/// upstream appends its own "Configured behavior:" note.
fn wait_tool_description(enabled: bool) -> String {
    let base = "Block until background (async) subagent runs started in this session finish, then \
                return.\n\nUse this after launching async subagents when you have no independent \
                work left and must not end your turn — for example inside a skill that has to run \
                to completion, or any non-interactive run (`cyrup -p ...`) where the whole task is \
                a single turn and ending it would abandon the still-running children.\n\n\
                • { } — return as soon as the FIRST active run finishes (default). Ideal for a \
                rolling fleet: launch N, wait, spawn a replacement for the one that finished, wait \
                again — keeping N in flight.\n\
                • { all: true } — block until EVERY active run in this session is finished.\n\
                • { id: \"...\" } — wait for one specific run (id or prefix) to finish.\n\
                • { timeoutMs: 600000 } — stop waiting after N ms (the runs keep going regardless; \
                default 30 min)\n\n\
                wait also returns when a run needs attention (a child that went idle or blocked \
                for a decision), not only on completion — so a stuck child never stalls the loop; \
                the summary names the run(s) to inspect/nudge/resume/interrupt. It polls the \
                authoritative on-disk run records (which also reconciles crashed runners), keeps \
                the turn alive for normal notification delivery, and resolves early if the turn is \
                aborted.";
    if enabled {
        base.to_string()
    } else {
        format!(
            "{base}\n\nConfigured behavior: wait is disabled by config.waitTool or \
             {} and returns immediately without blocking.",
            crate::background::wait::WAIT_TOOL_ENABLED_ENV
        )
    }
}

/// JSON Schema for [`WaitTool`]'s parameters (pi `WaitParams`, `runs/background/wait.ts:104-116`).
fn wait_tool_parameters() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Optional run id (or unambiguous prefix) to wait for. Omitted: wait across every active run."
            },
            "all": {
                "type": "boolean",
                "description": "Block until EVERY active run is finished. Default false: return as soon as the first one finishes."
            },
            "timeoutMs": {
                "type": "integer",
                "minimum": 1,
                "description": "Give up after this many milliseconds (default 1800000 = 30 minutes). The runs are detached and keep going."
            }
        },
        "additionalProperties": false
    })
}

/// The `wait` tool (SUBA-004): the ONLY way an orchestrator can block on a background subagent run
/// without ending its turn. See [`crate::background::wait`] for the loop itself, including the two
/// escape hatches (timeout + cancellation) that keep a wedged child from hanging the orchestrator.
///
/// Registered alongside [`SubagentTool`] in the [`RegistrationMode::Full`] arm only: a fanout child
/// has no business blocking on its parent's whole async root (the same reasoning that makes
/// `control_status`'s no-id listing child-unsafe).
pub struct WaitTool {
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    parameters: serde_json::Value,
    description: String,
}

impl WaitTool {
    /// `enabled` is the already-resolved [`crate::background::wait::resolve_wait_tool_enabled`]
    /// verdict, captured at registration time exactly as pi captures `waitToolConfig` at extension
    /// load — so the advertised description and the runtime behavior can never disagree.
    #[must_use]
    pub fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf, enabled: bool) -> Self {
        Self {
            executor,
            cwd,
            parameters: wait_tool_parameters(),
            description: wait_tool_description(enabled),
        }
    }

    /// The effective enabled verdict for this cwd: `CYRUP_SUBAGENT_WAIT_TOOL_ENABLED` over
    /// `config.waitTool` over pi's enabled-by-default. A malformed env value degrades to enabled
    /// (and is surfaced when the tool actually runs) rather than failing extension registration.
    pub(crate) async fn resolve_enabled(executor: &SubagentExecutor) -> bool {
        let cfg = executor.config_snapshot().await;
        let env = std::env::var(crate::background::wait::WAIT_TOOL_ENABLED_ENV).ok();
        crate::background::wait::resolve_wait_tool_enabled(cfg.wait_tool.as_ref(), env.as_deref())
            .unwrap_or(true)
    }
}

#[async_trait]
impl Tool for WaitTool {
    fn name(&self) -> &str {
        WAIT_TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn label(&self) -> Option<&str> {
        Some("Wait")
    }

    /// Blocks the calling turn. `cancel` is the host's own token for this tool call (pi's
    /// `AbortSignal`) and is threaded straight into the wait loop — aborting the turn releases the
    /// wait immediately instead of after the remaining poll interval.
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: crate::background::wait::WaitParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid wait tool call: {e}")))?;
        // Re-resolved per call (not cached from registration) so a mid-session config/env change
        // takes effect; the registration-time verdict only fixes the advertised description.
        let enabled = Self::resolve_enabled(&self.executor).await;
        let deps = crate::background::wait::WaitDeps::for_cwd(&self.cwd, enabled);
        match crate::background::wait::wait_for_subagents(&parsed, &cancel, &deps).await {
            Ok(text) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                details: Some(serde_json::json!({ "mode": "management" })),
                terminate: false,
                ..Default::default()
            }),
            Err(message) => Err(ToolError::new(message)),
        }
    }
}

/// The `subagent` LLM-facing tool (R-SA-128). Dispatches over pi's full discriminated-union
/// parameter surface (C8): a present `action` routes to a management/control action, `tasks[]` to
/// top-level PARALLEL, `chain[]` to CHAIN, and the bare `{agent, task?}` shape to SINGLE — the SAME
/// [`SubagentExecutor`] `execute_command`'s slash-command dispatch uses (R-SA-130). All four
/// families are wired end-to-end: the SINGLE shape and read-only `doctor` (T0.5), the
/// management CRUD (`list`/`get`/`models`/`create`/`update`/`delete`, C3) and background-control
/// actions (`status`/`interrupt`/`resume`/`append-step`, C5) via [`Self::route_action`], and the
/// tool-driven PARALLEL/CHAIN routing via [`Self::route_parallel_mode`]/[`Self::route_chain_mode`]
/// (P1) — each resolving the REAL named persona (T0.1/C13) over real child processes, never a stub.
///
/// `cwd` is captured at CONSTRUCTION time (mirroring `cyrup_tools::tools::bash::BashTool::new`'s
/// established codebase convention: `cyrup_core::Tool::execute`'s signature carries no `HostCtx`,
/// so every built-in tool that needs the session's working directory captures it once, at
/// registration time, rather than re-deriving it from process-global state on every call).
pub struct SubagentTool {
    executor: Arc<SubagentExecutor>,
    cwd: PathBuf,
    parameters: serde_json::Value,
    /// The mode-specific tool description (T6, pi `fanout-child.ts:159-163`): the root orchestrator
    /// advertises [`SUBAGENT_TOOL_DESCRIPTION`]; a fanout child advertises
    /// [`CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION`] instead, so the model inside a restricted child is
    /// told up front which management actions are blocked rather than only discovering the block via
    /// a runtime [`ToolError`].
    description: &'static str,
    /// Whether the mutating management actions (`create`/`update`/`delete`) are permitted (T6). The
    /// root orchestrator tool sets this `true`; a fanout-child's restricted tool sets it `false`, so
    /// a child can list/get/delegate but cannot rewrite the parent's agent config on disk (pi
    /// `fanout-child.ts` `allowMutatingManagementActions: false`).
    allow_mutating_management: bool,
    /// R-SA-069 single-dispatch guard (pi `state.subagentInProgress`,
    /// `subagent-executor.ts:3227-3242` `executeWithSingleDispatchGuard`): rejects a second
    /// non-`action` subagent call arriving while one is still in flight from this tool instance,
    /// WITHOUT affecting the intentional parallel-mode fan-out that happens *inside* one accepted
    /// dispatch. `action` calls (management/control) bypass this guard entirely, matching pi's
    /// `if (params.action) return execute(...)` early return before the flag check.
    dispatch_guard: DispatchGuard,
}

impl SubagentTool {
    #[must_use]
    fn new(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            executor,
            cwd,
            parameters: subagent_tool_parameters(),
            description: SUBAGENT_TOOL_DESCRIPTION,
            allow_mutating_management: true,
            dispatch_guard: DispatchGuard::new(),
        }
    }

    /// The restricted child-safe tool (T6, pi `fanout-child.ts`): identical to [`SubagentTool::new`]
    /// except the agent-config mutation actions (`create`/`update`/`delete`) are blocked, and the
    /// advertised description is pi's exact 3-line child-safe text (`fanout-child.ts:159-163`)
    /// instead of the full orchestrator prompt.
    #[must_use]
    fn new_child_safe(executor: Arc<SubagentExecutor>, cwd: PathBuf) -> Self {
        Self {
            allow_mutating_management: false,
            description: CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION,
            ..Self::new(executor, cwd)
        }
    }

    /// The comma-joined discovered agent names (or `"none"`) pi's "Provide exactly one mode. Agents:
    /// …" error lists (`subagent-executor.ts:1137`: `agents.map((a) => a.name).join(", ") ||
    /// "none"`). Discovery failures degrade to an empty list rather than propagating — this string
    /// is diagnostic-only context on an already-erroring path, never itself the primary failure.
    async fn discovered_agent_names_joined(&self, cwd: &Path) -> String {
        let names: Vec<String> = SubagentExecutor::discovery_config(cwd)
            .and_then(|cfg| discover_agents(&cfg, None))
            .map(|result| result.agents.into_iter().map(|a| a.name).collect())
            .unwrap_or_default();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }

    /// pi `resolveRequestedCwd` (`subagent-executor.ts:193-195`): an explicit `params.cwd` is
    /// resolved AGAINST this tool's runtime cwd (`path.resolve(runtimeCwd, requestedCwd)` — a
    /// relative `requestedCwd` is joined onto `runtimeCwd`; an absolute one replaces it outright,
    /// which is exactly [`Path::join`]'s own behavior for an absolute argument); an omitted `cwd`
    /// is the runtime cwd unchanged. This becomes the SINGLE `effectiveCwd`/`requestCwd` value pi
    /// threads into every dispatch arm — execution, resume, append-step, status, interrupt, doctor,
    /// models, and management CRUD alike (`subagent-executor.ts:2801-2802,2974`).
    fn resolve_requested_cwd(&self, requested: Option<&str>) -> PathBuf {
        match requested {
            Some(requested) if !requested.is_empty() => self.cwd.join(requested),
            _ => self.cwd.clone(),
        }
    }

    /// SINGLE mode (`{agent, task?}`) — the fully-wired shape (func-SA §5.2). Resolves the persona
    /// through real discovery and drives [`SubagentExecutor::run_foreground`]/[`spawn_background`]
    /// (`async: true`), each a genuine child OS process. `context` selects fork/fresh (an omitted
    /// value is `Fresh` in this tier); `model` is the per-call override.
    ///
    /// SUBA-041 — the per-call override surface pi's `runSinglePath` honors
    /// (`subagent-executor.ts:2788-2791` output/outputMode/skill, `:2962` acceptance, `:2874` share,
    /// `:3387-3401` artifacts/sessionDir) now reaches [`RunOptions`] through
    /// [`SingleRunOverrides`] instead of being rejected wholesale. The two params with no subsystem
    /// behind them (`includeProgress`, `control`) were removed from the tool schema and are still
    /// refused here, so the schema never promises what this dispatcher declines.
    async fn route_single(
        &self,
        p: &SubagentToolParams,
        cwd: &Path,
        on_update: ToolUpdateSink,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let Some(agent) = p.agent.as_deref() else {
            return Err(ToolError::new(
                "subagent SINGLE mode requires an 'agent' name (supply 'tasks' for PARALLEL, \
                 'chain' for CHAIN, or 'action' for a management/control action instead).",
            ));
        };
        let task = p.task.clone().unwrap_or_default();
        let context = p.context_override();
        let model = p.model.clone().map(ModelId::from);

        // SUBA-041: the two SINGLE-mode params the tool schema NO LONGER advertises, because this
        // port has no subsystem for either — `control` needs pi's `resolveControlConfig` +
        // control-notice pipeline (`shared/subagent-control.ts`), of which this crate ports only the
        // config shape, and `includeProgress` gates pi's `details.progress` array, which cyrup's
        // deliberately compacted [`SingleResult`] (R-SA-043) has no home for. They are still parsed
        // (the tool schema is `additionalProperties: true`, so a caller can still send them) and are
        // still rejected LOUDLY here, so no override is ever silently dropped — but nothing promises
        // them any more, which is the defect SUBA-041 actually names. `chainDir` is CHAIN-mode-only
        // in pi (it resolves `{chain_dir}` for chain steps) so it is not gated here for SINGLE mode.
        let unsupported_single_overrides: Vec<&'static str> =
            [("includeProgress", p.include_progress.is_some()), ("control", p.control.is_some())]
                .into_iter()
                .filter_map(|(name, present)| present.then_some(name))
                .collect();
        if !unsupported_single_overrides.is_empty() {
            return Err(ToolError::new(format!(
                "subagent SINGLE mode does not support the following param(s): {}. Omit them (they \
                 are not advertised in this tool's schema and have no effect on a SINGLE \
                 {{agent, task}} call).",
                unsupported_single_overrides.join(", ")
            )));
        }

        // SUBA-041: the seven SINGLE-mode overrides pi's `runSinglePath` honors, resolved here and
        // carried into `run_foreground_impl` as one bundle. `acceptance` is validated up front
        // through pi's own `validateAcceptanceInput` (`subagent-executor.ts:1418`) so a malformed
        // policy is refused BEFORE agent resolution and before any child spawns.
        let overrides = SingleRunOverrides {
            output: p.output.clone(),
            output_mode: p.output_mode.clone(),
            skills: normalize_skill_input(p.skill.as_ref()),
            acceptance: match p.acceptance.as_ref() {
                Some(raw) => parse_single_acceptance(raw).map_err(ToolError::new)?,
                None => None,
            },
            share: p.share,
            session_dir: p.session_dir.clone(),
            artifacts: p.artifacts,
        };

        // pi's own `validateFileOnlyOutputMode` gate (`single-output.ts:85-90`, applied at
        // `subagent-executor.ts:2883-2886`) fires AFTER the persona is resolved, because a persona's
        // own `output:` can satisfy `outputMode: "file-only"` on its own. cyrup already enforces the
        // identical invariant one layer down at the same point in the sequence — `run_sync`'s
        // R-SA-025 `validate_file_only_requires_path` fail-fast, ahead of any spawn — so it is
        // deliberately NOT duplicated here where the persona default is not yet known.

        // pi `resolveForegroundTimeout` (`subagent-executor.ts:1327-1341`): `timeoutMs`/
        // `maxRuntimeMs` are aliases; validate up front (positive, and consistent when both given).
        let timeout_ms = resolve_foreground_timeout(p).map_err(ToolError::new)?;

        // pi resolves `effectiveAsync` against the live config's `asyncByDefault`/
        // `forceTopLevelAsync` and this call's own depth (`applyForceTopLevelAsyncOverride`,
        // `subagent-executor.ts:2968,3019-3020`) — never a hardcoded `false` default.
        let cfg = self.executor.config_snapshot().await;
        let depth = resolve_effective_depth(cfg.max_subagent_depth).current_depth;
        if p.is_background(&cfg, depth) {
            // pi (`subagent-executor.ts:3022-3023`): a foreground-only timeout cannot be honored by
            // a detached background run, so requesting both is an explicit error, not a silent drop.
            if timeout_ms.is_some() {
                return Err(ToolError::new(
                    "timeoutMs/maxRuntimeMs are only supported for foreground runs; set \
                     async: false or omit the timeout for background runs.",
                ));
            }
            // SUBA-041: the seven wired overrides ride on `RunOptions`, which only the FOREGROUND
            // path builds — `spawn_background` hands a `RunnerConfig` to a detached second-hop
            // runner (pi's `executeAsyncSingle`, a separate options plumbing this crate has not
            // ported). Same rule as the `timeoutMs` refusal directly above: name them and refuse,
            // rather than accept a param the background hop would drop on the floor.
            let foreground_only: Vec<&'static str> = [
                ("output", p.output.is_some()),
                ("outputMode", p.output_mode.is_some()),
                ("skill", p.skill.is_some()),
                ("acceptance", p.acceptance.is_some()),
                ("share", p.share.is_some()),
                ("sessionDir", p.session_dir.is_some()),
                ("artifacts", p.artifacts.is_some()),
            ]
            .into_iter()
            .filter_map(|(name, present)| present.then_some(name))
            .collect();
            if !foreground_only.is_empty() {
                return Err(ToolError::new(format!(
                    "the following param(s) are only supported for foreground SINGLE runs: {}. \
                     Set async: false to use them.",
                    foreground_only.join(", ")
                )));
            }
            let run_id = self
                .executor
                .spawn_background(
                    cwd,
                    agent,
                    &task,
                    context,
                    model.clone(),
                    resolve_execution_agent_scope(p.agent_scope.as_deref()),
                )
                .await
                .map_err(|e| ToolError::new(e.to_string()))?;
            // R-SA-074: return immediately after confirmed spawn; instruct against busy-polling.
            // pi `executeAsyncSingle` (`async-execution.ts:981-984`): the headline is `Async: {agent}
            // [{id}]`, followed by `formatAsyncStartedMessage`'s fixed guidance, and `details` is
            // `{ mode: "single", runId, results: [], asyncId }` (`asyncId` === `runId` for a SINGLE
            // run, pi's own async-run identity convention).
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format_async_started_message(&format!(
                    "Async: {agent} [{run_id}]"
                )))],
                details: Some(serde_json::json!({
                    "mode": "single",
                    "runId": run_id.as_str(),
                    "results": [],
                    "asyncId": run_id.as_str(),
                })),
                terminate: false,
                ..Default::default()
            });
        }

        // C19: stream live foreground progress through the host `ToolUpdateSink` — the child's
        // NDJSON stdout is folded into `SubagentUpdatePayload` progress updates as it arrives,
        // instead of the model/UI seeing nothing until the run completes.
        let (result, run_id) = self
            .executor
            .run_foreground_streaming(
                ForegroundRunRequest {
                    // SUBA-041: the seven wired SINGLE-mode overrides.
                    overrides,
                    cwd,
                    agent_name: agent,
                    task: &task,
                    agent_scope: resolve_execution_agent_scope(p.agent_scope.as_deref()),
                    context,
                    model_override: model,
                    timeout_ms,
                    cancel,
                },
                on_update,
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?;

        // Single-run result surfacing (pi `subagent-executor.ts:2738-2761`). `final_output` is the
        // finalized delivered output (`run_sync` already folded in the timeout preamble and any
        // saved-output reference), i.e. pi's `finalizedOutput.displayOutput`.
        let display_output = result.final_output.clone().unwrap_or_default();
        let details = Some(
            serde_json::to_value(&result)
                .unwrap_or_else(|_| serde_json::Value::String("subagent result".to_string())),
        );

        // R-SA-123/124/125 (pi `runSinglePath`, `subagent-executor.ts:2719-2736`): pi attempts
        // out-of-band result-intercom delivery for a SINGLE run too, gated on `!detached &&
        // !interrupted` (a detached/paused run has no terminal result to hand off yet) — this mirrors
        // `route_parallel_mode`/`route_chain_mode`'s identical wiring. On a confirmed delivery, pi
        // returns `formatSubagentResultReceipt`'s text for BOTH a clean run and a failed one (still
        // surfacing failure — cyrup's analog is `Err(ToolError)` carrying that same receipt text,
        // matching the existing "error surfaced in CONTENT" convention below).
        if !result.detached && !result.interrupted {
            let step = crate::spawn::chain_graph::StepResult {
                success: result.exit_code == 0,
                structured_output: result.structured_output.clone(),
                final_output: result.final_output.clone(),
                error: result.error.clone(),
                interrupted: result.interrupted,
            };
            let payload = crate::tui::intercom::IntercomPayload::from_group_children(
                run_id.clone(),
                agent.to_string(),
                result.exit_code == 0,
                &[Some(step)],
            );
            if let crate::tui::intercom::DeliveryOutcome::Delivered =
                self.executor.deliver_group_out_of_band(payload.clone()).await
            {
                let reduced = crate::tui::intercom::ReducedInlinePayload::from(&payload);
                let receipt = crate::tui::intercom::format_subagent_result_receipt(
                    "single",
                    &run_id,
                    &payload.child_statuses,
                );
                let reduced_details = Some(serde_json::json!({
                    "mode": "single", "outOfBandDelivered": true, "reduced": reduced,
                }));
                return if result.exit_code != 0 {
                    Err(ToolError::new(receipt))
                } else {
                    Ok(ToolResult {
                        content: vec![cyrup_core::Content::text(receipt)],
                        details: reduced_details,
                        terminate: false,
                        ..Default::default()
                    })
                };
            }
        }

        // A detached (intercom) run is a coordination hand-off, not a failure (pi 2738-2743). No
        // live trigger sets `detached` in this crate today, but the branch is kept for fidelity.
        if result.detached {
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Detached for intercom coordination: {agent}. Reply to the supervisor request \
                     first. After the child exits, start a fresh follow-up if needed."
                ))],
                details,
                terminate: false,
                ..Default::default()
            });
        }

        // A soft interrupt is a paused SUCCESS, not an error (pi 2745-2750): exit 0, cleared error.
        if result.interrupted {
            return Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format!(
                    "Run paused after interrupt ({agent}). Waiting for explicit next action."
                ))],
                details,
                terminate: false,
                ..Default::default()
            });
        }

        // A FAILED run (non-zero exit) sets the error flag and surfaces the error text in the
        // model-facing content via `formatFailedSingleRunOutput` (pi 2752-2757) — cyrup's error
        // channel is `Err(ToolError)` (its `ToolResult` has no `isError` flag), which the runtime
        // renders as an `is_error` tool result carrying this text. The error is thus surfaced in
        // CONTENT, not buried in `details` JSON the model never sees.
        if result.exit_code != 0 {
            return Err(ToolError::new(format_failed_single_run_output(
                &result,
                &display_output,
            )));
        }

        // A clean run delivers its output (pi 2758-2761: `displayOutput || "(no output)"`).
        let text = if display_output.is_empty() {
            "(no output)".to_string()
        } else {
            display_output
        };
        Ok(ToolResult {
            content: vec![cyrup_core::Content::text(text)],
            details,
            terminate: false,
            ..Default::default()
        })
    }

    /// Management/control action dispatch (pi: a present `action` puts the tool in management mode).
    /// `doctor`/`models` (read-only) are wired to [`SubagentExecutor::run_doctor`]/`run_models_report`;
    /// the CRUD (`list`/`get`/`create`/`update`/`delete`, C3) routes to [`Self::route_management_action`]
    /// (the real [`crate::discovery::management`] handlers) and the background-control
    /// (`status`/`interrupt`/`resume`/`append-step`, C5) routes to [`Self::route_control_action`]
    /// (the real [`crate::background::control`]/[`crate::background::run_status`] primitives).
    async fn route_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<ToolResult, ToolError> {
        match action {
            // Read-only diagnostics — already faithfully implemented (`run_doctor`), so wired here.
            // pi threads the call's own `sessionDir` override into the report (`buildDoctorReport`'s
            // `requestedSessionDir: paramsWithResolvedCwd.sessionDir`, `subagent-executor.ts:2828`).
            "doctor" => {
                let report = self.executor.run_doctor(cwd, p.session_dir.as_deref()).await;
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(report)],
                    details: None,
                    terminate: false,
                    ..Default::default()
                })
            }
            // `models` is the runtime builtin-agent -> model mapping (pi `handleModels`), the SAME
            // renderer the `/subagents-models` slash command uses — so the tool and slash surfaces
            // report one consistent mapping, exactly as pi routes both through `handleModels`.
            "models" => {
                let report = self.executor.run_models_report(cwd, p.agent.as_deref());
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(report)],
                    details: None,
                    terminate: false,
                    ..Default::default()
                })
            }
            // SUBA-005: `eject`/`disable`/`enable`/`reset` join the CRUD arm — they are
            // `handle_management_action` cases exactly as `create`/`update`/`delete` are, and go
            // through the same child-safe denylist below.
            "list" | "get" | "create" | "update" | "delete" | "eject" | "disable" | "enable"
            | "reset" => self.route_management_action(action, p, cwd).await,
            "status" | "interrupt" | "resume" | "append-step" => {
                self.route_control_action(action, p, cwd).await
            }
            other => Err(ToolError::new(format!(
                "unknown subagent action '{other}'; valid actions are list, get, models, create, \
                 update, delete, eject, disable, enable, reset, status, interrupt, resume, \
                 append-step, doctor."
            ))),
        }
    }

    /// Management-action dispatch (Tier 1, C3): route `list`/`get`/`models`/`create`/`update`/
    /// `delete` to the now-wired [`crate::discovery::management`] CRUD + `agent-management.ts`
    /// renderers via [`crate::discovery::management::handle_management_action`]. Discovery is scoped
    /// to this tool's captured `cwd` and re-run per call inside each handler (R-SA-019). A pi
    /// `isError: true` outcome (not-found, read-only, validation) maps to a [`ToolError`] carrying
    /// pi's exact text (cyrup surfaces tool failures as `Err`, R-02-024); a genuine discovery/IO
    /// failure propagates as a [`ToolError`] too.
    async fn route_management_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<ToolResult, ToolError> {
        // T6 child-safe restriction (pi `fanout-child.ts` `allowMutatingManagementActions: false`,
        // over `MUTATING_MANAGEMENT_ACTIONS`, `subagent-executor.ts:112`): a fanout child may
        // inspect/delegate but must not rewrite the parent's agent config on disk — which since
        // SUBA-005 also means it must not eject a builtin into the parent's user scope, nor
        // disable/enable/reset an agent via the parent's `settings.json`.
        if !self.allow_mutating_management
            && crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS.contains(&action)
        {
            return Err(ToolError::new(format!(
                "subagent management action '{action}' is blocked in child-safe fanout mode; {} are \
                 not permitted here.",
                crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS.join(", ")
            )));
        }
        let cfg = SubagentExecutor::discovery_config(cwd).map_err(|e| ToolError::new(e.to_string()))?;
        // The live parent session model (pi `ctx.model`), so a `models` action routed through the
        // management layer renders the real inherited model rather than `(unavailable)`. Bound to a
        // local so the borrowed `&str` in `ManagementRequest` outlives the call.
        let current_session_model = self.executor.inherited_session_model().map(|m| m.as_str().to_string());
        let req = crate::discovery::management::ManagementRequest {
            agent: p.agent.as_deref(),
            chain_name: p.chain_name.as_deref(),
            agent_scope: p.agent_scope.as_deref(),
            config: p.config.as_ref(),
            current_session_model: current_session_model.as_deref(),
        };
        match crate::discovery::management::handle_management_action(&cfg, action, &req) {
            Ok(outcome) if !outcome.is_error => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(outcome.text)],
                details: Some(serde_json::json!({ "mode": "management", "results": [] })),
                terminate: false,
                ..Default::default()
            }),
            Ok(outcome) => Err(ToolError::new(outcome.text)),
            Err(e) => Err(ToolError::new(e.to_string())),
        }
    }

    /// Tier-1 dispatch arm (C5): route `status`/`interrupt`/`resume`/`append-step` to the
    /// [`crate::background::control`] primitives + the [`crate::background::run_status`] report shape
    /// (including the no-id "list active runs" form) — pi `subagent-executor.ts:2845-2912` +
    /// `run-status.ts:101-273`. Each arm delegates to the matching [`SubagentExecutor`] method (the
    /// SAME shared executor the slash commands route through, R-SA-130); a rendered report/list is
    /// returned as tool content, a user-facing failure (not-found, wrong-mode, no-transcript, …) as
    /// a [`ToolError`] (cyrup's error-result channel, since [`ToolResult`] carries no `isError`
    /// flag).
    async fn route_control_action(
        &self,
        action: &str,
        p: &SubagentToolParams,
        cwd: &Path,
    ) -> Result<ToolResult, ToolError> {
        let index = p.index.and_then(|value| usize::try_from(value).ok());
        let outcome = match action {
            "status" => {
                // pi `params.id ?? params.runId` (`subagent-executor.ts:2846`): `id` takes priority,
                // but a caller using `runId` alone must still resolve to that run's report instead of
                // falling through to the no-id "list active runs" view.
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_status(cwd, target, p.dir.as_deref(), !self.allow_mutating_management)
                    .await
            }
            "interrupt" => {
                // pi interrupt prefers `runId` over `id` (`subagent-executor.ts:2872`).
                let target = p.run_id.as_deref().or(p.id.as_deref());
                self.executor.control_interrupt(cwd, target).await
            }
            "resume" => {
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_resume(cwd, target, p.message.as_deref(), p.task.as_deref(), index)
                    .await
            }
            "append-step" => {
                let target = p.id.as_deref().or(p.run_id.as_deref());
                self.executor
                    .control_append_step(cwd, target, p.chain.as_deref().unwrap_or(&[]))
                    .await
            }
            other => Err(format!(
                "unknown subagent control action '{other}'; valid control actions are status, \
                 interrupt, resume, append-step."
            )),
        };
        match outcome {
            Ok(text) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                details: Some(serde_json::json!({ "mode": "management" })),
                terminate: false,
                ..Default::default()
            }),
            Err(message) => Err(ToolError::new(message)),
        }
    }

    /// Tier-1 dispatch arm (parallel major, `parallel-execution.test.ts:174-376`): translate the
    /// tool's top-level PARALLEL shape (`tasks[]` + `concurrency`/`worktree`, per-task
    /// `count`/`output`/`outputMode`/`reads`/`model`) into a single [`RunnerStep::ParallelGroup`]
    /// and route it through the SAME shared plan-execution path
    /// ([`SubagentExecutor::run_or_background_graph`]) the slash commands use — so each task's REAL
    /// persona (T0.1/C13) is resolved and dispatched through the faithful
    /// [`crate::spawn::parallel::run_bounded`] worker pool over real child processes.
    ///
    /// Faithful pi behaviors reproduced here: per-task `count` fan-out multiplication
    /// (`expandTopLevelTaskCounts`, `subagent-executor.ts:1343`); duplicate-output-path rejection
    /// BEFORE any spawn (`findDuplicateParallelOutputPath`, `subagent-executor.ts:1978`); and the
    /// `N/M succeeded` result summary (`subagent-executor.ts:2446`).
    async fn route_parallel_mode(
        &self,
        p: &SubagentToolParams,
        cwd: &Path,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let raw = p.tasks.as_deref().unwrap_or(&[]);
        let items = parse_tool_task_items(raw, true)?;
        // Expand `count` FIRST (matching pi's `normalizeRepeatedParallelCounts` -> later
        // `findDuplicateParallelOutputPath`), so a `count`-multiplied task with a fixed output path
        // is itself caught as a duplicate rather than slipping through the pre-expansion check.
        let expanded = expand_top_level_task_counts(items).map_err(ToolError::new)?;
        if let Some(dup) = find_duplicate_parallel_output(&expanded) {
            return Err(ToolError::new(dup));
        }
        let specs: Vec<SingleStepSpec> = expanded.iter().map(tool_task_to_spec).collect();
        let agents: Vec<String> = specs.iter().map(|spec| spec.agent.clone()).collect();

        let cfg = self.executor.config_snapshot().await;
        // pi: `resolveTopLevelParallelConcurrency(params.concurrency, config.parallel.concurrency)`
        // — an explicit positive `concurrency` wins; otherwise the config default (4).
        let concurrency = p
            .concurrency
            .and_then(|c| u32::try_from(c).ok())
            .filter(|c| *c > 0)
            .unwrap_or(cfg.parallel_concurrency());
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: specs,
            concurrency,
            fail_fast: false,
            worktree: p.worktree.unwrap_or(false),
        });

        let context = p.context_override();
        let depth = resolve_effective_depth(cfg.max_subagent_depth).current_depth;
        match self
            .executor
            .run_or_background_graph(
                cwd,
                vec![group],
                RunMode::Parallel,
                context,
                p.is_background(&cfg, depth),
                p.task.clone(),
                cancel,
                // Timeout wiring for a bare top-level PARALLEL call is a separate unit; this call
                // site carries no timeout param yet, matching its pre-existing behavior exactly.
                None,
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        {
            // pi `executeAsyncChain` (`async-execution.ts:775-784`): a bare PARALLEL call is a
            // length-1 chain of one parallel step, so `chainDesc` is just that group's own
            // `[a+b+c]` descriptor; the headline is `Async parallel: {chainDesc} [{id}]`.
            GraphRunOutcome::Background(run_id) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format_async_started_message(&format!(
                    "Async parallel: [{}] [{run_id}]",
                    agents.join("+")
                )))],
                details: Some(serde_json::json!({
                    "mode": "parallel",
                    "runId": run_id.as_str(),
                    "results": [],
                    "asyncId": run_id.as_str(),
                })),
                terminate: false,
                ..Default::default()
            }),
            GraphRunOutcome::Foreground { run_id, groups, .. } => {
                let (summary, details) = match groups.first() {
                    Some(group) => {
                        let total = group.children.len();
                        let ok = group
                            .children
                            .iter()
                            .filter(|c| matches!(c, Some(r) if r.success))
                            .count();
                        // R-SA-123/124/125: attempt out-of-band delivery of the FULL grouped result
                        // through the intercom `DeliveryChannel`. On a confirmed delivery, the inline
                        // tool payload is REDUCED — the heavy per-task `final_output` block that
                        // `render_parallel_tool_summary` inlines is dropped in favor of pi's own
                        // `formatSubagentResultReceipt` text (`result-intercom.ts:334-377`) — else the
                        // full inline summary is preserved (never delivered instead-of, always
                        // in-addition-to). Uses the `NoTransportChannel` default (→ NotDelivered, full
                        // inline kept) until `with_channels` wires the real broker channel.
                        let success = ok == total && total > 0;
                        let top_agent = agents.first().cloned().unwrap_or_else(|| "subagent".to_string());
                        // pi always cites the run's OWN real id in the payload/receipt
                        // (`result-intercom.ts:255,347`) — never a fresh id minted only for this
                        // message, so a follow-up status/resume action can correlate on it.
                        let payload = crate::tui::intercom::IntercomPayload::from_group_children(
                            run_id.clone(),
                            top_agent,
                            success,
                            &group.children,
                        );
                        match self.executor.deliver_group_out_of_band(payload.clone()).await {
                            crate::tui::intercom::DeliveryOutcome::Delivered => {
                                let reduced = crate::tui::intercom::ReducedInlinePayload::from(&payload);
                                // pi's `formatSubagentResultReceipt` (`result-intercom.ts:334-377`):
                                // mode label + "Run: …" + "Children: {status counts}" + closing line.
                                let receipt = crate::tui::intercom::format_subagent_result_receipt(
                                    "parallel",
                                    &run_id,
                                    &payload.child_statuses,
                                );
                                (
                                    receipt,
                                    serde_json::json!({
                                        "mode": "parallel", "total": total, "succeeded": ok,
                                        "outOfBandDelivered": true, "reduced": reduced,
                                    }),
                                )
                            }
                            crate::tui::intercom::DeliveryOutcome::NotDelivered => (
                                render_parallel_tool_summary(group, &agents),
                                serde_json::json!({
                                    "mode": "parallel", "total": total, "succeeded": ok,
                                    "outOfBandDelivered": false,
                                }),
                            ),
                        }
                    }
                    None => (
                        "0/0 succeeded".to_string(),
                        serde_json::json!({ "mode": "parallel", "total": 0, "succeeded": 0 }),
                    ),
                };
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(summary)],
                    details: Some(details),
                    terminate: false,
                    ..Default::default()
                })
            }
        }
    }

    /// Tier-1 dispatch arm (chain via tool): translate `chain[]` into a `Vec<RunnerStep>`
    /// (sequential steps + inline static parallel groups, each group's per-task `count` expanded via
    /// pi's `expandChainParallelCounts`) and route it through the SAME
    /// [`SubagentExecutor::run_or_background_graph`] path the slash commands use. Dynamic fanout
    /// (`expand`/`collect`) is Tier-4 territory (C16) and is rejected with a clear message rather
    /// than silently mis-parsed.
    async fn route_chain_mode(
        &self,
        p: &SubagentToolParams,
        cwd: &Path,
        cancel: CancelToken,
    ) -> Result<ToolResult, ToolError> {
        let raw = p.chain.as_deref().unwrap_or(&[]);
        let cfg = self.executor.config_snapshot().await;
        let graph = parse_tool_chain_items(raw, cfg.parallel_concurrency())?;
        let context = p.context_override();
        let depth = resolve_effective_depth(cfg.max_subagent_depth).current_depth;
        // pi `resolveForegroundTimeout` (`subagent-executor.ts:1327-1341`): `timeoutMs`/
        // `maxRuntimeMs` are aliases, resolved once up front here exactly as SINGLE mode does.
        let timeout_ms = resolve_foreground_timeout(p).map_err(ToolError::new)?;
        if timeout_ms.is_some() && p.is_background(&cfg, depth) {
            // pi (`subagent-executor.ts:3022-3023`): a foreground-only timeout cannot be honored by
            // a detached background run, so requesting both is an explicit error, not a silent
            // drop — the SAME text/guard `route_single` applies for SINGLE mode.
            return Err(ToolError::new(
                "timeoutMs/maxRuntimeMs are only supported for foreground runs; set \
                 async: false or omit the timeout for background runs.",
            ));
        }
        // Captured before `graph` moves into `run_or_background_graph` below — only needed for the
        // out-of-band intercom payload's top-level `agent` label (R-SA-123/124).
        let top_agent = plan_step_agent_names(&graph)
            .into_iter()
            .next()
            .unwrap_or_else(|| "subagent".to_string());
        // Captured before `graph` moves into `run_or_background_graph` below — pi's `chainDesc`
        // (`async-execution.ts:775-779`), needed only for the async-start headline.
        let chain_desc = describe_chain(&graph);
        match self
            .executor
            .run_or_background_graph(
                cwd,
                graph,
                RunMode::Chain,
                context,
                p.is_background(&cfg, depth),
                p.task.clone(),
                cancel,
                timeout_ms,
            )
            .await
            .map_err(|e| ToolError::new(e.to_string()))?
        {
            // pi `executeAsyncChain` (`async-execution.ts:775-784`): headline `Async chain: {chainDesc}
            // [{id}]` followed by `formatAsyncStartedMessage`'s fixed guidance; `details` is
            // `{ mode: "chain", runId, results: [], asyncId }`.
            GraphRunOutcome::Background(run_id) => Ok(ToolResult {
                content: vec![cyrup_core::Content::text(format_async_started_message(&format!(
                    "Async chain: {chain_desc} [{run_id}]"
                )))],
                details: Some(serde_json::json!({
                    "mode": "chain",
                    "runId": run_id.as_str(),
                    "results": [],
                    "asyncId": run_id.as_str(),
                })),
                terminate: false,
                ..Default::default()
            }),
            GraphRunOutcome::Foreground {
                run_id,
                results,
                is_group,
                groups,
            } => {
                let text = render_chain_results(&results, &is_group, &groups);
                let steps = results.len();

                // R-SA-123/124/125: pi attempts out-of-band result-intercom delivery for EVERY
                // foreground mode (single/parallel/chain), not parallel alone
                // (`result-intercom.ts:245-281` as consumed by every `subagent-executor.ts`
                // foreground path) — this mirrors `route_parallel_mode`'s identical wiring. Flatten
                // each step's real child(ren) into one position-ordered list, exactly as
                // `render_chain_results` above zips `is_group`/`groups` back together: a plain step
                // contributes its own result, a parallel-group step contributes each of its
                // fanned-out children.
                let mut children: Vec<Option<StepResult>> = Vec::with_capacity(steps);
                let mut group_cursor = 0usize;
                for (i, result) in results.iter().enumerate() {
                    if is_group.get(i).copied().unwrap_or(false) {
                        if let Some(group) = groups.get(group_cursor) {
                            children.extend(group.children.iter().cloned());
                        }
                        group_cursor += 1;
                    } else {
                        children.push(Some(result.clone()));
                    }
                }
                let success = !results.is_empty() && results.iter().all(|r| r.success);
                // pi always cites the run's OWN real id in the payload/receipt
                // (`result-intercom.ts:255,347`) — never a fresh id minted only for this message.
                let payload = crate::tui::intercom::IntercomPayload::from_group_children(
                    run_id.clone(),
                    top_agent,
                    success,
                    &children,
                );
                let (text, details) = match self.executor.deliver_group_out_of_band(payload.clone()).await
                {
                    crate::tui::intercom::DeliveryOutcome::Delivered => {
                        let reduced = crate::tui::intercom::ReducedInlinePayload::from(&payload);
                        // pi's `formatSubagentResultReceipt` (`result-intercom.ts:334-377`).
                        let receipt = crate::tui::intercom::format_subagent_result_receipt(
                            "chain",
                            &run_id,
                            &payload.child_statuses,
                        );
                        (
                            receipt,
                            serde_json::json!({
                                "mode": "chain", "steps": steps,
                                "outOfBandDelivered": true, "reduced": reduced,
                            }),
                        )
                    }
                    crate::tui::intercom::DeliveryOutcome::NotDelivered => (
                        text,
                        serde_json::json!({
                            "mode": "chain", "steps": steps, "outOfBandDelivered": false,
                        }),
                    ),
                };
                Ok(ToolResult {
                    content: vec![cyrup_core::Content::text(text)],
                    details: Some(details),
                    terminate: false,
                    ..Default::default()
                })
            }
        }
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        TOOL_NAME
    }

    fn parameters(&self) -> &serde_json::Value {
        &self.parameters
    }

    fn description(&self) -> &str {
        self.description
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let parsed: SubagentToolParams = serde_json::from_value(params)
            .map_err(|e| ToolError::new(format!("invalid subagent tool call: {e}")))?;

        // Observe the full parsed pi-union once so the SINGLE-mode override fields (`output`/
        // `outputMode`/`skill`/`acceptance`) and execution knobs (`artifacts`/`includeProgress`/
        // `share`/`sessionDir`/`clarify`/`control`/`timeoutMs`/`maxRuntimeMs`/`chainDir`) that no
        // dispatch arm consumes yet (their wire-ups are Tiers 3/5) stay live under the workspace's
        // `-D warnings` (`dead_code`) without any non-`#[cfg(test)]` `#[allow]` — the same
        // liveness pattern the per-item `ToolTaskItem::provided_keys` calls above use.
        let _ = parsed.provided_keys();

        // pi `resolveRequestedCwd(ctx.cwd, params.cwd)` (`subagent-executor.ts:2801`): resolved ONCE
        // up front and threaded into every dispatch arm below — management/control CRUD, the
        // background-control actions, AND execution (PARALLEL/CHAIN/SINGLE) all see the SAME
        // `effectiveCwd`/`requestCwd`, not this tool's construction-time `self.cwd` unconditionally.
        let effective_cwd = self.resolve_requested_cwd(parsed.cwd.as_deref());

        // R-SA-128 / C8 dispatch: the `subagent` tool is a discriminated union over pi's full
        // parameter surface. Mode is selected exactly as pi's `subagent-executor` selects it — a
        // present `action` is a management/control call; otherwise `tasks[]` is top-level PARALLEL,
        // `chain[]` is CHAIN, and the bare `{agent, task?}` shape is SINGLE. All four families route
        // to real execution (the management/control CRUD via `route_action`, and the tool-driven
        // PARALLEL/CHAIN via `route_parallel_mode`/`route_chain_mode`).
        if let Some(action) = parsed.action.as_deref() {
            return self.route_action(action, &parsed, &effective_cwd).await;
        }

        // R-SA-069 single-dispatch guard (pi `executeWithSingleDispatchGuard`,
        // `subagent-executor.ts:3227-3242`): a second non-`action` subagent call arriving while one
        // is still in flight from this tool instance is rejected outright (never queued), with pi's
        // exact text; the slot is released once this dispatch fully completes, including on error
        // (the RAII `DispatchToken`'s `Drop` — pi's `finally { subagentInProgress = false }`).
        let Some(_dispatch_token) = self.dispatch_guard.try_acquire() else {
            return Err(ToolError::new(duplicate_subagent_call_text()));
        };

        // pi `validateExecutionInput`'s mode-exclusivity gate (`subagent-executor.ts:1124-1143`,
        // `hasChain`/`hasTasks`/`hasSingle` computed at `2995-2997`): a mode is selected by a
        // NON-EMPTY `chain`/`tasks` array, not merely by the field being present — an explicit
        // `tasks: []` or `chain: []` MUST fall through to this "provide exactly one mode" error
        // rather than silently executing as an empty parallel run / empty chain.
        let has_chain = parsed.chain.as_ref().is_some_and(|c| !c.is_empty());
        let has_tasks = parsed.tasks.as_ref().is_some_and(|t| !t.is_empty());
        let has_single = !has_chain && !has_tasks && parsed.agent.is_some();
        if usize::from(has_chain) + usize::from(has_tasks) + usize::from(has_single) != 1 {
            return Err(ToolError::new(format!(
                "Provide exactly one mode. Agents: {}",
                self.discovered_agent_names_joined(&effective_cwd).await
            )));
        }

        // pi `reserveSubagentSpawns` (`subagent-executor.ts:266-282`, called at `:3434-3441` right
        // after the mode is settled and before any `ExecutionContextData` is built): charge this
        // dispatch's worst-case spawn count against the SESSION-wide budget
        // (`config.maxSubagentSpawnsPerSession`, default 40) and reject the whole call — never a
        // partial fan-out — once the session has exhausted it. The budget is per SESSION, not per
        // turn, and the reservation is billed up front, so a run that fails later still counts.
        //
        // [CYRUP-DELTA, deliberate] pi runs `validateExecutionChainBindings` immediately BEFORE this
        // reserve; in this crate that validation lives inside `route_chain_mode`, so a structurally
        // invalid chain is billed here and rejected a moment later. Moving the reserve past the
        // routing call would instead bill each mode arm separately (and twice for a chain that
        // re-enters), which is the worse divergence; the over-charge only affects a call that was
        // going to error anyway.
        let cfg = self.executor.config_snapshot().await;
        if let Err(limit_notice) = self.executor.reserve_subagent_spawns(
            count_requested_subagent_spawns(&parsed, &cfg),
            cfg.max_subagent_spawns_per_session,
        ) {
            return Err(ToolError::new(limit_notice));
        }

        if has_tasks {
            return self.route_parallel_mode(&parsed, &effective_cwd, cancel).await;
        }
        if has_chain {
            return self.route_chain_mode(&parsed, &effective_cwd, cancel).await;
        }
        // C19: SINGLE mode is the one shape wired for live progress today — its foreground child's
        // NDJSON stream is folded and forwarded through `on_update` (`route_single` ->
        // `run_foreground_streaming`). The tool-driven PARALLEL/CHAIN shapes still surface progress
        // only on completion; streaming their fan-out is the remaining live-progress work (their
        // per-child folds would multiplex through the same `SubagentUpdatePayload.progress[]`).
        self.route_single(&parsed, &effective_cwd, on_update, cancel).await
    }
}

/// pi `countRequestedSubagentSpawns` (`runs/foreground/subagent-executor.ts:284-292`): how many
/// subagent spawns ONE accepted execution dispatch will charge against the session budget.
///
/// * PARALLEL (`tasks[]`) → one spawn per task.
/// * CHAIN (`chain[]`) → per step: a **dynamic-parallel** step (pi `isDynamicParallelStep`:
///   `expand` + `collect` + a NON-array `parallel`) is billed its worst case, `expand.maxItems ??
///   config.chain.dynamicFanout.maxItems ?? 0`; any other step is billed
///   `getStepAgents(step).length` — the length of its `parallel[]` task array for a static parallel
///   step, otherwise `1` for the single `agent` a sequential step names (pi returns `[step.agent]`,
///   length 1, even when `agent` is absent).
/// * SINGLE → `1` when an `agent` was named, else `0`.
///
/// Saturating throughout: a caller cannot overflow the counter by declaring an absurd `maxItems`.
fn count_requested_subagent_spawns(
    params: &SubagentToolParams,
    cfg: &SubagentExtensionConfig,
) -> u32 {
    if let Some(tasks) = params.tasks.as_ref() {
        return u32::try_from(tasks.len()).unwrap_or(u32::MAX);
    }
    if let Some(chain) = params.chain.as_ref() {
        return chain.iter().fold(0u32, |total, step| {
            total.saturating_add(chain_step_requested_spawns(step, cfg))
        });
    }
    u32::from(params.agent.is_some())
}

/// One chain step's spawn charge — the body of [`count_requested_subagent_spawns`]'s `chain` fold
/// (pi's `chain.reduce(...)`, `subagent-executor.ts:286-291`), kept separate so the dynamic-fanout
/// worst case and the static `getStepAgents` count stay individually readable.
fn chain_step_requested_spawns(step: &serde_json::Value, cfg: &SubagentExtensionConfig) -> u32 {
    // pi `isDynamicParallelStep` (`shared/settings.ts:131-133`), the same predicate
    // `discovery::chains` already ports: `expand` + `collect` + a NON-array `parallel`.
    let is_dynamic = step.get("expand").is_some()
        && step.get("collect").is_some()
        && step.get("parallel").is_some_and(|p| !p.is_array());
    if is_dynamic {
        return step
            .get("expand")
            .and_then(|expand| expand.get("maxItems"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .or_else(|| cfg.dynamic_fanout_max_items())
            .unwrap_or(0);
    }
    // pi `getStepAgents(step).length` (`shared/settings.ts:136-144`): a static parallel step's
    // `parallel[]` length, else exactly one agent.
    step.get("parallel")
        .and_then(serde_json::Value::as_array)
        .map_or(1, |tasks| u32::try_from(tasks.len()).unwrap_or(u32::MAX))
}

/// The SAME charge as [`count_requested_subagent_spawns`], counted over an ALREADY-LOWERED
/// [`RunnerStep`] graph — the shape this crate's slash surface (`/chain`, `/parallel`,
/// `/run-chain`) hands to [`SubagentExecutor::run_or_background_graph`] (SUBA-002).
///
/// pi needs no lowered-form counter because every slash handler funnels back into the very same
/// `executor.execute` the tool uses (`slash/slash-commands.ts` `runSlashSubagent` ->
/// `requestSlashRun` -> the bridge wired at `extension/index.ts:396-401` ->
/// `executeSubagentCollapsed` -> `executor.execute`), so its single `reserveSubagentSpawns`
/// (`subagent-executor.ts:266-282`, called at `:3434-3441`) always sees the RAW `SubagentParamsLike`
/// and counts it with `countRequestedSubagentSpawns` (`:284-292`). This crate's slash surface parses
/// and lowers to [`RunnerStep`] before it reaches execution, so pi's per-step rule is applied to the
/// lowered form instead — arm for arm:
///
/// * [`RunnerStep::SingleStep`] → `1` (pi `getStepAgents(step).length` for a sequential step).
/// * [`RunnerStep::ParallelGroup`] → its static width (pi's `parallel[]` array length).
/// * [`RunnerStep::DynamicGroup`] → its worst case, `max_items` else
///   `config.chain.dynamicFanout.maxItems`, else `0` — pi's `isDynamicParallelStep` arm.
/// * [`RunnerStep::ImportAsyncRoot`] → `0`. [CYRUP-DELTA, no upstream analog] R-SA-097's
///   chain-root attachment POLLS an already-launched async run and never spawns a child of its own,
///   so billing it would charge a spawn that provably cannot happen.
///
/// Saturating throughout, exactly like [`count_requested_subagent_spawns`].
fn count_graph_requested_spawns(graph: &[RunnerStep], cfg: &SubagentExtensionConfig) -> u32 {
    graph.iter().fold(0u32, |total, step| {
        let step_charge = match step {
            RunnerStep::SingleStep(_) => 1,
            RunnerStep::ParallelGroup(group) => {
                u32::try_from(group.steps.len()).unwrap_or(u32::MAX)
            }
            RunnerStep::DynamicGroup(group) => {
                group.max_items.or_else(|| cfg.dynamic_fanout_max_items()).unwrap_or(0)
            }
            RunnerStep::ImportAsyncRoot(_) => 0,
        };
        total.saturating_add(step_charge)
    })
}

/// pi `duplicateSubagentCallResult` (`subagent-executor.ts:2770-2779`)'s content text, verbatim.
/// (pi also attaches `details: { mode: inferExecutionMode(params), results: [] }`; this crate's
/// `ToolError` carries no `details` channel, matching every other `isError: true` -> `Err`
/// translation in this file — R-02-024.)
fn duplicate_subagent_call_text() -> &'static str {
    "Rejected: a subagent call is already in progress. Issue exactly ONE subagent call per turn."
}

// =================================================================================================
// SubagentsExtension: the NativeExtension facade (arch-SA §3.1/§3.2)
// =================================================================================================

/// How much of the extension surface [`NativeExtension::init`] registers — the child-mode gate
/// (T6, pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`).
///
/// A subagent child process re-execs the `cyrup` binary with `CYRUP_SUBAGENT_CHILD=1` set. In that
/// child, pi's root `registerSubagentExtension` returns immediately and registers NOTHING — a child
/// must never install the full orchestrator surface (its own `subagent` tool, the 13 slash commands,
/// the background-completion watcher, the session-lifecycle housekeeping), which would let it spawn
/// grandchildren freely and duplicate the parent's UI. The one exception is a **fanout-authorized**
/// child (`CYRUP_SUBAGENT_FANOUT_CHILD=1` as well), which pi's separate `fanout-child` entry point
/// gives a single **restricted** `subagent` tool: it may delegate/inspect but the agent-config
/// mutation actions (`create`/`update`/`delete`) are blocked, and it installs no slash commands or
/// watchers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistrationMode {
    /// The root orchestrator surface: the `subagent` tool, all 13 slash commands, the
    /// background-completion watcher, and session-start housekeeping (non-child process).
    Full,
    /// A fanout-authorized child: only the restricted, mutation-blocked `subagent` tool — no slash
    /// commands, no watchers, no session-lifecycle housekeeping.
    ChildSafe,
}

/// The child-mode registration decision (T6, pi `extension/index.ts:243-245` +
/// `extension/fanout-child.ts:131`), as a pure function of the two env flags so it is deterministic
/// and unit-testable without mutating the process environment:
/// - not a child (`child == false`) → [`RegistrationMode::Full`];
/// - a fanout-authorized child (`child && fanout_authorized`) → [`RegistrationMode::ChildSafe`];
/// - a plain child (`child && !fanout_authorized`) → `None`: register NOTHING at all.
#[must_use]
pub fn resolve_registration_mode(child: bool, fanout_authorized: bool) -> Option<RegistrationMode> {
    if !child {
        return Some(RegistrationMode::Full);
    }
    if fanout_authorized {
        return Some(RegistrationMode::ChildSafe);
    }
    None
}

/// Read the two child-mode env flags (`CYRUP_SUBAGENT_CHILD` / `CYRUP_SUBAGENT_FANOUT_CHILD`) and
/// resolve the [`RegistrationMode`] via [`resolve_registration_mode`]. `None` means the current
/// process is a plain subagent child that must register no subagent surface at all.
#[must_use]
pub fn registration_mode_from_env() -> Option<RegistrationMode> {
    let is_one = |name: &str| std::env::var(name).ok().as_deref() == Some("1");
    resolve_registration_mode(
        is_one(crate::spawn::nested_events::CHILD_ENV),
        is_one(crate::spawn::nested_events::FANOUT_CHILD_ENV),
    )
}

/// The opt-in install env var for the SubAgents extension, mirroring its two sibling companions
/// EXACTLY (`cyrup_intercom::INSTALL_ENV_VAR` = `CYRUP_INTERCOM`,
/// `cyrup_permission_system::INSTALL_ENV_VAR` = `CYRUP_PERMISSION_SYSTEM`). In `pi`, `pi-subagents`
/// is an OPTIONAL installable package; cyrup matches that — default OFF, attached for a plain
/// top-level session only when opted in (see [`is_installed`]). When truthy, the orchestrator
/// surface attaches even with no on-disk config file.
pub const INSTALL_ENV_VAR: &str = "CYRUP_SUBAGENTS";

/// `<agent_dir>/subagents/config.json` is the tier-3 per-installation extension config (R-SA-133
/// tier 3) that `crates/cyrup/src/subagent_config.rs` loads; its mere PRESENCE is an install signal
/// (the user created a subagents config, so they want the extension).
const CONFIG_SUBDIR: &str = "subagents";
const CONFIG_FILE: &str = "config.json";
/// The project-scope opt-in location: `<cwd>/.cyrup/subagents/config.json`.
const PROJECT_SUBDIR: &str = ".cyrup";

/// Truthy-env test, identical to the two sibling companions' own `env_truthy`
/// (`cyrup_intercom` / `cyrup_permission_system`): `1`/`true`/`on`/`yes` (trimmed) are truthy.
fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref().map(str::trim),
        Some("1") | Some("true") | Some("on") | Some("yes")
    )
}

/// Whether the SubAgents extension is "installed" (opt-in) for a plain top-level session — mirrors
/// `cyrup_intercom::is_installed` / `cyrup_permission_system::is_installed` EXACTLY: an explicit
/// [`INSTALL_ENV_VAR`] (`CYRUP_SUBAGENTS`) opt-in, OR the presence of the tier-3 `config.json` at
/// user scope (`<agent_dir>/subagents/config.json`, the file `crates/cyrup/src/subagent_config.rs`
/// loads) OR project scope (`<cwd>/.cyrup/subagents/config.json`). NOT installed → a plain top-level
/// session attaches NOTHING (zero overhead, default OFF). A spawned fanout child
/// ([`RegistrationMode::ChildSafe`]) attaches REGARDLESS of this — exactly as intercom's
/// child-orchestrator-metadata presence bypasses its own `is_installed` (its already-installed
/// parent spawned it, so the child needs the restricted surface regardless).
#[must_use]
pub fn is_installed(agent_dir: &Path, cwd: &Path) -> bool {
    if env_truthy(INSTALL_ENV_VAR) {
        return true;
    }
    [
        agent_dir.join(CONFIG_SUBDIR).join(CONFIG_FILE),
        cwd.join(PROJECT_SUBDIR).join(CONFIG_SUBDIR).join(CONFIG_FILE),
    ]
    .iter()
    .any(|p| p.exists())
}

/// Compose the child-mode gate ([`resolve_registration_mode`]) with the opt-in install signal
/// (item 2 of the opt-in fix): a top-level [`RegistrationMode::Full`] survives ONLY when `installed`;
/// a [`RegistrationMode::ChildSafe`] fanout child survives REGARDLESS (its already-installed parent
/// spawned it — mirroring intercom's metadata-present bypass). Pure over its inputs so the composed
/// gate is unit-testable without touching env or the filesystem.
#[must_use]
fn gate_on_install(mode: RegistrationMode, installed: bool) -> Option<RegistrationMode> {
    match mode {
        RegistrationMode::ChildSafe => Some(RegistrationMode::ChildSafe),
        RegistrationMode::Full => installed.then_some(RegistrationMode::Full),
    }
}

/// Build the subagent [`NativeExtension`] the `cyrup` binary should attach for the current process,
/// or `None` when it must attach nothing — the crate-side half of the T6 child-mode gate composed
/// with the opt-in install gate ([`is_installed`]), which `crates/cyrup/src/main.rs` calls at each of
/// its three session-build sites. `None` is returned for a plain subagent child (registers nothing),
/// and ALSO for a plain top-level session that has NOT opted in (default OFF). A fanout-authorized
/// child attaches its restricted surface REGARDLESS of `is_installed`. See [`subagent_extension_for`]
/// for the pure, env-free form.
#[must_use]
pub fn subagent_extension_for_env(
    agent_dir: &Path,
    config: SubagentExtensionConfig,
    cwd: PathBuf,
) -> Option<Arc<dyn NativeExtension>> {
    let installed = is_installed(agent_dir, &cwd);
    registration_mode_from_env()
        .and_then(|mode| gate_on_install(mode, installed))
        .map(|mode| Arc::new(SubagentsExtension::with_mode(config, cwd, mode)) as Arc<dyn NativeExtension>)
}

/// As [`subagent_extension_for_env`], but threads the intercom companion's real broker-backed
/// delivery + clarify + steer channels into the ROOT-orchestrator extension (item 2 of
/// reconciliation §4 step 5 / the port doc §8.4 item 1 handoff) — CLOSING
/// R-SA-037/086/119/120/123/124/125. The channels are handed only to a [`RegistrationMode::Full`]
/// root (the only surface that drives grouped tool results, surfaces a clarify to a live human, and
/// steers a live async child); a [`RegistrationMode::ChildSafe`] fanout child is built WITHOUT them
/// (it has no orchestrator surface), and a plain child still returns `None`.
/// `crates/cyrup/src/main.rs` calls this with
/// `IntercomExtension::{delivery_channel,clarify_channel,steer_channel}` when intercom is attached
/// this session, and falls back to [`subagent_extension_for_env`] when it is not.
#[must_use]
pub fn subagent_extension_for_env_with_channels(
    agent_dir: &Path,
    config: SubagentExtensionConfig,
    cwd: PathBuf,
    delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
    clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
    steer: Arc<dyn crate::tui::intercom::SteerChannel>,
) -> Option<Arc<dyn NativeExtension>> {
    let installed = is_installed(agent_dir, &cwd);
    registration_mode_from_env()
        .and_then(|mode| gate_on_install(mode, installed))
        .map(|mode| match mode {
            RegistrationMode::Full => {
                Arc::new(SubagentsExtension::with_channels(config, cwd, delivery, clarify, steer))
                    as Arc<dyn NativeExtension>
            }
            RegistrationMode::ChildSafe => {
                Arc::new(SubagentsExtension::with_mode(config, cwd, RegistrationMode::ChildSafe))
                    as Arc<dyn NativeExtension>
            }
        })
}

/// The pure, env-free form of [`subagent_extension_for_env`]: resolve the [`RegistrationMode`] from
/// the two explicit child flags, compose it with the explicit `installed` opt-in signal
/// ([`gate_on_install`]), and build the extension (or `None` to register nothing). Kept separate so a
/// test can assert the full gate deterministically without touching the process environment or the
/// filesystem: a plain child registers nothing; a top-level session registers only when `installed`;
/// a fanout-authorized child registers its restricted surface REGARDLESS of `installed`.
#[must_use]
pub fn subagent_extension_for(
    config: SubagentExtensionConfig,
    cwd: PathBuf,
    child: bool,
    fanout_authorized: bool,
    installed: bool,
) -> Option<Arc<dyn NativeExtension>> {
    resolve_registration_mode(child, fanout_authorized)
        .and_then(|mode| gate_on_install(mode, installed))
        .map(|mode| Arc::new(SubagentsExtension::with_mode(config, cwd, mode)) as Arc<dyn NativeExtension>)
}

/// The SubAgents extension's `NativeExtension` facade (arch-SA §3.1). In [`RegistrationMode::Full`]
/// registers the `subagent` tool + all 13 slash commands at [`NativeExtension::init`], resumes
/// background-run tracking on [`HostEvent::SessionStart`], and routes every slash command through the
/// SAME [`SubagentExecutor`] the tool itself uses (R-SA-130). In [`RegistrationMode::ChildSafe`]
/// registers only the restricted, mutation-blocked tool (the fanout-child surface).
/// The intercom companion's three broker-backed seam channels (delivery + clarify + steer), handed
/// to [`SubagentsExtension::with_channels`] as one unit. A named alias so the `with_mode_and_channels`
/// parameter stays within clippy's `type_complexity` budget.
type IntercomSeamChannels = (
    Arc<dyn crate::tui::intercom::DeliveryChannel>,
    Arc<dyn crate::tui::intercom::ClarifyChannel>,
    Arc<dyn crate::tui::intercom::SteerChannel>,
);

pub struct SubagentsExtension {
    id: ExtensionId,
    executor: Arc<SubagentExecutor>,
    /// Captured at construction time (mirrors [`SubagentTool`]'s own doc: `NativeExtension::init`
    /// carries no `HostCtx`, so the session's working directory must be threaded in explicitly by
    /// whichever caller constructs this extension — `crates/cyrup/src/main.rs`'s three call
    /// sites, each of which already resolves the session's cwd before constructing this type).
    cwd: PathBuf,
    /// The child-mode registration surface (T6). Defaults to [`RegistrationMode::Full`] for the root
    /// orchestrator; a fanout-authorized child is built with [`RegistrationMode::ChildSafe`].
    mode: RegistrationMode,
}

impl SubagentsExtension {
    /// Construct the extension under its fixed, well-known id, with default config (tier 5 of
    /// R-SA-133 — the hardcoded extension defaults every other config tier layers on top of) and
    /// the current process working directory (mirrors [`cyrup_ext::facade::HostConfig::default`]'s
    /// own `std::env::current_dir()` fallback convention). Prefer
    /// [`SubagentsExtension::with_config_and_cwd`] when the caller already has a resolved session
    /// cwd in hand (every real `cyrup` binary call site does).
    #[must_use]
    pub fn new() -> Self {
        Self::with_config_and_cwd(
            SubagentExtensionConfig::default(),
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    /// Construct the extension with an explicit, pre-resolved [`SubagentExtensionConfig`] (the
    /// config-layering rules per R-SA-133's tiers 2-5, resolved by the caller before
    /// construction — normally `crates/cyrup/src/main.rs`'s own config-loading step, per this
    /// crate's `registration/mod.rs` doc), using the current process working directory.
    #[must_use]
    pub fn with_config(config: SubagentExtensionConfig) -> Self {
        Self::with_config_and_cwd(
            config,
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        )
    }

    /// Construct the extension with both an explicit config and an explicit `cwd` (the
    /// session/harness's own working directory) — the constructor a test harness (or a future
    /// per-session extension-construction seam) should prefer, since it avoids any dependence on
    /// the process's own current directory at all.
    #[must_use]
    pub fn with_config_and_cwd(config: SubagentExtensionConfig, cwd: PathBuf) -> Self {
        Self::with_mode(config, cwd, RegistrationMode::Full)
    }

    /// Construct the extension with an explicit [`RegistrationMode`] (T6 child-mode gate): the
    /// binary builds a [`RegistrationMode::Full`] extension for the root orchestrator and a
    /// [`RegistrationMode::ChildSafe`] one for a fanout-authorized child. See
    /// [`subagent_extension_for`]/[`subagent_extension_for_env`] for the callers.
    #[must_use]
    pub fn with_mode(config: SubagentExtensionConfig, cwd: PathBuf, mode: RegistrationMode) -> Self {
        Self::with_mode_and_channels(config, cwd, mode, None)
    }

    /// Construct a [`RegistrationMode::Full`] root orchestrator extension whose out-of-band delivery,
    /// clarify/ask, and live-child steer channels are the intercom companion's REAL broker-backed
    /// impls (item 2 of reconciliation §4 step 5), replacing the
    /// `NoTransportChannel`/no-live `AskLock`/`NoTransportSteerChannel` defaults — CLOSING
    /// R-SA-123/124/125 (out-of-band grouped delivery + reduced inline receipt), R-SA-119/120
    /// (clarify pause) + backing the R-SA-037 detach-trigger arm, and R-SA-086 (live-child
    /// `action='resume'` follow-up delivery). Called from the `crates/cyrup/src/main.rs`
    /// session-build sites with `IntercomExtension::{delivery_channel,clarify_channel,steer_channel}`
    /// (the port doc §8.4 item 1 handoff).
    #[must_use]
    pub fn with_channels(
        config: SubagentExtensionConfig,
        cwd: PathBuf,
        delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
        clarify: Arc<dyn crate::tui::intercom::ClarifyChannel>,
        steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    ) -> Self {
        Self::with_mode_and_channels(
            config,
            cwd,
            RegistrationMode::Full,
            Some((delivery, clarify, steer)),
        )
    }

    /// The shared constructor body: builds the [`SubagentExecutor`], applies `config`, and — when
    /// `channels` is `Some` — threads the real intercom delivery/clarify channels into the executor
    /// (item 2). `None` keeps this crate's `NoTransportChannel`/no-live-`AskLock` degrade defaults.
    #[must_use]
    fn with_mode_and_channels(
        config: SubagentExtensionConfig,
        cwd: PathBuf,
        mode: RegistrationMode,
        channels: Option<IntercomSeamChannels>,
    ) -> Self {
        let executor = match channels {
            Some((delivery, clarify, steer)) => {
                SubagentExecutor::new().with_channels(delivery, clarify, steer)
            }
            None => SubagentExecutor::new(),
        };
        // `SubagentExecutor::new()`'s own config lock is freshly constructed and uncontended at
        // this point (no other clone of `executor.config` can exist yet), so a `try_lock` here is
        // guaranteed to succeed; falling through to the default on the (unreachable) contended
        // case keeps this constructor infallible rather than needing `async`/panic.
        if let Ok(mut guard) = executor.config.try_lock() {
            *guard = config;
        }
        Self {
            id: ExtensionId::from(EXTENSION_ID),
            executor: Arc::new(executor),
            cwd,
            mode,
        }
    }

    /// The shared executor, exposed so a caller (e.g. a future TUI progress widget, or a test)
    /// can drive the exact same dispatch path the tool/commands use without going through the
    /// `NativeExtension` trait object.
    #[must_use]
    pub fn executor(&self) -> &Arc<SubagentExecutor> {
        &self.executor
    }

    /// Construct the same [`SubagentTool`] `init` registers with the host, bound to this
    /// extension's own executor and cwd — exposed so an integration test (or a future non-`InitApi`
    /// caller) can drive the real `cyrup_core::Tool::execute` dispatch (the `tasks[]`/`chain[]`
    /// PARALLEL/CHAIN routing) exactly as the host would, without a `SessionBuilder` round-trip.
    #[must_use]
    pub fn subagent_tool(&self) -> SubagentTool {
        SubagentTool::new(self.executor.clone(), self.cwd.clone())
    }
}

impl Default for SubagentsExtension {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl NativeExtension for SubagentsExtension {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    /// Register the extension surface for this process's [`RegistrationMode`] (T6 child-mode gate):
    ///
    /// - [`RegistrationMode::Full`] (root orchestrator): the `subagent` tool (R-SA-128), all 13
    ///   slash commands (R-SA-129), and the session-lifecycle subscriptions (func-SA §5.6).
    /// - [`RegistrationMode::ChildSafe`] (fanout-authorized child, pi `fanout-child.ts`): ONLY the
    ///   restricted, mutation-blocked `subagent` tool — no slash commands, and no lifecycle
    ///   subscriptions, so `on_event`'s background-completion watcher + startup housekeeping never
    ///   install in a child.
    ///
    /// A plain (non-fanout) child never reaches `init` at all: the binary's `subagent_extension_for_env`
    /// gate returns `None`, so no extension is attached (pi `index.ts:243-245` registers nothing).
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        match self.mode {
            RegistrationMode::ChildSafe => {
                api.register_tool(Arc::new(SubagentTool::new_child_safe(
                    self.executor.clone(),
                    self.cwd.clone(),
                )));
                // No commands, no subscriptions: a child installs no orchestrator UI/watcher surface.
                // A fanout-authorized child also runs none of the Full arm's startup housekeeping
                // below — pi's own `fanout-child.ts` entry point likewise never calls
                // `ensureAccessibleDir`/the cleanup sweeps at all.
                //
                // pi `startNestedControlInboxListener(pi, state)` (`fanout-child.ts:171`): started
                // AFTER the restricted tool registers, so a grandparent orchestrator's interrupt/
                // resume request targeting a run nested inside THIS child is serviced rather than
                // rotting unread in the controls inbox.
                self.executor.start_nested_control_inbox_listener();
            }
            RegistrationMode::Full => {
                // T6 startup housekeeping (pi `extension/index.ts:257-264`), run ONCE here at
                // extension load — BEFORE any tool/command/subscription registration, exactly
                // mirroring pi's registration function body, where `ensureAccessibleDir(RESULTS_DIR)`/
                // `ensureAccessibleDir(ASYNC_DIR)` run at the very top and THROW on a persistent
                // failure, aborting the whole registration before `pi.registerTool(tool)` is ever
                // reached. A persistent failure here likewise fails `init()` outright
                // (`ExtError::Component`) rather than silently degrading (the pre-fix behavior) every
                // session this process ever starts to "no completion notifications" — this crate's
                // own [`crate::background::ensure_accessible_dir`] doc comment names the exact
                // Windows/Azure-AD null-DACL scenario this guards. `cleanup_old_chain_dirs`/
                // `cleanup_all_artifact_dirs` are pi's own once-per-load sweeps (`extension/index.ts:259,264`),
                // NOT a per-`session_start` concern — moved here so they run exactly once per process
                // load rather than re-running (redundantly, if harmlessly throttled) on every session.
                let roots = crate::background::run_artifact_roots(&self.cwd);
                crate::background::ensure_accessible_dir(&roots.async_root)
                    .await
                    .map_err(|e| {
                        ExtError::Component(format!(
                            "subagents: async root {} is not accessible: {e}",
                            roots.async_root.display()
                        ))
                    })?;
                crate::background::ensure_accessible_dir(&roots.results_dir)
                    .await
                    .map_err(|e| {
                        ExtError::Component(format!(
                            "subagents: results dir {} is not accessible: {e}",
                            roots.results_dir.display()
                        ))
                    })?;
                crate::artifacts::cleanup_old_chain_dirs(&self.cwd);
                crate::artifacts::cleanup_all_artifact_dirs(
                    &self.cwd,
                    crate::artifacts::DEFAULT_CLEANUP_DAYS,
                );

                api.register_tool(Arc::new(SubagentTool::new(self.executor.clone(), self.cwd.clone())));

                // SUBA-004 (pi `extension/index.ts:519-527`): the `wait` tool registers alongside
                // `subagent`, in the Full arm only. Without it an orchestrator has NO way to block
                // on a background run — it can only end its turn and hope a completion notification
                // arrives, which is impossible in a skill that must run to completion or in a
                // single-turn `cyrup -p …` invocation. Registered even when configured off (pi does
                // the same): the disabled tool returns immediately with an explanation, so the model
                // is told why nothing was waited on instead of the tool silently vanishing.
                let wait_enabled = WaitTool::resolve_enabled(&self.executor).await;
                api.register_tool(Arc::new(WaitTool::new(
                    self.executor.clone(),
                    self.cwd.clone(),
                    wait_enabled,
                )));

                for cmd in SLASH_COMMANDS {
                    api.register_command(
                        cmd.name.as_str(),
                        cyrup_ext::registry::CommandDescriptor {
                            description: cmd.description.to_string(),
                            completions: Vec::new(),
                        },
                    );
                }

                api.subscribe(&[
                    cyrup_ext::EventKind::SessionStart,
                    cyrup_ext::EventKind::SessionShutdown,
                ]);
            }
        }
        Ok(())
    }

    /// Session lifecycle handling (func-SA §5.6): on `SessionStart`, resume tracking any
    /// background runs still recorded on disk from a prior process (R-SA-093); on
    /// `SessionShutdown`, mirror pi's own teardown (`extension/index.ts:644-680`) for every piece
    /// this crate has a live analog of — stop the completion watcher (pi `stopResultWatcher()`),
    /// abort+clear the job tracker's poll loop and in-memory job map (pi `clearInterval(state.poller)`
    /// + `state.asyncJobs.clear()`), and clear the captured parent-session anchor (pi `delete
    /// process.env[SUBAGENT_PARENT_SESSION_ENV]`). Pieces pi's teardown also touches that this crate
    /// has no live analog for yet are deliberately left alone here: pi's `pendingForegroundControlNotices`/
    /// `cleanupTimers`/slash-snapshot state and its two slash-invoked-run bridges
    /// (`slashBridge`/`promptTemplateBridge`, whose `cancelAll()` aborts in-flight slash-dispatched
    /// runs) have no ported equivalent in this crate (slash dispatch here is a direct in-process call
    /// via `dispatch_slash`, R-SA-130, not an event-bus bridge with its own cancellable in-flight
    /// registry); pi's `ui.setWidget(WIDGET_KEY, undefined)` has no analog since this crate renders no
    /// persistent host-UI widget. None of this omitted state affects whether a detached background
    /// run survives shutdown — a detached run MUST continue to completion even after the
    /// orchestrating process exits (R-SA-071/DI-SA-8), and nothing here sends it any signal.
    async fn on_event(&self, ev: &HostEvent, ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::SessionStart { .. } => {
                // T6's once-per-load housekeeping (`ensureAccessibleDir`/`cleanupOldChainDirs`/
                // `cleanupAllArtifactDirs`) now runs in `init()`, above — matching pi's own
                // registration-time closure body exactly (`extension/index.ts:257-264` runs once,
                // NOT per `session_start`). What DOES belong here, per-session, is pi's OWN
                // `session_start` handler body (`extension/index.ts:628-642`): the per-session-file
                // artifact sweep (`cleanupOldArtifacts(getArtifactsDir(sessionFile))`,
                // `resetSessionState`'s `cleanupSessionArtifacts` at `extension/index.ts:591-600`),
                // best-effort — a failure here must never block a session from starting.
                if let Some(session_file) =
                    self.executor.host_services().and_then(|s| s.session_file())
                {
                    let artifacts_dir =
                        crate::artifacts::resolve_artifacts_dir(Some(&session_file), None, &ctx.cwd);
                    crate::artifacts::cleanup_old_artifacts(
                        &artifacts_dir,
                        crate::artifacts::DEFAULT_CLEANUP_DAYS,
                    );
                }

                // R-SA-P1 (port doc §4 P-4): capture the canonical parent-session anchor ONCE from
                // the live session id (P-2) at the root orchestrator's SessionStart (depth 0 — a
                // `ChildSafe` child never subscribes to SessionStart, so this arm only runs for the
                // root). Every child this session spawns then inherits it via the spawn env overlay,
                // so the permission companion's child→parent ask-forwarding spool can address this
                // session's inbox.
                self.executor.capture_parent_session_anchor();

                // pi `resetSessionState`'s `state.subagentSpawns = { sessionId: state.currentSessionId,
                // count: 0 }` (`extension/index.ts:590`): a new session always starts with a fresh
                // per-session spawn budget. Ordered AFTER the anchor capture so the budget is stamped
                // with THIS session's id.
                self.executor.reset_spawn_budget();

                self.executor.resume_tracking(&ctx.cwd).await;
                // C6: install the background-completion watcher (notify.ts / result-watcher.ts) so a
                // detached run that finishes during this session surfaces its `subagent-notify`
                // message (with `triggerTurn`) and has its result file deleted (R-SA-099/101). When the
                // P-1 host-services slot is bound this installs the live turn-injecting
                // `HostServicesCompletionSink` (R-SA-101); otherwise the stderr LoggingCompletionSink.
                self.executor.install_completion_watcher(&ctx.cwd).await;
            }
            HostEvent::SessionShutdown { .. } => {
                self.executor.teardown_session().await;
            }
            _ => {}
        }
        HookOutcome::Noop
    }

    /// Dispatch a registered slash command through the SAME executor the `subagent` tool uses
    /// (R-SA-130: "a direct in-process function call" — this native extension has no
    /// module-decoupling boundary to bridge, unlike pi-subagents' own event-bus slash-bridge).
    async fn execute_command(
        &self,
        name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        ctx.require_command_tier()?;

        let Some(command) = SlashCommandName::from_str_exact(name) else {
            return Err(ExtError::Component(format!(
                "native extension has no handler for command `{name}`"
            )));
        };

        let output = self
            .dispatch_slash(command, args, &ctx.cwd)
            .await
            .unwrap_or_else(|err| format!("subagent command failed: {err}"));

        Ok(Some(output))
    }

    /// Late-bind the live capability backend (P-1, reconciliation §2 item 1). The session builder
    /// calls this via `load_native_with_services` (facade.rs:181) BEFORE `init`; stash the shared
    /// `Arc` in the executor's slot so the `SessionStart` anchor capture (R-SA-P1), the fork-context
    /// resolver (blocker #4), and the completion watcher's turn-injecting sink (R-SA-101) all reach
    /// the live session id/file + `inject_message` from OUTSIDE any `HostCtx`. Idempotent.
    fn set_host_services(&self, services: Arc<dyn cyrup_ext::host::HostServices>) {
        self.executor.set_host_services(services);
    }
}

impl SubagentsExtension {
    /// The single shared dispatch body [`NativeExtension::execute_command`] calls into
    /// (R-SA-130). Parses `args` via the real, already-built parsers in
    /// [`crate::registration::slash_commands`], then routes to [`SubagentExecutor`] exactly as
    /// the tool itself does for `/run`; the remaining commands route to their own
    /// already-implemented subsystem entry points (`registration::doctor`/`cost`/`profiles`).
    async fn dispatch_slash(
        &self,
        command: SlashCommandName,
        args: &str,
        cwd: &Path,
    ) -> Result<String, SubagentError> {
        match command {
            // Slash-live-state (T8, partial — pi `slash/slash-live-state.ts`): pi posts an IMMEDIATE
            // in-transcript placeholder message the moment `/run` is invoked, then UPDATES IT IN
            // PLACE as the run streams and finally renders the completed result over the same
            // transcript entry. The crate cannot post that immediate placeholder or update it in
            // place today: `NativeExtension::execute_command` returns a single `Option<String>` (its
            // one final transcript entry) and its `HostCtx` exposes no transcript-message sink and no
            // update-in-place handle. So the crate-side minimum here is to make the SINGLE returned
            // entry read as the placeholder RESOLVED to completion — a completion summary (status +
            // agent + tool/token stats) over the delivered output, exactly what pi's placeholder
            // renders once the run settles (`renderSubagentResult`). The immediate placeholder + live
            // in-place update is the remaining outer-layer step, gated on a host transcript-update
            // channel `cyrup-tui`/`HostCtx` must expose (the tool path already streams live progress
            // via `ToolUpdateSink`, C19 — the slash path has no equivalent sink yet).
            SlashCommandName::Run => {
                let parsed = slash_commands::parse_run_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
                let model = parsed.config.model.clone().map(ModelId::from);
                // SUBA-002 — charge the per-SESSION spawn budget on the SLASH surface too. Upstream
                // gets this for free: `/run`'s handler calls `runSlashSubagent` -> `requestSlashRun`
                // -> the bridge wired at `extension/index.ts:396-401` -> `executeSubagentCollapsed`
                // -> the SAME `executor.execute` the tool uses, whose `reserveSubagentSpawns`
                // (`subagent-executor.ts:266-282`, called at `:3434-3441`) therefore covers both
                // surfaces. Here `dispatch_slash` is an independent entry point into
                // `SubagentExecutor`, so without this the budget would be enforced on the tool path
                // ONLY and a session that had exhausted it could keep fanning out via `/run`.
                //
                // `/run` is pi's SINGLE shape (`params.agent` set, no `tasks`/`chain`), so
                // `countRequestedSubagentSpawns` bills it exactly `1` — the same `1` whether the run
                // goes foreground or background, which is why the charge sits after parsing and
                // ahead of the mode branch below rather than inside either arm (charging once, never
                // twice, and never a count that differs from what actually gets spawned).
                let run_cfg = self.executor.config_snapshot().await;
                self.executor
                    .reserve_subagent_spawns(1, run_cfg.max_subagent_spawns_per_session)
                    .map_err(SubagentError::SpawnLimitExceeded)?;
                if parsed.flags.background {
                    let run_id = self
                        .executor
                        .spawn_background(
                            cwd,
                            &parsed.agent,
                            &parsed.task,
                            context,
                            model,
                            AgentReadScope::Both,
                        )
                        .await?;
                    Ok(format!("Background subagent run started: {run_id}"))
                } else {
                    let result = self
                        .executor
                        .run_foreground(cwd, &parsed.agent, &parsed.task, context, model, None)
                        .await?;
                    Ok(format_slash_run_completion(&result))
                }
            }
            // pi's `/subagents-doctor` handler calls `runSlashSubagent(pi, ctx, { action: "doctor"
            // })` — no `sessionDir` override on the slash-command surface (`slash-commands.ts:1081-
            // 1087`), so `formatConfiguredSessionDir` falls through to the configured default.
            SlashCommandName::SubagentsDoctor => Ok(self.executor.run_doctor(cwd, None).await),
            SlashCommandName::SubagentsProfiles => {
                let profiles_dir = self.profiles_dir();
                let profiles = crate::registration::profiles::describe_profiles(&profiles_dir)?;
                if profiles.is_empty() {
                    Ok("No saved subagent profiles.".to_string())
                } else {
                    Ok(profiles
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join("\n"))
                }
            }
            SlashCommandName::SubagentsLoadProfile => {
                let name = slash_commands::parse_subagents_load_profile_command(args)
                    .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
                self.load_profile_into_settings(&name).await
            }
            SlashCommandName::SubagentCost => Ok(self.executor.run_cost_report(cwd).await),

            // -----------------------------------------------------------------------------------
            // /chain — linear sequence (with optional inline parallel groups), R-SA-129/§5.1/§5.3.
            // Routes into the SAME chain-graph walker (`spawn::chain_graph::walk_chain`) and the
            // SAME `ExecSingleStepExecutor` subprocess-spawning adapter the hop-2 background
            // runner uses for a saved/async chain (R-SA-130: one execution code path, never a
            // second divergent implementation for the foreground slash-command shape).
            // -----------------------------------------------------------------------------------
            SlashCommandName::Chain => {
                let parsed = slash_commands::parse_chain_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
                // `/chain` carries no separate top-level task arg — the first step's task seeds the
                // chain, so `{task}` falls back to it (`first_step_task`).
                self.run_or_background_chain(cwd, parsed.chain, RunMode::Chain, context, parsed.flags.background, None)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /parallel — a single static-width fan-out group (R-SA-129/§5.3). Represented as a
            // ONE-element `ChainGraph` whose sole element is a `RunnerStep::ParallelGroup`, so it
            // is dispatched by the identical `walk_chain`/`run_bounded` machinery a parallel GROUP
            // inside a longer `/chain` uses — never a second, parallel-only dispatch path.
            // -----------------------------------------------------------------------------------
            SlashCommandName::Parallel => {
                let parsed = slash_commands::parse_parallel_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
                let cfg = self.executor.config_snapshot().await;
                let group = RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
                    steps: parsed.tasks,
                    concurrency: cfg.parallel_concurrency(),
                    fail_fast: false,
                    worktree: false,
                });
                // `/parallel` carries no separate top-level task arg — `{task}` falls back to the
                // group's first task (`first_step_task`).
                self.run_or_background_chain(cwd, vec![group], RunMode::Parallel, context, parsed.flags.background, None)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /run-chain — invoke a saved chain (`.chain.md`/`.chain.json`) by name (R-SA-129).
            // Resolves the chain through the REAL discovery pipeline (R-SA-019/020), then routes
            // into the identical `walk_chain` machinery `/chain` itself uses.
            // -----------------------------------------------------------------------------------
            SlashCommandName::RunChain => {
                let parsed = slash_commands::parse_run_chain_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                let context = if parsed.flags.fork { Some(ContextMode::Fork) } else { None };
                // R-SA-055 (SAFETY-CRITICAL): the depth guard runs FIRST — before `resolve_chain`
                // below, which is a real discovery filesystem scan (R-SA-019/020) — so a blocked
                // call never touches discovery at all, not even for the saved-chain lookup this
                // command performs ahead of `run_or_background_chain`'s own (correct, but
                // necessarily later) independent re-check.
                let cfg = self.executor.config_snapshot().await;
                let depth = resolve_effective_depth(cfg.max_subagent_depth);
                if crate::spawn::depth::is_blocked(&depth) {
                    return Err(SubagentError::DepthExceeded {
                        current: depth.current_depth,
                        max: depth.max_depth,
                    });
                }
                let chain = self.executor.resolve_chain(cwd, &parsed.chain_name)?;
                // The functionality spec's own usage grammar (`/run-chain <chainName> -- <task>`)
                // gives no further detail on how the supplied task text combines with a saved
                // chain's own per-step task text beyond pi-subagents' `mapSavedChainSteps`
                // reference (`registration/slash_commands.rs`'s own module doc). The most complete
                // honest reading: the supplied task text seeds the FIRST step only (mirroring
                // `/chain`'s own "first element's task is what starts the chain" convention,
                // R-SA-053's own "cross-step data flows via named outputs from here forward"
                // model) — every later step keeps its saved, fixed task text verbatim.
                // A saved chain parses into `ChainStepConfig` authoring shapes (T0.2); lower each
                // to the runtime `RunnerStep` union here via the structural bridge — it carries the
                // real agent NAME (never a placeholder persona; name resolution stays the
                // executor's job) and defers plan-time model/acceptance enrichment. A group step's
                // omitted `concurrency` falls back to `cfg.parallel_concurrency()`, mirroring
                // `/parallel`'s own default above.
                let graph: Vec<RunnerStep> = chain
                    .steps
                    .iter()
                    .map(|step| {
                        crate::discovery::chains::chain_step_to_runner_step(
                            step,
                            cfg.parallel_concurrency(),
                        )
                    })
                    .collect();
                let steps = seed_first_step_task(graph, &parsed.task);
                // `/run-chain <name> -- <task>`: the supplied task seeds the first step AND is the
                // run-wide `{task}` value (pi `originalTask = params.task`).
                let task = (!parsed.task.trim().is_empty()).then(|| parsed.task.clone());
                self.run_or_background_chain(cwd, steps, RunMode::Chain, context, parsed.flags.background, task)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-models — report the RUNTIME builtin-agent -> model mapping (pi
            // `handleModels`, slash-commands.ts:1090-1111), NOT a dump of the static provider
            // catalog: each discovered builtin persona's effective model + provenance, optionally
            // filtered to one builtin.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsModels => {
                let parsed = slash_commands::parse_subagents_models_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                Ok(self.executor.run_models_report(cwd, parsed.agent.as_deref()))
            }

            // -----------------------------------------------------------------------------------
            // /subagents-refresh-provider-models — R-SA-129/142. The catalog-refresh ALGORITHM
            // (probe scheduling, catalog diffing, observed/derived classification) is explicitly
            // deferred (func-SA §9 item 31) — this crate has no provider-catalog CACHE FILE writer
            // anywhere yet, only `registration/doctor.rs`'s freshness-checking READER
            // (`provider_catalog_path`). The honest, most-complete implementation available today:
            // validate the provider name (R-SA-142's path-traversal guard, since this name feeds
            // the SAME cache-file path `doctor.rs` stats), confirm it resolves against the real
            // built-in model registry, and write/refresh a minimal, genuinely-real freshness-cache
            // marker file at the exact path `doctor.rs`'s own `check_provider_catalog_freshness`
            // reads — so `/subagents-doctor`'s freshness check (R-SA-131 item f) observes a REAL
            // effect of running this command, not a no-op. What remains explicitly OUT OF SCOPE
            // (per the same deferred item): actually spawning a probe subprocess against the named
            // provider's live API to discover/diff its real-time model list.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsRefreshProviderModels => {
                let parsed = slash_commands::parse_subagents_refresh_provider_models_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                self.refresh_provider_catalog_cache(cwd, &parsed.provider, parsed.force)
                    .await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-generate-profiles — R-SA-129/140/141/142. Profile *authoring* (writing a
            // NEW named-profile JSON file) is explicitly out of `registration/profiles.rs`'s
            // documented scope (that module is read-only over an already-authored profiles
            // directory — see its own module doc's "Deferred to a later phase" section) — full
            // provider-catalog-driven profile GENERATION is the same deferred item as
            // `/subagents-refresh-provider-models` (func-SA §9 item 31). The honest, most-complete
            // implementation available today: validate the provider name (R-SA-142), confirm it
            // resolves against the real built-in model registry, and WRITE the two named profiles
            // (`<provider>.quota`/`<provider>.quality`) this command's own usage string promises,
            // selecting the catalog's cheapest/highest-capability model for that provider as the
            // profile's `defaultModel` — a genuine, on-disk, load-through-`/subagents-load-profile`
            // artifact, not a placeholder acknowledgement.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsGenerateProfiles => {
                let provider = slash_commands::parse_subagents_generate_profiles_command(args)
                    .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
                self.generate_provider_profiles(&provider).await
            }

            // -----------------------------------------------------------------------------------
            // /subagents-check-profile — R-SA-129/140/141/142. Loads the named profile through the
            // real `registration::profiles::load_profile` primitive and checks every
            // `overrides.<agent>.model`/`defaultModel` value it declares against the real
            // built-in model registry, reporting which model references are genuinely known vs.
            // unresolvable — the honest, catalog-backed half of "still points to usable models" this command's
            // own usage string promises; a genuine LIVE reachability probe against the provider's
            // API is the same explicitly deferred item as the two commands above.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsCheckProfile => {
                let name = slash_commands::parse_subagents_check_profile_command(args)
                    .map_err(|e| SubagentError::UnsafePathToken(e.message))?;
                let profiles_dir = self.profiles_dir();
                let profile = crate::registration::profiles::load_profile(&profiles_dir, &name)?;
                Ok(render_profile_check_report(&name, &profile).await)
            }

            // -----------------------------------------------------------------------------------
            // /subagents-companions — R-SA-129. Faithful port of pi's `collectCompanionStatuses` +
            // `buildCompanionDoctorLines`/`buildCompanionCommandStatus` (status) and
            // `updateCompanionDismissal` (hide/show), `companion-suggestions.ts:201-351`. Neither
            // `pi-intercom` nor `pi-prompt-template-model` is a dynamically-loadable npm package in
            // this crate's architecture, so pi's `pi.getAllTools()`/`pi.getCommands()` sourceInfo
            // scan (its "is the companion package's tool/command active in THIS session" probe) has
            // no cyrup analogue and always resolves to "not active" here — the SAME status line pi
            // itself renders whenever a companion package genuinely is not installed, which is this
            // crate's actual, permanent state (func-SA §9 item 25). The status report shape, the
            // hide/show dismissal semantics, and the persisted config store they both read/write are
            // ported exactly: `status` always renders the full per-package doctor-line report
            // (`build_companion_doctor_lines`), and `hide`/`show` mutate
            // `SubagentExtensionConfig::companion_suggestions` — the SAME store `status`'s dismissed-
            // detection reads back — rather than an unkeyed side-marker file nothing else consults.
            // -----------------------------------------------------------------------------------
            SlashCommandName::SubagentsCompanions => {
                let parsed = slash_commands::parse_subagents_companions_command(args)
                    .map_err(|e| SubagentError::MalformedSettings(e.message))?;
                self.handle_companions_command(parsed, cwd).await
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // /chain, /parallel, /run-chain shared foreground-vs-background dispatch (R-SA-129/130)
    // ---------------------------------------------------------------------------------------

    /// Shared tail for `/chain`, `/parallel`, and `/run-chain`: resolve every step's effective
    /// fork-context (R-SA-137's eager whole-batch rule) — an omitted call-site `context` defers to
    /// each step's agent's own `default_context`, and each forking step gets its OWN per-index branch
    /// (R-SA-138: a sibling step's own explicit choice is never overridden) — then either walk the
    /// graph to completion in the foreground or hand it to [`SubagentExecutor::spawn_background_steps`].
    async fn run_or_background_chain(
        &self,
        cwd: &Path,
        graph: Vec<RunnerStep>,
        mode: RunMode,
        context: Option<ContextMode>,
        background: bool,
        task: Option<String>,
    ) -> Result<String, SubagentError> {
        if graph.is_empty() {
            return Ok("chain has no steps to run".to_string());
        }

        // SUBA-002 — charge the per-SESSION spawn budget for the chain-shaped SLASH surfaces
        // (`/chain`, `/parallel`, `/run-chain`), which all funnel through this one wrapper. Upstream
        // needs no charge here because those handlers call `runSlashSubagent` -> `requestSlashRun`
        // -> the bridge at `extension/index.ts:396-401` -> `executeSubagentCollapsed` -> the SAME
        // `executor.execute` the tool uses, so its `reserveSubagentSpawns`
        // (`subagent-executor.ts:266-282`, called at `:3434-3441`) already covers them; this crate
        // reaches `run_or_background_graph` directly from here, so the reserve has to be repeated.
        //
        // Placed AFTER the empty-graph short-circuit and after the caller has fully resolved the
        // mode into a concrete `RunnerStep` list, so the number billed is exactly the number of
        // children this run can spawn (pi's own "count the settled mode, not the request shape"
        // ordering). It is NOT double-charged with the tool path: the `subagent` tool's own
        // `chain[]`/`tasks[]` shapes reserve once in `SubagentTool::execute` and then reach
        // `run_or_background_graph` via `route_chain_mode`/`route_parallel_mode`, never through this
        // slash-only wrapper.
        let budget_cfg = self.executor.config_snapshot().await;
        self.executor
            .reserve_subagent_spawns(
                count_graph_requested_spawns(&graph, &budget_cfg),
                budget_cfg.max_subagent_spawns_per_session,
            )
            .map_err(SubagentError::SpawnLimitExceeded)?;

        // R-SA-130: delegate to the ONE shared plan-execution path `SubagentExecutor` exposes (the
        // identical method the `subagent` tool's `chain[]`/`tasks[]` shapes route through), then
        // render the sequential/per-step text this slash surface presents. Depth guard, plan-time
        // persona resolution (T0.1/C13), fork-context resolution (R-SA-137), and the foreground-vs-
        // background fork all live inside `run_or_background_graph` now, so both call sites share
        // them verbatim rather than each re-implementing the tail.
        // The slash-command surface (`/chain`, `/parallel`, `/run-chain`) has no host
        // `ToolCallId`/cancellation seam of its own (`NativeExtension::execute_command` takes no
        // cancel token) — a fresh, never-cancelled token here preserves this path's pre-existing
        // behavior exactly; only the `subagent` TOOL's `execute` threads the live host token
        // (`SubagentTool::execute` -> `route_parallel_mode`/`route_chain_mode`).
        match self
            .executor
            .run_or_background_graph(
                cwd,
                graph,
                mode,
                context,
                background,
                task,
                CancelToken::new(),
                // The slash-command surface (`/chain`/`/parallel`/`/run-chain`) exposes no timeout
                // param at all (pi's `timeoutMs`/`maxRuntimeMs` are tool-only) — always `None`.
                None,
            )
            .await?
        {
            GraphRunOutcome::Background(run_id) => {
                Ok(format!("Background subagent run started: {run_id}"))
            }
            GraphRunOutcome::Foreground {
                run_id: _,
                results,
                is_group,
                groups,
            } => Ok(render_chain_results(&results, &is_group, &groups)),
        }
    }

    // ---------------------------------------------------------------------------------------
    // /subagents-models, /subagents-refresh-provider-models, /subagents-generate-profiles,
    // /subagents-check-profile: cyrup-provider model-registry backed, with REAL live-probe
    // subprocess classification (pi `probeModel`/`classifyModel`, profiles.ts:250-335) — see
    // the free functions just above [`SubagentsExtension::provider_ranked_full_ids`] for the
    // ported probe/classification pipeline.
    // ---------------------------------------------------------------------------------------

    /// The path `registration/doctor.rs`'s `check_provider_catalog_freshness` (R-SA-131 item f)
    /// reads: refreshing/generating a provider catalog also touches this shared freshness marker so
    /// `/subagents-doctor`'s freshness check observes that a refresh genuinely ran.
    fn provider_catalog_cache_path(&self, cwd: &Path) -> PathBuf {
        let _ = cwd;
        dirs_home()
            .join(".cyrup")
            .join("subagents")
            .join("provider-catalog-cache.json")
    }

    /// A just-refreshed catalog's usable, RANKED, non-dominated `provider/id` full-id list — the
    /// pure, synchronous half of pi `generateProfilesForProvider`'s pipeline (profiles.ts:591-592:
    /// `catalog.models.filter(catalogModelIsUsable)` then `filterDominatedModels`, ordered by
    /// `derived.profileRank` ascending since [`write_provider_catalog_file`] already sorted
    /// `catalog.models` that way — profiles.ts:567). Cross-references each catalog entry's
    /// `probe_status`/`profile_rank` (computed once, by the real live-probe pass that wrote
    /// `catalog`) against the model registry for the `cost`/`reasoning`/`context_window`/`max_tokens`
    /// axes [`dominates`] needs, so a caller never re-probes.
    fn provider_ranked_full_ids_from_catalog(
        provider: &str,
        catalog: &crate::registration::profiles::ProviderModelCatalog,
    ) -> Vec<String> {
        let registry = registry_models();
        let mut candidates: Vec<RankedCandidate> = Vec::new();
        for entry in &catalog.models {
            if !probe_status_is_usable(&entry.probe_status) {
                continue;
            }
            let Some(m) = registry
                .iter()
                .find(|sm| sm.provider.as_str() == provider && sm.id.as_str() == entry.id)
            else {
                continue;
            };
            candidates.push(RankedCandidate {
                full_id: entry.full_id.clone(),
                cost: combined_cost(&m.cost).unwrap_or(0.0),
                profile_rank: entry.profile_rank,
                reasoning: m.reasoning,
                context_window: m.context_window,
                max_tokens: m.max_tokens,
            });
        }
        let mut candidates = filter_dominated(candidates);
        candidates.sort_by(|a, b| {
            a.profile_rank.cmp(&b.profile_rank).then_with(|| a.full_id.cmp(&b.full_id))
        });
        candidates.into_iter().map(|c| c.full_id).collect()
    }

    /// Build and persist a per-provider [`crate::registration::profiles::ProviderModelCatalog`]
    /// from the model registry ([`registry_models`], pi's `ctx.modelRegistry.getAvailable()`),
    /// REAL-probing every candidate model via [`probe_model`] and classifying it
    /// via [`classify_model`] (pi `refreshProviderModelCatalog`, profiles.ts:510-566), sorted by
    /// `profileRank` ascending then `fullId` (pi profiles.ts:567), plus refreshing the shared
    /// doctor freshness marker. Returns the model count.
    async fn write_provider_catalog_file(&self, provider: &str) -> Result<usize, SubagentError> {
        let matches: Vec<cyrup_provider::Model> =
            registry_models().iter().filter(|m| m.provider.as_str() == provider).cloned().collect();
        let ctx = build_classification_context(&matches);
        let mut models: Vec<crate::registration::profiles::ProviderCatalogModel> =
            Vec::with_capacity(matches.len());
        for m in &matches {
            let full_id = format!("{}/{}", m.provider.as_str(), m.id.as_str());
            let classification = classify_model(m, &ctx);
            let probe = probe_model(&full_id).await;
            models.push(crate::registration::profiles::ProviderCatalogModel {
                id: m.id.as_str().to_string(),
                full_id,
                profile_rank: classification.profile_rank,
                probe_status: probe.status.as_str().to_string(),
            });
        }
        // pi `models.sort((a,b) => a.derived.profileRank - b.derived.profileRank ||
        // a.fullId.localeCompare(b.fullId))`, profiles.ts:567.
        models.sort_by(|a, b| a.profile_rank.cmp(&b.profile_rank).then_with(|| a.full_id.cmp(&b.full_id)));
        let model_count = models.len();
        let file = crate::registration::profiles::ProviderModelCatalog {
            provider: provider.to_string(),
            refreshed_at_epoch_ms: now_epoch_ms(),
            max_age_days: crate::registration::profiles::DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS,
            // pi `sources` (profiles.ts:572): `["runtime-registry", ...(probe ? ["live-probe"] :
            // []), "heuristic-classifier"]` — this port always probes (no exposed `--no-probe`
            // slash-command flag, matching every real pi call site, which never passes
            // `probe: false` either).
            sources: vec![
                "runtime-registry".to_string(),
                "live-probe".to_string(),
                "heuristic-classifier".to_string(),
            ],
            models,
        };
        crate::registration::profiles::write_provider_catalog(&self.profiles_dir(), &file)?;

        // Also touch the shared freshness marker `registration/doctor.rs` stats (R-SA-131 item f).
        let cache_path = self.provider_catalog_cache_path(Path::new("."));
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(SubagentError::Spawn)?;
        }
        let marker = serde_json::json!({
            "provider": provider,
            "modelCount": model_count,
            "refreshedAtEpochMs": file.refreshed_at_epoch_ms,
        });
        write_atomic_json(&cache_path, &marker)
            .await
            .map_err(SubagentError::Spawn)?;
        Ok(model_count)
    }

    /// `/subagents-refresh-provider-models <provider> [--force]` (pi `refreshProviderModelCatalog`,
    /// profiles.ts:489-577). Writes a per-provider catalog file under
    /// `providers/<provider>.models.json`, REAL-probing + classifying every candidate model
    /// ([`write_provider_catalog_file`]); honors `--force` by reusing a still-fresh cache when
    /// `!force` and rewriting otherwise.
    async fn refresh_provider_catalog_cache(
        &self,
        cwd: &Path,
        provider: &str,
        force: bool,
    ) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        let profiles_dir = self.profiles_dir();

        // --force gate (pi `if (!options.force) { ... reuse fresh ... }`, profiles.ts:498-503):
        // a still-fresh cache is reused verbatim unless --force forces a rewrite. `.filter` keeps
        // the still-fresh cache and drops a stale one, avoiding nested `if`s.
        let fresh_cache = if force {
            None
        } else {
            crate::registration::profiles::read_provider_catalog(&profiles_dir, provider)?.filter(
                |existing| {
                    !crate::registration::profiles::is_provider_catalog_stale(
                        existing,
                        now_epoch_ms(),
                        existing.max_age_days,
                    )
                },
            )
        };
        if let Some(existing) = fresh_cache {
            return Ok(format!(
                "subagents-refresh-provider-models: provider '{provider}' — fresh cache reused \
                 ({} model(s)); pass --force to rewrite.",
                existing.models.len()
            ));
        }

        // pi `if (availableModels.length === 0) throw new Error(...)` (profiles.ts:506-508) — a
        // command ERROR, not an informational success string.
        let has_models = registry_models().iter().any(|m| m.provider.as_str() == provider);
        if !has_models {
            return Err(SubagentError::MalformedSettings(format!(
                "No models found in the current registry for provider '{provider}'."
            )));
        }
        let _ = cwd;
        let model_count = self.write_provider_catalog_file(provider).await?;

        Ok(format!(
            "subagents-refresh-provider-models: refreshed catalog cache for '{provider}' \
             ({model_count} model(s)), live-probed and classified."
        ))
    }

    /// `/subagents-generate-profiles <provider>` (pi `generateProfilesForProvider`,
    /// profiles.ts:579-606). Refreshes the per-provider catalog (REAL-probing + classifying every
    /// candidate model), filters to usable + non-dominated models, then writes `<provider>.quota`
    /// and `<provider>.quality` profiles — EACH carrying the full 8-agent tier map PLUS a
    /// representative `subagents.defaultModel` (the medium tier, the fallback for non-builtin
    /// agents) ([`crate::registration::profiles::build_profile_file`]).
    async fn generate_provider_profiles(&self, provider: &str) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        // pi's refreshProviderModelCatalog (called internally by generateProfilesForProvider,
        // profiles.ts:586) throws BEFORE any probing when the registry has zero models
        // (profiles.ts:506-508) — checked here, up front, so this mirrors that ordering exactly.
        let has_models = registry_models().iter().any(|m| m.provider.as_str() == provider);
        if !has_models {
            return Err(SubagentError::MalformedSettings(format!(
                "No models found in the current registry for provider '{provider}'."
            )));
        }
        // pi's generateProfilesForProvider refreshes the catalog first (profiles.ts:586).
        self.write_provider_catalog_file(provider).await?;

        let profiles_dir = self.profiles_dir();
        let catalog = crate::registration::profiles::read_provider_catalog(&profiles_dir, provider)?
            .ok_or_else(|| {
                SubagentError::MalformedSettings(format!(
                    "provider catalog for '{provider}' is missing immediately after refresh"
                ))
            })?;
        let ranked = Self::provider_ranked_full_ids_from_catalog(provider, &catalog);
        // pi `if (profileModels.length === 0) throw new Error(...)` (profiles.ts:593-595) — a
        // command ERROR, not an informational success string.
        if ranked.is_empty() {
            return Err(SubagentError::MalformedSettings(format!(
                "Provider '{provider}' has no usable models after filtering."
            )));
        }

        let generated = crate::registration::profiles::generate_provider_profiles(
            &profiles_dir,
            provider,
            &ranked,
        )?;

        Ok(format!(
            "Generated subagent profiles\n\
             Provider: {provider}\n\
             Quota: {quota}\n  cheap={qc}\n  medium={qm}\n  strong={qs}\n\
             Quality: {quality}\n  cheap={lc}\n  medium={lm}\n  strong={ls}\n\
             (8-agent tier map; live-probed and classified)",
            quota = generated.quota_path.display(),
            qc = generated.quota_models.cheap,
            qm = generated.quota_models.medium,
            qs = generated.quota_models.strong,
            quality = generated.quality_path.display(),
            lc = generated.quality_models.cheap,
            lm = generated.quality_models.medium,
            ls = generated.quality_models.strong,
        ))
    }
    // ---------------------------------------------------------------------------------------
    // /subagents-companions — pi `companion-suggestions.ts`. See the doc comment at this command's
    // `dispatch_slash` arm for why "active" is unconditionally `false` here.
    // ---------------------------------------------------------------------------------------

    /// The on-disk store `/subagents-companions hide|show` read-modify-writes, and `status`'s
    /// dismissed-detection reads back — pi's single per-installation `config.json`
    /// (`extension/config.ts:6-8`), reduced to this crate's own tier-3 `SubagentExtensionConfig`
    /// file (the same file this crate's other doc comments already point to as the tier-3 store,
    /// e.g. [`Self::profiles_dir`]'s sibling paths).
    fn extension_config_path(&self) -> PathBuf {
        dirs_home().join(".cyrup").join("subagents").join(CONFIG_FILE)
    }

    async fn handle_companions_command(
        &self,
        parsed: slash_commands::CompanionsCommand,
        cwd: &Path,
    ) -> Result<String, SubagentError> {
        use slash_commands::CompanionsCommand;
        match parsed {
            CompanionsCommand::Status => {
                let config = self.executor.config.lock().await.clone();
                let statuses = collect_companion_statuses(&config, cwd, self.executor.orchestrator_intercom_target());
                Ok(build_companion_command_status(&statuses))
            }
            CompanionsCommand::Hide { package, scope } => {
                let dismissal_scope = match scope {
                    slash_commands::CompanionsScope::User => CompanionDismissalScope::User,
                    slash_commands::CompanionsScope::Workspace => CompanionDismissalScope::Workspace,
                };
                self.update_companion_dismissal(&package, dismissal_scope, cwd).await?;
                // pi's exact two reply strings (`companion-suggestions.ts:347-350`).
                Ok(match scope {
                    slash_commands::CompanionsScope::User => {
                        format!("Hid {package} recommendations for this user.")
                    }
                    slash_commands::CompanionsScope::Workspace => {
                        format!("Hid {package} recommendations for this workspace.")
                    }
                })
            }
            CompanionsCommand::Show { package } => {
                self.update_companion_dismissal(&package, CompanionDismissalScope::Show, cwd)
                    .await?;
                // pi's fixed reply text regardless of whether anything was actually dismissed
                // (`companion-suggestions.ts:340-341`).
                Ok(format!("Showing {package} recommendations for this workspace again."))
            }
        }
    }

    /// Port of pi's `updateCompanionDismissal` (`companion-suggestions.ts:290-322`): read the
    /// on-disk extension config fresh, apply the SAME user/workspace/show mutation pi's updater
    /// applies, write it back, then mirror the mutated field into the in-memory config so this
    /// process's own subsequent `status` calls observe the change immediately (no restart needed) —
    /// exactly like pi's module-scope `config` variable being reassigned right after `saveConfig`.
    async fn update_companion_dismissal(
        &self,
        package_name: &str,
        scope: CompanionDismissalScope,
        cwd: &Path,
    ) -> Result<(), SubagentError> {
        let workspace_key = companion_workspace_key(cwd);
        let path = self.extension_config_path();
        let mut on_disk = read_extension_config_for_update(&path).await?;

        let mut companion_suggestions = match on_disk.companion_suggestions.take() {
            Some(CompanionSuggestionsSetting::Toggle(false)) => CompanionSuggestionsConfig {
                enabled: Some(false),
                packages: None,
            },
            Some(CompanionSuggestionsSetting::Toggle(_)) | None => {
                CompanionSuggestionsConfig::default()
            }
            Some(CompanionSuggestionsSetting::Config(cfg)) => cfg,
        };
        let mut packages = companion_suggestions.packages.take().unwrap_or_default();
        let mut package_cfg = packages.remove(package_name).unwrap_or_default();
        let mut dismissed = package_cfg.dismissed.take().unwrap_or_default();

        match scope {
            CompanionDismissalScope::User => dismissed.user = Some(true),
            CompanionDismissalScope::Workspace => {
                let mut workspaces = dismissed.workspaces.take().unwrap_or_default();
                if !workspaces.iter().any(|w| w == &workspace_key) {
                    workspaces.push(workspace_key.clone());
                }
                dismissed.workspaces = Some(workspaces);
            }
            CompanionDismissalScope::Show => {
                dismissed.user = None;
                let workspaces: Vec<String> = dismissed
                    .workspaces
                    .take()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|w| w != &workspace_key)
                    .collect();
                dismissed.workspaces = if workspaces.is_empty() {
                    None
                } else {
                    Some(workspaces)
                };
            }
        }

        package_cfg.dismissed = if dismissed.user.is_some() || dismissed.workspaces.is_some() {
            Some(dismissed)
        } else {
            None
        };
        packages.insert(package_name.to_string(), package_cfg);
        companion_suggestions.packages = Some(packages);
        on_disk.companion_suggestions =
            Some(CompanionSuggestionsSetting::Config(companion_suggestions.clone()));

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(SubagentError::Spawn)?;
        }
        write_atomic_json(&path, &on_disk).await.map_err(SubagentError::Spawn)?;

        self.executor.config.lock().await.companion_suggestions =
            Some(CompanionSuggestionsSetting::Config(companion_suggestions));
        Ok(())
    }

    fn profiles_dir(&self) -> PathBuf {
        dirs_home().join(".cyrup").join("subagents").join("profiles")
    }

    /// The user-scope `settings.json` the extension's discovery reads its `subagents.*` layer back
    /// from (`~/.cyrup/agents/settings.json` — the SAME file [`Self::discovery_config`] loads the
    /// user settings from). `/subagents-load-profile` writes the loaded profile's `subagents` block
    /// here so the next discovery pass picks it up, exactly as pi's `applySubagentProfile` writes to
    /// the same `getUserSettingsPath()` its discovery reads.
    fn user_settings_path(&self) -> PathBuf {
        dirs_home()
            .join(".cyrup")
            .join("agents")
            .join("settings.json")
    }

    /// `/subagents-load-profile <name>`: load the named profile and REPLACE ONLY the `subagents`
    /// key of the user settings file (pi `applySubagentProfile`, slash-commands.ts:1133-1176). Then
    /// surface the profile's `worker`-tier model (pi `getProfileWorkerModel`) as the model the user
    /// may want to switch the live session to.
    ///
    /// pi additionally *offers an interactive confirm* to switch the running session's model to the
    /// worker model, but ONLY when the host exposes `pi.setModel` + `ctx.modelRegistry`
    /// (slash-commands.ts:1150-1168); when it does not, pi falls straight through to the
    /// `else if (workerModel)` branch and simply reports the worker model (line 1167). A native
    /// `NativeExtension::execute_command` `HostCtx` exposes neither a `set_model` control op nor an
    /// interactive `confirm` today (that live session-model switch is the outer-layer UI tier,
    /// tracked separately) — so this reproduces pi's exact non-interactive branch: settings are
    /// written for real, and the worker model is reported.
    async fn load_profile_into_settings(&self, name: &str) -> Result<String, SubagentError> {
        let profiles_dir = self.profiles_dir();
        let profile = crate::registration::profiles::load_profile(&profiles_dir, name)?;
        let worker_model = crate::registration::profiles::profile_worker_model(&profile);
        let settings_path = self.user_settings_path();
        crate::registration::profiles::apply_profile_to_settings_file(&settings_path, &profile)?;

        let profile_path = crate::registration::profiles::profile_path(&profiles_dir, name)?;
        let mut lines = vec![
            format!("Loaded subagent profile: {name}"),
            format!("Profile: {}", profile_path.display()),
            format!("Updated: {}", settings_path.display()),
        ];
        if let Some(model) = worker_model {
            lines.push(format!("Profile worker model: {model}"));
        }
        Ok(lines.join("\n"))
    }
}

// =================================================================================================
// Free helper functions backing the dispatch_slash arms above
// =================================================================================================

/// Every agent name a [`RunnerStep`] graph will dispatch, in walk order — a single step's own
/// agent, each parallel-group child's agent, and a dynamic group's per-item template agent. This is
/// the plan-time persona resolver's input set: [`SubagentsExtension::run_or_background_chain`]
/// resolves the whole set via [`SubagentExecutor::resolve_plan_personas`] before any child
/// is spawned (T0.1/C13 plan-time resolution + upfront agent-name validation).
fn plan_step_agent_names(graph: &[RunnerStep]) -> Vec<String> {
    let mut names = Vec::new();
    for step in graph {
        match step {
            RunnerStep::SingleStep(spec) => names.push(spec.agent.clone()),
            RunnerStep::ParallelGroup(group) => {
                names.extend(group.steps.iter().map(|spec| spec.agent.clone()));
            }
            RunnerStep::DynamicGroup(dynamic) => names.push(dynamic.template.agent.clone()),
            // A root-attachment step names no agent to discover/resolve at plan time — its agent is
            // whatever the ALREADY-launched target run resolved for itself, read back from the
            // target's result at poll time (R-SA-097). Contributing its display name here would make
            // plan-time persona resolution demand an agent the attaching chain never spawns.
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    names
}

/// The graph's first step's first task text — the fallback `{task}` value when the call site
/// supplied no explicit top-level task (pi `originalTask = params.task ?? firstStepFirstTask`,
/// `chain-execution.ts:493-497`). A single step's own task, a parallel group's first child's task, or
/// a dynamic group's per-item template task; an `ImportAsyncRoot`-led graph (or an empty graph) has
/// no authored task text, yielding the empty string (so `{task}` → `""`, matching pi's empty case).
fn first_step_task(graph: &[RunnerStep]) -> String {
    graph
        .iter()
        .find_map(|step| match step {
            RunnerStep::SingleStep(spec) => Some(spec.task.clone()),
            RunnerStep::ParallelGroup(group) => group.steps.first().map(|spec| spec.task.clone()),
            RunnerStep::DynamicGroup(dynamic) => Some(dynamic.template.task.clone()),
            RunnerStep::ImportAsyncRoot(_) => None,
        })
        .unwrap_or_default()
}

/// pi `formatAsyncStartedMessage` (`async-execution.ts:200-208`): the mode-specific `headline`
/// followed verbatim by the fixed four-line detached-run guidance (blank line, then three
/// instruction lines), joined with `"\n"` exactly as pi's `.join("\n")` does.
fn format_async_started_message(headline: &str) -> String {
    [
        headline,
        "",
        "The async run is detached. Do not run sleep timers or polling loops just to wait for it.",
        "If you have independent work, continue that work. If you have nothing else to do until \
         the async result arrives, end your turn now; Pi will deliver the completion when the run \
         finishes.",
        "Use subagent({ action: \"status\", id: \"...\" }) when you need the current status/result, \
         or to inspect a blocked/stale run. Do not poll just to wait.",
    ]
    .join("\n")
}

/// One chain-step's display descriptor for the `chainDesc` join (pi `async-execution.ts:775-779`):
/// a sequential step is its bare agent name, a static parallel group is `[a+b]`, a dynamic group is
/// `expand:agent`, and a root-attachment step is its (fallback) display agent name.
fn describe_chain_step(step: &RunnerStep) -> String {
    match step {
        RunnerStep::SingleStep(spec) => spec.agent.clone(),
        RunnerStep::ParallelGroup(group) => format!(
            "[{}]",
            group
                .steps
                .iter()
                .map(|spec| spec.agent.as_str())
                .collect::<Vec<_>>()
                .join("+")
        ),
        RunnerStep::DynamicGroup(dynamic) => format!("expand:{}", dynamic.template.agent),
        RunnerStep::ImportAsyncRoot(spec) => spec.agent.clone(),
    }
}

/// The full `chainDesc` pi joins with `" -> "` (`async-execution.ts:775-779`) to build the async-start
/// headline for a CHAIN/PARALLEL run.
fn describe_chain(graph: &[RunnerStep]) -> String {
    graph.iter().map(describe_chain_step).collect::<Vec<_>>().join(" -> ")
}

/// Resolve every step's effective fork-context and, for each forking step, mint its OWN per-index
/// branch session file — the Tier-2 fork default-mode + per-index-branch wire-up (pi
/// `resolveAgentDefaultContextPolicy` + `preflightForkSessionsForStaticTasks`,
/// `subagent-executor.ts:1280-1521`). Two behaviors this replaces the old single-shared-branch
/// `apply_default_context` with:
///
/// 1. **Fork default-mode.** An OMITTED call-site `context` (`None`) no longer forces `Fresh` on
///    every step; each step independently falls back to ITS OWN agent's persona `default_context`
///    via [`resolve_effective_context`] (`personas[agent].default_context`). An explicit call-site
///    `context` still wins for every step; a step's own explicit `context` wins over both (R-SA-138).
/// 2. **Per-index branch.** Rather than resolving ONE fork branch (index 0) and splicing that same
///    session file into every step, each FORKING step resolves its own branch at its own flat step
///    index off the SINGLE shared `resolver` (whose per-index cache mints a distinct branch per
///    index) — so two sibling parallel tasks that both fork get two DISTINCT branch session files.
///    The flat index increments for EVERY step (matching pi's `preflightForkSessionsForStaticTasks`
///    flat-index walk), forking or not, so indices are stable and never collide.
///
/// A step that already carries an explicit `session_file` keeps it (never re-branched). Returns the
/// (mutated) graph plus the FIRST forking step's branch path, used only as the run-level resume
/// session metadata (`RunnerConfig::session_file`); it is never spliced onto any step here.
///
/// Fails hard (R-SA-137/DI-SA-2) if any forking step's branch cannot be resolved (unpersisted parent,
/// no leaf) — resolving every step up front, before any child is spawned, so a later step's fork
/// failure aborts the whole batch rather than leaving earlier children already running.
async fn apply_fork_contexts(
    resolver: &ForkContextResolver,
    call_site_context: Option<ContextMode>,
    personas: &BTreeMap<String, ResolvedAgentPersona>,
    mut graph: Vec<RunnerStep>,
) -> Result<(Vec<RunnerStep>, Option<PathBuf>), SubagentError> {
    let mut flat_index: u32 = 0;
    let mut first_session_file: Option<PathBuf> = None;
    for step in &mut graph {
        match step {
            RunnerStep::SingleStep(spec) => {
                resolve_step_fork_context(
                    resolver,
                    call_site_context,
                    personas,
                    spec,
                    &mut flat_index,
                    &mut first_session_file,
                )
                .await?;
            }
            RunnerStep::ParallelGroup(group) => {
                for spec in &mut group.steps {
                    resolve_step_fork_context(
                        resolver,
                        call_site_context,
                        personas,
                        spec,
                        &mut flat_index,
                        &mut first_session_file,
                    )
                    .await?;
                }
            }
            RunnerStep::DynamicGroup(dynamic) => {
                resolve_step_fork_context(
                    resolver,
                    call_site_context,
                    personas,
                    &mut dynamic.template,
                    &mut flat_index,
                    &mut first_session_file,
                )
                .await?;
            }
            // A root-attachment step carries no fork-vs-fresh context of its own: it imports another,
            // already-completed run's result rather than spawning a fresh child whose session context
            // this resolution would seed. Left untouched (and does not consume a flat index).
            RunnerStep::ImportAsyncRoot(_) => {}
        }
    }
    Ok((graph, first_session_file))
}

/// Resolve one step's effective context (per-step explicit > call-site > persona default > `Fresh`)
/// and, when it resolves to `Fork` and the step has no explicit session file yet, mint its own
/// per-`*flat_index*` branch off `resolver`. Always advances `*flat_index*` by one so sibling steps
/// never share an index (and therefore never a branch). See [`apply_fork_contexts`].
async fn resolve_step_fork_context(
    resolver: &ForkContextResolver,
    call_site_context: Option<ContextMode>,
    personas: &BTreeMap<String, ResolvedAgentPersona>,
    spec: &mut SingleStepSpec,
    flat_index: &mut u32,
    first_session_file: &mut Option<PathBuf>,
) -> Result<(), SubagentError> {
    let index = *flat_index;
    *flat_index += 1;

    // Precedence: a step's OWN explicit `context` wins; else the call-site `context`; else this
    // step's agent's persona `default_context`; else `Fresh` (`resolve_effective_context`).
    let persona_default = personas.get(&spec.agent).and_then(|p| p.default_context);
    let effective = resolve_effective_context(spec.context.or(call_site_context), persona_default);
    spec.context = Some(effective);

    if effective == ContextMode::Fork {
        if spec.session_file.is_none() {
            let fork_context = resolver.resolve(ContextMode::Fork, index).await?;
            spec.session_file = fork_context.session_file_path.clone();
            if first_session_file.is_none() {
                *first_session_file = fork_context.session_file_path;
            }
        } else if first_session_file.is_none() {
            first_session_file.clone_from(&spec.session_file);
        }
    }
    Ok(())
}

/// `/run-chain`'s task-seeding rule (see this command's own doc note in `dispatch_slash`): splice
/// `task` into the first element's first task only, leaving every later step's saved task text
/// verbatim.
fn seed_first_step_task(mut steps: Vec<RunnerStep>, task: &str) -> Vec<RunnerStep> {
    if task.is_empty() {
        return steps;
    }
    if let Some(first) = steps.first_mut() {
        match first {
            RunnerStep::SingleStep(spec) => spec.task = task.to_string(),
            RunnerStep::ParallelGroup(group) => {
                if let Some(first_task) = group.steps.first_mut() {
                    first_task.task = task.to_string();
                }
            }
            RunnerStep::DynamicGroup(_) => {
                // A `DynamicGroup` has no single fixed task to overwrite (its per-item tasks come
                // from `template` instantiated once per resolved array element) — left as saved.
            }
            RunnerStep::ImportAsyncRoot(_) => {
                // A root-attachment step's "task" is fixed by the target run it imports; there is no
                // free task text to seed (R-SA-097) — left as saved.
            }
        }
    }
    steps
}

/// Render [`StepResult`]s from a foreground `/chain`/`/parallel`/`/run-chain` run as human-readable
/// text — one line per step, in chain order (R-SA-051's ordering guarantee, restated at this
/// command's own text-rendering layer).
fn render_chain_results(results: &[StepResult], is_group: &[bool], groups: &[GroupStepResult]) -> String {
    let mut out = String::new();
    let mut group_cursor = 0usize;
    for (i, result) in results.iter().enumerate() {
        let step_is_group = is_group.get(i).copied().unwrap_or(false);
        if step_is_group {
            // A group step's own aggregate `StepResult::final_output` is always `None` by
            // construction (`chain_graph::collapse_fan_out`'s own doc: the aggregate carries only
            // a `structured_output` array, never a collapsed text field) — render each fanned-out
            // child's own text output instead, in the SAME position-indexed order `run_bounded`
            // guarantees (R-SA-051), so a `/parallel` caller can see every child's real output,
            // not just an aggregate "ok"/"failed" line with no text at all.
            let group = groups.get(group_cursor);
            group_cursor += 1;
            if result.success {
                out.push_str(&format!("step {}: ok (parallel group)\n", i + 1));
            } else {
                let err = result.error.clone().unwrap_or_else(|| "unknown error".to_string());
                out.push_str(&format!("step {}: FAILED (parallel group) — {err}\n", i + 1));
            }
            if let Some(group) = group {
                for (child_i, child) in group.children.iter().enumerate() {
                    match child {
                        Some(child_result) if child_result.success => {
                            let text = child_result
                                .final_output
                                .clone()
                                .unwrap_or_else(|| "(no text output)".to_string());
                            out.push_str(&format!("  child {}: ok\n  {text}\n", child_i + 1));
                        }
                        Some(child_result) => {
                            let err = child_result.error.clone().unwrap_or_else(|| "unknown error".to_string());
                            out.push_str(&format!("  child {}: FAILED — {err}\n", child_i + 1));
                        }
                        None => {
                            out.push_str(&format!("  child {}: skipped\n", child_i + 1));
                        }
                    }
                }
            }
        } else if result.success {
            let text = result
                .final_output
                .clone()
                .unwrap_or_else(|| "(no text output)".to_string());
            out.push_str(&format!("step {}: ok\n{text}\n", i + 1));
        } else {
            let err = result.error.clone().unwrap_or_else(|| "unknown error".to_string());
            out.push_str(&format!("step {}: FAILED — {err}\n", i + 1));
        }
        if i + 1 != results.len() {
            out.push('\n');
        }
    }
    if out.is_empty() {
        out.push_str("(chain produced no step results)");
    }
    out
}

/// pi `BUILTIN_AGENT_NAMES` (`agents.ts:25-33`): the fixed 8 shipped builtin personas, in pi's
/// declared order. `/subagents-models`' all-agents view (and the single-agent name gate) walks
/// EXACTLY this static list — not whatever discovery happened to find — so a name discovery didn't
/// resolve renders its own "missing"/"not found" row rather than silently shrinking the report.
const BUILTIN_AGENT_NAMES: [&str; 8] = [
    "context-builder",
    "delegate",
    "oracle",
    "planner",
    "researcher",
    "reviewer",
    "scout",
    "worker",
];

/// pi's `INHERIT_MODEL` sentinel (`runs/shared/model-fallback.ts:22`): a persona's `model` set to
/// the literal string `"inherit"` requests the parent session's model exactly as if `model` were
/// unset — it is NOT a real model id to resolve against the catalog or print verbatim.
const INHERIT_MODEL_SENTINEL: &str = "inherit";

/// pi `splitThinkingSuffix` (`runs/shared/model-fallback.ts:13-19`): split a model string on its
/// LAST `:`, isolating a trailing thinking-level suffix (`:high`, `:off`, ...) from the base model
/// id. No suffix present -> empty suffix, base = the whole string.
fn split_thinking_suffix(model: &str) -> (&str, &str) {
    match model.rfind(':') {
        Some(idx) => (model.get(..idx).unwrap_or(model), model.get(idx..).unwrap_or("")),
        None => (model, ""),
    }
}

/// One entry of pi's `ctx.modelRegistry.getAvailable()` (`shared/model-info.ts`'s `ModelInfo`),
/// reduced to the three fields `resolve_model_candidate` consults.
struct AvailableModelEntry {
    provider: String,
    id: String,
    full_id: String,
}

/// pi's `ctx.modelRegistry.getAvailable()` (`profiles.ts:505`, `agent-management.ts:169`) — the
/// model registry every model-facing subagents command consults — bound here to the REAL built-in
/// provider registry, [`cyrup_provider::catalog::builtin_catalog`], i.e. every model every
/// registered provider ships.
///
/// [CYRUP-DELTA] pi's `getAvailable()` additionally filters to providers whose auth is configured
/// (`ai/src/models.ts:394-408`); `cyrup-provider` has no `checkAuth`/`getAvailable` port yet
/// (PROV-003 — cyrup ships no login flow at all), so this is the credential-BLIND registry:
/// `getModels()`, pi's "complete synchronous catalog" (`models.ts:108`). That is a strictly wider
/// list than pi's, never a narrower one, so no model pi would offer is hidden here.
fn registry_models() -> &'static [cyrup_provider::Model] {
    cyrup_provider::catalog::builtin_catalog()
}

/// [`registry_models`] projected onto the three fields `resolve_model_candidate` consults.
fn registry_available_models() -> Vec<AvailableModelEntry> {
    registry_models()
        .iter()
        .map(|m| AvailableModelEntry {
            provider: m.provider.as_str().to_string(),
            id: m.id.as_str().to_string(),
            full_id: format!("{}/{}", m.provider.as_str(), m.id.as_str()),
        })
        .collect()
}

/// pi `resolveModelCandidate` (`runs/shared/model-fallback.ts:60-76`): resolve a bare or
/// fully-qualified model string against the available-model list. A `provider/id` string passes
/// through unchanged; a bare id resolves to its `fullId` when exactly one available model matches
/// (or, when multiple providers offer the same bare id, the `preferred_provider`'s match wins); an
/// unmatched bare id (no available models, or an ambiguous match with no preferred-provider hit)
/// passes through unchanged — pi's fallback `return model`.
fn resolve_model_candidate(
    model: Option<&str>,
    available: &[AvailableModelEntry],
    preferred_provider: Option<&str>,
) -> Option<String> {
    let model = model?;
    if model.is_empty() {
        return None;
    }
    if model.contains('/') {
        return Some(model.to_string());
    }
    if available.is_empty() {
        return Some(model.to_string());
    }
    let (base_model, thinking_suffix) = split_thinking_suffix(model);
    let matches: Vec<&AvailableModelEntry> =
        available.iter().filter(|entry| entry.id == base_model).collect();
    if let Some(preferred) = preferred_provider
        && let Some(m) = matches.iter().find(|entry| entry.provider == preferred)
    {
        return Some(format!("{}{thinking_suffix}", m.full_id));
    }
    if matches.len() != 1 {
        return Some(model.to_string());
    }
    let only = matches.into_iter().next()?;
    Some(format!("{}{thinking_suffix}", only.full_id))
}

/// pi `resolveSubagentModelOverride` (`runs/shared/model-fallback.ts:47-59`): the effective model a
/// discovered builtin persona resolves to. `requested_model` unset, empty, or the `"inherit"`
/// sentinel all resolve to the live parent session model (`provider/id`) when one is bound, else
/// `None` (pi's "(unresolved)" case); any other explicit value is resolved via
/// [`resolve_model_candidate`].
fn resolve_subagent_model_override(
    requested_model: Option<&str>,
    parent_model: Option<(&str, &str)>,
    available: &[AvailableModelEntry],
    preferred_provider: Option<&str>,
) -> Option<String> {
    let trimmed = requested_model.map(str::trim).unwrap_or("");
    let explicit = (!trimmed.is_empty() && trimmed != INHERIT_MODEL_SENTINEL).then_some(trimmed);
    match explicit {
        None => parent_model.map(|(provider, id)| format!("{provider}/{id}")),
        Some(explicit) => resolve_model_candidate(Some(explicit), available, preferred_provider),
    }
}

/// pi `resolveSubagentDefaultModel` (`agents.ts:716-728`): which scope's `subagents.defaultModel`
/// wins (project beats user when the project scope exists and declares one). `merge.rs`'s
/// `apply_default_model` already guarantees that whenever an agent's `model_source` is still
/// [`AgentModelSourceInfo::SettingsDefault`], `model` equals exactly the value this same
/// precedence resolves to (any override that changes `model` also resets `model_source` away from
/// `SettingsDefault`) — so [`format_model_source`] only needs the WINNING SCOPE from here, not the
/// value itself, to render pi's scope-qualified `"{scope} defaultModel"` provenance.
fn resolve_default_model_scope(settings: &LayeredOverrideSettings) -> Option<&'static str> {
    if settings.project_settings_path.is_some() && settings.project.default_model.is_some() {
        return Some("project");
    }
    if settings.user.default_model.is_some() {
        return Some("user");
    }
    None
}

/// Provenance of a builtin persona's resolved model (pi `formatModelSource`,
/// agent-management.ts:565-578). `default_model_scope` is [`resolve_default_model_scope`]'s
/// result for the current discovery run.
fn format_model_source(
    agent: &AgentDefinition,
    current_session_model: Option<&str>,
    default_model_scope: Option<&str>,
) -> String {
    // pi `agent.override && agent.model !== agent.override.base.model` (agent-management.ts:566-568):
    // the override branch fires only when the override actually changed the resolved model, not
    // merely because an override happens to be recorded (e.g. it only touched `disabled`/`tools`).
    if let Some(override_info) = &agent.override_info
        && agent.model != override_info.base_snapshot.model
    {
        let scope = match override_info.scope {
            OverrideScope::User => "user",
            OverrideScope::Project => "project",
        };
        return format!("{scope} override");
    }
    // pi `agent.modelSource?.type === "subagents.defaultModel" && agent.model === agent.modelSource.model`
    // (agent-management.ts:569-571): scope-qualified provenance, gated on the model still matching
    // what the default actually supplied (see this function's doc for why the value check is
    // redundant here and the scope alone suffices).
    if agent.model_source == Some(AgentModelSourceInfo::SettingsDefault)
        && let Some(scope) = default_model_scope
    {
        return format!("{scope} defaultModel");
    }
    if agent.model.is_some() {
        return "builtin agent config".to_string();
    }
    if current_session_model.is_some() {
        return "inherits current session model".to_string();
    }
    "inherit requested, but no current session model is available".to_string()
}

// =================================================================================================
// Live-probe + heuristic model classification (pi `probeModel`/`resolveProbeStatus`/`classifyModel`,
// profiles.ts:150-335) — the ported real-subprocess probe + pure classification pipeline
// `provider_ranked_full_ids`/`write_provider_catalog_file`/`render_profile_check_report` (below)
// all build on. pi's live `ctx.modelRegistry.getAvailable()` is bound here to [`registry_models`],
// the REAL built-in provider registry (`cyrup_provider::catalog::builtin_catalog()`) — every
// registry `Model` carries required (never-optional) `name`/`cost`/`context_window`/`max_tokens`/
// `reasoning` fields, so `classify_model` always takes pi's "official-metadata" branch (pi's
// heuristic-only fallback branch is unreachable here, a direct consequence of the embedded-catalog
// schema being fully populated rather than partial).
// =================================================================================================

/// pi `ProbeStatus` (profiles.ts:13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeStatus {
    Ok,
    Unavailable,
    Auth,
    Timeout,
    Error,
}

impl ProbeStatus {
    fn as_str(self) -> &'static str {
        match self {
            ProbeStatus::Ok => "ok",
            ProbeStatus::Unavailable => "unavailable",
            ProbeStatus::Auth => "auth",
            ProbeStatus::Timeout => "timeout",
            ProbeStatus::Error => "error",
        }
    }
}

/// The result of one [`probe_model`] call (pi's `{ status, message }` probe-result shape,
/// profiles.ts:322-335).
#[derive(Clone, Debug)]
struct ProbeOutcome {
    status: ProbeStatus,
    message: Option<String>,
}

/// Classify a non-zero probe exit's combined stderr/stdout text into a [`ProbeStatus`] (pi
/// `resolveProbeStatus`, profiles.ts:310-316): `timedOut` short-circuits to `Timeout` regardless
/// of text; empty text (no output at all) is `Error`; otherwise an auth/billing-shaped message
/// wins over an unavailable-shaped one (pi checks the auth regex first), and anything else falls
/// through to `Error`. Case-insensitive substring checks stand in for pi's `/i` regex alternations
/// (equivalent for these fixed keyword lists, and this crate has no `regex` dependency to spend on
/// them).
fn resolve_probe_status(text: &str, timed_out: bool) -> ProbeStatus {
    if timed_out {
        return ProbeStatus::Timeout;
    }
    if text.is_empty() {
        return ProbeStatus::Error;
    }
    let lower = text.to_lowercase();
    const AUTH_KEYWORDS: [&str; 7] = [
        "unauthorized",
        "unauthorised",
        "forbidden",
        "api key",
        "auth",
        "billing",
        "credit",
    ];
    if AUTH_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return ProbeStatus::Auth;
    }
    const UNAVAILABLE_KEYWORDS: [&str; 6] = [
        "not found",
        "unknown model",
        "model unavailable",
        "model disabled",
        "unsupported model",
        "unavailable",
    ];
    if UNAVAILABLE_KEYWORDS.iter().any(|kw| lower.contains(kw)) {
        return ProbeStatus::Unavailable;
    }
    ProbeStatus::Error
}

/// The 45-second probe timeout (pi `probeModel`'s `timeout: 45_000`, profiles.ts:328).
const PROBE_TIMEOUT_MS: u64 = 45_000;

/// The fixed probe prompt (pi `probeModel`, profiles.ts:326).
const PROBE_PROMPT: &str = "Reply with exactly \"OK\".";

/// Real live-probe subprocess call (pi `probeModel`, profiles.ts:318-335): spawns this crate's own
/// resolved `cyrup` binary ([`crate::spawn::resolve_spawn_command`], the exact analog of pi's
/// literal `"pi"` binary invocation — R-SA-045 mirrors pi-subagents' `PI_SUBAGENT_PI_BINARY`) with
/// `-p --model <fullId> --no-tools "Reply with exactly \"OK\"."`, cwd = the system temp directory
/// (pi `os.tmpdir()`), a 45s timeout, and classifies the result exactly as pi does: exit code 0 is
/// always `Ok` (message = stdout, or "Probe succeeded." if stdout is blank); any other outcome
/// (non-zero exit, spawn failure, or timeout) is classified via [`resolve_probe_status`] over the
/// combined stderr+stdout text (`killed`/timed-out short-circuits to `Timeout`, matching pi's
/// `result.killed === true` check).
async fn probe_model(full_id: &str) -> ProbeOutcome {
    probe_model_with(&crate::spawn::resolve_spawn_command(), full_id, PROBE_TIMEOUT_MS).await
}

/// The injectable core of [`probe_model`], parameterized over which [`crate::spawn::SpawnCommand`]
/// to spawn and how long to wait before treating the probe as timed out — mirrors this crate's own
/// `spawn_detached_runner`/`spawn_detached_runner_with_command` injectable-core convention, so a
/// test can substitute a fast, deterministic stand-in command (`true`/`false`/a scripted shell
/// invocation) and a short timeout instead of spawning a real provider-probing `cyrup -p` call.
async fn probe_model_with(
    spawn_command: &crate::spawn::SpawnCommand,
    full_id: &str,
    timeout_ms: u64,
) -> ProbeOutcome {
    let mut command = tokio::process::Command::new(&spawn_command.binary);
    command
        .args(&spawn_command.base_args)
        .arg("-p")
        .arg("--model")
        .arg(full_id)
        .arg("--no-tools")
        .arg(PROBE_PROMPT)
        .current_dir(std::env::temp_dir())
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ProbeOutcome {
                status: ProbeStatus::Error,
                message: Some(format!("failed to spawn probe: {e}")),
            };
        }
    };

    match tokio::time::timeout(
        std::time::Duration::from_millis(timeout_ms),
        child.wait_with_output(),
    )
    .await
    {
        Err(_elapsed) => ProbeOutcome {
            status: ProbeStatus::Timeout,
            message: Some(format!("Probe timed out after {timeout_ms}ms.")),
        },
        Ok(Err(e)) => ProbeOutcome {
            status: ProbeStatus::Error,
            message: Some(format!("probe wait failed: {e}")),
        },
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let combined = [stderr.as_str(), stdout.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n");
            let combined = combined.trim();
            if output.status.success() {
                let message = if stdout.is_empty() {
                    "Probe succeeded.".to_string()
                } else {
                    stdout
                };
                ProbeOutcome { status: ProbeStatus::Ok, message: Some(message) }
            } else {
                let status = resolve_probe_status(combined, false);
                let message = if combined.is_empty() {
                    format!(
                        "Probe exited with code {}.",
                        output
                            .status
                            .code()
                            .map_or_else(|| "unknown".to_string(), |c| c.to_string())
                    )
                } else {
                    combined.to_string()
                };
                ProbeOutcome { status, message: Some(message) }
            }
        }
    }
}

/// pi `extractVersionScore` (profiles.ts:150-154): the max of every `\d+(\.\d+)?` numeric token in
/// `id`, or `0.0` if none. Hand-rolled digit-run scan (no `regex` dependency in this crate) —
/// semantically identical to pi's global regex match + `Math.max`.
fn extract_version_score(id: &str) -> f64 {
    let bytes = id.as_bytes();
    let mut i = 0usize;
    let mut best: Option<f64> = None;
    let is_digit_at = |bytes: &[u8], idx: usize| bytes.get(idx).is_some_and(u8::is_ascii_digit);
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_digit() {
            let start = i;
            while is_digit_at(bytes, i) {
                i += 1;
            }
            if bytes.get(i) == Some(&b'.') && is_digit_at(bytes, i + 1) {
                i += 1;
                while is_digit_at(bytes, i) {
                    i += 1;
                }
            }
            if let Some(token) = bytes.get(start..i).and_then(|slice| std::str::from_utf8(slice).ok())
                && let Ok(value) = token.parse::<f64>()
                && value.is_finite()
            {
                best = Some(best.map_or(value, |b: f64| b.max(value)));
            }
        } else {
            i += 1;
        }
    }
    best.unwrap_or(0.0)
}

/// pi `modelNameTokens` (profiles.ts:156-163): lowercase, insert a space at every
/// letter-then-digit / digit-then-letter boundary, then split on runs of anything outside
/// `[a-z0-9.]`, dropping empty tokens. A single left-to-right scan reproduces pi's two sequential
/// global regex replaces (letter→digit, then digit→letter) exactly for every adjacent-character
/// transition, since both only ever look at one boundary at a time.
fn model_name_tokens(model_name: &str) -> Vec<String> {
    let lower = model_name.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut spaced = String::with_capacity(lower.len() + 4);
    for (idx, ch) in chars.iter().enumerate() {
        spaced.push(*ch);
        if let Some(next) = chars.get(idx + 1) {
            let cur_alpha = ch.is_ascii_lowercase();
            let cur_digit = ch.is_ascii_digit();
            let next_alpha = next.is_ascii_lowercase();
            let next_digit = next.is_ascii_digit();
            if (cur_alpha && next_digit) || (cur_digit && next_alpha) {
                spaced.push(' ');
            }
        }
    }
    spaced
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// pi `inferProfileBand` (profiles.ts:165-172): a coarse 0..=4 capability band inferred purely
/// from name tokens (spark/flash/nano/tiny/instant → 0; mini/haiku/small → 1; opus/max/ultra/pro →
/// 4; sonnet/turbo/plus → 3; anything else → 2).
fn infer_profile_band(model_name: &str) -> u8 {
    let tokens: std::collections::HashSet<String> =
        model_name_tokens(model_name).into_iter().collect();
    let has = |list: &[&str]| list.iter().any(|t| tokens.contains(*t));
    if has(&["spark", "flash", "nano", "tiny", "instant"]) {
        return 0;
    }
    if has(&["mini", "haiku", "small"]) {
        return 1;
    }
    if has(&["opus", "max", "ultra", "pro"]) {
        return 4;
    }
    if has(&["sonnet", "turbo", "plus"]) {
        return 3;
    }
    2
}

/// pi `combinedCost` (profiles.ts:199-204): the sum of every finite cost field. Since
/// `cyrup_provider::ModelCost`'s fields are required (never `Option`), this always yields
/// `Some(sum)` for a registry model (pi's `undefined` branch is reachable only when the
/// registry omits cost metadata entirely, which the embedded-catalog schema never does).
fn combined_cost(cost: &cyrup_provider::ModelCost) -> Option<f64> {
    let values = [cost.input, cost.output, cost.cache_read, cost.cache_write];
    let filtered: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
    if filtered.is_empty() { None } else { Some(filtered.iter().sum()) }
}

/// pi's `NumericStats` (profiles.ts:188-191): the min/max of a value set, used to min-max
/// normalize a raw metric into `0.0..=1.0`.
#[derive(Clone, Copy, Debug)]
struct NumericStats {
    min: f64,
    max: f64,
}

/// pi `collectStats` (profiles.ts:206-210): `None` when every input is missing/non-finite.
fn collect_stats(values: &[Option<f64>]) -> Option<NumericStats> {
    let filtered: Vec<f64> = values.iter().filter_map(|v| v.filter(|x| x.is_finite())).collect();
    if filtered.is_empty() {
        return None;
    }
    let min = filtered.iter().copied().fold(f64::INFINITY, f64::min);
    let max = filtered.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(NumericStats { min, max })
}

/// pi `normalize` (profiles.ts:212-216): min-max normalize `value` into `stats`' range; a
/// degenerate (all-equal) range normalizes to `0.5`.
fn normalize(value: Option<f64>, stats: Option<&NumericStats>) -> Option<f64> {
    let value = value?;
    let stats = stats?;
    if stats.max <= stats.min {
        return Some(0.5);
    }
    Some((value - stats.min) / (stats.max - stats.min))
}

/// pi `ClassificationContext` (profiles.ts:193-197), built once per provider-filtered candidate
/// set (pi `buildClassificationContext`, profiles.ts:218-224). pi's sibling `cost` stat feeds only
/// `costTier`/`latencyTier` (profiles.ts:285-295) — NEITHER of which contributes to `profileRank`
/// (profiles.ts:298's `qualitySignals` never includes `costNorm`) — so it is not modeled here; see
/// [`ModelClassification`]'s doc comment for why `profile_rank` is the only field this port keeps.
struct ClassificationContext {
    context_window: Option<NumericStats>,
    max_tokens: Option<NumericStats>,
}

fn build_classification_context(models: &[cyrup_provider::Model]) -> ClassificationContext {
    ClassificationContext {
        context_window: collect_stats(
            &models.iter().map(|m| Some(m.context_window as f64)).collect::<Vec<_>>(),
        ),
        max_tokens: collect_stats(
            &models.iter().map(|m| Some(m.max_tokens as f64)).collect::<Vec<_>>(),
        ),
    }
}

/// The result of pi `classifyModel` (profiles.ts:250-308), trimmed to the one field
/// `provider_ranked_full_ids`/`write_provider_catalog_file`/[`dominates`] actually consume as a
/// sort/selection key: `profile_rank` (pi `derived.profileRank`, profiles.ts:54/298). pi's sibling
/// `costTier`/`qualityTier`/`latencyTier`/`recommendedRoleTier`/`recommendedAgents`/
/// `classificationSources` fields feed only informational catalog-JSON display and the
/// `heuristicFallbackCount` reporting this port does not surface (a scope trim noted at this
/// crate's call sites) — every RANKING/FILTERING decision pi actually makes (tier selection,
/// `dominatesModel`) keys on `profileRank` alone, which this struct preserves byte-for-byte.
#[derive(Clone, Copy, Debug)]
struct ModelClassification {
    profile_rank: i64,
}

/// pi `classifyModel` (profiles.ts:250-308): the full heuristic + official-metadata blended
/// classification, reduced to its `profileRank` output (see [`ModelClassification`]'s doc comment).
/// See the module-level doc comment above for why this crate's registry input always has
/// "official metadata" (pi's `hasOfficialMetadata` is always `true` here).
fn classify_model(model: &cyrup_provider::Model, ctx: &ClassificationContext) -> ModelClassification {
    let model_name = if model.name.trim().is_empty() { model.id.as_str() } else { model.name.as_str() };
    let tokens: std::collections::HashSet<String> = model_name_tokens(model_name).into_iter().collect();
    let band = infer_profile_band(model_name);
    let version_score = extract_version_score(model.id.as_str());
    let context_norm = normalize(Some(model.context_window as f64), ctx.context_window.as_ref());
    let max_tokens_norm = normalize(Some(model.max_tokens as f64), ctx.max_tokens.as_ref());

    let heuristic_base = f64::from(band) / 4.0;
    let mut quality_signals: Vec<f64> = vec![heuristic_base];
    if let Some(v) = context_norm {
        quality_signals.push(v);
    }
    if let Some(v) = max_tokens_norm {
        quality_signals.push(v);
    }
    quality_signals.push(if model.reasoning { 1.0 } else { 0.0 });

    let latency_hints_fast = ["highspeed", "flash", "instant", "turbo"]
        .iter()
        .any(|t| tokens.contains(*t));

    #[allow(clippy::cast_precision_loss)]
    let mut quality_score = quality_signals.iter().sum::<f64>() / quality_signals.len() as f64;
    if latency_hints_fast {
        quality_score -= 0.2;
    }
    quality_score = quality_score.clamp(0.0, 1.0);

    let latency_penalty: i64 = if latency_hints_fast { 125 } else { 0 };
    let profile_rank =
        (quality_score * 100.0 * 10.0).round() as i64 + (version_score * 25.0).round() as i64 - latency_penalty;

    ModelClassification { profile_rank }
}

/// One usable, ranked candidate for [`filter_dominated`] (pi's `ProviderModelCatalogModel` fields
/// `dominatesModel`, profiles.ts:365-379, actually reads: `observed.cost`, `derived.profileRank`,
/// `observed.reasoning`, `observed.contextWindow`, `observed.maxTokens`).
#[derive(Clone, Debug)]
struct RankedCandidate {
    full_id: String,
    cost: f64,
    profile_rank: i64,
    reasoning: bool,
    context_window: u64,
    max_tokens: u64,
}

/// pi `dominatesModel` (profiles.ts:365-379): `a` dominates `b` when `a` is never worse on any
/// axis (cheaper-or-equal, ranked-at-least-as-high, reasoning-at-least-as-good, context/max-tokens
/// at-least-as-large) AND strictly better on at least one. Since this crate's `cost` is always
/// defined (never pi's `undefined` short-circuit — see [`combined_cost`]'s doc comment), that
/// branch of pi's function is unreachable here.
fn dominates(a: &RankedCandidate, b: &RankedCandidate) -> bool {
    if a.cost > b.cost {
        return false;
    }
    if a.profile_rank < b.profile_rank {
        return false;
    }
    if u8::from(a.reasoning) < u8::from(b.reasoning) {
        return false;
    }
    if a.context_window < b.context_window {
        return false;
    }
    if a.max_tokens < b.max_tokens {
        return false;
    }
    a.cost < b.cost
        || a.profile_rank > b.profile_rank
        || (a.reasoning && !b.reasoning)
        || a.context_window > b.context_window
        || a.max_tokens > b.max_tokens
}

/// pi `filterDominatedModels` (profiles.ts:381-383): drop every candidate that some OTHER
/// candidate in the set dominates. Identifies "the other candidate" by pointer identity
/// ([`std::ptr::eq`]) rather than a numeric index, so this never indexes `candidates` directly
/// (clippy's `indexing_slicing`, denied outside `#[cfg(test)]` by this crate's own lints).
fn filter_dominated(candidates: Vec<RankedCandidate>) -> Vec<RankedCandidate> {
    let keep: Vec<bool> = candidates
        .iter()
        .map(|candidate| {
            !candidates
                .iter()
                .any(|other| !std::ptr::eq(other, candidate) && dominates(other, candidate))
        })
        .collect();
    candidates.into_iter().zip(keep).filter(|(_, k)| *k).map(|(c, _)| c).collect()
}

/// pi `catalogModelIsUsable` (profiles.ts:402-404): usable iff the probe did NOT come back
/// unavailable/auth/timeout/error (`observed.availableInRegistry` is trivially always `true` here,
/// since every candidate is already drawn from the model registry).
fn probe_status_is_usable(status: &str) -> bool {
    !matches!(status, "unavailable" | "auth" | "timeout" | "error")
}

/// Render `/subagents-check-profile`'s report (pi `checkSubagentProfile`, profiles.ts:608-637):
/// for every `overrides.<agent>.model` the profile declares (pi does NOT check `defaultModel` —
/// `entries` at profiles.ts:615-617 only ever walks `profile.subagents.agentOverrides`), resolve it
/// against the model registry ([`registry_models`]) and REAL-probe the resolved full id
/// (or the raw string when unresolved) via [`probe_model`], with a per-probed-id cache so the same
/// model is never probed twice in one report (pi's `probeCache`, profiles.ts:618-628).
async fn render_profile_check_report(
    name: &str,
    profile: &crate::registration::profiles::NamedProfile,
) -> String {
    // pi's `entries` (profiles.ts:615-617) walks ONLY `agentOverrides`, never `defaultModel`.
    let mut refs: Vec<(String, String)> = Vec::new();
    for (agent_name, over) in &profile.subagents.overrides {
        if let crate::discovery::types::OverrideField::Value(model) = &over.model {
            let trimmed = model.trim();
            if !trimmed.is_empty() {
                refs.push((agent_name.clone(), trimmed.to_string()));
            }
        }
    }

    if refs.is_empty() {
        return format!("subagents-check-profile '{name}': no model references declared.");
    }

    // Recognize BOTH bare ids (`gpt-4o`) and fully-qualified `provider/id` refs (`openai/gpt-4o`)
    // — pi's `findModelInfo` resolves either form against `ctx.modelRegistry.getAvailable()`.
    let mut known: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in registry_models() {
        let full_id = format!("{}/{}", m.provider.as_str(), m.id.as_str());
        known.entry(m.id.as_str().to_string()).or_insert_with(|| full_id.clone());
        known.entry(full_id.clone()).or_insert(full_id);
    }

    let mut probe_cache: std::collections::HashMap<String, ProbeOutcome> = std::collections::HashMap::new();
    let mut out = format!("subagents-check-profile '{name}':\n");
    for (agent, model) in refs {
        let resolved_full_id = known.get(&model).cloned();
        let in_registry = resolved_full_id.is_some();
        let probe_id = resolved_full_id.unwrap_or_else(|| model.clone());
        let probe = match probe_cache.get(&probe_id) {
            Some(cached) => cached.clone(),
            None => {
                let result = probe_model(&probe_id).await;
                probe_cache.insert(probe_id.clone(), result.clone());
                result
            }
        };
        let message = probe
            .message
            .as_deref()
            .map(|m| m.lines().next().unwrap_or(""))
            .filter(|line| !line.is_empty())
            .map(|line| format!(" ({line})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {agent} → {model} — registry {}; probe {}{message}\n",
            if in_registry { "ok" } else { "missing" },
            probe.status.as_str(),
        ));
    }
    out
}

// =================================================================================================
// /subagents-companions support (pi `companion-suggestions.ts`)
// =================================================================================================

/// `hide <pkg> <workspace|user>` / `show <pkg>` — pi's `updateCompanionDismissal` third argument
/// (`"workspace" | "user" | "show"`, `companion-suggestions.ts:290`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CompanionDismissalScope {
    User,
    Workspace,
    Show,
}

/// pi `CompanionPackageStatus` (`companion-suggestions.ts:30-42`), trimmed to the fields
/// `buildCompanionDoctorLines`/`buildCompanionCommandStatus` render — this crate has no
/// `session_start`/`list` companion-suggestion surface to feed `surfaces`/`shouldRecommend` into,
/// so those fields are not modeled here.
struct CompanionPackageStatus {
    package_name: &'static str,
    active: bool,
    disabled: bool,
    dismissed: bool,
    install_command: &'static str,
    benefit: &'static str,
    status_source: &'static str,
    reason: String,
    details: Vec<String>,
}

/// pi `companionWorkspaceKey`/`nearestGitRoot` (`companion-suggestions.ts:98-110`): the nearest
/// ancestor directory containing a `.git` entry, or the resolved `cwd` when none is found.
fn companion_workspace_key(cwd: &Path) -> String {
    let resolved = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    let mut current = resolved.clone();
    loop {
        if current.join(".git").exists() {
            return current.display().to_string();
        }
        match current.parent() {
            Some(parent) => current = parent.to_path_buf(),
            None => return resolved.display().to_string(),
        }
    }
}

/// The resolved enabled/dismissed state for one companion package — pi `packageConfig`
/// (`companion-suggestions.ts:116-128`), trimmed to the two fields `isDismissed`/the doctor report
/// actually need (`surfaces` is not modeled, see [`CompanionPackageStatus`]'s own doc note).
struct ResolvedCompanionPackageConfig {
    enabled: bool,
    dismissed: Option<crate::registration::CompanionSuggestionDismissed>,
}

fn resolve_companion_package_config(
    config: &SubagentExtensionConfig,
    package_name: &str,
) -> ResolvedCompanionPackageConfig {
    if let Some(CompanionSuggestionsSetting::Toggle(false)) = &config.companion_suggestions {
        // pi: `if (companionConfig === false) return { enabled: false, ..., dismissed: false }` —
        // the whole-feature `false` shortcut disables every package and reports no dismissals.
        return ResolvedCompanionPackageConfig {
            enabled: false,
            dismissed: None,
        };
    }
    let companion_config = match &config.companion_suggestions {
        Some(CompanionSuggestionsSetting::Config(cfg)) => Some(cfg),
        _ => None,
    };
    let package_specific = companion_config
        .and_then(|cfg| cfg.packages.as_ref())
        .and_then(|packages| packages.get(package_name));
    let enabled = companion_config.and_then(|c| c.enabled) != Some(false)
        && package_specific.and_then(|p| p.enabled) != Some(false);
    ResolvedCompanionPackageConfig {
        enabled,
        dismissed: package_specific.and_then(|p| p.dismissed.clone()),
    }
}

/// pi `isDismissed` (`companion-suggestions.ts:130-133`).
fn companion_is_dismissed(
    config: &SubagentExtensionConfig,
    package_name: &str,
    workspace_key: &str,
) -> bool {
    let resolved = resolve_companion_package_config(config, package_name);
    let Some(dismissed) = resolved.dismissed else {
        return false;
    };
    dismissed.user == Some(true)
        || dismissed
            .workspaces
            .as_deref()
            .is_some_and(|list| list.iter().any(|w| w == workspace_key))
}

/// pi `readPiIntercomConfigStatus` (`companion-suggestions.ts:135-145`): whether `<agent
/// dir>/intercom/config.json`'s top-level `enabled` field is anything other than the literal
/// `false` (a missing file, a missing field, or a parse error all default to enabled — matching
/// pi's own catch-all fallback).
fn companion_intercom_config_status() -> (bool, Option<String>) {
    let config_path = dirs_home().join(".cyrup").join("intercom").join(CONFIG_FILE);
    let text = match std::fs::read_to_string(&config_path) {
        Ok(text) => text,
        Err(_) => return (true, None),
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Object(map)) => (
            map.get("enabled") != Some(&serde_json::Value::Bool(false)),
            None,
        ),
        Ok(_) => (true, None),
        Err(e) => (true, Some(format!("Error: {e}"))),
    }
}

/// Reduced port of pi `diagnoseIntercomBridge` (`intercom-bridge.ts:305-342`), covering the branches
/// this crate can actually observe (its call site — `resolveCompanionOrchestratorTarget` /
/// `index.ts:436-445` — passes no fork `context`, so the `mode === "fork-only"` branch never
/// applies here either): no orchestrator target, no on-disk `pi-intercom` extension directory, or a
/// disabled intercom config, in that exact order.
fn diagnose_companion_intercom_bridge(
    orchestrator_target: Option<&str>,
    intercom_config_enabled: bool,
) -> (bool, Option<String>) {
    if orchestrator_target.is_none() {
        return (false, Some("orchestrator target is not available".to_string()));
    }
    let extension_dir = dirs_home().join(".cyrup").join("extensions").join("pi-intercom");
    if !extension_dir.exists() {
        return (false, Some("pi-intercom extension was not found".to_string()));
    }
    if !intercom_config_enabled {
        return (false, Some("intercom config is disabled".to_string()));
    }
    (true, None)
}

/// pi `piIntercomStatus` (`companion-suggestions.ts:163-199`). `parentToolActive` (pi
/// `hasPackageTool(pi, PI_INTERCOM, "intercom")`) has no cyrup analogue — this crate has no
/// dynamically-loaded companion-package tool registry to probe, so it is unconditionally `false`,
/// which is also the exact value pi itself computes whenever pi-intercom is not actually installed
/// (this crate's permanent, genuine state). `active`'s `&&` chain is masked `false` by this term
/// regardless of the bridge sub-term, exactly as it would be for pi in that same state.
fn pi_intercom_status(
    config: &SubagentExtensionConfig,
    workspace_key: &str,
    orchestrator_target: Option<&str>,
) -> CompanionPackageStatus {
    let resolved = resolve_companion_package_config(config, "pi-intercom");
    let parent_tool_active = false;
    let (intercom_config_enabled, intercom_config_error) = companion_intercom_config_status();
    let (bridge_active, bridge_reason) =
        diagnose_companion_intercom_bridge(orchestrator_target, intercom_config_enabled);
    let active = parent_tool_active && intercom_config_enabled && bridge_active;
    let mut details = vec![
        format!(
            "parent runtime tool: {}",
            if parent_tool_active { "active" } else { "inactive" }
        ),
        format!(
            "bridge: {}{}",
            if bridge_active { "active" } else { "inactive" },
            bridge_reason
                .as_deref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ),
    ];
    if let Some(err) = intercom_config_error {
        details.push(format!("intercom config warning: {err}; runtime assumes enabled"));
    }
    let reason = if active {
        "active".to_string()
    } else if !intercom_config_enabled {
        "pi-intercom config is disabled".to_string()
    } else if parent_tool_active {
        "pi-intercom is active in the parent runtime, but child bridge discovery is not ready"
            .to_string()
    } else {
        "intercom tool from pi-intercom is not active in this session".to_string()
    };
    CompanionPackageStatus {
        package_name: "pi-intercom",
        active,
        disabled: !resolved.enabled,
        dismissed: companion_is_dismissed(config, "pi-intercom", workspace_key),
        install_command: "pi install npm:pi-intercom",
        benefit: "live supervisor decisions, progress updates, and grouped result delivery",
        status_source: "active runtime intercom tool plus intercom bridge diagnostics",
        reason,
        details,
    }
}

/// pi `promptTemplateModelStatus` (`companion-suggestions.ts:147-161`). `active` (pi
/// `hasPackageCommand(pi, PROMPT_TEMPLATE_MODEL, "prompt-tool")`) has no cyrup analogue for the same
/// reason as [`pi_intercom_status`]'s `parentToolActive`, and is unconditionally `false` here.
fn prompt_template_model_status(
    config: &SubagentExtensionConfig,
    workspace_key: &str,
) -> CompanionPackageStatus {
    let resolved = resolve_companion_package_config(config, "pi-prompt-template-model");
    let active = false;
    CompanionPackageStatus {
        package_name: "pi-prompt-template-model",
        active,
        disabled: !resolved.enabled,
        dismissed: companion_is_dismissed(config, "pi-prompt-template-model", workspace_key),
        install_command: "pi install npm:pi-prompt-template-model",
        benefit: "reusable prompt-template workflows with model/thinking/skill/subagent frontmatter",
        status_source: "active runtime command: prompt-tool",
        reason: if active {
            "active".to_string()
        } else {
            "prompt-tool command from pi-prompt-template-model is not active in this session"
                .to_string()
        },
        details: Vec::new(),
    }
}

/// pi `collectCompanionStatuses` (`companion-suggestions.ts:201-207`): pi-intercom FIRST, then
/// pi-prompt-template-model — the same order [`build_companion_doctor_lines`] renders in.
fn collect_companion_statuses(
    config: &SubagentExtensionConfig,
    cwd: &Path,
    orchestrator_target: Option<String>,
) -> Vec<CompanionPackageStatus> {
    let workspace_key = companion_workspace_key(cwd);
    vec![
        pi_intercom_status(config, &workspace_key, orchestrator_target.as_deref()),
        prompt_template_model_status(config, &workspace_key),
    ]
}

/// pi `buildCompanionDoctorLines` (`companion-suggestions.ts:229-242`).
fn build_companion_doctor_lines(statuses: &[CompanionPackageStatus]) -> Vec<String> {
    let mut lines = vec!["Companion packages".to_string()];
    for status in statuses {
        let hidden = if status.dismissed {
            " recommendation hidden by config"
        } else {
            ""
        };
        let disabled = if status.disabled { " disabled by config" } else { "" };
        lines.push(format!(
            "- {}: {}{hidden}{disabled}",
            status.package_name,
            if status.active { "active" } else { "inactive" }
        ));
        lines.push(format!("  install: {}", status.install_command));
        lines.push(format!("  benefit: {}", status.benefit));
        lines.push(format!("  status source: {}", status.status_source));
        lines.push(format!("  reason: {}", status.reason));
        for detail in &status.details {
            lines.push(format!("  {detail}"));
        }
    }
    lines
}

/// pi `buildCompanionCommandStatus` (`companion-suggestions.ts:324-326`).
fn build_companion_command_status(statuses: &[CompanionPackageStatus]) -> String {
    build_companion_doctor_lines(statuses).join("\n")
}

/// pi `readConfigForUpdate` (`extension/config.ts:10-17`): a missing file reads as
/// [`SubagentExtensionConfig::default`] (pi: `{}`); a present-but-malformed file is a hard error
/// (pi throws); a present, well-formed file parses normally.
async fn read_extension_config_for_update(
    path: &Path,
) -> Result<SubagentExtensionConfig, SubagentError> {
    match tokio::fs::read(path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            SubagentError::MalformedSettings(format!(
                "subagents config at '{}': {e}",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SubagentExtensionConfig::default()),
        Err(e) => Err(SubagentError::Spawn(e)),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    // NOTE: the `CYRUP_HOME`-sandboxed tests that used to live here (`child_env_gate_controls_
    // what_is_registered`, `top_level_with_optin_attaches_full`,
    // `init_registers_the_tool_and_all_thirteen_commands`,
    // `teardown_session_stops_the_tracker_and_clears_the_parent_session_anchor`) have moved to
    // `tests/cyrup_home_env_sandboxed_tests.rs`: they need `std::env::set_var`/`remove_var`, which
    // Rust requires `unsafe` for, and this crate's `src/lib.rs` is `#![forbid(unsafe_code)]` — see
    // that file's module doc for the full rationale (matches every other
    // `tests/*_integration.rs` file's identical env-mutation convention in this crate).

    #[test]
    fn id_is_stable() {
        let ext = SubagentsExtension::new();
        assert_eq!(ext.id(), ExtensionId::from("subagents"));
    }

    /// Regression (C16, dossier "Dynamic fanout unusable via the subagent tool"): a `chain[]`
    /// element carrying pi's `expand`/`parallel`/`collect` dynamic-fanout shape must now parse into
    /// a real [`RunnerStep::DynamicGroup`] — pre-fix, `parse_tool_chain_items` rejected ANY
    /// `expand`/`collect` key outright with `"not wired via the tool in this build yet (Tier 4,
    /// C16)"`, so a tool caller could never express dynamic fanout at all, only saved chain files
    /// could (`crate::discovery::chains::chain_step_to_runner_step`, `/run-chain`).
    #[test]
    fn parse_tool_chain_items_parses_a_dynamic_expand_collect_item_into_a_dynamic_group() {
        let raw = vec![serde_json::json!({
            "expand": {
                "from": { "output": "targets", "path": "/items" },
                "item": "target",
                "key": "/path",
                "maxItems": 4
            },
            "parallel": { "agent": "reviewer", "task": "Review {target.path}" },
            "collect": { "as": "reviews" }
        })];

        let graph = parse_tool_chain_items(&raw, 4).expect(
            "a well-formed expand/parallel/collect chain[] item must now parse into a \
             RunnerStep::DynamicGroup rather than erroring — the pre-fix 'not wired via the tool' \
             rejection",
        );
        assert_eq!(graph.len(), 1);
        match &graph[0] {
            RunnerStep::DynamicGroup(spec) => {
                assert_eq!(spec.expand, "outputs.targets/items");
                assert_eq!(spec.collect, "reviews");
                assert_eq!(spec.item.as_deref(), Some("target"));
                assert_eq!(spec.key.as_deref(), Some("/path"));
                assert_eq!(spec.max_items, Some(4));
                assert_eq!(spec.template.agent, "reviewer");
                assert_eq!(spec.template.task, "Review {target.path}");
            }
            other => panic!("expected RunnerStep::DynamicGroup, got: {other:?}"),
        }
    }

    /// Companion to the test above: a MALFORMED dynamic-fanout shape (missing `expand.from`) must
    /// still be rejected with pi's exact `validateDynamicStepShape` diagnostic, not silently
    /// mis-parsed into a bogus sequential/step-less graph — proving the new tool-parsing path
    /// reuses the SAME shape validation `discovery::chains::validate_dynamic_step_shape` already
    /// applies to saved chain files, rather than a looser, unvalidated conversion.
    #[test]
    fn parse_tool_chain_items_rejects_a_malformed_dynamic_item_with_pis_shape_error() {
        let raw = vec![serde_json::json!({
            "expand": { "item": "target" },
            "parallel": { "agent": "reviewer", "task": "Review {target}" },
            "collect": { "as": "reviews" }
        })];

        let err = parse_tool_chain_items(&raw, 4)
            .expect_err("a dynamic item missing expand.from must still be rejected");
        let message = err.to_string();
        assert!(
            message.contains("requires expand.from"),
            "must surface pi's exact shape-validation diagnostic: {message}"
        );
    }

    /// Regression (pi `chain-execution.ts:499-510`, dossier "No upfront
    /// validateChainOutputBindings for tool/slash chains; duplicate `as` silently overwrites"): a
    /// tool `chain[]` call with two steps sharing the SAME `as` name must be rejected up front,
    /// before any step (including its own agent-name resolution) is even attempted. Both step
    /// agents here are unresolvable (`ghost-one`/`ghost-two`) precisely so that a pre-fix run would
    /// instead reach `resolve_plan_personas` and fail with `SubagentError::AgentNotFound` — a
    /// DIFFERENT error than this test asserts on — proving the new upfront validation now wins the
    /// race.
    #[tokio::test]
    async fn chain_tool_call_rejects_duplicate_as_names_before_any_agent_resolution() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        let err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({
                    "chain": [
                        { "agent": "ghost-one", "task": "do a", "as": "shared" },
                        { "agent": "ghost-two", "task": "do b", "as": "shared" }
                    ]
                }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err(
                "a duplicate `as` name across two chain[] steps must be rejected up front",
            );
        let message = err.to_string();
        assert!(
            message.contains("Duplicate chain output name 'shared'"),
            "must reject with pi's exact duplicate-output diagnostic, not 'agent not found: \
             ghost-one' (which a pre-fix run would surface instead): {message}"
        );
    }

    /// Companion regression: an `{outputs.x}` reference to an output NO strictly-earlier step
    /// produces must also be rejected up front (pi's "Unknown chain output reference" diagnostic),
    /// again proven via unresolvable agent names so a pre-fix run's DIFFERENT failure
    /// (`AgentNotFound`, reached only once the referencing step's turn came up) would not
    /// accidentally satisfy this assertion.
    #[tokio::test]
    async fn chain_tool_call_rejects_an_unknown_outputs_reference_before_any_agent_resolution() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        let err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({
                    "chain": [
                        { "agent": "ghost-one", "task": "Use {outputs.never_produced}" }
                    ]
                }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an unknown {outputs.x} reference must be rejected up front");
        let message = err.to_string();
        assert!(
            message.contains("Unknown chain output reference '{outputs.never_produced}'"),
            "must reject with pi's exact unknown-reference diagnostic: {message}"
        );
    }

    /// T6 child-mode gate (pi `extension/index.ts:243-245` + `extension/fanout-child.ts:131`): the
    /// pure decision function encoding "a plain subagent child registers nothing; a fanout-authorized
    /// child gets the restricted tool; a non-child gets the full surface."
    #[test]
    fn resolve_registration_mode_encodes_the_child_gate() {
        // Not a child → full orchestrator surface (the fanout flag is irrelevant when not a child).
        assert_eq!(resolve_registration_mode(false, false), Some(RegistrationMode::Full));
        assert_eq!(resolve_registration_mode(false, true), Some(RegistrationMode::Full));
        // Fanout-authorized child → the restricted child-safe tool.
        assert_eq!(resolve_registration_mode(true, true), Some(RegistrationMode::ChildSafe));
        // Plain subagent child → register NOTHING.
        assert_eq!(resolve_registration_mode(true, false), None);
    }

    /// Opt-in gate, default OFF (mirrors `cyrup_permission_system` / `cyrup_intercom`): a plain
    /// TOP-LEVEL (non-child) session with NO `CYRUP_SUBAGENTS` env and NO `subagents/config.json`
    /// attaches NOTHING. Proven via the pure form with `installed = false` — deterministic, touching
    /// neither the env nor the filesystem — which is exactly the value `is_installed` yields in the
    /// no-opt-in state. (Requirement (a).)
    #[test]
    fn top_level_without_optin_attaches_nothing() {
        let cwd = std::env::temp_dir();
        let none = subagent_extension_for(
            SubagentExtensionConfig::default(),
            cwd,
            /* child */ false,
            /* fanout_authorized */ false,
            /* installed */ false,
        );
        assert!(none.is_none(), "a top-level session that has not opted in attaches nothing");
    }

    /// A fanout-authorized CHILD ([`RegistrationMode::ChildSafe`]) attaches its restricted surface
    /// REGARDLESS of the opt-in signal: with `installed = false` (no env, no config) it STILL yields an
    /// extension — mirroring intercom's child-orchestrator-metadata bypass of its own `is_installed`
    /// (its already-installed parent spawned it). (Requirement (d).)
    #[test]
    fn fanout_child_attaches_regardless_of_optin() {
        let cwd = std::env::temp_dir();
        let ext = subagent_extension_for(
            SubagentExtensionConfig::default(),
            cwd,
            /* child */ true,
            /* fanout_authorized */ true,
            /* installed */ false,
        );
        assert!(
            ext.is_some(),
            "a fanout-authorized child attaches its restricted surface regardless of is_installed"
        );
    }

    /// `is_installed`'s two config-file signals (the env branch is exercised in the `tests/`
    /// integration file, since this crate is `#![forbid(unsafe_code)]` and cannot mutate the process
    /// env in a `src/` test): a tier-3 `<agent_dir>/subagents/config.json` at user scope, OR
    /// `<cwd>/.cyrup/subagents/config.json` at project scope, each mark the extension installed; with
    /// neither present (and no env), it is NOT installed.
    #[test]
    fn is_installed_reads_the_config_file_signals() {
        let agent = tempfile::tempdir().expect("agent dir");
        let cwd = tempfile::tempdir().expect("cwd");
        // `is_installed` ORs the `CYRUP_SUBAGENTS` env signal with the config-file signals, so
        // account for whatever this process's ambient env already is (e.g. a developer/CI shell with
        // `CYRUP_SUBAGENTS=1` set workspace-wide) rather than assuming it is unset — this crate is
        // `#![forbid(unsafe_code)]`, so a `src/` test cannot sandbox the process env via
        // `set_var`/`remove_var` to force the "no env" case (the env branch itself is exercised,
        // fully sandboxed, by `tests/subagents_optin_gate_integration.rs`).
        let env_opted_in = env_truthy(INSTALL_ENV_VAR);

        // Neither file present → installed iff the ambient env already opted in.
        assert_eq!(is_installed(agent.path(), cwd.path()), env_opted_in);

        // User-scope tier-3 config present → installed regardless of env.
        let user_cfg = agent.path().join("subagents");
        std::fs::create_dir_all(&user_cfg).expect("mkdir user subagents");
        std::fs::write(user_cfg.join("config.json"), "{}").expect("write user config");
        assert!(is_installed(agent.path(), cwd.path()));

        // Project-scope config present (with a FRESH agent dir that has no user config) → installed
        // iff the ambient env already opted in, until the project config is written.
        let agent2 = tempfile::tempdir().expect("agent dir 2");
        assert_eq!(
            is_installed(agent2.path(), cwd.path()),
            env_opted_in,
            "sanity: agent2 has no user config yet"
        );
        let proj_cfg = cwd.path().join(".cyrup").join("subagents");
        std::fs::create_dir_all(&proj_cfg).expect("mkdir project subagents");
        std::fs::write(proj_cfg.join("config.json"), "{}").expect("write project config");
        assert!(is_installed(agent2.path(), cwd.path()));
    }

    /// The composed install gate ([`gate_on_install`]) in isolation: a top-level `Full` survives ONLY
    /// when installed; a `ChildSafe` fanout child survives REGARDLESS.
    #[test]
    fn gate_on_install_only_gates_full() {
        assert_eq!(gate_on_install(RegistrationMode::Full, true), Some(RegistrationMode::Full));
        assert_eq!(gate_on_install(RegistrationMode::Full, false), None);
        assert_eq!(
            gate_on_install(RegistrationMode::ChildSafe, true),
            Some(RegistrationMode::ChildSafe)
        );
        assert_eq!(
            gate_on_install(RegistrationMode::ChildSafe, false),
            Some(RegistrationMode::ChildSafe)
        );
    }

    /// T6 upfront agent-name validation for `/chain` (pi validates every named agent before starting
    /// a chain rather than spawning a partial run that dies mid-walk): a chain naming an agent that
    /// resolves to nothing in the discovery scope fails fast with [`SubagentError::AgentNotFound`]
    /// BEFORE any child process — and therefore before any spawn scratch directory — is ever created.
    #[tokio::test]
    async fn unknown_agent_in_a_chain_is_rejected_upfront_before_any_spawn() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let graph = vec![RunnerStep::SingleStep(SingleStepSpec {
            agent: "does-not-exist".to_string(),
            task: "do the thing".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];
        // `GraphRunOutcome` (the Ok type) is not `Debug`, so match manually rather than `expect_err`.
        match executor
            .run_or_background_graph(
                dir.path(),
                graph,
                RunMode::Chain,
                None,
                false,
                None,
                CancelToken::new(),
                None,
            )
            .await
        {
            Err(SubagentError::AgentNotFound(name)) => assert_eq!(name, "does-not-exist"),
            Err(other) => panic!("expected AgentNotFound(does-not-exist), got: {other}"),
            Ok(_) => panic!("an unknown agent in /chain must be rejected before running"),
        }
        assert!(
            !dir.path().join(".cyrup-subagent-scratch").exists(),
            "upfront rejection must happen before any child (and its scratch dir) is created"
        );
    }

    // ---- SUBA-041: the SINGLE-mode override normalizers ----

    /// pi `normalizeSingleOutputOverride` (`single-output.ts:11-19`) + `runSinglePath`'s persona
    /// fallback (`subagent-executor.ts:2789`), rule by rule.
    #[test]
    fn normalize_single_output_override_ports_pis_five_cases() {
        // Omitted param → the persona's own declared output.
        assert_eq!(
            normalize_single_output_override(None, Some("persona.md")),
            Some("persona.md".to_string())
        );
        assert_eq!(normalize_single_output_override(None, None), None);
        // Explicit disable, both spellings.
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!(false)), Some("persona.md")),
            None
        );
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!("false")), Some("persona.md")),
            None
        );
        // `true`/`"true"` means "the persona's own output".
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!(true)), Some("persona.md")),
            Some("persona.md".to_string())
        );
        assert_eq!(
            normalize_single_output_override(Some(&serde_json::json!("true")), None),
            None
        );
        // A real path wins over the persona default.
        assert_eq!(
            normalize_single_output_override(
                Some(&serde_json::json!("report.md")),
                Some("persona.md")
            ),
            Some("report.md".to_string())
        );
        // An empty string is "no output".
        assert_eq!(normalize_single_output_override(Some(&serde_json::json!("")), None), None);
    }

    /// pi `resolveSingleOutputPath` (`single-output.ts:21-34`) as `runSinglePath` calls it: a
    /// RELATIVE output resolves against the run's own scoped base dir, never the run cwd; an
    /// absolute one is used verbatim; the disable sentinels never produce a path.
    #[test]
    fn resolve_single_output_path_resolves_relative_against_the_run_output_base_dir() {
        let base = Path::new("/scoped/outputs/run123");
        assert_eq!(
            resolve_single_output_path(Some("report.md"), base),
            Some(PathBuf::from("/scoped/outputs/run123/report.md"))
        );
        assert_eq!(
            resolve_single_output_path(Some("/abs/report.md"), base),
            Some(PathBuf::from("/abs/report.md"))
        );
        assert_eq!(resolve_single_output_path(None, base), None);
        assert_eq!(resolve_single_output_path(Some("false"), base), None);
        assert_eq!(resolve_single_output_path(Some(""), base), None);
    }

    /// pi `normalizeSkillInput` (`agents/skills.ts:684-708`) — including the JSON-encoded-array
    /// guard models routinely trip, and the `false` → "no skills at all" form.
    #[test]
    fn normalize_skill_input_ports_pis_union() {
        assert_eq!(normalize_skill_input(None), None);
        assert_eq!(normalize_skill_input(Some(&serde_json::json!(true))), None);
        assert_eq!(normalize_skill_input(Some(&serde_json::json!(false))), Some(Vec::new()));
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!(["a", " b ", "", "a"]))),
            Some(vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!("rust, testing ,rust"))),
            Some(vec!["rust".to_string(), "testing".to_string()])
        );
        // A JSON-encoded array arriving as a string must NOT be comma-split into `["a"` / `"b"]`.
        assert_eq!(
            normalize_skill_input(Some(&serde_json::json!(r#"["a","b"]"#))),
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    /// SUBA-041: the `acceptance` param lowers onto a real [`crate::exec::acceptance::AcceptanceContract`]
    /// (never the heuristic fallback) for every explicit level, defers for `"auto"`, and refuses a
    /// malformed policy with pi's own `validateAcceptanceInput` text.
    #[test]
    fn parse_single_acceptance_lowers_levels_and_validates() {
        use crate::exec::acceptance::AcceptanceStatus;

        // "auto" is pi's "omitted means auto-inferred" — defer to the heuristic default.
        assert_eq!(parse_single_acceptance(&serde_json::json!("auto")), Ok(None));

        let checked = parse_single_acceptance(&serde_json::json!("checked"))
            .expect("valid level")
            .expect("an explicit level yields a contract");
        assert_eq!(checked.required_level, AcceptanceStatus::Checked);
        assert!(checked.explicit, "an explicit param arms R-SA-033's exit-code correction");

        // `false` is pi's `level: "none"` shorthand: explicit, but nothing to gate.
        let disabled = parse_single_acceptance(&serde_json::json!(false))
            .expect("valid")
            .expect("a contract");
        assert_eq!(disabled.required_level, AcceptanceStatus::NotRequired);
        assert!(disabled.explicit);
        assert!(disabled.is_no_op());

        // The object form carries `verify[].command` onto the contract.
        let verified = parse_single_acceptance(&serde_json::json!({
            "level": "verified",
            "verify": [{ "id": "t", "command": "cargo test" }]
        }))
        .expect("valid")
        .expect("a contract");
        assert_eq!(verified.required_level, AcceptanceStatus::Verified);
        assert_eq!(verified.verify, vec!["cargo test".to_string()]);

        // pi's verbatim validation failures (`acceptance.ts:143,152`).
        assert_eq!(
            parse_single_acceptance(&serde_json::json!("nope")),
            Err("acceptance has invalid level 'nope'.".to_string())
        );
        assert!(parse_single_acceptance(&serde_json::json!({ "bogus": 1 }))
            .expect_err("an unsupported key is rejected")
            .contains("acceptance.bogus is not supported."));
    }

    /// C8: the LLM-facing `subagent` tool schema exposes pi's FULL parameter union
    /// (`schemas.ts:195-265`), not just the pre-C8 5-property single-task shape. Asserts every
    /// top-level pi property name is present, the 11-value management/control `action` enum is
    /// complete and correctly ordered, the `context` fresh/fork enum is present, the `tasks[]`
    /// per-task `output`/`outputMode`/`reads`/`progress` fields exist, and the numeric bounds pi
    /// pins (`concurrency`/`timeoutMs`/`maxRuntimeMs` minimum, `index` minimum 0) are carried — the
    /// Rust analog of pi's own `test/unit/schemas.test.ts`.
    ///
    /// SUBA-041 re-scoped the property list: `includeProgress` and `control` were dropped from the
    /// expected set and are now asserted ABSENT, because this port has no subsystem behind either
    /// and [`SubagentTool::route_single`] refuses them. See
    /// [`single_mode_wires_the_seven_supported_overrides_and_refuses_only_the_two_unadvertised`]
    /// for the other half of that invariant.
    #[test]
    fn subagent_tool_schema_exposes_the_full_pi_parameter_union() {
        let schema = subagent_tool_parameters();
        let props = schema
            .get("properties")
            .and_then(|p| p.as_object())
            .expect("schema has a properties object");

        // Every top-level pi `SubagentParamsSchema` property (schemas.ts:195-263), in source order,
        // minus SUBA-041's two withholds.
        let expected_properties = [
            "agent", "task", "action", "id", "runId", "dir", "index", "message", "chainName",
            "config", "tasks", "concurrency", "worktree", "chain", "context", "chainDir", "async",
            "timeoutMs", "maxRuntimeMs", "agentScope", "cwd", "artifacts", "share", "sessionDir",
            "clarify", "output", "outputMode", "skill", "model", "acceptance",
        ];
        for name in expected_properties {
            assert!(
                props.contains_key(name),
                "schema must advertise the pi parameter '{name}'; got keys: {:?}",
                props.keys().collect::<Vec<_>>()
            );
        }

        // SUBA-041's core invariant: a param the dispatcher refuses must not be advertised. These
        // two have no subsystem in this port (`control` → no `resolveControlConfig`/notice pipeline;
        // `includeProgress` → no `details.progress` array on the compacted `SingleResult`).
        for withheld in ["includeProgress", "control"] {
            assert!(
                !props.contains_key(withheld),
                "'{withheld}' is rejected at dispatch, so the schema must NOT advertise it"
            );
        }

        // The management/control action enum (schemas.ts:199-202 + SUBAGENT_ACTIONS,
        // shared/types.ts:1121), exact values AND order. 15 of pi's 20: SUBA-005 added
        // eject/disable/enable/reset; the five still missing (`steer` + the four `schedule*`) are
        // explicitly deferred to SUBA-013/SUBA-016 and MUST NOT be advertised here until their
        // subsystems exist — advertising a verb the dispatcher rejects is worse than omitting it.
        let action_enum = props
            .get("action")
            .and_then(|a| a.get("enum"))
            .and_then(|e| e.as_array())
            .expect("action property carries an enum array");
        let action_values: Vec<&str> = action_enum.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(
            action_values,
            vec![
                "list", "get", "models", "create", "update", "delete", "eject", "disable",
                "enable", "reset", "status", "interrupt", "resume", "append-step", "doctor"
            ],
            "the action enum must be pi's SUBAGENT_ACTIONS union minus the deferred steer/schedule* five"
        );
        // Every advertised management verb must actually dispatch: an enum value the tool schema
        // shows the model but `route_action` answers with "unknown subagent action" is a worse
        // defect than the missing action was.
        for action in crate::discovery::management::MANAGEMENT_ACTIONS {
            assert!(
                action_values.contains(&action),
                "management action '{action}' is dispatched but not advertised in the tool schema"
            );
        }
        assert_eq!(props["action"]["type"], serde_json::json!("string"));

        // context fresh/fork enum.
        assert_eq!(props["context"]["type"], serde_json::json!("string"));
        assert_eq!(props["context"]["enum"], serde_json::json!(["fresh", "fork"]));

        // Top-level numeric bounds pi pins.
        assert_eq!(props["concurrency"]["minimum"], serde_json::json!(1));
        assert_eq!(props["timeoutMs"]["minimum"], serde_json::json!(1));
        assert_eq!(props["maxRuntimeMs"]["minimum"], serde_json::json!(1));
        assert_eq!(props["index"]["minimum"], serde_json::json!(0));

        // tasks[] per-task fields the description advertises (output/outputMode/reads/progress),
        // plus count's minimum.
        let task_props = props["tasks"]["items"]["properties"]
            .as_object()
            .expect("tasks[].items has a properties object");
        for per_task in ["agent", "task", "count", "output", "outputMode", "reads", "progress"] {
            assert!(
                task_props.contains_key(per_task),
                "tasks[] items must carry the per-task field '{per_task}'"
            );
        }
        assert_eq!(task_props["count"]["minimum"], serde_json::json!(1));
        assert_eq!(task_props["progress"]["type"], serde_json::json!("boolean"));
        assert_eq!(props["tasks"]["items"]["required"], serde_json::json!(["agent", "task"]));

        // chain[] items must be an additionalProperties:false object with the flattened
        // sequential/parallel/dynamic surface (schemas.ts:144-178).
        let chain_item = &props["chain"]["items"];
        assert_eq!(chain_item["type"], serde_json::json!("object"));
        assert_eq!(chain_item["additionalProperties"], serde_json::json!(false));
        let chain_props = chain_item["properties"]
            .as_object()
            .expect("chain[].items has a properties object");
        for chain_field in ["agent", "parallel", "expand", "collect", "concurrency", "failFast", "worktree"] {
            assert!(
                chain_props.contains_key(chain_field),
                "chain[] items must carry '{chain_field}'"
            );
        }

        // config/output/skill/acceptance are provider-friendly anyOf unions (no bare top-level type).
        assert!(props["config"].get("anyOf").is_some(), "config must be an anyOf union");
        assert!(props["output"].get("anyOf").is_some(), "output must be an anyOf union");
        assert!(props["skill"].get("anyOf").is_some(), "skill must be an anyOf union");
        assert!(props["acceptance"].get("anyOf").is_some(), "acceptance must be an anyOf union");

        // SUBA-041: the control fragment is no longer inserted into the advertised schema (no
        // `resolveControlConfig`/notice pipeline in this port), but it is KEPT as the shape record
        // for whichever tier lands that subsystem — so its nested attention thresholds + notify
        // enums are still pinned here, against the fragment rather than against `props`.
        let control_fragment = sj_control_overrides();
        let control_props = control_fragment["properties"]
            .as_object()
            .expect("control has a properties object");
        assert_eq!(control_props["needsAttentionAfterMs"]["minimum"], serde_json::json!(1));
        assert_eq!(
            control_props["notifyOn"]["items"]["enum"],
            serde_json::json!(["active_long_running", "needs_attention"])
        );
        assert_eq!(
            control_props["notifyChannels"]["items"]["enum"],
            serde_json::json!(["event", "async", "intercom"])
        );

        // The multi-section description (index.ts:461-495) — the substrings pi's own
        // tool-description executable spec pins (test/unit/tool-description.test.ts).
        let desc = SUBAGENT_TOOL_DESCRIPTION;
        for needle in [
            "use { action: \"list\" } to inspect configured agents/chains",
            "executable/non-disabled",
            "proactive skill subagent suggestions",
            "output?,reads?,progress?",
            "timeoutMs",
            "maxRuntimeMs",
            "only for foreground runs",
            "omit for async/background runs",
        ] {
            assert!(
                desc.contains(needle),
                "the tool description must contain the pi-pinned substring {needle:?}"
            );
        }
        assert!(
            !desc.contains("disabled builtins"),
            "the description must NOT contain 'disabled builtins' (pi tool-description.test.ts pins its absence)"
        );
    }

    /// T6 parity regression (pi `fanout-child.ts:156-168`): the fanout child's restricted tool must
    /// advertise pi's exact 3-line child-safe description — NOT the full orchestrator prompt — so
    /// the model inside a fanout child is told up front which actions are blocked. Pre-fix,
    /// `SubagentTool::description()` returned `SUBAGENT_TOOL_DESCRIPTION` unconditionally regardless
    /// of mode, so this assertion fails against the pre-fix behavior.
    #[test]
    fn child_safe_tool_advertises_pis_restricted_three_line_description() {
        let executor = Arc::new(SubagentExecutor::new());
        let full = SubagentTool::new(executor.clone(), PathBuf::from("/tmp"));
        let child_safe = SubagentTool::new_child_safe(executor, PathBuf::from("/tmp"));

        assert_eq!(
            Tool::description(&full),
            SUBAGENT_TOOL_DESCRIPTION,
            "the root orchestrator tool keeps the full description"
        );
        assert_eq!(
            Tool::description(&child_safe),
            "Delegate to subagents from child-safe fanout mode.\n\
             Allowed management/control actions: list, get, status, interrupt, resume, append-step, doctor.\n\
             Agent config mutation actions (create, update, delete, eject, disable, enable, reset) are blocked in this mode.",
            "the child-safe tool must advertise pi's exact fanout-child.ts:159-163 text"
        );
        // The advertised blocked list must name exactly the denylist the dispatcher enforces —
        // a child told only about create/update/delete would discover the eject/disable/enable/reset
        // block by runtime error instead, which is the whole failure this description exists to
        // prevent.
        for action in crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS {
            assert!(
                Tool::description(&child_safe).contains(action),
                "child-safe description must name the blocked action '{action}'"
            );
        }
        assert_ne!(
            Tool::description(&child_safe),
            Tool::description(&full),
            "a fanout child must NOT advertise the full orchestrator description"
        );
    }

    /// T6 parity regression (pi `fanout-child.ts:53-128`): a nested-control "interrupt" request
    /// targeting a run this executor has registered in `foreground_controls` must fire that run's
    /// interrupt token and report success; targeting an unregistered run id must report pi's exact
    /// "is not active in this fanout child" notice rather than silently doing nothing. Pre-fix, no
    /// `foreground_controls` registry existed at all (`resolve_nested_control_request` did not
    /// compile against the pre-fix `SubagentExecutor`), so this is a direct regression proof for the
    /// previously entirely-absent nested-control inbox listener.
    #[tokio::test]
    async fn resolve_nested_control_request_interrupts_a_registered_run_and_rejects_unknown_ones() {
        let executor = SubagentExecutor::new();
        let token = CancelToken::new();
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.insert(
                "run-nested-1".to_string(),
                ForegroundControlEntry {
                    interrupt: token.clone(),
                    current_agent: Some("reviewer".to_string()),
                    current_index: Some(0),
                },
            );
        }

        // Unknown target: pi's exact "not active in this fanout child" notice, `ok: false`.
        let unknown_request = crate::spawn::nested_events::NestedControlRequestRecord {
            event_type: "subagent.nested.control-request".to_string(),
            ts: 0,
            root_run_id: "root".to_string(),
            capability_token: "token".to_string(),
            request_id: "req-unknown".to_string(),
            target_run_id: "run-does-not-exist".to_string(),
            action: "interrupt".to_string(),
            message: None,
        };
        let (ok, message) = executor.resolve_nested_control_request(&unknown_request).await;
        assert!(!ok);
        assert_eq!(
            message,
            "Nested run run-does-not-exist is not active in this fanout child."
        );

        // Registered target, action=interrupt: fires the SAME token the live run races against.
        assert!(!token.is_cancelled());
        let interrupt_request = crate::spawn::nested_events::NestedControlRequestRecord {
            event_type: "subagent.nested.control-request".to_string(),
            ts: 0,
            root_run_id: "root".to_string(),
            capability_token: "token".to_string(),
            request_id: "req-interrupt".to_string(),
            target_run_id: "run-nested-1".to_string(),
            action: "interrupt".to_string(),
            message: None,
        };
        let (ok, message) = executor.resolve_nested_control_request(&interrupt_request).await;
        assert!(ok, "the first interrupt on a live token must succeed");
        assert_eq!(message, "Interrupt requested for nested run run-nested-1.");
        assert!(token.is_cancelled(), "the run's real interrupt token must now be cancelled");

        // A second interrupt on the now-already-cancelled token has nothing left to interrupt.
        let (ok, message) = executor.resolve_nested_control_request(&interrupt_request).await;
        assert!(!ok);
        assert_eq!(
            message,
            "Nested run run-nested-1 has no active child step to interrupt."
        );
    }

    /// T6 parity regression: `action="resume"` with a blank/whitespace-only message must report
    /// pi's exact "Nested resume requires message." notice (`fanout-child.ts:84-85`) BEFORE ever
    /// consulting `currentAgent`/attempting intercom delivery.
    #[tokio::test]
    async fn resolve_nested_control_request_resume_requires_a_non_blank_message() {
        let executor = SubagentExecutor::new();
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.insert(
                "run-nested-2".to_string(),
                ForegroundControlEntry {
                    interrupt: CancelToken::new(),
                    current_agent: Some("reviewer".to_string()),
                    current_index: Some(0),
                },
            );
        }
        let blank_message_request = crate::spawn::nested_events::NestedControlRequestRecord {
            event_type: "subagent.nested.control-request".to_string(),
            ts: 0,
            root_run_id: "root".to_string(),
            capability_token: "token".to_string(),
            request_id: "req-resume".to_string(),
            target_run_id: "run-nested-2".to_string(),
            action: "resume".to_string(),
            message: Some("   ".to_string()),
        };
        let (ok, message) = executor.resolve_nested_control_request(&blank_message_request).await;
        assert!(!ok);
        assert_eq!(message, "Nested resume requires message.");
    }

    /// T6 parity regression (pi `subagent-executor.ts:2968` + `fanout-child.ts:148`): an OMITTED
    /// `async` on a foreground-dispatched call must default to the extension config's
    /// `asyncByDefault`, exactly as the fanout child threads `config.asyncByDefault` into its
    /// executor. Also re-pins that `SubagentExtensionConfig` deserializes the config-file
    /// `asyncByDefault` camelCase key into `async_by_default`, so a real `config.json` value
    /// actually reaches `is_background` rather than staying stuck at the hardcoded default.
    #[test]
    fn async_by_default_config_key_deserializes_and_is_honored_by_is_background() {
        let cfg: SubagentExtensionConfig =
            serde_json::from_value(serde_json::json!({ "asyncByDefault": true })).unwrap_or_else(|e| {
                panic!("asyncByDefault must deserialize into SubagentExtensionConfig: {e}")
            });
        assert!(cfg.async_by_default);

        let omitted: SubagentToolParams =
            serde_json::from_value(serde_json::json!({ "agent": "worker", "task": "do it" }))
                .expect("single shape parses");
        assert!(
            omitted.is_background(&cfg, 0),
            "an omitted `async` must honor a config-file-sourced asyncByDefault: true"
        );
    }

    /// C8 permissive parsing (DI-SA-11): the full pi-union parse target accepts the SINGLE,
    /// PARALLEL, CHAIN, management, and control shapes, ignores unknown keys, and reports which keys
    /// were supplied — the routing dimension every `execute` dispatch arm branches on.
    #[test]
    fn subagent_tool_params_parse_every_pi_mode_shape() {
        // SINGLE with context/async/model.
        let single: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "context": "fork", "async": true,
            "model": "anthropic/claude-sonnet-4", "unknownFutureKey": 42
        }))
        .expect("single shape parses permissively (unknown keys ignored)");
        assert_eq!(single.agent.as_deref(), Some("worker"));
        assert!(single.is_background(&SubagentExtensionConfig::default(), 0));
        assert!(matches!(single.context_override(), Some(ContextMode::Fork)));
        assert!(single.provided_keys().contains(&"model"));
        assert!(!single.provided_keys().contains(&"unknownFutureKey"));

        // PARALLEL.
        let parallel: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "tasks": [{ "agent": "reviewer", "task": "review", "output": "r.md", "reads": ["in.md"], "progress": true }],
            "concurrency": 3, "worktree": true
        }))
        .expect("parallel shape parses");
        assert!(parallel.tasks.is_some());
        assert_eq!(parallel.concurrency, Some(3));
        assert_eq!(parallel.worktree, Some(true));

        // CHAIN.
        let chain: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "chain": [{ "agent": "a", "task": "Analyze {task}" }, { "parallel": [{ "agent": "b", "count": 2 }] }]
        }))
        .expect("chain shape parses");
        assert!(chain.chain.is_some());

        // Management + control actions (camelCase runId/chainName round-trip).
        let mgmt: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "action": "get", "chainName": "release", "agent": "pkg.reviewer"
        }))
        .expect("management shape parses");
        assert_eq!(mgmt.action.as_deref(), Some("get"));
        assert_eq!(mgmt.chain_name.as_deref(), Some("release"));

        let control: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "action": "resume", "runId": "abc", "index": 0, "message": "continue"
        }))
        .expect("control shape parses");
        assert_eq!(control.run_id.as_deref(), Some("abc"));
        assert_eq!(control.index, Some(0));
    }

    /// R-SA parity regression: `config.asyncByDefault`/`forceTopLevelAsync` must actually be
    /// consulted by [`SubagentToolParams::is_background`] (pi `subagent-executor.ts:2968,3019-3020`,
    /// `runs/background/top-level-async.ts:5-12`), not just parsed and discarded. Before this fix
    /// `is_background` hardcoded `self.r#async.unwrap_or(false)`, so every one of these assertions
    /// would fail pre-fix (an omitted `async` always resolved to foreground, `forceTopLevelAsync`
    /// never flipped anything to background, and `clarify: true` never suppressed an async request).
    #[test]
    fn is_background_honors_async_by_default_and_force_top_level_async() {
        // An omitted `async` falls back to `config.asyncByDefault`, not a hardcoded `false`.
        let omitted: SubagentToolParams =
            serde_json::from_value(serde_json::json!({ "agent": "worker", "task": "do it" }))
                .expect("single shape parses");
        let async_by_default_cfg =
            SubagentExtensionConfig { async_by_default: true, ..SubagentExtensionConfig::default() };
        assert!(
            omitted.is_background(&async_by_default_cfg, 0),
            "an omitted `async` must default to config.asyncByDefault"
        );
        assert!(
            !omitted.is_background(&SubagentExtensionConfig::default(), 0),
            "asyncByDefault: false (the default) must still leave an omitted `async` foreground"
        );

        // An explicit `async: false` still wins over `asyncByDefault: true` (only an OMITTED value
        // falls back to the config default).
        let explicit_false: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "async": false
        }))
        .expect("single shape parses");
        assert!(!explicit_false.is_background(&async_by_default_cfg, 0));

        // `forceTopLevelAsync` forces async ON at depth 0 regardless of the call's own `async`
        // value, but has no effect at a nested depth.
        let force_cfg = SubagentExtensionConfig {
            force_top_level_async: true,
            ..SubagentExtensionConfig::default()
        };
        assert!(
            explicit_false.is_background(&force_cfg, 0),
            "forceTopLevelAsync must force a top-level (depth 0) run to background even when the \
             call explicitly requested async: false"
        );
        assert!(
            !explicit_false.is_background(&force_cfg, 1),
            "forceTopLevelAsync must NOT apply at a nested depth"
        );

        // `clarify: true` always keeps the run foreground, even when async was requested.
        let clarify_true: SubagentToolParams = serde_json::from_value(serde_json::json!({
            "agent": "worker", "task": "do it", "async": true, "clarify": true
        }))
        .expect("single shape parses");
        assert!(!clarify_true.is_background(&SubagentExtensionConfig::default(), 0));
    }

    /// SUBA-002 regression (pi `reserveSubagentSpawns`, `subagent-executor.ts:266-282` +
    /// `:3434-3441`): `maxSubagentSpawnsPerSession` is ENFORCED across a session's successive
    /// dispatches, not merely parsed. Pre-fix, the config field had no read site anywhere in the
    /// crate, so every call below routed straight into execution and the second/third calls failed
    /// with `"agent not found: ghost"` instead of the spawn-limit notice.
    ///
    /// The budget is charged UP FRONT: call 1 requests 2 of a 2-spawn budget and is admitted (it
    /// then fails on the unresolvable agent, as pi's would), and that failure does NOT refund — call
    /// 2 is rejected before any routing at all.
    #[tokio::test]
    async fn spawn_budget_is_charged_per_session_and_rejects_the_call_that_would_exceed_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 2,
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let tool = ext.subagent_tool();

        async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }

        // Call 1: a 2-task fan-out exactly fills the 2-spawn budget (pi's comparison is a STRICT
        // `used + requested > maxSpawns`, so landing on the cap is admitted). It is admitted, and
        // therefore fails downstream on the unresolvable agent — NOT on the budget.
        let admitted = dispatch(&tool, serde_json::json!({
            "tasks": [{ "agent": "ghost", "task": "a" }, { "agent": "ghost", "task": "b" }]
        }))
        .await
        .expect_err("an unresolvable agent still fails after the reservation is granted");
        assert!(
            admitted.to_string().contains("agent not found: ghost"),
            "the first call must be ADMITTED past the budget (failing only on the agent): {admitted}"
        );

        // Call 2: the budget was billed up front and is not refunded by call 1's failure, so a
        // single further spawn is now over the cap and the whole call is rejected before routing.
        let rejected = dispatch(&tool, serde_json::json!({ "agent": "ghost", "task": "c" }))
            .await
            .expect_err("the session's spawn budget is exhausted");
        assert_eq!(
            rejected.to_string(),
            "Subagent spawn limit reached for this session (2/2 used, 1 requested). \
             Complete the work directly or start a new session.",
            "pi's verbatim over-limit notice, with used/max/requested filled in"
        );
        assert!(
            !rejected.to_string().contains("agent not found"),
            "the rejection must fire BEFORE any routing/agent resolution: {rejected}"
        );

        // A fresh session zeroes the budget (pi `resetSessionState`), so the very same call is
        // admitted again afterwards.
        ext.executor().reset_spawn_budget();
        let after_reset = dispatch(&tool, serde_json::json!({ "agent": "ghost", "task": "c" }))
            .await
            .expect_err("post-reset the call is admitted and fails only on the agent");
        assert!(
            after_reset.to_string().contains("agent not found: ghost"),
            "a session reset must restore the budget: {after_reset}"
        );
    }

    /// SUBA-002's request-counting rules (pi `countRequestedSubagentSpawns`,
    /// `subagent-executor.ts:284-292`), observed through the rejection notice's `N requested` field:
    /// a CHAIN bills each step, with a dynamic-parallel step billed its worst-case fan-out
    /// (`expand.maxItems`, else `config.chain.dynamicFanout.maxItems`, else 0) and a static parallel
    /// step billed its task count.
    #[tokio::test]
    async fn chain_spawn_count_bills_dynamic_fanout_worst_case_and_parallel_width() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                chain: Some(crate::registration::ExtensionChainConfig {
                    dynamic_fanout: Some(crate::registration::DynamicFanoutConfig {
                        max_items: Some(7),
                    }),
                }),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );
        let tool = ext.subagent_tool();

        async fn reject_text(tool: &SubagentTool, params: serde_json::Value) -> String {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("over the 1-spawn budget")
            .to_string()
        }

        // Every chain below is STRUCTURALLY VALID (the dynamic steps satisfy
        // `validate_dynamic_step_shape`), so the notice asserted here is the budget's, not a shape
        // diagnostic that happens to precede it.
        //
        // A sequential step (1) + a static parallel step of width 3 (3) + a dynamic-parallel step
        // with its own `expand.maxItems: 5` (5) == 9 requested.
        let explicit = reject_text(&tool, serde_json::json!({ "chain": [
            { "agent": "ghost", "task": "a", "as": "targets" },
            { "parallel": [
                { "agent": "ghost", "task": "b" },
                { "agent": "ghost", "task": "c" },
                { "agent": "ghost", "task": "d" }
            ] },
            {
                "expand": { "from": { "output": "targets", "path": "/items" }, "maxItems": 5 },
                "collect": { "as": "gathered" },
                "parallel": { "agent": "ghost", "task": "Handle {item}" }
            }
        ]}))
        .await;
        assert!(explicit.contains("(0/1 used, 9 requested)"), "got: {explicit}");

        // With `expand.maxItems` omitted the dynamic step falls back to the CONFIGURED
        // `chain.dynamicFanout.maxItems` (7 here), so 1 + 7 == 8 requested.
        ext.executor().reset_spawn_budget();
        let configured = reject_text(&tool, serde_json::json!({ "chain": [
            { "agent": "ghost", "task": "a", "as": "targets" },
            {
                "expand": { "from": { "output": "targets", "path": "/items" } },
                "collect": { "as": "gathered" },
                "parallel": { "agent": "ghost", "task": "Handle {item}" }
            }
        ]}))
        .await;
        assert!(configured.contains("(0/1 used, 8 requested)"), "got: {configured}");
    }

    /// A [`SingleStepSpec`](crate::spawn::chain_graph::SingleStepSpec) with nothing set beyond the
    /// agent + task the lowered slash graphs below need, so the spawn-budget assertions stay about
    /// the COUNT and not about step configuration.
    fn bare_single_step(agent: &str, task: &str) -> crate::spawn::chain_graph::SingleStepSpec {
        crate::spawn::chain_graph::SingleStepSpec {
            agent: agent.to_string(),
            task: task.to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    /// SUBA-002 regression: the per-SESSION spawn budget covers the SLASH surface, not the
    /// `subagent` tool alone. Upstream gets this structurally — `/run`'s handler goes
    /// `runSlashSubagent` -> `requestSlashRun` -> the bridge at `extension/index.ts:396-401` ->
    /// `executeSubagentCollapsed` -> the SAME `executor.execute` whose `reserveSubagentSpawns`
    /// (`subagent-executor.ts:266-282`, called at `:3434-3441`) charges the tool — so the cap is
    /// unbypassable there. In this crate `dispatch_slash` is an independent entry into
    /// `SubagentExecutor`, and pre-fix it reached `run_foreground`/`spawn_background` with no charge
    /// at all: this test's `/run` calls all sailed past an exhausted budget and failed downstream on
    /// the unresolvable agent instead.
    ///
    /// Drives the REAL production surface end to end (`dispatch_slash(SlashCommandName::Run, …)`,
    /// i.e. the argument string a user types), for both the foreground and the `--bg` shape, and
    /// pins the notice to the byte-identical text the tool path emits.
    #[tokio::test]
    async fn slash_run_is_charged_against_the_same_session_spawn_budget_as_the_tool() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        // Spend the session's single spawn through the TOOL. It is admitted past the budget and so
        // fails only on the unresolvable agent — and the reservation is never refunded.
        let spent = ext
            .subagent_tool()
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "agent": "ghost", "task": "a" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an unresolvable agent still fails after the reservation is granted");
        assert!(
            spent.to_string().contains("agent not found: ghost"),
            "the tool call must be ADMITTED past the budget: {spent}"
        );

        // The slash surface now sees an exhausted budget and must refuse — foreground AND `--bg`,
        // since pi bills the SINGLE shape exactly 1 either way.
        for args in ["ghost do the thing", "ghost do the thing --bg"] {
            let err = ext
                .dispatch_slash(SlashCommandName::Run, args, dir.path())
                .await
                .expect_err("the session's spawn budget is exhausted");
            assert!(
                matches!(err, SubagentError::SpawnLimitExceeded(_)),
                "`/run {args}` must be refused by the budget, got: {err:?}"
            );
            assert_eq!(
                err.to_string(),
                "Subagent spawn limit reached for this session (1/1 used, 1 requested). \
                 Complete the work directly or start a new session.",
                "pi's verbatim over-limit notice, identical to the tool path's"
            );
            assert!(
                !err.to_string().contains("agent not found"),
                "the refusal must fire BEFORE agent resolution / any spawn: {err}"
            );
        }

        // A fresh session zeroes the budget (pi `resetSessionState`), so the very same `/run` is
        // admitted again afterwards and fails only on the agent — proving the refusal above was the
        // budget, not a blanket slash-path rejection.
        ext.executor().reset_spawn_budget();
        let after_reset = ext
            .dispatch_slash(SlashCommandName::Run, "ghost do the thing", dir.path())
            .await
            .expect_err("post-reset the call is admitted and fails only on the agent");
        assert!(
            matches!(after_reset, SubagentError::AgentNotFound(_)),
            "a session reset must restore the slash surface's budget, got: {after_reset:?}"
        );
    }

    /// SUBA-002 regression, chain-shaped slash surfaces: `/chain`, `/parallel` and `/run-chain` all
    /// funnel through [`SubagentsExtension::run_or_background_chain`], which pre-fix reached
    /// `run_or_background_graph` with no spawn charge whatsoever. Each is now billed over the
    /// LOWERED graph by [`count_graph_requested_spawns`], applying pi's per-step rule
    /// (`countRequestedSubagentSpawns`, `subagent-executor.ts:284-292`) arm for arm — asserted
    /// through the `N requested` field of the refusal notice, and for both `background: false` and
    /// `background: true`, since the charge sits ahead of that split.
    #[tokio::test]
    async fn slash_chain_surfaces_bill_the_lowered_graph_against_the_session_budget() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 1,
                chain: Some(crate::registration::ExtensionChainConfig {
                    dynamic_fanout: Some(crate::registration::DynamicFanoutConfig {
                        max_items: Some(7),
                    }),
                }),
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        let dynamic = |max_items: Option<u32>| {
            RunnerStep::DynamicGroup(crate::spawn::chain_graph::DynamicGroupSpec {
                expand: "{outputs.targets}/items".to_string(),
                template: Box::new(bare_single_step("ghost", "Handle {item}")),
                collect: "gathered".to_string(),
                concurrency: 2,
                item: None,
                key: None,
                max_items,
                on_empty: crate::spawn::chain_graph::OnEmpty::Skip,
                collect_schema: None,
            })
        };

        // (graph, expected `N requested`) — two sequential steps bill 1 each; a static parallel
        // group bills its width; a dynamic group bills `expand.maxItems` when it has one, else the
        // configured `chain.dynamicFanout.maxItems` (7 here).
        let cases: Vec<(Vec<RunnerStep>, u32)> = vec![
            (
                vec![
                    RunnerStep::SingleStep(bare_single_step("ghost", "a")),
                    RunnerStep::SingleStep(bare_single_step("ghost", "b")),
                ],
                2,
            ),
            (
                vec![RunnerStep::ParallelGroup(crate::spawn::chain_graph::ParallelGroupSpec {
                    steps: vec![
                        bare_single_step("ghost", "a"),
                        bare_single_step("ghost", "b"),
                        bare_single_step("ghost", "c"),
                    ],
                    concurrency: 3,
                    fail_fast: false,
                    worktree: false,
                })],
                3,
            ),
            (vec![dynamic(Some(5))], 5),
            (vec![dynamic(None)], 7),
            (
                vec![
                    RunnerStep::SingleStep(bare_single_step("ghost", "a")),
                    dynamic(Some(5)),
                ],
                6,
            ),
        ];

        for (graph, expected) in cases {
            for background in [false, true] {
                ext.executor().reset_spawn_budget();
                let err = ext
                    .run_or_background_chain(
                        dir.path(),
                        graph.clone(),
                        RunMode::Chain,
                        None,
                        background,
                        None,
                    )
                    .await
                    .expect_err("the graph is over the 1-spawn budget");
                assert!(
                    matches!(err, SubagentError::SpawnLimitExceeded(_)),
                    "background={background}: expected a spawn-budget refusal, got: {err:?}"
                );
                assert_eq!(
                    err.to_string(),
                    format!(
                        "Subagent spawn limit reached for this session (0/1 used, {expected} \
                         requested). Complete the work directly or start a new session."
                    ),
                    "background={background}: the lowered graph must bill pi's per-step count"
                );
            }
        }

        // An EMPTY graph short-circuits ahead of the charge and never touches the counter (pi's
        // `if (input.requested <= 0) return undefined`), so the budget is still untouched after it.
        ext.executor().reset_spawn_budget();
        let empty = ext
            .run_or_background_chain(dir.path(), vec![], RunMode::Chain, None, false, None)
            .await
            .expect("an empty graph is not an error");
        assert_eq!(empty, "chain has no steps to run");
        assert!(
            ext.executor().reserve_subagent_spawns(1, 1).is_ok(),
            "an empty graph must not have consumed the session's spawn"
        );
    }

    /// SUBA-002's no-double-charge invariant: the `subagent` TOOL's chain/parallel shapes reserve
    /// exactly ONCE (in [`SubagentTool::execute`]) and then reach
    /// [`SubagentExecutor::run_or_background_graph`] through
    /// `route_chain_mode`/`route_parallel_mode` — never through the slash-only
    /// [`SubagentsExtension::run_or_background_chain`] wrapper that carries the second charge. A
    /// 3-wide tool fan-out under a 3-spawn budget must therefore bill 3, not 6.
    #[tokio::test]
    async fn tool_chain_dispatch_is_billed_exactly_once_not_twice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(
            SubagentExtensionConfig {
                max_subagent_spawns_per_session: 3,
                ..SubagentExtensionConfig::default()
            },
            dir.path().to_path_buf(),
        );

        let admitted = ext
            .subagent_tool()
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "chain": [
                    { "agent": "ghost", "task": "a" },
                    { "parallel": [
                        { "agent": "ghost", "task": "b" },
                        { "agent": "ghost", "task": "c" }
                    ] }
                ]}),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an unresolvable agent still fails after the reservation is granted");
        assert!(
            !admitted.to_string().contains("Subagent spawn limit reached"),
            "a 3-wide chain must fit exactly inside a 3-spawn budget: {admitted}"
        );

        // Exactly 3 charged, so `used` reads 3/3 (a double charge would have overflowed the cap
        // during the dispatch above and reported 6 requested against it).
        let exhausted = ext
            .dispatch_slash(SlashCommandName::Run, "ghost do the thing", dir.path())
            .await
            .expect_err("the session's spawn budget is now exactly exhausted");
        assert_eq!(
            exhausted.to_string(),
            "Subagent spawn limit reached for this session (3/3 used, 1 requested). \
             Complete the work directly or start a new session.",
            "the tool's chain dispatch must have been billed once (3), not twice (6)"
        );
    }

    /// Dispatch discrimination: management/control/parallel/chain modes are each RECOGNIZED and
    /// routed to their own arm rather than mis-parsed as a broken SINGLE call. Management/control
    /// still short-circuit at their P1 stubs; parallel/chain now route to REAL execution, proven
    /// here without any spawn by using an unresolvable agent so plan-time persona resolution fails
    /// (`AgentNotFound`) before any child process — the assertion stays on the dispatch decision.
    #[tokio::test]
    async fn tool_execute_routes_each_mode_to_its_dispatch_arm() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }

        // Management action → now wired (C3): `list` succeeds and renders the pi list shape,
        // proving the dispatch reached the real management arm rather than a stub.
        let mgmt_ok = dispatch(&tool, serde_json::json!({ "action": "list" }))
            .await
            .expect("management action 'list' is wired and returns the agent/chain listing");
        let mgmt_text = mgmt_ok
            .content
            .iter()
            .find_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(mgmt_text.contains("Executable agents:"), "got: {mgmt_text}");
        assert!(mgmt_text.contains("Chains:"), "got: {mgmt_text}");

        // Control action → now wired (C5): an unknown run id fails with the not-found notice,
        // proving the dispatch reached the real control arm rather than a stub.
        let control_err = dispatch(&tool, serde_json::json!({ "action": "status", "id": "run1" }))
            .await
            .expect_err("control action routes to real status, which fails on the unknown id");
        assert!(
            control_err.to_string().contains("Async run not found"),
            "got: {control_err}"
        );

        // PARALLEL (tasks[]) → parallel arm. Now routes through the REAL plan-execution path, so an
        // unresolvable agent fails at plan-time persona resolution (`AgentNotFound`) BEFORE any
        // spawn — proving the dispatch reached the parallel arm and its real routing, not a stub.
        let parallel_err = dispatch(&tool, serde_json::json!({ "tasks": [{ "agent": "x", "task": "y" }] }))
            .await
            .expect_err("tasks[] routes to real parallel execution, which fails on the unknown agent");
        assert!(
            parallel_err.to_string().contains("agent not found: x"),
            "got: {parallel_err}"
        );

        // CHAIN (chain[]) → chain arm, likewise failing at plan-time persona resolution.
        let chain_err = dispatch(&tool, serde_json::json!({ "chain": [{ "agent": "x", "task": "y" }] }))
            .await
            .expect_err("chain[] routes to real chain execution, which fails on the unknown agent");
        assert!(
            chain_err.to_string().contains("agent not found: x"),
            "got: {chain_err}"
        );

        // Unknown action → explicit unknown-action error listing the valid set.
        let unknown_err = dispatch(&tool, serde_json::json!({ "action": "frobnicate" }))
            .await
            .expect_err("an unknown action is rejected");
        assert!(unknown_err.to_string().contains("unknown subagent action 'frobnicate'"));
        // The unknown-action message must enumerate the actions that DO dispatch, so a model that
        // guessed wrong is told the real set (SUBA-005 widened it by four).
        for action in crate::discovery::management::MANAGEMENT_ACTIONS {
            assert!(
                unknown_err.to_string().contains(action),
                "the unknown-action error must list '{action}'; got: {unknown_err}"
            );
        }
    }

    /// SUBA-005 dispatch proof, separated from the omnibus test above because the assertion is on
    /// the handler's own text: each new verb reaches `handle_eject`/`handle_disable`/`handle_enable`/
    /// `handle_reset` and answers with pi's verbatim "Specify 'agent' for &lt;verb&gt;." validation —
    /// which is only reachable through the real handler. Pre-fix, `route_action` had no arm for any
    /// of the four and answered "unknown subagent action '&lt;verb&gt;'" instead.
    #[tokio::test]
    async fn tool_execute_routes_the_four_suba_005_actions_to_their_real_handlers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        for verb in ["eject", "disable", "enable", "reset"] {
            let err = tool
                .execute(
                    ToolCallId::from("t"),
                    serde_json::json!({ "action": verb }),
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await
                .expect_err("a management action with no 'agent' is an error outcome");
            assert_eq!(
                err.to_string(),
                format!("Specify 'agent' for {verb}."),
                "action '{verb}' must be serviced by its own handler, not the unknown-action arm"
            );
        }
    }

    /// T6 regression (pi `MUTATING_MANAGEMENT_ACTIONS`, `subagent-executor.ts:112`): a fanout child
    /// is refused ALL SEVEN mutating management actions — including the four SUBA-005 added — and
    /// the refusal happens BEFORE any discovery or filesystem access, so a child cannot even probe
    /// the parent's config through them. The read-only verbs are unaffected.
    #[tokio::test]
    async fn child_safe_tool_blocks_all_seven_mutating_management_actions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let child = SubagentTool::new_child_safe(
            Arc::new(SubagentExecutor::new()),
            dir.path().to_path_buf(),
        );

        for action in crate::discovery::management::MUTATING_MANAGEMENT_ACTIONS {
            let result = child
                .execute(
                    ToolCallId::from("t"),
                    serde_json::json!({ "action": action, "agent": "scout" }),
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await;
            let err = result.err().unwrap_or_else(|| {
                panic!("child-safe mode must refuse the mutating action '{action}'")
            });
            assert!(
                err.to_string().contains("blocked in child-safe fanout mode"),
                "action '{action}' must be refused by the T6 denylist; got: {err}"
            );
        }

        // The read-only verbs still work in child-safe mode — the denylist is a denylist, not a
        // blanket management block.
        let listed = child
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "action": "list" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect("child-safe mode still permits the read-only 'list'");
        assert!(!listed.content.is_empty());
    }

    /// SUBA-041 (re-scoped from `single_mode_rejects_unwired_override_params_before_any_agent_resolution`,
    /// which pinned the pre-fix behavior of rejecting all NINE schema-advertised SINGLE-mode
    /// overrides): the seven params pi's `runSinglePath` honors must now be ACCEPTED — a call
    /// carrying them proceeds past dispatch into agent resolution, so the only error left is the
    /// unresolvable agent — while the two the schema no longer advertises must still be refused
    /// LOUDLY by name, never silently dropped.
    ///
    /// The `"ghost"` agent makes the two outcomes trivially distinguishable: `agent not found:
    /// ghost` proves the param got through dispatch; the named refusal proves it did not. Against
    /// pre-SUBA-041 code every one of the seven produced the refusal instead, so this fails there.
    #[tokio::test]
    async fn single_mode_wires_the_seven_supported_overrides_and_refuses_only_the_two_unadvertised()
    {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        // The seven SUBA-041 wired the schema's promise back onto: each must reach agent resolution.
        let accepted = [
            serde_json::json!({ "share": true }),
            serde_json::json!({ "sessionDir": "~/x" }),
            serde_json::json!({ "artifacts": false }),
            serde_json::json!({ "output": "report.md" }),
            serde_json::json!({ "output": "report.md", "outputMode": "file-only" }),
            serde_json::json!({ "skill": "rust,testing" }),
            serde_json::json!({ "acceptance": "checked" }),
        ];
        for (i, extra) in accepted.iter().enumerate() {
            let mut params = serde_json::json!({ "agent": "ghost", "task": "do it" });
            for (key, value) in extra.as_object().expect("object literal") {
                params
                    .as_object_mut()
                    .expect("object literal")
                    .insert(key.clone(), value.clone());
            }
            let err = tool
                .execute(
                    ToolCallId::from(format!("accepted-{i}").as_str()),
                    params.clone(),
                    CancelToken::new(),
                    Box::new(|_u: cyrup_core::ToolUpdate| {}),
                )
                .await
                .expect_err("the agent is unresolvable, so the call still errors");
            let message = err.to_string();
            assert!(
                message.contains("agent not found"),
                "{params} must be ACCEPTED at dispatch and fail only on agent resolution: {message}"
            );
            assert!(
                !message.contains("does not support"),
                "{params} must not be refused as an unsupported param: {message}"
            );
        }

        // The two the schema no longer advertises are still named and refused, before any
        // agent resolution — an override is never silently dropped.
        let err = tool
            .execute(
                ToolCallId::from("refused"),
                serde_json::json!({
                    "agent": "ghost", "task": "do it",
                    "includeProgress": true, "control": { "enabled": true }
                }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("a param with no subsystem behind it must be refused");
        let message = err.to_string();
        assert!(message.contains("includeProgress"), "got: {message}");
        assert!(message.contains("control"), "got: {message}");
        assert!(
            !message.contains("agent not found"),
            "the refusal must fire BEFORE agent resolution ever runs: {message}"
        );

        // A malformed `acceptance` policy is refused up front with pi's own
        // `validateAcceptanceInput` message (`acceptance.ts:143`), not swallowed.
        let bad_acceptance = tool
            .execute(
                ToolCallId::from("bad-acceptance"),
                serde_json::json!({ "agent": "ghost", "task": "do it", "acceptance": "nonsense" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("an invalid acceptance level must be refused")
            .to_string();
        assert!(
            bad_acceptance.contains("acceptance has invalid level 'nonsense'."),
            "pi's verbatim validation message: {bad_acceptance}"
        );
        assert!(
            !bad_acceptance.contains("agent not found"),
            "acceptance validation must precede agent resolution: {bad_acceptance}"
        );
    }

    /// R-SA-069 (pi `executeWithSingleDispatchGuard`, `subagent-executor.ts:3227-3242`): a second
    /// non-`action` subagent call arriving while a prior one from the SAME tool instance is still in
    /// flight is rejected outright with pi's exact text — never queued, never silently allowed to
    /// run concurrently. Simulates "a prior dispatch is in progress" by holding the guard's one slot
    /// directly (rather than actually racing two `execute` futures), which isolates the assertion to
    /// the guard/rejection wiring itself. `action` calls remain unaffected (management/control
    /// bypasses the guard entirely, pi's `if (params.action) return execute(...)` early return).
    #[tokio::test]
    async fn subagent_tool_rejects_a_second_concurrent_dispatch_while_one_is_in_flight() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        let _held = tool
            .dispatch_guard
            .try_acquire()
            .expect("the guard's single slot is free before any dispatch has run");

        let err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "agent": "worker", "task": "do it" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("a second non-action call while one is in flight must be rejected outright");
        assert_eq!(
            err.to_string(),
            "Rejected: a subagent call is already in progress. Issue exactly ONE subagent call per turn."
        );

        // `action` calls are NEVER gated by the guard (pi's early return before the flag check).
        let action_err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "action": "status", "id": "run1" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("action calls resolve to the real control arm, which fails on the unknown id");
        assert!(
            action_err.to_string().contains("Async run not found"),
            "an `action` call must bypass the dispatch guard entirely, got: {action_err}"
        );
    }

    /// pi `validateExecutionInput`'s mode-exclusivity gate (`subagent-executor.ts:1124-1143`,
    /// `hasChain`/`hasTasks`/`hasSingle` at `2995-2997`): mode is selected by a NON-EMPTY array, not
    /// merely the field's presence — an explicit `tasks: []` or `chain: []` (with no `agent`) must
    /// fall through to "Provide exactly one mode", never silently execute as an empty parallel run
    /// (which would previously report a vacuous "0/0 succeeded") or an empty chain.
    #[tokio::test]
    async fn subagent_tool_rejects_empty_tasks_and_chain_arrays_as_no_mode_selected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());

        async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }

        let empty_tasks_err = dispatch(&tool, serde_json::json!({ "tasks": [] }))
            .await
            .expect_err("an explicit empty tasks[] must error rather than run as an empty parallel group");
        assert!(
            empty_tasks_err.to_string().starts_with("Provide exactly one mode. Agents:"),
            "got: {empty_tasks_err}"
        );

        let empty_chain_err = dispatch(&tool, serde_json::json!({ "chain": [] }))
            .await
            .expect_err("an explicit empty chain[] must error rather than run as an empty chain");
        assert!(
            empty_chain_err.to_string().starts_with("Provide exactly one mode. Agents:"),
            "got: {empty_chain_err}"
        );
    }

    /// pi `params.id ?? params.runId` (`subagent-executor.ts:2846`): a caller using `runId` alone
    /// (no `id`) for `action: "status"` must still resolve to THAT run's own report — surfacing its
    /// specific not-found error — rather than silently falling through to the no-id "list active
    /// runs" view (which would return an `Ok` empty-list result instead of this `Err`).
    #[tokio::test]
    async fn control_status_action_uses_run_id_when_id_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());
        let err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "action": "status", "runId": "run1" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("a runId-only status call must resolve to that run's own not-found report");
        assert!(
            err.to_string().contains("Async run not found"),
            "got: {err}; a `runId`-only status call must not silently degrade to the no-id \
             \"list active runs\" view"
        );
    }

    /// pi `run-status.ts:104-110`: the child-safe fanout tool's `{ action: "status" }` call with no
    /// id/runId/dir must hard-error with pi's exact message rather than listing the cwd's active
    /// runs. Pre-fix, `SubagentTool::new_child_safe` had no way to signal this to `control_status`,
    /// so this dispatch would have returned `Ok` with the "No active async runs." list instead of
    /// this `Err`.
    #[tokio::test]
    async fn child_safe_tool_status_with_no_id_hard_errors_instead_of_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let tool = SubagentTool::new_child_safe(Arc::new(SubagentExecutor::new()), dir.path().to_path_buf());
        let err = tool
            .execute(
                ToolCallId::from("t"),
                serde_json::json!({ "action": "status" }),
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
            .expect_err("child-safe no-id status must hard-error");
        assert_eq!(
            err.to_string(),
            "Child-safe subagent status requires an id when no foreground run is active."
        );
    }

    /// pi `resolveRequestedCwd` (`subagent-executor.ts:193-195,2801-2802`): an explicit `cwd` param
    /// must be resolved and threaded into the dispatch's own discovery, not silently ignored in
    /// favor of the tool's construction-time cwd. Proven end-to-end with the read-only `get`
    /// management action (no process spawn, so safe to drive to completion): an agent that exists
    /// ONLY under a disjoint `cwd` param is found when — and only when — that `cwd` is honored.
    #[tokio::test]
    async fn subagent_tool_cwd_param_is_resolved_and_threaded_into_dispatch() {
        let dir_a = tempfile::tempdir().expect("tempdir a");
        let dir_b = tempfile::tempdir().expect("tempdir b");
        let agents_dir_b = dir_b.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents_dir_b).expect("mkdir dirB agents");
        std::fs::write(
            agents_dir_b.join("beta.md"),
            "---\nname: beta\ndescription: Only discoverable under dirB\n---\nBody.\n",
        )
        .expect("write dirB agent fixture");

        let tool = SubagentTool::new(Arc::new(SubagentExecutor::new()), dir_a.path().to_path_buf());

        async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
            tool.execute(
                ToolCallId::from("t"),
                params,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }

        // Without an explicit `cwd`, discovery runs over the tool's construction-time `self.cwd`
        // (dirA), which has no "beta" agent.
        let without_cwd = dispatch(&tool, serde_json::json!({ "action": "get", "agent": "beta" }))
            .await
            .expect_err("dirA has no 'beta' agent, so 'get' must fail absent an explicit cwd");
        assert!(without_cwd.to_string().contains("not found"), "got: {without_cwd}");

        // With an explicit `cwd` pointing at dirB, discovery must run over dirB instead — finding
        // "beta". Pre-fix, `cwd` was parsed and discarded, so this would ALSO have failed exactly
        // like the call above (self.cwd never changes).
        let ok = dispatch(
            &tool,
            serde_json::json!({
                "action": "get",
                "agent": "beta",
                "cwd": dir_b.path().to_string_lossy(),
            }),
        )
        .await
        .expect("an explicit cwd must be resolved and fed into discovery, finding dirB's agent");
        let text = ok
            .content
            .iter()
            .find_map(|c| match c {
                cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert!(text.contains("beta"), "got: {text}");
    }

    #[test]
    fn slash_command_name_round_trips_every_registered_descriptor() {
        for descriptor in SLASH_COMMANDS {
            let parsed = SlashCommandName::from_str_exact(descriptor.name.as_str());
            assert_eq!(parsed, Some(descriptor.name));
        }
    }

    #[tokio::test]
    async fn resolve_agent_returns_not_found_for_an_unknown_name() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .resolve_agent(dir.path(), "no-such-agent", AgentReadScope::Both)
            .expect_err("unknown agent must error");
        assert!(matches!(err, SubagentError::AgentNotFound(_)));
    }

    #[tokio::test]
    async fn run_foreground_errors_before_any_spawn_when_agent_is_unknown() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .run_foreground(dir.path(), "ghost", "do something", Some(ContextMode::Fresh), None, None)
            .await
            .expect_err("unresolvable agent must fail before any subprocess spawn");
        assert!(matches!(err, SubagentError::AgentNotFound(_)));
    }

    /// R-SA-055 (SAFETY-CRITICAL): `run_foreground`'s depth guard must run BEFORE agent discovery
    /// — proven by supplying a completely unresolvable agent name (`"ghost"`, exactly the same
    /// name [`run_foreground_errors_before_any_spawn_when_agent_is_unknown`] above uses to prove
    /// discovery's own independent failure mode) alongside a config whose `max_subagent_depth` is
    /// already exhausted. If the depth guard ran AFTER discovery (or not at all), this call would
    /// surface `AgentNotFound` — exactly like the sibling test above — since `"ghost"` never
    /// resolves either way; observing `DepthExceeded` instead is structural proof the guard
    /// short-circuited before `resolve_agent` (and therefore before any discovery filesystem scan)
    /// ever ran.
    #[tokio::test]
    async fn run_foreground_rejects_on_depth_before_agent_discovery_ever_runs() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0; // current_depth (0, absent env) >= max_depth (0): blocked
        }
        let dir = tempfile::tempdir().expect("tempdir");
        // No `.cyrup/agents` directory is even created under `dir` — if discovery ran at all it
        // would find nothing and (for a real agent name) still fail with AgentNotFound; using the
        // exact same "ghost" name as the sibling discovery-failure test isolates this test's
        // assertion to purely WHICH error surfaces first.
        let err = executor
            .run_foreground(dir.path(), "ghost", "do something", Some(ContextMode::Fresh), None, None)
            .await
            .expect_err("a blocked depth ceiling must reject before agent discovery runs");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded (proving the guard ran BEFORE discovery could report its own \
             AgentNotFound for the same unresolvable name), got: {err:?}"
        );
    }

    /// The background (`bg: true`) shape's own independent entry point must enforce the identical
    /// R-SA-055 ordering: depth guard before discovery, fork-context resolution, run-directory
    /// creation, or the detached hop-1 process spawn. Proven the same way as the foreground test
    /// above — an unresolvable agent name combined with an exhausted depth ceiling must surface
    /// `DepthExceeded`, not `AgentNotFound`, AND no run directory may exist afterward (the
    /// filesystem-level proof that `spawn_background` never reached its own `create_dir_all`/
    /// detached-spawn steps, which live strictly after the depth check in program order).
    #[tokio::test]
    async fn spawn_background_rejects_on_depth_before_discovery_or_any_directory_creation() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let err = executor
            .spawn_background(
                dir.path(),
                "ghost",
                "do something",
                Some(ContextMode::Fresh),
                None,
                AgentReadScope::Both,
            )
            .await
            .expect_err("a blocked depth ceiling must reject before discovery or any spawn setup");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded ahead of discovery's own AgentNotFound, got: {err:?}"
        );
        // The load-bearing proof that NOTHING was set up: neither the async-run root nor the
        // results directory `spawn_background` would otherwise create via `create_dir_all` (both
        // strictly after the depth check in program order) may exist.
        assert!(
            !default_async_root(dir.path()).exists(),
            "the async-run root must never be created for a depth-blocked background dispatch"
        );
        assert!(
            !default_results_dir(dir.path()).exists(),
            "the results directory must never be created for a depth-blocked background dispatch"
        );
    }

    /// pi `executeAsyncSingle` (`async-execution.ts:849-855`): `params.modelOverride ?? agent.model`
    /// reaches the detached runner's step for an async SINGLE run regardless of whether that run is
    /// foreground or background. Before this fix, [`SubagentExecutor::spawn_background`] hardcoded
    /// `model: None` into the `SingleStepSpec` it wrote into `runner-config.json`, silently dropping
    /// any per-call model override the instant a SINGLE run went `bg: true` (it reached the runner
    /// fine on the foreground path, `run_foreground_streaming`'s `model_override`). Proven at the
    /// filesystem boundary: the one-shot `runner-config.json` handoff file this call writes (R-SA-073)
    /// must carry the override on its sole step.
    #[tokio::test]
    async fn spawn_background_single_carries_the_model_override_into_the_runner_config() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = executor
            .spawn_background(
                dir.path(),
                "worker",
                "do something",
                Some(ContextMode::Fresh),
                Some(ModelId::from("anthropic/claude-override-test")),
                AgentReadScope::Both,
            )
            .await
            .expect("spawn_background should succeed for a resolvable builtin agent");

        let crate::background::RunArtifactRoots { async_root, results_dir } =
            crate::background::run_artifact_roots(dir.path());
        let run_paths = crate::background::RunPaths::for_run(&async_root, &results_dir, &run_id);
        let cfg_path = run_paths.run_dir.join("runner-config.json");
        let raw = std::fs::read_to_string(&cfg_path)
            .expect("spawn_background must have written runner-config.json before spawning hop 1");
        let cfg: crate::background::runner_main::RunnerConfig =
            serde_json::from_str(&raw).expect("runner-config.json must deserialize");
        let RunnerStep::SingleStep(step) = &cfg.steps[0] else {
            panic!("a single-agent background run must produce exactly one SingleStep, got: {:?}", cfg.steps[0]);
        };
        assert_eq!(
            step.model.as_ref().map(cyrup_core::ModelId::as_str),
            Some("anthropic/claude-override-test"),
            "the per-call model override must reach the background single run's step, not be \
             silently dropped in favor of the persona's own model"
        );
    }

    /// Regression (pi `restoreActiveJobs`, `async-job-tracker.ts:405-420`): resuming tracking from
    /// disk must (a) skip any run whose RECONCILED state is already terminal (`complete`/`failed`/
    /// `paused`) — pi's own `listAsyncRuns({ states: ["queued", "running"] })` filter — and (b) seed
    /// each restored run's `events.jsonl` byte cursor at the file's CURRENT size (pi's
    /// `restoredControlEventCursor`), not `0`. Pre-fix, `resume_tracking` re-tracked EVERY
    /// subdirectory unconditionally (including an already-`Complete` run) and always seeded the
    /// cursor at `0`, which would cause a restored job's entire historical `events.jsonl` to be
    /// re-tailed the next poll tick.
    #[tokio::test]
    async fn resume_tracking_skips_terminal_runs_and_seeds_the_events_cursor_at_eof() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());

        // A still-running run: real live pid (this test process itself) so reconciliation leaves
        // it Running, plus a non-empty events.jsonl whose EXISTING bytes must never be re-tailed.
        let running_run_id = RunId::from_token("run-running");
        let running_paths = RunPaths::for_run(&async_root, &results_dir, &running_run_id);
        tokio::fs::create_dir_all(&running_paths.run_dir)
            .await
            .expect("mkdir running run_dir");
        let mut running_status = crate::background::RunStatus::queued(
            running_run_id.clone(),
            RunMode::Single,
            Some(std::process::id()),
        );
        running_status.advance_state(RunState::Running).expect("Queued -> Running");
        write_atomic_json(&running_paths.status, &running_status)
            .await
            .expect("write running status fixture");
        let events_content = b"{\"kind\":\"a\"}\n{\"kind\":\"b\"}\n";
        tokio::fs::write(&running_paths.events, events_content)
            .await
            .expect("seed events.jsonl for the running run");

        // A run that already finished before this process started: must NOT be re-tracked at all.
        let complete_run_id = RunId::from_token("run-complete");
        let complete_paths = RunPaths::for_run(&async_root, &results_dir, &complete_run_id);
        tokio::fs::create_dir_all(&complete_paths.run_dir)
            .await
            .expect("mkdir complete run_dir");
        let mut complete_status = crate::background::RunStatus::queued(
            complete_run_id.clone(),
            RunMode::Single,
            Some(1),
        );
        complete_status.state = RunState::Complete;
        write_atomic_json(&complete_paths.status, &complete_status)
            .await
            .expect("write complete status fixture");

        executor.resume_tracking(dir.path()).await;

        assert_eq!(
            executor.tracker().tracked_count(),
            1,
            "only the queued/running run may be restored — a terminal run must be skipped entirely"
        );
        assert!(
            executor.tracker().get(&complete_run_id).is_none(),
            "an already-terminal run must never be re-tracked by resume_tracking"
        );
        let restored = executor
            .tracker()
            .get(&running_run_id)
            .expect("the still-running run must be restored");
        assert_eq!(
            restored.events_cursor,
            events_content.len() as u64,
            "the restored job's events cursor must be seeded at the file's CURRENT size (EOF), \
             never 0, so historical control events are never re-tailed"
        );
    }

    /// pi `executeAsyncChain`/`executeAsyncSingle` (`async-execution.ts:585-589,650,672-678,717-750`
    /// / `826-830,895,914-920,935-967`): a background run started from WITHIN an already-nested run
    /// reroutes its storage under the inherited root's `nested-subagent-runs`/`nested` subtree,
    /// instead of the ordinary per-`cwd` shared async/results roots — otherwise it is
    /// indistinguishable from a top-level run and invisible to the root's own nested registry.
    /// Before this fix, `spawn_background_steps` had no nested-route awareness at all and
    /// unconditionally called `run_artifact_roots(cwd)` (this test's `resolve_background_storage_roots`
    /// callee did not exist pre-fix, so a nested route could never reroute anything).
    #[test]
    fn resolve_background_storage_roots_reroutes_under_the_inherited_nested_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let route = crate::spawn::nested_events::create_nested_route("root-parity-test-async-exec")
            .expect("create_nested_route should succeed");

        let (nested_async, nested_results) =
            resolve_background_storage_roots(dir.path(), Some(&route))
                .expect("nested rerouting must succeed for a valid route");
        assert!(
            nested_async.ends_with("root-parity-test-async-exec"),
            "the async root for a nested run must be keyed under the inherited route's own root \
             run id, got: {nested_async:?}"
        );
        assert!(
            nested_async.to_string_lossy().contains("nested-subagent-runs"),
            "a nested run's async root must live under the nested-subagent-runs subtree, got: \
             {nested_async:?}"
        );
        assert!(
            nested_results.to_string_lossy().contains("nested"),
            "a nested run's results dir must live under the nested results subtree, got: \
             {nested_results:?}"
        );

        let (default_async, default_results) = resolve_background_storage_roots(dir.path(), None)
            .expect("the non-nested default derivation must still succeed");
        assert_eq!(default_async, default_async_root(dir.path()));
        assert_eq!(default_results, default_results_dir(dir.path()));
        assert_ne!(
            nested_async, default_async,
            "a nested run must never land in the same shared per-cwd async root as a top-level run"
        );

        // Best-effort cleanup of the route directory this test created under the real temp root.
        if let Some(parent) = route.event_sink.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    /// pi `formatAsyncStartedMessage` (`async-execution.ts:200-208`): the mode-specific headline
    /// followed by the fixed 4-line detached-run guidance, `"\n"`-joined verbatim. Before this fix,
    /// an async-start tool result was the single flat sentence "Background subagent run started:
    /// {run_id}. Use the status/interrupt management actions to check on it later; do not poll in a
    /// tight loop." — this exact multi-line shape did not exist.
    #[test]
    fn format_async_started_message_matches_pis_fixed_four_line_guidance() {
        let msg = format_async_started_message("Async: worker [run00001]");
        assert_eq!(
            msg,
            "Async: worker [run00001]\n\
             \n\
             The async run is detached. Do not run sleep timers or polling loops just to wait for it.\n\
             If you have independent work, continue that work. If you have nothing else to do until \
             the async result arrives, end your turn now; Pi will deliver the completion when the run \
             finishes.\n\
             Use subagent({ action: \"status\", id: \"...\" }) when you need the current status/result, \
             or to inspect a blocked/stale run. Do not poll just to wait."
        );
    }

    /// pi's `chainDesc` (`async-execution.ts:775-779`): sequential steps joined by `" -> "`, a static
    /// parallel group rendered as `[a+b]`. Before this fix, the tool's async-start headline never
    /// described the chain shape at all — `describe_chain` did not exist.
    #[test]
    fn describe_chain_joins_sequential_steps_and_brackets_parallel_groups() {
        let graph = vec![
            RunnerStep::SingleStep(fork_test_step("a")),
            RunnerStep::ParallelGroup(ParallelGroupSpec {
                steps: vec![fork_test_step("b"), fork_test_step("c")],
                concurrency: 2,
                fail_fast: false,
                worktree: false,
            }),
        ];
        assert_eq!(describe_chain(&graph), "a -> [b+c]");
    }

    /// [`SubagentExecutor::run_chain_foreground`] (the foreground `/chain`/`/parallel` walker) must
    /// reject a blocked depth ceiling before walking a single [`RunnerStep`] — proven with a
    /// non-empty graph so that, absent the guard, `walk_chain` would otherwise attempt to dispatch
    /// at least one step (and, for a real agent, spawn at least one real child process).
    #[tokio::test]
    async fn run_chain_foreground_rejects_on_depth_before_walking_any_step() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let graph = vec![RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
            agent: "worker".to_string(),
            task: "do something".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];

        let err = executor
            .run_chain_foreground(
                dir.path(),
                graph,
                BTreeMap::new(),
                String::new(),
                None,
                CancelToken::new(),
                None,
            )
            .await
            .expect_err("a blocked depth ceiling must reject before walking any step");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "got: {err:?}"
        );
    }

    /// [`SubagentExecutor::spawn_background_steps`] (the general multi-step background dispatch
    /// [`SubagentExecutor::spawn_background`] itself wraps, and `/chain`/`/parallel`'s `--bg` shape
    /// calls directly) must reject a blocked depth ceiling before creating the async-run root,
    /// results directory, or run directory — the filesystem-level proof mirrors this test's own
    /// `spawn_background`-level sibling above, applied to this lower-level entry point directly
    /// rather than through the single-task wrapper.
    #[tokio::test]
    async fn spawn_background_steps_rejects_on_depth_before_any_directory_creation() {
        let executor = SubagentExecutor::new();
        {
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let step = RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
            agent: "worker".to_string(),
            task: "do something".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        });

        let err = executor
            .spawn_background_steps(
                dir.path(),
                BackgroundStepsSpec {
                    steps: vec![step],
                    mode: RunMode::Single,
                    session_file: None,
                    resolved_agents: BTreeMap::new(),
                    original_task: String::new(),
                    chain_dir: None,
                },
            )
            .await
            .expect_err("a blocked depth ceiling must reject before any directory creation");
        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "got: {err:?}"
        );
        assert!(!default_async_root(dir.path()).exists());
        assert!(!default_results_dir(dir.path()).exists());
    }

    /// R-SA-055 (SAFETY-CRITICAL), end to end through the full slash-command dispatch path:
    /// `/run-chain` must reject on a blocked depth ceiling BEFORE `resolve_chain`'s own real
    /// discovery filesystem scan ever runs. Proven the same "same unresolvable name, which error
    /// wins" way as the foreground/background tests above — no chain named `"ghost-chain"` is
    /// ever written to `dir`, so if the depth guard did NOT run first, this call would surface
    /// [`SubagentError::ChainNotFound`] (discovery's own genuine failure mode for an unresolvable
    /// name) instead of [`SubagentError::DepthExceeded`].
    #[tokio::test]
    async fn dispatch_slash_run_chain_rejects_on_depth_before_chain_discovery_ever_runs() {
        let cfg = SubagentExtensionConfig {
            max_subagent_depth: 0,
            ..SubagentExtensionConfig::default()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());

        let err = ext
            .dispatch_slash(
                SlashCommandName::RunChain,
                "ghost-chain -- do something",
                dir.path(),
            )
            .await
            .expect_err("a blocked depth ceiling must reject before chain discovery runs");

        assert!(
            matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
            "expected DepthExceeded ahead of resolve_chain's own ChainNotFound, got: {err:?}"
        );
    }

    /// The `/chain` and `/parallel` shared tail ([`SubagentsExtension::run_or_background_chain`])
    /// must likewise reject on a blocked depth ceiling before its own fork-context resolution (and
    /// therefore before either `run_chain_foreground`'s or `spawn_background_steps`' own
    /// independent, necessarily-later re-check) — proven directly against that private tail
    /// (accessible from this same-file `tests` submodule) with both `background: false` and
    /// `background: true`, since both branches share the identical guard at the top of the
    /// function, before the `if background` split.
    #[tokio::test]
    async fn run_or_background_chain_rejects_on_depth_before_fork_context_resolution() {
        let cfg = SubagentExtensionConfig {
            max_subagent_depth: 0,
            ..SubagentExtensionConfig::default()
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let ext = SubagentsExtension::with_config_and_cwd(cfg, dir.path().to_path_buf());

        let graph = vec![RunnerStep::SingleStep(crate::spawn::chain_graph::SingleStepSpec {
            agent: "worker".to_string(),
            task: "do something".to_string(),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        })];

        for background in [false, true] {
            let err = ext
                .run_or_background_chain(
                    dir.path(),
                    graph.clone(),
                    RunMode::Chain,
                    Some(ContextMode::Fresh),
                    background,
                    None,
                )
                .await
                .expect_err("a blocked depth ceiling must reject before any dispatch");
            assert!(
                matches!(err, SubagentError::DepthExceeded { current: 0, max: 0 }),
                "background={background}: expected DepthExceeded, got: {err:?}"
            );
        }
    }

    // ---------------------------------------------------------------------------------------
    // `/subagent-cost` walks the SESSION TRANSCRIPT (pi `buildSubagentCostReport`,
    // slash-commands.ts:289-328), not a background status file: this drives the REAL production
    // command path (`SubagentExecutor::run_cost_report` -> `cost_report_from_sessions_dir`) end to
    // end over a real on-disk session (created + appended via `cyrup_session::SessionManager`,
    // reloaded via `SessionManager::open`), proving the command sums the parent's own assistant
    // usage plus every subagent child's usage from the transcript. (The recursive nested-run
    // accumulator `registration::cost::compute_recursive_cost` remains a separate capability with
    // its own exhaustive unit tests in that module.)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn run_cost_report_walks_the_session_transcript() {
        let tmp = tempfile::tempdir().expect("real tempdir");
        let cwd = tmp.path().join("project");
        std::fs::create_dir_all(&cwd).expect("mkdir cwd");

        // A real, persisted session: one parent assistant turn (usage 200/100, $0.02) + one subagent
        // toolResult carrying a child result (usage 50/25, $0.005). `SessionManager::create` +
        // `append_message` write these to disk exactly as a live run would.
        let layout = cyrup_session::SessionLayout::new(tmp.path().join("sessions"), cwd.clone());
        let mut manager = cyrup_session::SessionManager::create(
            &cwd,
            &layout,
            cyrup_session::NewSessionOpts::default(),
        )
        .expect("create session");

        let user: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "go" }],
            "timestamp": 0,
        }))
        .expect("user message");
        manager.append_message(user).expect("append user");

        let assistant: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "text", "text": "ok" }],
            "provider": "anthropic",
            "model": "claude-sonnet-4",
            "usage": {
                "input": 200, "output": 100, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 300,
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.02 },
            },
            "stopReason": "stop",
            "timestamp": 1,
        }))
        .expect("assistant message");
        manager.append_message(assistant).expect("append assistant");

        let tool_result: cyrup_core::Message = serde_json::from_value(serde_json::json!({
            "role": "toolResult",
            "toolCallId": "call-1",
            "toolName": "subagent",
            "content": [{ "type": "text", "text": "done" }],
            "details": {
                "mode": "single",
                "results": [{
                    "agent": "worker",
                    "usage": {
                        "input": 50, "output": 25, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 75,
                        "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.005 },
                    },
                }],
            },
            "timestamp": 2,
        }))
        .expect("tool result message");
        manager.append_message(tool_result).expect("append tool result");

        let executor = SubagentExecutor::new();
        let report = executor.cost_report_from_sessions_dir(&layout.dir()).await;

        assert!(report.starts_with("Subagent cost\n"), "{report}");
        assert!(report.contains("Parent: ↑200 ↓100"), "parent assistant usage: {report}");
        assert!(report.contains("Child 1 (worker)"), "per-child breakdown: {report}");
        assert!(report.contains("Children: ↑50 ↓25"), "child subtotal: {report}");
        // Parent (200/100) + child (50/25) summed into the grand Total (250/125), with cost summed.
        assert!(report.contains("Total: ↑250 ↓125"), "parent+child total: {report}");
        assert!(report.contains("$0.0250"), "total cost sums parent+child: {report}");
    }

    // ---------------------------------------------------------------------------------------
    // `/subagents-models` reports the RUNTIME builtin-agent -> model mapping (pi `handleModels`),
    // NOT the static provider catalog. Env-agnostic: asserts the mapping header/shape and the
    // unknown-builtin rejection, which hold whether or not the bundled builtins resolve in this
    // ambient test environment.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_models_report_renders_runtime_mapping_and_rejects_unknown_builtin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();

        let full = executor.run_models_report(dir.path(), None);
        assert!(
            full.starts_with("Builtin subagent models\n"),
            "the report must be the runtime builtin->model mapping, not a catalog dump: {full}"
        );
        assert!(full.contains("Current session model:"), "{full}");
        // The old behavior dumped the static provider catalog ("... — context {n}k, reasoning=...");
        // the runtime mapping must not.
        assert!(
            !full.contains("reasoning="),
            "must not dump the static provider catalog: {full}"
        );

        let unknown = executor.run_models_report(dir.path(), Some("definitely-not-a-builtin"));
        assert!(
            unknown
                .contains("Builtin agent 'definitely-not-a-builtin' not found. Available:"),
            "an unknown builtin name must be rejected with the available list: {unknown}"
        );
    }

    /// A minimal [`cyrup_ext::host::HostServices`] double that reports only a canned current model
    /// (every other capability keeps the trait's deny/None default) — the analog of
    /// `cyrup-session-svc`'s `LiveHostServices` for proving the subagent session-model inheritance
    /// seam reads `HostServices::current_model` without a real live session.
    struct FixedModelHost(Option<String>);
    impl cyrup_ext::host::HostServices for FixedModelHost {
        fn current_model(&self) -> Option<String> {
            self.0.clone()
        }
    }

    #[test]
    fn inherited_session_model_reads_the_live_host_and_report_renders_it() {
        // (a)/(d) at the executor seam: with NO host bound the inheritance degrades to `None` and the
        // report shows `(unavailable)` exactly as before; once a host reporting model X is bound,
        // `inherited_session_model()` returns X (pi `ctx.model`) and `/subagents-models` renders X on
        // the `Current session model` line.
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();

        // No host bound (headless / SDK-embedder default): genuine no-host degrade.
        assert!(executor.inherited_session_model().is_none());
        assert!(
            executor
                .run_models_report(dir.path(), None)
                .contains("Current session model:\n  (unavailable)"),
            "no live host must degrade to (unavailable)"
        );

        // Bind a live host reporting the parent session model.
        executor.set_host_services(Arc::new(FixedModelHost(Some(
            "together/zai-org/GLM-5.2".to_string(),
        ))));
        assert_eq!(
            executor.inherited_session_model(),
            Some(ModelId::from("together/zai-org/GLM-5.2")),
            "inherited_session_model must read HostServices::current_model as a provider/id ModelId"
        );
        let report = executor.run_models_report(dir.path(), None);
        assert!(
            report.contains("Current session model:\n  together/zai-org/GLM-5.2"),
            "the live inherited model must render on the report instead of (unavailable): {report}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // pi-parity regressions (agent-management.ts `handleModels`/`formatModelSource`): the
    // "(unresolved)" placeholder (not a bespoke "inherits current session model" string), the
    // `"inherit"` sentinel resolving through the parent model instead of printing verbatim, the
    // "Requested model setting" block, and `formatModelSource`'s model-equality / scope gates.
    // ---------------------------------------------------------------------------------------

    #[test]
    fn run_models_report_uses_pi_unresolved_placeholder_not_inherits_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // No host bound (no live session model) and `delegate.md` declares no `model:` of its own,
        // so pi's `resolveSubagentModelOverride` has nothing to resolve to and `handleModels`
        // renders its exact `"(unresolved)"` placeholder (agent-management.ts:591) — pre-fix this
        // crate rendered the bespoke, non-pi "(inherits current session model)" text instead.
        let report = executor.run_models_report(dir.path(), Some("delegate"));
        assert!(
            report.contains("Effective model:\n  (unresolved)"),
            "must render pi's exact '(unresolved)' placeholder when nothing can be resolved: {report}"
        );
        assert!(
            !report.contains("inherits current session model"),
            "must not render the bespoke non-pi placeholder text: {report}"
        );
    }

    #[test]
    fn run_models_report_resolves_inherit_sentinel_to_parent_model_not_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        // A settings override that explicitly requests pi's `"inherit"` sentinel — same request as
        // leaving `model` unset, per `resolveSubagentModelOverride` (model-fallback.ts:47-59).
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"subagents":{"agentOverrides":{"delegate":{"model":"inherit"}}}}"#,
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedModelHost(Some(
            "openai/gpt-5-test".to_string(),
        ))));

        let report = executor.run_models_report(dir.path(), Some("delegate"));
        assert!(
            report.contains("Effective model:\n  openai/gpt-5-test"),
            "a literal 'inherit' model setting must resolve through the live parent session \
             model, not print verbatim: {report}"
        );
        assert!(
            !report.contains("Effective model:\n  inherit"),
            "must not render the raw 'inherit' sentinel literally: {report}"
        );
        assert!(
            report.contains("Requested model setting:\n  inherit"),
            "the raw declared setting must still be surfaced once it differs from the resolved \
             model (agent-management.ts:596-599): {report}"
        );
    }

    #[test]
    fn run_models_report_gates_override_provenance_on_actual_model_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        // This override only ever touches `disabled` — it never applies to `model` — so the
        // resolved model provenance must NOT claim an "override" changed it (pi
        // `agent.override && agent.model !== agent.override.base.model`, agent-management.ts:566-568).
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#,
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        let report = executor.run_models_report(dir.path(), Some("scout"));
        assert!(
            !report.contains("Source: project override"),
            "an override that never touched `model` must not claim '{{scope}} override' \
             provenance for the model: {report}"
        );
        assert!(
            report.contains("Source: inherit requested, but no current session model is available"),
            "with no model configured anywhere and no live session model, pi's unresolved-\
             fallback text must still apply: {report}"
        );
    }

    #[test]
    fn run_models_report_scopes_default_model_provenance_by_settings_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        std::fs::write(
            settings_dir.join("settings.json"),
            r#"{"subagents":{"defaultModel":"acme/shared-default"}}"#,
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        let report = executor.run_models_report(dir.path(), Some("scout"));
        // pi `agent.modelSource.type === "subagents.defaultModel"` renders `${scope} defaultModel`
        // (agent-management.ts:569-571); the pre-fix text hardcoded the unscoped "settings
        // defaultModel" regardless of which settings scope actually supplied the default.
        assert!(
            report.contains("Source: project defaultModel"),
            "a project-scope subagents.defaultModel must render scope-qualified provenance, not \
             the bespoke unscoped 'settings defaultModel' text: {report}"
        );
        assert!(
            !report.contains("settings defaultModel"),
            "must not render the bespoke unscoped provenance text: {report}"
        );
    }

    /// PROV-007: `/subagents-models` resolves a BARE model id against the real built-in model
    /// registry (pi `resolveModelCandidate` over `ctx.modelRegistry.getAvailable()`,
    /// model-fallback.ts:60-76), so a persona configured with a bare id from ANY registered
    /// provider renders its `provider/id`. The retired 2-model seed stub could only ever resolve
    /// `claude-sonnet-4-5`/`gpt-4o`; every other bare id fell through pi's "no match" fallback and
    /// rendered verbatim.
    ///
    /// The subject model is picked from the registry itself — the first model whose bare id is
    /// registry-unique and whose provider is neither `anthropic` nor `openai` — so a catalog
    /// refresh cannot rot this test.
    #[test]
    fn run_models_report_resolves_a_bare_id_from_any_registered_provider() {
        // Read the fixture straight from `cyrup-provider` (NOT through `registry_models`) so this
        // test fails on the RENDERED REPORT when the binding regresses, not on fixture selection.
        let registry = cyrup_provider::catalog::builtin_catalog();
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for m in registry {
            *counts.entry(m.id.as_str()).or_default() += 1;
        }
        let subject = registry
            .iter()
            .find(|m| {
                counts.get(m.id.as_str()).copied() == Some(1)
                    && !matches!(m.provider.as_str(), "anthropic" | "openai")
            })
            .expect("the registry must carry a unique bare id outside anthropic/openai");
        let bare = subject.id.as_str();
        let full = format!("{}/{}", subject.provider.as_str(), bare);

        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        std::fs::write(
            settings_dir.join("settings.json"),
            format!(r#"{{"subagents":{{"defaultModel":"{bare}"}}}}"#),
        )
        .expect("write settings.json");

        let executor = SubagentExecutor::new();
        let report = executor.run_models_report(dir.path(), Some("scout"));
        assert!(
            report.contains(&format!("Effective model:\n  {full}")),
            "a bare id from '{}' must resolve to its full provider/id against the real registry: \
             {report}",
            subject.provider.as_str()
        );
        // pi surfaces the raw declared setting once it differs from the resolved model
        // (agent-management.ts:596-599) — proof the resolution really happened.
        assert!(
            report.contains(&format!("Requested model setting:\n  {bare}")),
            "the raw bare id must still be surfaced alongside the resolved full id: {report}"
        );
    }

    // NOTE: `teardown_session_stops_the_tracker_and_clears_the_parent_session_anchor` (and its
    // `FixedSessionHost` double) moved to `tests/cyrup_home_env_sandboxed_tests.rs` — see that
    // file's module doc; it needs the `CYRUP_HOME` env-var sandbox that requires `unsafe`, which
    // this crate's `#![forbid(unsafe_code)]` `src/lib.rs` disallows in-crate.

    // ---------------------------------------------------------------------------------------
    // `run_doctor` parity regressions (pi `buildDoctorReport`/`formatConfiguredSessionDir`,
    // doctor.ts:108-128; caller `subagent-executor.ts:2801-2840`)
    // ---------------------------------------------------------------------------------------

    /// pi `formatConfiguredSessionDir` (doctor.ts:108-116): a per-call `sessionDir` wins over the
    /// configured `default_session_dir`, which wins over the literal `"not configured"`. Pre-fix,
    /// `run_doctor` always rendered the always-on computed `<home>/.cyrup/sessions/<cwd_key>`
    /// directory here regardless of either input, and `"not configured"` was unreachable — this
    /// test fails against that behavior on all three branches.
    #[test]
    fn format_configured_session_dir_prefers_requested_then_default_then_not_configured() {
        assert_eq!(
            format_configured_session_dir(Some("/abs/requested"), Some(Path::new("/abs/default"))),
            "/abs/requested",
            "an explicit per-call sessionDir must win over the configured default"
        );
        assert_eq!(
            format_configured_session_dir(None, Some(Path::new("/abs/default"))),
            "/abs/default",
            "with no per-call override, the configured default_session_dir must be used"
        );
        assert_eq!(
            format_configured_session_dir(None, None),
            "not configured",
            "with neither a per-call override nor a configured default, pi's literal \
             \"not configured\" must be reachable"
        );
        // An empty-string override is JS-falsy in pi (`if (input.requestedSessionDir)`) and must
        // fall through exactly like an absent one.
        assert_eq!(
            format_configured_session_dir(Some(""), Some(Path::new("/abs/default"))),
            "/abs/default"
        );
    }

    /// pi `expandTilde` (`extension/index.ts:86-88`) composed with `path.resolve`: a leading `~/`
    /// expands against the home directory before being resolved to an absolute path.
    #[test]
    fn format_configured_session_dir_expands_a_leading_tilde() {
        let rendered = format_configured_session_dir(Some("~/my-sessions"), None);
        let expected = dirs_home().join("my-sessions");
        assert_eq!(rendered, expected.display().to_string());
    }

    /// Divergence regression: pre-fix, `run_doctor` never consulted `params.sessionDir` at all —
    /// the report's `- configured session dir:` line was always the hardcoded computed sessions
    /// directory. This drives the REAL `SubagentExecutor::run_doctor` (not just the pure formatter)
    /// end to end and fails against that pre-fix behavior.
    #[tokio::test]
    async fn run_doctor_report_honors_a_per_call_session_dir_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();

        let report = executor.run_doctor(dir.path(), Some("/abs/custom-sessions")).await;
        assert!(
            report.contains("- configured session dir: /abs/custom-sessions"),
            "an explicit per-call sessionDir must be rendered verbatim (resolved): {report}"
        );

        let report_default = executor.run_doctor(dir.path(), None).await;
        assert!(
            report_default.contains("- configured session dir: not configured"),
            "with no per-call override and no configured default_session_dir, pi's literal \
             \"not configured\" must render, not the always-on computed sessions dir: \
             {report_default}"
        );

        {
            let mut cfg = executor.config.lock().await;
            cfg.default_session_dir = Some(PathBuf::from("/abs/configured-default"));
        }
        let report_configured_default = executor.run_doctor(dir.path(), None).await;
        assert!(
            report_configured_default
                .contains("- configured session dir: /abs/configured-default"),
            "the extension's own configured default_session_dir must be consulted when no \
             per-call override is present: {report_configured_default}"
        );
    }

    /// A minimal [`cyrup_ext::host::HostServices`] double reporting a canned live session id/file —
    /// the analog of `FixedModelHost` above, for proving `run_doctor` reads the SAME live handle
    /// [`SubagentExecutor::resolve_context`] already uses (P-1) instead of a per-cwd mtime guess.
    struct FixedSessionIdHost {
        id: Option<String>,
        file: Option<PathBuf>,
    }
    impl cyrup_ext::host::HostServices for FixedSessionIdHost {
        fn session_id(&self) -> Option<String> {
            self.id.clone()
        }
        fn session_file(&self) -> Option<PathBuf> {
            self.file.clone()
        }
    }

    /// Divergence regression: pre-fix, `run_doctor` unconditionally scanned the per-cwd sessions
    /// directory for the newest `.jsonl` by mtime and ignored any bound live session manager
    /// entirely. With NO on-disk session file under this fresh temp cwd but a bound live host
    /// reporting a session id/file, the pre-fix behavior renders "not available" for both — this
    /// test fails against that.
    #[tokio::test]
    async fn run_doctor_prefers_the_live_session_manager_over_an_mtime_scan() {
        let dir = tempfile::tempdir().expect("tempdir"); // no sessions dir, no .jsonl on disk at all
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some("live-session-id".to_string()),
            file: Some(PathBuf::from("/tmp/live-session.jsonl")),
        }));

        let report = executor.run_doctor(dir.path(), None).await;
        assert!(
            report.contains("- current session id: live-session-id"),
            "the live host's session id must be reported, not a disk-scan miss: {report}"
        );
        assert!(
            report.contains("- current session file: /tmp/live-session.jsonl"),
            "the live host's session file must be reported, not a disk-scan miss: {report}"
        );
    }

    /// pi's two-level fallback (doctor.ts:124: `currentSessionId ?? state.currentSessionId ??
    /// "not available"`): when the live host reports NO session id (but a session was captured
    /// earlier at this orchestrator's own `SessionStart`, `root_parent_session`), the cached id
    /// must be used rather than falling straight to "not available".
    #[tokio::test]
    async fn run_doctor_falls_back_to_the_cached_root_parent_session_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // A live host IS bound (so the mtime-scan fallback branch is not taken at all) but reports
        // NO session id (e.g. an unpersisted/ephemeral session) — exercises the
        // `services.session_id().or(cached_id)` fallback arm specifically.
        executor.set_host_services(Arc::new(FixedSessionIdHost { id: None, file: None }));
        // Directly seed the state-held id pi's `state.currentSessionId` plays — in production this
        // is populated once at THIS orchestrator's own `SessionStart` via
        // `capture_parent_session_anchor` (same live `session_id()` call, just captured earlier).
        *executor
            .root_parent_session
            .lock()
            .expect("root_parent_session mutex") = Some("root-session-id".to_string());

        let report = executor.run_doctor(dir.path(), None).await;
        assert!(
            report.contains("- current session id: root-session-id"),
            "must fall back to the cached SessionStart id when the live host reports none: {report}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // C5 control-action dispatch smoke tests (executor glue; read-only, no spawn, no home writes)
    //
    // These drive the real `SubagentExecutor::control_*` methods over a fresh temp cwd whose async
    // root has never been created, so every path is a pure read that returns the expected empty /
    // not-found rendering without spawning any process or touching the user's `~/.cyrup` tree. The
    // full per-run rendering + primitive behavior is covered by `background::run_status`'s own tests
    // against explicit temp roots.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn control_status_no_id_over_a_fresh_cwd_lists_no_active_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let text = executor
            .control_status(dir.path(), None, None, false)
            .await
            .expect("status list is Ok even with no runs");
        assert_eq!(text, "No active async runs.");
    }

    #[tokio::test]
    async fn control_status_unknown_id_is_the_not_found_notice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_status(dir.path(), Some("deadbeef0000"), None, false)
            .await
            .expect_err("an unknown id is a not-found error");
        assert_eq!(err, "Async run not found. Provide id or dir.");
    }

    /// pi `run-status.ts:104-110`: the child-safe fanout tool (`deps.nested` truthy) hard-errors on
    /// a no-id status call instead of listing the cwd's active runs — a fanout child has no
    /// business enumerating its parent's whole async root. Regression proof: pre-fix,
    /// `control_status` had no `child_safe` parameter at all and always fell through to
    /// `list_active_runs`, which would have made this assert `Ok("No active async runs.")` instead.
    #[tokio::test]
    async fn control_status_child_safe_no_id_hard_errors_instead_of_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_status(dir.path(), None, None, true)
            .await
            .expect_err("child-safe no-id status must hard-error, not list runs");
        assert_eq!(
            err,
            "Child-safe subagent status requires an id when no foreground run is active."
        );
    }

    #[tokio::test]
    async fn control_interrupt_with_no_run_reports_none_capable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_interrupt(dir.path(), None)
            .await
            .expect_err("no runs -> no interrupt-capable run");
        assert_eq!(err, "No interrupt-capable run found in this session.");
    }

    #[tokio::test]
    async fn control_resume_requires_a_message_then_an_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // Empty follow-up is rejected before anything else (pi `resume` requires `message`).
        let no_msg = executor
            .control_resume(dir.path(), Some("run00000000"), None, None, None)
            .await
            .expect_err("resume requires a message");
        assert_eq!(no_msg, "action='resume' requires message.");
        // With a message but no id, resume requires an id selector.
        let no_id = executor
            .control_resume(dir.path(), None, Some("carry on"), None, None)
            .await
            .expect_err("resume requires an id");
        assert_eq!(no_id, "action='resume' requires id.");
    }

    /// pi's `SteerRunning` delivered-follow-up confirmation (`subagent-executor.ts:846-871`): the
    /// header sent over the broker MUST include the resolved agent name (`Follow-up for async run
    /// ${runId} (${agent}):`), not just the run id. Proven with a REAL on-disk running-run fixture
    /// (not mocked) and a fake, always-delivers `SteerChannel` that records exactly what it received.
    #[tokio::test]
    async fn control_resume_steer_running_follow_up_header_includes_the_agent_name() {
        struct RecordingSteerChannel {
            received: std::sync::Mutex<Vec<(String, String)>>,
        }
        impl crate::tui::intercom::SteerChannel for RecordingSteerChannel {
            fn steer(
                &self,
                target: String,
                text: String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
            {
                self.received.lock().expect("lock").push((target, text));
                Box::pin(async { Ok(true) })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let run_id = RunId::from_token("run00042");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        // A genuinely live run always carries a real runner pid (`RunStatus::pid`'s own doc: "in
        // practice this is `Some` from the very first write") — the SteerRunning arm's own
        // interrupt-precondition check (pi `interruptLiveAsyncResumeTarget`,
        // `background/async-resume.ts:53-56`) now requires exactly that before it will even
        // attempt to interrupt, so this fixture must supply one to exercise the delivery path
        // rather than the "no interrupt-capable runner pid was found" abort.
        let mut status =
            crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, Some(4242));
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("researcher");
        step.status = crate::background::StepState::Running;
        status.steps = vec![step];
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write running status fixture");

        let steer = Arc::new(RecordingSteerChannel {
            received: std::sync::Mutex::new(Vec::new()),
        });
        let executor = SubagentExecutor::new().with_channels(
            Arc::new(crate::tui::intercom::NoTransportChannel),
            Arc::new(crate::tui::intercom::NoOpClarifyChannel),
            steer.clone(),
        );

        let confirmation = executor
            .control_resume(dir.path(), Some("run00042"), Some("carry on"), None, None)
            .await
            .expect("a running child with a delivering steer channel resumes via live steer");
        assert!(
            confirmation.starts_with("Interrupted live async child, then delivered follow-up."),
            "got: {confirmation}"
        );

        let received = steer.received.lock().expect("lock");
        assert_eq!(received.len(), 1, "the follow-up must be delivered exactly once");
        assert!(
            received[0].1.starts_with("Follow-up for async run run00042 (researcher):\n\n"),
            "the follow-up header must include the resolved agent name, got: {:?}",
            received[0].1
        );
    }

    /// pi's `deliverSubagentIntercomMessageEvent` bounds EVERY caller — including this live-child
    /// follow-up steer (`subagent-executor.ts:860`) — to a 500ms default timeout race
    /// (`result-intercom.ts:283-316`): the caller's own turn is never blocked longer than that
    /// waiting on a delivery ack. Proven with a `SteerChannel` whose `steer` never resolves at all
    /// (the real-world shape of "no receiver ever answers"): pre-fix, `control_resume` awaited the
    /// raw `SteerChannel::steer` future directly with no outer race, so this would hang forever;
    /// post-fix it must resolve to the "not registered" fallback within a small bounded multiple of
    /// [`crate::tui::intercom::DEFAULT_STEER_TIMEOUT`] (500ms).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn control_resume_steer_running_degrades_within_the_bounded_timeout_when_steer_never_resolves() {
        struct HangingSteerChannel;
        impl crate::tui::intercom::SteerChannel for HangingSteerChannel {
            fn steer(
                &self,
                _target: String,
                _text: String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
            {
                Box::pin(std::future::pending())
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let run_id = RunId::from_token("run00099");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        // A genuinely live run always carries a real runner pid (`RunStatus::pid`'s own doc: "in
        // practice this is `Some` from the very first write") — the SteerRunning arm's own
        // interrupt-precondition check (pi `interruptLiveAsyncResumeTarget`,
        // `background/async-resume.ts:53-56`) now requires exactly that before it will even
        // attempt to interrupt, so this fixture must supply one to exercise the delivery path
        // rather than the "no interrupt-capable runner pid was found" abort.
        let mut status =
            crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, Some(4242));
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("researcher");
        step.status = crate::background::StepState::Running;
        status.steps = vec![step];
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write running status fixture");

        let executor = SubagentExecutor::new().with_channels(
            Arc::new(crate::tui::intercom::NoTransportChannel),
            Arc::new(crate::tui::intercom::NoOpClarifyChannel),
            Arc::new(HangingSteerChannel),
        );

        let started = std::time::Instant::now();
        // Wrapped in an explicit, generous outer bound so a regression back to the pre-fix
        // unbounded-await behavior fails this test with a clear message instead of hanging the
        // whole suite indefinitely.
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            executor.control_resume(dir.path(), Some("run00099"), Some("carry on"), None, None),
        )
        .await
        .expect(
            "control_resume must resolve well within 5s even when the steer channel never \
             resolves — pre-fix this awaited the raw SteerChannel future with no outer race and \
             would hang forever",
        );
        let elapsed = started.elapsed();

        // A steer that never resolves must degrade to the documented "not registered" fallback —
        // never hang the caller's own turn indefinitely.
        let err = outcome.expect_err("an undelivered steer must degrade to the not-registered fallback");
        assert!(
            err.starts_with("Async child appears live but its intercom target is not registered."),
            "got: {err}"
        );
        assert!(
            elapsed < crate::tui::intercom::DEFAULT_STEER_TIMEOUT * 5,
            "must not block the caller's turn far past the documented 500ms steer timeout bound, \
             got: {elapsed:?}"
        );
    }

    /// pi `interruptLiveAsyncResumeTarget` (`background/async-resume.ts:53-56`): before EVER
    /// attempting to interrupt (or delivering any follow-up), `resume`'s live-steer arm must
    /// re-reconcile and REQUIRE `status.state === "running"` with a numeric pid — a run whose
    /// overall state claims `Running` but carries no known runner pid must abort the WHOLE resume
    /// with pi's exact diagnostic, never silently fall through to "steering" a child that was never
    /// confirmed interruptible. Pre-fix, `control_resume`'s `SteerRunning` arm discarded
    /// `control::interrupt`'s own `Ok(NotRunning)`-shaped outcomes (and, indirectly, a pid-less
    /// status) and proceeded straight to an intercom-delivery attempt regardless.
    #[tokio::test]
    async fn control_resume_steer_running_requires_a_running_status_with_a_known_pid() {
        struct RecordingSteerChannel {
            received: std::sync::Mutex<Vec<(String, String)>>,
        }
        impl crate::tui::intercom::SteerChannel for RecordingSteerChannel {
            fn steer(
                &self,
                target: String,
                text: String,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, String>> + Send + '_>>
            {
                self.received.lock().expect("lock").push((target, text));
                Box::pin(async { Ok(true) })
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = default_async_root(dir.path());
        let results_dir = default_results_dir(dir.path());
        let run_id = RunId::from_token("run0nopid");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");
        // `Running` overall state, but NO known runner pid (`pid: None`) — pi's guard treats this
        // identically to "no interrupt-capable runner pid was found", never as a steerable child.
        let mut status = crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, None);
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("researcher");
        step.status = crate::background::StepState::Running;
        status.steps = vec![step];
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write pid-less running status fixture");

        let steer = Arc::new(RecordingSteerChannel {
            received: std::sync::Mutex::new(Vec::new()),
        });
        let executor = SubagentExecutor::new().with_channels(
            Arc::new(crate::tui::intercom::NoTransportChannel),
            Arc::new(crate::tui::intercom::NoOpClarifyChannel),
            steer.clone(),
        );

        let err = executor
            .control_resume(dir.path(), Some("run0nopid"), Some("carry on"), None, None)
            .await
            .expect_err("a Running status with no known pid must abort the resume outright");
        assert_eq!(
            err,
            "Async run run0nopid is live but no interrupt-capable runner pid was found."
        );
        assert!(
            steer.received.lock().expect("lock").is_empty(),
            "no follow-up may ever be delivered when the interrupt precondition itself was never \
             satisfied"
        );
    }

    /// pi `buildRevivedAsyncTask` (`background/async-resume.ts:378-391`): a revived child's `{task}`
    /// must be the follow-up WRAPPED in the revival framing (source run/agent/session-file context
    /// plus an explicit "you are reviving..." preamble), never the orchestrator's raw follow-up text
    /// verbatim — the revived agent otherwise has no way to know it is resuming from a stored
    /// transcript rather than starting fresh.
    #[test]
    fn build_revived_async_task_wraps_the_follow_up_in_pi_s_revival_framing() {
        let task = SubagentExecutor::build_revived_async_task(
            "run00099",
            "researcher",
            Path::new("/tmp/session-abc.jsonl"),
            "please continue",
        );
        assert_eq!(
            task,
            "You are reviving a previous subagent conversation.\n\
             \n\
             Original run: run00099\n\
             Original agent: researcher\n\
             Original session file: /tmp/session-abc.jsonl\n\
             \n\
             Use the stored session context as background. Answer the orchestrator's follow-up \
             below. Do not assume the original child process is still alive.\n\
             \n\
             Follow-up:\n\
             please continue"
        );
        assert_ne!(
            task, "please continue",
            "the revived task must NOT be the raw follow-up passed through verbatim"
        );
    }

    /// pi `target.cwd ?? requestCwd` (`subagent-executor.ts:890`, fed by `status.cwd ?? result.cwd`
    /// at `background/async-resume.ts:373`): a terminal-revival `resume` must resolve the revived
    /// child's persona against the ORIGINAL run's own cwd (persisted onto `status.json` by
    /// `finish_run`), not whatever cwd happens to be current at resume time. Proven with a custom
    /// agent defined ONLY under the original run's cwd: pre-fix, `revive_from_transcript` always
    /// discovered against the REQUEST cwd, so this agent would never be found and the call would
    /// fail with `agent not found: orig-only-agent` before ever reaching the (deliberately, via
    /// `max_subagent_depth = 0`) blocked spawn step; post-fix it must resolve the persona
    /// successfully and fail one step later, at the depth ceiling instead — proving the ORIGINAL
    /// cwd was searched. The depth block keeps this test from ever reaching a real detached process
    /// spawn.
    #[tokio::test]
    async fn control_resume_revive_prefers_the_original_runs_cwd_over_the_request_cwd() {
        let orig_dir = tempfile::tempdir().expect("orig tempdir");
        let request_dir = tempfile::tempdir().expect("request tempdir");

        let agents_dir = orig_dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir orig agents dir");
        std::fs::write(
            agents_dir.join("orig-only-agent.md"),
            "---\nname: orig-only-agent\ndescription: Only discoverable under orig_dir\n---\nBody.\n",
        )
        .expect("write orig-only-agent fixture");

        let session_file = orig_dir.path().join("session.jsonl");
        std::fs::write(&session_file, "").expect("write dummy session file");

        // The source run's storage lives under `request_dir`'s own async root (resume looks it up
        // via the REQUEST cwd, matching pi's fixed-but-here-cwd-scoped async/results roots) — only
        // the run's OWN recorded `cwd` field (set by `finish_run`) points back at `orig_dir`.
        let async_root = default_async_root(request_dir.path());
        let results_dir = default_results_dir(request_dir.path());
        let run_id = RunId::from_token("run0revive");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.expect("mkdir run_dir");

        let mut status =
            crate::background::RunStatus::queued(run_id.clone(), RunMode::Single, Some(4242));
        status.advance_state(RunState::Running).expect("Queued -> Running");
        let mut step = crate::background::StepStatus::pending("orig-only-agent");
        step.status = crate::background::StepState::Complete;
        step.session_file = Some(session_file.clone());
        status.steps = vec![step];
        status.advance_state(RunState::Complete).expect("Running -> Complete");
        status.cwd = Some(orig_dir.path().to_path_buf());
        write_atomic_json(&paths.status, &status)
            .await
            .expect("write terminal status fixture");

        let executor = SubagentExecutor::new();
        {
            // Block the spawn AFTER persona resolution succeeds, so a correct fix observably fails
            // at the depth ceiling instead of ever launching a real detached subprocess.
            let mut cfg = executor.config.lock().await;
            cfg.max_subagent_depth = 0;
        }

        let err = executor
            .control_resume(
                request_dir.path(),
                Some("run0revive"),
                Some("please continue"),
                None,
                None,
            )
            .await
            .expect_err("the blocked depth ceiling must still reject this revive");

        assert!(
            err.contains("depth limit exceeded"),
            "the revived persona must resolve against the ORIGINAL run's cwd (orig_dir), reaching \
             the depth ceiling, not fail with an agent-not-found error from searching the request \
             cwd instead; got: {err}"
        );
        assert!(
            !err.contains("agent not found"),
            "pre-fix regression: revive_from_transcript searched the REQUEST cwd (which has no \
             'orig-only-agent') instead of the original run's own cwd; got: {err}"
        );
    }

    #[tokio::test]
    async fn control_append_step_validates_shape_before_touching_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // Missing id.
        let no_id = executor
            .control_append_step(dir.path(), None, &[])
            .await
            .expect_err("append-step requires id");
        assert_eq!(no_id, "action='append-step' requires id.");
        // Wrong-cardinality chain (must be exactly one step).
        let bad_chain = executor
            .control_append_step(dir.path(), Some("run00000000"), &[])
            .await
            .expect_err("append-step requires exactly one chain step");
        assert_eq!(
            bad_chain,
            "action='append-step' requires chain with exactly one step."
        );
    }

    // =====================================================================================
    // Tier-2 (a): fork default-mode + per-index branch (`apply_fork_contexts`).
    // =====================================================================================

    fn fork_user_msg(text: &str) -> cyrup_core::Message {
        cyrup_core::Message::User {
            content: vec![cyrup_core::Content::text(text)],
            timestamp: 0,
        }
    }

    fn fork_assistant_msg(text: &str) -> cyrup_core::Message {
        cyrup_core::Message::Assistant(cyrup_core::AssistantMessage {
            content: vec![cyrup_core::Content::text(text)],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: cyrup_core::Usage::default(),
            stop_reason: cyrup_core::StopReason::Stop,
            error_message: None,
            timestamp: 0,
        })
    }

    /// Build a REAL persisted parent session (tempdir-backed on-disk JSONL, never mocked — mirrors
    /// `fork_context.rs`'s own test setup) and a [`ForkContextResolver`] over it, so `Fork` requests
    /// actually branch a genuine new session file on disk. Returns the tempdir so the caller keeps it
    /// alive for the test's duration.
    async fn persisted_fork_resolver() -> (ForkContextResolver, tempfile::TempDir) {
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = root.path().join("proj");
        let layout = cyrup_session::SessionLayout::new(root.path().to_path_buf(), cwd.clone());
        let mut parent =
            cyrup_session::SessionManager::create(&cwd, &layout, cyrup_session::NewSessionOpts::default())
                .expect("create persisted parent session");
        parent.append_message(fork_user_msg("hello")).expect("append user");
        parent
            .append_message(fork_assistant_msg("hi there"))
            .expect("append assistant");
        let manager = Arc::new(AsyncMutex::new(parent));
        let resolver = ForkContextResolver::new(manager, layout);
        (resolver, root)
    }

    fn fork_test_step(agent: &str) -> SingleStepSpec {
        SingleStepSpec {
            agent: agent.to_string(),
            task: format!("do {agent}"),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    fn persona_with_default_context(
        name: &str,
        default_context: Option<ContextMode>,
    ) -> ResolvedAgentPersona {
        ResolvedAgentPersona {
            name: name.to_string(),
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            system_prompt_body: String::new(),
            tools: None,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            output: None,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            completion_guard: None,
            max_subagent_depth: None,
            default_context,
        }
    }

    fn single_step_of(step: &RunnerStep) -> &SingleStepSpec {
        match step {
            RunnerStep::SingleStep(spec) => spec,
            other => panic!("expected a SingleStep, got {other:?}"),
        }
    }

    /// (a) An OMITTED call-site `context` (`None`) resolves EACH step to its own agent's persona
    /// `default_context` — a `fork`-defaulting agent forks, a `fresh`-defaulting agent stays fresh —
    /// rather than the pre-Tier-2 forced-`Fresh` collapse. Mirrors pi's
    /// `resolveAgentDefaultContextPolicy` (`subagent-executor.ts:1280-1293`).
    #[tokio::test]
    async fn omitted_call_site_context_falls_back_to_each_agents_persona_default() {
        let (resolver, _root) = persisted_fork_resolver().await;
        let personas: BTreeMap<String, ResolvedAgentPersona> = [
            (
                "planner".to_string(),
                persona_with_default_context("planner", Some(ContextMode::Fork)),
            ),
            (
                "scout".to_string(),
                persona_with_default_context("scout", Some(ContextMode::Fresh)),
            ),
        ]
        .into_iter()
        .collect();
        let graph = vec![
            RunnerStep::SingleStep(fork_test_step("planner")),
            RunnerStep::SingleStep(fork_test_step("scout")),
        ];

        // call_site_context = None (omitted).
        let (graph, first_session) = apply_fork_contexts(&resolver, None, &personas, graph)
            .await
            .expect("fork contexts resolve against a persisted parent");

        let planner = single_step_of(&graph[0]);
        let scout = single_step_of(&graph[1]);
        assert_eq!(
            planner.context,
            Some(ContextMode::Fork),
            "planner's persona default_context (fork) must be honored when the call site omits context"
        );
        assert!(
            planner.session_file.as_deref().is_some_and(Path::exists),
            "a fork step must receive a real, on-disk branched session file"
        );
        assert_eq!(
            scout.context,
            Some(ContextMode::Fresh),
            "scout's persona default_context (fresh) must be honored independently — one sibling's \
             default must not leak into another's"
        );
        assert!(
            scout.session_file.is_none(),
            "a fresh step must carry no branched session file"
        );
        assert_eq!(
            first_session, planner.session_file,
            "the run-level resume session is the first forking step's branch"
        );
    }

    /// (a) Two parallel fork tasks get two DISTINCT branch session files (per-index branch), not one
    /// shared branch. Mirrors pi's per-index `sessionFileForTask(agent, index)`
    /// (`preflightForkSessionsForStaticTasks`, `subagent-executor.ts:1496-1499`).
    #[tokio::test]
    async fn two_parallel_fork_tasks_get_two_distinct_branch_session_files() {
        let (resolver, _root) = persisted_fork_resolver().await;
        let personas: BTreeMap<String, ResolvedAgentPersona> = [(
            "planner".to_string(),
            persona_with_default_context("planner", None),
        )]
        .into_iter()
        .collect();
        // Two sibling forking tasks (same agent) in one parallel group; explicit call-site fork.
        let group = RunnerStep::ParallelGroup(ParallelGroupSpec {
            steps: vec![fork_test_step("planner"), fork_test_step("planner")],
            concurrency: 4,
            fail_fast: false,
            worktree: false,
        });

        let (graph, _first) =
            apply_fork_contexts(&resolver, Some(ContextMode::Fork), &personas, vec![group])
                .await
                .expect("both parallel fork tasks resolve");

        let steps = match &graph[0] {
            RunnerStep::ParallelGroup(g) => &g.steps,
            other => panic!("expected a ParallelGroup, got {other:?}"),
        };
        let first = steps[0]
            .session_file
            .clone()
            .expect("parallel task 0 forks and gets a branch");
        let second = steps[1]
            .session_file
            .clone()
            .expect("parallel task 1 forks and gets a branch");
        assert_ne!(
            first, second,
            "two parallel fork tasks must get two DISTINCT branch session files, not one shared branch"
        );
        assert!(first.exists() && second.exists(), "both branch files must exist on disk");
    }

    // =====================================================================================
    // Tier-2 (c): package-tier enumeration -> a package agent is discovered at Package scope.
    // =====================================================================================

    /// (c) A package that declares an `agents` dir (here via manifest auto-discovery of a Path-source
    /// package's conventional `agents/`) has its persona discovered at
    /// [`crate::discovery::types::AgentSource::Package`] once the installed-packages registry is
    /// enumerated into the discovery config (the wire-up [`enumerate_installed_packages`] +
    /// `discovery_config` perform for real).
    #[test]
    fn a_package_provided_agent_is_discovered_at_package_scope() {
        let home = tempfile::tempdir().expect("tempdir");
        let global_dir = home.path().join(".cyrup");
        // A real on-disk package tree with a conventional agents/ dir holding one persona.
        let pkg_root = home.path().join("code-analysis-pkg");
        let agents_dir = pkg_root.join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir package agents dir");
        std::fs::write(
            agents_dir.join("analyzer.md"),
            "---\nname: analyzer\ndescription: A package-provided analyzer agent\n---\nYou analyze code.\n",
        )
        .expect("write package agent file");

        // Persist a Global-scope, Path-source install record in the global packages.json registry —
        // exactly what `enumerate_installed_packages` loads.
        let installed = cyrup_resources::InstalledPackages {
            packages: vec![cyrup_resources::InstalledPackage {
                id: cyrup_core::PackageId::from("path:code-analysis".to_string()),
                source: cyrup_resources::PackageSource::Path {
                    path: pkg_root.clone(),
                },
                scope: cyrup_resources::InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            }],
        };
        let store = cyrup_resources::PackageStore::new(global_dir.clone(), None);
        let registry_path = store
            .registry_path(cyrup_resources::InstallScope::Global)
            .expect("global registry path");
        cyrup_resources::package::lock::save(&registry_path, &installed)
            .expect("persist packages.json");

        // The wire-up under test: enumerate the registry, then discover.
        let enumerated = enumerate_installed_packages(&global_dir, None);
        assert_eq!(
            enumerated.packages.len(),
            1,
            "the global packages.json registry must enumerate its one installed package"
        );

        let cfg = AgentDiscoveryConfig {
            builtin_agents_dir: None,
            installed_packages: enumerated,
            global_dir,
            project_root: None,
            trusted_project: false,
            ..AgentDiscoveryConfig::default()
        };
        let result = discover_agents(&cfg, None).expect("discovery succeeds");
        let analyzer = result
            .agents
            .iter()
            .find(|a| a.name == "analyzer")
            .expect("the package-provided analyzer agent must be discovered");
        assert_eq!(
            analyzer.source,
            crate::discovery::types::AgentSource::Package,
            "a package-provided agent must be discovered at Package scope"
        );
    }

    // =============================================================================================
    // "profiles" unit divergence fixes: real live-probe classification/ranking (pi
    // `probeModel`/`classifyModel`/`refreshProviderModelCatalog`/`generateProfilesForProvider`,
    // profiles.ts:250-606) + Ok-vs-Err on empty-provider paths (profiles.ts:506-508/593-595).
    // =============================================================================================

    fn test_model(
        provider: &str,
        id: &str,
        name: &str,
        cost_total: f64,
        context_window: u64,
        max_tokens: u64,
        reasoning: bool,
    ) -> cyrup_provider::Model {
        cyrup_provider::Model {
            id: cyrup_core::ModelId::from(id),
            name: name.to_string(),
            api: cyrup_core::ApiId::from("test-api"),
            provider: cyrup_core::ProviderId::from(provider),
            base_url: "https://example.invalid".to_string(),
            reasoning,
            input: vec![cyrup_provider::Modality::Text],
            cost: cyrup_provider::ModelCost {
                input: cost_total / 2.0,
                output: cost_total / 2.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            },
            context_window,
            max_tokens,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    /// pi `resolveProbeStatus` (profiles.ts:310-316): `timedOut` always wins; empty text is
    /// `error`; an auth/billing-shaped message wins over an unavailable-shaped one; anything else
    /// is `error`.
    #[test]
    fn resolve_probe_status_matches_pi_precedence() {
        assert_eq!(resolve_probe_status("anything", true), ProbeStatus::Timeout);
        assert_eq!(resolve_probe_status("", false), ProbeStatus::Error);
        assert_eq!(resolve_probe_status("401 Unauthorized: bad API key", false), ProbeStatus::Auth);
        assert_eq!(resolve_probe_status("Error: model not found", false), ProbeStatus::Unavailable);
        assert_eq!(resolve_probe_status("connection reset by peer", false), ProbeStatus::Error);
    }

    /// pi `extractVersionScore` (profiles.ts:150-154): the max numeric token, decimals included.
    #[test]
    fn extract_version_score_takes_the_max_numeric_token() {
        assert_eq!(extract_version_score("claude-3-5-sonnet"), 5.0);
        assert_eq!(extract_version_score("gpt-4o"), 4.0);
        assert_eq!(extract_version_score("gemini-1.5-pro"), 1.5);
        assert_eq!(extract_version_score("no-numbers-here"), 0.0);
    }

    /// pi `modelNameTokens`/`inferProfileBand` (profiles.ts:156-172).
    #[test]
    fn infer_profile_band_recognizes_known_name_tokens() {
        assert_eq!(infer_profile_band("Claude Haiku 4.5"), 1);
        assert_eq!(infer_profile_band("Claude Opus 4.5"), 4);
        assert_eq!(infer_profile_band("Claude Sonnet 4.5"), 3);
        assert_eq!(infer_profile_band("Gemini 2.0 Flash"), 0);
        assert_eq!(infer_profile_band("Totally Unbranded Model"), 2);
    }

    /// THE core regression this unit's dossier item 3 flags: cyrup used to rank a provider's
    /// models by raw ascending `cost.input + cost.output` (`provider_ranked_full_ids`'s old body),
    /// NOT by pi's `derived.profileRank` (profiles.ts:298, driven by capability heuristics, not
    /// price). Construct two models where cost order and capability order are OPPOSITE — an
    /// expensive-but-weak model and a cheap-but-strong one — and assert `classify_model` ranks the
    /// weak model lower (as pi's `profileRank` does), even though it is the pricier of the two.
    /// The pre-fix cost-ascending sort would have put the cheap/strong model FIRST (i.e. into the
    /// "cheap" tier) and the expensive/weak model LAST (the "strong" tier) — exactly backwards.
    #[test]
    fn classify_model_ranks_by_capability_not_raw_cost() {
        let expensive_but_weak =
            test_model("acme", "acme-nano-1", "Acme Nano 1", 100.0, 4_000, 1_000, false);
        let cheap_but_strong =
            test_model("acme", "acme-opus-9", "Acme Opus 9", 2.0, 200_000, 64_000, true);
        let ctx = build_classification_context(&[expensive_but_weak.clone(), cheap_but_strong.clone()]);

        let weak_rank = classify_model(&expensive_but_weak, &ctx).profile_rank;
        let strong_rank = classify_model(&cheap_but_strong, &ctx).profile_rank;

        assert!(
            weak_rank < strong_rank,
            "the weak/expensive model must rank BELOW the strong/cheap one (profileRank {weak_rank} vs {strong_rank})"
        );
        // The pre-fix behavior (ascending raw cost) would order these the OTHER way: cheap (2.0)
        // before expensive (100.0) — i.e. strong before weak. Confirm the two orderings actually
        // disagree, so this test is a genuine regression proof, not a vacuous assertion.
        let cost_ascending_puts_strong_first =
            combined_cost(&cheap_but_strong.cost) < combined_cost(&expensive_but_weak.cost);
        assert!(cost_ascending_puts_strong_first, "test fixture must actually invert cost vs capability");
    }

    /// pi `catalogModelIsUsable` (profiles.ts:402-404): only `unavailable`/`auth`/`timeout`/`error`
    /// probe outcomes are unusable; `ok` (and any legacy/unknown string) is usable.
    #[test]
    fn probe_status_is_usable_matches_pi_predicate() {
        assert!(probe_status_is_usable("ok"));
        assert!(!probe_status_is_usable("unavailable"));
        assert!(!probe_status_is_usable("auth"));
        assert!(!probe_status_is_usable("timeout"));
        assert!(!probe_status_is_usable("error"));
    }

    /// pi `dominatesModel`/`filterDominatedModels` (profiles.ts:365-383): a candidate that is
    /// cheaper-or-equal, ranked-at-least-as-high, and never worse on reasoning/context/max-tokens —
    /// with at least one strict improvement — dominates and drops the other.
    #[test]
    fn filter_dominated_drops_strictly_worse_candidates() {
        let dominated = RankedCandidate {
            full_id: "acme/weak-and-pricier".to_string(),
            cost: 10.0,
            profile_rank: 5,
            reasoning: false,
            context_window: 1_000,
            max_tokens: 100,
        };
        let dominator = RankedCandidate {
            full_id: "acme/strong-and-cheaper".to_string(),
            cost: 5.0,
            profile_rank: 50,
            reasoning: true,
            context_window: 2_000,
            max_tokens: 200,
        };
        let incomparable = RankedCandidate {
            full_id: "acme/cheap-but-narrow".to_string(),
            cost: 1.0,
            profile_rank: 1,
            reasoning: false,
            context_window: 500,
            max_tokens: 50,
        };
        let kept = filter_dominated(vec![dominated, dominator.clone(), incomparable.clone()]);
        let kept_ids: Vec<&str> = kept.iter().map(|c| c.full_id.as_str()).collect();
        assert!(!kept_ids.contains(&"acme/weak-and-pricier"), "the dominated candidate must be dropped");
        assert!(kept_ids.contains(&"acme/strong-and-cheaper"));
        assert!(kept_ids.contains(&"acme/cheap-but-narrow"), "an incomparable (Pareto-optimal) candidate must survive");
    }

    /// pi `refreshProviderModelCatalog` throws `"No models found in the current registry for
    /// provider '...'."` (profiles.ts:506-508) when the registry has zero models for the provider —
    /// cyrup used to return `Ok("... nothing to refresh...")` for this exact case instead. The
    /// unknown-provider check runs BEFORE any filesystem write, so this is safe to exercise without
    /// `CYRUP_HOME` sandboxing (no real `~/.cyrup` write happens on this path).
    #[tokio::test]
    async fn refresh_provider_catalog_cache_errors_for_an_unknown_provider() {
        let ext = SubagentsExtension::new();
        let cwd = std::env::temp_dir();
        let result = ext
            .refresh_provider_catalog_cache(&cwd, "totally-unknown-provider-xyz", false)
            .await;
        match result {
            Err(SubagentError::MalformedSettings(msg)) => {
                assert!(
                    msg.contains("No models found in the current registry for provider"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!(
                "expected an Err(MalformedSettings) for an unknown provider (pi throws here), got {other:?}"
            ),
        }
    }

    /// pi `generateProfilesForProvider` -> `refreshProviderModelCatalog` throws the identical
    /// "No models found..." error (profiles.ts:506-508, invoked at profiles.ts:586) BEFORE any
    /// usable-model filtering — cyrup used to return `Ok("... nothing to generate...")` instead.
    /// Also safe without `CYRUP_HOME` sandboxing: the unknown-provider check is the very first
    /// thing this handler does, before any filesystem write.
    #[tokio::test]
    async fn generate_provider_profiles_errors_for_an_unknown_provider() {
        let ext = SubagentsExtension::new();
        let result = ext.generate_provider_profiles("totally-unknown-provider-xyz").await;
        match result {
            Err(SubagentError::MalformedSettings(msg)) => {
                assert!(
                    msg.contains("No models found in the current registry for provider"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!(
                "expected an Err(MalformedSettings) for an unknown provider (pi throws here), got {other:?}"
            ),
        }
    }

    /// [`SubagentsExtension::provider_ranked_full_ids_from_catalog`] must drop a probed-unavailable
    /// model entirely (pi `catalogModelIsUsable`) rather than still ranking it — proven against the
    /// REAL model registry so this exercises the actual registry cross-reference lookup, not just
    /// a synthetic fixture.
    #[test]
    fn provider_ranked_full_ids_from_catalog_drops_unusable_probe_results() {
        let anthropic_model = registry_models()
            .iter()
            .find(|m| m.provider.as_str() == "anthropic")
            .expect("the registry must carry at least one anthropic model for this test");
        let full_id = format!("anthropic/{}", anthropic_model.id.as_str());

        let usable_catalog = crate::registration::profiles::ProviderModelCatalog {
            provider: "anthropic".to_string(),
            refreshed_at_epoch_ms: 0,
            max_age_days: 7,
            sources: vec![],
            models: vec![crate::registration::profiles::ProviderCatalogModel {
                id: anthropic_model.id.as_str().to_string(),
                full_id: full_id.clone(),
                profile_rank: 10,
                probe_status: "ok".to_string(),
            }],
        };
        let ranked =
            SubagentsExtension::provider_ranked_full_ids_from_catalog("anthropic", &usable_catalog);
        assert_eq!(ranked, vec![full_id.clone()]);

        let unusable_catalog = crate::registration::profiles::ProviderModelCatalog {
            models: vec![crate::registration::profiles::ProviderCatalogModel {
                probe_status: "unavailable".to_string(),
                ..usable_catalog.models.first().expect("one model").clone()
            }],
            ..usable_catalog
        };
        let ranked_after_unavailable =
            SubagentsExtension::provider_ranked_full_ids_from_catalog("anthropic", &unusable_catalog);
        assert!(
            ranked_after_unavailable.is_empty(),
            "an unavailable-probe model must be filtered out of the ranked list entirely"
        );
    }

    /// [`probe_model_with`] exercised against REAL, fast, deterministic stand-in subprocesses (no
    /// live provider network call): a zero exit is `Ok`, a non-zero exit with an auth-shaped
    /// stderr message classifies as `Auth`, and a command that outlives the timeout classifies as
    /// `Timeout` (and is actually killed — `kill_on_drop`).
    #[tokio::test]
    async fn probe_model_with_classifies_real_subprocess_outcomes() {
        let sh = crate::spawn::SpawnCommand {
            binary: PathBuf::from("/bin/sh"),
            base_args: vec!["-c".to_string(), "printf OK".to_string()],
        };
        let ok_outcome = probe_model_with(&sh, "irrelevant/model", 5_000).await;
        assert_eq!(ok_outcome.status, ProbeStatus::Ok);

        let auth_failure = crate::spawn::SpawnCommand {
            binary: PathBuf::from("/bin/sh"),
            base_args: vec![
                "-c".to_string(),
                "echo '401 Unauthorized: invalid API key' 1>&2; exit 1".to_string(),
            ],
        };
        let auth_outcome = probe_model_with(&auth_failure, "irrelevant/model", 5_000).await;
        assert_eq!(auth_outcome.status, ProbeStatus::Auth);

        let sleeper = crate::spawn::SpawnCommand {
            binary: PathBuf::from("/bin/sh"),
            base_args: vec!["-c".to_string(), "sleep 30".to_string()],
        };
        let timeout_outcome = probe_model_with(&sleeper, "irrelevant/model", 50).await;
        assert_eq!(timeout_outcome.status, ProbeStatus::Timeout);
    }

    // =========================================================================================
    // SUBA-003: `subagents.modelScope` enforcement
    // (pi `runs/shared/model-scope.ts` + `model-fallback.ts:200-212`)
    // =========================================================================================

    /// Seed a cwd with one discoverable agent and (optionally) a `subagents` settings block.
    fn seed_scope_fixture(cwd: &Path, agent: &str, settings_json: Option<&str>) {
        let agents_dir = cwd.join(".cyrup").join("agents");
        std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
        std::fs::write(
            agents_dir.join(format!("{agent}.md")),
            format!("---\nname: {agent}\ndescription: Model-scope fixture agent\n---\nBody.\n"),
        )
        .expect("write agent fixture");
        if let Some(json) = settings_json {
            std::fs::write(agents_dir.join("settings.json"), json).expect("write settings.json");
        }
    }

    /// SUBA-003, the load-bearing observable behavior: with `subagents.modelScope.enforce` armed,
    /// a run that EXPLICITLY asks for a model outside the `allow` list is REFUSED — the call
    /// returns `Err(SubagentError::ModelOutOfScope)` carrying pi's verbatim violation message, and
    /// no child process is ever spawned.
    ///
    /// Before this fix `modelScope` was not even a field on `SubagentSettings`, so serde dropped
    /// the whole block silently and this call ran the out-of-scope model to completion.
    #[tokio::test]
    async fn an_explicit_out_of_scope_model_refuses_the_run_with_pis_verbatim_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(
            dir.path(),
            "scoped",
            Some(
                r#"{"subagents":{"modelScope":{"enforce":true,"allow":["anthropic/*","together/*"]}}}"#,
            ),
        );

        let executor = SubagentExecutor::new();
        let err = executor
            .run_foreground(
                dir.path(),
                "scoped",
                "do something",
                Some(ContextMode::Fresh),
                Some(ModelId::from("openai/gpt-5-nano")),
                None,
            )
            .await
            .expect_err("an out-of-scope explicit model must REFUSE the run, not run it");

        assert!(
            matches!(err, SubagentError::ModelOutOfScope(_)),
            "the refusal must be its own error kind, not folded into a generic failure: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Model 'openai/gpt-5-nano' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/*, together/*.",
            "the caller must see pi's verbatim violation text, naming the model AND the patterns"
        );
    }

    /// The thinking suffix must not defeat the policy: `<allowed>:max` is still the allowed model
    /// (pi strips a KNOWN suffix before matching), while `<disallowed>:max` is still refused and is
    /// REPORTED under its base id. `:max` is the 7th thinking level added by commit 6d29542.
    #[tokio::test]
    async fn a_thinking_suffix_neither_smuggles_a_model_in_nor_hides_one_from_the_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(
            dir.path(),
            "scoped",
            Some(r#"{"subagents":{"modelScope":{"enforce":true,"allow":["anthropic/claude-opus-4"]}}}"#),
        );
        let executor = SubagentExecutor::new();

        let err = executor
            .run_foreground(
                dir.path(),
                "scoped",
                "t",
                Some(ContextMode::Fresh),
                Some(ModelId::from("openai/gpt-5-nano:max")),
                None,
            )
            .await
            .expect_err("a thinking suffix must not smuggle an out-of-scope model past the gate");
        assert_eq!(
            err.to_string(),
            "Model 'openai/gpt-5-nano' is outside the configured subagent model scope. Allowed \
             patterns: anthropic/claude-opus-4.",
            "the reported model must be the BASE id, with the thinking suffix stripped"
        );

        // The mirror case is asserted at the decision boundary rather than through `run_foreground`,
        // because an ALLOWED model proceeds to a real subprocess spawn (this crate never fakes that).
        let scope = SubagentExecutor::resolve_model_scope(dir.path())
            .expect("settings parse")
            .expect("a modelScope block is configured");
        let mut available = Vec::new();
        let allowed = ModelId::from("anthropic/claude-opus-4:max");
        assert!(
            crate::exec::fallback::resolve_model_inheritance(
                Some(&allowed),
                None,
                None,
                &mut available,
                Some(&scope),
            )
            .is_ok(),
            "an ALLOWED model carrying a known thinking suffix must pass the gate unchanged"
        );
    }

    /// The refusal must be a REFUSAL, not a downgrade: the identical call with no `modelScope`
    /// configured must not produce a scope error at all, and the armed policy must never rewrite
    /// the requested model into an allowed one.
    #[tokio::test]
    async fn enforcement_is_off_without_a_policy_and_never_substitutes_an_allowed_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(dir.path(), "scoped", None);
        assert_eq!(
            SubagentExecutor::resolve_model_scope(dir.path()).expect("settings parse"),
            None,
            "no settings block means no policy — enforcement stays off"
        );

        // With no policy, the exact model the caller asked for is what resolves.
        let mut available = Vec::new();
        let requested = ModelId::from("openai/gpt-5-nano");
        let resolved = crate::exec::fallback::resolve_model_inheritance(
            Some(&requested),
            None,
            None,
            &mut available,
            None,
        )
        .expect("no policy configured, so nothing can be refused");
        assert_eq!(resolved, crate::exec::fallback::ModelOverride::Explicit(requested.clone()));

        // With a policy that REFUSES it, the outcome is an error — never `Ok(<some other model>)`.
        let scope = crate::exec::model_scope::ModelScopeConfig {
            enforce: Some(true),
            allow: Some(vec!["anthropic/*".to_string()]),
        };
        let refused = crate::exec::fallback::resolve_model_inheritance(
            Some(&requested),
            None,
            None,
            &mut available,
            Some(&scope),
        );
        assert!(
            refused.is_err(),
            "fail closed: an out-of-scope explicit model may not resolve to ANY model, {refused:?}"
        );
        assert!(
            available.is_empty(),
            "a refused resolution must not have mutated the availability set"
        );
    }

    /// R-SA-009: a malformed `modelScope` block ABORTS discovery rather than degrading to an
    /// unenforced policy — the fail-closed posture applied to the settings read itself. Before the
    /// fix, `SubagentSettings` had no such field and serde discarded every one of these silently.
    #[test]
    fn a_malformed_model_scope_block_aborts_discovery_instead_of_silently_disarming() {
        for (label, json) in [
            ("enforce without allow", r#"{"subagents":{"modelScope":{"enforce":true}}}"#),
            ("non-object", r#"{"subagents":{"modelScope":[]}}"#),
            ("non-boolean enforce", r#"{"subagents":{"modelScope":{"enforce":"yes"}}}"#),
            (
                "non-string allow entries",
                r#"{"subagents":{"modelScope":{"enforce":true,"allow":[1]}}}"#,
            ),
        ] {
            let dir = tempfile::tempdir().expect("tempdir");
            seed_scope_fixture(dir.path(), "scoped", Some(json));
            let err = SubagentExecutor::resolve_model_scope(dir.path())
                .expect_err(&format!("{label} must abort, not silently disarm the policy"));
            assert!(
                matches!(err, SubagentError::MalformedSettings(_)),
                "{label}: expected MalformedSettings, got {err:?}"
            );
        }
    }

    /// A well-formed block is actually READ (the SUBA-003 root cause: it was parsed by nothing),
    /// with project scope winning over user scope exactly as every other `subagents.*` scalar does.
    #[test]
    fn a_well_formed_model_scope_block_is_read_and_normalized() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_scope_fixture(
            dir.path(),
            "scoped",
            Some(r#"{"subagents":{"modelScope":{"enforce":true,"allow":["  anthropic/*  "]}}}"#),
        );
        let scope = SubagentExecutor::resolve_model_scope(dir.path())
            .expect("settings parse")
            .expect("the configured block must be read, not dropped");
        assert_eq!(scope.enforce, Some(true));
        assert_eq!(scope.allow, Some(vec!["anthropic/*".to_string()]), "patterns are trimmed");
        assert!(scope.is_armed());
    }

    /// The background path is not a hole in the policy: the resolved scope is baked into the
    /// serialized `RunnerConfig` the detached hop-2 runner is handed over `--config`, which is the
    /// only channel by which anything reaches that separate OS process.
    #[test]
    fn the_model_scope_reaches_the_detached_runner_through_the_serialized_config() {
        let scope = crate::exec::model_scope::ModelScopeConfig {
            enforce: Some(true),
            allow: Some(vec!["anthropic/*".to_string()]),
        };
        let config = crate::background::runner_main::RunnerConfig {
            run_id: RunId::new(),
            mode: RunMode::Single,
            steps: Vec::new(),
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            global_concurrency_limit: 4,
            worktree_base_dir: None,
            max_subagent_depth: 2,
            async_root: PathBuf::new(),
            results_dir: PathBuf::new(),
            resolved_agents: BTreeMap::new(),
            original_task: String::new(),
            chain_dir: None,
            orchestrator_intercom_target: None,
            inherited_session_model: None,
            model_scope: Some(scope.clone()),
            nested_route: None,
            nested_self: None,
            dynamic_fanout_max_items: None,
        };
        let json = serde_json::to_value(&config).expect("config serializes");
        assert_eq!(
            json.get("modelScope").and_then(|v| v.get("allow")),
            Some(&serde_json::json!(["anthropic/*"])),
            "the policy must be present in the on-disk config handed to the child: {json}"
        );
        let round_tripped: crate::background::runner_main::RunnerConfig =
            serde_json::from_value(json).expect("config round-trips");
        assert_eq!(round_tripped.model_scope, Some(scope));
    }
}
