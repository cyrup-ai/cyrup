//! [`WaitCompletionStore`] — the in-process record of terminal payloads this process has already
//! consumed, wired onto the completion-watcher's own observer fan-out, and the durable
//! completion-replay mirror written beside it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::background::RunId;
use crate::background::completion_replay::{CompletionReplayWrite, write_completion_replay};
use crate::background::watch::{
    CompletionBand, CompletionNotification, CompletionObserver, DEDUP_TTL,
};
use crate::identity::SessionId;

use super::project::{WaitCompletion, to_wait_completion};

/// pi's `persistence?: { resultsDir, sessionId }` (`wait-completions.ts:130`) — where the durable
/// mirror of an in-process record is written, and whose it is.
///
/// `session_id` is a required [`SessionId`], not an `Option`: upstream's only call site
/// (`result-watcher.ts:434-437`) has already passed `:411-412`'s session-presence gate, and a
/// record without one is unreadable by construction
/// ([`crate::background::completion_replay::CompletionReplayRecord`]). Expressing the gate as
/// `Option<&ReplayPersistence>` at the CALLER rather than `Option<SessionId>` inside the record is
/// what keeps "we cannot attribute this" and "we can, here it is" two different shapes.
pub struct ReplayPersistence<'a> {
    /// The per-cwd results directory the replay and archive files live under.
    pub results_dir: &'a Path,
    /// The session that owns the completion — the durable record's OPTIONAL read filter.
    pub session_id: &'a SessionId,
}

/// The in-process record of terminal payloads this process has already consumed.
///
/// pi `SubagentState.completedResults: Map<runId, {seenAt, completion}>` (`shared/types.ts:2260`),
/// written by `recordWaitCompletion` (`wait-completions.ts:124-152`) from the result watcher
/// (`result-watcher.ts:434-437`).
///
/// # Why this is load-bearing and not a cache
///
/// The completion watcher deletes a payload once its delivery receipt lands
/// (`watch/install.rs`'s delete-last). A `wait` resolving immediately afterwards has no file to
/// read, and without this record reports a completion it demonstrably observed as absent. This map
/// is the only source between the unlink and the durable tier
/// ([`crate::background::completion_replay`]), which this type also writes.
///
/// A [`std::sync::Mutex`] with poison recovery — this crate's convention for short non-`await`
/// critical sections (`runner_main/status.rs`'s `lock_status`, which states the rule and names
/// `background/tracker.rs` as its precedent): `.lock().unwrap_or_else(PoisonError::into_inner)`,
/// never `.unwrap()` (the crate denies `clippy::unwrap_used`). No guard is ever held across an
/// `.await`.
#[derive(Debug, Default)]
pub struct WaitCompletionStore {
    entries: Mutex<HashMap<RunId, RecordedCompletion>>,
    /// Where the durable replay records go — `None` until a session start supplies it.
    ///
    /// Interior-mutable and OVERWRITABLE, not a `OnceLock`: this store is built in
    /// `SubagentExecutor::new`, long before any cwd is known, and
    /// `SubagentExecutor::install_completion_watcher` re-runs on EVERY `SessionStart` with a
    /// possibly different cwd — a write-once cell would pin the first session's results dir for the
    /// life of the process and quietly mirror a later session's completions into it.
    ///
    /// The alternative shape — carrying `results_dir` on every
    /// [`CompletionNotification`] — was rejected because that value travels a `broadcast`-adjacent
    /// path and is identical for every notification a given watcher emits; a per-notification copy
    /// of a per-watcher constant is the wrong place for it.
    replay_dir: Mutex<Option<PathBuf>>,
}

/// pi's `{ seenAt, completion }` — a named pair, not a tuple: `seen_at` exists only to be compared
/// against the TTL, and a bare `(i64, WaitCompletion)` would leave that unsaid.
#[derive(Clone, Debug)]
struct RecordedCompletion {
    /// Epoch millis at which this payload was consumed — the TTL clock,
    /// [`crate::time::now_epoch_millis`].
    seen_at: i64,
    completion: WaitCompletion,
}

impl WaitCompletionStore {
    /// Clones out the recorded completion for `run_id`, if this process has already consumed one.
    #[must_use]
    pub fn get(&self, run_id: &RunId) -> Option<WaitCompletion> {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .map(|entry| entry.completion.clone())
    }

    /// Point the durable mirror at this session's results directory.
    ///
    /// Called from `SubagentExecutor::install_completion_watcher` with the same `results_dir` the
    /// watcher itself is installed over, so the record lands beside the payload it replaces.
    /// Idempotent and overwriting — see [`Self::replay_dir`]'s own doc for why it must be.
    pub fn set_replay_dir(&self, results_dir: PathBuf) {
        *self
            .replay_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(results_dir);
    }

    /// The directory [`Self::set_replay_dir`] last named, if any.
    #[must_use]
    pub fn replay_dir(&self) -> Option<PathBuf> {
        self.replay_dir
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Record `run_id`'s terminal payload, pruning every entry older than `ttl_ms` first and — when
    /// `persistence` is supplied — mirroring it into the durable replay tier.
    ///
    /// pi `recordWaitCompletion` (`wait-completions.ts:124-152`).
    pub async fn record(
        &self,
        run_id: &RunId,
        data: &serde_json::Value,
        now_ms: i64,
        ttl_ms: i64,
        persistence: Option<&ReplayPersistence<'_>>,
    ) {
        record_wait_completion(self, run_id, data, now_ms, ttl_ms, persistence).await;
    }
}

/// pi `recordWaitCompletion` (`wait-completions.ts:124-152`), the free function
/// [`WaitCompletionStore::record`] calls, in upstream's own order.
async fn record_wait_completion(
    store: &WaitCompletionStore,
    run_id: &RunId,
    data: &serde_json::Value,
    now_ms: i64,
    ttl_ms: i64,
    persistence: Option<&ReplayPersistence<'_>>,
) {
    // 1. Prune. WORKFLOW_4 held ONE guard across the prune and the insert so a reader could not
    //    observe a pruned-but-not-yet-inserted map; step 3's `.await` makes that impossible — a
    //    `std::sync::MutexGuard` must never be held across one — so the acquisition is split. The
    //    only observable difference is exactly the window that argument named, and
    //    `collect_wait_completions` already tolerates it: its ENOENT arm re-checks this store
    //    precisely because the watcher runs on its own task (`collect.rs`'s rung 2).
    {
        let mut entries = store
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries.retain(|_, entry| now_ms.saturating_sub(entry.seen_at) <= ttl_ms);
    }

    // 2. Project. Upstream's throw here lands inside a filesystem-watcher tick where nothing can
    //    catch it; a warn-and-skip is the same observable minus the ability to kill the watcher —
    //    the seam this runs on (`CompletionObserver::observe`) already documents that returning
    //    `false` never aborts the pipeline or blocks delivery, so degrading here is that seam's
    //    own contract, not an invention.
    let completion = match to_wait_completion(data, run_id) {
        Ok(completion) => completion,
        Err(error) => {
            tracing::warn!(run_id = %run_id, %error, "not recording an unprojectable completion");
            return;
        }
    };

    // 3. Persist the durable mirror, pi `result-watcher.ts:432-433`, comment verbatim:
    //
    //      "Recorded before dedupe and before the unlink below so bg_wait can
    //       use the in-memory record or its bounded durable replay after cleanup."
    //
    //    In cyrup that position is STRUCTURAL rather than lexical: `deliver_pending_completions`
    //    runs Phase 1 (this observer) synchronously in the scan loop and only then spawns the
    //    delivery, whose receipt is what finally authorises `settle_delivery`'s
    //    `watcher.consume(payload, receipt)`. The record therefore lands before the unlink by
    //    construction. A test asserts it anyway
    //    (`watch::install::tests::the_replay_record_is_written_before_the_payload_is_deleted`),
    //    because a refactor that moved this persist into the spawned task would still compile and
    //    would still "record the completion".
    //
    //    A persistence failure is LOGGED AND SWALLOWED (pi `wait-completions.ts:147-149` does the
    //    same with `console.error`, then stores the pre-persist projection anyway): degrading to
    //    the in-process tier alone is strictly better than losing both.
    let completion = match persistence {
        None => completion,
        Some(persistence) => {
            let write = CompletionReplayWrite {
                results_dir: persistence.results_dir,
                run_id,
                session_id: persistence.session_id,
                completion: &completion,
                data,
                now: now_ms,
                ttl_ms,
            };
            match write_completion_replay(&write).await {
                // The archive-path-BEARING copy, not the pre-persist projection — otherwise the
                // in-process and durable tiers disagree about `archive_path` and a `wait` sees a
                // different completion depending on which rung answered.
                Ok(record) => record.completion,
                Err(error) => {
                    tracing::warn!(
                        run_id = %run_id,
                        %error,
                        "durable completion replay not written; the in-process record still lands"
                    );
                    completion
                }
            }
        }
    };

    // 4. Insert.
    store
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            run_id.clone(),
            RecordedCompletion {
                seen_at: now_ms,
                completion,
            },
        );
}

/// The store is a [`CompletionObserver`] because that seam fires at pi's exact recording
/// position: `deliver_pending_completions` reaches it AFTER ownership has been confirmed and
/// BEFORE the delivery whose receipt authorises the eventual unlink (pi
/// `result-watcher.ts:432-433`).
///
/// # Registration order is behaviour
///
/// This member MUST precede [`crate::background::watch::CompletionBus`] in the composite. The bus
/// publish is the edge an in-flight `wait` selects on, and a wait that woke on that edge reads
/// this store immediately. Registering the bus first would let a wait wake, look, and find
/// nothing — reintroducing the race this store exists to close, on a much tighter window.
///
/// # Upstream also records before dedup
///
/// cyrup dedupes earlier, inside `ResultsWatcher::scan_candidates`'s own seen-set, so only a
/// newly-seen notification reaches an observer at all. Same net effect — the first observation
/// records — with one fewer redundant re-projection.
#[async_trait::async_trait]
impl CompletionObserver for WaitCompletionStore {
    async fn observe(&self, notification: &CompletionNotification) -> bool {
        // Only the OWNED band. A completion this instance may not consume is one it must not
        // answer a `wait` with either.
        if notification.band != CompletionBand::Ours {
            return true;
        }
        // `CompletionNotification` carries the PARSED result. Re-serialize it rather than
        // re-reading the file: the value round-trips through the same wire shape the projector
        // reads, and a second read would race the unlink this record exists to survive.
        let Ok(data) = serde_json::to_value(&notification.result) else {
            return true; // a result always serializes; degrade rather than fail the pipeline
        };
        // pi's `:411-412` session-presence gate, expressed as an `Option` rather than an early
        // `return`: on the `Ours` band `session_id` is `Some` by construction
        // (`delivery/custody.rs`'s `classify` destructures it out of the `Option` before the band
        // exists), so the `None` arm is reachable only when no session start has supplied a results
        // dir — a headless embedder — where degrading to the in-process tier is exactly right.
        let replay_dir = self.replay_dir();
        let persistence = match (
            replay_dir.as_deref(),
            notification.result.session_id.as_ref(),
        ) {
            (Some(results_dir), Some(session_id)) => Some(ReplayPersistence {
                results_dir,
                session_id,
            }),
            _ => None,
        };
        self.record(
            &notification.result.run_id,
            &data,
            crate::time::now_epoch_millis(),
            DEDUP_TTL.as_millis().try_into().unwrap_or(i64::MAX),
            persistence.as_ref(),
        )
        .await;
        true
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
    use serde_json::json;

    const TTL: i64 = 600_000;

    fn run(token: &str) -> RunId {
        RunId::from_token(token)
    }

    /// A store hit is what a `wait` reads after the watcher has consumed and deleted the payload —
    /// the whole reason this type is load-bearing rather than a cache. The recorded completion is
    /// the PROJECTED one, and it answers for the run it was recorded under.
    #[tokio::test]
    async fn a_recorded_completion_survives_the_payload_it_was_projected_from() {
        let store = WaitCompletionStore::default();
        assert_eq!(store.get(&run("r1")), None);

        store
            .record(
                &run("r1"),
                &json!({ "agent": "coder", "results": [{ "exitCode": 0 }] }),
                1_000,
                TTL,
                None,
            )
            .await;

        let recorded = store.get(&run("r1")).expect("recorded");
        assert_eq!(recorded.run_id, "r1");
        assert_eq!(recorded.agent.as_deref(), Some("coder"));
        assert_eq!(recorded.results.len(), 1);
        assert_eq!(recorded.results[0].success, Some(true));
        // Only the run it was recorded under; a store hit must never answer for a sibling.
        assert_eq!(store.get(&run("r2")), None);
    }

    /// Prune-then-insert (pi `recordWaitCompletion` `:124-152`). The ordering is observable: an
    /// entry at exactly the TTL boundary is kept (`<=`), one past it is evicted, and the entry
    /// being written survives its own prune.
    #[tokio::test]
    async fn recording_prunes_expired_entries_before_inserting_and_never_the_new_one() {
        let store = WaitCompletionStore::default();
        store.record(&run("old"), &json!({}), 0, TTL, None).await;
        store
            .record(&run("boundary"), &json!({}), 1_000, TTL, None)
            .await;

        // A write at `TTL + 1_000` makes `old` exactly `TTL + 1_000` ms stale (evicted) and
        // `boundary` exactly `TTL` ms stale (kept).
        store
            .record(&run("fresh"), &json!({}), TTL + 1_000, TTL, None)
            .await;

        assert_eq!(store.get(&run("old")), None, "past the TTL, evicted");
        assert!(store.get(&run("boundary")).is_some(), "at the TTL, kept");
        assert!(
            store.get(&run("fresh")).is_some(),
            "the entry being written is inserted after the prune, not swept by it"
        );
    }

    /// The warn-and-skip: upstream throws here, inside a filesystem-watcher tick where nothing can
    /// catch it. Degrading leaves the store untouched — it must not record a half-projected value,
    /// and must not drop the good record it already holds for the same run.
    #[tokio::test]
    async fn an_unprojectable_payload_is_skipped_and_leaves_the_store_intact() {
        let store = WaitCompletionStore::default();
        store
            .record(&run("r1"), &json!({ "agent": "coder" }), 1_000, TTL, None)
            .await;

        // `workflowChildren` belonging to a different run — the one projection failure that is
        // reported rather than degraded (`project.rs`'s strict seam).
        store
            .record(
                &run("r1"),
                &json!({ "agent": "replaced", "workflowChildren": {
                    "version": 1,
                    "parentToolCallId": "tool-1",
                    "workflowRunId": "someone-else",
                    "inventoryComplete": true,
                    "workflowState": "completed",
                    "children": [],
                } }),
                2_000,
                TTL,
                None,
            )
            .await;

        let held = store
            .get(&run("r1"))
            .expect("the good record is still there");
        assert_eq!(held.agent.as_deref(), Some("coder"));
    }

    /// pi `wait-completions.ts:137-150`: with `persistence`, the durable record is written and the
    /// copy stored in memory is the ARCHIVE-PATH-BEARING one the writer hands back. Storing the
    /// pre-persist projection instead would make a `wait` see a different completion depending on
    /// which rung answered it.
    #[tokio::test]
    async fn persistence_writes_the_durable_record_and_stores_the_archive_bearing_copy() {
        use crate::background::completion_replay::{
            ReplayReadFilter, completion_replay_path, read_completion_replay,
        };

        let tmp = tempfile::tempdir().expect("tempdir");
        let session = SessionId::parse("s1").expect("non-empty");
        let store = WaitCompletionStore::default();
        store.set_replay_dir(tmp.path().to_path_buf());
        assert_eq!(store.replay_dir().as_deref(), Some(tmp.path()));

        store
            .record(
                &run("r1"),
                &json!({ "agent": "coder", "results": [{ "finalOutput": "42" }] }),
                1_000,
                TTL,
                Some(&ReplayPersistence {
                    results_dir: tmp.path(),
                    session_id: &session,
                }),
            )
            .await;

        assert!(completion_replay_path(tmp.path(), &run("r1")).exists());
        let in_memory = store.get(&run("r1")).expect("recorded");
        let archive_path = in_memory
            .archive_path
            .as_deref()
            .expect("the stored copy carries the archive path");

        let durable = read_completion_replay(
            tmp.path(),
            &run("r1"),
            ReplayReadFilter {
                session_id: Some(&session),
                now: Some(2_000),
            },
        )
        .await
        .expect("the durable record reads back");
        assert_eq!(durable.completion, in_memory, "the two tiers agree exactly");
        assert_eq!(durable.archive_path.to_string_lossy(), archive_path);
    }

    /// A persistence failure must not cost the in-process record too (pi `:147-149`'s
    /// `console.error` and carry on). An unwritable results dir is the cheapest way to force it.
    #[tokio::test]
    async fn a_failed_persist_still_records_in_process() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // A FILE where the results dir should be: `create_dir_all` under it cannot succeed.
        let blocked = tmp.path().join("not-a-dir");
        tokio::fs::write(&blocked, b"".as_slice())
            .await
            .expect("write");
        let session = SessionId::parse("s1").expect("non-empty");
        let store = WaitCompletionStore::default();

        store
            .record(
                &run("r1"),
                &json!({ "agent": "coder" }),
                1_000,
                TTL,
                Some(&ReplayPersistence {
                    results_dir: &blocked,
                    session_id: &session,
                }),
            )
            .await;

        let recorded = store.get(&run("r1")).expect("the in-process record lands");
        assert_eq!(recorded.agent.as_deref(), Some("coder"));
        assert_eq!(
            recorded.archive_path, None,
            "the pre-persist projection, since nothing was written"
        );
    }

    /// Only the OWNED band. A completion this instance may not consume is one it must not answer
    /// a `wait` with either — and `observe` never reports failure, because returning `false` on
    /// this seam would not abort the pipeline anyway.
    #[tokio::test]
    async fn the_observer_records_the_owned_band_and_ignores_the_others() {
        use crate::background::RunState;
        use crate::background::watch::CompletionBand;
        use crate::background::watch::tests::sample_result;

        let store = WaitCompletionStore::default();

        for (run_id, band) in [
            ("run-foreign", CompletionBand::Foreign),
            ("run-observed", CompletionBand::Observed),
            ("run-ours", CompletionBand::Ours),
        ] {
            let notification = CompletionNotification {
                result: sample_result(run_id, RunState::Complete, true),
                result_path: std::path::PathBuf::new(),
                exhausted: false,
                band,
            };
            assert!(
                store.observe(&notification).await,
                "observing never fails the pipeline"
            );
        }

        assert_eq!(store.get(&run("run-foreign")), None);
        assert_eq!(store.get(&run("run-observed")), None);
        let ours = store
            .get(&run("run-ours"))
            .expect("the owned band recorded");
        assert_eq!(ours.run_id, "run-ours");
        // The notification carries the PARSED result; re-serializing it is what lets the projector
        // see the same wire shape it would have read off disk, without racing the unlink.
        assert_eq!(ours.agent.as_deref(), Some("researcher"));
        // No replay dir was ever supplied, so the durable mirror degrades off rather than guessing
        // a directory — the headless-embedder case.
        assert_eq!(ours.archive_path, None);
    }

    /// The observer's own persistence wiring: with a replay dir supplied, an OWNED notification
    /// writes the durable record too — and it takes its session from the notification's result,
    /// which the `Ours` band guarantees is `Some`.
    #[tokio::test]
    async fn the_observer_persists_when_a_replay_dir_has_been_supplied() {
        use crate::background::RunState;
        use crate::background::completion_replay::completion_replay_path;
        use crate::background::watch::tests::sample_result;

        let tmp = tempfile::tempdir().expect("tempdir");
        let store = WaitCompletionStore::default();
        store.set_replay_dir(tmp.path().to_path_buf());

        let notification = CompletionNotification {
            result: sample_result("run-ours", RunState::Complete, true),
            result_path: std::path::PathBuf::new(),
            exhausted: false,
            band: CompletionBand::Ours,
        };
        assert!(store.observe(&notification).await);

        assert!(
            completion_replay_path(tmp.path(), &run("run-ours")).exists(),
            "the owned band's completion is mirrored durably"
        );
        assert!(
            store
                .get(&run("run-ours"))
                .expect("recorded")
                .archive_path
                .is_some()
        );
    }
}
