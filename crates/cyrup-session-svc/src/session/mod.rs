//! `AgentSession` — the single integration seam every front-end consumes (func-11 R-11-023).
//!
//! Wires the agent loop + tools + session persistence + config + resources + extensions behind one
//! async API: start/resume, prompt (→ an `EventStream<AgentSessionEvent>`), steer/follow-up,
//! interrupt, compaction, fork/branch + branch-summary, switch model — with durable persistence
//! across every turn. No mode reaches behaviour that does not flow through this object.
//!
//! ## Layout
//! The seam is one struct with ~200 methods, so its `impl` blocks are split by concern:
//! [`run`] (prompting + the post-run driver), [`commands`] (slash/wasm command execution),
//! [`queue`] (steering/follow-up + abort), [`compaction`] / [`auto_compaction`] / [`retry`]
//! (the three post-run policies), [`forking`] (fork/branch/tree), [`model`] and [`thinking`]
//! (model + reasoning control), [`control`] (the extension control-op drain), [`lifecycle`]
//! (dispose/bind/announce), [`transcript`] (naming, export and the JSON/DAG views),
//! [`accessors`] (plain getters), [`files`] (session files on disk), [`stats`], [`inject`],
//! [`bash`], [`tools`], [`adapters`] and [`types`].
//!
//! The struct, its fields, construction, and the primitives every concern shares (`lock`,
//! `fanout_emit`, `spawn_event_pump`, `now_ms`, `Drop`) stay here. Those are private to this
//! module, which Rust already makes visible to every child module above.

mod accessors;
mod adapters;
mod auto_compaction;
mod bash;
mod commands;
mod compaction;
mod control;
mod files;
mod forking;
mod inject;
mod lifecycle;
mod model;
mod queue;
mod retry;
mod run;
mod stats;
mod thinking;
mod tools;
mod transcript;
mod types;

// The seam surface `lib.rs` re-exports (`pub use session::{...}`) — same names, same paths.
pub use files::{delete_session_file_at, rename_session_file_at};
pub use types::{
    BindOptions, CompactionCostKind, DeleteMethod, ForkAnchor, ForkOutcome, ForkPosition,
    ModelCycleResult, NavigateTreeOptions, NavigateTreeOutcome, ReplayItem, ScopedModel,
    SessionDagKind, SessionDagNode,
};

// `tests/delete_session_file_trash.rs` names this through `crate::session::trash_args`; the
// re-export is `cfg(test)` because nothing in a normal build reaches it (`delete_session_file_at`
// calls it from inside `files`), and an unconditional one would be an unused import.
#[cfg(test)]
pub(crate) use files::trash_args;

// Doc-only: the `Drop` rationale below is written against the guard that plays the same role for
// the compaction cancel slot. `CompactionCancelGuard` is `pub(super)` and never re-exported, so
// rustdoc still reports `links to private item` — which is the warning this link carried before
// the split, and is `CARGO_DOC_WARNINGS.md`'s to resolve, not this task's.
#[cfg(doc)]
use compaction::CompactionCancelGuard;

// Doc-only: the `runtime_actions` field doc names the variant a runtime-tier op surfaces;
// `session.rs:35` had this in scope, and nothing in `mod.rs` names it in code.
#[cfg(doc)]
use crate::error::SessionServiceError;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_agent::{Agent, AgentMessage};
use cyrup_core::{AssistantMessage, CancelToken, EventStream, ModelRef, SessionId};
use cyrup_session::compaction::BranchSummarySettings;
use cyrup_session::manager::SessionManager;
use cyrup_tools::ProcOps;
use tokio::sync::Mutex as AsyncMutex;

use crate::event::AgentSessionEvent;
use crate::host_services::InjectRequest;
use crate::provider_swap::ProviderSwap;
use crate::services::AgentSessionServices;
use crate::subscriber::Fanout;
use crate::tools::DynamicToolState;
use cyrup_ext::host::InjectOutcome;
use inject::{CompactionGate, SteeredInjection, merge_injection_batch, split_by_group_trigger};
use run::InjectionOffer;

use adapters::{SessionActivityHandle, SessionCatalogHandle};

/// The build-time inputs the facade threads into [`AgentSession::from_parts`] for the subsystems
/// added beyond the core seam (retry/auto-compaction/bash/dynamic-tools/attribution). Grouped to
/// keep the constructor signature bounded.
pub(crate) struct SessionExtras {
    pub telemetry_enabled: bool,
    pub branch_summary_settings: BranchSummarySettings,
    pub auto_compaction_enabled: bool,
    pub auto_retry_enabled: bool,
    pub retry_max_retries: u32,
    pub retry_base_delay_ms: u64,
    /// `retry.maxAgentDelayMs` (Pi `getRetrySettings().maxAgentDelayMs`, default 60 000).
    pub retry_max_agent_delay_ms: u64,
    pub proc: Arc<dyn ProcOps>,
    /// `shellPath` setting (Pi `getShellPath`, settings-manager.ts:864-865); resolved fresh on
    /// every immediate-bash call (see [`AgentSession::execute_bash`]), never baked in at build time.
    pub shell_path: Option<String>,
    /// `shellCommandPrefix` setting (Pi `getShellCommandPrefix`, settings-manager.ts:895-896).
    pub shell_command_prefix: Option<String>,
    /// Shared with the session's [`crate::host_services::LiveHostServices`] backend so a loaded
    /// guest's `setActiveTools`/`getActiveTools` capability operates on the ONE authoritative
    /// active-tool view (Pi `getActiveTools`/`setActiveTools` bind to the same `agent.state.tools`,
    /// agent-session.ts:2281,2283).
    pub dynamic_tools: Arc<Mutex<DynamicToolState>>,
    /// The shared self-handle the builder also handed to the persist+fan-out subscriber.
    pub handle: Arc<SessionHandle>,
    /// The live session metadata the `bash` tool publishes to every child as `CYRUP_*`
    /// (Pi `resolveSpawnContext`, bash.ts:171-181). Shared with the `bash` tool the builder
    /// registered; [`AgentSession`] mutates it whenever the model or the thinking level changes, so
    /// the NEXT command sees the new values without a rebuild
    /// (Pi docs/environment-variables.md:27).
    pub bash_session_env: cyrup_tools::config::SessionEnvHandle,
    /// `read`'s view of whether the ACTIVE model accepts image input, re-pushed on every `/model`
    /// switch so the tool's non-vision warning tracks the live model rather than the startup one.
    pub read_model_vision: cyrup_tools::config::ModelVisionHandle,
}

/// A settable, weak self-reference so the persist+fan-out subscriber (which the agent owns) and the
/// post-run driver task can reach the `Arc<AgentSession>` that owns them. The owning `Arc` is created
/// by the caller (runtime / `into_shared`) AFTER `from_parts` returns, so the weak is filled in then
/// via [`AgentSession::into_shared`]. While unset (a plain by-value `AgentSession`), the post-run loop
/// is inert and the subscriber falls back to its legacy persist+fan-out-only behavior.
#[derive(Default)]
pub(crate) struct SessionHandle {
    weak: OnceLock<Weak<AgentSession>>,
}

impl SessionHandle {
    /// Upgrade to the owning session, if it was bound and is still alive.
    pub(crate) fn get(&self) -> Option<Arc<AgentSession>> {
        self.weak.get().and_then(Weak::upgrade)
    }
}

/// The integration seam (arch-11 §3.1). Cheaply shareable via `Arc`; every method is `&self`.
pub struct AgentSession {
    agent: Arc<Agent>,
    manager: Arc<AsyncMutex<SessionManager>>,
    fanout: Arc<Fanout>,
    /// The swappable stream source the agent loop streams through (`ProviderSwap`). Holds the
    /// currently-installed provider (faux offline default, or a resolved real provider) and is
    /// mutated in place on a cross-provider `/model` select so the agent streams against the new
    /// provider without rebuilding — 1:1 with Pi's live model+provider switch.
    provider: Arc<ProviderSwap>,
    services: AgentSessionServices,
    /// The active model address (mutated by `set_model`), or `None` when the session launched with
    /// no model at all — pi `AgentSession.model: Model | undefined` (agent-session.ts:866-868),
    /// which is `undefined` whenever `findInitialModel` found nothing to select
    /// (sdk.ts:216-218, model-resolver.ts:648-650). This is the state a credential-less first run
    /// starts in so the TUI can offer `/login` and then `/model` (SEAM-075).
    model: Mutex<Option<ModelRef>>,
    /// The resolved summarization/compaction model (kept in lockstep with `model`); `None` in the
    /// same modelless state.
    compaction_model: Mutex<Option<cyrup_provider::Model>>,
    /// The LIVE base system prompt — the value a run falls back to when no `before_agent_start`
    /// handler replaced it (Pi `private _baseSystemPrompt`, agent-session.ts:371).
    ///
    /// Seeded from the builder-assembled `services.system_prompt`, but MUTABLE thereafter: a
    /// tool-set rebuild rewrites it ([`Self::push_active_tools`]), exactly as Pi reassigns
    /// `this._baseSystemPrompt = this._rebuildSystemPrompt(validToolNames)` inside
    /// `setActiveToolsByName` (agent-session.ts:939). `services.system_prompt` is owned by value on
    /// an all-`&self` type and so is frozen at build time; reading the reset path from it made every
    /// run with a `before_agent_start` subscriber revert the prompt to the startup tool set.
    base_system_prompt: Mutex<String>,
    /// The `before_agent_start` handler's replacement prompt for the CURRENT run, or `None` when no
    /// handler replaced it — Pi `private _systemPromptOverride?: string` (agent-session.ts:373
    /// @v0.83.0). Assigned at `:1247`, cleared at `:1251`, and cleared again in `_runAgentPrompt`'s
    /// `finally` (`:1069`) so it never outlives its run. Every site that writes
    /// `agent.state.systemPrompt` resolves `this._systemPromptOverride ?? this._baseSystemPrompt`
    /// (`:534`, `:940`) — [`Self::effective_system_prompt`].
    ///
    /// Holding it apart from [`Self::base_system_prompt`] is the whole point: it is what lets the
    /// turn-boundary refresh re-push a system prompt WITHOUT undoing a handler's mid-run
    /// sanitization, which is why cyrup's single-slot version could not push one at all (DRIFT-033).
    system_prompt_override: Mutex<Option<String>>,
    branch_summary_settings: BranchSummarySettings,
    /// Long-lived token handed to the extension subscriber (distinct from per-run cancellation).
    session_cancel: CancelToken,
    session_id: SessionId,
    /// Latches `true` the first time a `--mode json` run writes the session header (Pi
    /// `sessionManager.getHeader()` → JSONL line 1, print-mode.ts:112-117). Pi writes the header
    /// exactly ONCE, before the whole message loop in `runPrintMode`; cyrup replays follow-up
    /// prompts through additional [`crate::AgentSession::prompt`]-scoped `run_json` calls, so the
    /// header emitter ([`Self::claim_json_header`]) consults this latch to stay one-shot per session.
    json_header_written: AtomicBool,
    /// Latches `true` the first time this session is announced with `session_start`. Pi emits its
    /// `_sessionStartEvent` exactly once per `AgentSession` (agent-session.ts:2250, reached from
    /// `bindExtensions`); the latch keeps that one-shot contract when a host binds a session the
    /// runtime already announced.
    start_announced: AtomicBool,
    /// Facade-side mirror of the steering queue text (Pi `_steeringMessages`, agent-session.ts:476)
    /// for `queue_update` emission + introspection; the authoritative queue lives in the agent.
    steering_messages: Mutex<Vec<String>>,
    /// Facade-side mirror of the follow-up queue text (Pi `_followUpMessages`, agent-session.ts:477).
    follow_up_messages: Mutex<Vec<String>>,
    /// Warning surfaced when a resumed session's saved model could not be restored (Pi
    /// `modelFallbackMessage`, sdk.ts:91/192). `None` when the model resolved cleanly.
    model_fallback_message: Option<String>,
    /// Cancel handle for an in-flight manual compaction (Pi `_compactionAbortController`,
    /// agent-session.ts:1654); set while [`Self::compact`] runs, cleared in its `finally`.
    compaction_cancel: Mutex<Option<CancelToken>>,
    /// Cancel handle for an in-flight branch summarization (Pi `_branchSummaryAbortController`,
    /// agent-session.ts:1796).
    branch_summary_cancel: Mutex<Option<CancelToken>>,
    /// Messages staged to ride the NEXT prompt turn (Pi `_pendingNextTurnMessages`,
    /// agent-session.ts:1339); drained into the run by [`Self::assemble_run_messages`].
    pending_next_turn: Mutex<Vec<AgentMessage>>,
    /// Custom messages deferred because a run was live and the caller passed `triggerTurn: false`
    /// (Pi `_pendingCustomMessages`, agent-session.ts:350, pushed at `:1966` @v0.87.1). Appending
    /// one mid-turn would place it between an assistant tool call and its result, which providers
    /// that validate message order reject on replay — so it waits for a turn boundary and is
    /// drained by [`Self::flush_pending_custom_messages`]. SEAM-127.
    pending_custom_messages: Mutex<Vec<AgentMessage>>,
    /// Models available for `cycle_model` (Pi `_scopedModels`, agent-session.ts:870).
    scoped_models: Mutex<Vec<ScopedModel>>,
    /// Facade mirror of the agent's steering-queue mode (the agent exposes only a setter; Pi reads
    /// `agent.steeringMode`, agent-session.ts:845).
    steering_mode: Mutex<cyrup_agent::QueueMode>,
    /// Facade mirror of the agent's follow-up-queue mode (Pi `agent.followUpMode`, :850).
    follow_up_mode: Mutex<cyrup_agent::QueueMode>,
    /// Whether provider install-telemetry is on (gates default attribution headers, Pi sdk.ts:323).
    telemetry_enabled: bool,
    // ---- retry subsystem (Pi agent-session.ts:778,2484-2572) ----
    /// Current retry attempt (0 when not retrying; Pi `_retryAttempt`).
    retry_attempt: Mutex<u32>,
    /// Cancel handle for the in-flight backoff sleep (Pi `_retryAbortController`).
    retry_cancel: Mutex<Option<CancelToken>>,
    /// Runtime override of the settings `retry.enabled` toggle (Pi `setAutoRetryEnabled`).
    auto_retry_override: Mutex<Option<bool>>,
    /// `retry.enabled` default sourced from settings at build time.
    retry_enabled_default: bool,
    retry_max_retries: u32,
    retry_base_delay_ms: u64,
    retry_max_agent_delay_ms: u64,
    // ---- auto-compaction (Pi agent-session.ts:831,1811-1905,2078-2086) ----
    /// Runtime override of the settings `compaction.enabled` toggle (Pi `setAutoCompactionEnabled`).
    auto_compaction_override: Mutex<Option<bool>>,
    auto_compaction_enabled_default: bool,
    /// Cancel handle for an in-flight auto-compaction (Pi `_autoCompactionAbortController`).
    auto_compaction_cancel: Mutex<Option<CancelToken>>,
    /// Set once after an overflow auto-compaction so a second overflow does not loop (Pi
    /// `_overflowRecoveryAttempted`, agent-session.ts:1859).
    overflow_recovery_attempted: Mutex<bool>,
    /// Latched by [`Self::abort`] while a run is active, so every post-run decision point can tell
    /// an abort from a normal `agent_end` — Pi `_agentRunAbortRequested`
    /// (`agent-session.ts:339` @v0.87.1), cleared where a run starts (`_runAgentPrompt` :1469) and
    /// consulted by the continuation loop (:1473), `_handlePostAgentRun` (:1497,1504,1507,1522,1528)
    /// and `_willRetryAfterAgentEnd` (:977).
    ///
    /// An `AtomicBool` rather than a `Mutex<bool>`: it is WRITTEN from a signal handler / RPC cancel
    /// and READ from the driver task, so it must be lock-free on the writer side. `SeqCst`
    /// throughout — the latch orders against the cancel-token stores in the same `abort()`.
    agent_run_abort_requested: AtomicBool,
    // ---- immediate-bash seam (Pi agent-session.ts:2582-2684) ----
    proc: Arc<dyn ProcOps>,
    shell_path: Option<String>,
    shell_command_prefix: Option<String>,
    /// Cancel handles for the in-flight `execute_bash` calls, one entry per call — Pi
    /// `private readonly _bashAbortControllers = new Set<AbortController>()`
    /// (agent-session.ts:337 @v0.83.0). Pi keys the set on `AbortController` object IDENTITY, not on
    /// the caller-supplied `options.id` (which is optional and may repeat), so cyrup keys it on a
    /// private monotonic handle minted per call by [`Self::next_bash_cancel_id`]. It was a single
    /// `Option<CancelToken>` slot: with two user bash commands in flight the second overwrote the
    /// first, so `abort_bash` reached only the newest and the first to finish cleared the slot while
    /// the other still ran (DRIFT-029).
    bash_cancels: Mutex<Vec<(u64, CancelToken)>>,
    /// Source of the identity handles in [`Self::bash_cancels`]; see that field.
    next_bash_cancel_id: std::sync::atomic::AtomicU64,
    /// Bash messages deferred while a run streams, flushed after the turn (Pi `_pendingBashMessages`).
    pending_bash: Mutex<Vec<AgentMessage>>,
    /// The live session metadata the `bash` TOOL publishes to every child as `CYRUP_*` (Pi
    /// `resolveSpawnContext`, bash.ts:171-181). The same handle the builder gave the registered
    /// `BashTool`; Pi reads these off a per-call `ExtensionContext`, so they track the session
    /// automatically. cyrup's `Tool::execute` takes no context, so the values are PUSHED here
    /// whenever the model or the thinking level changes — which is what makes "the values are
    /// resolved when each command starts. Switching models or changing the reasoning level
    /// therefore affects the next bash command" (docs/environment-variables.md:27) true of cyrup
    /// too, rather than only of the tool in isolation.
    bash_session_env: cyrup_tools::config::SessionEnvHandle,
    read_model_vision: cyrup_tools::config::ModelVisionHandle,
    // ---- dynamic tools (Pi agent-session.ts:786-828,2304) ----
    /// Shared (`Arc`) with [`crate::host_services::LiveHostServices`] so a live wasm guest's
    /// `setActiveTools`/`getActiveTools` and the host/CLI tool-toggle read+mutate the SAME state.
    dynamic_tools: Arc<Mutex<DynamicToolState>>,
    // ---- post-run execution loop (Pi `_runAgentPrompt`/`_handlePostAgentRun`,
    //      agent-session.ts:973-1022; the assembled-run driver) ----
    /// Weak self-reference, bound by [`Self::into_shared`]; shared with the persist+fan-out
    /// subscriber so both the subscriber's `_handleAgentEvent` work and the spawned post-run driver
    /// can reach `Arc<AgentSession>`.
    handle: Arc<SessionHandle>,
    /// The last assistant message of the in-flight run (Pi `_lastAssistantMessage`,
    /// agent-session.ts:510): set by the subscriber on every assistant `message_end`, taken by the
    /// driver in [`Self::handle_post_agent_run`].
    last_assistant: Mutex<Option<AssistantMessage>>,
    /// `true` while a post-run driver task owns the run (Pi has no analogue — cyrup spawns the loop in
    /// the background because `prompt` returns an event stream rather than awaiting the whole run).
    /// [`Self::wait_for_idle`] waits on this so a one-shot caller sees the WHOLE loop settle, not just
    /// the first `agent_end`.
    driver_tx: tokio::sync::watch::Sender<bool>,
    /// A keep-alive receiver so `driver_tx.send` never fails for want of a live receiver (a watch
    /// `Sender` with zero receivers drops the sent value); `wait_for_idle` subscribes fresh ones.
    _driver_keepalive: tokio::sync::watch::Receiver<bool>,
    /// `true` from the moment a settling run clears pi's `_isAgentRunActive` (the first statement of
    /// `_emitAgentSettled`, `agent-session.ts:872` @v0.87.1) until `driver_tx` drops or a new run
    /// starts. `driver_tx` is cyrup's run latch AND its idle latch; pi releases the first BEFORE the
    /// `agent_settled` emit and the second only after the deferred settled actions (SEAM-129), so
    /// [`Self::is_run_active`] reads `driver_tx` through this.
    run_released: AtomicBool,
    /// Pi `_isEmittingAgentSettled` (`agent-session.ts:378` @v0.87.1): set for the length of the
    /// `agent_settled` emit, during which a prompt or a trigger-turn message is deferred.
    emitting_settled: AtomicBool,
    /// Pi `_deferredSettledActions` (`agent-session.ts:379`): submissions made while
    /// [`Self::emitting_settled`] was set, run in order once the emit returns.
    deferred_settled: Mutex<Vec<run::DeferredSettled>>,
    /// Bumped every time a compaction or branch-summary cancel slot is cleared — the idle edge of a
    /// compaction (SEAM-125). [`Self::is_idle`] counts a running compaction as busy (pi v0.85.1+
    /// `isIdle = !_isAgentRunActive && !isCompacting`), so [`Self::wait_for_idle`] needs something
    /// to wake on when one ends; pi resolves its idle wait from `_clearManualCompactionState()`
    /// (`agent-session.ts:2386-2388` @v0.87.1). Raised only by `CompactionCancelGuard::clear`.
    /// Written with `send_modify`, which never needs a live receiver.
    compaction_settled: tokio::sync::watch::Sender<u64>,
    // ---- extension control sinks (SEAM-003 / EXT-005) ----
    /// The RUNTIME-tier control sink (Pi `ExtensionCommandContextActions`, extensions/types.ts:
    /// 1652-1672), installed by [`crate::AgentSessionRuntime`] before this session is announced.
    /// `None` on a bare session built straight from [`crate::SessionBuilder`]; a runtime-tier op
    /// then surfaces [`SessionServiceError::NoRuntimeHost`] as a warning diagnostic rather than being
    /// silently dropped — Pi's pre-bind stubs throw `"Extension runtime not initialized…"` for the
    /// same reason (extensions/loader.ts:173-176 `notInitialized`).
    runtime_actions: OnceLock<Arc<dyn crate::runtime::RuntimeActions>>,
    /// Latches `true` when a loaded extension calls `ctx.shutdown()` (Pi `ctx.shutdown()`,
    /// extensions/types.ts:344 → `runner.shutdown()` → the host's `shutdownHandler`,
    /// rpc-mode.ts:344-346). Hosts poll [`Self::shutdown_requested`] at their next settle point —
    /// which is exactly what `agent_settled` is for (Pi rpc-mode.ts:355-358).
    shutdown_requested: AtomicBool,
}

impl AgentSession {
    /// Build from the assembled parts (called by [`crate::SessionBuilder::build`]).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_parts(
        agent: Arc<Agent>,
        manager: Arc<AsyncMutex<SessionManager>>,
        fanout: Arc<Fanout>,
        provider: Arc<ProviderSwap>,
        services: AgentSessionServices,
        model: Option<ModelRef>,
        session_cancel: CancelToken,
        session_id: SessionId,
        model_fallback_message: Option<String>,
        extras: SessionExtras,
    ) -> Self {
        let compaction_model = services.model.clone();
        let base_system_prompt = services.system_prompt.clone();
        // Seed the queue-mode mirrors from the resolved settings (the builder wired the same modes
        // into the agent), so the getters report the live mode without an agent-side getter.
        let eff = services.settings.effective();
        let steering_mode = crate::builder::parse_queue_mode(&eff.steering_mode());
        let follow_up_mode = crate::builder::parse_queue_mode(&eff.follow_up_mode());
        // The post-run-driver liveness channel; the keep-alive receiver keeps `send` from failing for
        // want of a live receiver (see field docs).
        let (driver_tx_init, driver_keepalive) = tokio::sync::watch::channel(false);
        Self {
            agent,
            manager,
            fanout,
            provider,
            services,
            model: Mutex::new(model),
            compaction_model: Mutex::new(compaction_model),
            base_system_prompt: Mutex::new(base_system_prompt),
            system_prompt_override: Mutex::new(None),
            branch_summary_settings: extras.branch_summary_settings,
            session_cancel,
            session_id,
            json_header_written: AtomicBool::new(false),
            start_announced: AtomicBool::new(false),
            steering_messages: Mutex::new(Vec::new()),
            follow_up_messages: Mutex::new(Vec::new()),
            model_fallback_message,
            compaction_cancel: Mutex::new(None),
            branch_summary_cancel: Mutex::new(None),
            pending_next_turn: Mutex::new(Vec::new()),
            pending_custom_messages: Mutex::new(Vec::new()),
            scoped_models: Mutex::new(Vec::new()),
            steering_mode: Mutex::new(steering_mode),
            follow_up_mode: Mutex::new(follow_up_mode),
            telemetry_enabled: extras.telemetry_enabled,
            retry_attempt: Mutex::new(0),
            retry_cancel: Mutex::new(None),
            auto_retry_override: Mutex::new(None),
            retry_enabled_default: extras.auto_retry_enabled,
            retry_max_retries: extras.retry_max_retries,
            retry_base_delay_ms: extras.retry_base_delay_ms,
            retry_max_agent_delay_ms: extras.retry_max_agent_delay_ms,
            auto_compaction_override: Mutex::new(None),
            auto_compaction_enabled_default: extras.auto_compaction_enabled,
            auto_compaction_cancel: Mutex::new(None),
            overflow_recovery_attempted: Mutex::new(false),
            agent_run_abort_requested: AtomicBool::new(false),
            proc: extras.proc,
            shell_path: extras.shell_path,
            shell_command_prefix: extras.shell_command_prefix,
            bash_cancels: Mutex::new(Vec::new()),
            next_bash_cancel_id: std::sync::atomic::AtomicU64::new(0),
            pending_bash: Mutex::new(Vec::new()),
            bash_session_env: extras.bash_session_env,
            read_model_vision: extras.read_model_vision,
            dynamic_tools: extras.dynamic_tools,
            handle: extras.handle,
            last_assistant: Mutex::new(None),
            driver_tx: driver_tx_init,
            _driver_keepalive: driver_keepalive,
            run_released: AtomicBool::new(false),
            emitting_settled: AtomicBool::new(false),
            deferred_settled: Mutex::new(Vec::new()),
            compaction_settled: tokio::sync::watch::channel(0).0,
            runtime_actions: OnceLock::new(),
            shutdown_requested: AtomicBool::new(false),
        }
    }

    /// Install the RUNTIME-tier control sink (SEAM-003). Called by [`crate::AgentSessionRuntime`] on
    /// every session it owns — the initial one before `bind_extensions()`, each replacement before
    /// its `session_start` — mirroring Pi, which passes `commandContextActions` INTO
    /// `bindExtensions` and re-passes it on every `rebindSession` (rpc-mode.ts:341-346). Idempotent:
    /// the first install wins (a host that also binds cannot displace the runtime's sink).
    pub fn install_runtime_actions(&self, actions: Arc<dyn crate::runtime::RuntimeActions>) {
        let _ = self.runtime_actions.set(actions);
    }

    /// Whether a loaded extension has asked the host to exit (Pi `ctx.shutdown()` → the host's
    /// `shutdownHandler` setting `shutdownRequested = true`, rpc-mode.ts:344-346). A host checks
    /// this at its settle point — see `AgentSessionEvent::AgentSettled` (SEAM-005), which is where
    /// Pi checks it (rpc-mode.ts:355-358).
    /// ORs two latches on purpose. The backend's is set SYNCHRONOUSLY inside
    /// `HostServices::control(ControlOp::Shutdown)` (Pi's `shutdownHandler`, rpc-mode.ts:344-346);
    /// this session's own is set by the turn-boundary control drain. Reading only the latter made
    /// the answer depend on whether a turn boundary happened to follow the request — a shutdown
    /// asked for from a background task on an idle session, or in the window after the in-flight
    /// run's drain had already run, stayed queued forever and the host never exited.
    pub fn shutdown_requested(&self) -> bool {
        self.shutdown_requested.load(Ordering::SeqCst)
            || self.services.host_services.shutdown_requested()
    }

    /// Wrap a freshly-built session in its owning `Arc` and bind the self-handle so the persist+fan-out
    /// subscriber and the post-run driver can reach it (arch-11; the assembled-run wiring). MUST be used
    /// instead of a bare `Arc::new` wherever the post-run execution loop has to run (the runtime, print
    /// mode, the SDK) so auto-retry / post-run auto-compaction / the queued-continuation actually fire
    /// from a completed turn.
    pub fn into_shared(self) -> Arc<Self> {
        let handle = self.handle.clone();
        let arc = Arc::new(self);
        let _ = handle.weak.set(Arc::downgrade(&arc));
        // Bind the message-injection seam (R-SA-101 / P-2): a background task calling
        // `LiveHostServices::inject_message`/`inject_message_ack` (e.g. cyrup-ext-subagents'
        // completion sink, or a native extension holding the P-1 host-services Arc) reaches THIS
        // live session's turn loop. Bound only on a shared session — a by-value session has no
        // post-run driver to run the turn anyway. If `into_shared` runs outside a tokio runtime
        // (some by-value tests), no pump is spawned and the seam stays unbound, so injection
        // reports itself unavailable rather than panicking (workspace denies `panic`).
        //
        // The sender is handed over ONLY when a consumer exists to drain it: binding a queue with
        // no pump would accept messages that nothing could ever deliver, which is precisely the
        // "acknowledged but lost" failure this seam was rebuilt to remove.
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<InjectRequest>();
            let weak = Arc::downgrade(&arc);
            runtime.spawn(drive_injections(weak, rx));
            arc.services.host_services.set_inject_sink(tx);
        }
        // EXT-005: give the capability backend a LIVE readback of run activity + a real interrupt,
        // so a guest's `ctx.isIdle()`/`ctx.hasPendingMessages()` answer from this session and its
        // `ctx.abort()` stops the run that is in flight (Pi binds all three straight to the session,
        // agent-session.ts:2409-2419). Weak, so the backend never keeps the session alive.
        arc.services
            .host_services
            .attach_session_activity(Arc::new(SessionActivityHandle(Arc::downgrade(&arc))));
        // EXT-037/EXT-038: give the capability backend the LIVE introspection catalog behind a
        // guest's `getCommands()` and the extension-tool provenance half of its `getAllTools()`. Pi
        // binds both straight to the session in `_bindExtensionCore` (agent-session.ts:2394,2397),
        // and only the session can see the prompt templates + skills `getCommands()` concatenates.
        // Weak, so the backend never keeps the session alive.
        arc.services
            .host_services
            .attach_session_catalog(Arc::new(SessionCatalogHandle(Arc::downgrade(&arc))));
        // AGENT-029: install pi's per-request `transformHeaders` (sdk.ts:312-328 @v0.83.0,
        // byte- and offset-identical at v0.84.1). pi merges provider-attribution + session-affinity
        // headers inside a callback closed over the `model` argument of THAT `streamSimple` call —
        // i.e. the model the loop chose for that turn (`agent-loop.ts:308`, whose `config.model` is
        // the possibly-overridden `nextTurnSnapshot.model ?? config.model` from `:237`). cyrup
        // latched them into `StateInner::headers`, written only by the two SESSION-level model-change
        // paths, so a per-turn `TurnUpdate::model` override retargeted the request while the previous
        // provider's attribution rode along. Weak, so the resolver never keeps the session alive.
        let hw = Arc::downgrade(&arc);
        arc.agent.set_header_fn(Some(Arc::new(move |m: &ModelRef| {
            hw.upgrade().and_then(|s| s.headers_for_model_ref(m))
        })));
        arc
    }

    /// Whether NO session work is in flight (Pi `isIdle`): no agent run or post-run continuation
    /// (retry / auto-compaction / queued continuation) AND no compaction or branch summary — the
    /// same latches [`Self::wait_for_idle`] waits on, read without awaiting so the SYNC
    /// `ctx.isIdle()` host import can answer.
    ///
    /// SEAM-125 — pi v0.85.1 (`bea67d90d`) made this `!this._isAgentRunActive && !this.isCompacting`
    /// (`agent-session.ts:1234-1236` @v0.87.1). A manual `/compact` and a `/tree` branch summary run
    /// OUTSIDE the post-run driver, so before this read `driver_tx` and the agent latch only, the
    /// session reported idle for the whole summarization call: an extension reading `ctx.isIdle()`
    /// (pi-intercom's inbound decision) was told to start work, and the injection pump started a
    /// model run over the pre-compaction transcript that the compaction then swapped out from under
    /// it with `set_messages` (ICOM-062).
    ///
    /// This is NOT the complement of [`Self::is_run_active`], exactly as pi's `isIdle` is not the
    /// complement of its `isStreaming`: a compaction is busy without being a run, and submissions
    /// route on the run latch alone.
    pub fn is_idle(&self) -> bool {
        !self.is_run_active() && !self.is_compacting()
    }

    /// Wake every [`Self::wait_for_idle`] parked behind a compaction or branch summary. Called by
    /// `CompactionCancelGuard::clear` right after it empties its slot.
    pub(super) fn notify_compaction_settled(&self) {
        self.compaction_settled
            .send_modify(|n| *n = n.wrapping_add(1));
    }

    /// Whether either steering/follow-up queue of the AGENT still holds a message (pi
    /// `agent.hasQueuedMessages()`). Unlike [`Self::pending_message_count`], which counts only the
    /// user-text mirrors pi's `pendingMessageCount` counts, this sees a custom message an extension
    /// or the injection pump steered onto the live run (ICOM-035).
    pub fn has_queued_messages(&self) -> bool {
        self.agent.has_queued_messages()
    }

    /// Whether the session is processing an agent run **or a post-run continuation** — pi's
    /// `_isAgentRunActive` (`packages/coding-agent/src/core/agent-session.ts:313` @v0.83.0), set at
    /// the top of `_runAgentPrompt` at `:1062` and cleared only in `_emitAgentSettled` at `:582`, so
    /// it spans `_handlePostAgentRun()` and every `agent.continue()`.
    ///
    /// AGENT-030: this — not the agent's per-run streaming flag — is what pi's `get isStreaming()`
    /// returns (`:876-877`) and what `prompt()` consults at `:1159` to route a submission to
    /// `_queueSteer` / `_queueFollowUp` instead of starting a run. cyrup's
    /// [`Self::is_streaming`] reads the AGENT's run latch, which releases the moment each
    /// INDIVIDUAL run settles, so a submission gated on it in the post-run gap (an auto-retry, an
    /// auto-compaction, a queued continuation) would start a SECOND run that races `drive_run`'s
    /// `continue_run()` — every routing site therefore reads this predicate.
    ///
    /// Two latches: `driver_tx` covers the whole post-run loop on a BOUND session, and
    /// `agent.is_running()` covers an unbound session, where `spawn_run` drives `agent.prompt`
    /// directly and no driver loop exists. A running compaction is deliberately NOT part of it
    /// (pi's `isStreaming` is `_isAgentRunActive` alone) — see [`Self::is_idle`] for the predicate
    /// that does count it (SEAM-125).
    ///
    /// SEAM-129 — `driver_tx` stays `true` through the `agent_settled` emit and the deferred actions
    /// after it, because it is also the idle latch; pi's run latch is already `false` there
    /// (`_emitAgentSettled` clears it before emitting, `agent-session.ts:872` @v0.87.1), so the
    /// released window reads as not active.
    pub fn is_run_active(&self) -> bool {
        (*self.driver_tx.borrow() && !self.run_released.load(Ordering::SeqCst))
            || self.agent.is_running()
    }

    /// Whether an [`Self::abort`] landed during the current run — Pi `_agentRunAbortRequested`
    /// (`agent-session.ts:339` @v0.87.1).
    pub(crate) fn abort_requested(&self) -> bool {
        self.agent_run_abort_requested.load(Ordering::SeqCst)
    }

    /// Set or clear the run-abort latch. Pi assigns the field directly at its two WRITE sites:
    /// `abort()` (:2077) and the head of `_runAgentPrompt` (:1469, clearing it for the new run). Its
    /// READ sites are the continuation loop (:1473), the `finally`'s `_finishCancelledRetry` guard
    /// (:1484 — `settle_run`), the four checks inside `_handlePostAgentRun` (:1497/:1504/:1507/:1522)
    /// and its tail (:1528), plus `_willRetryAfterAgentEnd` (:977). All are ported.
    pub(crate) fn set_abort_requested(&self, v: bool) {
        self.agent_run_abort_requested.store(v, Ordering::SeqCst);
    }

    /// Lock a `std::sync::Mutex` ignoring poisoning (no panic; arch-00 no-panic).
    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A long-lived event subscription (TUI / SDK observer) — lives until the stream is dropped.
    pub fn subscribe(&self) -> EventStream<AgentSessionEvent> {
        self.fanout.subscribe_persistent()
    }

    async fn fanout_emit(&self, ev: AgentSessionEvent) {
        // Reuse the same fan-out the agent subscriber feeds; session-level events interleave with
        // agent events on the live streams.
        self.fanout.emit_external(ev).await;
    }

    /// Forward events posted to `rx` onto the seam fan-out, from a DEDICATED task.
    ///
    /// Pi emits `summarization_retry_*` / `bash_execution_update` inline from synchronous callbacks
    /// (`agent-session.ts:2645-2668` and `:2785-2787`); cyrup's fan-out is `async` (it awaits
    /// per-subscriber backpressure) and those callbacks are not, so they post to an unbounded
    /// channel that this task drains. Two properties are deliberate:
    ///
    /// * **A separate task, not an inline `select!`.** Every summarization-bearing operation runs
    ///   while holding the session-manager lock, and [`Self::fanout_emit`] can block on a lagging
    ///   subscriber's bounded channel. Emitting inline would mean awaiting that backpressure WITH
    ///   the manager lock held — and a subscriber that is both lagging and waiting on the manager
    ///   lock would deadlock the session. Every other emit site in this file drops the guard first
    ///   for exactly that reason; this task keeps that invariant without serializing the operation
    ///   behind the fan-out.
    /// * **Ends when the last sender drops.** Callers close the queue by dropping the emitter (the
    ///   compactor / bash sink that owns it) and then `await` the returned handle — AFTER releasing
    ///   the manager guard — which flushes every queued event BEFORE the operation's own terminal
    ///   event (`compaction_end`, the bash result), matching Pi's inline ordering. An early return
    ///   that never awaits the handle still terminates the task, because the emitter is dropped at
    ///   scope exit.
    fn spawn_event_pump(
        &self,
        mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentSessionEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let fanout = Arc::clone(&self.fanout);
        tokio::spawn(async move {
            while let Some(ev) = rx.recv().await {
                fanout.emit_external(ev).await;
            }
        })
    }
}

/// Mark this session's extension instances stale when the session itself goes away — the backstop
/// for every path that never reaches the tail of [`AgentSession::dispose_with`].
///
/// **[CYRUP-DELTA]** vs pi `AgentSession.dispose()` → `this._extensionRunner.invalidate(...)`
/// (`core/agent-session.ts:850-852` @v0.84.2). Upstream has NO destructor half at all, and cannot:
/// JavaScript gives a class no deterministic finalizer, so `invalidate` is reachable only from the
/// one explicit `dispose()` call. Rust does give one, and the ported call site sits behind three
/// `.await`s a dropped future never resumes past (see below), so the upstream mechanism alone is
/// strictly weaker HERE than it is there. This adds the half the language makes available; it
/// changes no observable behaviour on any path that does reach `dispose_with`, because
/// `GuestState::invalidate` is first-reason-wins (pi `extensions/loader.ts:207`).
///
/// # Why a `Drop` as well as the ordered call
///
/// `dispose_with` invalidates at pi's exact position: after `session_shutdown` has been fanned out
/// and every handler has finished, after the host's `before_session_invalidate` hook, and before
/// the session token is cancelled (pi `teardownCurrent` → `this.session.dispose()` →
/// `this._extensionRunner.invalidate(<stale text>)`, `core/agent-session-runtime.ts:176-177` +
/// `core/agent-session.ts:850-852` @v0.84.2). That ORDER is the contract, so this `Drop` is not a
/// replacement for it — it is the same relationship [`CompactionCancelGuard`] has with the
/// hand-written clears, and for the identical reason.
///
/// Three of `dispose_with`'s statements are `.await`s (`abort_and_settle`, `fanout_emit`,
/// `dispatch_notify`), and everything after them — the hook, the invalidation, the cancel — runs
/// only if the future is polled to completion. A Rust future can be dropped at any `.await`, and
/// this crate already documents callers that do exactly that to a session future: the
/// `tokio::time::timeout` around the `cyrup-sdk` handle and `run_rpc`'s `select!` dropping the
/// driver when the write pump reports a broken pipe (`cyrup-modes/src/rpc.rs:668-676`) — see
/// [`CompactionCancelGuard`]'s doc, which names both. `dispatch_notify` is the worst of the three
/// to be cut at: it awaits extension handlers, including guest calls across the wasm boundary, so
/// it is the statement most likely to be slow enough to be raced or to unwind on a native
/// extension's panic. In any of those cases the outgoing instances stayed un-stale forever and
/// pi's `assertActive` refusal never fired for a call still in flight on one of them.
///
/// A session that is DROPPED WITHOUT `dispose` at all is the same hole reached from the other side.
/// `cyrup_sdk::Session::close` documents that dropping a `Session` without calling it is silent,
/// and correctly explains that `Drop` cannot do the async half — `session_shutdown` is dispatched
/// and awaited. Invalidation is the SYNC half, and is therefore precisely the part a `Drop` can
/// still honour; this closes that half without disturbing the documented contract for the rest.
///
/// # Why this is safe to do unconditionally
///
/// * **Idempotent.** `GuestState::invalidate` is `if state.staleMessage) return;` — the FIRST
///   reason wins (pi `extensions/loader.ts:207`), so running after a normal `dispose_with` is a
///   no-op and cannot overwrite a more specific reason with the default text.
/// * **Correctly scoped.** It cannot stale the REPLACEMENT session's extensions, because a session
///   does not share its host: every replacement path goes through `SessionFactory::build*` →
///   `SessionBuilder::build`, which constructs a fresh `ExtensionHost` (`builder.rs`, `let
///   ext_host = Arc::new(host)`) and loads its own guest set into it. That mirrors upstream, where
///   each `createRuntime` builds a new `DefaultResourceLoader` and `await resourceLoader.reload()`
///   hands the replacement a fresh `createExtensionRuntime()` whose `staleMessage` is unset
///   (`core/agent-session-services.ts:154`, `core/resource-loader.ts:283`,
///   `core/extensions/loader.ts:174-178`). This is why pi can invalidate on every teardown without
///   disabling the session that follows, and why cyrup can too.
/// * **A no-op without live guests**, including the whole `--no-default-features` arm, where
///   `ExtensionHost::invalidate_live` is a `#[cfg]`'d empty body.
impl Drop for AgentSession {
    fn drop(&mut self) {
        self.services.ext_host.invalidate_live(None);
    }
}

/// The session's ONE injection consumer, and the owner of the pending inbox.
///
/// # Single-consumer is the correctness property, not an optimisation
///
/// Every injected message reaches the agent through this task and no other, so two background
/// completions can never race each other for the run latch. The seam this replaced spawned an
/// independent task per injection, and those tasks fought over `Agent::prompt`'s CAS: exactly one
/// won and the losers were warn-logged away with their payloads already deleted.
///
/// # Holding the inbox is what makes "not yet" a schedule instead of an error
///
/// A message the agent cannot take right now stays here, owned, and is offered again on the next
/// idle edge. Because nothing hands it to a queue that may never be drained (a steer landing past
/// the run loop's last `poll_steering`/`poll_follow_up` strands with the session idle), there is
/// no instant at which a message is nobody's responsibility — which is what lets the ack mean
/// something.
///
/// # A no-turn message steers onto a live run — and stays owned until the idle edge (ICOM-035)
///
/// pi delivers `sendCustomMessage(msg, { deliverAs: "steer" })` to a streaming session with
/// `agent.steer` (`agent-session.ts:1949-1954` @v0.87.1), which is how a peer's intercom message
/// reaches a BUSY session's running model at its next steering boundary. So while a run is active
/// this task does not merely park: it keeps receiving, and hands every message injected with that
/// delivery ([`cyrup_ext::host::HostServices::inject_message_steer`]) to the run's steering queue
/// the moment it arrives. It keeps each one (with its ack) in `steered`, and at
/// the idle edge [`AgentSession::append_no_turn_messages`] takes back any the run never drained —
/// the stranded steer this task was built to prevent — and appends them to the tree and transcript
/// instead. A turn-triggering message still waits for the idle edge, as it always has.
async fn drive_injections(
    session: Weak<AgentSession>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<InjectRequest>,
) {
    let mut inbox: Vec<InjectRequest> = Vec::new();
    let mut steered: Vec<SteeredInjection> = Vec::new();
    // `false` once every producer is gone; what is already owned is still delivered.
    let mut open = true;
    loop {
        if inbox.is_empty() && steered.is_empty() {
            if !open {
                return;
            }
            match rx.recv().await {
                Some(req) => inbox.push(req),
                // Every producer is gone (the host services backend dropped): nothing more can
                // ever arrive, and nothing is owned, so there is nothing to answer for.
                None => return,
            }
        }
        // Coalesce ONLY what is already queued — never wait to batch, so a lone completion pays no
        // added latency. pi batches too: `sendCompletion` takes an ARRAY of completion details and
        // ORs their `triggerTurn` (`notify.ts:399-412` @v0.64.0), so one turn carrying three
        // notifications is upstream's own shape — and it is what lets an orchestrator answer about
        // a whole fan-out in a single turn instead of three sequential ones.
        if open {
            open = drain_ready(&mut rx, &mut inbox);
        }
        let Some(session) = session.upgrade() else {
            for req in inbox.drain(..) {
                req.ack.answer(InjectOutcome::SessionUnavailable);
            }
            for s in steered.drain(..) {
                s.ack.answer(InjectOutcome::SessionUnavailable);
            }
            return;
        };
        // The busy phase: steer what can be steered, keep receiving, and wait for the idle edge.
        // `wait_for_idle` is a real await on the session's latches (the run driver's `watch`, the
        // agent's run latch and the compaction-settled signal) — not a poll, and not a
        // check-then-act — and it returns at once when the session is already idle.
        loop {
            session.steer_injections_onto_live_run(&mut inbox, &mut steered);
            if session.is_idle() {
                break;
            }
            tokio::select! {
                () = session.wait_for_idle() => {}
                next = rx.recv(), if open => match next {
                    Some(req) => {
                        inbox.push(req);
                        open = drain_ready(&mut rx, &mut inbox);
                    }
                    None => open = false,
                },
            }
        }
        // SUBA-017 — re-drain AFTER the idle wait, not only before it. Every message that arrived
        // while the session was busy is owed to the SAME next turn: without this the batch taken
        // before the wait runs alone and the late arrivals pay a turn of their own (a fan-out's
        // completions split c1 | c2 | c3..cN — three turns where one suffices). It costs no latency:
        // it only takes what is already queued. It helps every producer (completions, steers,
        // watchdog warnings, intercom), which is why it lives here and not in the completion batcher.
        if open {
            open = drain_ready(&mut rx, &mut inbox);
        }
        let (durable, turn) = split_by_group_trigger(std::mem::take(&mut inbox));
        // The no-turn half first: it is independent of the turn, and answering it before a possible
        // `AgentBusy` on the turn below is what keeps a re-offered batch from appending it twice.
        let durable_messages = merge_injection_batch(&durable).durable;
        match session
            .append_no_turn_messages(durable_messages, &steered, CompactionGate::Wait)
            .await
        {
            Ok(InjectionOffer::Taken) => {
                for req in durable {
                    req.ack.answer(InjectOutcome::Accepted);
                }
                for s in steered.drain(..) {
                    s.ack.answer(InjectOutcome::Accepted);
                }
            }
            // The session went busy again in the gap after the idle wait (a user prompt, a
            // compaction). Not a failure, and reported to nobody: everything is still owned and the
            // next pass parks until it settles. The yield only guarantees forward progress if the
            // session flip-flops idle/busy faster than this loop can observe it.
            Ok(InjectionOffer::AgentBusy) => {
                inbox = durable;
                inbox.extend(turn);
                tokio::task::yield_now().await;
                continue;
            }
            // A fault that waiting cannot fix (a poisoned append, …). Hand every message back so
            // its producer keeps whatever it was announcing and can retry on its own terms.
            Err(fault) => {
                tracing::warn!(error = %fault, "session cannot accept injected messages");
                for req in durable.into_iter().chain(turn) {
                    req.ack.answer(InjectOutcome::SessionUnavailable);
                }
                for s in steered.drain(..) {
                    s.ack.answer(InjectOutcome::SessionUnavailable);
                }
                continue;
            }
        }
        if turn.is_empty() {
            continue;
        }
        let plan = merge_injection_batch(&turn);
        // Exhaustive on purpose, with NO catch-all arm: the three outcomes demand three different
        // responses, and folding the last two together (as "any error means try again") turns a
        // permanent fault into an unbounded spin that delivers nothing and answers nobody.
        //
        // Pi `_runAgentPrompt(appMessage)`: the turn's input IS the injected message(s). On `Taken`
        // the agent has claimed its latch with these messages already pushed onto the run's
        // transcript, so `message_end` — and therefore the durable persist — necessarily follows;
        // that is what makes acceptance a sufficient acknowledgement.
        match session.run_injection(plan.turn).await {
            Ok(InjectionOffer::Taken) => {
                for req in turn {
                    req.ack.answer(InjectOutcome::Accepted);
                }
            }
            // A user prompt claimed the latch in the gap after `wait_for_idle` returned. The inbox
            // holds every turn message again and the next pass parks until that run settles.
            Ok(InjectionOffer::AgentBusy) => {
                inbox = turn;
                tokio::task::yield_now().await;
            }
            Err(fault) => {
                tracing::warn!(error = %fault, "session cannot accept injected messages");
                for req in turn {
                    req.ack.answer(InjectOutcome::SessionUnavailable);
                }
            }
        }
    }
}

/// Move every request already queued on `rx` into `inbox` without waiting. Returns `false` once the
/// channel is closed and empty — every producer is gone.
fn drain_ready(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<InjectRequest>,
    inbox: &mut Vec<InjectRequest>,
) -> bool {
    loop {
        match rx.try_recv() {
            Ok(req) => inbox.push(req),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return true,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return false,
        }
    }
}

/// Current wall-clock time in milliseconds (Pi `Date.now()`); 0 on a clock fault.
fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}
