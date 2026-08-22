//! The control-notice and completion-watcher pipeline: the foreground-control notifier pump,
//! sink resolution, watcher install/teardown and goal-continuation notices.

use std::path::Path;
use std::sync::Arc;

use cyrup_core::CancelToken;

use crate::background::RunId;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root, default_results_dir};
use crate::extension::tool::mission::MissionSyncCompletionObserver;

/// One live foreground run's control surface (pi `SubagentState.foregroundControls`'s per-entry
/// shape, `shared/types.ts`): the soft-interrupt token for its current attempt, plus the live
/// message-route coordinates (`currentAgent`/`currentIndex`) a nested-control "resume" request
/// resolves to the SAME [`crate::spawn::intercom_target::resolve_subagent_intercom_target`] string
/// the child registered its broker presence under at spawn.
#[derive(Clone)]
pub(crate) struct ForegroundControlEntry {
    /// Fires this run's soft interrupt (pi `control.interrupt?.()`); shared with the live
    /// [`RunOptions::interrupt`] token the running child's own attempt loop races against.
    pub(crate) interrupt: CancelToken,
    /// The run's current step agent name (pi `control.currentAgent`); `None` means no live message
    /// route exists yet (pi's "has no active child message route" guard).
    pub(crate) current_agent: Option<String>,
    /// The run's current step's flat child index (pi `control.currentIndex ?? 0`).
    pub(crate) current_index: Option<usize>,
    /// The run's live control activity state (pi `control.currentActivityState`, read by
    /// `isForegroundNoticeStillActionable`, `extension/control-notices.ts:59-65` @v0.34.0): a debounced
    /// foreground notice is only delivered while this is still `NeedsAttention`. Updated by
    /// [`SubagentExecutor::foreground_control_notifier`] on every raised control event (pi
    /// `applyControlEventToRememberedForegroundRun`, `subagent-executor.ts:549-570` @v0.43.0).
    pub(crate) current_activity_state: Option<crate::background::ActivityState>,
    /// pi `ForegroundRunControl.mode` (`shared/types.ts:1506`) — the agent label the fleet roster
    /// falls back to when no `currentAgent` is known (`fleet.ts:166`) and the `Mode:` line of the
    /// detail pane (`fleet.ts:248`). Fixed for a run's lifetime.
    pub(crate) mode: crate::background::RunMode,
    /// pi `ForegroundRunControl.description` (`shared/types.ts:1509`) — the caller's task, rendered
    /// as the roster row's identity and the detail pane's `Task` line (`fleet.ts:170,434-437`).
    pub(crate) description: Option<String>,
    /// pi `ForegroundRunControl.currentTool` (`fleet.ts:252`) — the tool in flight, refreshed from
    /// every raised control event alongside [`Self::current_activity_state`].
    pub(crate) current_tool: Option<String>,
    /// pi `ForegroundRunControl.currentPath` (`fleet.ts:252`).
    pub(crate) current_path: Option<String>,
    /// pi `ForegroundRunControl.turnCount` (`fleet.ts:253`).
    pub(crate) turn_count: Option<u64>,
    /// pi `ForegroundRunControl.toolCount` (`fleet.ts:254`).
    pub(crate) tool_count: Option<u64>,
    /// pi `ForegroundRunControl.tokens` (`fleet.ts:255`).
    pub(crate) tokens: Option<u64>,
    /// pi `ForegroundRunControl.startedAt` (`shared/types.ts:1507`) — epoch millis at registration.
    /// Added for the FleetView port: `fleet-status.ts:174` renders the elapsed time from it and
    /// `fleet.ts:251` renders it as the run's ISO `Started:` line, both of which would otherwise
    /// read as 1970.
    pub(crate) started_at: i64,
    /// pi `ForegroundRunControl.updatedAt` (`shared/types.ts:1508`) — the key
    /// `collectFleetSnapshot` sorts live foreground rows on, newest first (`fleet.ts:142`). Bumped
    /// on every control-event activity-state transition, which is the only liveness signal cyrup's
    /// foreground registry observes.
    pub(crate) updated_at: i64,
}

/// How long [`ForegroundControlNotifier::flush`] waits for the notice pump to acknowledge that it
/// has drained every event raised before teardown.
///
/// A bound (rather than an unbounded await) is deliberate and load-bearing: the pump is a spawned
/// task, and a runtime being shut down — or a pump that has already exited because every
/// [`crate::exec::control::ControlEventSink`] clone was dropped — must degrade to "stop waiting and
/// finish tearing the run down", never to a hang inside a tool call. The value is generous relative
/// to the work being flushed (each queued event is two short mutex sections plus arming a timer),
/// so a timeout here means something is genuinely wrong, not that the pump was merely busy.
const FOREGROUND_CONTROL_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// One message on a foreground run's ordered control-notice pump (see
/// [`SubagentExecutor::foreground_control_notifier`]).
enum ForegroundControlPumpMsg {
    /// A raised control event to project + (possibly) surface as a transcript notice. Boxed so the
    /// enum's size is not dominated by [`crate::exec::control::ControlEvent`]'s many optional
    /// fields, given the far more common `Flush` variant carries only a oneshot sender.
    Event(Box<crate::exec::control::ControlEvent>),
    /// A teardown barrier: the pump acknowledges it only after every event queued ahead of it has
    /// been fully applied.
    Flush(tokio::sync::oneshot::Sender<()>),
}

/// One foreground run's control-notice notifier: the sync [`crate::exec::control::ControlEventSink`]
/// handed to [`crate::exec::RunOptions::on_control_event`], plus the handle its owner uses to drain
/// the pump before declaring the run over.
///
/// See [`SubagentExecutor::foreground_control_notifier`] for why the pump exists at all.
pub(crate) struct ForegroundControlNotifier {
    sink: crate::exec::control::ControlEventSink,
    tx: tokio::sync::mpsc::UnboundedSender<ForegroundControlPumpMsg>,
}

impl ForegroundControlNotifier {
    /// The sink to install on [`crate::exec::RunOptions::on_control_event`].
    pub(crate) fn sink(&self) -> crate::exec::control::ControlEventSink {
        self.sink.clone()
    }

    /// Block until every control event raised so far has been applied to the notice machine's live
    /// projection — the happens-before that lets the caller run pi's teardown sequence
    /// (`clearPendingForegroundControlNotices(state, runId)` then
    /// `state.foregroundControls.delete(runId)`, `subagent-executor.ts:3579-3581` @v0.34.0) without
    /// a still-unpolled event racing in behind it and resurrecting the finished run.
    ///
    /// Never fails and never hangs: a closed channel (pump already exited) and an expired
    /// [`FOREGROUND_CONTROL_FLUSH_TIMEOUT`] both mean "there is nothing left that can be waited
    /// for", and both simply return.
    pub(crate) async fn flush(&self) {
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        if self.tx.send(ForegroundControlPumpMsg::Flush(ack_tx)).is_err() {
            return;
        }
        let _ = tokio::time::timeout(FOREGROUND_CONTROL_FLUSH_TIMEOUT, ack_rx).await;
    }
}
impl SubagentExecutor {

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

    /// The effective control-notice delivery sink, resolved per delivery with the same precedence
    /// as [`Self::effective_completion_sink`]: an explicit [`Self::with_control_notice_sink`]
    /// override → the live [`crate::tui::notices::HostServicesControlNoticeSink`] (pi's
    /// `pi.sendMessage`) when the P-1 host-services slot is bound → the stderr
    /// [`crate::tui::notices::LoggingControlNoticeSink`].
    fn effective_control_notice_sink(&self) -> Arc<dyn crate::tui::notices::ControlNoticeSink> {
        if let Some(sink) = &self.control_notice_sink_override {
            return Arc::clone(sink);
        }
        if let Some(services) = self.host_services() {
            return Arc::new(crate::tui::notices::HostServicesControlNoticeSink::new(services));
        }
        Arc::new(crate::tui::notices::LoggingControlNoticeSink)
    }

    /// pi `createForegroundControlNotifier` (`subagent-executor.ts:1582-1611`) +
    /// `emitControlNotification` (`:505-535`) + the `SUBAGENT_CONTROL_EVENT` listener it feeds
    /// (`extension/index.ts:637-661` → `handleSubagentControlNotice`), collapsed into the one
    /// callback [`crate::exec::RunOptions::on_control_event`] takes, plus the ORDERED hand-off that
    /// callback needs in a multi-threaded runtime (see "Ordering", below). All line numbers are
    /// `@v0.34.0`.
    ///
    /// Per raised event, in the source's own order:
    ///
    /// 1. Refresh the live `foregroundControls` entry's `currentActivityState` — pi assigns it from
    ///    the child's progress fold (`subagent-executor.ts:2972`), and it is what the debounced
    ///    notice's actionability re-check (`isForegroundNoticeStillActionable`,
    ///    `control-notices.ts:59-65` @v0.34.0) reads a second later.
    /// 2. Resolve `childIntercomTarget` exactly as `emitControlNotification` does
    ///    (`:512-513`): `intercomBridge.active ? resolveSubagentIntercomTarget(event.runId,
    ///    event.agent, event.index) : undefined`. It is not decoration — it renders the
    ///    "Direct intercom target: …" line of the notice body AND is the leading component of the
    ///    dedup key (`controlNotificationKey`, `shared/subagent-control.ts:142-145`).
    /// 3. `shouldNotifyControlEvent` (already applied one layer down —
    ///    [`crate::exec::control::ControlMonitor::emit_control_event`] gates on it before this sink
    ///    is ever called) and then the `notifyChannels.includes("event")` CHANNEL gate (`:521`). A
    ///    config whose channels exclude `event` still raises the event onto
    ///    [`crate::exec::SingleResult::control_events`], it just delivers no transcript notice.
    /// 4. `handleSubagentControlNotice`'s own first line: `active_long_running` is NEVER surfaced
    ///    as a transcript notice (`control-notices.ts:50`) — it is informational telemetry only.
    /// 5. Hand the notice to [`crate::tui::notices::ControlNoticeState`], which owns the debounce,
    ///    the at-fire-time actionability re-check and the at-most-once dedup.
    ///
    /// # Ordering (SUBA-N05) — why this returns a pump, not just a closure
    ///
    /// Upstream's whole pipeline is synchronous on one event-loop thread: `onControlEvent` →
    /// `emitControlNotification` → `pi.events.emit` → the listener → `handleSubagentControlNotice`
    /// all run to completion, in raise order, before the child's stdout reader resumes. cyrup's
    /// notice state lives behind a `tokio::sync::Mutex`, so the hand-off must be async while this
    /// sink is sync.
    ///
    /// The previous revision did that with a bare `tokio::spawn` PER EVENT, which silently gave up
    /// two properties upstream has by construction:
    ///
    /// - **Order.** N spawned tasks are scheduled, not sequenced. Two events raised microseconds
    ///   apart could apply their `observe_run` projections in either order, leaving the
    ///   actionability oracle holding the OLDER of the two views.
    /// - **A happens-before against run teardown.** `run_foreground_impl` calls `forget_run` once
    ///   the run settles. Nothing ordered a late event's spawned task against it, so a task that
    ///   had not been polled yet would `observe_run` AFTER `forget_run` — resurrecting a finished
    ///   run in `live_runs` permanently (nothing ever removes it again) and making its pending
    ///   notice pass the "is this run still tracked" check it is supposed to fail.
    ///
    /// Both are fixed by funnelling every event through ONE unbounded mpsc into ONE pump task:
    /// `UnboundedSender::send` is non-blocking and callable from this sync closure, and the channel
    /// is FIFO, so the pump applies events in exactly raise order. Teardown then sends a
    /// [`ForegroundControlNotifier::flush`] marker through the SAME channel and awaits its ack,
    /// which — again by FIFO — cannot arrive until every previously-raised event has been fully
    /// applied. The ack wait is bounded ([`FOREGROUND_CONTROL_FLUSH_TIMEOUT`]) so a pump that has
    /// already exited, or a runtime being torn down, degrades to "proceed" rather than to a hang.
    ///
    /// # The `intercom` channel leg
    ///
    /// `emitControlNotification`'s second leg (`:524-530`) emits `SUBAGENT_CONTROL_INTERCOM_EVENT`
    /// onto pi's in-process event bus. At the ported baseline that bus event has NO delivering
    /// subscriber anywhere in `pi-subagents` — the only consumers are `runs/background/wait.ts`,
    /// which lists it purely as a WAKE channel (`runs/background/wait.ts:136-141` @v0.34.0), and
    /// `async-job-tracker.ts:160-166`, which re-emits it for the async source. Nothing in upstream
    /// actually sends that message to the broker; an out-of-tree extension subscribing to the bus
    /// would. cyrup has no such bus, and its `wait` is documented poll-only
    /// (`background/wait.rs:32`), so there is no cyrup-side subscriber for this leg to feed either.
    /// The rendered body is ported and tested
    /// ([`crate::exec::control::format_control_intercom_message`]); the gate that decides whether
    /// it would be sent is honoured below, so re-adding a delivery target is a call-site change.
    pub(crate) fn foreground_control_notifier(
        &self,
        run_id: RunId,
        agent: String,
        config: crate::exec::control::ResolvedControlConfig,
    ) -> ForegroundControlNotifier {
        let foreground_controls = Arc::clone(&self.foreground_controls);
        let notices = Arc::clone(&self.notices);
        let notice_sink = self.effective_control_notice_sink();
        // pi's `intercomBridge.active && intercomBridge.orchestratorTarget` predicate; this crate's
        // equivalent live-bridge signal is the same one `RunOptions::orchestrator_intercom_target`
        // is built from, so a headless/no-intercom session resolves no child target at all.
        let bridge_active = self.orchestrator_intercom_target().is_some();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ForegroundControlPumpMsg>();

        let pump_run_id = run_id.clone();
        let pump_agent = agent.clone();
        drop(tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let event = match msg {
                    ForegroundControlPumpMsg::Flush(ack) => {
                        // FIFO: every event queued before this marker has already been applied.
                        let _ = ack.send(());
                        continue;
                    }
                    ForegroundControlPumpMsg::Event(event) => *event,
                };
                // (2) pi `emitControlNotification`'s `childIntercomTarget` (`:512-513`).
                let child_intercom_target = bridge_active.then(|| {
                    crate::spawn::intercom_target::resolve_subagent_intercom_target_opt(
                        &event.run_id,
                        &event.agent,
                        event.index.map(|i| i as usize),
                    )
                });
                let notice = crate::tui::ControlNotice {
                    key: crate::tui::ControlNoticeKey {
                        run_id: pump_run_id.clone(),
                        kind: crate::tui::ControlNoticeKind::NeedsAttention,
                        notification_key: crate::exec::control::control_notification_key(
                            &event,
                            child_intercom_target.as_deref(),
                        ),
                    },
                    source: crate::tui::RunSource::Foreground,
                    agent: Some(pump_agent.clone()),
                    step_index: event.index,
                    reason: crate::exec::control::control_event_reason_wire(
                        event.reason.unwrap_or(crate::exec::control::ControlEventReason::Idle),
                    )
                    .to_string(),
                    // pi's `noticeText` (`subagent-executor.ts:519`) — the full rendered body,
                    // built ONCE at raise time and carried on the payload, not re-derived at
                    // delivery.
                    message: crate::exec::control::format_control_notice_message(
                        &event,
                        child_intercom_target.as_deref(),
                    ),
                };
                let live = crate::tui::notices::LiveRunView {
                    current_agent: Some(pump_agent.clone()),
                    current_step_index: event.index,
                    needs_attention: event.to == crate::background::ActivityState::NeedsAttention,
                };
                notices.lock().await.observe_run(pump_run_id.clone(), live);
                crate::tui::notices::ControlNoticeState::handle(
                    &notices,
                    notice,
                    Arc::clone(&notice_sink),
                )
                .await;
            }
        }));

        let sink_tx = tx.clone();
        let sink = crate::exec::control::ControlEventSink::new(move |event| {
            // (1) refresh the live control entry's activity state. Done SYNCHRONOUSLY, on the drive
            // loop's own thread, so the nested-control inbox listener and the notice pump both see
            // transitions in raise order (this is the map `resolve_nested_control_request` reads).
            {
                let mut controls = foreground_controls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(entry) = controls.get_mut(run_id.as_str()) {
                    entry.current_activity_state = Some(event.to);
                    // The SAME write also refreshes the run's live telemetry: pi's control record
                    // carries `currentTool`/`currentPath`/`turnCount`/`toolCount`/`tokens`
                    // (`shared/types.ts:1510-1523`) and the fleet roster renders all five
                    // (`fleet.ts:252-255,399-404`). Every one of them already rides on the control
                    // event; leaving them unread is what made the live rows show a name and nothing
                    // else. `None` on an event never clobbers a value a previous event supplied —
                    // a control event reports what it observed, not a full snapshot.
                    if event.current_tool.is_some() {
                        entry.current_tool = event.current_tool.clone();
                    }
                    if event.current_path.is_some() {
                        entry.current_path = event.current_path.clone();
                    }
                    if let Some(turns) = event.turns {
                        entry.turn_count = Some(turns);
                    }
                    if let Some(tools) = event.tool_count {
                        entry.tool_count = Some(u64::from(tools));
                    }
                    if let Some(tokens) = event.tokens {
                        entry.tokens = Some(tokens);
                    }
                    // pi `control.updatedAt = Date.now()` alongside the activity write
                    // (`subagent-executor.ts:549-570` @v0.43.0) — the FleetView's newest-first sort
                    // key (`fleet.ts:142`).
                    entry.updated_at = crate::background::now_epoch_millis_pub();
                }
            }
            // (3) the `notifyChannels.includes("event")` CHANNEL gate (`:521`). `shouldNotifyControlEvent`
            // already ran one layer down. Note the ordering matters for parity: pi computes
            // `childIntercomTarget`/`noticeText` BEFORE this gate, but both are pure, so evaluating
            // the gate first is observationally identical and avoids the render on the dropped path.
            if !config
                .notify_channels
                .contains(&crate::registration::ControlNotificationChannel::Event)
            {
                return;
            }
            // (4) pi `handleSubagentControlNotice`'s `active_long_running` early return
            // (`control-notices.ts:50`).
            if event.event_type == crate::registration::ControlEventType::ActiveLongRunning {
                return;
            }
            // (5) ordered hand-off. A closed channel (pump already gone) drops the notice, which is
            // the same outcome pi's own fire-and-forget `setTimeout` has once its state is torn down.
            let _ = sink_tx.send(ForegroundControlPumpMsg::Event(Box::new(event.clone())));
        });

        ForegroundControlNotifier { sink, tx }
    }

    /// Construct an executor whose control notices are delivered to `sink` instead of the default
    /// host-services/logging pair — the seam a test uses to capture what the notice pipeline
    /// actually delivered. Mirrors [`Self::with_completion_sink`]'s precedence exactly.
    #[must_use]
    pub fn with_control_notice_sink(
        sink: Arc<dyn crate::tui::notices::ControlNoticeSink>,
    ) -> Self {
        Self { control_notice_sink_override: Some(sink), ..Self::new() }
    }

    /// Override the control-notice debounce window (production: 1000ms, pi's
    /// `foregroundDelayMs ?? 1000`). Test-facing, so a notice-delivery assertion need not sleep out
    /// the full production window; `SubagentExecutor` otherwise always uses the production value.
    pub async fn set_control_notice_debounce(&self, debounce: std::time::Duration) {
        let mut guard = self.notice_state().lock().await;
        *guard = crate::tui::notices::ControlNoticeState::with_debounce(debounce);
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
        match crate::background::watch::install_completion_watcher_with_observer(
            results_dir,
            self.effective_completion_sink(),
            // SUBA-034: pi's async-complete EVENT has several independent listeners
            // (`extension/index.ts:648-659` @v0.43.0 registers three; `wait-subscriptions.ts` adds
            // the wait wake-up). cyrup's one-observer seam could only model the mission sync, so
            // both now hang off a `CompositeCompletionObserver` in the same registration order.
            Some(Arc::new(crate::background::watch::CompositeCompletionObserver::new(vec![
                // pi `asyncCompleteHandler`'s third subscriber (`extension/index.ts:655`):
                // `syncMissionFromAsyncCompletion(payload)`. A background run that carries a
                // `mission.json` binding gets its mission reconciled the moment its result file is
                // observed — including in a LATER process than the one that launched it, which is
                // the whole reason the binding file exists.
                Arc::new(MissionSyncCompletionObserver {
                    async_root: default_async_root(cwd),
                }),
                // SUBA-034: the wake-up every in-flight `wait` is selecting on.
                Arc::new(self.completion_bus.clone()),
            ]))),
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

    /// pi's `agent_end` goal-mission handler (`extension/index.ts:585-601` @v0.43.0): bump the
    /// turn counter, collect every GOAL mission owned by this session that is idle, open and
    /// un-exhausted, and surface each one's continuation notice through the SAME control-notice
    /// pipeline a live run's `needs_attention` uses.
    ///
    /// Best-effort by construction, exactly like upstream's `try`/`catch` around the whole block
    /// (`:597-600` logs and moves on): a failure here must never break the turn that just ended.
    /// Returns the number of notices delivered, which is what the tests assert on.
    ///
    /// The notice's `source` is [`crate::tui::RunSource::Goal`] — delivered immediately (there is
    /// no live run for a debounce to re-validate against) but WITHOUT triggering a turn.
    pub async fn raise_goal_continuation_notices(&self, cwd: &Path) -> usize {
        let Some(owner_session_id) =
            self.host_services().and_then(|services| services.session_id())
        else {
            // pi `:587-588`: no session id, no goal scan.
            return 0;
        };
        let turn_id = self.goal_turn_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let config = self.config_snapshot().await.missions.clone();
        let location =
            crate::missions::resolve_mission_store_location(cwd, config.as_ref(), None);
        // [CYRUP-DELTA] pi passes `listRetainedChildren(DIRS.async, ownerSessionId)`. That list is
        // built from async runs carrying a `parentWorkflowRunId` — a `workflowScript` concept this
        // crate has no runtime for — so it is necessarily empty here and is passed as such rather
        // than faked. See `missions::goal_driver`'s own note.
        let notices = match crate::missions::collect_goal_continuation_notices(
            &location,
            &owner_session_id,
            &[],
            turn_id,
            None,
        ) {
            Ok(notices) => notices,
            Err(e) => {
                tracing::warn!("Failed to evaluate goal missions: {e}");
                return 0;
            }
        };
        let sink = self.effective_control_notice_sink();
        let delivered = notices.len();
        for notice in notices {
            crate::tui::notices::ControlNoticeState::handle(
                &self.notices,
                crate::tui::ControlNotice {
                    key: crate::tui::ControlNoticeKey {
                        run_id: RunId::from_token(notice.event.run_id.as_str()),
                        kind: crate::tui::ControlNoticeKind::NeedsAttention,
                        notification_key: crate::exec::control::control_notification_key(
                            &notice.event,
                            None,
                        ),
                    },
                    source: crate::tui::RunSource::Goal,
                    agent: Some(notice.event.agent.clone()),
                    step_index: None,
                    reason: crate::exec::control::control_event_reason_wire(
                        notice.event.reason.unwrap_or(crate::exec::control::ControlEventReason::Idle),
                    )
                    .to_string(),
                    message: notice.message,
                },
                Arc::clone(&sink),
            )
            .await;
        }
        delivered
    }

    /// Tear down this session's completion watcher (pi `session_shutdown`'s `stopResultWatcher()`,
    /// `extension/index.ts:656`): drop the held [`crate::background::watch::CompletionWatcherHandle`],
    /// whose `Drop` impl aborts the drain task and releases the filesystem watch. A no-op if no
    /// watcher was ever installed (headless / a degraded install, `install_completion_watcher`'s own
    /// best-effort failure path).
    pub async fn stop_completion_watcher(&self) {
        *self.completion_watcher.lock().await = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::testsupport::FixedSessionIdHost;
    use crate::extension::testsupport::arm_scoped_missions;
    use crate::extension::testsupport::scoped_missions;

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
                    current_activity_state: None,
                    mode: crate::background::RunMode::Single,
                    description: None,
                    current_tool: None,
                    current_path: None,
                    turn_count: None,
                    tool_count: None,
                    tokens: None,
                    started_at: crate::background::now_epoch_millis_pub(),
                    updated_at: crate::background::now_epoch_millis_pub(),
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
                    current_activity_state: None,
                    mode: crate::background::RunMode::Single,
                    description: None,
                    current_tool: None,
                    current_path: None,
                    turn_count: None,
                    tool_count: None,
                    tokens: None,
                    started_at: crate::background::now_epoch_millis_pub(),
                    updated_at: crate::background::now_epoch_millis_pub(),
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

    // ---------------------------------------------------------------------------------------
    // SUBA-N05: the foreground control-notice pump's ordering + teardown contract.
    // ---------------------------------------------------------------------------------------

    /// Build the `ControlEvent` a `needs_attention` raise produces, with an explicit reason so
    /// distinct reasons yield distinct `controlNotificationKey`s (pi
    /// `shared/subagent-control.ts:142-145` @v0.43.0).
    fn pump_test_event(
        run_id: &RunId,
        index: u32,
        reason: crate::exec::control::ControlEventReason,
    ) -> crate::exec::control::ControlEvent {
        crate::exec::control::build_control_event(
            crate::background::ActivityState::NeedsAttention,
            crate::exec::control::ControlEventInput {
                event_type: Some(crate::registration::ControlEventType::NeedsAttention),
                ts: 1_700_000_000_000,
                run_id: run_id.as_str().to_string(),
                agent: "scout".to_string(),
                index: Some(index),
                reason: Some(reason),
                ..Default::default()
            },
        )
    }

    /// SUBA-N05 — the ORDERING + TEARDOWN contract [`ForegroundControlNotifier`] exists to provide.
    ///
    /// Upstream gets both for free: `onControlEvent` → `emitControlNotification` → the
    /// `SUBAGENT_CONTROL_EVENT` listener → `handleSubagentControlNotice` is one synchronous
    /// call chain on one event-loop thread, so by the time the child's stdout reader resumes, the
    /// event has been fully applied. cyrup's notice state is behind a `tokio::sync::Mutex` and the
    /// sink is sync, so the hand-off has to cross an async boundary — and the previous revision
    /// crossed it with a bare `tokio::spawn` PER EVENT, which is neither ordered nor ordered
    /// against run teardown.
    ///
    /// This test pins the replacement's guarantee directly, with no sleeping and no scheduler luck:
    ///
    /// 1. The notice lock is held while three events are raised, so the hand-off provably CANNOT
    ///    have been applied yet — the raise path is non-blocking, exactly as the drive loop needs.
    /// 2. `flush()` returns only once all three have been applied. Asserted IMMEDIATELY on return,
    ///    with no `sleep` anywhere: all three debounce timers are armed and the live projection
    ///    already reflects the LAST event (`step_index: 2`), which is the FIFO property.
    /// 3. `forget_run` then leaves the run untracked AND aborts every armed timer — pi's own
    ///    teardown pair (`clearPendingForegroundControlNotices(deps.state, runId)` immediately
    ///    followed by `foregroundControls.delete(runId)`, `subagent-executor.ts:3579-3581`) — and a
    ///    generous settle window afterwards cannot resurrect it.
    ///
    /// Step 2's assertion is what a spawn-per-event hand-off cannot satisfy: it has no barrier to
    /// wait on at all, so the checks would run against zero-to-three applied events depending on
    /// the scheduler. Step 3 is the production bug that shape caused — an event still unpolled at
    /// teardown re-inserted its run into `live_runs`, where nothing ever removes it again, and its
    /// pending notice then passed the "is this run still tracked" check it is supposed to fail.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn control_events_are_applied_in_order_and_never_after_the_run_is_torn_down() {
        let delivered: Arc<std::sync::Mutex<Vec<crate::tui::ControlNotice>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = Arc::clone(&delivered);
        let executor = SubagentExecutor::with_control_notice_sink(Arc::new(
            move |notice: crate::tui::ControlNotice, _trigger: bool| {
                captured.lock().unwrap_or_else(|e| e.into_inner()).push(notice);
            },
        ));
        // Long enough that no timer can fire during this test: every assertion below is about the
        // PUMP, not about delivery.
        executor.set_control_notice_debounce(std::time::Duration::from_secs(600)).await;

        let run_id = RunId::new();
        let notifier = executor.foreground_control_notifier(
            run_id.clone(),
            "scout".to_string(),
            crate::exec::control::ResolvedControlConfig::default(),
        );
        executor.notice_state().lock().await.observe_run(
            run_id.clone(),
            crate::tui::notices::LiveRunView {
                current_agent: Some("scout".to_string()),
                current_step_index: Some(0),
                needs_attention: false,
            },
        );

        // Three distinct reasons => three distinct `controlNotificationKey`s => three independent
        // debounce timers, so "all three were applied" is observable rather than coalesced away.
        let events = [
            pump_test_event(&run_id, 0, crate::exec::control::ControlEventReason::Idle),
            pump_test_event(
                &run_id,
                1,
                crate::exec::control::ControlEventReason::ToolFailures,
            ),
            pump_test_event(
                &run_id,
                2,
                crate::exec::control::ControlEventReason::SupervisorRequest,
            ),
        ];

        // (1) Raise all three while the notice lock is held: the hand-off must not block the drive
        // loop, and must not have been applied by the time the last `emit` returns.
        {
            let guard = executor.notice_state().lock().await;
            for event in &events {
                notifier.sink().emit(event);
            }
            assert_eq!(
                guard.live_view(&run_id).and_then(|v| v.current_step_index),
                Some(0),
                "raising an event must not have applied it yet — the lock is still held here"
            );
        }

        // (2) The barrier. No sleep: everything below is asserted on flush's own guarantee.
        notifier.flush().await;
        {
            let guard = executor.notice_state().lock().await;
            assert_eq!(
                guard.live_view(&run_id).and_then(|v| v.current_step_index),
                Some(2),
                "the live projection must reflect the LAST raised event — FIFO, not whichever \
                 hand-off happened to be scheduled last"
            );
            for event in &events {
                let key = crate::tui::ControlNoticeKey {
                    run_id: run_id.clone(),
                    kind: crate::tui::ControlNoticeKind::NeedsAttention,
                    notification_key: crate::exec::control::control_notification_key(event, None),
                };
                assert!(
                    guard.has_pending(&key),
                    "every raised event must have armed its own debounce timer by the time \
                     flush() returns; missing {}",
                    key.notification_key
                );
            }
        }

        // (3) pi's teardown pair, then a settle window a straggler could have used.
        executor.notice_state().lock().await.forget_run(&run_id);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        {
            let guard = executor.notice_state().lock().await;
            assert!(
                guard.live_view(&run_id).is_none(),
                "a finished run must stay untracked — nothing may re-register it after teardown"
            );
            for event in &events {
                let key = crate::tui::ControlNoticeKey {
                    run_id: run_id.clone(),
                    kind: crate::tui::ControlNoticeKind::NeedsAttention,
                    notification_key: crate::exec::control::control_notification_key(event, None),
                };
                assert!(
                    !guard.has_pending(&key),
                    "forget_run must abort this run's armed timers (pi \
                     clearPendingForegroundControlNotices)"
                );
            }
        }
        assert!(
            delivered.lock().unwrap_or_else(|e| e.into_inner()).is_empty(),
            "with a 600s debounce nothing may have been delivered"
        );
    }

    /// SUBA-N05 — the `notifyChannels` gate, which had ZERO runtime consumers before this change.
    ///
    /// pi routes a raised event onto the transcript channel only when
    /// `controlConfig.notifyChannels.includes("event")` (`subagent-executor.ts:817` @v0.43.0). A
    /// config of `notifyChannels: ["intercom"]` must therefore raise the event (it still lands on
    /// `SingleResult::control_events`) and deliver NO transcript notice. Before this change the
    /// notifier had no channel check at all, so such a config still produced a notice.
    ///
    /// Also pins the two gates that were already correct, so a refactor cannot quietly drop them:
    /// `active_long_running` is never surfaced as a transcript notice
    /// (`control-notices.ts:50`), and an `["event"]` config does surface `needs_attention`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn notify_channels_gates_the_transcript_notice_without_suppressing_the_event() {
        use crate::registration::ControlNotificationChannel;

        for (channels, expect_pending) in [
            (vec![ControlNotificationChannel::Event], true),
            (vec![ControlNotificationChannel::Intercom], false),
            (vec![ControlNotificationChannel::Async], false),
            (Vec::new(), false),
        ] {
            let executor = SubagentExecutor::new();
            executor.set_control_notice_debounce(std::time::Duration::from_secs(600)).await;
            let run_id = RunId::new();
            let notifier = executor.foreground_control_notifier(
                run_id.clone(),
                "scout".to_string(),
                crate::exec::control::ResolvedControlConfig {
                    notify_channels: channels.clone(),
                    ..crate::exec::control::ResolvedControlConfig::default()
                },
            );
            let event = pump_test_event(&run_id, 0, crate::exec::control::ControlEventReason::Idle);
            notifier.sink().emit(&event);
            notifier.flush().await;

            let key = crate::tui::ControlNoticeKey {
                run_id: run_id.clone(),
                kind: crate::tui::ControlNoticeKind::NeedsAttention,
                notification_key: crate::exec::control::control_notification_key(&event, None),
            };
            assert_eq!(
                executor.notice_state().lock().await.has_pending(&key),
                expect_pending,
                "notifyChannels {channels:?} must {} a transcript notice",
                if expect_pending { "arm" } else { "suppress" }
            );
        }

        // `active_long_running` is telemetry only — never a transcript notice, on any channel set.
        let executor = SubagentExecutor::new();
        executor.set_control_notice_debounce(std::time::Duration::from_secs(600)).await;
        let run_id = RunId::new();
        let notifier = executor.foreground_control_notifier(
            run_id.clone(),
            "scout".to_string(),
            crate::exec::control::ResolvedControlConfig::default(),
        );
        let long_running = crate::exec::control::build_control_event(
            crate::background::ActivityState::ActiveLongRunning,
            crate::exec::control::ControlEventInput {
                event_type: Some(crate::registration::ControlEventType::ActiveLongRunning),
                ts: 1_700_000_000_000,
                run_id: run_id.as_str().to_string(),
                agent: "scout".to_string(),
                index: Some(0),
                reason: Some(crate::exec::control::ControlEventReason::ActiveLongRunning),
                ..Default::default()
            },
        );
        notifier.sink().emit(&long_running);
        notifier.flush().await;
        let key = crate::tui::ControlNoticeKey {
            run_id: run_id.clone(),
            kind: crate::tui::ControlNoticeKind::NeedsAttention,
            notification_key: crate::exec::control::control_notification_key(&long_running, None),
        };
        assert!(
            !executor.notice_state().lock().await.has_pending(&key),
            "handleSubagentControlNotice drops active_long_running before any debounce is armed"
        );
    }

    /// The turn-end GOAL scan (pi `extension/index.ts:585-601`), driven through the REAL
    /// `HostEvent::AgentEnd` hook: an idle goal mission owned by this session raises exactly one
    /// control notice per turn, delivered immediately and WITHOUT triggering a turn.
    #[tokio::test]
    async fn agent_end_raises_a_goal_continuation_notice_through_the_control_notice_pipeline() {
        use crate::tui::notices::ControlNoticeSink;

        #[derive(Default)]
        struct Recording {
            delivered: std::sync::Mutex<Vec<(crate::tui::ControlNotice, bool)>>,
        }
        impl ControlNoticeSink for Recording {
            fn emit_control_notice(&self, notice: crate::tui::ControlNotice, trigger_turn: bool) {
                self.delivered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((notice, trigger_turn));
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let sink = Arc::new(Recording::default());
        let executor = Arc::new(SubagentExecutor::with_control_notice_sink(sink.clone()));
        arm_scoped_missions(&executor, dir.path()).await;
        let services: Arc<dyn cyrup_ext::host::HostServices> =
            Arc::new(FixedSessionIdHost { id: Some("goal-session".to_string()), file: None });
        executor.set_host_services(services);

        let location = crate::missions::resolve_mission_store_location(
            dir.path(),
            Some(&scoped_missions(dir.path())),
            None,
        );
        let record = crate::missions::create_mission(
            &location,
            &crate::missions::MissionCreateInput {
                title: "Keep going".to_string(),
                objective: "finish the long thing".to_string(),
                goal: Some(true),
                budget: Some(crate::missions::MissionTokenBudget { tokens: 5_000 }),
                status: Some(crate::missions::MissionStatus::Active),
                labels: None,
                owner_session_id: Some("goal-session".to_string()),
            },
            0,
            None,
        )
        .expect("create goal mission");

        assert_eq!(executor.raise_goal_continuation_notices(dir.path()).await, 1);
        let delivered =
            sink.delivered.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        assert_eq!(delivered.len(), 1, "exactly one notice per idle goal mission per turn");
        let (notice, trigger_turn) = &delivered[0];
        assert!(!trigger_turn, "a goal notice must NOT trigger a turn (pi source === \"async\")");
        assert_eq!(notice.source, crate::tui::RunSource::Goal);
        assert_eq!(notice.agent.as_deref(), Some("goal mission"));
        assert_eq!(notice.key.run_id.as_str(), format!("goal-{}-turn-1", record.id));
        assert!(
            notice.message.contains("Goal mission needs attention: Keep going"),
            "{}",
            notice.message
        );
        assert!(
            notice.message.contains("Remaining budget: 5000 tokens (0/5000 used)"),
            "{}",
            notice.message
        );
        assert!(
            notice.message.contains("Next ready action: Continue objective: finish the long thing"),
            "{}",
            notice.message
        );

        // A SECOND turn raises a second, non-deduplicated notice (the run id carries the turn).
        assert_eq!(executor.raise_goal_continuation_notices(dir.path()).await, 1);
        let delivered =
            sink.delivered.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        assert_eq!(delivered.len(), 2, "the per-turn run id must defeat the at-most-once dedup");
        assert_eq!(delivered[1].0.key.run_id.as_str(), format!("goal-{}-turn-2", record.id));
    }

    /// G77 — a live FOREGROUND run gets its own refusal pointing at `interrupt`
    /// (`subagent-executor.ts:4797`), not the async not-found text.
    #[tokio::test]
    async fn stopping_a_live_foreground_run_points_at_interrupt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = Arc::new(SubagentExecutor::new());
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls.insert(
                "fgstop0001".to_string(),
                ForegroundControlEntry {
                    interrupt: CancelToken::new(),
                    current_agent: Some("scout".to_string()),
                    current_index: Some(0),
                    current_activity_state: None,
                    mode: crate::background::RunMode::Single,
                    description: None,
                    current_tool: None,
                    current_path: None,
                    turn_count: None,
                    tool_count: None,
                    tokens: None,
                    started_at: crate::background::now_epoch_millis_pub(),
                    updated_at: crate::background::now_epoch_millis_pub(),
                },
            );
        }
        let err = executor
            .control_stop(dir.path(), Some("fgstop0001"), None)
            .await
            .expect_err("a foreground run is not stoppable");
        assert_eq!(
            err,
            "action='stop' supports async runs only. Use action='interrupt' for foreground runs."
        );
    }

}
