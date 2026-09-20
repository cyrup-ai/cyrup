//! SUBA-034 — the completion fan-out: the [`CompletionObserver`] side-channel every scanned
//! completion is announced on before delivery, an ordered composite over several observers, and
//! the in-process broadcast bus `background::wait` subscribes to instead of re-deriving the same
//! fact on its own poll. Split out of `background/watch.rs`; ports pi
//! `extension/index.ts:648-659`'s `SUBAGENT_ASYNC_COMPLETE_EVENT` fan-out.

use super::classify::{ClassifiedOutcome, classify_outcome};
use super::results_watcher::CompletionNotification;
use crate::background::RunId;
use std::sync::Arc;

/// A side-observer of every scanned completion, invoked BEFORE the notification is delivered and
/// regardless of whether delivery succeeds.
///
/// This is the seam pi's event bus provides for free: upstream's `SUBAGENT_ASYNC_COMPLETE_EVENT`
/// has THREE independent subscribers (`extension/index.ts:648-659`) — `handleComplete` (the
/// notification), `scheduledRunManager.handleAsyncCompletion`, and
/// `syncMissionFromAsyncCompletion` — and a `CompletionSink` alone can only model the first.
/// [`crate::missions::sync_mission_from_async_completion`] is the third one, and it must run
/// whether or not the notification lands (a mission reconciliation is not conditional on a
/// message reaching the transcript).
#[async_trait::async_trait]
pub trait CompletionObserver: Send + Sync {
    /// Observe one scanned, not-yet-delivered completion. Must not fail the pipeline: any error
    /// belongs inside the implementation.
    ///
    /// Returns whether the observation SUCCEEDED. pi tracks this per observer
    /// (`result-watcher.ts:414-424`, `observerSucceeded`) for one specific purpose: the
    /// cross-session mission observer index is retired only when every observer ran cleanly
    /// (`:425`). An observer that failed may need to see this completion again, and the index
    /// entry is what will resurface it.
    ///
    /// Returning `false` never aborts the pipeline or blocks delivery — it only preserves the
    /// observer index for a retry.
    async fn observe(&self, notification: &CompletionNotification) -> bool;
}

/// SUBA-034 — a fan-out [`CompletionObserver`], so ONE watcher can feed several independent
/// subscribers exactly as pi's `SUBAGENT_ASYNC_COMPLETE_EVENT` does (`extension/index.ts:648-659`
/// @v0.43.0 registers three listeners on the one event; `wait-subscriptions.ts` adds a fourth).
///
/// All four are now real: SCOPE_11 landed the fourth as
/// `extension/executor/wait_subscriptions.rs`'s `WaitSubscriptionCompletionObserver`, registered
/// LAST in `extension/executor/notices.rs`'s composite so the
/// [`crate::background::wait_completions::WaitCompletionStore`] record member #1 writes is already
/// present when a fired subscription reads its completion back.
///
/// Before this existed the install seam took a single `Option<Arc<dyn CompletionObserver>>`, which
/// could model pi's mission-sync listener and nothing else — so the `wait` wake-up had nowhere to
/// attach. Each member is awaited in registration order and none may fail the pipeline, matching
/// the trait's own contract and pi's `for (const handler of handlers) await handler(...)`.
pub struct CompositeCompletionObserver {
    members: Vec<Arc<dyn CompletionObserver>>,
}

impl CompositeCompletionObserver {
    /// Fan out to `members`, in order.
    #[must_use]
    pub fn new(members: Vec<Arc<dyn CompletionObserver>>) -> Self {
        Self { members }
    }
}

#[async_trait::async_trait]
impl CompletionObserver for CompositeCompletionObserver {
    /// Fan out to every member and AND their outcomes.
    ///
    /// Deliberately not short-circuiting: `&&` in Rust would skip the remaining members after the
    /// first failure, but pi runs each listener in its own `try`/`catch` and only then combines
    /// (`result-watcher.ts:414-424`). A mission sync that throws must not prevent a `wait` wake-up
    /// from firing.
    async fn observe(&self, notification: &CompletionNotification) -> bool {
        let mut succeeded = true;
        for member in &self.members {
            succeeded &= member.observe(notification).await;
        }
        succeeded
    }
}

/// pi `SUBAGENT_ASYNC_COMPLETE_EVENT` (`src/shared/types.ts:2355` @v0.68.0) — the
/// INTER-EXTENSION topic every observed background completion is republished on by
/// [`BusAnnouncingCompletionObserver`].
///
/// Owned here, by the emitter, rather than by [`crate::extension::rpc`], which merely advertises
/// it: a topic constant belongs to whatever publishes on it, so an advertisement can never name a
/// topic nothing emits.
pub const SUBAGENT_ASYNC_COMPLETE_EVENT: &str = "subagent:async-complete";

/// PB-8 §4.5 — republish every observed background completion on the host-owned inter-extension
/// bus, so a host or sibling extension that delegated work learns it finished instead of polling
/// `status` in a loop.
///
/// This is pi's `pi.events.emit(SUBAGENT_ASYNC_COMPLETE_EVENT, {...})`
/// (`runs/background/result-watcher.ts:589-606` @v0.68.0). Upstream's completion signal IS an
/// event-bus emit, so its four in-process listeners and any cross-extension listener are the same
/// mechanism; cyrup's completion fan-out is the in-process [`CompletionObserver`] composite, and
/// before this member existed NOTHING in this crate reached the inter-extension bus with a
/// completion at all. That is why [`crate::extension::rpc`]'s `ping` reply may advertise
/// `events.asyncComplete`: the advertisement is paid for here.
///
/// # Ordering in the composite
///
/// Registered AFTER [`crate::background::wait_completions::WaitCompletionStore`] (which must stay
/// first — see `extension/executor/notices.rs`'s ordering doc) and before the wait-subscription
/// reconciler (which must stay last). Its own position among the middle members is not
/// load-bearing: it neither reads nor writes any state the others touch.
///
/// # `[CYRUP-DELTA]` — the payload is the result file, and only the result file
///
/// Upstream spreads the result payload and adds `runId`, `triggerTurn`, `intercomDelivered` and a
/// re-normalized `results` array (`result-watcher.ts:590-605`). `runId` is already a field of
/// [`crate::background::ResultFile`] here and `results` is already the per-child vector, so both
/// arrive for free. `triggerTurn` and `intercomDelivered` are decisions made by the DELIVERY half
/// (the sink), one layer past this observer, and are not knowable at the moment of observation —
/// this observer runs BEFORE delivery, by the trait's own contract — so they are omitted rather
/// than guessed.
///
/// Fire-and-forget: [`cyrup_ext::host::HostServices::emit_event`] queues on
/// [`cyrup_ext::bus::SharedBus`] and the host fans out at its next seam boundary. With no backend
/// bound (a by-value session) there is no bus, so nothing is published and the observation still
/// counts as successful — a missing coordination channel must never make a completion look failed
/// and get it retried.
pub struct BusAnnouncingCompletionObserver {
    /// The executor's own late-bound P-1 slot (`extension/executor/mod.rs:120`), shared rather
    /// than copied: `set_host_services` runs before `init`, but the completion watcher is
    /// installed later still, and a REINSTALL on a subsequent `SessionStart` must see whatever the
    /// slot holds then.
    host_services: Arc<std::sync::OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
}

impl BusAnnouncingCompletionObserver {
    /// Announce onto whatever backend `host_services` resolves to at observation time.
    #[must_use]
    pub fn new(
        host_services: Arc<std::sync::OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
    ) -> Self {
        Self { host_services }
    }
}

#[async_trait::async_trait]
impl CompletionObserver for BusAnnouncingCompletionObserver {
    async fn observe(&self, notification: &CompletionNotification) -> bool {
        let Some(services) = self.host_services.get() else {
            return true;
        };
        match serde_json::to_value(&notification.result) {
            Ok(payload) => services.emit_event(SUBAGENT_ASYNC_COMPLETE_EVENT, &payload),
            Err(error) => {
                // A result that cannot be serialized is a bug in `ResultFile`, not a delivery
                // failure; say so and keep the pipeline moving (the trait forbids failing it).
                tracing::warn!(
                    target: "cyrup_ext_subagents::rpc",
                    run_id = %notification.result.run_id,
                    %error,
                    "completion could not be encoded for the inter-extension bus"
                );
            }
        }
        true
    }
}

/// pi `SUBAGENT_PROCESS_TERMINAL_EVENT` (`src/shared/types.ts:2356` @v0.68.0) — the
/// INTER-EXTENSION topic a run's process-terminal PROOF is republished on by
/// [`ProcessTerminalAnnouncingCompletionObserver`].
///
/// Owned here, by the emitter, for the reason [`SUBAGENT_ASYNC_COMPLETE_EVENT`]'s own doc gives:
/// a topic constant belongs to whatever publishes on it, so an advertisement
/// ([`crate::extension::rpc`]'s `pingData`, `events.processTerminal`) can never name a topic
/// nothing emits.
///
/// Distinct from [`crate::background::process_terminal::PROCESS_TERMINAL_EVENT_TYPE`], which is
/// the `events.jsonl` LINE type (`subagent.run.process_terminal`, `process-terminal.ts:305`)
/// written by the RUNNER into the run's own log. Upstream keeps the two spellings apart for the
/// same reason: one is a bus topic, the other a record tag.
pub const SUBAGENT_PROCESS_TERMINAL_EVENT: &str = "subagent:process-terminal";

/// How long [`ProcessTerminalAnnouncingCompletionObserver`] keeps re-reading a `pending` sidecar
/// before accepting `pending` as the answer.
///
/// It covers ONE gap and is sized for it: the runner's own `finish_run` -> lease release ->
/// candidate write -> `finalize_process_terminal` tail, which is a handful of small local writes.
/// A budget of zero is the defect this constant exists to fix (a clean run's event dropped
/// forever); an unbounded wait would hold the observers registered after this one behind a runner
/// that hung. Neither direction is reached in practice, because the pid short-circuit below
/// answers the crash case long before the deadline.
const PROCESS_TERMINAL_SETTLE_BUDGET: std::time::Duration = std::time::Duration::from_millis(2_000);

/// The re-read cadence inside [`PROCESS_TERMINAL_SETTLE_BUDGET`] — far finer than the results
/// watcher's own poll, because this is waiting on a write that has already been ordered rather
/// than on a run.
const PROCESS_TERMINAL_SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// pi `emitProcessTerminalEvent` (`runs/background/async-execution.ts:666-672` @v0.68.0) —
/// republish a finished run's process-terminal proof on the inter-extension bus, so a host that
/// delegated work learns HOW the run's process ended and not merely that it ended.
///
/// Upstream's subscriber is `extension/index.ts:897-900`: `refreshActiveAsyncCapacity()` and a
/// fleet refresh. Both are decisions a consumer can only make once it knows the owning process is
/// gone — which is the fact [`crate::background::process_terminal`] exists to establish and the
/// one [`SUBAGENT_ASYNC_COMPLETE_EVENT`] cannot carry, because a terminal
/// [`crate::background::ResultFile`] says the RUN ended, not that its RUNNER did.
///
/// # `[CYRUP-DELTA]` — the emit site moves from the launcher to the completion fan-out
///
/// Upstream emits from the PARENT, in `async-execution.ts:668`, immediately after
/// `finalizeProcessTerminal` returns on the runner's `close` event: its runner is a child it keeps
/// a handle to. cyrup's runner is genuinely detached — `spawn_detached_runner_with_command` puts
/// it in its own process group and drops the handle without ever awaiting it
/// (`crates/cyrup/src/subagent_runner_cmd.rs:1-7`, R-SA-078) — so `finalize_process_terminal` runs
/// inside the RUNNER's own process ([`crate::background::runner_main`]), which has no bus.
///
/// The parent's first in-process edge after that write is the completion watcher observing the
/// run's terminal result file, which is exactly where [`BusAnnouncingCompletionObserver`] already
/// sits. So the proof is read off disk here and published there. The delta is the LATENCY (bounded
/// below by the watcher's poll cadence, the same delta [`CompletionBus`] documents), never the
/// payload: what is published is the sidecar the runner wrote, verbatim.
///
/// # The delta has a RACE in it, and this is where it is paid for
///
/// Moving the emit onto the completion watcher moves it onto a DIFFERENT trigger. The runner's
/// tail is `finish_run` — `status.json`, then the `ResultFile` (R-SA-077's ordering) — and only
/// then the lease release and `finalize_process_terminal`'s sidecar write
/// (`runner_main/entry.rs`). The completion watcher fires on the ResultFile. So a poll tick that
/// lands in the millisecond gap between those two writes reads the launch's `pending` placeholder
/// for a run that is closing perfectly cleanly — and because the watcher dedups a dispatched
/// result for its whole TTL, a naive `pending => publish nothing` would drop that run's event
/// PERMANENTLY. A subscriber refreshing capacity or fleet state off `events.processTerminal`
/// (advertised at `extension/rpc/ping.rs`) would wait forever.
///
/// So a `pending` read is not an answer here; it is "not yet". This observer re-probes for
/// [`PROCESS_TERMINAL_SETTLE_BUDGET`] at [`PROCESS_TERMINAL_SETTLE_INTERVAL`], and stops the
/// moment either half of the question is genuinely answered:
///
/// * a NON-pending sidecar appears — the runner finalized; publish it, verbatim.
/// * the runner's recorded pid is demonstrably gone while the sidecar is still `pending` — the
///   runner died between its result write and its proof write, and no proof is ever coming.
///
/// **That wait runs on a DETACHED task and `observe` returns immediately.** It has to:
/// `install.rs:373-384` calls this in the watcher's phase-1 synchronous band, whose own comment
/// records that starving it was `ASYNC_NOTIFY_BUG_REPORT` RC1 — the `wait` wake-up and the mission
/// sync are behind it. An observer that parked that scan for up to two seconds per completion
/// would trade this defect for that one. The detached task is best effort, exactly like every
/// other publisher on this bus: a process that exits inside the window loses the event, and the
/// capacity rung's pid ladder is what speaks for that run instead — the same fallback the
/// `pending` case already relies on.
///
/// # What is NOT published, and why that is upstream's shape too
///
/// A sidecar that is STILL `pending` when that settles is skipped. `pending` is what
/// [`initialize_process_terminal`](crate::background::process_terminal::initialize_process_terminal)
/// writes at launch and what a runner that was killed leaves behind; upstream has no emit for that
/// case either, because its emit is downstream of a `finalizeProcessTerminal` that never ran. A
/// subscriber must not be told "a proof landed" about a file that is still the launch's
/// placeholder — the absence of the event is itself the crash signal, and the capacity release
/// rung's pid ladder is what speaks for that run instead
/// ([`crate::background::active_async_capacity`]).
pub struct ProcessTerminalAnnouncingCompletionObserver {
    /// This session's async root, so a completed run's sidecar can be located from the run id
    /// alone — the same derivation `MissionSyncCompletionObserver` makes for `mission.json`.
    async_root: std::path::PathBuf,
    /// The executor's own late-bound host-services slot, shared rather than copied, for the
    /// reason [`BusAnnouncingCompletionObserver`]'s field doc gives.
    host_services: Arc<std::sync::OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
}

impl ProcessTerminalAnnouncingCompletionObserver {
    /// Read this run's sidecar, treating `pending` as *not yet* rather than as an answer — see the
    /// type's own "The delta has a RACE in it" section.
    ///
    /// `None` means there is nothing to publish: no readable sidecar at all, or one that was
    /// still `pending` when the question was genuinely settled.
    async fn settled_proof(
        run_dir: &crate::background::RunDir,
        expectation: crate::background::process_terminal::ProofExpectation<'_>,
    ) -> Option<crate::background::process_terminal::ProcessTerminal> {
        use crate::background::process_terminal::{ProcessTerminalState, read_process_terminal};

        let deadline = std::time::Instant::now() + PROCESS_TERMINAL_SETTLE_BUDGET;
        loop {
            let proof = read_process_terminal(run_dir, expectation).await;
            match proof.as_ref().map(|proof| proof.state()) {
                // A real verdict — observed, unknown, or the launch's not-started record.
                Some(state) if state != ProcessTerminalState::Pending => return proof,
                // No sidecar at all is not this race: `initialize_process_terminal` writes the
                // `pending` placeholder BEFORE the spawn, so a run that reached a terminal
                // ResultFile without one was launched by a build that writes neither, and no
                // amount of waiting produces one.
                None => return None,
                Some(_) => {}
            }
            // The crash short-circuit. A runner whose pid is demonstrably gone while its sidecar
            // still reads `pending` died between its result write and its proof write; that
            // `pending` IS the final answer and the rest of the budget would buy nothing.
            if !Self::runner_may_still_be_writing(run_dir).await {
                return None;
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(PROCESS_TERMINAL_SETTLE_INTERVAL).await;
        }
    }

    /// `true` while the run's recorded runner pid could still reach its proof write.
    ///
    /// Deliberately permissive in both unknowable directions, because the cost of each is
    /// asymmetric: a status that cannot be read or carries no pid yields `true` (wait out the
    /// budget and re-read, which at worst delays this one event by
    /// [`PROCESS_TERMINAL_SETTLE_BUDGET`]), and only
    /// [`Liveness::Dead`](crate::background::reconcile::Liveness::Dead) — a real `ESRCH` — ends
    /// the wait. `Unknown` (an `EPERM`-class probe under sandboxing) is NOT death, per R-SA-089.
    async fn runner_may_still_be_writing(run_dir: &crate::background::RunDir) -> bool {
        use crate::background::reconcile::{Liveness, check_pid_liveness};

        let Ok(Some(status)) =
            crate::background::control::read_status_file(&run_dir.status()).await
        else {
            return true;
        };
        status
            .pid
            .is_none_or(|pid| check_pid_liveness(pid) != Liveness::Dead)
    }

    /// Announce proofs found under `async_root` onto whatever backend `host_services` resolves to
    /// at observation time.
    #[must_use]
    pub fn new(
        async_root: std::path::PathBuf,
        host_services: Arc<std::sync::OnceLock<Arc<dyn cyrup_ext::host::HostServices>>>,
    ) -> Self {
        Self {
            async_root,
            host_services,
        }
    }
}

#[async_trait::async_trait]
impl CompletionObserver for ProcessTerminalAnnouncingCompletionObserver {
    async fn observe(&self, notification: &CompletionNotification) -> bool {
        let Some(services) = self.host_services.get().map(Arc::clone) else {
            return true;
        };
        // The run is known; the runner instance is not, because the result file does not carry
        // one — so the read below asks for well-formedness and THIS run, which is the strongest
        // expectation this reader can honestly state. A sidecar belonging to another run degrades
        // to `unknown` at the read rather than being republished as this run's.
        let run_id = &notification.result.run_id;
        // Detached, for the reason on this type's doc: the watcher's phase-1 band must not be
        // parked while a runner finishes writing.
        let run_id = run_id.clone();
        let async_root = self.async_root.clone();
        tokio::spawn(async move {
            let run_dir = crate::background::RunDir::new(&async_root, &run_id);
            let expectation = crate::background::process_terminal::ProofExpectation {
                run_id: Some(&run_id),
                runner_process_instance_id: None,
            };
            let Some(proof) = Self::settled_proof(&run_dir, expectation).await else {
                return;
            };
            match serde_json::to_value(&proof) {
                Ok(payload) => services.emit_event(SUBAGENT_PROCESS_TERMINAL_EVENT, &payload),
                Err(error) => {
                    tracing::warn!(
                        target: "cyrup_ext_subagents::rpc",
                        run_id = %run_id,
                        %error,
                        "process-terminal proof could not be encoded for the inter-extension bus"
                    );
                }
            }
        });
        true
    }
}

/// SUBA-034 — the payload published on [`CompletionBus`] when a background run reaches a terminal
/// state: pi's `SUBAGENT_ASYNC_COMPLETE_EVENT` payload, narrowed to the fields a subscriber can act
/// on without re-reading the run tree.
///
/// Deliberately NOT the whole [`crate::background::ResultFile`]: a `broadcast` channel keeps every queued value alive
/// for every receiver, and the result file carries the full per-step result vector. The one
/// subscriber this exists for ([`crate::background::wait`]) re-reads authoritative state from disk
/// the instant it wakes — the event is a WAKE-UP, never the source of truth, which is pi's own
/// stated contract for the same subscription ("With no bus, `wait` degrades to pure polling").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionEvent {
    /// The run that reached a terminal state.
    pub run_id: RunId,
    /// Its terminal outcome, as classified by [`classify_outcome`] (the SAME classification the
    /// notification text is built from, so a subscriber and the transcript can never disagree).
    pub outcome: ClassifiedOutcome,
    /// SCOPE_17 — the SAME per-child body [`super::message::format_completion_message`] renders, so
    /// a `wait` and the notification can never disagree about what a child said.
    ///
    /// # Why a rendered String and not the `ResultFile`
    ///
    /// This type's existing doc rules out carrying the whole [`crate::background::ResultFile`],
    /// and that reasoning is unchanged: a `broadcast` channel keeps every queued value alive for
    /// every receiver, and the result file holds the full per-step vector. A bounded, already
    /// rendered string is a fixed small cost and is the exact thing both consumers want — the
    /// alternative (re-reading the payload from `wait`) races the unlink this event precedes.
    ///
    /// Bounded to [`COMPLETION_EVENT_SUMMARY_MAX_BYTES`] through
    /// [`crate::exec::output::utf8_safe_prefix`], so `COMPLETION_BUS_CAPACITY` (64) events are a
    /// bounded worst case rather than an unbounded one.
    pub summary: String,
}

/// The bus copy's ceiling. Distinct from `CHILD_OUTPUT_PREVIEW_MAX_BYTES` (`watch/message.rs`),
/// which bounds ONE child's structured fallback: this bounds the whole multi-child render that
/// rides the broadcast buffer. The notification itself is NOT bounded by this — it is delivered
/// once, not buffered 64 times — so a long body still reaches the orchestrator in full through the
/// notify.
const COMPLETION_EVENT_SUMMARY_MAX_BYTES: usize = 16 * 1024;

/// [`super::message::result_display_summary`] bounded to
/// [`COMPLETION_EVENT_SUMMARY_MAX_BYTES`] — the bus copy this run's [`CompletionEvent`] carries,
/// computed HERE because `observe` is the last point that still holds the parsed payload before
/// `deliver_pending_completions` unlinks it a few lines later.
fn bounded_completion_summary(result: &crate::background::ResultFile) -> String {
    crate::exec::output::utf8_safe_prefix(
        &super::message::result_display_summary(result),
        COMPLETION_EVENT_SUMMARY_MAX_BYTES,
    )
    .to_string()
}

/// SUBA-034 — the in-process completion bus: pi's event bus, as the one thing cyrup can actually
/// reproduce of it.
///
/// # What this is, and the one thing it is not
///
/// pi's completion signal is an in-process event because pi's runner is in-process; cyrup's runner
/// is a detached OS process whose only signal is the terminal [`crate::background::ResultFile`] it writes
/// (R-SA-077). The ORCHESTRATOR half is still in-process, though: [`crate::background::watch::ResultsWatcher`] observes that
/// file inside the same process the `wait` tool runs in, so once the file has been observed there
/// is a real in-process edge to publish, and a waiter no longer has to discover the same fact again
/// on its own independent 1 s cadence.
///
/// **[CYRUP-DELTA]** — pi's publisher is the run itself, so upstream's wake is immediate; cyrup's
/// publisher is [`crate::background::watch::ResultsWatcher`], so the wake is bounded below by that watcher's own
/// [`crate::background::watch::RESULTS_DIR_POLL_INTERVAL`] (500 ms) rather than by 0. What the bus removes is the SECOND,
/// independent [`crate::background::wait::DEFAULT_POLL_INTERVAL_MS`] (1 s) delay stacked on top of
/// it — a waiter now reacts to the observation instead of re-deriving it. Closing the remaining
/// 500 ms would mean replacing `notify::PollWatcher` with a native backend, which is a separate,
/// deliberate R-SA-098 decision documented at the top of this module and is NOT changed here.
///
/// A lagging or dropped receiver is not an error: the poll under it is the reconciliation path
/// (upstream says the same of its own subscription), which is why [`Self::subscribe`] hands back a
/// receiver whose `Lagged` errors the waiter treats as "something happened, go look".
#[derive(Debug, Clone)]
pub struct CompletionBus {
    tx: tokio::sync::broadcast::Sender<CompletionEvent>,
}

/// How many completion events the bus keeps for a receiver that has not yet polled. A waiter only
/// ever needs to learn THAT something finished (it then re-reads the run tree), so the exact depth
/// is not load-bearing — but a fan-out of many children finishing together must not make a slow
/// receiver miss the edge entirely, and `Lagged` is itself treated as a wake-up.
const COMPLETION_BUS_CAPACITY: usize = 64;

impl Default for CompletionBus {
    fn default() -> Self {
        Self::new()
    }
}

impl CompletionBus {
    /// A fresh bus with no subscribers.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = tokio::sync::broadcast::channel(COMPLETION_BUS_CAPACITY);
        Self { tx }
    }

    /// Subscribe to every completion published from THIS point on.
    ///
    /// A subscriber must call this BEFORE it takes its own first snapshot of the run tree,
    /// otherwise a completion landing between the snapshot and the subscription is observed by
    /// neither and the waiter falls back to its poll — correct, but slow, which is the whole defect
    /// this closes.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CompletionEvent> {
        self.tx.subscribe()
    }

    /// Publish one terminal transition. Returns without error when nobody is listening — the
    /// common case, since `wait` is only subscribed while a wait is actually in flight.
    pub fn publish(&self, event: CompletionEvent) {
        let _ = self.tx.send(event);
    }
}

#[async_trait::async_trait]
impl CompletionObserver for CompletionBus {
    async fn observe(&self, notification: &CompletionNotification) -> bool {
        self.publish(CompletionEvent {
            run_id: notification.result.run_id.clone(),
            outcome: classify_outcome(&notification.result),
            // SCOPE_17 — rendered HERE because this is the last point that still holds the parsed
            // payload: `deliver_pending_completions` unlinks it a few lines later.
            summary: bounded_completion_summary(&notification.result),
        });
        // Publishing on a broadcast channel with no live receivers is not a failure: a `wait` that
        // nobody is blocked on is the normal case.
        true
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::super::tests::sample_result;
    use super::*;
    use crate::background::RunState;
    use crate::background::watch::CompletionBand;
    use std::path::PathBuf;
    use tokio::sync::Mutex as AsyncMutex;

    // =============================================================================================
    // SUBA-034 — the completion bus and the observer fan-out
    // =============================================================================================

    /// The bus publishes the SAME classification the notification text is built from, keyed by the
    /// run id — so a subscriber that woke on the event and a reader of the transcript can never
    /// disagree about which run finished or how.
    #[tokio::test]
    async fn the_bus_publishes_the_classified_outcome_of_each_observed_completion() {
        let bus = CompletionBus::new();
        let mut rx = bus.subscribe();

        // `state: Complete` with `success: false` is the R-SA-100 case a naive `state`-only
        // classifier gets wrong, so it is the one worth pinning on the wire.
        let notification = CompletionNotification {
            result: sample_result("run-bus-1", RunState::Complete, false),
            result_path: PathBuf::from("/tmp/run-bus-1.json"),
            exhausted: false,
            band: CompletionBand::Ours,
        };
        bus.observe(&notification).await;

        let event = rx.try_recv().expect("one event published");
        assert_eq!(event.run_id.as_str(), "run-bus-1");
        assert_eq!(event.outcome, ClassifiedOutcome::Failed);
    }

    /// D13 — the LOSING interleaving of the announce race, which is the one the naive reader gets
    /// wrong and the one that actually happens.
    ///
    /// The runner publishes its `ResultFile` — which is what wakes the completion watcher — BEFORE
    /// it writes its process-terminal proof (`runner_main::entry`'s tail: finish_run, lease
    /// release, candidate write, then `finalize_own_process_terminal`). So the observer routinely
    /// arrives while the sidecar still reads `pending`. A reader that accepts `pending` as an
    /// answer publishes nothing, and the run's proof is lost forever even though it landed
    /// milliseconds later.
    ///
    /// This test forces exactly that order: `pending` on disk when [`settled_proof`] is entered,
    /// `observed` written after it has already re-read at least twice. It fails if `pending` is
    /// ever treated as the answer — whether by returning it or by giving up and returning `None`.
    #[tokio::test]
    async fn a_pending_sidecar_that_settles_within_the_budget_is_still_announced() {
        use crate::background::process_terminal::{
            ProcessInstanceExit, ProcessTerminal, ProcessTerminalBase, ProcessTerminalState,
            ProofExpectation, RunnerProcessInstanceId, initialize_process_terminal,
        };
        use crate::background::{RunDir, RunId, RunMode, RunStatus};

        let tmp = tempfile::tempdir().expect("tempdir");
        let run_id = RunId::from_token("run-race-1");
        let instance = RunnerProcessInstanceId::from_token("instance-race-1");
        let run_dir = RunDir::new(tmp.path(), &run_id);
        tokio::fs::create_dir_all(run_dir.as_path())
            .await
            .expect("run dir");

        // The launch placeholder: `pending`, exactly as `initialize_process_terminal` leaves it
        // before the spawn.
        initialize_process_terminal(&run_dir, &run_id, &instance)
            .await
            .expect("initialize");

        // A status with NO recorded pid keeps `runner_may_still_be_writing` permissive, so the
        // crash short-circuit cannot end the wait early and what is under test is the `pending`
        // re-read itself rather than the pid probe.
        let status = RunStatus::queued(run_id.clone(), RunMode::Single, None);
        crate::background::atomic::write_atomic_json(&run_dir.status(), &status)
            .await
            .expect("status");

        // The runner's proof write, landing well inside PROCESS_TERMINAL_SETTLE_BUDGET but after
        // several PROCESS_TERMINAL_SETTLE_INTERVAL re-reads.
        let writer_dir = run_dir.clone();
        let writer_run = run_id.clone();
        let writer_instance = instance.clone();
        let writer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let observed = ProcessTerminal::Observed {
                base: ProcessTerminalBase::new(writer_run, writer_instance.clone()),
                observed_at: 1_700_000_000_000,
                instances: vec![ProcessInstanceExit::Runner {
                    process_instance_id: writer_instance,
                    close_observed_at: 1_700_000_000_000,
                    exit_code: Some(0),
                    signal: None,
                }],
                canonical_session: None,
            };
            crate::background::atomic::write_atomic_json(&writer_dir.process_terminal(), &observed)
                .await
                .expect("observed proof");
        });

        let proof = ProcessTerminalAnnouncingCompletionObserver::settled_proof(
            &run_dir,
            ProofExpectation::new(&run_id, &instance),
        )
        .await;
        writer.await.expect("writer task");

        let proof = proof.expect(
            "a sidecar that was `pending` on entry and `observed` 150ms later must be announced, \
             not dropped: the runner writes its ResultFile before its proof",
        );
        assert_eq!(proof.state(), ProcessTerminalState::Observed);
    }

    /// Publishing with nobody listening is a no-op, not an error: `wait` only subscribes while a
    /// wait is actually in flight, so the common case has zero receivers.
    #[tokio::test]
    async fn publishing_to_a_bus_with_no_subscribers_is_not_an_error() {
        let bus = CompletionBus::new();
        bus.observe(&CompletionNotification {
            result: sample_result("run-bus-2", RunState::Complete, true),
            result_path: PathBuf::from("/tmp/run-bus-2.json"),
            exhausted: false,
            band: CompletionBand::Ours,
        })
        .await;
        // A subscriber taken AFTER the publish sees nothing — which is exactly why
        // `wait_for_subagents` subscribes before its first listing.
        assert!(bus.subscribe().try_recv().is_err());
    }

    /// Every member of the fan-out runs, in registration order. The two members are given DISTINCT
    /// observable effects on purpose: a composite that silently dropped one of them (or that ran
    /// only the first) would still pass a test whose members were interchangeable.
    #[tokio::test]
    async fn the_composite_observer_runs_every_member_in_order() {
        struct Recorder {
            tag: &'static str,
            log: Arc<AsyncMutex<Vec<String>>>,
        }
        #[async_trait::async_trait]
        impl CompletionObserver for Recorder {
            async fn observe(&self, notification: &CompletionNotification) -> bool {
                self.log.lock().await.push(format!(
                    "{}:{}",
                    self.tag,
                    notification.result.run_id.as_str()
                ));
                true
            }
        }

        let log: Arc<AsyncMutex<Vec<String>>> = Arc::new(AsyncMutex::new(Vec::new()));
        let bus = CompletionBus::new();
        let mut rx = bus.subscribe();
        let composite = CompositeCompletionObserver::new(vec![
            Arc::new(Recorder {
                tag: "first",
                log: Arc::clone(&log),
            }),
            Arc::new(bus),
            Arc::new(Recorder {
                tag: "last",
                log: Arc::clone(&log),
            }),
        ]);

        composite
            .observe(&CompletionNotification {
                result: sample_result("run-fanout", RunState::Complete, true),
                result_path: PathBuf::from("/tmp/run-fanout.json"),
                exhausted: false,
                band: CompletionBand::Ours,
            })
            .await;

        assert_eq!(
            log.lock().await.clone(),
            vec![
                "first:run-fanout".to_string(),
                "last:run-fanout".to_string()
            ],
            "both recorders must run, in registration order"
        );
        assert_eq!(
            rx.try_recv()
                .expect("the bus member published too")
                .run_id
                .as_str(),
            "run-fanout",
            "the bus sitting BETWEEN two other members must not be skipped"
        );
    }
}
