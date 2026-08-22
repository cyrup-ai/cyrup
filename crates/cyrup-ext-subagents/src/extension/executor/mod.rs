//! The [`SubagentExecutor`] itself: its state, its construction, and the accessors every
//! other `executor::*` submodule and both `Tool` adapters reach it through.
//!
//! This is the ONE shared code path the `subagent` tool and every slash command route
//! through (R-SA-130). It holds no per-call state; every method takes what it needs as
//! parameters.

pub(crate) mod background;
pub(crate) mod chain;
pub(crate) mod control;
pub(crate) mod foreground;
pub(crate) mod nested_control;
pub(crate) mod notices;
pub(crate) mod paths;
pub(crate) mod reports;
pub(crate) mod requests;
pub(crate) mod resolve;
pub(crate) mod session_state;
pub(crate) mod spawn_budget;
pub(crate) mod status;

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex as AsyncMutex;

use crate::background::tracker::JobTracker;
use crate::registration::SubagentExtensionConfig;
use crate::extension::executor::notices::ForegroundControlEntry;
use crate::extension::executor::session_state::ParentModelMemory;
use crate::extension::executor::spawn_budget::SpawnBudget;

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
    /// SUBA-034 — the in-process completion bus (pi's `SUBAGENT_ASYNC_COMPLETE_EVENT`). Published
    /// into by the watcher installed above (as one member of its observer fan-out) and subscribed
    /// to by the `wait` tool, so a wait wakes on the observation of a terminal result instead of
    /// re-discovering it on its own 1 s cadence.
    ///
    /// Owned here rather than created per-install because it must outlive any single watcher: the
    /// session's watcher is REPLACED on every `SessionStart`, and a bus recreated with it would
    /// hand every already-subscribed waiter a `Closed` receiver.
    completion_bus: crate::background::watch::CompletionBus,
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
    /// `resetSessionState`, `extension/index.ts:695-706` @v0.43.0).
    spawn_budget: std::sync::Mutex<SpawnBudget>,
    /// pi `state.lastParentModel` (`shared/types.ts`; written by `rememberParentModel`,
    /// `subagent-executor.ts:284-291` @v0.43.0): the last well-formed parent-session model observed
    /// in THIS session, so a dispatch that arrives while the live `ctx.model` read is momentarily
    /// unavailable still inherits the model the session has been running on instead of collapsing
    /// to an empty ladder. Read through [`SubagentExecutor::remembered_parent_model`], which owns
    /// the whole state machine; never read directly.
    parent_model_memory: std::sync::Mutex<ParentModelMemory>,
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
            completion_bus: crate::background::watch::CompletionBus::new(),
            host_services: Arc::new(OnceLock::new()),
            root_parent_session: Arc::new(std::sync::Mutex::new(None)),
            root_parent_session_name: Arc::new(std::sync::Mutex::new(None)),
            steer: Arc::new(crate::tui::intercom::NoTransportSteerChannel),
            delivery: Arc::new(crate::tui::intercom::NoTransportChannel),
            clarify: Arc::new(crate::tui::intercom::AskLock::new_with_no_live_channel()),
            foreground_controls: Arc::new(std::sync::Mutex::new(HashMap::new())),
            spawn_budget: std::sync::Mutex::new(SpawnBudget::default()),
            parent_model_memory: std::sync::Mutex::new(ParentModelMemory::default()),
            notices: Arc::new(AsyncMutex::new(crate::tui::notices::ControlNoticeState::new())),
            control_notice_sink_override: None,
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

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
        seed_orphaned_run(dir.path(), "run0alive000", Some("session-a"), Some(std::process::id()));

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
        assert!(listing.contains("run0alive000"), "a refused dismissal changes nothing: {listing}");
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

        let err = dispatch_tool(&tool, serde_json::json!({ "action": "dismiss", "id": "run0childsafe" }))
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
        assert!(listing.contains("run0childsafe"), "the run must be untouched: {listing}");
    }

    /// A non-goal mission, and a goal mission owned by a DIFFERENT session, raise nothing.
    #[tokio::test]
    async fn the_goal_scan_ignores_non_goal_and_foreign_session_missions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        arm_scoped_missions(&executor, dir.path()).await;
        let services: Arc<dyn cyrup_ext::host::HostServices> =
            Arc::new(FixedSessionIdHost { id: Some("mine".to_string()), file: None });
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
        assert_eq!(executor.raise_goal_continuation_notices(dir.path()).await, 0);
    }

}
