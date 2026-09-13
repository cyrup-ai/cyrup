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
