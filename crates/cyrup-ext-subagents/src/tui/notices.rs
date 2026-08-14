//! `ControlNoticeState` — the control-notice debounce/actionability/dedup state machine (func-SA
//! §5.5, R-SA-114/115/116/117/118/121/122; arch-SA §6.7).
//!
//! This module is the SOLE owner of the "when does a `needs-attention`/`active-long-running`
//! heuristic notice actually reach the transcript" decision. Three independent behaviors compose
//! here, and getting their interaction right is what func-SA/arch-SA both flag as the trickiest
//! piece of this subsystem to port faithfully:
//!
//! 1. **Source-dependent dispatch (R-SA-116/117).** An [`RunSource::Async`] notice is delivered
//!    immediately, with no delay. An [`RunSource::Foreground`] notice is *debounced*: it is held
//!    for a fixed window (`DEBOUNCE_MS`, target 1000ms) and, only if the window elapses without a
//!    newer ping for the same `(run_id, kind)` superseding it, re-validated against the run's
//!    *then-current* live state before being delivered at all.
//! 2. **At-fire-time re-validation, never at-schedule-time (R-SA-116).** The three actionability
//!    checks — "is this run still tracked at all", "is the current agent still the one this notice
//!    was raised about", "is the current step index still the one this notice was raised about",
//!    "is the run's live activity state still `NeedsAttention`" — are evaluated against
//!    [`ControlNoticeState::live_runs`] **at the moment the debounce timer fires**, never captured
//!    at schedule time. A run that advances past the relevant step, switches agent, changes step
//!    index, or stops needing attention before the timer fires causes the notice to be silently
//!    dropped, not delivered stale (per DI-SA-10: a "needs attention" heuristic must never override
//!    or outlive the live state it was diagnosing).
//! 3. **At-most-once delivery dedup, surviving hot-reload (R-SA-115/122).** `delivered` is keyed on
//!    [`super::ControlNoticeKey`] — `(run_id, notice_kind, pi's own
//!    `controlNotificationKey(event, childIntercomTarget)`)` — and is checked immediately before
//!    any delivery — both the async-immediate and the foreground-debounced-and-revalidated paths
//!    fold through the exact same [`ControlNoticeState::deliver`] choke point, so there is exactly
//!    one place dedup is enforced, not two independently-maintained sets. Per R-SA-122 this set
//!    MUST persist across an in-process hot-reload of the orchestrating extension for the lifetime
//!    of the *process* — this module satisfies that by construction: `ControlNoticeState` holds no
//!    reference to anything reload-scoped (no extension handle, no session handle), so a caller
//!    that keeps the same `Arc<tokio::sync::Mutex<ControlNoticeState>>` alive across a hot-reload
//!    (arch-SA §3.1's `SubagentsExtension.notices` field) trivially preserves this set intact; nothing
//!    in this module itself needs to special-case "was this a reload".
//!
//! # What this module deliberately does NOT do
//!
//! Per the crate-wide mandatory-mechanism constraint (`lib.rs` module docs; func-SA §1.1): this is
//! a **pure, in-process bookkeeping state machine** over plain data ([`super::ControlNotice`],
//! [`super::SubagentProgressSnapshot`]) — it holds no subprocess handle, no session handle, and no
//! reference to any live agent turn loop. Delivery is a callback through [`ControlNoticeSink`], a
//! narrow trait that later-phase call sites (`extension.rs`, `tui/render.rs`) implement against
//! whatever concrete transcript-event sink `spec/extensions/subagents/01-architecture.md` §12 item
//! 4/5 eventually wires up (an `AgentSessionEvent`/`Entry` variant per §6.7's "Rendering
//! integration" note) — this module does not need, and must not gain, any earlier knowledge of
//! that concrete type. Turn-triggering (R-SA-118) is likewise expressed as a plain `bool` argument
//! passed to the sink, never as this module itself reaching into an agent turn loop.
//!
//! # Live-state feed (`live_runs`)
//!
//! [`ControlNoticeState::live_runs`] is fed by [`ControlNoticeState::observe_run`], which the
//! owning caller (the same per-run NDJSON-folding task that maintains
//! [`super::SubagentProgressSnapshot`] elsewhere in this crate, per arch-SA §5.2) is expected to
//! call on every observed progress update — this module does not itself poll or subscribe to
//! anything; it is a passive re-validation oracle over whatever the caller most recently told it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::background::RunId;

use super::{ControlNotice, ControlNoticeKey, RunSource};

// =================================================================================================
// Tuning constants
// =================================================================================================

/// The fixed foreground debounce window (R-SA-116's target: 1000ms). A [`RunSource::Foreground`]
/// notice is held for this long — re-armed on every superseding ping for the same key — before
/// being re-validated and (possibly) delivered.
pub const DEBOUNCE_MS: Duration = Duration::from_millis(1000);

// =================================================================================================
// LiveRunView — the minimal live-state projection actionability re-checks are evaluated against
// =================================================================================================

/// The minimal slice of a run's current live state this module needs to re-validate a pending
/// foreground notice at fire time (R-SA-116). Deliberately narrower than
/// [`super::SubagentProgressSnapshot`] (which this type is typically projected from by the
/// caller): only the fields the three actionability checks actually consult.
///
/// # `needs_attention` is a heuristic flag, not a lifecycle state (R-SA-114/DI-SA-10)
///
/// This field is a plain `bool` the caller computes independently of
/// [`crate::background::RunState`] (`Running`/`Paused`/`Complete`/`Failed`/`Queued`) — it MUST be
/// `false` whenever the run's underlying lifecycle state is `Paused` (intentional soft-interrupt)
/// or a terminal state (`Complete`/`Failed`), so that this module's re-check can never conflate
/// "no fresh activity for a while" with "the run was deliberately paused" or "the run already
/// finished". Enforcing that exclusion is the caller's responsibility when constructing/updating a
/// [`LiveRunView`] (typically the same fold point that derives
/// [`super::SubagentProgressSnapshot::last_activity_at`] staleness); this module only ever reads
/// the flag, never derives it from a [`crate::background::RunState`] itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRunView {
    /// The agent persona currently active for this run, if known.
    pub current_agent: Option<String>,
    /// The zero-based step/chain index currently active for this run, if applicable.
    pub current_step_index: Option<u32>,
    /// Whether this run's live activity state is presently `needs-attention` — independent of, and
    /// never derived from, `Paused`/`Failed` lifecycle state (R-SA-114).
    pub needs_attention: bool,
}

// =================================================================================================
// ControlNoticeSink — the narrow delivery seam (no in-process agent-turn coupling)
// =================================================================================================

/// The delivery callback [`ControlNoticeState`] invokes once a notice has cleared dedup (and, for
/// foreground notices, the debounce+actionability re-check). Implemented by a later-phase call
/// site against whatever concrete transcript-event sink is ultimately wired up (arch-SA §6.7's
/// "Rendering integration" note; §12 items 4/5) — this trait is intentionally the only contract
/// this module has with that outside world, so `tui/notices.rs` itself never needs to know the
/// concrete sink type, and gains no session/agent-loop access of its own.
///
/// `trigger_turn` carries R-SA-118's source-dependent behavior: `true` for a delivered async
/// notice (which SHOULD be capable of prompting the orchestrator's own agent to react on its next
/// turn), `false` for a delivered foreground notice (the orchestrator is already mid-turn when a
/// foreground notice fires, so it SHOULD NOT force a new one). The sink is responsible for
/// actually acting on that flag — this module only computes and passes it through.
pub trait ControlNoticeSink: Send + Sync + 'static {
    /// Deliver `notice` to the transcript as a distinguishable, non-tool-result entry (R-SA-121).
    /// `trigger_turn` is `true` only for a delivered async notice (R-SA-118).
    fn emit_control_notice(&self, notice: ControlNotice, trigger_turn: bool);
}

/// A [`ControlNoticeSink`] blanket impl over any `Fn(ControlNotice, bool)` closure, so tests and
/// lightweight call sites can supply an inline delivery callback without declaring a named type.
impl<F> ControlNoticeSink for F
where
    F: Fn(ControlNotice, bool) + Send + Sync + 'static,
{
    fn emit_control_notice(&self, notice: ControlNotice, trigger_turn: bool) {
        self(notice, trigger_turn);
    }
}

/// pi `SUBAGENT_CONTROL_MESSAGE_TYPE` (`extension/control-notices.ts:5`): the `customType` a
/// delivered control notice is injected under, so a transcript renderer can style it as its own
/// entry class rather than as ordinary assistant prose (R-SA-121).
pub const SUBAGENT_CONTROL_MESSAGE_TYPE: &str = "subagent_control_notice";

/// The graceful-degradation delivery sink: writes the notice to stderr. Used when no live host
/// message channel is bound (headless / SDK embedder), so the pipeline stays observable instead of
/// silently discarding notices — the same shape (and rationale) as
/// [`crate::background::watch::LoggingCompletionSink`].
#[derive(Debug, Default)]
pub struct LoggingControlNoticeSink;

impl ControlNoticeSink for LoggingControlNoticeSink {
    fn emit_control_notice(&self, notice: ControlNotice, trigger_turn: bool) {
        eprintln!(
            "[subagent-control] (trigger_turn={trigger_turn}) {}",
            notice.message
        );
    }
}

/// The REAL transcript-injecting sink: pi's `pi.sendMessage({customType, content, display}, {
/// triggerTurn})` (`extension/control-notices.ts:33-41`), routed through the P-1
/// [`cyrup_ext::host::HostServices::inject_message`] backend — the identical hand-off
/// [`crate::background::watch::HostServicesCompletionSink`] already uses for background-completion
/// notifications.
///
/// `inject_message` is a synchronous host round-trip, and this trait method is synchronous and
/// called from inside the notice state machine's lock, so the injection is handed to a blocking
/// task rather than performed inline: a slow turn loop must never stall the debounce machinery or
/// the run whose child raised the notice.
pub struct HostServicesControlNoticeSink {
    services: Arc<dyn cyrup_ext::host::HostServices>,
}

impl HostServicesControlNoticeSink {
    /// Build a sink over the late-bound live capability backend (P-1).
    #[must_use]
    pub fn new(services: Arc<dyn cyrup_ext::host::HostServices>) -> Self {
        Self { services }
    }
}

impl ControlNoticeSink for HostServicesControlNoticeSink {
    fn emit_control_notice(&self, notice: ControlNotice, trigger_turn: bool) {
        let services = Arc::clone(&self.services);
        let content = notice.message;
        // Fire-and-forget: a failed injection is logged by the host, and — unlike a completion
        // notification, which gates deletion of a result file on delivery — a dropped control
        // notice has nothing to retry against, since the dedup set has already claimed its key
        // (which is pi's behaviour too: `sendMessage` is not awaited or checked there either).
        drop(tokio::task::spawn_blocking(move || {
            let _ = services.inject_message(
                &content,
                Some(SUBAGENT_CONTROL_MESSAGE_TYPE),
                true,
                trigger_turn,
            );
        }));
    }
}

// =================================================================================================
// ControlNoticeState
// =================================================================================================

/// The control-notice debounce/actionability/dedup state machine (arch-SA §6.7).
///
/// Plain, non-`Arc`-wrapped struct — per arch-SA §3.1, the owning [`SubagentsExtension`]-analogous
/// caller wraps this in its own `Arc<tokio::sync::Mutex<ControlNoticeState>>`, since the extension
/// host's `on_event`/command handlers may need to reach it from a different task than the one
/// folding NDJSON progress events. Methods that must schedule a delayed re-check (foreground
/// debounce) therefore take `self: &Arc<tokio::sync::Mutex<Self>>` — the identical "shared-self"
/// shape [`crate::background::tracker::JobTracker`] already uses for its own
/// self-starting/self-stopping poller — rather than this type inventing a second, bespoke
/// concurrency shape for the same "a struct needs to hand a clone of its own shared handle to a
/// spawned task" problem.
#[derive(Default)]
pub struct ControlNoticeState {
    /// At-most-once delivery dedup, keyed on `(run_id, notice_kind)` (R-SA-115/122). Once a key is
    /// inserted here it is delivered for the lifetime of this `ControlNoticeState` value — never
    /// removed, so a caller that keeps this state alive across a hot-reload (arch-SA §3.1) gets
    /// R-SA-122's persistence property for free.
    delivered: HashSet<ControlNoticeKey>,

    /// Outstanding foreground debounce timers, keyed on the same `(run_id, notice_kind)` identity.
    /// A newer ping for a key that already has a pending timer aborts and replaces it (coalescing
    /// repeated pings into a single re-armed wait), rather than allowing two competing timers for
    /// the same key to race.
    pending: HashMap<ControlNoticeKey, JoinHandle<()>>,

    /// The live-state projection every foreground re-check is evaluated against at fire time
    /// (R-SA-116) — fed by [`ControlNoticeState::observe_run`]/[`ControlNoticeState::forget_run`],
    /// never mutated by the debounce machinery itself.
    live_runs: HashMap<RunId, LiveRunView>,

    /// The debounce window used for [`RunSource::Foreground`] notices — [`DEBOUNCE_MS`] in
    /// production, overridable via [`ControlNoticeState::with_debounce`] so tests need not sleep
    /// out the full 1000ms production window for every scenario.
    debounce: Duration,
}

impl ControlNoticeState {
    /// Constructs an empty state machine using the production [`DEBOUNCE_MS`] window.
    #[must_use]
    pub fn new() -> Self {
        Self {
            delivered: HashSet::new(),
            pending: HashMap::new(),
            live_runs: HashMap::new(),
            debounce: DEBOUNCE_MS,
        }
    }

    /// Constructs an empty state machine with an explicit debounce window — the constructor tests
    /// use to run a short window so the four-scenario debounce/actionability suite does not need
    /// to wait out the full 1000ms production default per test.
    #[must_use]
    pub fn with_debounce(debounce: Duration) -> Self {
        Self {
            delivered: HashSet::new(),
            pending: HashMap::new(),
            live_runs: HashMap::new(),
            debounce,
        }
    }

    /// Records (or refreshes) the live-state projection for `run_id` — the input every foreground
    /// re-check's three actionability checks (R-SA-116) reads at fire time. The caller (the
    /// per-run NDJSON-folding task that also maintains [`super::SubagentProgressSnapshot`]) is
    /// expected to call this on every observed progress update, including updates that do not
    /// themselves raise a notice.
    pub fn observe_run(&mut self, run_id: RunId, view: LiveRunView) {
        self.live_runs.insert(run_id, view);
    }

    /// Removes `run_id` from the live-state projection entirely — e.g. once a run's tracked
    /// lifetime ends and the caller no longer wants it consulted for actionability re-checks.
    ///
    /// Also ABORTS every pending foreground debounce timer keyed to this run, which is pi's own
    /// run-teardown sequence verbatim: `clearPendingForegroundControlNotices(deps.state, runId)`
    /// immediately followed by `deps.state.foregroundControls.delete(runId)`
    /// (`subagent-executor.ts:3579-3581` @v0.34.0; the timer map is keyed
    /// `"{runId}:{controlNotificationKey}"` precisely so it can be filtered by run id,
    /// `control-notices.ts:23-36` @v0.34.0). The abort is not merely hygiene: without it a timer for a
    /// finished run stays armed for the rest of its window holding an `Arc` to the state and the
    /// sink, and — if anything ever re-registers that run id — would fire against the NEW
    /// registration's live view. Dropping the live entry alone already makes such a timer
    /// non-actionable (R-SA-116 check 1), so this is belt-and-braces in the same order upstream
    /// applies it.
    pub fn forget_run(&mut self, run_id: &RunId) {
        self.live_runs.remove(run_id);
        self.pending.retain(|key, timer| {
            if &key.run_id == run_id {
                timer.abort();
                return false;
            }
            true
        });
    }

    /// Returns `true` if a notice with this exact `(run_id, kind)` key has already been delivered
    /// (R-SA-115/122). Exposed primarily for tests and diagnostic call sites; production dispatch
    /// never needs to consult this ahead of calling [`ControlNoticeState::handle`], since `handle`
    /// performs the check itself before any delivery.
    #[must_use]
    pub fn was_delivered(&self, key: &ControlNoticeKey) -> bool {
        self.delivered.contains(key)
    }

    /// `true` if a foreground debounce timer is currently outstanding for `key`. Exposed for
    /// tests that need to assert coalescing behavior (a second ping for the same key replaces,
    /// rather than adds to, the pending set).
    #[must_use]
    pub fn has_pending(&self, key: &ControlNoticeKey) -> bool {
        self.pending.contains_key(key)
    }

    /// The live-state projection currently recorded for `run_id`, or `None` if the run is not
    /// tracked at all — R-SA-116's check 1, exposed for inspection.
    ///
    /// This is the third of the same family as [`Self::was_delivered`]/[`Self::has_pending`]:
    /// a read-only window onto one of the three maps this state machine owns, so a caller (in
    /// practice a test) can assert the machine's own view rather than inferring it from delivery
    /// side effects. Production dispatch never needs it — [`Self::handle`] consults `live_runs`
    /// itself, at fire time.
    ///
    /// It is what makes the "a control event raised in a run's dying moments must never be applied
    /// AFTER teardown" property directly observable: `forget_run` must leave this `None`, and no
    /// later hand-off may resurrect it.
    #[must_use]
    pub fn live_view(&self, run_id: &RunId) -> Option<&LiveRunView> {
        self.live_runs.get(run_id)
    }

    /// Dispatches `ev` per its [`RunSource`] (R-SA-116/117):
    ///
    /// - [`RunSource::Async`] notices are delivered immediately, with no debounce (R-SA-117).
    /// - [`RunSource::Foreground`] notices are debounced: armed (or re-armed, coalescing any
    ///   already-pending timer for the same key) for the state's own `debounce` window, then
    ///   re-validated against live state at fire time before being delivered (R-SA-116).
    ///
    /// Free (non-method) function taking `state: &Arc<tokio::sync::Mutex<ControlNoticeState>>`
    /// rather than a `self`-receiver method: Rust's `self: T` receiver sugar only supports a
    /// small fixed set of wrapper shapes (`Self`, `&Self`, `Box<Self>`, `Rc<Self>`, `Arc<Self>`,
    /// `Pin<P>`), none of which cover the doubly-wrapped `Arc<tokio::sync::Mutex<Self>>` shape
    /// arch-SA §3.1 requires here (the `Mutex` layer is load-bearing — `ControlNoticeState`
    /// itself must stay a plain, lock-free struct per its own type doc, with the *caller*
    /// supplying the `Arc<Mutex<_>>` wrapper) — so this is a plain associated function over an
    /// explicit shared handle instead, matching call sites as `notices::handle(&state, ev,
    /// sink).await`.
    pub async fn handle<S: ControlNoticeSink + ?Sized>(
        state: &Arc<AsyncMutex<Self>>,
        ev: ControlNotice,
        sink: Arc<S>,
    ) {
        match ev.source {
            // R-SA-117: async notices are delivered immediately, no debounce. A GOAL notice takes
            // the same immediate path (pi `handleSubagentControlNotice` early-returns ONLY for
            // `"foreground"`, `extension/control-notices.ts:49`); it differs from an async notice
            // in `trigger_turn`, which `deliver` decides.
            RunSource::Async | RunSource::Goal => {
                let mut guard = state.lock().await;
                guard.deliver(ev, sink.as_ref());
            }
            // R-SA-116: foreground notices are debounced, then re-validated at fire time.
            RunSource::Foreground => Self::debounce_then_check(state, ev, sink).await,
        }
    }

    /// Arms (or re-arms) the foreground debounce timer for `ev.key`. If a timer is already
    /// pending for this exact key, it is aborted and replaced (coalescing repeated pings into a
    /// single re-armed wait, per arch-SA §6.7 step 1) rather than left to race a second timer for
    /// the same key.
    async fn debounce_then_check<S: ControlNoticeSink + ?Sized>(
        state: &Arc<AsyncMutex<Self>>,
        ev: ControlNotice,
        sink: Arc<S>,
    ) {
        let key = ev.key.clone();
        // ONE critical section spans coalesce -> arm -> register, mirroring the fact that pi does
        // all three in a single synchronous block (`clearTimeout` / `setTimeout` / `pending.set`,
        // `extension/control-notices.ts:84-91` @`6a26f25^`, the pre-v0.43.0 shape this module
        // ports). JS cannot interleave a timer callback with the block that arms it; Rust can, and
        // splitting this into two lock acquisitions around a `tokio::spawn` opened a window where a
        // short-debounce timer fires, removes its (not-yet-present) key, and then has its already
        // dead `JoinHandle` inserted by the arming task — leaving `pending` permanently claiming a
        // timer that has already delivered, so the next ping "coalesces" against nothing.
        let mut guard = state.lock().await;
        // Coalesce: abort any timer already pending for this key before arming a fresh one,
        // so a burst of repeated "still needs attention" pings for the same run+kind collapses
        // to one re-armed wait rather than N racing deliveries.
        if let Some(old) = guard.pending.remove(&key) {
            old.abort();
        }
        // The deadline is fixed HERE, at ping time, exactly as pi's `setTimeout(…, delayMs)`
        // (`extension/control-notices.ts:85` @`6a26f25^`) fixes it at call time. Constructing the
        // `Sleep` inside the spawned task instead would start the countdown at the task's FIRST
        // POLL — an unbounded, scheduler-dependent amount of time later — so a coalescing ping
        // would push the fire time out by however long the runtime took to pick the task up.
        let timer = tokio::time::sleep(guard.debounce);

        let state_for_task = Arc::clone(state);
        let key_for_task = key.clone();
        let handle = tokio::spawn(async move {
            timer.await;
            // Fire: re-validate against LIVE state at this exact moment, never against whatever
            // was true when this timer was scheduled (R-SA-116's core correctness property).
            let mut guard = state_for_task.lock().await;
            // This timer's own pending-map entry is cleared unconditionally on fire, whether or
            // not the notice ultimately survives the actionability re-check below — a fired timer
            // is no longer "pending" either way.
            guard.pending.remove(&key_for_task);
            if guard.is_still_actionable(&ev) {
                guard.deliver(ev, sink.as_ref());
            }
            // else: silently dropped — the run advanced past the relevant step, changed agent,
            // changed step index, or is no longer in the needs-attention activity state (R-SA-116).
        });

        guard.pending.insert(key, handle);
    }

    /// The three R-SA-116 actionability checks, evaluated against
    /// [`ControlNoticeState::live_runs`] as it stands at the moment this method is called (never
    /// against a value captured earlier) — the caller (the debounce timer's fire handler) is
    /// responsible for only calling this at actual fire time, not at schedule time.
    ///
    /// Returns `false` (drop) if:
    /// 1. The tracked live state for `ev.key.run_id` no longer exists at all (the run was
    ///    forgotten, e.g. because it completed and was reaped).
    /// 2. `ev.agent` was recorded and no longer matches the run's live `current_agent`.
    /// 3. `ev.step_index` was recorded and no longer matches the run's live `current_step_index`.
    /// 4. The run's live activity state is no longer `needs_attention` (it caught up, was paused,
    ///    or finished — R-SA-114/DI-SA-10: this is a heuristic re-check, never a lifecycle-state
    ///    override).
    ///
    /// An unset `ev.agent`/`ev.step_index` (`None`) is treated as "not applicable to this check" —
    /// mirroring arch-SA §6.7's illustrative `ev.agent.is_none() || live.current_agent ==
    /// ev.agent` shape — rather than an automatic pass/fail, since not every notice kind
    /// necessarily carries both.
    fn is_still_actionable(&self, ev: &ControlNotice) -> bool {
        let Some(live) = self.live_runs.get(&ev.key.run_id) else {
            return false; // check 1: tracked state no longer exists
        };
        let agent_still_matches = ev.agent.is_none() || ev.agent == live.current_agent;
        let step_still_matches =
            ev.step_index.is_none() || ev.step_index == live.current_step_index;
        agent_still_matches            // check 2
            && step_still_matches      // check 3
            && live.needs_attention    // check 4: still in the needs-attention activity state
    }

    /// The single delivery choke point every dispatch path (async-immediate, foreground-debounced-
    /// and-revalidated) folds through. Performs the at-most-once dedup check (R-SA-115/122) and,
    /// only on first delivery for this key, invokes the sink with R-SA-118's source-dependent
    /// `trigger_turn` flag.
    fn deliver<S: ControlNoticeSink + ?Sized>(&mut self, ev: ControlNotice, sink: &S) {
        if !self.delivered.insert(ev.key.clone()) {
            return; // R-SA-115/122: already delivered for this (run_id, kind) — never re-notify.
        }
        // R-SA-118: a delivered async notice may trigger a new orchestrator turn; a delivered
        // foreground notice never does, since the orchestrator is already mid-turn when it fires.
        // A GOAL notice does not either — pi's `{ triggerTurn: details.source === "async" }`
        // (`extension/control-notices.ts:39`) is an equality test on `"async"`, not a negation of
        // `"foreground"`, and the difference only becomes observable once a third source exists.
        let trigger_turn = ev.source == RunSource::Async;
        sink.emit_control_notice(ev, trigger_turn);
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
    use super::super::ControlNoticeKind;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A tiny recording sink: counts deliveries and remembers the `trigger_turn` flag and message
    /// for the most recent delivery, so tests can assert both "did it deliver" and "with what
    /// turn-triggering behavior" without needing a real transcript.
    #[derive(Default)]
    struct RecordingSink {
        deliveries: std::sync::Mutex<Vec<(ControlNotice, bool)>>,
    }

    impl RecordingSink {
        fn count(&self) -> usize {
            self.deliveries.lock().unwrap_or_else(|e| e.into_inner()).len()
        }

        fn last(&self) -> Option<(ControlNotice, bool)> {
            self.deliveries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last()
                .cloned()
        }
    }

    impl ControlNoticeSink for RecordingSink {
        fn emit_control_notice(&self, notice: ControlNotice, trigger_turn: bool) {
            self.deliveries
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((notice, trigger_turn));
        }
    }

    fn needs_attention_notice(
        run_id: &RunId,
        agent: Option<&str>,
        step_index: Option<u32>,
        source: RunSource,
    ) -> ControlNotice {
        ControlNotice {
            key: ControlNoticeKey {
                run_id: run_id.clone(),
                kind: ControlNoticeKind::NeedsAttention,
                notification_key: format!("{run_id}:needs_attention:idle"),
            },
            source,
            agent: agent.map(str::to_string),
            step_index,
            reason: "no NDJSON activity for 60s".to_string(),
            message: "This run may need attention.".to_string(),
        }
    }

    fn actionable_view(agent: &str, step_index: u32) -> LiveRunView {
        LiveRunView {
            current_agent: Some(agent.to_string()),
            current_step_index: Some(step_index),
            needs_attention: true,
        }
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-117: async delivers immediately, no debounce.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn async_notice_delivers_immediately_with_trigger_turn_true() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_secs(60), // deliberately long — async must not wait on this at all
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Async);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        assert_eq!(sink.count(), 1, "async notice must deliver without any wait");
        let (_, trigger_turn) = sink.last().expect("one delivery recorded");
        assert!(trigger_turn, "R-SA-118: async delivery must trigger a new turn");
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-115/122: at-most-once dedup per (run_id, kind).
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn repeated_async_pings_for_the_same_key_deliver_exactly_once() {
        // A-SA-15: two consecutive "needs attention" firings for the same run in the same
        // attention state produce exactly one delivered notice.
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::new()));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();
        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev1 = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Async);
        let ev2 = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Async);

        ControlNoticeState::handle(&state, ev1, Arc::clone(&sink)).await;
        ControlNoticeState::handle(&state, ev2, Arc::clone(&sink)).await;

        assert_eq!(
            sink.count(),
            1,
            "the same (run_id, kind) key must never be delivered twice"
        );
    }

    #[tokio::test]
    async fn dedup_state_persists_across_a_simulated_hot_reload() {
        // R-SA-122: dedup must persist across a hot-reload of the orchestrating extension for
        // the lifetime of the process. Simulated here by simply continuing to use the same
        // `ControlNoticeState` value (which is exactly what R-SA-122 requires the owning
        // extension's `Arc<Mutex<ControlNoticeState>>` field to do across a reload) and asserting
        // a post-"reload" ping still doesn't re-deliver.
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::new()));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();
        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Async);
        ControlNoticeState::handle(&state, ev.clone(), Arc::clone(&sink)).await;
        assert_eq!(sink.count(), 1);

        // "Hot-reload" happens here in spirit: nothing about this call changes because a reload
        // occurred, since `ControlNoticeState` itself holds no reload-scoped reference.
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;
        assert_eq!(sink.count(), 1, "dedup must survive across the simulated reload");
    }

    #[tokio::test]
    async fn distinct_notice_kinds_for_the_same_run_are_independently_delivered() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::new()));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();
        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let needs_attention =
            needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Async);
        let mut active_long_running = needs_attention.clone();
        active_long_running.key.kind = ControlNoticeKind::ActiveLongRunning;

        ControlNoticeState::handle(&state, needs_attention, Arc::clone(&sink)).await;
        ControlNoticeState::handle(&state, active_long_running, Arc::clone(&sink)).await;

        assert_eq!(
            sink.count(),
            2,
            "distinct (run_id, kind) pairs are independent dedup keys"
        );
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-118: foreground delivery never triggers a new turn.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn foreground_delivery_sets_trigger_turn_false() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();
        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        wait_for_delivery(&sink).await;
        let (_, trigger_turn) = sink.last().expect("delivered after debounce");
        assert!(
            !trigger_turn,
            "R-SA-118: foreground delivery must never trigger a new orchestrator turn"
        );
    }

    // ---------------------------------------------------------------------------------------
    // The four-scenario foreground debounce/actionability re-check suite (R-SA-116), mirroring
    // pi-subagents' own `control-notices.test.ts` four scenarios. Each is a SEPARATE test case.
    // ---------------------------------------------------------------------------------------

    /// Scenario 1: the run advanced past the notice-relevant step by fire time — dropped.
    ///
    /// The notice was raised while the run was at step 0; by the time the debounce timer fires,
    /// live state shows the run has moved on to step 1 (same agent). Per R-SA-116 check 3
    /// (step index no longer matches), the notice must be silently dropped.
    #[tokio::test]
    async fn scenario_1_run_advanced_past_the_relevant_step_is_dropped() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        // Live state at schedule time: step 0, needs attention.
        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        // The run advances to step 1 with the SAME agent before the timer fires.
        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 1))
            .await;

        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            0,
            "a notice for a step the run has since advanced past must be dropped, not delivered"
        );
    }

    /// Scenario 2: the agent changed by fire time — dropped.
    ///
    /// The notice was raised while `scout` was active; by fire time live state shows a different
    /// agent (`delegate`) is now active for this run. Per R-SA-116 check 2, dropped.
    #[tokio::test]
    async fn scenario_2_agent_changed_by_fire_time_is_dropped() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        // A different agent is now active for the same run and step index before the timer fires.
        state
            .observe_run_locked(run_id.clone(), actionable_view("delegate", 0))
            .await;

        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            0,
            "a notice for an agent the run has since moved on from must be dropped"
        );
    }

    /// Scenario 3: the index/step changed by fire time — dropped (distinct from scenario 1's
    /// "advanced past" framing: this covers a step-index-only mismatch on its own, independent of
    /// which specific direction the index moved).
    #[tokio::test]
    async fn scenario_3_step_index_changed_by_fire_time_is_dropped() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("planner", 2))
            .await;

        let ev = needs_attention_notice(&run_id, Some("planner"), Some(2), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        // Step index moves to 5 (e.g. a chain-append/resume changed the active index) before fire.
        state
            .observe_run_locked(run_id.clone(), actionable_view("planner", 5))
            .await;

        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            0,
            "a notice whose recorded step index no longer matches live state must be dropped"
        );
    }

    /// Scenario 4: still actionable at fire time — delivered.
    ///
    /// Live state at fire time is unchanged from schedule time: same agent, same step index, still
    /// needs attention. The notice must survive the re-check and be delivered exactly once, with
    /// `trigger_turn = false` (R-SA-118).
    #[tokio::test]
    async fn scenario_4_still_actionable_at_fire_time_is_delivered() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("researcher", 1))
            .await;

        let ev =
            needs_attention_notice(&run_id, Some("researcher"), Some(1), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        // Live state is unchanged by fire time.
        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            1,
            "a notice that is still actionable at fire time must be delivered"
        );
        let (delivered, trigger_turn) = sink.last().expect("delivered");
        assert_eq!(delivered.key.run_id, run_id);
        assert!(!trigger_turn, "R-SA-118: foreground delivery never triggers a new turn");
    }

    // ---------------------------------------------------------------------------------------
    // Additional R-SA-114/116 coverage: activity-state-no-longer-needs-attention drop path, and
    // the "tracked state no longer exists at all" drop path.
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn dropped_when_run_no_longer_needs_attention_at_fire_time() {
        // R-SA-114/DI-SA-10: the run caught up (e.g. new NDJSON activity arrived) between
        // schedule time and fire time — the heuristic must not fire a stale notice.
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("worker", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("worker"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        // The run produced fresh activity; needs_attention flips false while agent/step stay put.
        state
            .observe_run_locked(
                run_id.clone(),
                LiveRunView {
                    current_agent: Some("worker".to_string()),
                    current_step_index: Some(0),
                    needs_attention: false,
                },
            )
            .await;

        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            0,
            "a run that caught back up before fire time must not deliver a stale notice"
        );
    }

    #[tokio::test]
    async fn dropped_when_tracked_run_no_longer_exists_at_fire_time() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("oracle", 0))
            .await;

        let ev = needs_attention_notice(&run_id, Some("oracle"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        // The run is forgotten entirely (e.g. it was reaped) before the timer fires.
        state.forget_run_locked(&run_id).await;

        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            0,
            "a notice for a run that is no longer tracked at all must be dropped"
        );
    }

    #[tokio::test]
    async fn unset_agent_and_step_on_the_notice_are_not_treated_as_mismatches() {
        // A notice that never recorded an agent/step (both None) must not be spuriously dropped
        // merely because live state now has concrete values — R-SA-116's checks are "if recorded,
        // must still match", not "must always have matched".
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(20),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("context-builder", 3))
            .await;

        let ev = needs_attention_notice(&run_id, None, None, RunSource::Foreground);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        wait_past_debounce().await;

        assert_eq!(
            sink.count(),
            1,
            "an unset agent/step on the notice must not block delivery"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Coalescing: a second ping for the same key before the first timer fires replaces it,
    // rather than both timers independently firing.
    // ---------------------------------------------------------------------------------------

    /// SUBA-032 — this used to be three REAL-clock sleeps (20/45/40 ms) around a 60 ms debounce,
    /// with the load-bearing "must not have fired yet" assertion landing 15 ms inside the deadline
    /// and an overshoot on the SECOND sleep being fatal. On a loaded box the second sleep
    /// overshoots, the test flakes, a flaky test gets `#[ignore]`d, and the debounce loses coverage
    /// entirely. The in-repo precedent is commit `1806375`, which removed a structurally identical
    /// assertion from `cyrup-ext/src/caps/proc.rs`.
    ///
    /// `start_paused = true` puts the runtime on a virtual clock: `tokio::time::advance` moves it
    /// by an EXACT amount and nothing else can, so the margins below are not margins at all — they
    /// are the actual timeline. Runtime is ~0 regardless of machine load.
    #[tokio::test(start_paused = true)]
    async fn a_second_ping_before_fire_time_coalesces_the_pending_timer() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::with_debounce(
            Duration::from_millis(60),
        )));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();

        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;

        let ev1 = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev1, Arc::clone(&sink)).await;

        {
            let guard = state.lock().await;
            assert!(guard.has_pending(&ControlNoticeKey {
                run_id: run_id.clone(),
                kind: ControlNoticeKind::NeedsAttention,
                notification_key: format!("{run_id}:needs_attention:idle"),
            }));
        }

        // Re-ping partway through the first window — this must abort/replace the first timer.
        tokio::time::advance(Duration::from_millis(20)).await;
        let ev2 = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Foreground);
        ControlNoticeState::handle(&state, ev2, Arc::clone(&sink)).await;

        // t = 65 ms: 45 ms after the SECOND ping, and 5 ms PAST where the first timer would have
        // fired (60 ms after the first ping). On the virtual clock this is exact, so the assertion
        // below is a statement about the coalescing, not about scheduler luck.
        tokio::time::advance(Duration::from_millis(45)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            sink.count(),
            0,
            "the original timer must have been aborted, not allowed to fire independently"
        );

        // t = 105 ms, past the second timer's own 80 ms deadline.
        tokio::time::advance(Duration::from_millis(40)).await;
        // The delivery runs in a spawned task; yield until it has been polled to completion.
        for _ in 0..8 {
            if sink.count() > 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(sink.count(), 1, "the re-armed (coalesced) timer must still eventually fire");
    }

    #[tokio::test]
    async fn was_delivered_reports_true_only_after_an_actual_delivery() {
        let state = Arc::new(AsyncMutex::new(ControlNoticeState::new()));
        let sink = Arc::new(RecordingSink::default());
        let run_id = RunId::new();
        let key = ControlNoticeKey {
            run_id: run_id.clone(),
            kind: ControlNoticeKind::NeedsAttention,
            notification_key: format!("{run_id}:needs_attention:idle"),
        };

        assert!(!state.lock().await.was_delivered(&key));

        state
            .observe_run_locked(run_id.clone(), actionable_view("scout", 0))
            .await;
        let ev = needs_attention_notice(&run_id, Some("scout"), Some(0), RunSource::Async);
        ControlNoticeState::handle(&state, ev, Arc::clone(&sink)).await;

        assert!(state.lock().await.was_delivered(&key));
    }

    // ---------------------------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------------------------

    /// Small extension trait giving tests a terse way to lock-and-mutate the shared state without
    /// repeating `state.lock().await.observe_run(...)` at every call site.
    #[async_trait::async_trait]
    trait SharedStateTestExt {
        async fn observe_run_locked(&self, run_id: RunId, view: LiveRunView);
        async fn forget_run_locked(&self, run_id: &RunId);
    }

    #[async_trait::async_trait]
    impl SharedStateTestExt for Arc<AsyncMutex<ControlNoticeState>> {
        async fn observe_run_locked(&self, run_id: RunId, view: LiveRunView) {
            self.lock().await.observe_run(run_id, view);
        }

        async fn forget_run_locked(&self, run_id: &RunId) {
            self.lock().await.forget_run(run_id);
        }
    }

    /// Waits comfortably past every test's short (20-60ms) debounce window used above, so a
    /// fired timer's async delivery (itself just a synchronous sink call once the timer's own
    /// lock is acquired) has had every opportunity to run.
    async fn wait_past_debounce() {
        tokio::time::sleep(Duration::from_millis(120)).await;
    }

    /// Polls briefly for at least one delivery to land, for tests that only assert on the
    /// content of a delivery rather than its absence (where a fixed sleep would otherwise be
    /// either flaky-short or needlessly long).
    async fn wait_for_delivery(sink: &RecordingSink) {
        for _ in 0..50 {
            if sink.count() > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Guards against `AtomicUsize`/`Ordering` import drift if a future edit removes the last use
    /// of the recording-sink counter pattern above in favor of the `RecordingSink` struct's own
    /// `Vec`-based bookkeeping — kept as a trivial compiling use so the import is never dead code
    /// mid-refactor.
    #[test]
    fn atomic_usize_import_is_available_for_future_counter_based_assertions() {
        let counter = AtomicUsize::new(0);
        counter.fetch_add(1, Ordering::SeqCst);
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}
