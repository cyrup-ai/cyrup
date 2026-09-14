//! The live completion-watcher lifecycle: install a real `notify::PollWatcher` over `ResultsDir`,
//! drain it on every wake-up, and run the scan → observe → format → deliver → delete-last pipeline
//! (R-SA-099's delete-last, R-SA-102's retry-in-place). Dropping the returned handle tears the
//! watch down. Split out of `background/watch.rs`; ports pi `runs/background/result-watcher.ts` +
//! `notify.ts`.

use super::message::{
    format_completion_message, format_missing_payload_message, format_undeliverable_message,
};
use super::observer::CompletionObserver;
use super::results_watcher::{
    CompletionBand, CompletionNotification, RESULTS_DIR_POLL_INTERVAL, ResolvedCandidate,
    ResultsWatcher,
};
use super::sink::CompletionSink;
use crate::background::delivery::{Attribution, CompletionDelivery, ResultDeliveryOwnership};
use crate::background::result_index::{self, ConsumablePayload};
use crate::background::{ResultFile, RunId};
use crate::error::SubagentError;
use crate::identity::SessionId;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A live completion-watcher: keeps a real `notify::PollWatcher` over `ResultsDir` alive and a
/// background task draining it. Dropping this handle stops both (the poll-watcher is released and
/// the drain task is aborted), so a session's watcher is torn down cleanly when the extension
/// replaces or forgets it.
pub struct CompletionWatcherHandle {
    /// Held only to keep the underlying filesystem watch alive — dropped (stopping the watch) when
    /// this handle is dropped.
    _poll_watcher: notify::PollWatcher,
    /// The background drain task; aborted on drop.
    task: tokio::task::JoinHandle<()>,
}

impl Drop for CompletionWatcherHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Install a live completion watcher over `results_dir` (C6; pi `result-watcher.ts` +
/// `notify.ts`): construct a [`ResultsWatcher`], attach its real `notify::PollWatcher`
/// ([`ResultsWatcher::install`]), and spawn a background task that on every filesystem wake-up (and
/// once immediately, priming any results already on disk) scans for freshly-completed runs, formats
/// each into a [`crate::background::watch::CompletionMessage`], delivers it via `sink`, and — only after successful delivery —
/// deletes the result file (R-SA-099's delete-last; a failed delivery leaves the file for
/// retry-in-place, R-SA-102). Returns a [`CompletionWatcherHandle`] the caller MUST retain for the
/// watch to stay live.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] if the underlying `notify::PollWatcher` cannot be attached to
/// `results_dir` (e.g. it does not exist — the caller must `mkdir` it first, mirroring how
/// `AsyncRoot`/`ResultsDir` are established by extension initialization).
pub fn install_completion_watcher(
    results_dir: PathBuf,
    sink: Arc<dyn CompletionSink>,
    ownership: ResultDeliveryOwnership,
) -> Result<CompletionWatcherHandle, SubagentError> {
    install_completion_watcher_with_observer(results_dir, sink, None, ownership)
}

/// [`install_completion_watcher`] with an additional [`CompletionObserver`].
///
/// # Errors
///
/// As [`install_completion_watcher`].
pub fn install_completion_watcher_with_observer(
    results_dir: PathBuf,
    sink: Arc<dyn CompletionSink>,
    observer: Option<Arc<dyn CompletionObserver>>,
    ownership: ResultDeliveryOwnership,
) -> Result<CompletionWatcherHandle, SubagentError> {
    let watcher = ResultsWatcher::new(results_dir);
    let (poll_watcher, rx) = watcher.install()?;
    let task = tokio::spawn(drive_completion_watcher(
        watcher, rx, sink, observer, ownership,
    ));
    Ok(CompletionWatcherHandle {
        _poll_watcher: poll_watcher,
        task,
    })
}

/// The deliveries this drain loop has spawned and not yet reaped.
///
/// The serial `.await` this replaces was the defect (`ASYNC_NOTIFY_BUG_REPORT` RC1): the
/// production sink's ack resolves only when the orchestrator session goes fully idle, so one
/// busy-session ack parked OBSERVATION of every later completion — an in-flight `wait` could
/// never be told about a run that finished while the drain sat inside `sink.deliver`.
struct DeliveryFleet {
    /// The spawned, not-yet-reaped deliveries. Drained by [`reap_finished_deliveries`] around
    /// every scan, and by [`drive_completion_watcher`]'s own `select!` arm the moment an ack
    /// lands mid-sleep.
    tasks: tokio::task::JoinSet<DeliveryOutcome>,
    /// Run ids whose ack is still pending. Guards against a rescan spawning a SECOND injection
    /// for the same payload — impossible before this split only because the loop blocked, and
    /// newly possible because `evict_expired` drops the seen entry after
    /// [`super::results_watcher::DEDUP_TTL`] (10 min), which a parked ack can easily outlive.
    ///
    /// Behind an `Arc<Mutex<..>>` so the spawned task can hold an RAII guard over its own entry:
    /// a task that is aborted (watcher torn down) or panics still runs its locals' `Drop`, so the
    /// set cannot leak an id and permanently silence a run.
    in_flight: Arc<Mutex<HashSet<RunId>>>,
}

/// One reaped delivery, carrying back everything consumption or retry needs.
struct DeliveryOutcome {
    /// The run the delivery was for.
    run_id: RunId,
    /// The entitlement to destroy the payload. `Some` for an owned completion; `None` for the
    /// `scan.missing` band, whose payload is already gone — on `Delivered` the join site then
    /// retires the dangling index entries instead of consuming.
    payload: Option<ConsumablePayload>,
    /// Needed only to rebuild the [`ResolvedCandidate`] `record_processing_failure` takes when an
    /// owned delivery is deferred. `None` for the missing band, which has no result file left.
    result: Option<ResultFile>,
    /// Whether the completion was surfaced as exhausted (R-SA-102's retry bound).
    exhausted: bool,
    /// The scan-time session identity, carried so the missing band's join site can still retire
    /// its index entries (an owned band's payload carries its own session and ignores this).
    session_id: Option<SessionId>,
    /// What became of the message.
    delivery: CompletionDelivery,
}

/// Removes its run id from `in_flight` on drop — including on unwind and on `JoinSet` abort.
struct InFlightGuard {
    /// The fleet's shared in-flight set.
    set: Arc<Mutex<HashSet<RunId>>>,
    /// The id this guard releases.
    run_id: RunId,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.run_id);
        }
    }
}

/// Reap every delivery whose ack has landed. Never blocks on one still pending.
async fn reap_finished_deliveries(watcher: &ResultsWatcher, fleet: &mut DeliveryFleet) {
    while let Some(joined) = fleet.tasks.try_join_next() {
        let Ok(outcome) = joined else {
            // A `JoinError` (runtime abort / panic — the body is one `.await`, so this should be
            // unreachable) loses the payload handle: the file stays on disk untouched and the
            // seen entry expires at `DEDUP_TTL`, after which the next scan re-surfaces it. The
            // `InFlightGuard` has already released the id. Degrade, never destroy.
            continue;
        };
        settle_delivery(watcher, outcome).await;
    }
}

/// Settle one reaped delivery, preserving both custody rules verbatim: consumption happens ONLY
/// against the receipt the delivery produced (R-SA-099's delete-last), and a deferred delivery
/// retries in place with the existing attempt bound (R-SA-102). Shared by
/// [`reap_finished_deliveries`] and [`drive_completion_watcher`]'s `join_next` arm, so neither
/// site can drop a real outcome on the floor.
async fn settle_delivery(watcher: &ResultsWatcher, outcome: DeliveryOutcome) {
    match outcome.delivery {
        CompletionDelivery::Delivered(receipt) => {
            debug_assert_eq!(receipt.run_id(), &outcome.run_id);
            match outcome.payload {
                // R-SA-099 delete-last, authorised by payload + receipt together (`consume`
                // re-asserts their agreement itself).
                Some(payload) => {
                    let _ = watcher.consume(payload, receipt).await;
                }
                // The missing band: nothing to consume; retire the dangling index entries so the
                // loss is not re-announced on every later scan.
                None => {
                    if let Some(session_id) = outcome.session_id.as_ref() {
                        watcher.retire(session_id, &outcome.run_id, 0).await;
                    }
                }
            }
        }
        CompletionDelivery::Deferred => {
            // R-SA-102 retry-in-place, including the MAX_PROCESSING_ATTEMPTS →
            // `format_undeliverable_message` escalation, which is enforced on the read side by
            // `check_and_mark_seen`. The missing band has nothing to retry against — its payload
            // is already gone, and the missing-streak counter re-surfaces the loss by itself.
            if let (Some(result), Some(payload)) = (outcome.result, outcome.payload) {
                watcher
                    .record_processing_failure(&ResolvedCandidate {
                        result,
                        payload,
                        async_dir: None,
                        exhausted: outcome.exhausted,
                    })
                    .await;
            }
        }
    }
}

/// The background drain loop [`install_completion_watcher`] spawns: prime once (so results already
/// on disk at install time are delivered without waiting for a filesystem event), then
/// deliver-on-every-wake-up until the watch is dropped (the channel closes and `recv` yields
/// `None`).
async fn drive_completion_watcher(
    watcher: ResultsWatcher,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<()>,
    sink: Arc<dyn CompletionSink>,
    observer: Option<Arc<dyn CompletionObserver>>,
    ownership: ResultDeliveryOwnership,
) {
    let mut fleet = DeliveryFleet {
        tasks: tokio::task::JoinSet::new(),
        in_flight: Arc::new(Mutex::new(HashSet::new())),
    };
    deliver_pending_completions(&watcher, &sink, observer.as_ref(), &ownership, &mut fleet).await;
    reap_finished_deliveries(&watcher, &mut fleet).await;
    // A filesystem wake-up OR a fixed tick, whichever comes first.
    //
    // The tick is not redundant with the watch, and leaving it out cost a whole class of
    // completions: `notify::PollWatcher` reports CHANGES, so a directory that goes quiet produces
    // no more wake-ups — and the state this subsystem must react to is precisely a quiet one. A
    // payload consumed by another process changes nothing further; its index entry then sits
    // there, unresolvable, with no event to prompt anyone to look again. Scanning on a cadence is
    // what lets the missing-payload grace window elapse at all, and it is R-SA-098's own
    // "fixed-interval poll fallback" applied to the drain rather than only to the watch.
    let mut tick = tokio::time::interval(RESULTS_DIR_POLL_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            wake = rx.recv() => {
                if wake.is_none() {
                    return;  // the watch was dropped
                }
            }
            _ = tick.tick() => {}
            // An ack that resolves between ticks is settled ON THE SPOT instead of up to a poll
            // interval later. The `if` guard is load-bearing: `join_next()` on an EMPTY `JoinSet`
            // returns `None` immediately, which would resolve this arm on every iteration and
            // spin the loop at 100% CPU. The outcome is BOUND and fed to the same handler the
            // reap uses, so a real outcome is settled, never dropped on the floor.
            Some(joined) = fleet.tasks.join_next(), if !fleet.tasks.is_empty() => {
                if let Ok(outcome) = joined {
                    settle_delivery(&watcher, outcome).await;
                }
                // A `JoinError` here is the same degrade-never-destroy case documented on
                // `reap_finished_deliveries`: the payload stays on disk for a later scan.
            }
        }
        reap_finished_deliveries(&watcher, &mut fleet).await;
        deliver_pending_completions(&watcher, &sink, observer.as_ref(), &ownership, &mut fleet)
            .await;
        reap_finished_deliveries(&watcher, &mut fleet).await;
    }
}

/// Scan once and walk the chain of custody for everything the scan turned up.
///
/// # Three piles, three treatments, and no predicate to forget
///
/// `results_dir` is shared by every cyrup instance in a directory, so "found a result" and "may
/// consume this result" are different questions. They used to be answered by one list plus two
/// boolean accessors consulted afterwards, which left the destructive operation reachable for any
/// completion the loop happened to be holding.
///
/// Now the answer travels in the type. [`Attribution::classify`] hands each own-partition
/// candidate to exactly one arm:
///
/// * [`Attribution::Unattributed`] — no recorded session, so nobody's. pi returns at
///   `result-watcher.ts:408`, **before** the observer band, so nothing at all happens.
/// * [`Attribution::Foreign`] — a live session that is not ours. Observers run (`:415`/`:421`)
///   and the mission observer index is retired (`:425`), then `:428` returns. The payload is left
///   on disk because the instance that owns it still needs it — and this arm receives an
///   [`crate::background::delivery::ObservableCompletion`], which carries nothing `consume` will
///   accept, so that is a property of the code rather than of the comment.
/// * [`Attribution::Ours`] — observe, deliver, and consume ONLY against the receipt the delivery
///   produced (R-SA-099's delete-last; a deferred delivery leaves the payload for retry-in-place,
///   R-SA-102).
///
/// A fourth pile — completions whose payload is gone — is reported rather than skipped, which is
/// the whole of `scan.missing`.
///
/// # One snapshot per scan
///
/// [`ResultDeliveryOwnership::snapshot`] is taken once, before the loop. A session change midway
/// through would otherwise judge some results against one identity and the rest against another.
///
/// # Observation is unconditional; delivery is concurrent; consumption is still receipt-only
///
/// The `Ours` arm used to run `sink.deliver(..).await` INLINE in this loop, and that serial await
/// was the defect (`ASYNC_NOTIFY_BUG_REPORT` RC1): the production sink's ack resolves only when
/// the orchestrator session goes fully idle, so one completion delivered into a busy session
/// parked the entire scan → observe → deliver pipeline for the rest of the turn — later
/// completions were neither observed (no bus publish, so an in-flight `wait` could never surface
/// them inline) nor delivered. The arm is now split: Phase 1 OBSERVES synchronously on every
/// scan, and Phase 2 spawns the delivery onto `fleet`, whose acks the drain loop reaps
/// ([`reap_finished_deliveries`] / [`settle_delivery`]) — where consumption still happens ONLY
/// against the `DeliveryReceipt` the delivery produced (R-SA-099's delete-last), and a deferral
/// still retries in place (R-SA-102). The fleet's `in_flight` set is what keeps a rescan from
/// spawning a second injection for a payload whose ack outlives the seen-set's `DEDUP_TTL`.
async fn deliver_pending_completions(
    watcher: &ResultsWatcher,
    sink: &Arc<dyn CompletionSink>,
    observer: Option<&Arc<dyn CompletionObserver>>,
    ownership: &ResultDeliveryOwnership,
    fleet: &mut DeliveryFleet,
) {
    let snapshot = ownership.snapshot();
    let sessions = snapshot.readable_sessions();
    let Ok(scan) = watcher.scan_candidates(&sessions, &[]).await else {
        return;
    };

    // 1. Completions reached through a cross-session band. They are OBSERVED and nothing else:
    //    the scan hands them over without an entitlement, so there is no value here capable of
    //    destroying a payload whose owner still needs it.
    for observed in scan.observed {
        let notification = CompletionNotification {
            result: observed.result,
            result_path: observed.path,
            exhausted: false,
            band: CompletionBand::Observed,
        };
        let observed_cleanly = match observer {
            Some(observer) => observer.observe(&notification).await,
            None => true,
        };
        // pi `:425` — retire the cross-session obligation only once every observer ran cleanly; a
        // failed observer needs this entry to resurface the completion.
        if observed_cleanly {
            result_index::remove_mission_observer_index(
                watcher.results_dir(),
                &notification.result.run_id,
            )
            .await;
        }
    }

    // 2. Candidates from this instance's own partition, classified into the three bands. The
    //    `match` cannot answer "may I consume this?" by omission: each arm receives a different
    //    type, and only one of them carries anything `consume` accepts.
    for candidate in scan.resolved {
        let ResolvedCandidate {
            result,
            payload,
            exhausted,
            ..
        } = candidate;
        match Attribution::classify(result, payload, &snapshot) {
            // pi `result-watcher.ts:408` returns BEFORE the observer band: nothing at all happens.
            Attribution::Unattributed(_) => {}
            Attribution::Foreign(foreign) => {
                let notification = CompletionNotification {
                    result: foreign.result().clone(),
                    result_path: PathBuf::new(),
                    exhausted: false,
                    band: CompletionBand::Foreign,
                };
                let observed_cleanly = match observer {
                    Some(observer) => observer.observe(&notification).await,
                    None => true,
                };
                if observed_cleanly {
                    result_index::remove_mission_observer_index(
                        watcher.results_dir(),
                        foreign.run_id(),
                    )
                    .await;
                }
            }
            Attribution::Ours(owned) => {
                // ---- Phase 1: SYNCHRONOUS observation. Runs on every scan, never behind a
                // delivery ack — this publish is the edge `wait` selects on, and starving it was
                // `ASYNC_NOTIFY_BUG_REPORT` RC1.
                let notification = CompletionNotification {
                    result: owned.result().clone(),
                    result_path: PathBuf::new(),
                    exhausted,
                    band: CompletionBand::Ours,
                };
                if let Some(observer) = observer {
                    observer.observe(&notification).await;
                }
                result_index::remove_mission_observer_index(watcher.results_dir(), owned.run_id())
                    .await;

                // ---- Phase 2: CONCURRENT delivery. The ack can be parked for a whole turn, so
                // it must never park the scan; the fleet's join sites settle it (consume on
                // receipt, retry-in-place on deferral).
                let run_id = owned.run_id().clone();
                let claimed = match fleet.in_flight.lock() {
                    Ok(mut set) => set.insert(run_id.clone()),
                    // A poisoned lock means another thread panicked mid-update; not spawning is
                    // the safe reading (the payload stays on disk and the next scan retries).
                    Err(_) => false,
                };
                if !claimed {
                    continue; // its ack is still pending; the payload stays exactly where it is
                }
                // A completion the sink has already refused `MAX_PROCESSING_ATTEMPTS` times is not
                // offered again: it is announced as a loss, so a permanently un-injectable result
                // still reaches the orchestrator instead of being dropped here.
                let message = if exhausted {
                    format_undeliverable_message(owned.result())
                } else {
                    format_completion_message(owned.result())
                };
                let result = owned.result().clone();
                let payload = owned.into_payload();
                let sink = Arc::clone(sink);
                let guard = InFlightGuard {
                    set: Arc::clone(&fleet.in_flight),
                    run_id: run_id.clone(),
                };
                fleet.tasks.spawn(async move {
                    let _guard = guard;
                    let delivery = sink.deliver(&run_id, message).await;
                    DeliveryOutcome {
                        run_id,
                        payload: Some(payload),
                        result: Some(result),
                        exhausted,
                        session_id: None,
                        delivery,
                    }
                });
            }
        }
    }

    // 3. Runs that finished and whose payload is gone. Announced with whatever their own records
    //    still hold, then their dangling index entries are retired so the candidate list stays
    //    bounded. Silence is not an option here: it is exactly what happened nineteen times.
    //    Joins the same fleet as the owned band for uniformity (`ASYNC_NOTIFY_BUG_REPORT` F1.5) —
    //    a parked ack must not park the scan — with the same in-flight guard so a rescan cannot
    //    spawn a second announcement while one is pending.
    for report in scan.missing {
        let run_id = report.run_id.clone();
        let claimed = match fleet.in_flight.lock() {
            Ok(mut set) => set.insert(run_id.clone()),
            Err(_) => false,
        };
        if !claimed {
            continue;
        }
        let message = format_missing_payload_message(&report);
        let session_id = snapshot.current_session().cloned();
        let sink = Arc::clone(sink);
        let guard = InFlightGuard {
            set: Arc::clone(&fleet.in_flight),
            run_id: run_id.clone(),
        };
        fleet.tasks.spawn(async move {
            let _guard = guard;
            let delivery = sink.deliver(&run_id, message).await;
            DeliveryOutcome {
                run_id,
                payload: None,
                result: None,
                exhausted: false,
                session_id,
                delivery,
            }
        });
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::super::message::CompletionMessage;
    use super::super::tests::{
        child_result, owning, publish_result, published_path, result_with_children,
        temp_results_dir,
    };
    use super::*;
    use crate::background::RunState;
    use std::time::{Duration, Instant};
    use tokio::sync::Mutex as AsyncMutex;

    /// A capturing [`CompletionSink`] for tests: records every delivered message and reports
    /// delivered.
    #[derive(Clone, Default)]
    struct CapturingSink {
        delivered: Arc<AsyncMutex<Vec<CompletionMessage>>>,
    }

    #[async_trait::async_trait]
    impl CompletionSink for CapturingSink {
        async fn deliver(
            &self,
            run_id: &crate::background::RunId,
            message: CompletionMessage,
        ) -> CompletionDelivery {
            self.delivered.lock().await.push(message);
            CompletionDelivery::delivered(run_id.clone())
        }
    }

    /// A [`CompletionSink`] that inspects the filesystem from INSIDE `deliver` — the one moment
    /// strictly between "the completion was recorded" and "its payload was unlinked".
    ///
    /// The record must already exist when `deliver` is entered, because the delivery's receipt is
    /// what authorises `settle_delivery`'s `watcher.consume(payload, receipt)` (`install.rs`'s
    /// delete-last). A sink positioned here is the only observer between the two, and it is what
    /// makes the ordering test FAIL for a record-after-delivery placement that "records the
    /// completion" just as truthfully.
    struct OrderingSink {
        results_dir: PathBuf,
        replay_existed_at_delivery: Arc<std::sync::atomic::AtomicBool>,
        payload_existed_at_delivery: Arc<std::sync::atomic::AtomicBool>,
        payload_path: PathBuf,
    }

    #[async_trait::async_trait]
    impl CompletionSink for OrderingSink {
        async fn deliver(
            &self,
            run_id: &crate::background::RunId,
            _message: CompletionMessage,
        ) -> CompletionDelivery {
            use std::sync::atomic::Ordering;
            self.replay_existed_at_delivery.store(
                crate::background::completion_replay::completion_replay_path(
                    &self.results_dir,
                    run_id,
                )
                .exists(),
                Ordering::SeqCst,
            );
            self.payload_existed_at_delivery
                .store(self.payload_path.exists(), Ordering::SeqCst);
            CompletionDelivery::delivered(run_id.clone())
        }
    }

    /// SUBA-056's ordering law, ported verbatim from pi `result-watcher.ts:432-433`:
    ///
    /// > Recorded before dedupe and before the unlink below so bg_wait can use the in-memory record
    /// > or its bounded durable replay after cleanup.
    ///
    /// This asserts ORDERING, not presence. Presence alone passes with the record written after
    /// delivery, which is exactly the refactor the architecture would still let compile: observation
    /// is Phase 1 and synchronous, the delivery is Phase 2 and spawned, and moving the persist into
    /// the spawned task would still "record the completion" — just after the payload it exists to
    /// outlive is already gone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_replay_record_is_written_before_the_payload_is_deleted() {
        use crate::background::wait_completions::WaitCompletionStore;
        use std::sync::atomic::{AtomicBool, Ordering};

        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir results_dir");

        let result = result_with_children(
            "run-replay-order",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some("the answer"), 0)],
        );
        let payload_path = published_path(&results_dir, &result);
        publish_result(&results_dir, &result).await;

        let replay_seen = Arc::new(AtomicBool::new(false));
        let payload_seen = Arc::new(AtomicBool::new(false));
        let sink = OrderingSink {
            results_dir: results_dir.clone(),
            replay_existed_at_delivery: Arc::clone(&replay_seen),
            payload_existed_at_delivery: Arc::clone(&payload_seen),
            payload_path: payload_path.clone(),
        };

        // The real observer seam, with its durable mirror armed exactly as
        // `SubagentExecutor::install_completion_watcher` arms it.
        let store = Arc::new(WaitCompletionStore::default());
        store.set_replay_dir(results_dir.clone());
        let handle = install_completion_watcher_with_observer(
            results_dir.clone(),
            Arc::new(sink),
            Some(Arc::clone(&store) as Arc<dyn CompletionObserver>),
            owning(),
        )
        .expect("watcher installs");

        // The unlink now lives in `settle_delivery`, reached only once the spawned delivery task
        // joins — so poll for the payload's ABSENCE rather than assuming it is synchronous with
        // `deliver`.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && payload_path.exists() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(
            !payload_path.exists(),
            "the payload must be consumed after delivery (R-SA-099 delete-last)"
        );

        assert!(
            payload_seen.load(Ordering::SeqCst),
            "sanity: the sink runs BEFORE the unlink, or this test proves nothing"
        );
        assert!(
            replay_seen.load(Ordering::SeqCst),
            "the durable replay record must already exist when the sink is entered — \
             recorded before dedupe and before the unlink (`result-watcher.ts:432-433`)"
        );

        drop(handle);
    }

    /// THE acceptance test for this change: two cyrup instances, one shared results directory.
    ///
    /// Reproduces the reported defect — runs launched by pid 488520 were consumed and deleted by
    /// another instance in the same cwd — and asserts the fix end to end through the real drain
    /// loop and a real `notify::PollWatcher`, not through the pure classifier.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_instances_sharing_a_directory_each_receive_only_their_own_completions() {
        use crate::background::delivery::ResultDeliveryOwnership;
        use crate::identity::{CompletionOwnerId, SessionId};

        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir results_dir");

        let session_a = SessionId::parse("session-A").expect("non-empty");
        let session_b = SessionId::parse("session-B").expect("non-empty");
        let owner_a = CompletionOwnerId::parse("proc-A").expect("non-empty");
        let owner_b = CompletionOwnerId::parse("proc-B").expect("non-empty");

        // Instance A launched run-a; instance B launched run-b. Both land in the SAME directory.
        for (run, session, owner, output) in [
            ("run-a", &session_a, &owner_a, "OUTPUT-FROM-A"),
            ("run-b", &session_b, &owner_b, "OUTPUT-FROM-B"),
        ] {
            let mut result = result_with_children(
                run,
                RunState::Complete,
                true,
                None,
                vec![child_result("worker", Some(output), 0)],
            );
            result.session_id = Some(session.clone());
            result.completion_owner_id = Some(owner.clone());
            publish_result(&results_dir, &result).await;
        }
        let payload_a = crate::background::result_index::owned_payload_path(
            &results_dir,
            &session_a,
            &crate::background::RunId::from_token("run-a"),
        );
        let payload_b = crate::background::result_index::owned_payload_path(
            &results_dir,
            &session_b,
            &crate::background::RunId::from_token("run-b"),
        );
        assert!(payload_a.exists());
        assert!(payload_b.exists());

        // Instance B drains. It must take ONLY run-b.
        let sink_b = CapturingSink::default();
        let delivered_b = Arc::clone(&sink_b.delivered);
        let handle_b = install_completion_watcher(
            results_dir.clone(),
            Arc::new(sink_b),
            ResultDeliveryOwnership::new(Some(session_b), Some(owner_b)),
        )
        .expect("B installs");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && delivered_b.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Give B a further beat to (incorrectly) take a second one, if it were going to.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let b_messages = delivered_b.lock().await.clone();
        assert_eq!(b_messages.len(), 1, "B delivered {b_messages:?}");
        assert!(
            b_messages[0].content.contains("OUTPUT-FROM-B"),
            "B must receive its OWN completion, got {:?}",
            b_messages[0].content
        );
        assert!(
            !b_messages[0].content.contains("OUTPUT-FROM-A"),
            "B must never receive A's completion"
        );

        // A's payload is UNTOUCHED — the defect was that it got consumed and deleted.
        assert!(
            payload_a.exists(),
            "A's result must survive B's drain; destroying it is the reported bug"
        );
        assert!(
            !payload_b.exists(),
            "B's own result is consumed after successful delivery (R-SA-099 delete-last)"
        );
        drop(handle_b);

        // Now A drains and still finds its own result waiting.
        let sink_a = CapturingSink::default();
        let delivered_a = Arc::clone(&sink_a.delivered);
        let handle_a = install_completion_watcher(
            results_dir.clone(),
            Arc::new(sink_a),
            ResultDeliveryOwnership::new(Some(session_a), Some(owner_a)),
        )
        .expect("A installs");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && delivered_a.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let a_messages = delivered_a.lock().await.clone();
        assert_eq!(a_messages.len(), 1, "A delivered {a_messages:?}");
        assert!(
            a_messages[0].content.contains("OUTPUT-FROM-A"),
            "A finally receives the completion B did not steal, got {:?}",
            a_messages[0].content
        );
        drop(handle_a);
    }

    /// The dangling-index repair, end to end through the real drain loop: a completed run whose
    /// payload was taken by an index-blind process must reach the orchestrator as a LOUD
    /// notification carrying what the run's own records still hold, and must leave no index
    /// entries behind to be re-resolved forever.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stolen_payload_is_announced_and_its_dangling_index_retired() {
        use crate::background::result_index;

        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");

        let result = result_with_children(
            "run-stolen",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some("the answer"), 0)],
        );
        publish_result(&results_dir, &result).await;
        let payload = published_path(&results_dir, &result);

        // An index-blind sibling consumes the payload and leaves the index untouched — the exact
        // residue this repair exists for.
        tokio::fs::remove_file(&payload)
            .await
            .expect("steal the payload");
        let session = result.session_id.clone().expect("fixture session");
        assert!(
            result_index::result_candidates_for_session(&results_dir, &session)
                .await
                .expect("enumerate")
                .iter()
                .any(|c| c.run_id == result.run_id),
            "precondition: the dangling index entry is still a candidate"
        );

        let sink = CapturingSink::default();
        let delivered = Arc::clone(&sink.delivered);
        let handle = install_completion_watcher(results_dir.clone(), Arc::new(sink), owning())
            .expect("watcher installs");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && delivered.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let messages = delivered.lock().await.clone();
        assert_eq!(
            messages.len(),
            1,
            "exactly one loss notification: {messages:?}"
        );
        assert!(
            messages[0].display,
            "a lost result is never delivered silently"
        );
        assert!(
            messages[0].content.contains("removed before delivery"),
            "{}",
            messages[0].content
        );

        // The dangling entries are gone, so the candidate list stays bounded and the loss is not
        // re-announced on every later scan.
        assert!(
            result_index::result_candidates_for_session(&results_dir, &session)
                .await
                .expect("enumerate")
                .is_empty(),
            "the dangling index trio must be retired"
        );
        drop(handle);
    }

    /// The load-bearing C6 test: a completing background run fires EXACTLY ONE notify and its
    /// result file is deleted. Uses the real `notify::PollWatcher` install + drain pipeline, a
    /// capturing sink, and a real on-disk result file.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_completion_watcher_fires_exactly_one_notify_and_deletes_the_result() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir results_dir");

        let sink = CapturingSink::default();
        let delivered = Arc::clone(&sink.delivered);
        let handle = install_completion_watcher(results_dir.clone(), Arc::new(sink), owning())
            .expect("watcher installs");

        // A completing background run writes its terminal ResultFile into ResultsDir (the runner's
        // last file-writing act, R-SA-077).
        let result = result_with_children(
            "run-notify-1",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", Some("all done"), 0)],
        );
        let result_path = published_path(&results_dir, &result);
        publish_result(&results_dir, &result).await;

        // Wait for the watcher to fire and delete the file (bounded).
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let count = delivered.lock().await.len();
            let gone = !result_path.exists();
            if count >= 1 && gone {
                break;
            }
            if Instant::now() >= deadline {
                panic!("watcher did not fire+delete in time: delivered={count}, file_gone={gone}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Let any duplicate poll ticks / filesystem events settle, then assert EXACTLY ONE notify.
        tokio::time::sleep(Duration::from_millis(750)).await;
        let messages = delivered.lock().await;
        assert_eq!(
            messages.len(),
            1,
            "a completing background run must fire exactly one notify, got: {messages:?}"
        );
        assert_eq!(messages[0].custom_type, "subagent-notify");
        assert!(
            messages[0].trigger_turn,
            "the notify must trigger a turn (R-SA-101)"
        );
        assert!(
            !messages[0].display,
            "SUBA-090: the `display` handed to the sink for a plain successful completion is false"
        );
        // SCOPE_17 — every child now gets an `{agent}:` prefix (the fix for §2's divergence #1),
        // including a single-child run.
        assert_eq!(
            messages[0].content,
            "Background task completed: **worker**\n\nworker:\nall done"
        );
        assert!(
            !result_path.exists(),
            "the result file must be deleted after notify"
        );

        drop(handle);
    }
}
