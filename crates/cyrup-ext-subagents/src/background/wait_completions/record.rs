//! [`WaitCompletionStore`] — the in-process record of terminal payloads this process has already
//! consumed, wired onto the completion-watcher's own observer fan-out.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::background::RunId;
use crate::background::watch::{
    CompletionBand, CompletionNotification, CompletionObserver, DEDUP_TTL,
};

use super::project::{WaitCompletion, to_wait_completion};

/// The in-process record of terminal payloads this process has already consumed.
///
/// pi `SubagentState.completedResults: Map<runId, {seenAt, completion}>` (`shared/types.ts:2260`),
/// written by `recordWaitCompletion` (`wait-completions.ts:124-152`) from the result watcher
/// (`result-watcher.ts:431`).
///
/// # Why this is load-bearing and not a cache
///
/// The completion watcher deletes a payload once its delivery receipt lands
/// (`watch/install.rs`'s delete-last). A `wait` resolving immediately afterwards has no file to
/// read, and without this record reports a completion it demonstrably observed as absent. This
/// map is the only source between the unlink and a future durable replay.
///
/// A [`std::sync::Mutex`] with poison recovery — this crate's convention for short non-`await`
/// critical sections (`runner_main/status.rs`'s `lock_status`, which states the rule and names
/// `background/tracker.rs` as its precedent): `.lock().unwrap_or_else(PoisonError::into_inner)`,
/// never `.unwrap()` (the crate denies `clippy::unwrap_used`). The guard is never held across an
/// `.await`.
#[derive(Debug, Default)]
pub struct WaitCompletionStore {
    entries: Mutex<HashMap<RunId, RecordedCompletion>>,
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

    /// Record `run_id`'s terminal payload, pruning every entry older than `ttl_ms` first.
    ///
    /// pi `recordWaitCompletion` (`wait-completions.ts:124-152`).
    pub fn record(&self, run_id: &RunId, data: &serde_json::Value, now_ms: i64, ttl_ms: i64) {
        record_wait_completion(self, run_id, data, now_ms, ttl_ms);
    }
}

/// pi `recordWaitCompletion` (`wait-completions.ts:124-152`), the free function
/// [`WaitCompletionStore::record`] calls, in upstream's own order.
fn record_wait_completion(
    store: &WaitCompletionStore,
    run_id: &RunId,
    data: &serde_json::Value,
    now_ms: i64,
    ttl_ms: i64,
) {
    let mut entries = store
        .entries
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // 1. Prune first, inside the SAME lock acquisition: two separate acquisitions would let a
    //    reader observe a pruned-but-not-yet-inserted map in between.
    entries.retain(|_, entry| now_ms.saturating_sub(entry.seen_at) <= ttl_ms);

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

    // 3. A durable completion-replay seam for SCOPE_4: upstream's `persistence` argument routes
    //    the completion through `writeCompletionReplay` and stores the archive-path-bearing copy.
    //    Left OFF this signature on purpose — an always-absent parameter no caller can populate
    //    would be worse than adding it later alongside its own consumer. SCOPE_4 is what adds the
    //    `persistence` parameter here.

    // 4. Insert.
    entries.insert(
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
/// `result-watcher.ts:428-435`).
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
        self.record(
            &notification.result.run_id,
            &data,
            crate::time::now_epoch_millis(),
            DEDUP_TTL.as_millis().try_into().unwrap_or(i64::MAX),
        );
        true
    }
}
