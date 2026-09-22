//! The [`SubagentExecutor`] itself: its state, its construction, and the accessors every
//! other `executor::*` submodule and both `Tool` adapters reach it through.
//!
//! This is the ONE shared code path the `subagent` tool and every slash command route
//! through (R-SA-130). It holds no per-call state; every method takes what it needs as
//! parameters.

pub(crate) mod background;
pub(crate) mod chain;
pub(crate) mod control;
pub(crate) mod detach;
pub(crate) mod foreground;
pub(crate) mod foreground_actions;
pub(crate) mod foreground_control;
pub(crate) mod foreground_history;
pub(crate) mod foreground_transcript;
pub(crate) mod nested_control;
pub(crate) mod notices;
pub(crate) mod paths;
pub(crate) mod reports;
pub(crate) mod requests;
pub(crate) mod resolve;
pub(crate) mod scheduled_runs;
pub(crate) mod session_state;
pub(crate) mod spawn_budget;
pub(crate) mod status;
pub(crate) mod wait_subscriptions;
pub(crate) mod workflow;
pub(crate) mod workflow_child_stops;
pub(crate) mod workflow_controllers;
pub(crate) mod workflow_detach;
pub(crate) mod workflow_launch;
pub(crate) mod workflow_steering;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex as AsyncMutex;

use crate::background::tracker::JobTracker;
use crate::extension::executor::notices::ForegroundControlEntry;
use crate::extension::executor::session_state::{ParentModelMemory, ParentThinkingMemory};
use crate::extension::executor::spawn_budget::SpawnBudget;
use crate::extension::executor::workflow_controllers::WorkflowController;
use crate::registration::SubagentExtensionConfig;

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
    /// at install time": a live [`crate::background::watch::HostServicesCompletionSink`] when the P-1 `host_services` slot is
    /// bound (R-SA-101, the real turn-injecting sink), else the graceful-degradation
    /// [`crate::background::watch::LoggingCompletionSink`] (log + delete). Set via
    /// [`SubagentExecutor::with_completion_sink`].
    completion_sink_override: Option<Arc<dyn crate::background::watch::CompletionSink>>,
    /// The live [`crate::background::watch::CompletionWatcherHandle`] for the current session's
    /// `ResultsDir`, installed on `SessionStart` ([`SubagentExecutor::install_completion_watcher`])
    /// and retained here so the watch stays live for the session's lifetime (dropping it stops the
    /// watch). Re-installing replaces (and thereby tears down) any prior handle.
    completion_watcher: AsyncMutex<Option<crate::background::watch::CompletionWatcherHandle>>,
    /// The detached retention sweep scheduled alongside that watcher (pi's
    /// `resultIndexCleanupTimer`, `extension/index.ts:445-452`).
    ///
    /// Retained for the same reason upstream retains its timer handle and `clearTimeout`s it at
    /// teardown (`extension/index.ts:1044`): `install_completion_watcher` is re-run on EVERY
    /// `SessionStart`, and without an abort a rapid session loop would stack one sleeping sweep per
    /// start. Aborting is also all that is needed — the sweep holds nothing but a path.
    retention_sweep: AsyncMutex<Option<crate::extension::executor::notices::RetentionSweepHandle>>,
    /// SUBA-034 — the in-process completion bus (pi's `SUBAGENT_ASYNC_COMPLETE_EVENT`). Published
    /// into by the watcher installed above (as one member of its observer fan-out) and subscribed
    /// to by the `wait` tool, so a wait wakes on the observation of a terminal result instead of
    /// re-discovering it on its own 1 s cadence.
    ///
    /// Owned here rather than created per-install because it must outlive any single watcher: the
    /// session's watcher is REPLACED on every `SessionStart`, and a bus recreated with it would
    /// hand every already-subscribed waiter a `Closed` receiver.
    completion_bus: crate::background::watch::CompletionBus,
    /// This executor's consumed-payload record (pi `SubagentState.completedResults`,
    /// `shared/types.ts:2260`): the seam that keeps a completion visible to `wait` after its
    /// payload has been consumed and deleted (`watch/install.rs`'s delete-last).
    ///
    /// Owned here for the same reason [`Self::completion_bus`] above is: the session's completion
    /// watcher is REPLACED on every `SessionStart`, and a store recreated with it would drop every
    /// record a `wait` in flight is about to read.
    wait_completions: std::sync::Arc<crate::background::wait_completions::WaitCompletionStore>,
    /// SCOPE_11 — this session's durable wait-subscription manager
    /// ([`crate::background::wait_subscriptions`]), or `None` when none is installed.
    ///
    /// Installed on `SessionStart` and only for a session with a UI (pi's `ctx?.hasUI` gate,
    /// `wait-tool.ts:33`); torn down on `SessionShutdown`. A slot rather than a value, and a
    /// SHARED slot rather than a snapshot, because the completion observer registered into the
    /// watcher's composite must reach whichever manager is current at observation time — the two
    /// are rebuilt on independent `SessionStart` edges.
    wait_subscriptions: wait_subscriptions::WaitSubscriptionSlot,
    /// SUBA-016 — the scheduled-run manager for this session, installed on `SessionStart` and
    /// disposed on `SessionShutdown`. A SHARED slot for the same reason
    /// [`Self::wait_subscriptions`] is one: the `schedule.*` tool arm must reach whichever manager
    /// is current at dispatch time, and the two are rebuilt on independent `SessionStart` edges.
    scheduled_runs: scheduled_runs::Slot,
    /// `ASYNC_NOTIFY_BUG_REPORT` F3.5 — the claim/answer ledger the `wait` tool (and the headless
    /// auto-drain) share with the completion watcher's delivery decorator
    /// ([`crate::background::watch::InlineAnsweredSink`]), so a value a live wait already
    /// surfaced inline is not injected a second time as a standalone notification.
    ///
    /// Executor-owned for the same reason `completion_bus` above is: the watcher is REPLACED on
    /// every `SessionStart`, and a ledger recreated with it would drop the claims of a wait
    /// already in flight.
    inline_answers: crate::background::watch::InlineAnswerLedger,
    /// The late-bound live capability backend (P-1, reconciliation §2 item 1). Captured by
    /// [`cyrup_ext::native::NativeExtension::set_host_services`] (which the builder calls via
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
    /// [`crate::extension::SubagentsExtension::with_channels`] → [`SubagentExecutor::with_channels`]. Consumed by
    /// [`Self::control_resume`]'s `SteerRunning` arm to DELIVER `action='resume'`'s follow-up to a
    /// still-running async child over the broker (pi `subagent-executor.ts:860-878`).
    steer: Arc<dyn crate::tui::intercom::SteerChannel>,
    /// The out-of-band grouped-result delivery channel (R-SA-123/124/125). Defaults to
    /// [`crate::tui::intercom::NoTransportChannel`] (always "not delivered", full inline preserved);
    /// the intercom companion's broker-backed `DeliveryChannel` is threaded in via
    /// [`crate::extension::SubagentsExtension::with_channels`] → [`SubagentExecutor::with_channels`].
    delivery: Arc<dyn crate::tui::intercom::DeliveryChannel>,
    /// The single-slot clarify/ask lock (R-SA-119/120) backed by a [`crate::tui::intercom::ClarifyChannel`].
    /// Defaults to [`crate::tui::intercom::AskLock::new_with_no_live_channel`]; the intercom companion's
    /// broker-backed `ClarifyChannel` is threaded in via [`crate::extension::SubagentsExtension::with_channels`]. Consumed
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
    /// VL-S6 — pi `state.herdrProjectPanes` (`extension/index.ts:864`, `:980-981`): the project
    /// panes this session knows about, keyed by project root.
    ///
    /// Lives on the executor rather than on the extension because it has TWO readers on opposite
    /// sides of the seam — `SessionStart` restores it
    /// ([`crate::inspectors::herdr::restore_herdr_project_pane_snapshots`]) and
    /// [`Self::fleet_state`] projects it onto the roster — and the executor is the one thing both
    /// already hold. A `std::sync::Mutex`, matching `foreground_controls`: every access is a
    /// short synchronous read or swap, never held across an `await`.
    herdr_project_panes: Arc<std::sync::Mutex<crate::inspectors::types::ProjectPaneSnapshots>>,
    /// pi `state.workflowControllers` (`shared/types.ts:2269-2270`; created `subagent-executor.ts:4828`,
    /// populated `:5098`, drained `:5272`/`:5796`, aborted-and-cleared `extension/index.ts:1038-1041`):
    /// every workflow shell THIS process is currently driving, keyed by its workflow run id
    /// (WORKFLOW_6 §2).
    ///
    /// Keyed by [`crate::background::RunId`], not `String` — unlike `foreground_controls` above,
    /// which is keyed by `String` only because `resolve_nested_control_request` and
    /// `is_live_foreground_run` do PREFIX matching over its keys (`nested_control.rs:200-210`).
    /// Nothing prefix-matches a workflow id: every consumer does an exact `has(runId)`/`keys()`, so
    /// the typed key is free and keeps the parse at the boundary.
    ///
    /// `std::sync::Mutex`, matching `foreground_controls`: every access is a short synchronous
    /// insert/remove/contains with no `.await` inside the critical section.
    workflow_controllers:
        Arc<std::sync::Mutex<HashMap<crate::background::RunId, WorkflowController>>>,
    /// pi `state.workflowChildStops` (`shared/types.ts:2315-2316`; created
    /// `subagent-executor.ts:4958`, written by the engine's `registerStopChild` registrar at
    /// `:5691-5694`, deleted at settlement `:5927`, cleared at teardown `extension/index.ts:1042`):
    /// the live per-child stop handle of every workflow shell THIS process is driving, keyed the
    /// same way `workflow_controllers` above is (WORKFLOW_18 §2.1).
    ///
    /// A SEPARATE map from `workflow_controllers`, deliberately and for upstream's own reason: a
    /// controller aborts a WORKFLOW, a stop handle stops ONE CHILD of one. Fusing them into a
    /// single entry would make "stop child b" abort the run.
    ///
    /// ⚠ The value pins the engine's whole `Arc<RunShared>` — every child result, trace entry and
    /// console line of the run. The registrar's `None` arm MUST remove the entry, never tombstone
    /// it; see [`workflow_child_stops`]'s module doc for the full lifetime contract.
    ///
    /// `std::sync::Mutex`, matching both siblings: every access is a short synchronous
    /// insert/remove/clone with no `.await` inside the critical section, and the handle itself is
    /// a synchronous callback the engine invokes from arbitrary host threads.
    workflow_child_stops: Arc<
        std::sync::Mutex<
            HashMap<crate::background::RunId, crate::workflows::scripted::WorkflowStopChild>,
        >,
    >,
    /// pi `state.foregroundRuns` (`shared/types.ts`; written by `rememberForegroundRun`,
    /// `subagent-executor.ts:749-753`; bounded at `:716-722`): settled foreground runs still worth
    /// inspecting, keyed by run id (WORKFLOW_7 §2.5).
    ///
    /// Distinct from `foreground_controls` above in exactly one way that matters: an entry here is
    /// created when a run SETTLES, and `foreground_controls`' entry for the SAME id is removed at
    /// that exact point (`foreground.rs::settle_foreground_run`) — the two maps are disjoint by
    /// construction, which is what `tui/fleet.rs`'s `!active_foreground_ids.contains(...)` assumes
    /// and what pi's own `fleet-view.ts:406-408` filter re-checks.
    ///
    /// A SUPERSET of what `foreground_history::persist` writes to disk: every settled status is
    /// remembered here; only the four RESTORABLE ones (`foreground_history::record::RESTORABLE`)
    /// are ever persisted.
    foreground_runs: Arc<
        std::sync::Mutex<
            HashMap<
                crate::background::RunId,
                crate::extension::executor::foreground_history::ForegroundHistoryRun,
            >,
        >,
    >,
    /// The per-SESSION subagent spawn budget (pi `SubagentState.subagentSpawns`,
    /// `shared/types.ts:842`: `{ sessionId: string | null; count: number }`). Charged UP FRONT by
    /// [`Self::reserve_subagent_spawns`] at every accepted execution dispatch, so a run that later
    /// fails still consumes its reservation — exactly pi's `reserveSubagentSpawns`
    /// (`runs/foreground/subagent-executor.ts:266-282`), which sets `count = used + requested`
    /// before any child is planned and never refunds. Reset when the recorded session id no longer
    /// matches the live one, and again at `SessionStart` ([`Self::reset_spawn_budget`], pi
    /// `resetSessionState`, `extension/index.ts:695-706` @v0.43.0).
    spawn_budget: std::sync::Mutex<SpawnBudget>,
    /// pi `state.lastParentModel` (`shared/types.ts`; written by `rememberParentModel`,
    /// `subagent-executor.ts:284-291` @v0.43.0): the last well-formed parent-session model observed
    /// in THIS session, so a dispatch that arrives while the live `ctx.model` read is momentarily
    /// unavailable still inherits the model the session has been running on instead of collapsing
    /// to an empty ladder. Read through [`SubagentExecutor::remembered_parent_model`], which owns
    /// the whole state machine; never read directly.
    parent_model_memory: std::sync::Mutex<ParentModelMemory>,
    /// SCOPE_19/A1 — the thinking twin of [`Self::parent_model_memory`]: the last recognized
    /// reasoning level the live parent session reported, so a dispatch that lands while
    /// `HostServices::thinking_level()` momentarily answers `None` still inherits the level the
    /// session has been reasoning at instead of silently dropping the child to its persona's (or
    /// to off). Read through [`SubagentExecutor::remembered_parent_thinking`], which owns the whole
    /// state machine; never read directly.
    parent_thinking_memory: std::sync::Mutex<ParentThinkingMemory>,
    /// The control-notice debounce/actionability/dedup state machine (pi
    /// `extension/control-notices.ts`: its `pendingForegroundControlNotices` timer map + the
    /// `__piSubagentVisibleControlNotices` global dedup set). Held on the EXECUTOR — not rebuilt
    /// per run — because both halves must outlive any single run: the dedup set is at-most-once for
    /// the process (R-SA-115/122, pi's own reload-surviving global store), and a foreground
    /// notice's 1s debounce timer routinely outlives the run that raised it.
    notices: Arc<AsyncMutex<crate::tui::notices::ControlNoticeState>>,
    /// pi's module-scoped `goalTurnId` (`extension/index.ts:589`'s `goalTurnId += 1`): a
    /// monotonically increasing turn counter folded into every goal-continuation notice's
    /// synthetic run id (`goal-<missionId>-turn-<n>`), so an idle goal mission raises a FRESH,
    /// non-deduplicated notice each turn rather than being suppressed by the at-most-once dedup
    /// after the first.
    goal_turn_id: Arc<std::sync::atomic::AtomicU64>,
    /// An EXPLICITLY-injected control-notice delivery sink (a test's capturing sink, or a caller
    /// wiring its own transcript surface). `None` — the production default — derives the effective
    /// sink per delivery: a live [`crate::tui::notices::HostServicesControlNoticeSink`] when the
    /// P-1 `host_services` slot is bound (pi's `pi.sendMessage`), else the stderr
    /// [`crate::tui::notices::LoggingControlNoticeSink`] degradation.
    control_notice_sink_override: Option<Arc<dyn crate::tui::notices::ControlNoticeSink>>,
    /// SUBA-084 — this executor's partition of pi's runtime agent registry
    /// (`runtime-agent-registry.ts:71-74` @v0.64.0 keys records on the owning `ExtensionAPI`;
    /// here the owner IS the executor). Read into every [`Self::discovery_config`] so a registered
    /// agent reaches each discovery consumer; cleared by [`Self::teardown_session`]
    /// (`clearRuntimeAgentsForPi`, `extension/index.ts:971`).
    runtime_agents: Arc<crate::discovery::runtime_registry::RuntimeAgentRegistry>,
    /// SCOPE_3d — the session-scoped workflow resource registry (pi's
    /// `Symbol.for("pi-subagents.workflow-resources.v1")` global, `workflow-resources.ts:45-56`).
    /// Hung off the executor rather than a `static`/`OnceLock` for the same reason
    /// `completion_bus` above is executor-owned: a `static` registry cannot be reset between
    /// sessions, and pi's own store is scoped to the extension host, not the process.
    workflow_resources: crate::workflows::WorkflowResourceRegistry,
    /// SCOPE_3j — the cached model-exclusion registry (pi `runs/shared/model-exclusions.ts`'s
    /// module-global `exclusions`/`loaded` state), owned here for the same reason
    /// `workflow_resources` above is: a `static` registry cannot be reset between sessions, and
    /// pi's own store is scoped to the extension host, not the process.
    ///
    /// One store per executor, shared by every concurrent run it drives — which is what makes a
    /// failure recorded by one run visible to the next one's ladder without a reload.
    model_exclusions: Arc<crate::exec::model_exclusions::ModelExclusionStore>,
}

impl Default for SubagentExecutor {
    fn default() -> Self {
        Self::new()
    }
}
impl SubagentExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: Arc::new(AsyncMutex::new(SubagentExtensionConfig::default())),
            goal_turn_id: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            tracker: Arc::new(JobTracker::new()),
            completion_sink_override: None,
            completion_watcher: AsyncMutex::new(None),
            retention_sweep: AsyncMutex::new(None),
            completion_bus: crate::background::watch::CompletionBus::new(),
            wait_completions: Arc::new(
                crate::background::wait_completions::WaitCompletionStore::default(),
            ),
            wait_subscriptions: Arc::new(std::sync::Mutex::new(None)),
            scheduled_runs: Arc::new(std::sync::Mutex::new(None)),
            inline_answers: crate::background::watch::InlineAnswerLedger::default(),
            host_services: Arc::new(OnceLock::new()),
            root_parent_session: Arc::new(std::sync::Mutex::new(None)),
            root_parent_session_name: Arc::new(std::sync::Mutex::new(None)),
            steer: Arc::new(crate::tui::intercom::NoTransportSteerChannel),
            delivery: Arc::new(crate::tui::intercom::NoTransportChannel),
            clarify: Arc::new(crate::tui::intercom::AskLock::new_with_no_live_channel()),
            foreground_controls: Arc::new(std::sync::Mutex::new(HashMap::new())),
            herdr_project_panes: Arc::new(std::sync::Mutex::new(
                crate::inspectors::types::ProjectPaneSnapshots::new(),
            )),
            workflow_controllers: Arc::new(std::sync::Mutex::new(HashMap::new())),
            workflow_child_stops: Arc::new(std::sync::Mutex::new(HashMap::new())),
            foreground_runs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            spawn_budget: std::sync::Mutex::new(SpawnBudget::default()),
            parent_model_memory: std::sync::Mutex::new(ParentModelMemory::default()),
            parent_thinking_memory: std::sync::Mutex::new(ParentThinkingMemory::default()),
            notices: Arc::new(AsyncMutex::new(
                crate::tui::notices::ControlNoticeState::new(),
            )),
            control_notice_sink_override: None,
            runtime_agents: Arc::new(
                crate::discovery::runtime_registry::RuntimeAgentRegistry::new(),
            ),
            workflow_resources: crate::workflows::WorkflowResourceRegistry::new(),
            model_exclusions: Arc::new(
                crate::exec::model_exclusions::ModelExclusionStore::from_env(),
            ),
        }
    }

    /// SCOPE_3d — the session-scoped workflow resource registry this executor owns (see the
    /// field's doc). Register through it with a live [`crate::identity::SessionId`]; dispose a
    /// session's registrations at `session_shutdown` via
    /// [`crate::workflows::WorkflowResourceRegistry::dispose_session`] — issued permits remain
    /// valid (pi `workflow-resources.ts:58`).
    #[must_use]
    pub fn workflow_resources(&self) -> &crate::workflows::WorkflowResourceRegistry {
        &self.workflow_resources
    }

    /// Re-root the cached-exclusion registry once the extension's real [`crate::paths::Roots`] are
    /// known.
    ///
    /// [`Self::new`] has no config to read, so it seeds the store from the process environment;
    /// [`crate::extension::host`] then replaces it with one rooted on the config's own roots before
    /// the executor is shared. `&mut self` is the enforcement: this can only happen while the
    /// executor is still sole-owned, so no run can observe the store changing under it.
    pub(crate) fn replace_model_exclusions(
        &mut self,
        store: Arc<crate::exec::model_exclusions::ModelExclusionStore>,
    ) {
        self.model_exclusions = store;
    }

    /// SCOPE_3j — the cached model-exclusion registry this executor owns (see the field's doc).
    /// Cloned onto every [`crate::exec::RunOptions`] this executor builds, so the ladder both
    /// filters against it and records into it.
    #[must_use]
    pub fn model_exclusions(&self) -> Arc<crate::exec::model_exclusions::ModelExclusionStore> {
        Arc::clone(&self.model_exclusions)
    }

    /// SUBA-084 — pi's public `registerAgent({ pi, name, definition })` (`src/api/agents.ts:2`
    /// re-exporting `registerRuntimeAgent`, `runtime-agent-registry.ts:371-398` @v0.64.0): define
    /// an agent in-process, with no file and no settings write. It is visible to the very next
    /// discovery (tool routing, `/run`, chains, the management `list`) and stays until the
    /// returned handle's `dispose()` or this session's teardown. See
    /// [`crate::discovery::runtime_registry`] for the validation and collision contract.
    ///
    /// # Errors
    ///
    /// Every upstream refusal (name/definition validation, a reserved code-owned selection name,
    /// a builtin or runtime identity collision, the 200-per-owner cap) as
    /// [`crate::error::SubagentError::Management`] with upstream's text.
    pub fn register_agent(
        &self,
        name: &str,
        definition: &crate::discovery::runtime_registry::RuntimeAgentDefinition,
    ) -> Result<
        crate::discovery::runtime_registry::RuntimeAgentRegistration,
        crate::error::SubagentError,
    > {
        self.runtime_agents.register(name, definition)
    }

    /// SUBA-084 — this executor's runtime agent registry (the `pi`-keyed partition of
    /// `runtime-agent-registry.ts:71-74`), for a caller that needs `list()`/`clear()` or the
    /// untyped `register_value` path directly.
    #[must_use]
    pub fn runtime_agents(&self) -> &Arc<crate::discovery::runtime_registry::RuntimeAgentRegistry> {
        &self.runtime_agents
    }

    /// Construct an executor whose background-completion notifications (C6) are delivered to
    /// `sink` instead of the default graceful-degradation logging sink — the seam a host uses to
    /// route completions into a live session's turn loop (R-SA-101), and a test uses to capture
    /// them. Explicitly overriding the sink here wins over the P-1 `host_services`-derived
    /// [`crate::background::watch::HostServicesCompletionSink`] at install time (so a test's scripted sink is authoritative).
    #[must_use]
    pub fn with_completion_sink(sink: Arc<dyn crate::background::watch::CompletionSink>) -> Self {
        Self {
            completion_sink_override: Some(sink),
            ..Self::new()
        }
    }

    /// Construct an executor that starts from `config` rather than
    /// [`SubagentExtensionConfig::default`]. [`Self::new`] leaves a caller able to build this type
    /// but not to configure it — the extension itself reaches past that through the `pub(crate)`
    /// [`Self::config_cell`], which no other crate can use. This is that seam, public: the same
    /// `with_*` shape [`Self::with_completion_sink`] already establishes.
    #[must_use]
    pub fn with_config(config: SubagentExtensionConfig) -> Self {
        Self {
            config: Arc::new(AsyncMutex::new(config)),
            ..Self::new()
        }
    }

    /// Late-bind the live capability backend (P-1). Called by
    /// [`cyrup_ext::native::NativeExtension::set_host_services`] (which the builder invokes via
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

    /// SUBA-034 — a handle on this orchestrator's completion bus (pi's
    /// `SUBAGENT_ASYNC_COMPLETE_EVENT`), for a caller that needs to WAKE on completions rather than
    /// consume them: the `wait` tool subscribes through this.
    ///
    /// Cloning shares the one underlying channel, so a subscriber taken before a `SessionStart`
    /// re-installs the watcher keeps receiving from the new one.
    #[must_use]
    pub fn completion_bus(&self) -> crate::background::watch::CompletionBus {
        self.completion_bus.clone()
    }

    /// A handle on this executor's consumed-payload record (see the field), for the `wait`
    /// surfaces that resolve a completion the watcher has already deleted. Cloning the `Arc`
    /// shares the one map, mirroring [`Self::completion_bus`].
    #[must_use]
    pub fn wait_completions(
        &self,
    ) -> std::sync::Arc<crate::background::wait_completions::WaitCompletionStore> {
        Arc::clone(&self.wait_completions)
    }

    /// `ASYNC_NOTIFY_BUG_REPORT` F3.5 — a handle on this executor's inline-answer ledger (see the
    /// field), for the `wait` surfaces that claim/answer runs and for the watcher install that
    /// wraps the completion sink in [`crate::background::watch::InlineAnsweredSink`]. Cloning
    /// shares the one underlying state, mirroring [`Self::completion_bus`].
    #[must_use]
    pub fn inline_answers(&self) -> crate::background::watch::InlineAnswerLedger {
        self.inline_answers.clone()
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

    /// Current effective extension config snapshot (tier 3 of R-SA-133).
    pub async fn config_snapshot(&self) -> SubagentExtensionConfig {
        self.config_cell().lock().await.clone()
    }

    /// LANES_2 — whether a FOREGROUND run this process launched is provably over, for
    /// [`crate::spawn::cleanup_plan`]'s ownership probe. Port of the closure upstream injects at
    /// `subagent-executor.ts:6230-6235` @v0.68.0:
    ///
    /// ```text
    /// if (deps.state.foregroundControls.has(runId)) return "active";
    /// const remembered = deps.state.foregroundRuns?.get(runId);
    /// if (!remembered || remembered.children.length === 0
    ///     || remembered.children.some((c) => c.status === "detached")) return "unknown";
    /// return "terminal";
    /// ```
    ///
    /// The two maps are disjoint by construction (see [`Self::foreground_runs`]'s own note): an
    /// entry appears in `foreground_runs` at the exact moment its `foreground_controls` entry is
    /// removed. So "live" and "remembered and fully settled" are the only two provable answers,
    /// and **everything else is [`ForegroundRunOwnership::Unknown`]** — a run this process never
    /// saw, a run whose memory was evicted, or a run with a detached child that may still be
    /// writing into its worktree. Absence of proof is never proof of termination, which is why
    /// the caller treats `Unknown` as non-removable.
    #[must_use]
    pub(crate) fn foreground_run_ownership(
        &self,
        run_id: &str,
    ) -> crate::spawn::cleanup_plan::model::ForegroundRunOwnership {
        use crate::spawn::cleanup_plan::model::ForegroundRunOwnership;

        if self
            .foreground_controls
            .lock()
            .is_ok_and(|controls| controls.contains_key(run_id))
        {
            return ForegroundRunOwnership::Active;
        }
        let Ok(runs) = self.foreground_runs.lock() else {
            return ForegroundRunOwnership::Unknown;
        };
        let Some(remembered) = runs
            .iter()
            .find(|(id, _)| id.as_str() == run_id)
            .map(|(_, run)| run)
        else {
            return ForegroundRunOwnership::Unknown;
        };
        if remembered.children.is_empty()
            || remembered
                .children
                .iter()
                .any(|child| child.status == "detached")
        {
            return ForegroundRunOwnership::Unknown;
        }
        ForegroundRunOwnership::Terminal
    }

    /// VL-S6 — pi `extension/index.ts:980-981`'s session-start restore, applied to this
    /// session's project-pane map.
    ///
    /// The root set is upstream's own union, verbatim: the keys already known, plus every root
    /// the owner's on-disk index names, plus the owner root itself.
    ///
    /// ADDITIVE, exactly as upstream is (`new Map(state.herdrProjectPanes ?? [])`,
    /// `project-panes.ts:303`, and this crate's
    /// [`crate::inspectors::herdr::restore_herdr_project_pane_snapshots`] says so on itself): a
    /// root whose binding has since been removed KEEPS whatever the session already knew rather
    /// than being silently dropped. `project.close` is what removes an entry, and it removes the
    /// binding with it.
    ///
    /// Reads binding files only — it never calls herdr — so a session starts at the same speed on
    /// a box with no herdr installed.
    pub(crate) fn restore_herdr_project_panes(&self, owner_root: &Path) {
        let mut panes = self
            .herdr_project_panes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut roots: Vec<PathBuf> = panes.keys().cloned().collect();
        roots.extend(
            crate::inspectors::herdr::list_herdr_project_pane_roots(owner_root).unwrap_or_default(),
        );
        roots.push(owner_root.to_path_buf());
        crate::inspectors::herdr::restore_herdr_project_pane_snapshots(
            &mut panes,
            roots,
            crate::time::now_epoch_millis(),
        );
    }

    /// The LIVE project-pane map, for the `project.*` handler to write through.
    ///
    /// pi threads `state.herdrProjectPanes` into `handleHerdrProjectPaneAction`
    /// (`extension/index.ts:864`) and reads the same map back through its `getProjectPaneCount`
    /// closure; this is that thread. Without it the map's only writer is
    /// [`Self::restore_herdr_project_panes`] at `SessionStart`, so both readers —
    /// [`Self::herdr_project_pane_snapshots`] (the roster) and
    /// [`Self::open_herdr_project_pane_count`] (the herdr pane label's `" · N panes"`) — would
    /// stay frozen at whatever that restore found, denying a pane the agent had just opened and
    /// still counting one it had just closed, until the next session start.
    #[must_use]
    pub(crate) fn herdr_project_pane_map(
        &self,
    ) -> &std::sync::Mutex<crate::inspectors::types::ProjectPaneSnapshots> {
        &self.herdr_project_panes
    }

    /// The restored project panes, flattened for [`crate::tui::fleet_state::FleetState`].
    #[must_use]
    pub(crate) fn herdr_project_pane_snapshots(
        &self,
    ) -> Vec<crate::inspectors::types::HerdrProjectPaneSnapshot> {
        self.herdr_project_panes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// VL-S6 — pi `getProjectPaneCount` (`extension/index.ts:864`), the closure the herdr status
    /// bridge folds into the pane label as `" · N panes"` (`integrations/herdr-status.ts:162-163`).
    #[must_use]
    pub(crate) fn open_herdr_project_pane_count(&self) -> usize {
        crate::inspectors::herdr::open_project_pane_count(
            &self
                .herdr_project_panes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// The shared background-job tracker (R-SA-093), so `on_event`'s `SessionStart` handler can
    /// resume tracking any runs still recorded on disk from a prior process.
    #[must_use]
    pub fn tracker(&self) -> &Arc<JobTracker> {
        &self.tracker
    }

    /// The live config cell. `pub(crate)` — not `pub` — because [`crate::extension::host`] seeds it
    /// at construction time and this crate's own tests write it, and neither is a descendant of this
    /// module any more. Prefer [`Self::config_snapshot`] for reads; this is the write handle.
    #[must_use]
    pub(crate) fn config_cell(&self) -> &AsyncMutex<SubagentExtensionConfig> {
        &self.config
    }

    /// The control-notice state machine, for the notice-pipeline tests that drive `observe_run`/
    /// `forget_run`/`has_pending` directly rather than through a whole run.
    #[must_use]
    pub(crate) fn notice_state(&self) -> &AsyncMutex<crate::tui::notices::ControlNoticeState> {
        &self.notices
    }

    /// Snapshot `state.foregroundControls` into the rows the detached-workflow reconciler's
    /// identity back-fill reads (`workflow_detach::identity`'s clause 1).
    ///
    /// This is SCOPE_8 §Y-1 shape (a), made real: the CALLER snapshots the registry and hands the
    /// rows in, rather than the reconciler reaching into a live executor handle it may not have.
    /// The snapshot is what keeps the registry's `std::sync::Mutex` — documented as "every access
    /// is a short synchronous read" — from ever being alive across the reconciler's `.await`s, and
    /// it is why this returns an owned `Vec` rather than a guard.
    ///
    /// The run id is the map KEY here and a FIELD on the row, because
    /// [`ForegroundControlEntry`] carries none (the same reason
    /// `resolve_workflow_foreground_steering_target` carries it out as a pair).
    #[must_use]
    pub(crate) fn live_foreground_controls(&self) -> Vec<workflow_detach::LiveForegroundControl> {
        self.foreground_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .map(|(run_id, entry)| workflow_detach::LiveForegroundControl {
                run_id: crate::background::RunId::from_token(run_id.clone()),
                parent_workflow_run_id: entry.parent_workflow_run_id.clone(),
                workflow_key: entry.workflow_key.clone(),
            })
            .collect()
    }

    /// The live foreground-run control registry (pi `state.foregroundControls`). `pub(crate)` for
    /// the same reason as [`Self::config_cell`]: the stop/steer surfaces' own tests seed a live
    /// foreground run through it, and they no longer live inside this module.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn foreground_controls(
        &self,
    ) -> &std::sync::Mutex<HashMap<String, ForegroundControlEntry>> {
        &self.foreground_controls
    }
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
    use crate::extension::testsupport::FixedSessionHost;
    use crate::extension::testsupport::FixedSessionIdHost;
    use crate::extension::testsupport::arm_scoped_missions;
    use crate::extension::testsupport::dispatch_tool;
    use crate::extension::testsupport::scoped_missions;
    use crate::extension::testsupport::seed_orphaned_run;
    use crate::extension::tool::SubagentTool;
    use cyrup_core::ModelId;
    use std::path::PathBuf;

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

    #[test]
    fn run_models_report_resolves_inherit_sentinel_to_parent_model_not_verbatim() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings_dir = dir.path().join(".cyrup").join("agents");
        std::fs::create_dir_all(&settings_dir).expect("mkdir settings dir");
        // A settings override that explicitly requests pi's `"inherit"` sentinel — same request as
        // leaving `model` unset, per `resolveSubagentModelOverride` (model-fallback.ts:196-220).
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
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: None,
            file: None,
        }));
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

    /// pi `async-dismiss-action.ts:37-42` — the refusal the item's Verify names verbatim. cyrup's
    /// carrier for `state.workflowControllers.has(runId)` is a liveness probe of the recorded pid
    /// (see the `[CYRUP-DELTA]` on [`SubagentExecutor::control_dismiss`]); this process's own pid
    /// is one this process can definitely signal, so it stands in for a live controller.
    ///
    /// Pre-fix: no method to call, and the tool answered `unknown subagent action 'dismiss'`.
    #[tokio::test]
    async fn dismiss_refuses_a_run_that_still_has_a_live_controller() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionHost("session-a")));
        seed_orphaned_run(
            dir.path(),
            "run0alive000",
            Some("session-a"),
            Some(std::process::id()),
        );

        let err = executor
            .control_dismiss(dir.path(), Some("run0alive000"))
            .await
            .expect_err("a run with a live controller must be refused");
        assert_eq!(
            err,
            "Workflow 'run0alive000' still has a live controller and cannot be dismissed."
        );

        // And the refusal must be total: no marker was written, so the run is still listed.
        let listing = executor
            .control_status(dir.path(), None, None, false)
            .await
            .expect("status list");
        assert!(
            listing.contains("run0alive000"),
            "a refused dismissal changes nothing: {listing}"
        );
    }

    /// pi `subagent-executor.ts:5865-5870`: `dismiss` is in upstream's
    /// `MUTATING_MANAGEMENT_ACTIONS` (`:175`) and the child-safe gate runs immediately BEFORE the
    /// `if (action === "dismiss")` block, so a fanout child never reaches the handler.
    ///
    /// Pre-fix this asserted the wrong sentence entirely: with no `dismiss` arm the child got the
    /// unknown-action did-you-mean message, which both fails to refuse and advertises nothing.
    #[tokio::test]
    async fn dismiss_is_refused_from_child_safe_fanout_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        arm_scoped_missions(&executor, dir.path()).await;
        executor.set_host_services(Arc::new(FixedSessionHost("session-a")));
        seed_orphaned_run(dir.path(), "run0childsafe", Some("session-a"), None);
        let tool = SubagentTool::new_child_safe(executor.clone(), dir.path().to_path_buf());

        let err = dispatch_tool(
            &tool,
            serde_json::json!({ "action": "dismiss", "id": "run0childsafe" }),
        )
        .await
        .expect_err("a fanout child must be refused");
        assert_eq!(
            err.to_string(),
            "Action 'dismiss' is not available from child-safe subagent fanout mode."
        );

        // The refusal must be a real gate, not just a different message: no marker was written.
        let listing = executor
            .control_status(dir.path(), None, None, false)
            .await
            .expect("status list");
        assert!(
            listing.contains("run0childsafe"),
            "the run must be untouched: {listing}"
        );
    }

    /// A non-goal mission, and a goal mission owned by a DIFFERENT session, raise nothing.
    #[tokio::test]
    async fn the_goal_scan_ignores_non_goal_and_foreign_session_missions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        arm_scoped_missions(&executor, dir.path()).await;
        let services: Arc<dyn cyrup_ext::host::HostServices> = Arc::new(FixedSessionIdHost {
            id: Some("mine".to_string()),
            file: None,
        });
        executor.set_host_services(services);
        let location = crate::missions::resolve_mission_store_location(
            dir.path(),
            Some(&scoped_missions(dir.path())),
            None,
        );
        crate::missions::create_mission(
            &location,
            &crate::missions::MissionCreateInput {
                title: "Plain".to_string(),
                objective: "no goal".to_string(),
                status: Some(crate::missions::MissionStatus::Active),
                owner_session_id: Some("mine".to_string()),
                ..Default::default()
            },
            0,
            None,
        )
        .expect("create");
        crate::missions::create_mission(
            &location,
            &crate::missions::MissionCreateInput {
                title: "Theirs".to_string(),
                objective: "someone else's goal".to_string(),
                goal: Some(true),
                budget: Some(crate::missions::MissionTokenBudget { tokens: 100 }),
                status: Some(crate::missions::MissionStatus::Active),
                labels: None,
                owner_session_id: Some("theirs".to_string()),
            },
            0,
            None,
        )
        .expect("create");
        assert_eq!(
            executor.raise_goal_continuation_notices(dir.path()).await,
            0
        );
    }

    /// The OBSERVATION SEAM, asserted as the exact expression all three `&self` launch sites use.
    ///
    /// This is what can actually break in the threading, and none of it is provable from the
    /// `RunOptions` literals themselves:
    ///
    /// * the `.as_deref()` coercion — `host_services()` hands back `Option<Arc<dyn HostServices>>`
    ///   and `host_builtin_tool_names` takes `Option<&dyn HostServices>`; `.as_ref()` yields
    ///   `Option<&Arc<_>>` and does NOT coerce;
    /// * the late-bind ordering — the `OnceLock` is filled by `set_host_services` before `init`,
    ///   so a launch site running after `init` really does see a bound host; and
    /// * unbound → UNKNOWN — a headless embedder yields `None`, which makes `resolve_tool_surface`
    ///   skip the intersection rather than report every tool missing.
    ///
    /// Driving this through `build_foreground_run_options` instead would mean constructing a
    /// private 22-field `ForegroundRunOptionsInput` — built at exactly one production site and
    /// never in a test — to re-prove that `field: expr` assigns `expr`.
    #[test]
    fn the_observation_seam_reads_the_live_host() {
        let executor = SubagentExecutor::new();

        // No host bound (headless / SDK-embedder default): UNKNOWN, never "nothing is available".
        assert_eq!(
            crate::exec::tool_surface::host_builtin_tool_names(executor.host_services().as_deref()),
            None,
            "an unbound host must read as UNKNOWN, so the intersection is skipped rather than \
             refusing every review lane on a headless host"
        );

        executor.set_host_services(Arc::new(crate::exec::testsupport::RowsHost(Some(vec![
            serde_json::json!({"name": "read", "sourceInfo": {"source": "builtin"}}),
            serde_json::json!({"name": "grep", "sourceInfo": {"source": "builtin"}}),
        ]))));

        assert_eq!(
            crate::exec::tool_surface::host_builtin_tool_names(executor.host_services().as_deref()),
            Some(vec!["read".to_string(), "grep".to_string()]),
            "once a host is bound, the seam the three `&self` launch sites call must report its \
             builtin rows"
        );
    }
}
