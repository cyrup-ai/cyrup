//! The real, subprocess-spawning [`crate::spawn::chain_graph::SingleStepExecutor`]
//! ([`ExecSingleStepExecutor`], func-SA §1.1, R-SA-130): one executor for both the foreground
//! and background paths. Split out of `background/runner_main.rs`; ports
//! pi `runs/background/subagent-runner.ts`.

use super::settle::child_stopped_step_result;
use super::status::TelemetryMsg;
use crate::background::RunId;
use crate::background::child_stop::ChildStopRegistry;
use crate::background::control;
use crate::error::SubagentError;
use crate::exec;
use crate::exec::{AgentConfig, ResolvedAgentPersona, RunOptions, SingleResult};
use crate::fork_context::{ContextMode, ForkContext};
use crate::spawn::chain_graph::{ChainRunContext, SingleStepExecutor, SingleStepSpec, StepResult};
use crate::spawn::depth::DepthEnvelope;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

// =================================================================================================
// ExecSingleStepExecutor — the real, subprocess-spawning SingleStepExecutor (func-SA §1.1)
// =================================================================================================

/// The production [`SingleStepExecutor`] this runner's [`walk_chain`](crate::spawn::chain_graph::walk_chain) calls dispatch through: runs
/// one [`SingleStepSpec`] to completion via [`exec::run_sync`], which — per func-SA §1.1's
/// mandated mechanism — spawns a genuine OS subprocess re-exec of the `cyrup` binary for every
/// attempt. This struct itself spawns nothing directly; it is a thin adapter translating a
/// [`SingleStepSpec`] (this file's/`chain_graph`'s own data-only step shape) into the
/// [`AgentConfig`]/[`RunOptions`] pair `exec::run_sync` actually consumes.
///
/// `pub(crate)` (rather than private to this module) so `extension.rs`'s FOREGROUND `/chain`,
/// `/parallel`, and `/run-chain` slash-command dispatch (R-SA-130: same executor as every other
/// call site, never a second divergent implementation) can drive the exact same
/// [`SingleStepExecutor`] this hop-2 background runner uses, rather than hand-rolling a second
/// `SingleStepSpec` -> `AgentConfig`/`RunOptions` adapter that could silently drift out of sync
/// with this one.
pub(crate) struct ExecSingleStepExecutor {
    pub(crate) depth: DepthEnvelope,
    pub(crate) interrupted: Arc<std::sync::atomic::AtomicBool>,
    /// The run-wide SHARED soft-interrupt token (R-SA-084). Cloned into every dispatched step's
    /// [`RunOptions::interrupt`] so that when the control-inbox watcher cancels it — an interrupt
    /// landing while a step's child is still running — that child is actually signalled and torn
    /// down mid-flight via `exec::run_sync`'s own `opts.interrupt` race, not merely noticed between
    /// steps. For a foreground executor (no control-inbox watcher) this token is never cancelled.
    pub(crate) interrupt_cancel: cyrup_core::CancelToken,
    /// SUBA-087 — the run's child-scoped stop registry (`ControlFlags::child_stops`), through which
    /// each dispatched step's OWN stop handle (registered by `run_inner` under the step's flat
    /// index) is read back as that child's `RunOptions::interrupt`. `None` for a foreground
    /// executor, which has no control inbox and no child-scoped stops.
    pub(crate) child_stops: Option<ChildStopRegistry>,
    /// In-process binary override applied to every step this executor dispatches, mirroring
    /// [`RunOptions::spawn_command`]. `Some` only on the FOREGROUND chain/parallel walk, which is
    /// constructed where the extension config is in hand; `None` on the background and detached
    /// paths, whose steps resolve their command from the environment they inherited exactly as
    /// before — `RunnerConfig` crosses a process boundary as JSON and carries no such value.
    pub(crate) spawn_command: Option<crate::spawn::SpawnCommand>,
    /// Caller-supplied additions to each dispatched step's child environment
    /// ([`RunnerOverrides::child_env`](super::RunnerOverrides::child_env)). Empty on the real detached runner.
    pub(crate) child_env: std::collections::HashMap<String, String>,
    /// The live-telemetry channel (`None` for a foreground executor with no `status.json` to
    /// update): each dispatched step installs a [`RunOptions::live_events`] sink that forwards every
    /// raw child NDJSON line here, tagged with the dispatch's own
    /// [`crate::spawn::chain_graph::ChainRunContext::step_slot`] index, for the runner's own
    /// telemetry task to fold into `status.json` (pi `updateStepFromChildEvent`).
    pub(crate) telemetry: Option<tokio::sync::mpsc::UnboundedSender<TelemetryMsg>>,
    /// The fully-resolved persona for every agent any dispatched step may name (T0.1 / C13), keyed
    /// by the exact [`SingleStepSpec::agent`] string — resolved EAGERLY at plan time by the
    /// orchestrator (via [`crate::exec::resolve_step_agent_config`]) and threaded in here so
    /// [`Self::run_single`] dispatches the REAL named persona rather than re-discovering (this
    /// executor has, by design, no discovery dependency) or synthesizing a placeholder. `Arc`-wrapped
    /// so it can be cheaply shared across every fanned-out child of a parallel/dynamic group without
    /// cloning the whole map per step. Mirrors pi's already-resolved `agents` list every child
    /// resolves against (`chain-execution.ts:1011`, `parallel-execution.test.ts:134-172`).
    pub(crate) resolved_agents: Arc<BTreeMap<String, ResolvedAgentPersona>>,
    /// The launching orchestrator's own intercom presence target (pi
    /// `config.controlIntercomTarget` / `data.intercomBridge.orchestratorTarget`), threaded into
    /// every dispatched step's [`crate::exec::RunOptions::orchestrator_intercom_target`] so each
    /// spawned child activates its `contact_supervisor` bridge addressed at that supervisor. `None`
    /// (headless runner with no live intercom orchestrator, or a foreground run with no session id)
    /// leaves each child un-bridged — the clean no-intercom path.
    pub(crate) orchestrator_intercom_target: Option<String>,
    /// This run's id (pi `runId`/`config.runId`), folded with each step's agent + flat index into
    /// that child's own deterministic presence label
    /// ([`crate::spawn::intercom_target::resolve_subagent_intercom_target`]) — the address
    /// `control_resume` steers. Paired with [`Self::orchestrator_intercom_target`]: both `Some` is
    /// the child-bridge activation gate.
    pub(crate) run_id: Option<RunId>,
    /// The live PARENT session model (pi `ctx.model`, `${provider}/${id}`), inherited by any step
    /// whose persona declares no `model:` and that carries no per-step `model` override — the
    /// analog of the foreground single-run path's `SubagentExecutor::inherited_session_model()`.
    /// Threaded here (rather than read from a `HostServices` handle) because the detached hop-2
    /// runner is a separate OS process with NO host-services backend at all: the orchestrator
    /// captures it at plan time and carries it verbatim through
    /// [`RunnerConfig::inherited_session_model`](super::RunnerConfig::inherited_session_model). `None` (headless / no live session, or a detached
    /// runner launched before any model was active) leaves each inheriting step's ladder to fall
    /// through to its persona's own `model`/`fallback_models`, exactly as before this seam existed.
    /// Consumed by [`Self::run_single`] via [`crate::exec::fallback::resolve_model_inheritance`].
    pub(crate) inherited_session_model: Option<cyrup_core::ModelId>,
    /// SCOPE_19/A1 — the live PARENT session's reasoning level, the thinking half of
    /// [`Self::inherited_session_model`] and threaded by the same two routes: the one-shot
    /// [`RunnerConfig::inherited_session_thinking`](super::RunnerConfig::inherited_session_thinking)
    /// on the background path, [`Self::with_inherited_session_thinking`] on the foreground
    /// chain/parallel walk. Folded in [`Self::build_step_agent_config`] as the default BELOW the
    /// persona's own `thinking:` — an inheriting step reasons at the parent's level, a persona that
    /// declares a level keeps it, and the caller's explicit per-call level (foreground overrides /
    /// the plan-time persona stamp on the async single path) still beats both. `None` (headless /
    /// no live session at plan time) leaves each step on its persona's own level, exactly as
    /// before this seam existed.
    pub(crate) inherited_session_thinking: Option<String>,
    /// The launching process's host builtin-tool observation (pi `config.hostAvailableBuiltins`),
    /// threaded into every dispatched step's [`crate::exec::RunOptions::host_available_builtins`].
    ///
    /// Two feeders, because this executor has two constructors: the FOREGROUND `/chain`//`/parallel`
    /// walk observes the live host directly ([`Self::foreground`]'s caller has `HostServices` in
    /// hand), and the detached hop-2 runner reads
    /// [`RunnerConfig::host_available_builtins`](super::RunnerConfig::host_available_builtins) — it is
    /// a separate OS process whose own registry is not the parent's. `None` is UNKNOWN and skips the
    /// intersection.
    pub(crate) host_available_builtins: Option<Vec<String>>,
    /// The effective `subagents.modelScope` policy for this run (SUBA-003), carried from the
    /// orchestrator via [`RunnerConfig::model_scope`](super::RunnerConfig::model_scope) (background) or handed directly by
    /// [`Self::foreground`]. Consumed by [`Self::run_single`], where a per-step `model:` override
    /// outside the scope FAILS the step (fail-closed, pi's `explicit` severity) rather than being
    /// quietly replaced by an allowed model. `None` = enforcement off.
    pub(crate) model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
    /// SUBA-N05 — the run's FULLY-RESOLVED live-control config (pi `controlConfig`,
    /// `subagent-runner.ts:1953` / `chain-execution.ts:322,491` @v0.34.0), carried from
    /// [`RunnerConfig::control`](super::RunnerConfig::control) (background) or handed by [`Self::with_control`] (foreground
    /// `/chain`, `/parallel`, `/run-chain`). Threaded onto every dispatched step's
    /// [`crate::exec::RunOptions::control_config`], so an explicit `control` override really does
    /// move the attention/long-running thresholds each child stream is judged against.
    ///
    /// `None` is pi's `?? DEFAULT_CONTROL_CONFIG` degrade, applied inside `run_sync`.
    pub(crate) control: Option<crate::exec::control::ResolvedControlConfig>,
    /// SUBA-N06 — the run's `includeProgress` flag, carried from
    /// [`RunnerConfig::include_progress`](super::RunnerConfig::include_progress) (background) or handed by [`Self::with_control`]'s
    /// sibling [`Self::with_include_progress`] (foreground `/chain`, `/parallel`, `/run-chain`).
    /// Threaded onto every dispatched step's [`crate::exec::RunOptions::include_progress`], so each
    /// step's [`crate::exec::SingleResult`] carries its own progress snapshot.
    pub(crate) include_progress: Option<bool>,
    /// SUBA-N03 — the run's `share` opt-in (pi `config.share` ← `params.share`), threaded onto
    /// every dispatched step's [`crate::exec::RunOptions::share`]. Its one effect is pi's
    /// `sessionEnabled = Boolean(sessionFile || sessionDir) || share` term
    /// (`runs/foreground/execution.ts:1027,1039` @v0.34.0): `Some(true)` keeps a child's session store on
    /// where it would otherwise be spawned `--no-session`. `None`/`Some(false)` is not enabling.
    pub(crate) share: Option<bool>,
    /// SUBA-008 — the run-level assistant-TURN budget, carried from
    /// [`RunnerConfig::turn_budget`](super::RunnerConfig::turn_budget) and threaded onto every dispatched step's
    /// [`crate::exec::RunOptions::turn_budget`] (pi `ctx.turnBudget` →
    /// `runSubagentProcess({ … turnBudget: ctx.turnBudget })`, `subagent-runner.ts:1091`/`:1409`).
    /// `None` is unbudgeted.
    pub(crate) turn_budget: Option<crate::exec::turn_budget::ResolvedTurnBudget>,
    /// SUBA-073 — the run-level, fully-merged permission policy, carried from
    /// [`RunnerConfig::permission_rules`](super::RunnerConfig::permission_rules) and threaded onto every dispatched step's
    /// [`crate::exec::RunOptions::permission_rules`]. `None` is no policy.
    pub(crate) permission_rules: Option<crate::watchdog::permission_arbiter::PermissionRules>,
    /// SUBA-021 — the run-level USAGE budget, carried from [`RunnerConfig::usage_budget`](super::RunnerConfig::usage_budget) and
    /// threaded onto every dispatched step's [`crate::exec::RunOptions::usage_budget`]. `None` is
    /// unbudgeted.
    pub(crate) usage_budget: Option<crate::exec::usage_budget::UsageBudgetConfig>,
    /// SUBA-N03 — where this run's per-step artifact quadruple is written (pi `ctx.artifactsDir`,
    /// `runs/background/subagent-runner.ts:879-890,1117-1125` @v0.34.0 @v0.34.0), paired with
    /// [`Self::artifact_config`]. `None` disables artifact writing outright, which is exactly pi's
    /// own first gate term (`if (ctx.artifactsDir && ctx.artifactConfig?.enabled !== false)`) and
    /// is how an explicit `artifacts: false` reaches this hop.
    pub(crate) artifacts_dir: Option<PathBuf>,
    /// SUBA-N03 — which of the four artifact files each dispatched step writes (pi
    /// `ctx.artifactConfig`). Read together with [`Self::artifacts_dir`]; `enabled: false` disables
    /// the write just as an absent dir does.
    pub(crate) artifact_config: crate::artifacts::ArtifactConfig,
    /// G90 — this run's async run directory, the root of the steer control inbox
    /// (`<run_dir>/control/steer-targets/<flatIndex>/`). Each dispatched step derives its OWN
    /// per-child inbox from it and hands the path to the child in
    /// [`crate::exec::RunOptions::steer_inbox_dir`], which is pi's
    /// `steerInboxDir: stepSteerInboxDir(asyncDir, fi)` (`subagent-runner.ts:2313,2600,2797`
    /// @v0.34.0).
    ///
    /// `None` for a FOREGROUND executor, matching upstream exactly: `steerInboxDir` is supplied
    /// only by the background runner, because the inbox lives inside an async run directory and a
    /// foreground `/chain`//`/parallel` walk has none. That is also why `control_steer` refuses a
    /// foreground run outright (`crate::extension::STEER_FOREGROUND_RUN_REFUSAL`) rather than
    /// queueing into a directory nothing would ever read.
    pub(crate) run_dir: Option<PathBuf>,
}

/// The execution-ready dispatch inputs [`ExecSingleStepExecutor::build_step_agent_config`] lowers a
/// [`SingleStepSpec`] to: the persona-derived [`AgentConfig`] `exec::run_sync` runs, plus the three
/// [`RunOptions`] fields that same lowering decides.
struct StepAgentSetup {
    /// The persona's execution-ready config, stamped with this process's own depth envelope and
    /// with the step's `tools` / `max_depth_override` overrides applied.
    agent: AgentConfig,
    /// The union the availability filter selects from — the persona's fallback ladder + its own
    /// model + any per-step override + (when inheriting) the parent session model.
    available_models: Vec<cyrup_core::ModelId>,
    /// The resolved override the candidate ladder puts first, past the SUBA-003 scope gate.
    model_override: crate::exec::fallback::ModelOverride,
    /// This step's own lowered acceptance contract (SUBA-N04), `None` when it declared none.
    acceptance: Option<crate::exec::acceptance::AcceptanceContract>,
}

impl ExecSingleStepExecutor {
    /// Construct one for a FOREGROUND (non-detached-runner) caller: no live interrupt signal
    /// source exists at this call site (a foreground `/chain`/`/parallel`/`/run-chain` slash
    /// command has no control-inbox watcher, R-SA-082, of its own — that machinery is exclusively
    /// the hop-2 detached runner's), so `interrupted` starts (and stays) `false` for the lifetime
    /// of this executor; cancellation for a foreground run is instead carried by
    /// [`crate::spawn::chain_graph::ChainRunContext::cancel`], which every dispatched step's own
    /// `RunOptions::cancel` already threads through `exec::run_sync` regardless of this flag.
    ///
    /// `resolved_agents` is the SAME plan-time persona map a background run carries in
    /// [`RunnerConfig::resolved_agents`](super::RunnerConfig::resolved_agents) — the foreground orchestrator (`extension.rs`'s `/chain`//
    /// `/parallel` dispatch) resolves every step's persona via
    /// [`crate::exec::resolve_step_agent_config`] up front and hands the map here, so the SAME real
    /// persona reaches the child on both the foreground and background paths (R-SA-130: one
    /// executor, never two divergent resolutions).
    ///
    /// `orchestrator_intercom_target` (the foreground orchestrator's own intercom presence target,
    /// via `SubagentExecutor::orchestrator_intercom_target`) + `run_id` (a fresh id minted for this
    /// foreground walk) activate the child intercom bridge on the foreground `/chain`//`/parallel`
    /// path exactly as [`RunnerConfig`](super::RunnerConfig) does on the background path — so a foreground-spawned child's
    /// `contact_supervisor` reaches the live human orchestrator. `None`/absent leaves each child
    /// un-bridged (headless / no live intercom session).
    ///
    /// `inherited_session_model` (the live PARENT session model, via
    /// `SubagentExecutor::inherited_session_model()`) is the model an inheriting foreground chain/
    /// parallel step falls back to when its persona declares no `model:` and it carries no per-step
    /// override — the SAME session-model inheritance the foreground single-run path applies, so a
    /// `## reviewer` step with no configured model runs the parent's live model rather than an empty
    /// ladder. `None` (headless / no live session) leaves each inheriting step on its persona's own
    /// `model`/`fallback_models`, unchanged.
    ///
    /// `model_scope` is the cwd's effective `subagents.modelScope` policy (SUBA-003), resolved by
    /// the same orchestrator discovery pass that produced `resolved_agents`, so a foreground chain
    /// step's `model:` override is policed by exactly the policy the single-run path enforces.
    ///
    /// `host_available_builtins` is this walk's observation of the LIVE host tool registry (pi
    /// `hostAvailableBuiltins`), taken by the caller — which is the process that HAS the
    /// `HostServices` handle — and threaded onto every step this executor dispatches.
    ///
    /// Eight parameters is one over clippy's threshold, and deliberately so.
    /// `host_available_builtins` is not a `with_*` value: it is as fundamental as
    /// `inherited_session_model` above, and a builder would let the one production caller silently
    /// forget it — leaving the FOREGROUND `/chain`//`/parallel` walk launching with `None` while
    /// every other path carries a real observation. The same trade is taken by this call's own
    /// caller (`extension::executor::chain`'s `run_chain_foreground_with_control`) and by
    /// [`crate::exec::spawn_plan::build_attempt_spawn_plan_with_read_requirement`].
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub(crate) fn foreground(
        depth: DepthEnvelope,
        resolved_agents: Arc<BTreeMap<String, ResolvedAgentPersona>>,
        orchestrator_intercom_target: Option<String>,
        run_id: Option<RunId>,
        inherited_session_model: Option<cyrup_core::ModelId>,
        model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
        spawn_command: Option<crate::spawn::SpawnCommand>,
        host_available_builtins: Option<Vec<String>>,
    ) -> Self {
        Self {
            depth,
            spawn_command,
            // A foreground executor's child env comes from its own `RunOptions`, not from here.
            child_env: std::collections::HashMap::new(),
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            resolved_agents,
            // A foreground executor has no control-inbox watcher, so this token is never cancelled;
            // foreground cancellation flows through `ChainRunContext::cancel`/`RunOptions::cancel`.
            interrupt_cancel: cyrup_core::CancelToken::new(),
            // SUBA-087: no control inbox → no child-scoped stops on the foreground walk.
            child_stops: None,
            telemetry: None,
            orchestrator_intercom_target,
            run_id,
            inherited_session_model,
            host_available_builtins,
            // Set separately via `with_inherited_session_thinking` — same rationale as `control`
            // below: the value is resolved by the caller's own plan phase
            // (`remembered_parent_thinking`), not by the single discovery pass the positional
            // arguments all come from.
            inherited_session_thinking: None,
            model_scope,
            // Set separately via `with_control` rather than as a positional argument — see
            // that method's doc.
            control: None,
            // Same rationale, via `with_include_progress`.
            include_progress: None,
            // SUBA-N03: a FOREGROUND `/chain`//`/parallel`//`/run-chain` walk carries no run-level
            // `share`/artifacts config of its own — those three slash commands expose no such flag
            // (only the `subagent` tool's SINGLE mode does, and that path never builds this
            // executor), and neither does pi's own foreground chain path. Deliberately NOT given a
            // `with_*` builder: an unused one would be dead code, and the background runner sets
            // these three directly in its own `ExecSingleStepExecutor` literal from `RunnerConfig`.
            share: None,
            // SUBA-008: the foreground chain/parallel slash surfaces advertise no `turnBudget`
            // param — upstream's is on the `subagent` TOOL's schema (`extension/schemas.ts:328`),
            // not on `/chain`//`/parallel`//`/run-chain` — so a foreground walk is unbudgeted, as
            // it is upstream. The SINGLE-mode tool path does not build this executor; it passes
            // its own `RunOptions::turn_budget` directly.
            turn_budget: None,
            // SUBA-073: same as `turn_budget` — the foreground chain/parallel slash surfaces
            // expose no permission-policy input of their own either, so a foreground walk carries
            // no policy. The SINGLE-mode tool path does not build this executor; it resolves its
            // own `RunOptions::permission_rules` directly (`run_foreground_impl`).
            permission_rules: None,
            // SUBA-021: same as `turn_budget` — the foreground chain/parallel slash surfaces
            // advertise no `usageBudget` param upstream either, so a foreground walk is unbudgeted.
            usage_budget: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            // G90: a foreground walk has no async run directory, hence no steer inbox — the same
            // reason upstream supplies `steerInboxDir` only from the background runner.
            run_dir: None,
        }
    }

    /// SCOPE_19/A1 — install the live parent session's reasoning level for this walk's inheriting
    /// steps (see the field's doc). A builder step for the same reason [`Self::with_control`] is
    /// one: the value comes from the caller's own plan phase rather than the discovery pass that
    /// produced [`Self::foreground`]'s positional arguments.
    #[must_use]
    pub(crate) fn with_inherited_session_thinking(mut self, thinking: Option<String>) -> Self {
        self.inherited_session_thinking = thinking;
        self
    }

    /// Install this run's `includeProgress` flag (SUBA-N06) — R-SA-043 compaction's opt-out,
    /// threaded onto every dispatched step's [`crate::exec::RunOptions::include_progress`].
    ///
    /// A builder step for the same reason [`Self::with_control`] is one: [`Self::foreground`] takes
    /// only values produced by the single discovery pass, and this is not one of them.
    #[must_use]
    pub(crate) fn with_include_progress(mut self, include_progress: Option<bool>) -> Self {
        self.include_progress = include_progress;
        self
    }

    /// Install this run's already-resolved live-control config (SUBA-N05).
    ///
    /// A builder step rather than a [`Self::foreground`] parameter because it mirrors how the value
    /// actually flows — every caller resolves it with
    /// [`crate::exec::control::resolve_control_config`] at a different point in its own plan phase,
    /// whereas that constructor's positional arguments are all products of the single discovery
    /// pass.
    #[must_use]
    pub(crate) fn with_control(
        mut self,
        control: Option<crate::exec::control::ResolvedControlConfig>,
    ) -> Self {
        self.control = control;
        self
    }

    /// G90: the steer inbox the child at flat index `index` must be handed — pi
    /// `steerInboxDir: stepSteerInboxDir(asyncDir, fi)` (`subagent-runner.ts:2313,2600,2797`
    /// @v0.34.0).
    ///
    /// Named rather than inlined because the two halves of the runner hop have to agree on it and
    /// they are written 800 lines apart: `handle_steer_request` routes an accepted request into
    /// `control::enqueue_step_steer(run_dir, index, …)` (which writes to
    /// `step_steer_inbox_dir(run_dir, index)`), and this is where the SAME path is handed to the
    /// child. Deriving it from the run-level `steer_requests_dir`, or from a step index rather
    /// than the FLAT index, would leave both sides individually plausible and the feature silently
    /// dead — the failure mode this whole item is about.
    #[must_use]
    pub(crate) fn steer_inbox_for(&self, index: usize) -> Option<PathBuf> {
        self.run_dir
            .as_deref()
            .map(|run_dir| control::step_steer_inbox_dir(run_dir, index))
    }

    /// SUBA-049 — the RETURN half of [`Self::steer_inbox_for`], derived from the same run dir and
    /// the same flat index so the request hop and the acknowledgment hop cannot address different
    /// children (pi `steerAckDir: steerAcksDir(asyncDir, fi)` / `steerCapabilityPath:
    /// steerCapabilityPath(asyncDir, fi)`, `runs/shared/pi-args.ts:766-768,764-765` @v0.43.0).
    #[must_use]
    pub(crate) fn steer_ack_dir_for(&self, index: usize) -> Option<PathBuf> {
        self.run_dir
            .as_deref()
            .map(|run_dir| control::steer_acks_dir(run_dir, index))
    }

    /// SUBA-049 — this child's capability file, same derivation as [`Self::steer_ack_dir_for`].
    #[must_use]
    pub(crate) fn steer_capability_path_for(&self, index: usize) -> Option<PathBuf> {
        self.run_dir
            .as_deref()
            .map(|run_dir| control::steer_capability_path(run_dir, index))
    }

    /// T0.1 / C13: dispatch the REAL named persona. Every step's agent was resolved to a full
    /// persona at plan time by the orchestrator (`extension.rs` via
    /// `exec::resolve_step_agent_config`) and threaded in through `self.resolved_agents` — this
    /// executor never re-discovers (it has, by design, no discovery dependency). An agent absent
    /// from the map is dispatched as `Unknown agent: <name>` (a step FAILURE, mirroring pi's
    /// `agents.find((a) => a.name === seqStep.agent)` miss returning `Unknown agent`,
    /// `chain-execution.ts:1011-1019` / `execution.ts:898-908`) — never silently downgraded to a
    /// placeholder persona. This is what makes `## reviewer` in a chain actually run the
    /// reviewer persona (its own system prompt, model, fallback ladder, tools, and
    /// completion-guard flag), not an empty-system-prompt / `--model default` / guard-disabled
    /// stand-in.
    ///
    /// `Err` carries the pre-spawn REJECTION this step must report: an unknown agent (above),
    /// an out-of-scope explicit `model:` (SUBA-003) or a malformed `acceptance` policy
    /// (SUBA-N04). Each is a step FAILURE rather than a [`SubagentError`], because that is how
    /// this executor reports every other pre-spawn rejection — keeping the run's own status
    /// record and the surrounding chain semantics intact. It is BOXED because a `StepResult` is 176
    /// bytes, which `clippy::result_large_err` (rightly) refuses to widen every `Ok` return of this
    /// pre-spawn path by for the sake of three cold rejection arms.
    fn build_step_agent_config(
        &self,
        step: &SingleStepSpec,
    ) -> Result<StepAgentSetup, Box<StepResult>> {
        let Some(persona) = self.resolved_agents.get(&step.agent) else {
            return Err(Box::new(StepResult::failure(format!(
                "Unknown agent: {}",
                step.agent
            ))));
        };

        // Reconstitute the execution-ready config from the persona, stamping THIS process's own
        // live depth envelope (a per-process runtime value the persona deliberately does not carry).
        let mut agent: AgentConfig = persona.to_agent_config(self.depth);
        // SCOPE_19/A1 — session-thinking inheritance, the effort half of the model inheritance
        // resolved below: a persona with no `thinking:` of its own reasons at the level the
        // launching session was reasoning at (which is also what keeps the child's request prefix
        // aligned with — and served from — the parent's cache), while a persona that declares a
        // level keeps it. The caller's explicit per-call level was folded ABOVE this rung by the
        // dispatch site (foreground `run_foreground_impl`'s overrides fold / `spawn_background`'s
        // persona stamp), so the full order is: model `:suffix` > caller param > persona > session
        // > off. A `:suffix` still wins downstream because `apply_thinking_suffix` never replaces
        // an existing recognized suffix without a fork override's licence.
        if agent.thinking.is_none() {
            agent.thinking = self.inherited_session_thinking.clone();
        }
        // Per-step tri-state tool override (func-SA §4.2): `Some(_)` overrides the persona's own
        // allowlist, `None` defers to the persona (which is exactly what `to_agent_config` already
        // copied in). Same shape pi's `resolveStepBehavior` applies for a step-level tool override.
        if step.tools.is_some() {
            agent.tools = step.tools.clone();
        }
        // A per-step depth-ceiling override tightens the agent's own declared ceiling further; when
        // absent, the persona's own `max_subagent_depth` stands. `next_envelope` (at the spawn
        // boundary, `exec::mod`) applies the tightening-only `min()` against the inherited ceiling.
        if step.max_depth_override.is_some() {
            agent.max_subagent_depth = step.max_depth_override;
        }

        // Model-fallback ladder inputs, mirroring the single-run path (`extension.rs::run_foreground`):
        // a per-step `model` override wins (Explicit); else a persona `model:` is primary (Inherit);
        // else the live PARENT session model is inherited (pi `ctx.model`); else the ladder falls
        // through to the persona's own `fallback_models`. `available_models` is the union the
        // availability filter selects from — the persona's fallback ladder + its own model + any
        // per-step override + (when inheriting) the parent session model — so a persona with a real
        // configured model, OR an inheriting persona under a live parent, yields a non-empty ladder
        // without any `--model default` placeholder ever being synthesized (the C13/inheritance
        // defect). `self.inherited_session_model` is `None` for a headless runner or one launched
        // before any model was active, which degrades to the persona's own models exactly as before.
        let mut available_models: Vec<cyrup_core::ModelId> = agent.fallback_models.clone();
        available_models.extend(agent.model.clone());
        if let Some(step_model) = &step.model {
            available_models.push(step_model.clone());
        }
        // SUBA-003 fail-closed gate, per step: a chain/parallel step's own `model:` is an EXPLICIT
        // caller-supplied model (pi `chain-execution.ts:1118` passes `source: explicitStepModel ?
        // "explicit" : "inherited"`), so one outside `subagents.modelScope` FAILS this step with
        // pi's verbatim message rather than silently running some allowed model instead. A step
        // failure — not a `SubagentError` — because that is how this executor reports every other
        // pre-spawn rejection (`Unknown agent: …` directly above), keeping the run's own status
        // record and the surrounding chain semantics intact.
        let model_override = match crate::exec::fallback::resolve_model_inheritance(
            step.model.as_ref(),
            agent.model.as_ref(),
            self.inherited_session_model.as_ref(),
            &mut available_models,
            self.model_scope.as_ref(),
        ) {
            Ok(resolved) => resolved,
            Err(violation) => return Err(Box::new(StepResult::failure(violation.message))),
        };

        // SUBA-N04: lower THIS step's declared acceptance contract (pi `chain-execution.ts:400`
        // `acceptance: task.acceptance` for a parallel task / `:1335` `acceptance:
        // seqStep.acceptance` for a sequential step — both handed straight into the same `runSync`
        // call the SINGLE path uses) through the SAME
        // `exec::acceptance::lower_acceptance_input` the `subagent` tool's SINGLE-mode `acceptance`
        // param goes through (`extension.rs::route_single`). `run_sync` then resolves the effective
        // contract (R-SA-023), injects the `## Acceptance Contract` block into the task text,
        // EXECUTES any declared `verify[]` command as a real subprocess (R-SA-032/DI-SA-5), and —
        // because an explicitly-declared contract sets `AcceptanceContract::explicit` — applies
        // R-SA-033's post-hoc exit-code correction, so a rejected gate turns this step's `exit_code`
        // nonzero and therefore its `StepResult` into a FAILURE below.
        //
        // This field was previously a hard `None`. A chain/parallel/background step that declared
        // `acceptance` was parsed, carried all the way here, and then discarded with no warning: the
        // step ran completely UNVERIFIED and reported success on the exact same path an accepted run
        // reports it — silent, unlike a refusal, and reachable through the `tasks:[{…}]` surface
        // SUBA-041 documents as the workaround for the background SINGLE surface.
        //
        // A malformed policy FAILS the step (pi's own verbatim `validateAcceptanceInput` message)
        // rather than degrading to "no contract" — the same fail-closed choice the `modelScope`
        // violation directly above makes, and for the same reason: silently running a gate-less
        // child is the defect, not the remedy. The tool boundary
        // (`extension.rs::execute` -> `validate_execution_acceptance`, pi
        // `subagent-executor.ts:1757`) normally refuses such a policy before any child spawns; this
        // is the last line of defence for a step reaching the runner from a config file that was
        // hand-edited after validation.
        let acceptance = match step.acceptance.as_ref() {
            Some(raw) => match crate::exec::acceptance::lower_acceptance_input(raw) {
                Ok(contract) => contract,
                Err(message) => {
                    return Err(Box::new(StepResult::failure(format!(
                        "subagent step '{}' has an invalid acceptance policy: {message}",
                        step.agent
                    ))));
                }
            },
            None => None,
        };

        Ok(StepAgentSetup {
            agent,
            available_models,
            model_override,
            acceptance,
        })
    }

    /// Lower this step's spec and the chain-run context into the [`RunOptions`] `exec::run_sync`
    /// consumes, together with the four per-dispatch inputs that exist only for the duration of one
    /// dispatch and have nowhere else to live: the run-wide interrupt token, the live-telemetry
    /// sink, the fork context, and the step's effective cwd (from which its `output` path is
    /// resolved).
    ///
    /// The model ladder and the acceptance contract are passed in rather than re-derived here:
    /// [`Self::build_step_agent_config`] decides them, and its two fail-closed gates have to have
    /// run before any of this.
    ///
    /// Deliberately NOT shared with the SINGLE path's own `RunOptions` literal
    /// (`extension::executor::foreground::run_foreground_impl`): that one is assembled on a
    /// different type, from a different input set (the tool call's `overrides`, a live
    /// `ClarifyDispatch`, a control-notice sink, `steer_*: None`), and the two agree on the struct
    /// alone — a shared builder would have to reproduce both field-by-field with nothing left in
    /// common.
    fn build_step_run_options(
        &self,
        step: &SingleStepSpec,
        ctx: &ChainRunContext,
        available_models: Vec<cyrup_core::ModelId>,
        model_override: crate::exec::fallback::ModelOverride,
        acceptance: Option<crate::exec::acceptance::AcceptanceContract>,
    ) -> RunOptions {
        // R-SA-084 mid-flight interrupt (C, `subagent-runner.ts:1333,2002-2005,2069` @v0.34.0): clone the
        // run-wide SHARED interrupt token so an interrupt landing WHILE this child is running (the
        // control-inbox watcher cancels `self.interrupt_cancel`) actually tears the child down via
        // `run_sync`'s `opts.interrupt` race — not merely gets noticed between steps. Previously a
        // fresh per-step token was cancelled only if an interrupt had ALREADY landed at dispatch
        // time, so interrupting a single-step run was a total no-op (the child ran to completion).
        //
        // SUBA-087: when `run_inner` registered a per-step stop handle for the step being
        // dispatched (a child token of that same run-wide token), THAT is the child's interrupt
        // token, so a child-scoped stop cancels this child alone while every run-wide verb still
        // reaches it through the parent (pi hands `runSubagentProcess` both `stopSignal` and
        // `registerStop`, `subagent-runner.ts:4268-4270`).
        //
        // SUBA-093: the handle is looked up under THIS dispatch's own flat slot, which
        // `dispatch_group` stamps per member — so a child-scoped stop aimed at one member of a
        // `tasks[]` fan-out cancels that member alone and its siblings run on.
        let interrupt_token = self
            .child_stops
            .as_ref()
            .and_then(|registry| registry.active_token(ctx.step_slot.index()))
            .unwrap_or_else(|| self.interrupt_cancel.clone());
        if self.interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            interrupt_token.cancel();
        }

        // Live telemetry (pi's child-event pump, `subagent-runner.ts:1430-1517`): if this is the
        // detached hop-2 runner (a telemetry channel is installed), publish THIS step's flat index
        // and hand `run_sync` a raw-line sink that forwards every child NDJSON line — tagged with
        // that index — to the runner's telemetry task, which folds it into `status.json`.
        // The flat index this sink tags events with is published by `run_inner` into
        // `self.current_flat_index` immediately before each dispatch (a `SingleStepSpec` carries no
        // index of its own), so the sink reads the CURRENT step's index at event time.
        let live_events = self.telemetry.as_ref().map(|sender| {
            let sender = sender.clone();
            let flat_index = ctx.step_slot.index();
            crate::exec::LiveEventSink::new(move |raw: &str| {
                let _ = sender.send(TelemetryMsg {
                    flat_index,
                    raw: raw.to_string(),
                });
            })
        });

        let fork_context = match &step.session_file {
            Some(path) => ForkContext {
                mode: ContextMode::Fork,
                session_file_path: Some(path.clone()),
                // SUBA-075: hop 1 resolved and sanitized this branch, but `SingleStepSpec` carries
                // only its PATH across the hand-off, so any thinking override it resolved is not
                // recoverable here. Reconstructing it would mean widening the runner config — the
                // async half of SUBA-075, filed separately. `None` is what hop 2 can honestly say.
                thinking_override: None,
            },
            None => ForkContext::fresh(),
        };

        let effective_cwd = step.cwd.clone().unwrap_or_else(|| ctx.cwd.clone());
        // File-output handoff wiring (Tier-2): resolve this step's `output` FILE path (relative
        // against the step's effective cwd, absolute used verbatim — pi's `resolveSingleOutputPath`
        // fallback, `single-output.ts:64-77`) and hand it to `run_sync`, so `exec/output.rs`'s
        // stat-snapshot handoff runs and the saved-output reference message is emitted. Previously
        // hard-`None`, which is exactly why the whole file-output path was dead code.
        let output_path = step.output_path.as_deref().map(|raw| {
            let candidate = std::path::Path::new(raw);
            if candidate.is_absolute() {
                candidate.to_path_buf()
            } else {
                effective_cwd.join(candidate)
            }
        });
        RunOptions {
            spawn_command: self.spawn_command.clone(),
            child_env: self.child_env.clone(),
            // pi `hostAvailableBuiltins` — the LAUNCHING process's host observation, applied per
            // step exactly as the run-level budgets below are. The one lowering both feeders share:
            // the foreground `/chain`//`/parallel` walk sets the field from its live host, the
            // detached hop-2 runner from its `RunnerConfig`.
            host_available_builtins: self.host_available_builtins.clone(),
            // SUBA-021 — the RUN-level usage budget applied per step, exactly as `turn_budget`
            // below is (pi applies one `AsyncExecutionParams.usageBudget` across the whole run
            // rather than giving each step a fresh one).
            usage_budget: self.usage_budget,
            // SCOPE_3j — the detached hop-2 runner holds no executor to inherit a store from, so it
            // builds its own from the environment. That is not a second registry: it resolves to
            // the SAME on-disk file the foreground wrote, which is the whole reason the store is
            // persisted rather than merely in-process.
            model_exclusions: Some(std::sync::Arc::new(
                crate::exec::model_exclusions::ModelExclusionStore::from_env(),
            )),
            // SUBA-008 — pi `turnBudget: ctx.turnBudget` on every step's `runSubagentProcess`
            // call (`subagent-runner.ts:1409`): the RUN-level budget, applied per step, exactly
            // as upstream applies one `AsyncExecutionParams.turnBudget` to every step of an async
            // chain rather than giving each step a fresh one.
            turn_budget: self.turn_budget,
            // SUBA-073 — the RUN-level, fully-merged permission policy, applied per step, exactly
            // as `turn_budget` immediately above.
            permission_rules: self.permission_rules.clone(),
            // pi's `enforceHardTurnLimit` reaches `runSubagentProcess` only from the slash
            // delegation adapter (`slash/delegation-adapters.ts:298`); the async runner never sets
            // it, so the mid-tool-work deferral stays armed here as upstream leaves it.
            enforce_hard_turn_limit: false,
            cwd: effective_cwd,
            deadline_at: ctx.deadline_at,
            // pi `chain-execution.ts:335-336,741-742,1197-1198` @v0.34.0: every step's `runSync` call carries BOTH
            // the chain-wide `deadlineAt` (raced against) and the nominal `timeoutMs` (only used to
            // render the timed-out message) — the same two values for every step, never re-derived
            // per step.
            timeout_ms: ctx.timeout_ms,
            // SUBA-003: carried into `run_sync` so this step's fallback ladder warns on out-of-scope
            // entries, the same way the foreground single-run path does. The step's explicit
            // `model:` was already hard-gated by `build_step_agent_config`.
            model_scope: self.model_scope.clone(),
            output_path,
            output_mode: step
                .output_mode
                .unwrap_or(crate::discovery::types::OutputMode::Inline),
            // SUBA-054 residual, stated rather than silently defaulted: a step dispatched through
            // this runner already gets its `[Read from: …]` line from
            // `spawn::chain_graph::build_chain_instructions`, which resolves `step.reads` against
            // the CHAIN dir. Populating `RunOptions::reads` here as well would emit the line TWICE
            // for every chain step. Upstream's async single path resolves against `effectiveCwd`
            // (`async-execution.ts:1300-1302`), so closing the async half means teaching the step
            // builder which of the two cwds applies — not setting this field.
            reads: None,
            structured_output_schema: step.structured_output_schema.clone(),
            model_override,
            // SUBA-078: hop 2 does not re-read settings — its ceiling arrives through the
            // `CYRUP_SUBAGENT_THINKING_CEILING` env var hop 1 wrote, and `run_sync` folds that
            // inherited value in. `None` here is "nothing beyond what the environment says".
            thinking_ceiling: None,
            // SUBA-088 / pi `currentModelProvider: parentModel?.provider`
            // (`subagent-executor.ts:1297` @v0.64.0, consumed at `async-execution.ts:930` as
            // `a.modelProvider ?? ctx.currentModelProvider`): the parent session's provider, split
            // off the SAME `inherited_session_model` the inheritance rung above used, so a step whose
            // persona names a bare model id is qualified against it before spawn.
            preferred_provider: self
                .inherited_session_model
                .as_ref()
                .and_then(crate::exec::fallback::provider_of),
            available_models,
            cancel: ctx.cancel.clone(),
            interrupt: interrupt_token,
            // SUBA-N03 — pi `share: shareEnabled` (`async-execution.ts:965`) reaching this run's
            // children as one of the two `sessionEnabled` terms (`execution.ts:1027,1039` @v0.34.0). Carried
            // from `RunnerConfig::share`; `None` is "omitted", which is NOT enabling.
            share: self.share,
            // SUBA-N03 — this step's own already-resolved session directory (pi's `--session-dir`,
            // `runs/shared/pi-args.ts:109-111`). Resolved PARENT-side and carried on the step rather than
            // derived here from a run-level root: see `SingleStepSpec::session_dir`'s
            // [CYRUP-DELTA] note for why an index-derived path would be unsafe at this seam.
            session_dir: step.session_dir.clone(),
            // SUBA-N03 — this step's own SKILL override (pi's runner-step `skills`,
            // `subagent-runner.ts:872` ← `async-execution.ts:990`). `run_sync` applies pi's
            // `opts.skills ?? agent.skills` fallthrough, so `None` still defers to the resolved
            // persona's own `skills:` list (carried on the `AgentConfig` `build_step_agent_config`
            // returns) and `Some(vec![])` is the explicit `skill: false` "no skills" form. The
            // orchestrator/runtime fallback cwd is not threaded through the one-shot runner config,
            // so a background step resolves skill NAMES against its own step cwd.
            skills: step.skills.clone(),
            runtime_cwd: None,
            // SUBA-N06: the run's `includeProgress`, carried from `RunnerConfig::include_progress`
            // through this executor, so a background step's persisted `SingleResult` carries the
            // same progress snapshot the foreground path returns.
            include_progress: self.include_progress,
            agent_scope: step.agent_scope,
            // SUBA-N04: the step's own lowered contract (resolved by `build_step_agent_config`),
            // NOT a hard `None`.
            acceptance,
            fork_context,
            live_events,
            // R-SA-P1 / PERM-001: the anchor the hop-1 spawn injected into THIS runner's own
            // environment (`background::spawn_detached`'s `env_overlay`, sourced from
            // `background::parent_anchor::detached_runner_env_overlay`), resolved explicitly here
            // and threaded on rather than left to the spawn site's fallback.
            //
            // The comment this replaces asserted that the runner "inherited
            // `CYRUP_SUBAGENT_PARENT_SESSION` in its OWN env from the hop-1 spawn" — which was
            // simply untrue: until PERM-001 the hop-1 spawn added NO env overlay whatsoever, and
            // the only writer of that variable anywhere in the workspace is
            // `exec::build_attempt_spawn_plan`, which no process ever runs against itself. A root
            // orchestrator's background run therefore reached here with no anchor in scope, every
            // hop-3 child was spawned without one, and `cyrup-permission-system`'s child gate
            // fail-closed denied every `ask` against a null forwarding target with no prompt ever
            // shown to the operator. Hop 1 now really does inject it, so the claim is finally true
            // — and this call site states the dependency instead of assuming it.
            parent_session_id: crate::background::parent_anchor::resolve_parent_session_anchor(),
            // The detached hop-2 runner has no live orchestrator human session to surface a clarify
            // ask to; a child's blocking `contact_supervisor` ask routes over the broker to whichever
            // supervisor its intercom metadata names, not through this headless runner's exec loop.
            clarify: None,
            // Intercom child-bridge activation (pi `subagent-runner.ts:779-783`): thread the
            // launching orchestrator's presence target + this run's id + THIS step's flat index so the
            // spawned child registers `contact_supervisor` (addressed at that supervisor) + a broker
            // presence under `resolve_subagent_intercom_target(run_id, step.agent, flat_index)` — the
            // SAME string `control_resume`'s `SteerRunning` arm recovers from `status.steps[index]` to
            // steer this child. The flat index is the one `run_inner` publishes into
            // `current_flat_index` immediately before each dispatch (a `SingleStepSpec` carries none
            // of its own), matching the `status.steps` position the steer path indexes by.
            orchestrator_intercom_target: self.orchestrator_intercom_target.clone(),
            run_id: self.run_id.clone(),
            child_index: Some(ctx.step_slot.index()),
            // G90 (pi `steerInboxDir: stepSteerInboxDir(asyncDir, fi)`,
            // `subagent-runner.ts:2313,2600,2797` @v0.34.0): THIS step's own per-child steer inbox,
            // handed to the spawned child so its live steering watcher has a path to attach to. The
            // index is the same `current_flat_index` `child_index` above uses — the position the
            // runner's own `deliver_steer_request` routes an accepted request to
            // (`control::enqueue_step_steer`), so the two halves of the hop address the same
            // directory by construction. `None` for a foreground executor (no async run dir).
            steer_inbox_dir: self.steer_inbox_for(ctx.step_slot.index()),
            // SUBA-049: the return path, keyed off the SAME flat index as the inbox above — see
            // `steer_ack_dir_for`'s doc for why the derivation is shared rather than re-written.
            steer_ack_dir: self.steer_ack_dir_for(ctx.step_slot.index()),
            steer_capability_path: self.steer_capability_path_for(ctx.step_slot.index()),
            // SUBA-N05: the run's resolved live-control config, threaded from
            // [`RunnerConfig::control`] (background) or [`ExecSingleStepExecutor::with_control`]
            // (foreground chain/parallel) — pi `controlConfig: input.controlConfig` on the
            // per-step `runSync` call (`chain-execution.ts:322,491,733` @v0.34.0), and
            // `config.controlConfig ?? DEFAULT_CONTROL_CONFIG` in the async runner
            // (`subagent-runner.ts:1802`). `None` still degrades to `DEFAULT_CONTROL_CONFIG` inside
            // `run_sync`, so an omitted config keeps control tracking ON with stock thresholds
            // rather than turning it off.
            control_config: self.control.clone(),
            // No live notice SINK on this path: the detached runner has no orchestrator transcript
            // to inject into, and a foreground chain/parallel walk's notices are surfaced by the
            // parent from `SingleResult::control_events`. Events are still RAISED — they land on
            // each step's `SingleResult::control_events` and travel back in the result file — which
            // is what `notifyChannels: ["async"]` describes upstream, where the runner appends them
            // to the async dir's control-event log for the parent tracker to replay
            // (`subagent-runner.ts:2270-2280` → `async-job-tracker.ts:138-166` @v0.34.0). That
            // replay hop is not ported; the events themselves are not lost.
            on_control_event: None,
            // G80 — pi `artifactsDir: ctx.artifactsDir` on the background hop's own
            // `evaluateAcceptance` call (`runs/background/subagent-runner.ts:1638-1639` @v0.43.0),
            // which is how a step's verify[] results get memoized under
            // `<artifactsDir>/acceptance/verify/<runId>/`. Gated by the SAME two-term gate every
            // other artifact write on this hop uses (`ctx.artifactsDir && ctx.artifactConfig
            // ?.enabled !== false`, `subagent-runner.ts:1192`), so `artifacts: false` disarms
            // memoization along with the quadruple.
            artifacts_dir: self
                .artifacts_dir
                .clone()
                .filter(|_| self.artifact_config.enabled),
            // pi `createStructuredOutputRuntime(step.structuredOutputSchema,
            // path.join(path.dirname(ctx.outputFile), "structured-output"), …)`
            // (`subagent-runner.ts:783-785`): a dispatched step's structured capture is RUN-SCOPED
            // and deliberately never swept, so the published
            // `SingleResult::structured_output_path` stays resolvable off the terminal
            // `ResultFile` long after the run ended. This port's run-scoped durable root on this
            // seam is the artifacts dir, under the SAME two-term gate as `artifacts_dir` above —
            // upstream's own foreground sweep is conditional on exactly that gate
            // (`subagent-executor.ts:3986`/`:4014`), so an artifacts-disabled run falls back to
            // the swept foreground scratch policy rather than leaking captures.
            structured_output_dir: self
                .artifacts_dir
                .clone()
                .filter(|_| self.artifact_config.enabled)
                .map(|dir| dir.join("structured-output")),
        }
    }

    /// SUBA-N03 / T6 on the SECOND hop — pi `runs/background/subagent-runner.ts:877-889`
    /// @v0.34.0: the artifact quadruple is written by the ASYNC runner too, not only by the
    /// foreground path, and its `_input.md` is written BEFORE the child spawns (`:882-885`,
    /// `mkdirSync` then `writeFileSync(inputPath, …)`) precisely so a child that crashes still
    /// leaves a record of what it was asked to do. The gate is pi's own two-term one:
    /// `ctx.artifactsDir && ctx.artifactConfig?.enabled !== false` (`:879`) — an absent dir is
    /// exactly as disabling as `enabled: false`, which is how the SINGLE-mode `artifacts: false`
    /// param reaches this hop.
    ///
    /// Best-effort throughout: a failed artifact write must never alter the `StepResult` the
    /// walker observes, matching pi (whose artifact writes are un-guarded side-effects) and the
    /// foreground path's identical convention.
    ///
    /// Index: pi passes the step's own index into `getArtifactPaths` so a chain's steps do not
    /// overwrite each other's files. `index` is this dispatch's own flat slot (SUBA-093) — the
    /// SAME index `RunOptions::child_index` uses, and per MEMBER inside a `ParallelGroup`, so two
    /// concurrently-running siblings no longer write the same artifact quadruple.
    fn write_step_input_artifact(
        &self,
        step: &SingleStepSpec,
        resolved_task: &str,
        index: usize,
    ) -> Option<(crate::artifacts::ArtifactPaths, String)> {
        self.artifacts_dir
            .as_ref()
            .filter(|_| self.artifact_config.enabled)
            .map(|dir| {
                let run_token = self
                    .run_id
                    .as_ref()
                    .map_or("run", RunId::as_str)
                    .to_string();
                let paths =
                    crate::artifacts::artifact_paths(dir, &run_token, &step.agent, Some(index));
                let _ = crate::artifacts::ensure_artifacts_dir(dir);
                if self.artifact_config.include_input {
                    let _ = crate::artifacts::write_artifact(
                        &paths.input_path,
                        &format!("# Task for {}\n\n{resolved_task}", step.agent),
                    );
                }
                (paths, run_token)
            })
    }

    /// SUBA-N03 / T6: the after-run half (pi `subagent-runner.ts:1117-1134` — `_output.md`,
    /// `_meta.json`, and this crate's reconstructed `.jsonl`). Shares ONE implementation with
    /// the foreground path via `artifacts::run_artifact_metadata`/`run_artifact_jsonl_lines`,
    /// so an async run's artifacts are byte-shaped identically to a foreground run's rather
    /// than being a second, drifting hand-rolled emitter.
    fn write_step_result_artifacts(
        &self,
        artifact_paths: Option<&(crate::artifacts::ArtifactPaths, String)>,
        result: &SingleResult,
    ) {
        if let Some((paths, run_token)) = artifact_paths {
            if self.artifact_config.include_output {
                let _ = crate::artifacts::write_artifact(
                    &paths.output_path,
                    result.final_output.as_deref().unwrap_or(""),
                );
            }
            if self.artifact_config.include_metadata {
                let _ = crate::artifacts::write_metadata(
                    &paths.metadata_path,
                    &crate::artifacts::run_artifact_metadata(run_token, result),
                );
            }
            if self.artifact_config.include_jsonl {
                for line in crate::artifacts::run_artifact_jsonl_lines(result) {
                    let _ = crate::artifacts::append_jsonl(&paths.jsonl_path, &line);
                }
            }
        }
    }
}

/// Deregisters one step's live child-stop handle when the dispatch it belongs to ends — however
/// it ends. See its only construction site in [`ExecSingleStepExecutor::run_single`].
struct ActiveStopGuard<'a> {
    registry: &'a crate::background::child_stop::ChildStopRegistry,
    index: usize,
}

impl Drop for ActiveStopGuard<'_> {
    fn drop(&mut self) {
        self.registry.clear_active(self.index);
    }
}

#[async_trait::async_trait]
impl SingleStepExecutor for ExecSingleStepExecutor {
    async fn run_single(
        &self,
        step: &SingleStepSpec,
        resolved_task: &str,
        ctx: &ChainRunContext,
    ) -> Result<StepResult, SubagentError> {
        let StepAgentSetup {
            agent,
            available_models,
            model_override,
            acceptance,
        } = match self.build_step_agent_config(step) {
            Ok(setup) => setup,
            Err(rejection) => return Ok(*rejection),
        };

        // SUBA-093 / SUBA-087 — the child-scoped stop handle is registered HERE, per dispatch,
        // which is where pi registers it too: `registerStop: (stop) => registerStepStop(fi, stop)`
        // appears at all three of pi's dispatch sites (`subagent-runner.ts:4268` parallel, `:4667`
        // dynamic, `:5034` sequential @v0.64.0), each with its OWN `fi`. Registering per top-level
        // step instead — which is what this runner did before this item — gave every member of a
        // `tasks[]` fan-out the same handle, so stopping one member stopped all of them.
        //
        // Only an EXCLUSIVE slot registers: a dynamic group's members still share one flat slot
        // (a recorded SUBA-093 residual), and two live children under one index would let a stop
        // aimed at either tear down whichever registered last.
        let stop_slot = self
            .child_stops
            .as_ref()
            .zip(ctx.step_slot.exclusive_index());
        if let Some((registry, index)) = stop_slot {
            // pi `if (childStopRequests.has(fi)) return childStopResult(fi, …)` immediately ahead
            // of each dispatch (`:4221`, `:4604`, `:4937`): a stop queued against a member that
            // has not started yet is applied without ever spawning a child.
            if registry.is_requested(index) {
                return Ok(child_stopped_step_result());
            }
            // A child token of the run-wide interrupt token: a run-wide stop/interrupt/timeout
            // still reaches this child through the parent, a child-scoped stop cancels it alone.
            registry.register_active(index, self.interrupt_cancel.child_token());
        }
        // pi `registerStepStop(flatIndex, undefined)` (`:3049-3052`): this child is gone, so a
        // later child-scoped stop against its index is `stop_failed`, not a cancel of a token
        // nothing is listening to. A GUARD rather than a statement after the await, because this
        // future is dropped rather than completed whenever a fan-out sibling fail-fasts or the
        // group is cancelled — and the plain statement never ran on that path, leaving a dead
        // token registered under a live index for the rest of the run.
        let _active = stop_slot.map(|(registry, index)| ActiveStopGuard { registry, index });

        let opts =
            self.build_step_run_options(step, ctx, available_models, model_override, acceptance);

        let artifact_paths =
            self.write_step_input_artifact(step, resolved_task, ctx.step_slot.index());

        let result = exec::run_sync(&agent, resolved_task, &opts).await;

        self.write_step_result_artifacts(artifact_paths.as_ref(), &result);

        // SUBA-093 — a child torn down by ITS OWN child-scoped stop reports pi's stopped result
        // (`exitCode: 1`), not the paused-success (exit 0) an interrupt yields:
        // `requiredStatusStep(fi).exitCode = stopped || childStopped ? 1 : …` and the matching
        // `singleResult` (`subagent-runner.ts:4286-4295` @v0.64.0). Without this, a stopped MEMBER
        // of a fan-out came back successful, its group's aggregate stayed successful, and a run
        // whose member the user explicitly stopped ended `Complete`. The whole-run verbs are
        // unaffected: they cancel through the parent token and leave nothing recorded here.
        if stop_slot.is_some_and(|(registry, index)| registry.is_requested(index)) {
            return Ok(child_stopped_step_result());
        }

        Ok(build_step_result(
            &agent.name,
            result,
            artifact_paths.as_ref(),
        ))
    }
}

/// R-SA-084: carry the mid-flight interrupt flag up so `run_inner` treats an interrupted
/// step as the pause point (`Paused`, not `Complete`). An interrupted `run_sync` reports
/// `exit_code == 0` (pi's paused-success), so it maps to `StepResult::success` here, with
/// `interrupted` set from the winning attempt's own flag.
fn build_step_result(
    agent_name: &str,
    result: SingleResult,
    artifact_paths: Option<&(crate::artifacts::ArtifactPaths, String)>,
) -> StepResult {
    // Destructured ONCE, up front: the `success`/`failure` constructors below consume
    // `final_output`/`structured_output`/`error` by value, and while fields were read off `result`
    // piecemeal a future addition could silently read a moved-out value. With the destructure the
    // compiler owns that ordering problem.
    let SingleResult {
        exit_code,
        usage,
        turns,
        model,
        attempted_models,
        final_output,
        structured_output,
        session_file,
        output_state,
        structured_output_path,
        error,
        interrupted,
        timed_out,
        timeout_recovery,
        context_overflow,
        saved_output_path,
        control_events,
        ..
    } = result;
    let mut step_result = if exit_code == 0 {
        StepResult::success(final_output, structured_output)
    } else {
        StepResult::failure(error.unwrap_or_else(|| {
            format!("subagent step '{agent_name}' exited with code {exit_code}")
        }))
    };
    step_result.interrupted = interrupted;
    // Carry the per-child detail pi's `collectDynamicResults` copies verbatim onto a dynamic
    // fan-out's collect records (`runs/shared/dynamic-fanout.ts:278-284` @v0.34.0). All four
    // are known HERE and nowhere upstream of here: the walker sees only `StepResult`, so
    // without this hop a timed-out child is indistinguishable from an ordinary failure, every
    // failure reports exactly `1` rather than its real code, and a later chain step cannot
    // locate the files its fanned-out siblings wrote.
    step_result.exit_code = Some(exit_code);
    step_result.timed_out = timed_out;
    // SUBA-3c: the deadline kill's recovery evidence rides the same waist as `timed_out` — pi's
    // status write (`subagent-runner.ts:4645`) and chain-results copy (`:4946`) both read it off
    // the step's own `singleResult`, and the trailing `..` in the destructure above means nothing
    // enforces this line either: dropping it builds clean and silently loses the evidence.
    step_result.timeout_recovery = timeout_recovery;
    // pi copies `contextOverflow` onto the chain-results array (`subagent-runner.ts:4486`) and the
    // status payload (`:3668`/`:4083`/`:4570`) from the step's own `singleResult` — this hop is
    // the ONLY channel by which the ladder's terminal overflow classification reaches an async
    // run's `StepStatus`/`ResultFile`, and the trailing `..` in the destructure above means
    // nothing enforces it: dropping this line builds clean and silently reports `false`.
    step_result.context_overflow = context_overflow;
    step_result.saved_output_path = saved_output_path;
    // pi stamps `result.artifactPaths` from the bundle it computed for this same step
    // (`runs/foreground/execution.ts:1826-1830`, gated on the run having an artifacts dir at
    // all). The `artifact_paths` parameter is precisely that bundle, under precisely pi's gate
    // (`artifactsDir && artifactConfig?.enabled !== false`), so reuse it rather than
    // recomputing — a second `artifact_paths()` call would have to re-read `current_flat_index`
    // after it has already advanced. Assigned as the typed struct: the untyped `to_value` hop
    // this replaces is what let the four/five field divergence go unnoticed.
    step_result.artifact_paths = artifact_paths.map(|(paths, _)| paths.clone());
    // SUBA-N05: carry the events this step's control monitor raised out of `run_sync` so
    // `step_result_to_single_result` can put them on the terminal `ResultFile`. Without this
    // hop the whole async control path is inert: the thresholds are honoured, the events are
    // raised, and then they die here.
    step_result.control_events = control_events;
    // The telemetry the waist used to drop (the same "known HERE and nowhere upstream of here"
    // rationale as the four fields above): without this hop `step_result_to_single_result` had
    // nothing to write but `Usage::default()`/`None`, so every async run's terminal `ResultFile`
    // reported zero tokens, zero cost, zero turns and no model for every child.
    step_result.usage = usage;
    step_result.turns = turns;
    step_result.model = model;
    step_result.attempted_models = attempted_models;
    step_result.session_file = session_file;
    step_result.output_state = output_state;
    step_result.structured_output_path = structured_output_path;
    step_result
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::tests::single_step;
    use super::*;
    use crate::spawn::parallel::GlobalConcurrencyLimit;

    // ---------------------------------------------------------------------------------------
    // T0.1 / C13: an unresolved step agent is dispatched as `Unknown agent: <name>` (a step
    // FAILURE) BEFORE any spawn setup — never silently downgraded to a placeholder persona.
    // Provable without the fixture binary: the persona-map miss short-circuits ahead of every
    // filesystem side effect (`run_sync`'s scratch-dir creation, the first thing any real spawn
    // attempt does), mirroring pi's `agents.find(...)` miss returning `Unknown agent`
    // (`chain-execution.ts:1011-1019`).
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn run_single_rejects_an_unresolved_agent_as_unknown_before_any_spawn() {
        let dir = tempfile::tempdir().expect("real tempdir");
        // The executor carries an EMPTY persona map — exactly the state that must NOT dispatch a
        // placeholder.
        let executor = ExecSingleStepExecutor {
            spawn_command: None,
            child_env: std::collections::HashMap::new(),
            host_available_builtins: None,
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt_cancel: cyrup_core::CancelToken::new(),
            child_stops: None,
            telemetry: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            resolved_agents: Arc::new(BTreeMap::new()),
            orchestrator_intercom_target: None,
            run_id: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            model_scope: None,
            control: None,
            include_progress: None,
            run_dir: None,
        };
        let ctx = ChainRunContext {
            cwd: dir.path().to_path_buf(),
            deadline_at: None,
            timeout_ms: None,
            cancel: cyrup_core::CancelToken::new(),
            global_limit: GlobalConcurrencyLimit::new(4),
            worktree_base_dir: None,
            original_task: String::new(),
            chain_dir: None,
            dynamic_fanout_max_items: None,
            step_slot: crate::spawn::chain_graph::StepSlot::Exclusive(0),
            // No worktree group in this context, so nothing publishes a handoff manifest.
            handoff: None,
        };
        let step = single_step("nonexistent-reviewer", "review the change");

        let result = executor
            .run_single(&step, "review the change", &ctx)
            .await
            .expect("run_single itself returns Ok, carrying the step-level failure in StepResult");

        assert!(
            !result.success,
            "an unresolved agent must be a step failure: {result:?}"
        );
        assert!(
            result
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("Unknown agent: nonexistent-reviewer"),
            "expected an `Unknown agent` failure naming the missing persona, got: {:?}",
            result.error
        );
        assert!(
            !crate::background::attempt_scratch_dir(dir.path()).exists(),
            "an unresolved-agent rejection must happen before run_sync's spawn-scratch dir is ever \
             created — proving no placeholder child was ever spawned"
        );
    }

    /// SUBA-093 review fix — the live child-stop handle is deregistered even when the dispatch's
    /// future is DROPPED rather than run to completion, which is what happens to every sibling of
    /// a fail-fast member inside `spawn::parallel::run_bounded`.
    ///
    /// Before the guard the `clear_active` call sat after the `run_sync` await, so a dropped
    /// member left a dead token registered under a live flat index for the rest of the run.
    #[test]
    fn a_dropped_dispatch_still_deregisters_its_child_stop_handle() {
        let registry = crate::background::child_stop::ChildStopRegistry::new();
        registry.register_active(2, cyrup_core::CancelToken::new());
        assert!(registry.active_token(2).is_some());
        {
            let _guard = ActiveStopGuard {
                registry: &registry,
                index: 2,
            };
        }
        assert!(
            registry.active_token(2).is_none(),
            "dropping the dispatch must clear the handle"
        );
        // pi `stopChildStep`'s "was there a live child?" answer is now `false`, so a later
        // child-scoped stop against this index is `stop_failed` rather than a cancel of a token
        // nothing is listening to.
        assert!(!registry.cancel_active(2));
    }

    /// SUBA-3c — the recovery summary survives the whole async waist:
    /// `run_sync`'s `SingleResult` → `build_step_result` (the `..`-destructure hop nothing
    /// enforces) → `record_step_outcome` (the status surface) → `step_result_to_single_result`
    /// (the terminal `ResultFile` surface). Upstream's same chain: `subagent-runner.ts:1616` →
    /// `:4645` → `:4946`.
    #[test]
    fn timeout_recovery_survives_the_step_result_waist_end_to_end() {
        let evidence = crate::exec::mutation_evidence::TrackedMutationEvidence {
            source: Default::default(),
            tracked_only: true,
            changed_files: vec!["src/half-written.rs".to_string()],
            attempted_mutation: true,
            truncated: false,
            unavailable: None,
        };
        let summary = crate::exec::mutation_evidence::build_timeout_recovery_summary(
            crate::exec::mutation_evidence::TimeoutRecoveryInput {
                termination: crate::exec::mutation_evidence::Termination::TimedOut,
                evidence: &evidence,
                required_output_missing: Some(true),
                current_tool: Some("edit"),
                current_tool_args: None,
                current_path: None,
                session_file: None,
                transcript_path: None,
                artifact_paths: None,
            },
        );

        let mut single = super::super::settle::stopped_single_result(
            &crate::spawn::chain_graph::RunnerStep::SingleStep(single_step(
                "coder",
                "write the module",
            )),
        );
        single.stopped = false;
        single.timed_out = true;
        single.timeout_recovery = Some(summary.clone());

        // Hop 1: the `..`-destructure in `build_step_result`.
        let step_result = build_step_result("coder", single, None);
        assert_eq!(step_result.timeout_recovery.as_ref(), Some(&summary));

        // Hop 2: the status surface (`record_step_outcome`'s single-slot arm).
        let step = crate::spawn::chain_graph::RunnerStep::SingleStep(single_step(
            "coder",
            "write the module",
        ));
        let mut status = crate::background::RunStatus::queued(
            RunId::from_token("run-3c-test".to_string()),
            crate::background::RunMode::Single,
            Some(1),
        );
        status.steps = vec![crate::background::StepStatus::pending("coder")];
        super::super::status::record_step_outcome(&mut status, &(0..1), &step, &step_result, None);
        assert_eq!(
            status
                .steps
                .first()
                .and_then(|s| s.timeout_recovery.as_ref()),
            Some(&summary),
            "the status step carries the FULL summary (pi `shared/types.ts:1917`)"
        );

        // Hop 3: the terminal `ResultFile` surface.
        let terminal = super::super::settle::step_result_to_single_result(&step, &step_result);
        assert_eq!(terminal.timeout_recovery.as_ref(), Some(&summary));

        // And the wire: a `SingleResult` written BEFORE the field existed still decodes (`None`),
        // while a populated one round-trips.
        let mut legacy = serde_json::to_value(&terminal).expect("serialize");
        legacy
            .as_object_mut()
            .expect("object")
            .remove("timeoutRecovery")
            .expect("the populated field serializes under pi's key");
        let decoded: SingleResult = serde_json::from_value(legacy).expect("legacy decode");
        assert_eq!(decoded.timeout_recovery, None);
        let round: SingleResult =
            serde_json::from_value(serde_json::to_value(&terminal).expect("serialize"))
                .expect("round-trip");
        assert_eq!(round.timeout_recovery, Some(summary));
    }

    // ---------------------------------------------------------------------------------------
    // The host observation (pi `hostAvailableBuiltins`) reaches every dispatched step.
    //
    // `build_step_run_options` is the ONE lowering both of this executor's constructors share:
    // the foreground `/chain`//`/parallel` walk sets `host_available_builtins` from the live
    // host, the detached hop-2 runner from its `RunnerConfig`. Pinning it here therefore covers
    // both feeders at the single point where forgetting it would silently disarm the host
    // intersection for a whole chain.
    // ---------------------------------------------------------------------------------------

    /// A `ChainRunContext` sufficient to drive `build_step_run_options`.
    fn host_test_ctx(cwd: &std::path::Path) -> ChainRunContext {
        ChainRunContext {
            cwd: cwd.to_path_buf(),
            deadline_at: None,
            timeout_ms: None,
            cancel: cyrup_core::CancelToken::new(),
            global_limit: GlobalConcurrencyLimit::new(4),
            worktree_base_dir: None,
            original_task: String::new(),
            chain_dir: None,
            dynamic_fanout_max_items: None,
            step_slot: crate::spawn::chain_graph::StepSlot::Exclusive(0),
            // No worktree group in this context, so nothing publishes a handoff manifest.
            handoff: None,
        }
    }

    /// The hop-2 feeder's half: an executor built as `turn_loop` builds it (a struct literal fed
    /// from `RunnerConfig`) forwards its observation onto every step's `RunOptions`.
    #[test]
    fn the_step_executor_forwards_the_observation_to_every_step() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let executor = ExecSingleStepExecutor {
            spawn_command: None,
            child_env: std::collections::HashMap::new(),
            host_available_builtins: Some(vec!["read".to_string()]),
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt_cancel: cyrup_core::CancelToken::new(),
            child_stops: None,
            telemetry: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            resolved_agents: Arc::new(BTreeMap::new()),
            orchestrator_intercom_target: None,
            run_id: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            model_scope: None,
            control: None,
            include_progress: None,
            run_dir: None,
        };

        let opts = executor.build_step_run_options(
            &single_step("reviewer", "review the change"),
            &host_test_ctx(dir.path()),
            Vec::new(),
            crate::exec::fallback::ModelOverride::Inherit,
            None,
        );

        assert_eq!(
            opts.host_available_builtins,
            Some(vec!["read".to_string()]),
            "the run-level observation must reach the step's RunOptions, or `resolve_tool_surface` \
             sees UNKNOWN and skips the intersection for every step of the run"
        );
    }

    /// The FOREGROUND feeder's half. Without this nothing pins the `/chain`//`/parallel` path: it
    /// is the one launch path whose observation arrives as a constructor argument, so dropping the
    /// argument would compile and simply leave every foreground chain step launching with `None`.
    #[test]
    fn the_foreground_chain_executor_carries_the_observation() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let executor = ExecSingleStepExecutor::foreground(
            DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            Arc::new(BTreeMap::new()),
            None,
            None,
            None,
            None,
            None,
            Some(vec!["read".to_string()]),
        );

        assert_eq!(
            executor.host_available_builtins,
            Some(vec!["read".to_string()]),
            "the constructor's 8th argument must land on the field"
        );

        let opts = executor.build_step_run_options(
            &single_step("reviewer", "review the change"),
            &host_test_ctx(dir.path()),
            Vec::new(),
            crate::exec::fallback::ModelOverride::Inherit,
            None,
        );
        assert_eq!(
            opts.host_available_builtins,
            Some(vec!["read".to_string()]),
            "and must then be forwarded onto every step this foreground walk dispatches"
        );
    }
}
