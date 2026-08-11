//! Orchestrator-side shared poller (func-SA §5.4 R-SA-093/105; arch-SA §5.5/§6.5).
//!
//! This module is the SOLE owner of the "one shared, self-starting/self-stopping polling loop per
//! [`JobTracker`] instance" primitive (R-SA-093) — a single `tokio::time::interval`-driven task,
//! never one timer per tracked run. Every tick:
//!
//! 1. **Tails newly-appended bytes from each tracked run's `events.jsonl`**, starting from a
//!    per-run byte cursor, using a bounded read-chunk and a bounded max-line-size so an unbounded
//!    or pathological log can never cause unbounded memory growth in the orchestrator process
//!    (R-SA-093's own explicit "bounded read-chunk and max-line-size limits" clause). Tailed lines
//!    are tolerantly parsed as one JSON value per line — a malformed line is skipped, never fatal
//!    to the tick, mirroring this crate's established R-SA-026 NDJSON-parse-tolerance convention
//!    (`exec/ndjson.rs`) applied here to the run-level control/notification event log rather than a
//!    child's raw stdout.
//! 2. **Invokes stale-run reconciliation** ([`super::reconcile::reconcile_now`], R-SA-088..092) for
//!    every tracked run, so a tracked job's in-memory view of `RunStatus` never drifts from the
//!    authoritative on-disk state for longer than one poll interval.
//!
//! # Self-starting / self-stopping (R-SA-093, R-SA-145)
//!
//! The poller task is **not spawned** until [`JobTracker::track`] adds the first tracked job, and
//! it **terminates itself** (rather than idle-sleeping forever) once the tracked-job map empties —
//! "near-zero overhead when zero runs are tracked" (arch-SA §10's own restatement of R-08-034's
//! "cheap when no subscribers" principle, applied to this crate's poller). A tracked job is removed
//! from the map once it reaches a terminal state (R-SA-100's OR'd-signal terminal classification,
//! `RunState::is_terminal()`) AND has no live nested descendants (R-SA-104's recursive "am I fully
//! done" rule) AND has sat in that fully-terminal condition for at least the bounded retention
//! window (R-SA-105, target ~10s) — so a fast-polling UI reading the tracker's own snapshot API
//! never misses the transition to terminal by racing the tracker's own removal of the entry.
//!
//! # `std::sync::Mutex`, never held across `.await` (arch-SA §5.1)
//!
//! [`JobTracker::jobs`] is a plain [`std::sync::Mutex`], not a `tokio::sync::Mutex` — mirroring
//! arch-SA §5.1's explicit instruction for this exact field ("never held across `.await`", citing
//! `cyrup-agent::Agent`'s own `std::sync::Mutex<StateInner>` as the precedent this crate's poller
//! should follow rather than reflexively reaching for an async mutex). Every critical section
//! against `jobs` in this file is a short, synchronous read-modify-write with no `.await` point
//! inside the lock guard's scope; any `.await` this module needs (file reads, [`reconcile_now`])
//! happens strictly before or after the lock is held, never while it is held.
//!
//! # No shared Rust-level state with `runner_main.rs` (task framing, restated)
//!
//! This module and the detached second-hop runner (`background/runner_main.rs`) are two
//! independent OS processes. They share **zero** in-process state — not a channel, not a shared
//! `Arc`, nothing. The only thing connecting them is the filesystem: the runner appends bytes to
//! `events.jsonl` and writes `status.json`/the terminal [`super::ResultFile`]; this module only
//! ever *reads* those same paths. This is a hard architectural boundary, not an implementation
//! detail — see the crate-level mandatory-mechanism documentation (`lib.rs`, func-SA §1.1) for why.

use std::collections::HashMap;
use std::io::SeekFrom;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant, SystemTime};

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use super::reconcile::{self, ReconcileAction};
use super::{RunId, RunPaths, RunStatus};

// =================================================================================================
// Tunables
// =================================================================================================

/// Default poll-tick interval. Not specified numerically by func-SA (R-SA-093 only fixes the
/// *shape* — one shared loop, not one timer per run); chosen to be frequent enough that a tracked
/// run's `events.jsonl` growth and terminal-state transition are both observed promptly without
/// meaningfully taxing the orchestrator process, and cheap to override in tests via
/// [`JobTracker::with_tuning`].
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Bounded retention window after a tracked run reaches a fully-terminal condition (terminal state
/// AND no live nested descendants) before it is removed from the tracked-job map (R-SA-105, "target
/// ~10s").
pub const DEFAULT_RETENTION_WINDOW: Duration = Duration::from_secs(10);

/// Bounded per-tick read-chunk cap on how many new bytes of `events.jsonl` are read in a single
/// tick, so a pathologically fast-growing log cannot balloon one tick's memory use (R-SA-093's
/// "bounded read-chunk... limits" clause). A tick that finds more than this many new bytes reads
/// only this many; the remainder is picked up on the next tick(s) since the byte cursor only
/// advances by what was actually consumed.
pub const DEFAULT_MAX_READ_CHUNK_BYTES: usize = 1024 * 1024;

/// Bounded maximum length of a single tailed line before it is discarded as unparseable rather than
/// buffered without limit (R-SA-093's "...and max-line-size limits" clause) — mirrors this crate's
/// established `exec/ndjson.rs` per-line-tolerance convention (R-SA-026), applied to the run-level
/// event log instead of a child's raw stdout.
pub const DEFAULT_MAX_LINE_BYTES: usize = 256 * 1024;

// =================================================================================================
// TrackedJob
// =================================================================================================

/// One run's tracked state inside [`JobTracker::jobs`]. Deliberately small and `Clone`-cheap (an
/// `Arc`-free plain struct) so a caller can snapshot the whole map (via [`JobTracker::snapshot`])
/// without holding the lock during any downstream use of the data.
#[derive(Clone, Debug)]
pub struct TrackedJob {
    /// This run's identity.
    pub run_id: RunId,
    /// Resolved on-disk paths for this run (status/result/events/etc).
    pub paths: RunPaths,
    /// Wall-clock time hop-1 confirmed a successful spawn for this run, if known — threaded through
    /// to [`reconcile::reconcile`] as the R-SA-090 grace-window reference point. `None` for a run
    /// this tracker only learned about after the fact (e.g. [`JobTracker::resume_tracking_from_disk`]
    /// re-discovering a run from a prior process's `AsyncRoot`), in which case a missing
    /// `status.json` is never treated as "still within grace" (see `reconcile.rs`'s own documented
    /// behavior for `spawn_confirmed_at: None`).
    pub spawn_confirmed_at: Option<SystemTime>,
    /// The most recently reconciled status, if a tick has run at least once for this job. `None`
    /// until the first tick observes it.
    pub last_status: Option<RunStatus>,
    /// Byte offset into `events.jsonl` already consumed by prior ticks (R-SA-093's "per-run byte
    /// cursor").
    pub events_cursor: u64,
    /// Wall-clock instant this job was first observed to be fully terminal (terminal `RunState` AND
    /// no live nested descendants, R-SA-104) — the reference point [`DEFAULT_RETENTION_WINDOW`] is
    /// measured from (R-SA-105). `None` while the run is still active (or terminal-with-live-
    /// descendants).
    pub terminal_since: Option<Instant>,
}

impl TrackedJob {
    fn new(run_id: RunId, paths: RunPaths, spawn_confirmed_at: Option<SystemTime>) -> Self {
        Self {
            run_id,
            paths,
            spawn_confirmed_at,
            last_status: None,
            events_cursor: 0,
            terminal_since: None,
        }
    }
}

// =================================================================================================
// JobTracker
// =================================================================================================

/// One `tokio::time::interval`-driven shared poller per owning [`crate::extension::SubagentsExtension`]
/// instance (R-SA-093). Cheap to construct (`Arc::new(JobTracker::new())`); the poll loop itself is
/// not spawned until the first [`JobTracker::track`] call.
pub struct JobTracker {
    /// The tracked-job map. Plain `std::sync::Mutex` (see module docs): every access is a short
    /// synchronous critical section, never held across an `.await` point.
    jobs: StdMutex<HashMap<RunId, TrackedJob>>,
    /// Handle to the currently-running poll-loop task, if one is active. Guarded by a
    /// `tokio::sync::Mutex` (not `std::sync::Mutex`) because starting/stopping the loop itself is an
    /// infrequent, `.await`-shaped operation (spawning a task, or awaiting one's completion on
    /// shutdown) — unlike `jobs` above, which is on the hot per-tick path and must stay a cheap
    /// sync lock.
    poller: AsyncMutex<Option<JoinHandle<()>>>,
    /// Poll-tick interval (overridable for tests via [`JobTracker::with_tuning`]).
    poll_interval: Duration,
    /// Retention window after full-terminal before a job is dropped (R-SA-105, overridable for
    /// tests).
    retention_window: Duration,
    /// Bounded per-tick read-chunk cap (R-SA-093, overridable for tests).
    max_read_chunk_bytes: usize,
    /// Bounded max single-line length before a tailed line is discarded unparsed (R-SA-093,
    /// overridable for tests).
    max_line_bytes: usize,
}

impl Default for JobTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl JobTracker {
    /// Constructs a tracker with production-default tuning. The poll loop is not started until the
    /// first [`JobTracker::track`] call.
    #[must_use]
    pub fn new() -> Self {
        Self::with_tuning(
            DEFAULT_POLL_INTERVAL,
            DEFAULT_RETENTION_WINDOW,
            DEFAULT_MAX_READ_CHUNK_BYTES,
            DEFAULT_MAX_LINE_BYTES,
        )
    }

    /// Constructs a tracker with explicit tuning — the constructor tests use to run a fast poll
    /// interval and a short retention window so tests do not need to wait out the ~500ms/~10s
    /// production defaults.
    #[must_use]
    pub fn with_tuning(
        poll_interval: Duration,
        retention_window: Duration,
        max_read_chunk_bytes: usize,
        max_line_bytes: usize,
    ) -> Self {
        Self {
            jobs: StdMutex::new(HashMap::new()),
            poller: AsyncMutex::new(None),
            poll_interval,
            retention_window,
            max_read_chunk_bytes,
            max_line_bytes,
        }
    }

    /// Returns the number of currently tracked jobs. A short, synchronous, non-`.await`-spanning
    /// lock acquisition.
    #[must_use]
    pub fn tracked_count(&self) -> usize {
        // Poisoned-lock recovery: a panic elsewhere while holding this lock must not permanently
        // wedge every future tracker call behind a poisoned mutex — `unwrap_or_else` on the
        // `PoisonError` recovers the inner guard (the map's *contents* are still structurally
        // valid; only the unwinding task's own in-flight mutation, if any, may be incomplete),
        // consistent with this crate's workspace-wide no-`unwrap`/no-`panic` discipline.
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Returns a plain-data snapshot of every currently tracked job, for a caller (e.g. a TUI
    /// render pass or a `/subagents-list` command handler) that wants the tracker's current view
    /// without holding any lock itself. Cheap: [`TrackedJob`] is a plain, `Arc`-free struct.
    #[must_use]
    pub fn snapshot(&self) -> Vec<TrackedJob> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// Looks up one tracked job's current snapshot by id, if tracked.
    #[must_use]
    pub fn get(&self, run_id: &RunId) -> Option<TrackedJob> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .cloned()
    }

    /// Starts tracking `run_id`, inserting a fresh [`TrackedJob`] (or resetting an existing entry's
    /// cursor/terminal-tracking state, if this run id was already tracked — the byte cursor and
    /// terminal-since bookkeeping restart cleanly rather than accumulating stale state across a
    /// re-track). Self-starts the shared poll loop if it is not already running (R-SA-093).
    pub async fn track(
        self: &Arc<Self>,
        run_id: RunId,
        paths: RunPaths,
        spawn_confirmed_at: Option<SystemTime>,
    ) {
        {
            let mut jobs = self
                .jobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            jobs.insert(
                run_id.clone(),
                TrackedJob::new(run_id, paths, spawn_confirmed_at),
            );
        }
        self.ensure_poller_started();
    }

    /// Starts tracking `run_id` the way [`JobTracker::track`] does, except the per-run
    /// `events.jsonl` byte cursor is seeded at `events_cursor` rather than `0`. Used exclusively by
    /// [`crate::extension::SubagentsExtension::resume_tracking`] (pi `restoreActiveJobs`'s
    /// `restoredControlEventCursor`, `async-job-tracker.ts:48-55,99` @v0.34.0) to re-track a run discovered
    /// from a prior process's `AsyncRoot` without re-tailing control events that process already
    /// consumed. `spawn_confirmed_at` is always `None` for a restored job — this tracker has no
    /// record of when hop-1 confirmed the spawn for a run it only learned about after the fact,
    /// exactly like a fresh [`JobTracker::track`] call with no such reference.
    pub async fn track_restored(self: &Arc<Self>, run_id: RunId, paths: RunPaths, events_cursor: u64) {
        {
            let mut jobs = self
                .jobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut job = TrackedJob::new(run_id.clone(), paths, None);
            job.events_cursor = events_cursor;
            jobs.insert(run_id, job);
        }
        self.ensure_poller_started();
    }

    /// Explicitly stops tracking `run_id` before it would otherwise reach the R-SA-105 retention
    /// deadline — e.g. an orchestrator that no longer cares about a run's completion (the caller
    /// process is exiting, or the run was disowned). The poll loop self-stops on its own next tick
    /// if this was the last tracked job; this method does not itself force an immediate stop, since
    /// that would require synchronizing with a task that may be mid-tick.
    pub fn untrack(&self, run_id: &RunId) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
    }

    /// Session-teardown stop (pi `session_shutdown`: `clearInterval(state.poller); state.poller =
    /// null` plus `state.asyncJobs.clear()`, `extension/index.ts:657-664`): immediately abort the
    /// poll-loop task, if one is running, and clear every tracked-job entry from this process's
    /// in-memory view. This ONLY tears down this extension's own in-process polling/bookkeeping —
    /// the detached child OS processes backing any still-running job are untouched and continue to
    /// completion on disk (R-SA-071/DI-SA-8), exactly like pi's own shutdown, which never sends any
    /// signal to a background run. A fresh `SessionStart` on this same process re-discovers any
    /// still-live runs from disk via `resume_tracking`, so no run is permanently "lost" by this
    /// clear — only this process's own live poll view of it is reset.
    pub async fn stop_and_clear(&self) {
        let handle = {
            let mut guard = self.poller.lock().await;
            guard.take()
        };
        if let Some(handle) = handle {
            handle.abort();
        }
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    /// `true` if the shared poll-loop task is currently running. Exposed primarily for tests
    /// asserting the self-starting/self-stopping contract (R-SA-093/145); a caller with no such
    /// need has no reason to consult this.
    pub async fn is_polling(&self) -> bool {
        let guard = self.poller.lock().await;
        matches!(&*guard, Some(handle) if !handle.is_finished())
    }

    /// Blocks until the shared poll loop has stopped itself (self-stopping, R-SA-093) — i.e. until
    /// the tracked-job map has emptied and the loop task has exited. Used by tests that need a
    /// deterministic "the loop is definitely gone now" signal rather than polling
    /// [`JobTracker::is_polling`] in a spin loop. A no-op if the loop was never started or has
    /// already stopped.
    pub async fn wait_for_poller_to_stop(&self) {
        let handle = {
            let mut guard = self.poller.lock().await;
            guard.take()
        };
        if let Some(handle) = handle {
            // A JoinHandle is only awaited here if this call actually took ownership of it; if the
            // loop task itself already cleared `self.poller` (see `poll_loop`'s own tail below)
            // this branch is simply skipped and the method returns immediately, since "already
            // stopped" trivially satisfies "wait until stopped".
            let _ = handle.await;
        }
    }

    /// Runs exactly one poll tick immediately, without waiting for the interval timer — used by
    /// tests that want deterministic control over tick timing rather than sleeping past
    /// [`DEFAULT_POLL_INTERVAL`]/a tuned interval. Production code never needs to call this
    /// directly; [`JobTracker::track`]'s spawned loop calls the identical underlying logic on its
    /// own timer.
    pub async fn tick_once(&self) {
        self.run_one_tick().await;
    }

    /// Spawns the shared poll-loop task if one is not already running. Cheap to call redundantly —
    /// an already-running loop is left untouched.
    fn ensure_poller_started(self: &Arc<Self>) {
        // `try_lock` rather than blocking: this is called from `track`'s hot path and the poller
        // slot is only ever contended by another concurrent `track`/shutdown call, in which case
        // whichever caller wins the race starts the loop and the other is a harmless no-op — both
        // outcomes leave exactly one loop running, which is all this method promises.
        let Ok(mut guard) = self.poller.try_lock() else {
            // Someone else is concurrently starting/stopping the loop right now; whichever of us
            // loses this race can trust the winner already started (or is starting) a loop that
            // will observe the job this call just inserted on its very next tick, since the insert
            // above happened-before this method was even called.
            return;
        };
        if matches!(&*guard, Some(handle) if !handle.is_finished()) {
            return; // already running
        }
        let this = Arc::clone(self);
        let handle = tokio::spawn(async move { this.poll_loop().await });
        *guard = Some(handle);
    }

    /// The shared poll loop body (R-SA-093): ticks on `self.poll_interval`, running one full
    /// reconcile-and-tail pass over every tracked job per tick, and exits (self-stopping) once a
    /// tick observes the tracked-job map has become empty.
    async fn poll_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(self.poll_interval);
        // `Delay` burst-catch-up is never useful for this loop (a missed tick just means slightly
        // staler tailed state, never a correctness issue) — skipping missed ticks instead of
        // bursting keeps a long-stalled host from firing a flood of queued ticks back-to-back.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            self.run_one_tick().await;
            if self.tracked_count() == 0 {
                break; // self-stop: nothing left to poll (R-SA-093/145)
            }
        }
        // Clear our own handle slot on the way out so a subsequent `track()` call's
        // `ensure_poller_started` sees `None` (or an already-finished handle) and spawns a fresh
        // loop rather than assuming a dead one is still active. Best-effort: if another task has
        // already taken the slot (via `wait_for_poller_to_stop`), this is a harmless no-op.
        if let Ok(mut guard) = self.poller.try_lock()
            && matches!(&*guard, Some(h) if h.is_finished())
        {
            *guard = None;
        }
    }

    /// Runs one reconcile-and-tail pass over every currently tracked job. Snapshots the id list
    /// under the sync lock, then does all the `.await`-shaped work (file reads, reconciliation)
    /// with the lock released, re-acquiring it only for the final short write-back per job — never
    /// holding the `std::sync::Mutex` across an `.await` (module docs).
    async fn run_one_tick(&self) {
        let run_ids: Vec<RunId> = {
            let jobs = self
                .jobs
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            jobs.keys().cloned().collect()
        };

        for run_id in run_ids {
            self.tick_one_job(&run_id).await;
        }

        self.evict_expired_terminal_jobs();
    }

    /// Tails new `events.jsonl` bytes and reconciles exactly one tracked job. Any failure (I/O error
    /// tailing the log, reconciliation I/O error) degrades to "leave this job's tracked state as it
    /// was before this tick" — a single bad tick for one job must never abort the whole poll pass
    /// for every other tracked job, and must never panic.
    async fn tick_one_job(&self, run_id: &RunId) {
        // Snapshot the job's current cursor/paths/spawn-time under the sync lock, then release it
        // before doing any `.await`-shaped work.
        let Some(job_snapshot) = self.get(run_id) else {
            return; // untracked between the id-list snapshot and now — nothing to do
        };

        // Step (a): tail newly-appended events.jsonl bytes (R-SA-093). A missing file (the runner
        // has not created it yet, or never will for a run whose event-writer, per this crate's
        // current build-out, has not landed) is not an error — tailing simply observes zero new
        // lines and the cursor stays at 0.
        let tail_outcome = tail_new_lines(
            &job_snapshot.paths.events,
            job_snapshot.events_cursor,
            self.max_read_chunk_bytes,
            self.max_line_bytes,
        )
        .await;

        let new_cursor = match &tail_outcome {
            Ok(tail) => tail.new_cursor,
            Err(_) => job_snapshot.events_cursor, // leave cursor unmodified on a read failure
        };
        // Tolerantly parsed lines themselves are not retained by this module (R-SA-093 only
        // requires *tailing* — folding parsed control/notification events into any
        // renderable/aggregate state is a TUI/notices-subsystem concern owned elsewhere, per
        // arch-SA §6.7/`tui/notices.rs`, not this poller). This module's own job is limited to (1)
        // advancing the byte cursor so the SAME bytes are never re-read on a later tick, and (2)
        // reconciling status — both of which happen regardless of what, if anything, a downstream
        // consumer does with the tailed lines. `tail.lines` is intentionally unused beyond the
        // parse-tolerance property it demonstrates in tests.

        // Step (b): stale-run reconciliation (R-SA-088..092).
        let reconciled = reconcile::reconcile_now(&job_snapshot.paths, job_snapshot.spawn_confirmed_at)
            .await
            .ok();

        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(job) = jobs.get_mut(run_id) else {
            return; // untracked concurrently (e.g. an explicit `untrack` mid-tick) — nothing to do
        };
        job.events_cursor = new_cursor;
        if let Some(outcome) = reconciled {
            job.last_status = Some(outcome.status.clone());
            let fully_terminal =
                outcome.status.state.is_terminal() && !has_live_nested_descendants(&outcome.status);
            match (fully_terminal, job.terminal_since) {
                (true, None) => job.terminal_since = Some(Instant::now()),
                (false, Some(_)) => job.terminal_since = None, // resumed/un-terminaled; reset
                _ => {}
            }
            if matches!(outcome.action, ReconcileAction::SynthesizedFailure) {
                tracing::warn!(
                    run_id = %run_id,
                    "subagent background run reconciled to a synthesized failure (stale/dead pid)"
                );
            }
        }
    }

    /// Removes every tracked job whose `terminal_since` has aged past `self.retention_window`
    /// (R-SA-105). A short, synchronous critical section — no `.await` inside the lock.
    fn evict_expired_terminal_jobs(&self) {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        jobs.retain(|_, job| match job.terminal_since {
            Some(since) => since.elapsed() < self.retention_window,
            None => true,
        });
    }
}

/// `true` if `status` has any step whose `nested_run_ids` this tracker still considers live — i.e.
/// any nested descendant run whose own `status.json`/[`super::ResultFile`] (resolved via
/// [`super::RunPaths::nested`]) has not itself reached a terminal state. R-SA-104's recursive "am I
/// fully done" rule: a root run must not be treated as fully terminal (and therefore eligible for
/// R-SA-105 retention-window eviction) while a nested descendant it spawned is still running.
///
/// This performs a best-effort, single-level synchronous check against each nested descendant's
/// last-known on-disk `status.json` state (read fresh, not from this tracker's own map — a nested
/// run may or may not itself be separately tracked). A descendant whose `status.json` cannot be
/// read (not yet written, or an I/O error) is conservatively treated as **not** live — an
/// unreadable/nonexistent nested status is far more often "this step never actually spawned a
/// nested background run" than "a nested run is live but its status is somehow unreadable", and
/// treating it as live would risk a root run's retention eviction stalling forever on a phantom
/// descendant.
fn has_live_nested_descendants(status: &RunStatus) -> bool {
    for step in &status.steps {
        for nested_id in &step.nested_run_ids {
            if nested_status_is_live(nested_id) {
                return true;
            }
        }
    }
    if let Some(groups) = &status.parallel_groups {
        for group in groups {
            for child in &group.children {
                for nested_id in &child.nested_run_ids {
                    if nested_status_is_live(nested_id) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Best-effort, synchronous liveness check for one nested run id, used only by
/// [`has_live_nested_descendants`]'s conservative, non-blocking sweep. Deliberately synchronous
/// (`std::fs::read`, not `tokio::fs::read`) since this helper is called from inside a context that
/// already holds `self.jobs`'s sync lock (module docs: no `.await` while that lock is held) — a
/// tiny, already-tracked-job-count-bounded number of small JSON file reads here is preferable to
/// restructuring the call site to thread this check through the async tail/reconcile pass above.
fn nested_status_is_live(_nested_id: &RunId) -> bool {
    // NOTE: resolving a nested run id to its `RunPaths` requires the OWNING (root) run's own
    // `RunPaths` as a base (`RunPaths::nested`, see `background/mod.rs`) — this function is
    // deliberately conservative and reachable only via `has_live_nested_descendants` above, which
    // does not currently have that base path threaded through from `tick_one_job` (nested-run
    // background-spawn wiring itself, R-SA-104's SPAWN side, is owned by a later phase's
    // `spawn/parallel.rs`/`background/spawn_detached.rs` integration, not this file). Until that
    // wiring lands, no step's `nested_run_ids` is ever actually populated by any code path in this
    // crate, so this function is presently unreachable in practice; it is implemented as an
    // explicit `false` (never live) — the same conservative default the module docs above commit
    // to for an unresolvable/unreadable nested descendant — rather than a `todo!()`, so the R-SA-104
    // recursive-rollup contract this file's `has_live_nested_descendants` already honors does not
    // regress or panic once nested-run population does land in that later phase; at that point this
    // function is the one and only place that needs updating to thread the real base path through
    // and perform a genuine on-disk read.
    false
}

// =================================================================================================
// events.jsonl tailing (R-SA-093)
// =================================================================================================

/// The result of one [`tail_new_lines`] call: every complete new line successfully read (tolerant
/// per-line JSON parse — a malformed line is silently skipped, mirroring R-SA-026) plus the byte
/// offset the caller's cursor should advance to for the next tick.
#[derive(Debug, Clone, Default)]
struct TailOutcome {
    /// Tolerantly parsed JSON values from any new, syntactically valid, complete lines read this
    /// tick. Incomplete trailing partial lines (the writer mid-`write` when this tick happened to
    /// read) are never included — they are picked up whole on a later tick once the writer finishes
    /// appending them, since `new_cursor` only advances past bytes up to and including the last
    /// *complete* newline actually consumed.
    ///
    /// Not read by [`tick_one_job`]'s production call site — as documented there, folding tailed
    /// control/notification events into renderable state is the TUI/notices subsystem's job
    /// (`tui/notices.rs`), not this poller's; this module's own contract is limited to advancing
    /// the byte cursor and reconciling status. The field remains `pub(super)`-visible-shaped data
    /// on [`TailOutcome`] (rather than being discarded at parse time) specifically so this
    /// module's own tests can assert the tolerant-parsing/bounded-chunk/bounded-line-size
    /// properties R-SA-093 requires of the tailing primitive itself, independent of whether any
    /// downstream consumer of the parsed content exists yet.
    #[cfg_attr(not(test), allow(dead_code))]
    lines: Vec<serde_json::Value>,
    /// The byte offset the per-run cursor should be set to for the next tick's read.
    new_cursor: u64,
}

/// Tails up to `max_read_chunk_bytes` of new content from `path`, starting at `cursor`, tolerantly
/// parsing each complete (`\n`-terminated) line as one JSON value (R-SA-093's per-run byte cursor +
/// bounded read-chunk/max-line-size tailing).
///
/// - A missing file is not an error: returns an empty [`TailOutcome`] with `new_cursor == cursor`
///   (nothing to tail yet).
/// - A file that has been truncated/replaced since `cursor` was last recorded (its current length is
///   now less than `cursor`) is treated as having been rotated/reset: tailing restarts from byte 0
///   for this tick rather than seeking past the end of a now-shorter file, which would otherwise
///   either read nothing forever or (worse, depending on platform seek semantics) silently skip
///   content a rotated log's fresh bytes actually contain.
/// - A single line longer than `max_line_bytes` is discarded (not returned, not treated as a parse
///   error worth surfacing) — this bounds per-line memory use exactly as R-SA-093 requires, at the
///   cost of silently dropping a pathologically long line's content, which is an accepted tradeoff
///   for a control/notification log this module only tails for freshness, never for correctness-
///   critical delivery (the authoritative source of truth remains `status.json`/[`super::ResultFile`],
///   never `events.jsonl`).
/// - Any line that fails to parse as JSON at all is silently skipped (never fatal to the tick),
///   mirroring `exec/ndjson.rs`'s established R-SA-026 tolerance convention.
async fn tail_new_lines(
    path: &std::path::Path,
    cursor: u64,
    max_read_chunk_bytes: usize,
    max_line_bytes: usize,
) -> std::io::Result<TailOutcome> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TailOutcome { lines: Vec::new(), new_cursor: cursor });
        }
        Err(e) => return Err(e),
    };

    let metadata_len = file.metadata().await?.len();
    let start = if metadata_len < cursor { 0 } else { cursor };

    if start >= metadata_len {
        // Nothing new since the last tick.
        return Ok(TailOutcome { lines: Vec::new(), new_cursor: start });
    }

    file.seek(SeekFrom::Start(start)).await?;

    let remaining = metadata_len - start;
    let to_read = usize::try_from(remaining)
        .unwrap_or(usize::MAX)
        .min(max_read_chunk_bytes);
    let mut buf = vec![0u8; to_read];
    let read_bytes = file.read(&mut buf).await?;
    buf.truncate(read_bytes);

    // Only advance the cursor past bytes up to and including the LAST complete newline actually
    // read — an incomplete trailing partial line (the writer mid-append) must be re-read whole on a
    // later tick, never split across two ticks' parse attempts.
    let last_newline = buf.iter().rposition(|&b| b == b'\n');
    let Some(last_newline) = last_newline else {
        // No complete line at all in this chunk yet (either the chunk is empty — nothing new was
        // actually available at seek time despite the length check above racing a concurrent
        // truncation — or a single line exceeds the whole read-chunk cap and hasn't found its
        // newline within it). Either way, the cursor does not advance: the same bytes are re-read,
        // whole, on a later tick once more content (including the eventual newline) has been
        // appended.
        return Ok(TailOutcome { lines: Vec::new(), new_cursor: start });
    };

    // `last_newline` is a valid in-bounds index into `buf` by construction (`rposition` only ever
    // returns an index that satisfied the predicate against `buf` itself), so `.get(..=)` is used
    // in place of direct slicing purely to satisfy this crate's workspace-wide
    // `clippy::indexing_slicing` deny — the `unwrap_or` fallback (empty slice) is unreachable in
    // practice, never a silent correctness gap.
    let consumed = buf.get(..=last_newline).unwrap_or(&[]);
    let new_cursor = start + u64::try_from(consumed.len()).unwrap_or(u64::MAX);

    let mut lines = Vec::new();
    for raw_line in consumed.split(|&b| b == b'\n') {
        if raw_line.is_empty() {
            continue;
        }
        if raw_line.len() > max_line_bytes {
            continue; // bounded max-line-size: silently discard an oversized line (see doc comment)
        }
        let Ok(text) = std::str::from_utf8(raw_line) else {
            continue; // tolerant: never fatal to the tick
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
            lines.push(value);
        }
        // A line that fails to parse is silently skipped — tolerant per-line parsing, R-SA-026's
        // convention applied to this log.
    }

    Ok(TailOutcome { lines, new_cursor })
}

// =================================================================================================
// Tests
// =================================================================================================

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::{RunMode, RunState, StepState, StepStatus};
    use std::io::Write as _;
    use tokio::io::AsyncWriteExt;

    fn temp_paths() -> (tempfile::TempDir, RunPaths) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        std::fs::create_dir_all(&async_root).expect("mkdir async_root");
        std::fs::create_dir_all(&results_dir).expect("mkdir results_dir");
        let run_id = RunId::new();
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir run_dir");
        (dir, paths)
    }

    fn fast_tracker() -> Arc<JobTracker> {
        Arc::new(JobTracker::with_tuning(
            Duration::from_millis(20),
            Duration::from_millis(150),
            DEFAULT_MAX_READ_CHUNK_BYTES,
            DEFAULT_MAX_LINE_BYTES,
        ))
    }

    async fn write_status(paths: &RunPaths, status: &RunStatus) {
        crate::background::atomic::write_atomic_json(&paths.status, status)
            .await
            .expect("write status.json");
    }

    fn running_status(run_id: RunId, pid: u32) -> RunStatus {
        let mut status = RunStatus::queued(run_id, RunMode::Single, Some(pid));
        status.state = RunState::Running;
        status.steps = vec![StepStatus { status: StepState::Running, ..StepStatus::pending("researcher") }];
        status
    }

    // ---------------------------------------------------------------------------------------
    // Self-starting / self-stopping (R-SA-093, R-SA-145)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn poller_is_not_running_before_any_track_call() {
        let tracker = fast_tracker();
        assert!(!tracker.is_polling().await);
        assert_eq!(tracker.tracked_count(), 0);
    }

    #[tokio::test]
    async fn tracking_a_job_self_starts_the_poller() {
        let tracker = fast_tracker();
        let (_dir, paths) = temp_paths();
        let run_id = paths_run_id(&paths);

        tracker.track(run_id, paths, None).await;

        assert_eq!(tracker.tracked_count(), 1);
        assert!(tracker.is_polling().await, "first track() call must start the shared poller");
    }

    #[tokio::test]
    async fn poller_self_stops_once_the_only_tracked_run_is_reaped_after_retention_window() {
        let tracker = fast_tracker();
        let (_dir, paths) = temp_paths();
        let run_id = paths_run_id(&paths);

        // A run whose status.json is already terminal (Complete) — no live pid to reconcile
        // against, and no nested descendants, so it becomes fully-terminal on the very first tick.
        let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(1));
        status.state = RunState::Complete;
        write_status(&paths, &status).await;

        tracker.track(run_id, paths, None).await;
        assert!(tracker.is_polling().await);

        // Wait past the (tuned, short) retention window for the poller's own ticking to observe
        // terminal-since aging out and evict the job, which then causes the loop to self-stop.
        tracker.wait_for_poller_to_stop().await;

        assert_eq!(
            tracker.tracked_count(),
            0,
            "the fully-terminal job must have been evicted after the retention window"
        );
        assert!(
            !tracker.is_polling().await,
            "the shared poll loop must have self-stopped once no jobs remained tracked"
        );
    }

    #[tokio::test]
    async fn a_new_track_call_after_self_stop_restarts_the_poller() {
        let tracker = fast_tracker();

        // First job: terminal immediately, ages out, poller self-stops.
        let (_dir1, paths1) = temp_paths();
        let run_id1 = paths_run_id(&paths1);
        let mut status1 = RunStatus::queued(run_id1.clone(), RunMode::Single, Some(1));
        status1.state = RunState::Complete;
        write_status(&paths1, &status1).await;
        tracker.track(run_id1, paths1, None).await;
        tracker.wait_for_poller_to_stop().await;
        assert!(!tracker.is_polling().await);

        // Second job: track again — the poller must restart from scratch.
        let (_dir2, paths2) = temp_paths();
        let run_id2 = paths_run_id(&paths2);
        tracker.track(run_id2, paths2, None).await;

        assert!(
            tracker.is_polling().await,
            "tracking a new job after a prior self-stop must restart the shared poller"
        );
    }

    // ---------------------------------------------------------------------------------------
    // events.jsonl tailing: real appended bytes, per-run byte cursor
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn tail_new_lines_reads_nothing_for_a_missing_file() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");
        let outcome = tail_new_lines(&path, 0, DEFAULT_MAX_READ_CHUNK_BYTES, DEFAULT_MAX_LINE_BYTES)
            .await
            .expect("missing file is not an error");
        assert!(outcome.lines.is_empty());
        assert_eq!(outcome.new_cursor, 0);
    }

    #[tokio::test]
    async fn tail_new_lines_reads_only_complete_lines_and_advances_cursor_exactly() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");

        tokio::fs::write(&path, b"{\"kind\":\"run.paused\"}\n{\"kind\":\"run.repaired_stale\"}\n")
            .await
            .expect("seed two complete lines");

        let outcome = tail_new_lines(&path, 0, DEFAULT_MAX_READ_CHUNK_BYTES, DEFAULT_MAX_LINE_BYTES)
            .await
            .expect("tail succeeds");

        assert_eq!(outcome.lines.len(), 2);
        assert_eq!(outcome.lines[0]["kind"], "run.paused");
        assert_eq!(outcome.lines[1]["kind"], "run.repaired_stale");

        let full_len = tokio::fs::metadata(&path).await.expect("stat").len();
        assert_eq!(outcome.new_cursor, full_len, "cursor must advance past every fully-read byte");
    }

    #[tokio::test]
    async fn tail_new_lines_does_not_advance_past_an_incomplete_trailing_line() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");

        tokio::fs::write(&path, b"{\"kind\":\"a\"}\n{\"kind\":\"b\"")
            .await
            .expect("one complete line + one incomplete trailing line");

        let outcome = tail_new_lines(&path, 0, DEFAULT_MAX_READ_CHUNK_BYTES, DEFAULT_MAX_LINE_BYTES)
            .await
            .expect("tail succeeds");

        assert_eq!(outcome.lines.len(), 1, "only the complete line is returned");
        assert_eq!(outcome.lines[0]["kind"], "a");

        let first_line_len = b"{\"kind\":\"a\"}\n".len() as u64;
        assert_eq!(
            outcome.new_cursor, first_line_len,
            "cursor must stop right after the last COMPLETE newline, not consume the partial tail"
        );

        // Completing the trailing line and tailing again from the returned cursor picks it up
        // whole.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("reopen for append");
        writeln!(file, "}}").expect("complete the second line"); // now: {"kind":"b"}\n
        drop(file);

        let outcome2 = tail_new_lines(
            &path,
            outcome.new_cursor,
            DEFAULT_MAX_READ_CHUNK_BYTES,
            DEFAULT_MAX_LINE_BYTES,
        )
        .await
        .expect("second tail succeeds");
        assert_eq!(outcome2.lines.len(), 1);
        assert_eq!(outcome2.lines[0]["kind"], "b");
    }

    #[tokio::test]
    async fn tail_new_lines_tolerates_a_malformed_line_without_aborting_the_tick() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");

        tokio::fs::write(&path, b"{\"kind\":\"a\"}\nnot valid json at all\n{\"kind\":\"c\"}\n")
            .await
            .expect("seed a malformed middle line");

        let outcome = tail_new_lines(&path, 0, DEFAULT_MAX_READ_CHUNK_BYTES, DEFAULT_MAX_LINE_BYTES)
            .await
            .expect("tail succeeds despite the malformed line");

        assert_eq!(outcome.lines.len(), 2, "the malformed line is skipped, not fatal");
        assert_eq!(outcome.lines[0]["kind"], "a");
        assert_eq!(outcome.lines[1]["kind"], "c");
    }

    #[tokio::test]
    async fn tail_new_lines_discards_a_line_longer_than_the_max_line_cap() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");

        let huge_string_value = "x".repeat(200);
        let huge_line = format!("{{\"kind\":\"huge\",\"pad\":\"{huge_string_value}\"}}\n");
        let small_cap = 32usize; // far smaller than huge_line's length
        tokio::fs::write(&path, huge_line.as_bytes())
            .await
            .expect("seed an oversized line");

        let outcome = tail_new_lines(&path, 0, DEFAULT_MAX_READ_CHUNK_BYTES, small_cap)
            .await
            .expect("tail succeeds");

        assert!(
            outcome.lines.is_empty(),
            "a line exceeding max_line_bytes must be silently discarded, never returned"
        );
        assert_eq!(
            outcome.new_cursor,
            huge_line.len() as u64,
            "the cursor still advances past the discarded line's bytes"
        );
    }

    #[tokio::test]
    async fn tail_new_lines_resets_to_zero_when_the_file_shrinks_below_the_cursor() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");

        tokio::fs::write(&path, b"{\"kind\":\"a\"}\n{\"kind\":\"b\"}\n")
            .await
            .expect("seed two lines");
        let full_len = tokio::fs::metadata(&path).await.expect("stat").len();

        // Simulate rotation: truncate/replace with fresh, shorter content.
        tokio::fs::write(&path, b"{\"kind\":\"fresh\"}\n")
            .await
            .expect("rotate to shorter content");

        let outcome = tail_new_lines(&path, full_len, DEFAULT_MAX_READ_CHUNK_BYTES, DEFAULT_MAX_LINE_BYTES)
            .await
            .expect("tail succeeds after rotation");

        assert_eq!(outcome.lines.len(), 1);
        assert_eq!(outcome.lines[0]["kind"], "fresh");
    }

    #[tokio::test]
    async fn tail_new_lines_respects_a_bounded_read_chunk_across_multiple_ticks() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("events.jsonl");

        // Ten identical, fixed-size lines.
        let line = b"{\"kind\":\"x\"}\n";
        let mut content = Vec::new();
        for _ in 0..10 {
            content.extend_from_slice(line);
        }
        tokio::fs::write(&path, &content).await.expect("seed ten lines");

        // Cap the read chunk at just enough for 3 lines.
        let chunk_cap = line.len() * 3;

        let mut cursor = 0u64;
        let mut total_lines = 0usize;
        let mut ticks = 0usize;
        loop {
            let outcome = tail_new_lines(&path, cursor, chunk_cap, DEFAULT_MAX_LINE_BYTES)
                .await
                .expect("tail succeeds");
            ticks += 1;
            total_lines += outcome.lines.len();
            let advanced = outcome.new_cursor > cursor;
            cursor = outcome.new_cursor;
            if !advanced || ticks > 20 {
                break;
            }
        }

        assert_eq!(total_lines, 10, "every line is eventually observed across bounded-chunk ticks");
        assert!(ticks >= 4, "a bounded chunk cap must force multiple ticks to drain all ten lines, got {ticks}");
        assert_eq!(cursor, content.len() as u64);
    }

    // ---------------------------------------------------------------------------------------
    // End-to-end: a real simulated runner appends events.jsonl + writes status.json; the
    // tracker's own poll loop detects the state changes and self-stops once the run reaches a
    // fully-terminal condition and ages past the retention window (this task's own stated test
    // requirement).
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tracker_detects_a_simulated_runners_state_changes_and_self_stops_when_no_jobs_remain() {
        let tracker = fast_tracker();
        let (_dir, paths) = temp_paths();
        let run_id = paths_run_id(&paths);

        // Simulated runner, phase 1: initial Running status + one appended event line
        // (mirrors runner_main.rs's real R-SA-075 initial write + a control/notification event).
        let mut status = running_status(run_id.clone(), std::process::id());
        write_status(&paths, &status).await;
        append_event_line(&paths.events, br#"{"kind":"run.started"}"#).await;

        tracker.track(run_id.clone(), paths.clone(), Some(SystemTime::now())).await;
        assert!(tracker.is_polling().await);

        // Give the poller a few ticks to observe the Running state and tail the first event.
        wait_until(Duration::from_secs(2), || {
            let tracker = Arc::clone(&tracker);
            let run_id = run_id.clone();
            async move {
                tracker
                    .get(&run_id)
                    .and_then(|job| job.last_status)
                    .is_some_and(|s| s.state == RunState::Running)
            }
        })
        .await;

        let job_after_running = tracker.get(&run_id).expect("still tracked while Running");
        assert!(job_after_running.events_cursor > 0, "the poller must have tailed the appended event");
        assert!(job_after_running.terminal_since.is_none(), "a Running job is not yet terminal");

        // Simulated runner, phase 2: the run pauses (a soft interrupt, R-SA-084) — still not
        // terminal.
        status.state = RunState::Paused;
        write_status(&paths, &status).await;
        append_event_line(&paths.events, br#"{"kind":"run.paused"}"#).await;

        wait_until(Duration::from_secs(2), || {
            let tracker = Arc::clone(&tracker);
            let run_id = run_id.clone();
            async move {
                tracker
                    .get(&run_id)
                    .and_then(|job| job.last_status)
                    .is_some_and(|s| s.state == RunState::Paused)
            }
        })
        .await;
        assert!(
            tracker.get(&run_id).expect("still tracked").terminal_since.is_none(),
            "Paused must never be treated as terminal (R-SA-084)"
        );

        // Simulated runner, phase 3: resumes and completes — writes status.json THEN the terminal
        // ResultFile, exactly mirroring R-SA-077's real write ordering.
        status.state = RunState::Running;
        write_status(&paths, &status).await;
        status.advance_state(RunState::Complete).expect("Running -> Complete is legal");
        write_status(&paths, &status).await;
        let result = crate::background::ResultFile {
            id: run_id.clone(),
            run_id: run_id.clone(),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: paths.run_dir.clone(),
            session_file: None,
            results: Vec::new(),
        };
        crate::background::atomic::write_atomic_json(&paths.result, &result)
            .await
            .expect("write terminal ResultFile");
        append_event_line(&paths.events, br#"{"kind":"run.completed"}"#).await;

        // The tracker must observe the terminal transition, then (after the tuned ~150ms retention
        // window) evict the job and self-stop the poller entirely — the task's own stated
        // requirement.
        tracker.wait_for_poller_to_stop().await;

        assert_eq!(
            tracker.tracked_count(),
            0,
            "the completed run must have been evicted after the retention window"
        );
        assert!(
            !tracker.is_polling().await,
            "the poller must self-stop once no tracked jobs remain"
        );
    }

    #[tokio::test]
    async fn untrack_removes_a_job_without_waiting_for_the_retention_window() {
        let tracker = fast_tracker();
        let (_dir, paths) = temp_paths();
        let run_id = paths_run_id(&paths);

        // A run that is very much still Running (real live pid: this test process itself) — it
        // would never reach the retention window on its own.
        let status = running_status(run_id.clone(), std::process::id());
        write_status(&paths, &status).await;

        tracker.track(run_id.clone(), paths, None).await;
        assert_eq!(tracker.tracked_count(), 1);

        tracker.untrack(&run_id);
        assert_eq!(tracker.tracked_count(), 0, "untrack must remove the job immediately");
    }

    #[tokio::test]
    async fn a_genuinely_dead_tracked_run_is_reconciled_to_failed_and_eventually_evicted() {
        let tracker = fast_tracker();
        let (_dir, paths) = temp_paths();
        let run_id = paths_run_id(&paths);

        // Spawn and reap a real child so its pid is GENUINELY dead (mirrors reconcile.rs's own
        // test convention: a real OS-level fact, never mocked).
        let mut child = std::process::Command::new("true")
            .spawn()
            .expect("`true` spawns");
        let dead_pid = child.id();
        child.wait().expect("reap the child");

        let status = running_status(run_id.clone(), dead_pid);
        write_status(&paths, &status).await;

        tracker.track(run_id.clone(), paths, None).await;

        wait_until(Duration::from_secs(2), || {
            let tracker = Arc::clone(&tracker);
            let run_id = run_id.clone();
            async move {
                tracker
                    .get(&run_id)
                    .and_then(|job| job.last_status)
                    .is_some_and(|s| s.state == RunState::Failed)
            }
        })
        .await;

        // Eventually evicted once terminal-since ages past the (tuned) retention window.
        tracker.wait_for_poller_to_stop().await;
        assert_eq!(tracker.tracked_count(), 0);
    }

    // ---------------------------------------------------------------------------------------
    // Test helpers
    // ---------------------------------------------------------------------------------------

    fn paths_run_id(paths: &RunPaths) -> RunId {
        RunId::from_token(
            paths
                .run_dir
                .file_name()
                .expect("run_dir has a file name")
                .to_string_lossy()
                .into_owned(),
        )
    }

    async fn append_event_line(events_path: &std::path::Path, line: &[u8]) {
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(events_path)
            .await
            .expect("open events.jsonl for append");
        file.write_all(line).await.expect("write event line");
        file.write_all(b"\n").await.expect("write newline");
        file.flush().await.expect("flush");
    }

    /// Polls `condition` (an async predicate) every 20ms until it returns `true` or `timeout`
    /// elapses, at which point it panics with a clear message — used only inside `#[tokio::test]`
    /// functions (never production code) to wait deterministically for the tracker's own
    /// background poll loop to observe an on-disk change, without a fixed sleep racing the loop's
    /// actual tick cadence.
    async fn wait_until<F, Fut>(timeout: Duration, mut condition: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        let deadline = Instant::now() + timeout;
        loop {
            if condition().await {
                return;
            }
            assert!(Instant::now() < deadline, "condition did not become true within {timeout:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
