//! Async-root retention — the reaper's functional core: the policy, the batched scan, and
//! tombstones.
//!
//! Ports the run half of pi `runs/background/async-retention.ts` (912 LOC @`v0.68.0`, byte-
//! identical at `v0.67.0`) plus the whole of its discovery worker,
//! `async-retention-discovery-worker.mjs` (180 LOC, repo root), which is where the batching
//! actually lives.
//!
//! # Why this exists, and why it has no session dimension
//!
//! `async-retention.ts` contains **zero** `sessionId` references — it is the one background
//! surface in this port that is not session-scoped. It is here because the directory it reaps is
//! SHARED. [`crate::background::run_artifact_roots`]'s own doc states it:
//!
//! > *"EVERY cyrup instance in a directory resolves these SAME two paths. The key is `cwd_key` —
//! > the working directory, and **nothing else**. Running several cyrup instances in one project
//! > is ordinary, not an edge case, and they all share these roots byte for byte. Anything
//! > written here is therefore visible to all of them, and anything one of them deletes is gone
//! > for all of them."*
//!
//! Every run any instance ever launched in a working directory leaves a directory tree under
//! `<temp_root>/async/<cwd_key>/`, and nothing in this crate has ever pruned one. Growth is
//! therefore unbounded in the number of runs, not in the number of sessions — and it is
//! **unmeasured in this repo**: there is no diagnosis on disk recording a per-`cwd` directory
//! count, so this doc states the mechanism rather than a figure it cannot back.
//!
//! A reader who needs "only my runs" filters on [`crate::background::RunStatus::session_id`];
//! a reaper cannot, because a run belonging to a long-dead session is exactly what it is for.
//!
//! # THE BOUNDARY: this module owns the ASYNC ROOT and nothing inside `results_dir`
//!
//! Upstream's single file sweeps two trees. cyrup has already split them, and the seam is
//! load-bearing rather than incidental:
//!
//! | tree | owner in cyrup |
//! |---|---|
//! | `<results_dir>/completion-replay/`, `<results_dir>/output-archives/` | [`crate::background::completion_replay::cleanup_completion_replay_if_due`] |
//! | `<results_dir>/result-index/` | [`crate::background::result_index::cleanup_result_indexes`] |
//! | `<async_root>/<runId>/`, `<async_root>/.deleting-run-*/` | **this module** |
//!
//! This module never deletes anything under `results_dir`. It READS two addresses there —
//! [`crate::background::result_index`]'s mission-observer entry, as a liveness guard — and
//! deletes neither. Building a second sweep over `completion-replay/` or `result-index/` here
//! would be a parallel mechanism over directories that already have an owner.
//!
//! ## Upstream symbols that are deliberately out of scope, named so they are not re-derived
//!
//! | upstream symbol | `:line` | why not here |
//! |---|---|---|
//! | `resultSkipReason` | `:483-511` | results-side; the two dirs it guards are owned above |
//! | `resultRunId`, `resultTimestamp`, `terminalResult` | `:436-459` | ditto |
//! | `resultHasResumableSession`, `hasUnresolvedResultHandoff`, `completionMode` | `:461-481` | ditto |
//! | `RESULT_TOMBSTONE_PREFIX` | `:24` | ditto |
//! | every `Result*Cursor*` type | `:42-50` | ditto |
//! | `"replay-reference"` (`:505`) | `:505` | a LIVE replay record protecting an ARCHIVE. Owed-in-principle since `SCOPE_4.md`; still results-side, so still not here |
//! | `LOG_NAME` (`async-retention-maintenance.jsonl`) | `:22` | out of the programme |
//! | `LOCK_NAME`, `LOCK_STALE_MS`, `CURSOR_NAME` | `:20`, `:26`, `:21` | sweep control flow — SCOPE_14 |
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs         facade, constants, this narrative, `maintenance_root`
//! policy.rs      the decision — PURE. No `std::fs`, no `tokio::fs`, no clock read.
//! scan.rs        one cursor-windowed pass over the async root + every fact the policy needs
//! tombstone.rs   the run-tombstone MARKER: its on-disk format, address, write, read, self-heal
//! wait_refs.rs   the fail-closed wait-subscription run-id reader
//! ```
//!
//! # The pass, and where it runs from
//!
//! [`cleanup_async_retention`] is the whole of it: take the cross-instance lock ([`lock`]), read
//! the persisted cursor ([`cursor`]), scan one bounded window, decide, rename-then-delete, report
//! ([`report`]), advance the cursor. It never returns a `Result` — the report carries every
//! failure — and it never sleeps inside a pass.
//!
//! Its ONE production caller is the third stage of
//! `extension/executor/notices.rs`'s `spawn_retention_sweep`, armed when a session
//! installs its completion watcher and fired 60 s later, detached. That stage supplies the live
//! [`crate::background::tracker::JobTracker`] and the live workflow-controller registry as
//! `protected_run_ids`, so a run this process still has a handle on is never a candidate
//! regardless of age or on-disk state.
//!
//! The pass's own record of itself — `<maintenance_root>/async-retention-maintenance.jsonl` — is
//! what `/subagents-doctor` renders as its `Async retention` block
//! ([`crate::registration::doctor::AsyncRetentionDoctor`]), which is the only way an operator can
//! see that any of this happened.
//!
//! # The one guard cyrup has and upstream's schedule does not close for it
//!
//! Upstream's `activeMarkerExists` (`:205-207`) IS ported ([`scan_run_candidates`] probes
//! `<async_root>/.active-runs/<runId>`), so a run live in ANOTHER process is protected by its
//! active marker, by its wait subscription, by being non-terminal, and by the 30-day window.
//! `scheduledRunManager.referencedAsyncRunIds()` (`extension/index.ts:612`) used to be the one
//! protected source with no cyrup analogue. SUBA-016 landed the manager, so ALL THREE of
//! upstream's sources now feed `protected_run_ids`
//! ([`crate::background::scheduled_runs::ScheduledRunManager::referenced_async_run_ids`]) — and
//! the coupling is load-bearing rather than tidy: `schedule.delete`'s active-run guard opens a
//! referenced run's `status.json` to confirm the run ended, so reaping that directory would leave
//! a schedule holding a claim it could never clear.

pub mod cursor;
pub mod lock;
mod policy;
pub mod report;
mod scan;
pub mod sweep;
mod tombstone;
mod wait_refs;

pub use cursor::{CURSOR_NAME, RetentionCursor, cursor_path, read_cursor, write_cursor};
pub use lock::{
    LOCK_NAME, LOCK_STALE_MS, RetentionLockIdentity, RetentionLockOwner, lock_dir, machine_hostname,
};
pub use policy::{
    RECHECK_REASON_PREFIX, RetentionDecision, RunRetentionFacts, SkipReason, decide, skip_reason,
};
pub use report::{
    AsyncRetentionResult, MAINTENANCE_LOG_NAME, MaintenanceLogLine, MaintenanceLogRead,
    append_maintenance_log, compact_error, compact_message, increment, maintenance_log_path,
    read_last_maintenance_line,
};
pub use scan::{RunScanRequest, RunScanWindow, run_candidate_facts, scan_run_candidates};
pub use sweep::{AsyncRetentionOptions, cleanup_async_retention, is_pass_level_skip};
pub use tombstone::{
    RunTombstoneMarker, TombstoneMarkerState, TombstoneMarkerVersion, marker_matches,
    normalize_tombstone_path, read_run_tombstone_marker, read_run_tombstone_marker_self_healing,
    remove_run_tombstone_marker, run_tombstone_marker_path, write_run_tombstone_marker,
};
pub use wait_refs::wait_run_ids;

use std::path::{Path, PathBuf};

/// pi `RUN_TOMBSTONE_PREFIX` (`:23`) — re-exported under its pi name.
///
/// The literal itself lives in [`crate::background::terminal_run_index`], with the predicate that
/// enforces it as a RESERVED ASYNC-ROOT NAME. See that constant's own doc for why, and
/// [`crate::background::terminal_run_index::is_reserved_async_root_entry`] for the production
/// scanners that would otherwise mistake a tombstone for a run.
pub use crate::background::terminal_run_index::RUN_TOMBSTONE_PREFIX;

/// pi `ASYNC_RETENTION_DAYS = 30` (`:14`).
pub const ASYNC_RETENTION_DAYS: i64 = 30;

/// pi `RETENTION_MS` (`:19`) — the derived window [`decide`] actually compares a candidate's
/// timestamp against, as `now - ASYNC_RETENTION_MS`.
///
/// `i64` throughout, matching [`crate::time::now_epoch_millis`] and every on-disk timestamp
/// ([`crate::background::RunStatus::started_at`], `ended_at`, `last_update`).
pub const ASYNC_RETENTION_MS: i64 = ASYNC_RETENTION_DAYS * 24 * 60 * 60 * 1000;

/// pi `ASYNC_RETENTION_BATCH_SIZE = 100` (`:15`) — clamped to `[1, 100]` at the entry point
/// (`:656`).
///
/// **Load-shedding, not tuning.** The reaper runs beside live sessions in a shared directory; the
/// budget is what stops one pass from holding the disk for the length of a full tree walk.
///
/// # It bounds the candidate WINDOW, not the `readdir`
///
/// `streamDirWindow` (`async-retention-discovery-worker.mjs:22-48`) reads the directory to
/// exhaustion every pass — `rawReads` counts all of it — and keeps only the `limit` smallest
/// names above the cursor. A "stop after 100 entries" scan would never wrap and never converge.
/// See [`scan_run_candidates`].
///
/// # [CYRUP-DELTA] the whole budget is the RUN budget
///
/// Upstream splits it (`:711-712`): `runBudget = ceil(batchSize/2)`, `resultBudget = batchSize -
/// runBudget`. Under the boundary above cyrup has no result candidates in this module, so halving
/// it would halve throughput for no reason. `usize` because it bounds a `Vec`.
pub const ASYNC_RETENTION_BATCH_SIZE: usize = 100;

/// pi `ASYNC_RETENTION_DELAY_MS = 60_000` (`:16`).
///
/// # This is NOT an inter-batch pause, and nothing in `async-retention.ts` reads it
///
/// `git grep -n ASYNC_RETENTION_DELAY_MS v0.68.0 -- src/` returns exactly three lines: this
/// declaration, an import in `src/extension/index.ts:42`, and its single use as the `setTimeout`
/// delay at `src/extension/index.ts:613`. It is a **post-install one-shot delay before the first
/// (and only) pass** — `cleanupAsyncRetention` processes one budgeted window and returns, with no
/// inner loop over batches anywhere in the file.
///
/// Recorded here, on the constant, so part B cannot miss it: if cyrup grows a multi-pass loop
/// separated by this delay, that loop is a `[CYRUP-DELTA]`, not a port.
pub const ASYNC_RETENTION_DELAY_MS: i64 = 60_000;

/// pi `ASYNC_RETENTION_TOMBSTONE_GRACE_MS` (`:17`) — 24 hours.
///
/// # What it is: crash / concurrency hysteresis. Both readings, because they get conflated.
///
/// **The mechanism.** In the happy path a tombstone is minted and destroyed inside ONE loop
/// iteration with no wait of any kind: `:807` mints the path, `:808` writes the marker, `:811`
/// renames the run tree onto it, `:817-819` re-reads and re-decides, `:830` deletes it. Elapsed
/// grace: zero. This constant is consulted at exactly one place for runs — `:790`,
/// `currentTime - stat.mtimeMs < tombstoneGraceMs` — and that branch is reachable only for an
/// entry **already named `.deleting-run-*` when the pass began** (`:785`), i.e. a tombstone that a
/// previous, crashed or aborted pass left behind, or one a CONCURRENT instance in the same shared
/// root is working on right now. The concurrency reading is the one that matters for cyrup,
/// because [`crate::background::run_artifact_roots`] guarantees the root is shared.
///
/// **What it is NOT.** It is not "tombstone now, reap 24 h later". Reading it that way turns
/// every reap into a two-pass, ≥24 h operation and leaves a `.deleting-run-*` directory sitting
/// in the shared async root for a day, where every async-root scanner can see it. The correct
/// shape is: *reap in one pass; the grace only gates a tombstone that outlived the pass that made
/// it.*
///
/// **What actually protects a run being reconciled concurrently** is a different mechanism:
/// [`decide`]'s liveness guards ([`SkipReason::RuntimeReference`],
/// [`SkipReason::WaitReference`], [`SkipReason::ActiveIndex`], `:292-294`) plus the re-check
/// AFTER the rename (`:817-829`), which re-reads the status and the wait subscriptions and
/// renames the tree BACK if anything changed. The 24 h number is not that.
///
/// # Not the same constant as `wait_subscriptions::FOREIGN_SWEEP_GRACE_MS`
///
/// That one is also 24 h and also a grace, and the two will be conflated unless this says
/// otherwise: it withholds a FOREIGN session's expired subscription record from deletion, in a
/// directory this module never touches. Same number, different directory, different job.
pub const ASYNC_RETENTION_TOMBSTONE_GRACE_MS: i64 = 24 * 60 * 60 * 1000;

/// pi `RUN_TOMBSTONE_MARKERS_DIR` (`:25`) — the subdirectory of [`maintenance_root`] holding one
/// `<enc(runId)>.json` marker per in-flight run tombstone.
pub const RUN_TOMBSTONE_MARKERS_DIR: &str = "async-retention-run-tombstones";

/// The per-`cwd` maintenance root: `<async_root>/.async-retention/`.
///
/// Pure path arithmetic; never touches the filesystem (creation is
/// [`write_run_tombstone_marker`]'s job, via
/// [`crate::background::ensure_accessible_dir`]). See
/// [`crate::background::terminal_run_index::ASYNC_RETENTION_MAINTENANCE_DIR`] for why it is
/// nested inside the async root rather than pi's `path.dirname(asyncDirRoot)` (`:658`), which on
/// this side would be a directory shared by every working directory on the machine.
#[must_use]
pub fn maintenance_root(async_root: &Path) -> PathBuf {
    async_root.join(crate::background::terminal_run_index::ASYNC_RETENTION_MAINTENANCE_DIR)
}
