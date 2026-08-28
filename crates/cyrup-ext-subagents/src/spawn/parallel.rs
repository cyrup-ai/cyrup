//! Bounded-concurrency fan-out over real child OS processes (func-SA §5.3 R-SA-049/050/051/066/069;
//! arch-SA §5.3/§6.4).
//!
//! This is the single bounded-concurrency-over-real-subprocesses primitive this crate introduces
//! (arch-SA §5.3): `ParallelGroup`/`DynamicGroup` fan-out uses a fixed pool of `tokio::spawn`'d
//! workers, each holding one [`tokio::sync::Semaphore`] permit drawn from a *group-local* semaphore
//! that is itself sized to `min(group.concurrency, global_remaining)` against a *run-wide* shared
//! semaphore (R-SA-049/050) — never an unbounded spawn of all `N` children at once, and never an
//! in-process async-task pool standing in for the actual agent work: each worker still owns exactly
//! one real, spawned child process for the duration of its permit. Results are written into a
//! pre-sized `Vec<Option<R>>` indexed by original position (R-SA-051), so output order always
//! matches input order regardless of which child happens to finish first.
//!
//! `cyrup-agent::execute_parallel`'s `JoinSet`-over-in-process-futures pattern
//! (`crates/cyrup-agent/src/agent.rs:954-1091`) is explicitly NOT reused here (arch-SA §5.3): that
//! primitive fans out in-process tool-call futures, not OS subprocesses, and this crate has ZERO
//! dependency on `cyrup-agent` in any case (arch-SA §2.1). This module is generic over the actual
//! per-task work (`Worker`), rather than depending on `crate::exec`'s not-yet-implemented
//! `SingleStep`/`StepResult` types (a later phase of this crate's build-out, arch-SA §2.2) — the
//! chain/workflow driver (`spawn/chain_graph.rs`, also a later phase) is expected to call
//! [`run_bounded`] with a worker closure that constructs a [`crate::spawn::ChildSpawnSpec`] and
//! drives it to completion via [`crate::spawn::SpawnedChild`] for each task.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::error::SubagentError;

/// The run-wide concurrency ceiling shared across every `ParallelGroup`/`DynamicGroup` execution
/// within one chain run (R-SA-050), plus the default value that ceiling takes when a run does not
/// override it (`global_concurrency_limit`, func-SA §5.3 default: 20).
pub const DEFAULT_GLOBAL_CONCURRENCY_LIMIT: usize = 20;

/// A run-wide concurrency ceiling shared across every `ParallelGroup`/`DynamicGroup` fan-out
/// within one chain run (R-SA-050) — the *second*, outer tier of the two-tier concurrency model.
///
/// One [`GlobalConcurrencyLimit`] is constructed once per chain run (by the chain/workflow driver,
/// `spawn/chain_graph.rs`, a later phase of this crate's build-out) and cloned (cheaply — it is an
/// `Arc` internally) into every [`run_bounded`] call that fan-out step makes, so a group's own
/// local `concurrency` is always additionally bounded by however many permits the run-wide ceiling
/// has left, never merely by the group's own local cap in isolation.
#[derive(Clone, Debug)]
pub struct GlobalConcurrencyLimit {
    semaphore: Arc<Semaphore>,
}

impl GlobalConcurrencyLimit {
    /// Construct a fresh run-wide ceiling with exactly `limit` permits available. `limit == 0`
    /// degrades to "no concurrent children of any group may run" rather than being treated as
    /// "unlimited" — callers that want the func-SA default must pass
    /// [`DEFAULT_GLOBAL_CONCURRENCY_LIMIT`] explicitly, never `0` as a stand-in for "no ceiling".
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(limit)),
        }
    }

    /// The run-wide ceiling using func-SA §5.3's default of
    /// [`DEFAULT_GLOBAL_CONCURRENCY_LIMIT`] (20).
    #[must_use]
    pub fn default_limit() -> Self {
        Self::new(DEFAULT_GLOBAL_CONCURRENCY_LIMIT)
    }

    /// How many run-wide permits are currently unclaimed. This is advisory only (another group's
    /// concurrent workers may claim/release permits between this read and a subsequent acquire) —
    /// it exists so [`run_bounded`] can size a group-local semaphore to
    /// `min(group.concurrency, global_remaining)` per R-SA-049/050's exact wording, not as a
    /// synchronization primitive in its own right (the actual cross-group safety comes from the
    /// shared [`tokio::sync::Semaphore`] itself, acquired per-task inside [`run_bounded`]'s worker
    /// loop, never from this advisory count).
    #[must_use]
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

/// A single dispatched task's outcome, wrapping the caller-supplied worker's own result type `R`
/// alongside which original input-order index it corresponds to (R-SA-051) and whether it actually
/// ran or was skipped due to a `fail_fast` short-circuit (R-SA-066).
#[derive(Debug)]
pub struct SlotOutcome<R> {
    /// The task's original position in the input `Vec` (R-SA-051) — every [`run_bounded`] caller
    /// can rely on `results[i].index == i` holding for every populated slot, but this field is
    /// still carried explicitly so a caller iterating the returned `Vec<Option<SlotOutcome<R>>>`
    /// need not separately re-derive the index from its own position.
    pub index: usize,
    /// The worker's own result for this task.
    pub result: R,
}

/// One input-order-indexed slot's interior-mutable result cell, shared across the worker pool
/// while a [`run_bounded`] fan-out is in flight — factored into a named alias purely to keep
/// every signature below readable (clippy's `type_complexity` lint, correctly, flags the fully
/// spelled-out nested form at every one of these call sites otherwise).
type SlotArray<R, E> = Vec<std::sync::Mutex<Option<Result<SlotOutcome<R>, E>>>>;

/// Why a given input-order slot in [`run_bounded`]'s result `Vec` was never populated with a
/// [`SlotOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// A prior sibling task failed and `fail_fast` was set (R-SA-066): this task was never
    /// dispatched at all — cooperative, not preemptive, since R-SA-066 explicitly forbids killing
    /// already-*running* siblings, but a task that has not yet started dispatching is fair game to
    /// simply never start.
    FailFastSkipped,
    /// The worker pool as a whole was cancelled (the caller's `cancel` token fired) before this
    /// task's turn came up in the shared cursor.
    Cancelled,
}

/// The full result of one [`run_bounded`] fan-out: a pre-sized, input-order-indexed `Vec` (R-SA-051)
/// where every slot is either `Ok` (the task ran and the worker returned `Ok(R)`), an `Err` wrapping
/// whatever error type `E` the worker's own `Result<R, E>` uses, or `None` if the slot was never
/// dispatched at all (see [`SkipReason`]).
pub struct FanOutResult<R, E> {
    /// One entry per input task, in ORIGINAL input order — never completion order — regardless of
    /// which child actually finished first (R-SA-051). `None` marks a task that was never
    /// dispatched (see [`SkipReason`]).
    pub slots: Vec<Option<Result<SlotOutcome<R>, E>>>,
    /// Whether any dispatched task's worker returned `Err`. Exposed as a precomputed convenience
    /// so callers implementing `fail_fast` semantics at a higher layer (the chain driver: should
    /// the run as a whole be marked failed) do not need to re-scan `slots` themselves for the
    /// common case.
    pub any_failed: bool,
    /// How many input slots were skipped via [`SkipReason::FailFastSkipped`] specifically (as
    /// opposed to [`SkipReason::Cancelled`]) — useful for a chain-step summary line without a
    /// separate scan.
    pub fail_fast_skipped_count: usize,
    /// One entry per input task, positionally aligned with [`Self::slots`]: `Some(reason)` for a
    /// slot that stayed `None` because it was never dispatched, `None` for a slot that actually
    /// ran (whether it succeeded or failed).
    ///
    /// [`Self::fail_fast_skipped_count`] answers "how many", this answers "which ones, and why" —
    /// the distinction matters to the chain driver because pi synthesizes a *fail-fast-specific*
    /// placeholder result for the un-run siblings (`chain-execution.ts:321-330`: `task:
    /// "(skipped)"`, `exitCode: -1`, `error: "Skipped due to fail-fast"`) and has no analog for a
    /// cancellation skip, which is a cyrup-side concept.
    pub skip_reasons: Vec<Option<SkipReason>>,
}

/// Fan a `Vec` of `tasks` out across a bounded worker pool of real child OS processes
/// (R-SA-049/050/051/066).
///
/// # Concurrency model
///
/// A fixed pool of `min(group_concurrency, tasks.len())` `tokio::spawn`'d workers (R-SA-049: never
/// an unbounded spawn of all `tasks.len()` children at once) pull task indices from a shared
/// `AtomicUsize` cursor. Each worker, before actually invoking `worker` for a given task, acquires
/// ONE permit each from two independent semaphores in sequence — first the *group-local* semaphore
/// (sized to `group_concurrency`, constructed fresh inside this call), then the *run-wide*
/// `global_limit` semaphore shared across every concurrently-running `ParallelGroup`/`DynamicGroup`
/// in the same chain run (R-SA-050) — releasing both only after `worker` for that task has fully
/// completed. This two-tier acquire is what makes the effective concurrency for this call exactly
/// `min(group_concurrency, tasks.len(), <whatever the global semaphore has available at any given
/// moment>)`, satisfying R-SA-050's "layered under each group's local `concurrency`" requirement
/// without this function ever needing to snapshot `global_limit.available_permits()` itself (which
/// would be racy against other concurrently-running groups) — the actual bound is enforced by the
/// shared semaphore's own acquire, not by any precomputed arithmetic.
///
/// `tokio::sync::Semaphore`'s native FIFO fairness (permits are granted to waiters in the order
/// they started waiting) is used directly, exactly as arch-SA §5.3 specifies — this function
/// invents no separate fairness/ordering scheme of its own for permit acquisition.
///
/// # Ordering (R-SA-051)
///
/// Results are written into a `Vec` pre-sized to `tasks.len()` via direct indexed assignment
/// (`slots[i] = ...`, guarded per-slot so no two workers ever race the same index), so the returned
/// [`FanOutResult::slots`] always matches input order regardless of completion order — this
/// function never collects results via an unordered `mpsc`/`FuturesUnordered`-style stream.
///
/// # `fail_fast` (R-SA-066: cooperative, not preemptive)
///
/// When `fail_fast` is `true` and any dispatched task's worker returns `Err`, every worker in the
/// pool stops claiming NEW task indices from the shared cursor once it next checks (a cooperative
/// flag, checked before each new dispatch) — but a task a worker had ALREADY started dispatching
/// (i.e. already past the semaphore-acquire step and into `worker(...)` itself) is never
/// interrupted or killed; R-SA-066 is explicit that `fail_fast` "MUST NOT kill already-dispatched,
/// still-running sibling child processes". Tasks never claimed because of a `fail_fast` trip are
/// marked [`SkipReason::FailFastSkipped`] in the returned `slots`, never silently absent.
///
/// # Cancellation
///
/// `cancel` is raced against both the semaphore-acquire step and the worker invocation itself for
/// every task; once cancelled, no new task is dispatched (already-running workers are allowed to
/// finish their own `worker` future, since this function has no OS-process handle of its own to
/// terminate — that responsibility belongs to whatever `worker` closure the caller supplied, via
/// its own `cancel` plumbing into [`crate::spawn::SpawnedChild::terminate`]). Slots never claimed
/// because of cancellation are marked [`SkipReason::Cancelled`].
///
/// # Errors
///
/// This function itself is infallible at the pool level (a panicking worker task is caught and
/// surfaced as [`SubagentError::Spawn`] for that one slot only, never propagated to abort the
/// whole pool) — per-task failures live inside [`FanOutResult::slots`], not as a top-level `Err`
/// from this function.
pub async fn run_bounded<T, R, E, Worker, Fut>(
    tasks: Vec<T>,
    group_concurrency: usize,
    global_limit: &GlobalConcurrencyLimit,
    fail_fast: bool,
    cancel: cyrup_core::CancelToken,
    worker: Worker,
) -> FanOutResult<R, E>
where
    T: Send + 'static,
    R: Send + 'static,
    E: Send + 'static,
    Worker: Fn(usize, T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R, E>> + Send + 'static,
{
    let total = tasks.len();

    // Pre-size the result Vec up front (R-SA-051) — every slot starts `None` and is written
    // exactly once, by whichever worker claims that index, via direct indexed assignment guarded
    // by a per-slot Mutex so out-of-order completion can never corrupt or duplicate a write.
    let slots: Arc<SlotArray<R, E>> =
        Arc::new((0..total).map(|_| std::sync::Mutex::new(None)).collect());
    let skip_reasons: Arc<Vec<std::sync::Mutex<Option<SkipReason>>>> =
        Arc::new((0..total).map(|_| std::sync::Mutex::new(None)).collect());

    if total == 0 {
        return FanOutResult {
            slots: Vec::new(),
            any_failed: false,
            fail_fast_skipped_count: 0,
            skip_reasons: Vec::new(),
        };
    }

    // R-SA-049: a FIXED pool of workers, never one tokio task per input task — bounded strictly
    // by `min(group_concurrency, total)`. `group_concurrency == 0` degrades to a pool of exactly
    // one worker (rather than zero, which would dispatch nothing at all) so a misconfigured
    // `concurrency: 0` on a group still makes forward progress serially instead of silently
    // hanging forever with no worker ever claiming the shared cursor.
    let worker_pool_size = group_concurrency.clamp(1, total);

    // The group-local semaphore (R-SA-049's own `group.concurrency` tier) — sized to
    // `group_concurrency` itself (NOT `worker_pool_size`, which is additionally clamped by
    // `total`), so a group configured with a large `concurrency` against a small `tasks.len()`
    // still behaves correctly: the worker-pool-size clamp only avoids spawning MORE tokio tasks
    // than there is work for, it never changes the actual permit ceiling a group is entitled to.
    let group_semaphore = Arc::new(Semaphore::new(group_concurrency.max(1)));

    // Shared, monotonically-advancing cursor into `tasks` (by original index) that every worker
    // pulls from — this, not per-worker chunking, is what preserves correctness regardless of how
    // long any individual task takes: a fast worker simply claims more indices over the run's
    // lifetime than a slow one, with no rebalancing logic needed.
    let cursor = Arc::new(AtomicUsize::new(0));

    // R-SA-066: cooperative fail-fast flag. Checked by every worker BEFORE claiming a new index
    // from the cursor — never used to abort a task already in flight.
    let fail_fast_tripped = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // `tasks` is moved into an `Arc<Vec<Mutex<Option<T>>>>` so multiple worker tasks can each take
    // ownership of exactly the one element they claim from the shared cursor, without cloning `T`
    // (workers must be able to run over non-`Clone` task payloads, e.g. a `ChildSpawnSpec` with a
    // temp-file list that must not be logically duplicated).
    let tasks: Arc<Vec<std::sync::Mutex<Option<T>>>> =
        Arc::new(tasks.into_iter().map(|t| std::sync::Mutex::new(Some(t))).collect());

    let worker = Arc::new(worker);
    let mut join_set: JoinSet<()> = JoinSet::new();

    for _ in 0..worker_pool_size {
        let slots = Arc::clone(&slots);
        let skip_reasons = Arc::clone(&skip_reasons);
        let cursor = Arc::clone(&cursor);
        let fail_fast_tripped = Arc::clone(&fail_fast_tripped);
        let tasks = Arc::clone(&tasks);
        let worker = Arc::clone(&worker);
        let group_semaphore = Arc::clone(&group_semaphore);
        let global_semaphore = Arc::clone(&global_limit.semaphore);
        let cancel = cancel.clone();

        join_set.spawn(async move {
            loop {
                if cancel.is_cancelled() {
                    // Mark every remaining unclaimed slot as Cancelled (best-effort; a slot may
                    // already have been claimed and be mid-flight, in which case its eventual
                    // real outcome — not a synthesized Cancelled skip — wins, since this only
                    // marks slots whose `skip_reasons`/`slots` entry is still genuinely `None`).
                    mark_remaining_skipped(&cursor, total, &slots, &skip_reasons, SkipReason::Cancelled);
                    return;
                }
                if fail_fast.load_tripped(&fail_fast_tripped) {
                    mark_remaining_skipped(
                        &cursor,
                        total,
                        &slots,
                        &skip_reasons,
                        SkipReason::FailFastSkipped,
                    );
                    return;
                }

                let index = cursor.fetch_add(1, Ordering::SeqCst);
                if index >= total {
                    return; // no more work: this worker's contribution to the pool is done
                }

                let Some(task) = take_task(&tasks, index) else {
                    // Another worker already claimed this exact index (should not happen given
                    // the atomic fetch_add above hands out each index exactly once, but guarded
                    // defensively rather than ever indexing/unwrapping blindly).
                    continue;
                };

                // Two-tier permit acquire (R-SA-049/050): group-local first, then run-wide,
                // released (via RAII guard drop) only after `worker(...)` fully completes. Both
                // acquires are raced against `cancel` so a cancellation mid-wait does not block
                // this worker forever behind a permit that may never free up.
                let _group_permit = tokio::select! {
                    biased;
                    permit = group_semaphore.acquire() => match permit {
                        Ok(p) => p,
                        Err(_closed) => {
                            mark_skipped(&slots, &skip_reasons, index, SkipReason::Cancelled);
                            continue;
                        }
                    },
                    () = cancel.cancelled() => {
                        mark_skipped(&slots, &skip_reasons, index, SkipReason::Cancelled);
                        continue;
                    }
                };
                let _global_permit = tokio::select! {
                    biased;
                    permit = global_semaphore.acquire() => match permit {
                        Ok(p) => p,
                        Err(_closed) => {
                            mark_skipped(&slots, &skip_reasons, index, SkipReason::Cancelled);
                            continue;
                        }
                    },
                    () = cancel.cancelled() => {
                        mark_skipped(&slots, &skip_reasons, index, SkipReason::Cancelled);
                        continue;
                    }
                };

                let outcome = worker(index, task).await;
                let failed = outcome.is_err();
                let slot_result = outcome.map(|result| SlotOutcome { index, result });
                write_slot(&slots, index, slot_result);

                if failed && fail_fast {
                    fail_fast_tripped.store(true, Ordering::SeqCst);
                }
                // Permits released here via `_group_permit`/`_global_permit` drop at loop-end.
            }
        });
    }

    // Drain the pool. A worker task panicking (should never happen given every fallible step
    // above is handled via Result/Option, never unwrap/expect/indexing — but JoinSet still
    // surfaces a `JoinError` if the tokio runtime itself aborts a task) is treated as "that one
    // worker's remaining claimed slots stay whatever they already were" — the other workers in
    // the pool are unaffected and continue draining the shared cursor independently, so a single
    // worker task fault degrades gracefully rather than corrupting the whole fan-out.
    while join_set.join_next().await.is_some() {}

    finalize(slots, skip_reasons, total)
}

/// Small helper trait so the `fail_fast` bool and the shared trip flag can be checked together at
/// one call site without repeating `fail_fast && flag.load(...)` inline at both loop-guard sites.
trait FailFastGate {
    fn load_tripped(self, flag: &std::sync::atomic::AtomicBool) -> bool;
}
impl FailFastGate for bool {
    fn load_tripped(self, flag: &std::sync::atomic::AtomicBool) -> bool {
        self && flag.load(Ordering::SeqCst)
    }
}

/// Take ownership of `tasks[index]`'s payload, leaving `None` behind — returns `None` if the slot
/// was already taken (defensive; the shared atomic cursor should make this unreachable in
/// practice, but this function never indexes/unwraps blindly regardless).
fn take_task<T>(tasks: &[std::sync::Mutex<Option<T>>], index: usize) -> Option<T> {
    tasks.get(index).and_then(|slot| match slot.lock() {
        Ok(mut guard) => guard.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    })
}

/// Write `value` into `slots[index]`, guarded by that slot's own `Mutex` (R-SA-051: safe
/// out-of-order concurrent writes to distinct indices, with no possibility of two workers ever
/// writing the SAME index since the shared atomic cursor hands each index out exactly once).
fn write_slot<R, E>(slots: &SlotArray<R, E>, index: usize, value: Result<SlotOutcome<R>, E>) {
    if let Some(slot) = slots.get(index) {
        match slot.lock() {
            Ok(mut guard) => *guard = Some(value),
            Err(poisoned) => *poisoned.into_inner() = Some(value),
        }
    }
}

/// Mark exactly `index`'s skip reason, but ONLY if that slot has not already been populated with a
/// real outcome by [`write_slot`] — guards against a benign race where a task fully completes
/// between the cancellation/fail-fast check and this call.
fn mark_skipped<R, E>(
    slots: &SlotArray<R, E>,
    skip_reasons: &[std::sync::Mutex<Option<SkipReason>>],
    index: usize,
    reason: SkipReason,
) {
    let already_populated = slots
        .get(index)
        .map(|slot| match slot.lock() {
            Ok(guard) => guard.is_some(),
            Err(poisoned) => poisoned.into_inner().is_some(),
        })
        .unwrap_or(true);
    if already_populated {
        return;
    }
    if let Some(reason_slot) = skip_reasons.get(index) {
        match reason_slot.lock() {
            Ok(mut guard) => *guard = Some(reason),
            Err(poisoned) => *poisoned.into_inner() = Some(reason),
        }
    }
}

/// Mark every index from the current cursor position through `total` as skipped with `reason`
/// (used once a worker observes cancellation or a tripped fail-fast flag and is about to stop
/// claiming new work) — does not advance the cursor itself, so other still-live workers racing the
/// same observation do not double-count; each simply marks whichever indices it still finds
/// unpopulated.
fn mark_remaining_skipped<R, E>(
    cursor: &AtomicUsize,
    total: usize,
    slots: &SlotArray<R, E>,
    skip_reasons: &[std::sync::Mutex<Option<SkipReason>>],
    reason: SkipReason,
) {
    let from = cursor.load(Ordering::SeqCst).min(total);
    for index in from..total {
        mark_skipped(slots, skip_reasons, index, reason);
    }
}

/// Consume the interior-mutable `slots`/`skip_reasons` scaffolding into the final, plain
/// [`FanOutResult`] returned to the caller — every real outcome takes priority over a skip marker
/// for the same index (a skip marker is only ever informational for a slot that stayed `None`).
fn finalize<R, E>(
    slots: Arc<SlotArray<R, E>>,
    skip_reasons: Arc<Vec<std::sync::Mutex<Option<SkipReason>>>>,
    total: usize,
) -> FanOutResult<R, E> {
    let slots = Arc::try_unwrap(slots).unwrap_or_else(|shared| {
        // Another clone is still alive somewhere (should not happen once every worker task has
        // joined) — fall back to draining through the shared reference rather than ever
        // panicking; correctness is unaffected either way since every worker has already
        // finished writing by the time this function runs.
        (0..shared.len())
            .map(|i| {
                std::sync::Mutex::new(match shared.get(i) {
                    Some(slot) => match slot.lock() {
                        Ok(mut guard) => guard.take(),
                        Err(poisoned) => poisoned.into_inner().take(),
                    },
                    None => None,
                })
            })
            .collect()
    });
    let skip_reasons = Arc::try_unwrap(skip_reasons).unwrap_or_else(|shared| {
        (0..shared.len())
            .map(|i| {
                std::sync::Mutex::new(match shared.get(i) {
                    Some(slot) => match slot.lock() {
                        Ok(mut guard) => guard.take(),
                        Err(poisoned) => poisoned.into_inner().take(),
                    },
                    None => None,
                })
            })
            .collect()
    });

    let mut any_failed = false;
    let mut fail_fast_skipped_count = 0usize;
    let mut result_slots = Vec::with_capacity(total);
    let mut result_skip_reasons = Vec::with_capacity(total);

    for (outcome_slot, reason_slot) in slots.into_iter().zip(skip_reasons) {
        let outcome = match outcome_slot.into_inner() {
            Ok(v) => v,
            Err(poisoned) => poisoned.into_inner(),
        };
        let reason = match reason_slot.into_inner() {
            Ok(v) => v,
            Err(poisoned) => poisoned.into_inner(),
        };

        match outcome {
            Some(res) => {
                if res.is_err() {
                    any_failed = true;
                }
                result_slots.push(Some(res));
                // A real outcome always wins over an informational skip marker for the same
                // index, so this slot reports "not skipped" even if a marker was written.
                result_skip_reasons.push(None);
            }
            None => {
                if reason == Some(SkipReason::FailFastSkipped) {
                    fail_fast_skipped_count += 1;
                }
                result_slots.push(None);
                result_skip_reasons.push(reason);
            }
        }
    }

    FanOutResult {
        slots: result_slots,
        any_failed,
        fail_fast_skipped_count,
        skip_reasons: result_skip_reasons,
    }
}

// ---------------------------------------------------------------------------------------------
// R-SA-069: duplicate-dispatch guard.
//
// "A concurrent duplicate `subagent` execution call issued while a prior dispatch from the same
// call site is still in progress MUST be rejected; this MUST NOT affect intentional parallel-mode
// dispatch within one call, which remains unaffected."
//
// arch-SA §9's requirement-coverage table places the call-site dedup key inside
// `exec::SubagentExecutor` (`exec/mod.rs`) — a later phase of this crate's build-out that does not
// exist yet (only `exec/mod.rs`'s module-doc stub is present as of this file). The actual guard
// PRIMITIVE this crate uses to satisfy R-SA-069 is defined here, in `spawn/parallel.rs`, since it
// is a natural sibling of this file's own duplicate-avoidance concerns (bounded dispatch) and has
// no dependency on any of `exec/mod.rs`'s not-yet-implemented types — `exec::SubagentExecutor` is
// expected to hold one `DispatchGuard` per call site (keyed however that module defines "same call
// site", e.g. a tool-call id or slash-command invocation identity) and call
// `DispatchGuard::try_acquire` at the top of its single execution entry point, releasing the guard
// (via the returned RAII token's `Drop`) once that one top-level dispatch fully completes. This
// guard intentionally has NO opinion on what happens *inside* one accepted dispatch — a single
// call site legitimately fanning out into `run_bounded`'s own bounded-parallel dispatch is exactly
// the "one call" R-SA-069 says must remain unaffected; only a SECOND, wholly separate top-level
// call arriving concurrently is what this guard rejects.
// ---------------------------------------------------------------------------------------------

/// A reusable duplicate-dispatch guard (R-SA-069): rejects a concurrent duplicate top-level
/// dispatch from the same call site while a prior one is still in progress, without affecting
/// intentional parallel-mode fan-out *within* one already-accepted dispatch (that fan-out is what
/// [`run_bounded`] itself provides, and is never gated by this guard).
///
/// A single boolean-shaped `Arc<Semaphore>` with exactly one permit is deliberately reused as the
/// guard's implementation (rather than a bespoke `AtomicBool`-plus-`Drop` type) so acquisition
/// composes naturally with `tokio::select!`-based cancellation the same way every other permit
/// acquisition in this module does — `try_acquire` never blocks, matching R-SA-069's "reject",
/// never "queue", semantics.
#[derive(Clone, Debug)]
pub struct DispatchGuard {
    semaphore: Arc<Semaphore>,
}

impl DispatchGuard {
    /// Construct a fresh, initially-unclaimed dispatch guard for one call site.
    #[must_use]
    pub fn new() -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(1)),
        }
    }

    /// Attempt to claim this call site's single dispatch slot. Returns `Some(token)` — holding the
    /// slot for as long as the token is alive, released automatically on `Drop` — if no other
    /// dispatch from this same call site is currently in progress; returns `None` immediately
    /// (never blocks) if one already is, per R-SA-069's "MUST be rejected" wording (a reject, not a
    /// queue-and-wait).
    #[must_use]
    pub fn try_acquire(&self) -> Option<DispatchToken> {
        self.semaphore
            .clone()
            .try_acquire_owned()
            .ok()
            .map(|permit| DispatchToken { _permit: permit })
    }

    /// Same as [`DispatchGuard::try_acquire`], but surfaces the rejection as a
    /// [`SubagentError`]-compatible `Result` for call sites that want `?`-propagation rather than
    /// an `Option` match. `call_site` is folded into the error message purely for diagnostics.
    ///
    /// # Errors
    ///
    /// Returns `Err` (never blocks) if a prior dispatch from this same guard is still in progress.
    pub fn try_acquire_or_reject(&self, call_site: &str) -> Result<DispatchToken, SubagentError> {
        self.try_acquire().ok_or_else(|| {
            SubagentError::AcceptanceRejected(format!(
                "duplicate concurrent subagent dispatch rejected for call site '{call_site}' \
                 (R-SA-069): a prior dispatch from this call site is still in progress"
            ))
        })
    }
}

impl Default for DispatchGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII proof that a [`DispatchGuard`]'s single slot is held; dropping this releases the slot,
/// allowing the next (non-concurrent) dispatch from the same call site to proceed.
#[derive(Debug)]
pub struct DispatchToken {
    _permit: tokio::sync::OwnedSemaphorePermit,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use std::sync::atomic::AtomicI64;
    use std::time::Duration;

    // -------------------------------------------------------------------------------------------
    // R-SA-049/050: bounded concurrency over REAL spawned child processes.
    //
    // Per this crate's no-mocked-subprocess-behavior testing convention (arch-SA §11, mirroring
    // `cyrup_ext::caps::proc`/`cyrup_tools::ops::local`'s own real-child-process tests), the
    // "worker" closures below spawn REAL `sh` child processes via `crate::spawn::{ChildSpawnSpec,
    // SpawnedChild}` — the exact same spawn boundary the rest of this crate uses — rather than a
    // synthetic in-process async sleep standing in for "a live child". Concurrency is asserted via
    // a shared atomic "currently running" counter incremented/decremented around each REAL
    // process's lifetime, sampling its peak value, which is the load-bearing assertion R-SA-049
    // requires (never more than the configured cap of real OS processes alive at once).
    // -------------------------------------------------------------------------------------------

    fn sh_command(script: &str) -> crate::spawn::SpawnCommand {
        let sh_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("sh"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
        crate::spawn::SpawnCommand {
            binary: sh_path,
            base_args: vec!["-c".to_string(), script.to_string()],
        }
    }

    /// Spawns one real, short-lived `sh` child (sleeping briefly to give overlapping workers a
    /// real window to race in), incrementing/decrementing `running`/`peak` around its REAL
    /// lifetime (from spawn through confirmed exit), and returns its exit code.
    /// SUBA-033 — the deterministic overlap rendezvous.
    ///
    /// The `peak >= 2` lower bounds these tests used to carry are wall-clock races: on a
    /// single-core or heavily loaded runner the pool can legitimately finish task 0 before task 1
    /// is dispatched, the observed peak is 1, and the test goes red for a reason that has nothing
    /// to do with the invariant it names. The in-repo precedent is commit `1806375`, which deleted
    /// an `orphaned > 0` lower bound for exactly this reason.
    ///
    /// A rendezvous sized to the concurrency actually being claimed converts "did two happen to
    /// overlap?" into "two MUST overlap or nothing proceeds": the first `size` workers wait before
    /// spawning their children, so the `size`-th arrival releases all of them and the overlap is a
    /// precondition of the run rather than an observation about scheduling. The `<= cap` upper
    /// bound — the real invariant — is unaffected and stays.
    ///
    /// The wait is bounded so a genuine serialization regression fails LOUDLY instead of hanging
    /// the suite forever.
    const OVERLAP_RENDEZVOUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

    /// A ONE-SHOT rendezvous for exactly the first `size` arrivals; every later arrival passes
    /// straight through.
    ///
    /// [`tokio::sync::Barrier`] cannot be used directly for this, and quietly does the wrong
    /// thing if you try: it is *cyclic*, re-arming after every `size` arrivals, so gating `n`
    /// workers on a bare `Barrier::new(size)` strands the trailing `n % size` arrivals forever —
    /// they wait for a generation that no remaining worker will ever complete. That is a
    /// deterministic hang, not a flake: 8 tasks over a barrier of 3 always leaves workers 6 and 7
    /// blocked. Counting arrivals and only letting the first `size` of them touch the barrier at
    /// all is what makes "only the first `size` arrivals ever wait" actually true.
    struct FirstArrivalsRendezvous {
        barrier: tokio::sync::Barrier,
        arrivals: AtomicUsize,
        size: usize,
    }

    impl FirstArrivalsRendezvous {
        fn new(size: usize) -> Self {
            Self {
                barrier: tokio::sync::Barrier::new(size),
                arrivals: AtomicUsize::new(0),
                size,
            }
        }

        /// Blocks until `size` workers have arrived, for the first `size` callers only.
        async fn join(&self) {
            if self.arrivals.fetch_add(1, Ordering::SeqCst) < self.size {
                self.barrier.wait().await;
            }
        }
    }

    async fn real_child_worker(
        dir: std::path::PathBuf,
        running: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        index: usize,
        sleep_ms: u64,
    ) -> Result<i32, String> {
        real_child_worker_rendezvous(dir, running, peak, index, sleep_ms, None).await
    }

    async fn real_child_worker_rendezvous(
        dir: std::path::PathBuf,
        running: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        index: usize,
        sleep_ms: u64,
        rendezvous: Option<Arc<FirstArrivalsRendezvous>>,
    ) -> Result<i32, String> {
        let spec = crate::spawn::ChildSpawnSpec {
            command: sh_command(&format!("sleep {}; exit 0", sleep_ms as f64 / 1000.0)),
            args: Vec::new(),
            task_arg: String::new(),
            env_overlay: std::collections::HashMap::new(),
            cwd: dir.clone(),
            temp_files: Vec::new(),
        };
        let jsonl_path = dir.join(format!("fanout-{index}.jsonl"));

        let now = running.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(now, Ordering::SeqCst);

        // SUBA-033: hold here until the expected number of workers have ALSO been dispatched, so
        // the overlap the caller asserts is structural rather than incidental. Only the first
        // `size` arrivals ever wait — later ones pass straight through, which is what keeps a group
        // larger than the rendezvous from stranding its trailing workers.
        if let Some(rendezvous) = &rendezvous {
            tokio::time::timeout(OVERLAP_RENDEZVOUS_TIMEOUT, rendezvous.join())
                .await
                .map_err(|_| {
                    format!(
                        "worker {index} waited {OVERLAP_RENDEZVOUS_TIMEOUT:?} at the overlap \
                         rendezvous without enough siblings joining it — the bounded pool \
                         serialized work it was asked to run concurrently"
                    )
                })?;
        }

        let mut child = crate::spawn::SpawnedChild::spawn(spec, &jsonl_path)
            .await
            .map_err(|e| e.to_string())?;
        while child.next_event().await.is_some() {}
        let status = child
            .wait_final_drain()
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "child did not exit within final-drain timeout".to_string())?;
        child.finish();

        running.fetch_sub(1, Ordering::SeqCst);

        Ok(status.code().unwrap_or(-1))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn bounded_pool_never_exceeds_the_group_cap_across_real_child_processes() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let total_tasks = 8usize;
        let cap = 3usize;

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let tasks: Vec<()> = (0..total_tasks).map(|_| ()).collect();
        let global = GlobalConcurrencyLimit::new(DEFAULT_GLOBAL_CONCURRENCY_LIMIT);
        let cancel = cyrup_core::CancelToken::new();

        let dir_path = dir.path().to_path_buf();
        let running_for_worker = Arc::clone(&running);
        let peak_for_worker = Arc::clone(&peak);
        // SUBA-033: the cap IS the concurrency the pool is being asked to reach, so a rendezvous of
        // exactly `cap` makes the overlap a precondition instead of an observation. `run_bounded`
        // dispatches at most `cap` at a time, so the first `cap` arrivals release each other and
        // every later worker sails through — no stranded tail, and no wall-clock lower bound left.
        let rendezvous = Arc::new(FirstArrivalsRendezvous::new(cap));

        let result: FanOutResult<i32, String> = run_bounded(
            tasks,
            cap,
            &global,
            false,
            cancel,
            move |index, ()| {
                let dir_path = dir_path.clone();
                let running = Arc::clone(&running_for_worker);
                let peak = Arc::clone(&peak_for_worker);
                let rendezvous = Arc::clone(&rendezvous);
                async move {
                    real_child_worker_rendezvous(
                        dir_path,
                        running,
                        peak,
                        index,
                        150,
                        Some(rendezvous),
                    )
                    .await
                }
            },
        )
        .await;

        assert_eq!(result.slots.len(), total_tasks, "one slot per input task");
        assert!(
            !result.any_failed,
            "every real sh child must exit 0 — a rendezvous timeout surfaces here as a failed \
             slot: {:?}",
            result.slots
        );
        assert_eq!(result.fail_fast_skipped_count, 0);

        let observed_peak = peak.load(Ordering::SeqCst);
        // `<= cap` is the invariant this test exists for (the semaphore pair enforces it). `>= cap`
        // is now equally deterministic rather than a wall-clock race: every one of the first `cap`
        // workers increments `running` BEFORE it waits at the rendezvous, and none of them is
        // released until all `cap` have arrived, so the peak provably reaches `cap`. Asserting the
        // equality therefore also catches the failure mode a bare `<=` cannot — a pool that
        // silently under-dispatches and never reaches the concurrency it was configured for.
        assert_eq!(
            observed_peak, cap,
            "exactly {cap} real child processes must be concurrently alive at the peak — no more \
             (the group cap) and no fewer (the rendezvous forces all {cap} to overlap)"
        );
        assert_eq!(
            running.load(Ordering::SeqCst),
            0,
            "every real child must have been confirmed exited by the time run_bounded returns"
        );
    }

    /// R-SA-050: the run-wide global ceiling additionally bounds concurrency even when a group's
    /// own local `concurrency` would otherwise allow more — proven by racing TWO groups against
    /// the SAME [`GlobalConcurrencyLimit`] and asserting the peak concurrent REAL child count
    /// across BOTH groups combined never exceeds the smaller, global cap.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn global_ceiling_bounds_concurrency_across_two_concurrent_groups() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let global_cap = 2usize;
        let global = GlobalConcurrencyLimit::new(global_cap);

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let make_group = |group_id: &'static str| {
            let dir_path = dir.path().to_path_buf();
            let running = Arc::clone(&running);
            let peak = Arc::clone(&peak);
            let global = global.clone();
            async move {
                let tasks: Vec<()> = (0..4).map(|_| ()).collect();
                let cancel = cyrup_core::CancelToken::new();
                let running_for_worker = Arc::clone(&running);
                let peak_for_worker = Arc::clone(&peak);
                run_bounded::<(), i32, String, _, _>(
                    tasks,
                    // Each group's OWN local cap (4) is deliberately larger than the shared
                    // global cap (2), so if R-SA-050's global ceiling were NOT actually enforced,
                    // this test would observe a peak > 2.
                    4,
                    &global,
                    false,
                    cancel,
                    move |index, ()| {
                        let dir_path = dir_path.join(group_id);
                        let running = Arc::clone(&running_for_worker);
                        let peak = Arc::clone(&peak_for_worker);
                        async move {
                            let _ = std::fs::create_dir_all(&dir_path);
                            real_child_worker(dir_path, running, peak, index, 150).await
                        }
                    },
                )
                .await
            }
        };

        let (result_a, result_b) = tokio::join!(make_group("group-a"), make_group("group-b"));

        assert!(!result_a.any_failed);
        assert!(!result_b.any_failed);

        let observed_peak = peak.load(Ordering::SeqCst);
        assert!(
            observed_peak <= global_cap,
            "the SHARED global ceiling ({global_cap}) must bound concurrency across BOTH groups \
             combined even though each group's own local cap (4) is individually larger, \
             observed peak {observed_peak}"
        );
    }

    // -------------------------------------------------------------------------------------------
    // R-SA-051: result ordering preserved regardless of completion order.
    // -------------------------------------------------------------------------------------------

    /// Tasks are deliberately given DECREASING sleep durations (task 0 sleeps longest, the last
    /// task sleeps shortest) so completion order is the REVERSE of input order — yet the returned
    /// `slots` Vec must still be indexed by ORIGINAL input position.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn result_ordering_matches_input_order_despite_reverse_completion_order() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let total_tasks = 5usize;
        // Sleep durations chosen so task i sleeps for (total_tasks - i) * 120ms — task 0 finishes
        // LAST, task (total_tasks - 1) finishes FIRST, a genuine out-of-order completion pattern.
        let sleep_ms_for = move |i: usize| ((total_tasks - i) as u64) * 120;

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let completion_order = Arc::new(std::sync::Mutex::new(Vec::<usize>::new()));

        let tasks: Vec<usize> = (0..total_tasks).collect();
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();

        let dir_path = dir.path().to_path_buf();
        let running_for_worker = Arc::clone(&running);
        let peak_for_worker = Arc::clone(&peak);
        let completion_order_for_worker = Arc::clone(&completion_order);

        let result: FanOutResult<usize, String> = run_bounded(
            tasks,
            // Full concurrency (no artificial serialization) so completion order genuinely
            // reflects each task's own sleep duration rather than pool-scheduling order.
            total_tasks,
            &global,
            false,
            cancel,
            move |index, original_value| {
                let dir_path = dir_path.clone();
                let running = Arc::clone(&running_for_worker);
                let peak = Arc::clone(&peak_for_worker);
                let completion_order = Arc::clone(&completion_order_for_worker);
                async move {
                    let sleep_ms = sleep_ms_for(index);
                    let exit_code =
                        real_child_worker(dir_path, running, peak, index, sleep_ms).await?;
                    if let Ok(mut order) = completion_order.lock() {
                        order.push(index);
                    }
                    if exit_code == 0 {
                        Ok(original_value)
                    } else {
                        Err(format!("nonzero exit for task {index}"))
                    }
                }
            },
        )
        .await;

        assert_eq!(result.slots.len(), total_tasks);
        assert!(!result.any_failed);

        // The load-bearing assertion: slot i's result value must be i, for every i, regardless of
        // when that task's real child process actually finished.
        for (expected_index, slot) in result.slots.iter().enumerate() {
            let outcome = slot
                .as_ref()
                .expect("slot must be populated")
                .as_ref()
                .expect("slot must have succeeded");
            assert_eq!(
                outcome.index, expected_index,
                "SlotOutcome::index must match its Vec position"
            );
            assert_eq!(
                outcome.result, expected_index,
                "slot {expected_index}'s value must be the ORIGINAL task's own value, not \
                 whatever happened to complete at that position in time"
            );
        }

        // Sanity-check the test actually exercised genuine out-of-order completion (otherwise the
        // ordering assertion above would be vacuous) — the observed completion order must NOT
        // equal the input order.
        let observed_completion_order = completion_order.lock().expect("lock").clone();
        assert_ne!(
            observed_completion_order,
            (0..total_tasks).collect::<Vec<_>>(),
            "the test's own sleep durations must produce genuine out-of-order completion, got \
             completion order {observed_completion_order:?}"
        );
    }

    // -------------------------------------------------------------------------------------------
    // R-SA-066: cooperative, not preemptive, fail_fast.
    // -------------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn fail_fast_stops_new_dispatch_but_never_kills_an_already_running_sibling() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let total_tasks = 6usize;

        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let dir_path = dir.path().to_path_buf();

        // Task 0 fails almost immediately (real `sh` exiting 1). Task 1 is a real, already-running
        // long-sleeping sibling child that fail_fast must NOT kill — proven by observing it runs
        // to genuine completion (its own real exit code recorded) rather than being cut short.
        // Concurrency is capped at 2 so tasks 0 and 1 dispatch together, and by the time task 0's
        // failure trips fail_fast, task 1 is already a live, running child.
        let tasks: Vec<usize> = (0..total_tasks).collect();

        let result: FanOutResult<i32, String> = run_bounded(
            tasks,
            2,
            &global,
            true, // fail_fast
            cancel,
            move |index, _| {
                let dir_path = dir_path.clone();
                async move {
                    let sleep_ms = if index == 0 { 0 } else { 800 };
                    let exit_code_script = if index == 0 {
                        "exit 1".to_string()
                    } else {
                        format!("sleep {}; exit 0", sleep_ms as f64 / 1000.0)
                    };
                    let spec = crate::spawn::ChildSpawnSpec {
                        command: sh_command(&exit_code_script),
                        args: Vec::new(),
                        task_arg: String::new(),
                        env_overlay: std::collections::HashMap::new(),
                        cwd: dir_path.clone(),
                        temp_files: Vec::new(),
                    };
                    let jsonl_path = dir_path.join(format!("failfast-{index}.jsonl"));
                    let mut child = crate::spawn::SpawnedChild::spawn(spec, &jsonl_path)
                        .await
                        .map_err(|e| e.to_string())?;
                    while child.next_event().await.is_some() {}
                    let status = child
                        .wait_final_drain()
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "child did not exit".to_string())?;
                    child.finish();
                    let code = status.code().unwrap_or(-1);
                    if code == 0 {
                        Ok(code)
                    } else {
                        Err(format!("task {index} exited {code}"))
                    }
                }
            },
        )
        .await;

        assert!(result.any_failed, "task 0's real nonzero exit must surface as a failure");
        assert!(
            result.slots[0].as_ref().expect("slot 0 populated").is_err(),
            "task 0 itself failed"
        );
        assert!(
            result.slots[1].as_ref().expect("slot 1 populated").is_ok(),
            "task 1 (already dispatched as a real running child before fail_fast tripped) must \
             be allowed to run to its own genuine completion, never killed mid-flight by \
             fail_fast — R-SA-066 forbids preemptive kill of already-running siblings"
        );

        // At least one LATER task must have been skipped due to fail_fast — proving the
        // cooperative "stop claiming new work" half of R-SA-066 actually took effect.
        assert!(
            result.fail_fast_skipped_count >= 1,
            "at least one not-yet-dispatched task must be skipped once fail_fast trips, got {} \
             skipped",
            result.fail_fast_skipped_count
        );
        let skipped_indices: Vec<usize> = result
            .slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.is_none())
            .map(|(i, _)| i)
            .collect();
        assert!(
            !skipped_indices.is_empty() && skipped_indices.iter().all(|&i| i >= 2),
            "only tasks that had not yet been dispatched (index >= 2, since 0 and 1 dispatch \
             together under cap 2) may be skipped, got skipped indices {skipped_indices:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_fail_fast_a_failing_task_does_not_stop_dispatch_of_later_tasks() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let dir_path = dir.path().to_path_buf();

        let tasks: Vec<usize> = (0..4).collect();
        let result: FanOutResult<i32, String> = run_bounded(
            tasks,
            4,
            &global,
            false, // fail_fast disabled
            cancel,
            move |index, _| {
                let dir_path = dir_path.clone();
                async move {
                    let script = if index == 0 { "exit 1" } else { "exit 0" };
                    let spec = crate::spawn::ChildSpawnSpec {
                        command: sh_command(script),
                        args: Vec::new(),
                        task_arg: String::new(),
                        env_overlay: std::collections::HashMap::new(),
                        cwd: dir_path.clone(),
                        temp_files: Vec::new(),
                    };
                    let jsonl_path = dir_path.join(format!("no-failfast-{index}.jsonl"));
                    let mut child = crate::spawn::SpawnedChild::spawn(spec, &jsonl_path)
                        .await
                        .map_err(|e| e.to_string())?;
                    while child.next_event().await.is_some() {}
                    let status = child
                        .wait_final_drain()
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "child did not exit".to_string())?;
                    child.finish();
                    let code = status.code().unwrap_or(-1);
                    if code == 0 {
                        Ok(code)
                    } else {
                        Err(format!("task {index} exited {code}"))
                    }
                }
            },
        )
        .await;

        assert!(result.any_failed);
        assert_eq!(
            result.fail_fast_skipped_count, 0,
            "without fail_fast, nothing is ever skipped for that reason"
        );
        for (i, slot) in result.slots.iter().enumerate() {
            assert!(
                slot.is_some(),
                "every task must have been dispatched when fail_fast is disabled, slot {i} was \
                 never populated"
            );
        }
    }

    // -------------------------------------------------------------------------------------------
    // R-SA-069: duplicate-dispatch guard.
    // -------------------------------------------------------------------------------------------

    #[test]
    fn dispatch_guard_rejects_a_concurrent_duplicate_from_the_same_call_site() {
        let guard = DispatchGuard::new();
        let first = guard.try_acquire().expect("first dispatch claims the slot");
        let second = guard.try_acquire();
        assert!(
            second.is_none(),
            "a concurrent duplicate dispatch from the same call site must be rejected while the \
             first is still in progress"
        );
        drop(first);
        let third = guard.try_acquire();
        assert!(
            third.is_some(),
            "once the prior dispatch's token is dropped (completed), a new dispatch must be \
             allowed"
        );
    }

    #[test]
    fn dispatch_guard_try_acquire_or_reject_surfaces_a_subagent_error() {
        let guard = DispatchGuard::new();
        let _held = guard.try_acquire().expect("first dispatch claims the slot");
        let rejected = guard.try_acquire_or_reject("tool:subagent");
        assert!(matches!(rejected, Err(SubagentError::AcceptanceRejected(_))));
    }

    /// R-SA-069's other half: rejecting a concurrent duplicate top-level call must NOT affect
    /// intentional parallel-mode dispatch WITHIN one already-accepted call — proven by holding one
    /// [`DispatchGuard`] token for the whole duration of a real `run_bounded` fan-out and asserting
    /// that fan-out still achieves genuine concurrency across its own real child processes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dispatch_guard_does_not_interfere_with_parallel_fanout_within_one_dispatch() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let guard = DispatchGuard::new();
        let _token = guard
            .try_acquire()
            .expect("the one accepted top-level dispatch holds this token for its duration");

        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<()> = (0..4).map(|_| ()).collect();
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let dir_path = dir.path().to_path_buf();
        let running_for_worker = Arc::clone(&running);
        let peak_for_worker = Arc::clone(&peak);
        // SUBA-033: two is the concurrency this test's claim is about, so that is the rendezvous
        // size. It is one-shot, so the two workers that do NOT take part (4 tasks, 2 gated) pass
        // straight through rather than waiting on a generation that will never fill.
        let rendezvous = Arc::new(FirstArrivalsRendezvous::new(2));

        let result: FanOutResult<i32, String> = run_bounded(
            tasks,
            4,
            &global,
            false,
            cancel,
            move |index, ()| {
                let dir_path = dir_path.clone();
                let running = Arc::clone(&running_for_worker);
                let peak = Arc::clone(&peak_for_worker);
                let rendezvous = Arc::clone(&rendezvous);
                async move {
                    real_child_worker_rendezvous(
                        dir_path,
                        running,
                        peak,
                        index,
                        150,
                        Some(rendezvous),
                    )
                    .await
                }
            },
        )
        .await;

        // SUBA-033: the claim — "the dispatch guard being held must not serialize the intentional
        // in-call parallel fan-out down to peak concurrency 1" — is now enforced by the rendezvous
        // itself: two workers cannot pass the barrier unless two are dispatched concurrently, and a
        // serializing regression fails the run with the rendezvous-timeout message rather than
        // flaking on an observed peak.
        assert!(
            !result.any_failed,
            "a serialized fan-out surfaces as a rendezvous timeout in a failed slot: {:?}",
            result.slots
        );
        // The rendezvous already makes this structural, so stating it is free and makes the claim
        // the test's name asserts readable in the assertion itself rather than only in a comment.
        let observed_peak = peak.load(Ordering::SeqCst);
        assert!(
            observed_peak >= 2,
            "holding the top-level DispatchGuard token must not serialize the in-call fan-out; \
             observed peak concurrent children {observed_peak}"
        );
        assert_eq!(
            running.load(Ordering::SeqCst),
            0,
            "every real child must have been confirmed exited by the time run_bounded returns"
        );
    }

    // -------------------------------------------------------------------------------------------
    // Cancellation: no new task dispatched once cancelled; already-populated slots preserved.
    // -------------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_stops_new_dispatch_and_marks_unclaimed_slots() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let total_tasks = 6usize;
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let cancel_for_trigger = cancel.clone();
        let dir_path = dir.path().to_path_buf();

        let dispatched_count = Arc::new(AtomicI64::new(0));
        let dispatched_for_worker = Arc::clone(&dispatched_count);

        let tasks: Vec<usize> = (0..total_tasks).collect();

        // Cancel shortly after the pool starts, giving only the first wave of (cap-bounded)
        // workers a chance to claim their initial index before later claims observe cancellation.
        let cancel_trigger = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            cancel_for_trigger.cancel();
        });

        let result: FanOutResult<i32, String> = run_bounded(
            tasks,
            2,
            &global,
            false,
            cancel,
            move |index, _| {
                let dir_path = dir_path.clone();
                let dispatched = Arc::clone(&dispatched_for_worker);
                async move {
                    dispatched.fetch_add(1, Ordering::SeqCst);
                    let spec = crate::spawn::ChildSpawnSpec {
                        command: sh_command("sleep 0.3; exit 0"),
                        args: Vec::new(),
                        task_arg: String::new(),
                        env_overlay: std::collections::HashMap::new(),
                        cwd: dir_path.clone(),
                        temp_files: Vec::new(),
                    };
                    let jsonl_path = dir_path.join(format!("cancel-{index}.jsonl"));
                    let mut child = crate::spawn::SpawnedChild::spawn(spec, &jsonl_path)
                        .await
                        .map_err(|e| e.to_string())?;
                    while child.next_event().await.is_some() {}
                    let status = child
                        .wait_final_drain()
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "child did not exit".to_string())?;
                    child.finish();
                    Ok(status.code().unwrap_or(-1))
                }
            },
        )
        .await;

        cancel_trigger.await.expect("trigger task completes");

        let unclaimed = result.slots.iter().filter(|s| s.is_none()).count();
        assert!(
            unclaimed > 0,
            "cancellation partway through must leave at least one later task never dispatched, \
             got 0 unclaimed slots out of {total_tasks}"
        );
        assert!(
            (dispatched_count.load(Ordering::SeqCst) as usize) < total_tasks,
            "cancellation must have prevented every task from being dispatched"
        );
    }

    // -------------------------------------------------------------------------------------------
    // Edge cases.
    // -------------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_task_list_returns_an_empty_result_without_spawning_anything() {
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let tasks: Vec<()> = Vec::new();
        let result: FanOutResult<(), String> =
            run_bounded(tasks, 4, &global, false, cancel, |_, ()| async { Ok(()) }).await;
        assert!(result.slots.is_empty());
        assert!(!result.any_failed);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn single_task_completes_with_group_concurrency_of_one() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let dir_path = dir.path().to_path_buf();

        let result: FanOutResult<i32, String> = run_bounded(
            vec![()],
            1,
            &global,
            false,
            cancel,
            move |index, ()| {
                let dir_path = dir_path.clone();
                let running = Arc::clone(&running);
                let peak = Arc::clone(&peak);
                async move { real_child_worker(dir_path, running, peak, index, 10).await }
            },
        )
        .await;

        assert_eq!(result.slots.len(), 1);
        assert!(result.slots[0].as_ref().expect("populated").is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn zero_group_concurrency_still_makes_progress_serially_rather_than_hanging() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let global = GlobalConcurrencyLimit::default_limit();
        let cancel = cyrup_core::CancelToken::new();
        let dir_path = dir.path().to_path_buf();
        let running_for_worker = Arc::clone(&running);
        let peak_for_worker = Arc::clone(&peak);

        let result: FanOutResult<i32, String> = tokio::time::timeout(
            Duration::from_secs(10),
            run_bounded(
                vec![(), ()],
                0, // misconfigured: must not silently dispatch nothing
                &global,
                false,
                cancel,
                move |index, ()| {
                    let dir_path = dir_path.clone();
                    let running = Arc::clone(&running_for_worker);
                    let peak = Arc::clone(&peak_for_worker);
                    async move { real_child_worker(dir_path, running, peak, index, 10).await }
                },
            ),
        )
        .await
        .expect("group_concurrency: 0 must not hang forever");

        assert_eq!(result.slots.len(), 2);
        assert!(result.slots.iter().all(|s| s.is_some()));
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "group_concurrency: 0 degrades to serial (one worker), never zero workers"
        );
    }
}
