//! The pass: lock, scan, decide, destroy, report, advance.
//!
//! Ports pi `cleanupAsyncRetention` (`:650-912` @`v0.68.0`) — its run half. This is the module
//! that turns [`super`]'s functional core on.
//!
//! # One bounded pass, and NO sleep inside it
//!
//! [`ASYNC_RETENTION_DELAY_MS`](super::ASYNC_RETENTION_DELAY_MS) is a **schedule** delay, not an
//! inter-batch pause: `git grep ASYNC_RETENTION_DELAY_MS v0.68.0 -- src/` finds it only as the
//! `setTimeout` delay of a one-shot, post-activation, `unref`'d timer (`extension/index.ts:613`).
//! `cleanupAsyncRetention` evaluates at most `batch_size` candidates and returns (`:756`, `:839`).
//! The load-shedding is the BOUND plus the persisted cursor ([`super::cursor`]), not a pause.
//!
//! A sleeping loop here would be strictly worse than upstream on this side:
//! `extension/executor/notices.rs`'s sweep task is `abort()`ed on watcher teardown, so a
//! pass parked between batches would be killed mid-flight by any session that ends inside the
//! window — leaving a `.deleting-run-*` tombstone and an uncommitted cursor every time.
//!
//! # Deletion is RENAME-then-delete, and that is the whole crash-safety story
//!
//! There is no "remove `status.json` last" ordering, here or upstream: `remove_dir_all` promises
//! nothing about order and a hand-rolled ordered delete would re-introduce the half-deleted tree
//! the rename exists to prevent. Instead (`:807-833`):
//!
//! | crash point | what is left | what the next pass does |
//! |---|---|---|
//! | before the marker write | the run tree, untouched | re-evaluates it, indistinguishable from a first sighting |
//! | after the marker, before the rename | the run tree plus a dangling marker | [`super::read_run_tombstone_marker_self_healing`] unlinks the marker; the run is re-evaluated |
//! | after the rename, during the delete | `.deleting-run-<uuid>` plus its marker | re-discovers it, requires the marker to match, waits out the grace, re-reads the wait references, completes the delete as a `reaped_tombstone` |
//!
//! [`SkipReason::IdentityMismatch`](super::SkipReason::IdentityMismatch)'s tombstone-prefix
//! exemption (`:291`) is what keeps a renamed tree attributable to its `status.run_id`, and an
//! ORPHANED tombstone — one whose marker is gone — is skipped as `run-tombstone-marker` (`:787`)
//! rather than deleted. Fail-closed in both directions.
//!
//! # This module never deletes anything under `results_dir`
//!
//! `AsyncRetentionOptions::results_dir` is READ for two liveness guards
//! ([`super::scan_run_candidates`]'s mission-observer probe) and for
//! addressing a run's [`RunPaths`], and is written or unlinked nowhere.
//! [`AsyncRetentionResult::deleted_results`] is therefore permanently `0`; see that field's doc
//! for who owns the results-side sweeps and where they run.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Instant;

use cyrup_core::CancelToken;

use crate::background::reconcile::ReconcileAction;
use crate::background::result_index::errno;
use crate::background::{RunId, RunPaths, RunState};
use crate::identity::RunDirName;

use super::cursor::{CursorVersion, RetentionCursor, read_cursor, write_cursor};
use super::lock::{
    LockOwnerVersion, RetentionLockIdentity, RetentionLockOwner, acquire_retention_lock, lock_dir,
    lock_owner_token_matches, release_retention_lock,
};
use super::policy::{RECHECK_REASON_PREFIX, RetentionDecision, RunRetentionFacts, decide};
use super::report::{AsyncRetentionResult, append_maintenance_log, compact_error, increment};
use super::scan::{RunScanRequest, run_candidate_facts, scan_run_candidates};
use super::{
    ASYNC_RETENTION_BATCH_SIZE, ASYNC_RETENTION_MS, ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
    RUN_TOMBSTONE_PREFIX, remove_run_tombstone_marker, wait_run_ids, write_run_tombstone_marker,
};

/// The `skipped` keys that belong to the PASS rather than to any one candidate.
///
/// Kept as named constants beside each other because two separate readers depend on the exact
/// spelling — [`AsyncRetentionResult::accounts_for_every_candidate`], which must exclude every one
/// of them from the accounting identity, and
/// [`crate::registration::doctor::AsyncRetentionDoctor`], which renders `lock-busy` differently
/// from a candidate skip. The per-candidate vocabulary is
/// [`SkipReason::as_str`](super::SkipReason::as_str)'s closed enum and is NOT repeated here.
pub mod skip_reasons {
    /// pi `:702` — another instance in this shared scope holds the retention lock.
    pub const LOCK_BUSY: &str = "lock-busy";
    /// pi `:744`, `:903` — the lock was broken out from under this pass; its cursor is refused.
    pub const LOCK_OWNER_CHANGED: &str = "lock-owner-changed";
    /// pi `:753` — a wait-subscription record could not be read, so the whole pass declines to
    /// delete anything. Fail-closed: an unreadable subscription might be protecting any run in the
    /// root.
    pub const WAIT_REFERENCES_UNKNOWN: &str = "wait-references-unknown";
    /// pi `:697` — the pass observed its cancel token at a checkpoint.
    pub const CANCELLED: &str = "cancelled";
    /// pi `:898` — a candidate mutated the tree and then failed or rolled back, so the cursor is
    /// not advanced past it.
    pub const COMMIT_FAILURE: &str = "commit-failure";
    /// pi `:735` `worker-failure`, renamed: **[CYRUP-DELTA]** cyrup's discovery is in-process
    /// `tokio::fs`, so there is no worker to fail — what can fail is the scan itself.
    pub const DISCOVERY_FAILURE: &str = "discovery-failure";
    /// **[CYRUP-DELTA]** upstream's `mkdirSync(maintenanceRoot)` (`:700`) throws out of the
    /// function; this port never returns a `Result`, so the condition is reported instead.
    pub const MAINTENANCE_ROOT_UNAVAILABLE: &str = "maintenance-root-unavailable";
    /// **[CYRUP-DELTA]** upstream's `writeAtomicJson(cursor)` (`:907`) throws out of the function
    /// AFTER the deletions, losing the whole report. Reported instead: the pass really did happen,
    /// it just could not record where it got to, and the next pass will re-scan from the old
    /// cursor rather than skip.
    pub const CURSOR_WRITE_FAILURE: &str = "cursor-write-failure";
    /// pi `:764` — the candidate is not a directory (or became a symlink) between discovery and
    /// commit. Raised INSIDE the candidate loop, so it is deliberately absent from
    /// [`super::is_pass_level_skip`].
    pub const UNSAFE_RUN_PATH: &str = "unsafe-run-path";
}

/// `true` for a `skipped` key raised outside the per-candidate loop.
///
/// Used by [`AsyncRetentionResult::accounts_for_every_candidate`]: these keys belong to no
/// candidate, so counting them would break the identity on exactly the passes that scanned
/// nothing. `unsafe-run-path` and every `recheck-…` key are candidate-level and are NOT listed.
#[must_use]
pub fn is_pass_level_skip(reason: &str) -> bool {
    matches!(
        reason,
        skip_reasons::LOCK_BUSY
            | skip_reasons::LOCK_OWNER_CHANGED
            | skip_reasons::WAIT_REFERENCES_UNKNOWN
            | skip_reasons::CANCELLED
            | skip_reasons::COMMIT_FAILURE
            | skip_reasons::DISCOVERY_FAILURE
            | skip_reasons::MAINTENANCE_ROOT_UNAVAILABLE
            | skip_reasons::CURSOR_WRITE_FAILURE
    )
}

/// pi `AsyncRetentionOptions` (`:81-101`), narrowed to the run half.
///
/// A borrowed-field input struct rather than a twelve-argument function, the convention
/// [`crate::background::completion_replay::CompletionReplayWrite`] and
/// `result_index::ResultWrite` already set: three of the fields are roots that must agree with
/// each other, and a positional list of `&Path`s is the shape in which they get transposed.
#[derive(Clone, Copy, Debug)]
pub struct AsyncRetentionOptions<'a> {
    /// `<temp_root>/async/<cwd_key>` — [`crate::background::RunArtifactRoots::async_root`]. The
    /// tree this pass reaps.
    pub async_root: &'a Path,
    /// `<temp_root>/results/<cwd_key>` — READ ONLY here (see the module doc).
    pub results_dir: &'a Path,
    /// [`crate::background::wait_subscriptions_dir_in`]`(roots, cwd)`. pi `:84`.
    pub wait_subscriptions_dir: &'a Path,
    /// [`super::maintenance_root`]`(async_root)` — the lock, the cursor, the maintenance log and
    /// the run-tombstone markers.
    ///
    /// Passed rather than re-derived so a test can point one pass at a different maintenance root
    /// than another and prove two scopes do not share a lock. **Never**
    /// `dirname(async_root)`: that is upstream's default (`:658`) and is correct only because
    /// upstream's async dir is flat — on this side it is `<run_scratch>/async`, shared by every
    /// working directory on the machine.
    pub maintenance_root: &'a Path,
    /// pi `protectedRunIds` (`:85`) — every run this PROCESS still has a handle on. Built at the
    /// call site from the live [`crate::background::tracker::JobTracker`] and the live
    /// workflow-controller registry; see
    /// `AsyncRetentionSchedule` (`extension/executor/notices.rs`).
    pub protected_run_ids: &'a BTreeSet<RunId>,
    /// Epoch millis, injected. [`crate::time::now_epoch_millis`] at the one production call site.
    pub now: i64,
    /// pi `:87`, default [`ASYNC_RETENTION_MS`].
    pub retention_ms: i64,
    /// pi `:88`, default [`ASYNC_RETENTION_TOMBSTONE_GRACE_MS`].
    pub tombstone_grace_ms: i64,
    /// pi `:89`. Clamped to `[1, ASYNC_RETENTION_BATCH_SIZE]` (`:656`): a caller can only ever make
    /// the batch SMALLER, never larger, so a `batch_size: 10_000` is silently 100.
    ///
    /// **[CYRUP-DELTA]** upstream then splits it (`:711-712`) — `run_budget = ceil(size/2)`,
    /// `result_budget = size - run_budget`. Under this module's boundary there are no result
    /// candidates, so the whole budget is the run budget. The day the results half lands, that is
    /// the line to restore.
    pub batch_size: usize,
    /// Who this process is, for the cross-instance lock. [`RetentionLockIdentity::current`] at the
    /// production call site.
    pub lock_identity: &'a RetentionLockIdentity,
    /// pi's `AbortSignal` (`:694-699`, `:742`, `:757`, `:840`), as cyrup's own cancellation type.
    ///
    /// Checked at every checkpoint upstream checks. `None` is a pass nothing can cancel — which is
    /// safe here only because the pass is bounded by construction.
    pub cancel: Option<&'a CancelToken>,
}

impl<'a> AsyncRetentionOptions<'a> {
    /// One pass over `async_root`, on the default 30-day policy and the default batch size.
    ///
    /// The four paths, the protected set, the clock and the lock identity have no sensible
    /// default — every one of them is either a root that must agree with the others or an
    /// environment probe the pass must not make for itself — so they are arguments, and only the
    /// three tuning knobs and the cancel token are defaulted.
    #[must_use]
    pub fn new(
        async_root: &'a Path,
        results_dir: &'a Path,
        wait_subscriptions_dir: &'a Path,
        maintenance_root: &'a Path,
        protected_run_ids: &'a BTreeSet<RunId>,
        lock_identity: &'a RetentionLockIdentity,
        now: i64,
    ) -> Self {
        Self {
            async_root,
            results_dir,
            wait_subscriptions_dir,
            maintenance_root,
            protected_run_ids,
            now,
            retention_ms: ASYNC_RETENTION_MS,
            tombstone_grace_ms: ASYNC_RETENTION_TOMBSTONE_GRACE_MS,
            batch_size: ASYNC_RETENTION_BATCH_SIZE,
            lock_identity,
            cancel: None,
        }
    }
}

/// Everything one pass accumulates. A struct rather than a pile of `let mut`s so the per-candidate
/// helpers can be real functions instead of one 300-line body.
#[derive(Debug, Default)]
struct PassState {
    result: AsyncRetentionResult,
    /// pi `deletedIds` (`:694`) — `run:<id>` / `run-tombstone:<id>`, for the maintenance log.
    deleted_ids: Vec<String>,
    /// pi `cursorCommitUnsafe` (`:687`). Set by any candidate that mutated the tree and then
    /// failed or rolled back.
    cursor_commit_unsafe: bool,
    /// When the destructive half began, for `commit_duration_ms`.
    commit_started: Option<Instant>,
}

/// One retention pass over `options.async_root`.
///
/// **Never returns a [`std::io::Result`].** Upstream returns its report unconditionally (`:650`,
/// `:688-693`) and captures every per-candidate failure into
/// [`AsyncRetentionResult::errors`] — the same discipline
/// [`crate::registration::doctor::DoctorRunner::run`] states for itself. A reaper that refused to
/// report because one directory was unreadable would be strictly less useful than one that reports
/// the unreadable directory, and its one production caller is a detached task with nowhere to
/// propagate an error to.
///
/// The report is also written to `<maintenance_root>/async-retention-maintenance.jsonl` before
/// returning, unconditionally — including on the `lock-busy` early return. That file is the only
/// channel by which `/subagents-doctor`, in a later process, can see that retention ran.
pub async fn cleanup_async_retention(options: &AsyncRetentionOptions<'_>) -> AsyncRetentionResult {
    let started = Instant::now();
    let mut state = PassState::default();

    run_pass(options, &mut state).await;

    if let Some(commit_started) = state.commit_started {
        state.result.commit_duration_ms = elapsed_ms(commit_started);
    }
    state.result.duration_ms = elapsed_ms(started);

    // pi `finish()` (`:688-693`) logs unconditionally. A failure to log is not a failure of the
    // pass — the deletions already happened — so it is traced and dropped.
    if let Err(error) = append_maintenance_log(
        options.maintenance_root,
        &state.result,
        options.now,
        &state.deleted_ids,
    )
    .await
    {
        tracing::debug!(%error, "could not append the async-retention maintenance log");
    }

    state.result
}

/// Acquire, sweep, release. The `release` is unconditional for the same reason upstream's is a
/// `finally` (`:909-911`).
async fn run_pass(options: &AsyncRetentionOptions<'_>, state: &mut PassState) {
    if let Err(error) = crate::background::ensure_accessible_dir(options.maintenance_root).await {
        state.result.errors.push(compact_error(&error));
        increment(
            &mut state.result.skipped,
            skip_reasons::MAINTENANCE_ROOT_UNAVAILABLE,
        );
        return;
    }

    let lock = lock_dir(options.maintenance_root);
    let token = uuid::Uuid::new_v4().to_string();
    let owner = RetentionLockOwner {
        version: LockOwnerVersion,
        token: token.clone(),
        pid: options.lock_identity.pid,
        hostname: options.lock_identity.hostname.clone(),
        started_at: options.now,
        process_start_identity: options.lock_identity.process_start_identity.clone(),
    };
    match acquire_retention_lock(&lock, &owner, options.lock_identity).await {
        Ok(true) => {}
        Ok(false) => {
            increment(&mut state.result.skipped, skip_reasons::LOCK_BUSY);
            return;
        }
        Err(error) => {
            // A filesystem fault while taking the lock is reported AND counted as `lock-busy`,
            // because the observable outcome is identical: this pass does not hold the lock and
            // must not touch the root.
            state.result.errors.push(compact_error(&error));
            increment(&mut state.result.skipped, skip_reasons::LOCK_BUSY);
            return;
        }
    }
    state.result.acquired = true;

    locked_pass(options, state, &lock, &token).await;

    release_retention_lock(&lock, &token).await;
}

/// Everything that happens while this pass holds the lock.
async fn locked_pass(
    options: &AsyncRetentionOptions<'_>,
    state: &mut PassState,
    lock: &Path,
    token: &str,
) {
    if mark_cancelled(options, state) {
        return;
    }

    let batch_size = options.batch_size.clamp(1, ASYNC_RETENTION_BATCH_SIZE);
    let cursor = read_cursor(options.maintenance_root).await;
    let request = RunScanRequest {
        async_root: options.async_root,
        results_dir: options.results_dir,
        maintenance_root: Some(options.maintenance_root),
        after: cursor.run_after.as_deref(),
        budget: batch_size,
    };

    let discovery_started = Instant::now();
    let window = match scan_run_candidates(request).await {
        Ok(window) => window,
        Err(error) => {
            state.result.discovery_duration_ms = elapsed_ms(discovery_started);
            state.result.errors.push(compact_error(&error));
            increment(&mut state.result.skipped, skip_reasons::DISCOVERY_FAILURE);
            return;
        }
    };
    state.result.discovery_duration_ms = elapsed_ms(discovery_started);
    state.result.raw_reads = window.raw_reads;
    state.result.source_exhausted.insert(
        "runs".to_string(),
        window.cursor_cleared || window.candidates.len() < batch_size,
    );
    let next_after = window.next_after;

    state.commit_started = Some(Instant::now());
    if mark_cancelled(options, state) {
        return;
    }
    // pi `:743` — the first of the two post-discovery owner checks. Discovery can take a while on
    // a large root, and a lock broken during it means another pass is already deciding.
    if !lock_owner_token_matches(lock, token).await {
        increment(&mut state.result.skipped, skip_reasons::LOCK_OWNER_CHANGED);
        return;
    }

    // pi `:748-755`. Read ONCE before the loop, and re-read before every destructive action
    // (`:794`, `:818`) — upstream's own comment: *"Wait records are the active, swept subscription
    // set. Each destructive action re-reads it to close the race with a newly armed wait."*
    let Some(wait_ids) = wait_run_ids(options.wait_subscriptions_dir).await else {
        increment(
            &mut state.result.skipped,
            skip_reasons::WAIT_REFERENCES_UNKNOWN,
        );
        return;
    };

    for facts in window.candidates {
        if mark_cancelled(options, state) {
            return;
        }
        // pi `:758` — FIRST, before any fallible work, so `scanned` counts candidates CONSIDERED.
        // `AsyncRetentionResult::errored_candidates` is what keeps that honest.
        state.result.scanned += 1;

        let dir_name = facts.dir_name.clone();
        let mut mutated = false;
        if let Err(error) =
            process_candidate(options, state, &request, facts, &wait_ids, &mut mutated).await
        {
            // pi `:834-837`.
            if mutated {
                state.cursor_commit_unsafe = true;
            }
            state.result.errored_candidates += 1;
            // `is_absent`, NOT `is_ignorable_listing_error`: a candidate vanishing between
            // discovery and commit is ordinary, but a permission fault on a SPECIFIC run directory
            // is a real fault the report must carry (`errno`'s own "quiet when scanning, loud when
            // asked a direct question").
            if !errno::is_absent(&error) {
                state.result.errors.push(compact_error(&error));
            }
            tracing::debug!(candidate = %dir_name, %error, "async retention candidate failed");
        }
    }

    // pi `:897-900`.
    if state.cursor_commit_unsafe {
        increment(&mut state.result.skipped, skip_reasons::COMMIT_FAILURE);
        return;
    }
    if mark_cancelled(options, state) {
        return;
    }
    // pi `:902-905` — the second owner check, immediately before the commit.
    if !lock_owner_token_matches(lock, token).await {
        increment(&mut state.result.skipped, skip_reasons::LOCK_OWNER_CHANGED);
        return;
    }

    if let Err(error) = write_cursor(
        options.maintenance_root,
        &RetentionCursor {
            version: CursorVersion,
            run_after: next_after,
        },
    )
    .await
    {
        state.result.errors.push(compact_error(&error));
        increment(
            &mut state.result.skipped,
            skip_reasons::CURSOR_WRITE_FAILURE,
        );
    }
}

/// pi `markCancelled` (`:694-699`).
fn mark_cancelled(options: &AsyncRetentionOptions<'_>, state: &mut PassState) -> bool {
    if !options.cancel.is_some_and(CancelToken::is_cancelled) {
        return false;
    }
    state.result.cancelled = true;
    increment(&mut state.result.skipped, skip_reasons::CANCELLED);
    true
}

/// One candidate, pi `:759-838`.
async fn process_candidate(
    options: &AsyncRetentionOptions<'_>,
    state: &mut PassState,
    request: &RunScanRequest<'_>,
    facts: RunRetentionFacts,
    wait_ids: &BTreeSet<RunId>,
    mutated: &mut bool,
) -> std::io::Result<()> {
    // pi `:761-766`. `symlink_metadata`, never `metadata`: `metadata` follows the link and defeats
    // the check outright. Because it reports the LINK, `is_dir()` alone is the whole test —
    // `is_symlink()` is implied false by it.
    let metadata = tokio::fs::symlink_metadata(&facts.run_dir).await?;
    if !metadata.file_type().is_dir() {
        increment(&mut state.result.skipped, skip_reasons::UNSAFE_RUN_PATH);
        return Ok(());
    }

    let facts = repair_running_run(options, state, request, facts).await?;

    match decide(
        &facts,
        options.now,
        options.retention_ms,
        options.tombstone_grace_ms,
        options.protected_run_ids,
        wait_ids,
    ) {
        RetentionDecision::Keep(reason) => {
            increment(&mut state.result.skipped, reason.as_str());
            Ok(())
        }
        RetentionDecision::Reap => reap_tombstone(options, state, &facts, mutated).await,
        RetentionDecision::Tombstone => retire_run(options, state, request, &facts, mutated).await,
    }
}

/// pi `:768-779` — the `running` repair hop, and the reason this task can reap anything at all.
///
/// A run whose runner was `SIGKILL`ed leaves `status.json` claiming `Running` forever. Without this
/// hop that run is `non-terminal` on every future pass and is **never** reapable — which is
/// precisely the orphan class the whole module exists for. So the hop is ported rather than
/// skipped, via [`crate::background::reconcile::reconcile_now`], which probes the recorded pid and
/// synthesises a terminal failure for a dead one.
///
/// The four guards are upstream's, and each is load-bearing: a run whose directory name disagrees
/// with its id would have reconciliation write into the wrong tree, and a PROTECTED run must not
/// be reconciled at all — this process is still driving it.
///
/// Because `reconcile_now` may REWRITE `status.json`, the facts are re-gathered afterwards. A
/// decision taken on the pre-repair status would be a decision about a state that no longer
/// exists.
async fn repair_running_run(
    options: &AsyncRetentionOptions<'_>,
    state: &mut PassState,
    request: &RunScanRequest<'_>,
    facts: RunRetentionFacts,
) -> std::io::Result<RunRetentionFacts> {
    let Some(status) = facts.status.as_ref() else {
        return Ok(facts);
    };
    if status.state != RunState::Running
        || RunDirName::parse(status.run_id.as_str()).is_none()
        || facts.dir_name != status.run_id.as_str()
        || options.protected_run_ids.contains(&status.run_id)
    {
        return Ok(facts);
    }

    let paths = RunPaths::for_run(options.async_root, options.results_dir, &status.run_id);
    let outcome = crate::background::reconcile::reconcile_now(&paths, None).await?;
    if outcome.action != ReconcileAction::NoneNeeded {
        state.result.repaired_runs += 1;
    }

    let dir_name = facts.dir_name.clone();
    run_candidate_facts(*request, &dir_name).await
}

/// pi `:794-805` — complete the delete of a `.deleting-run-*` tree an earlier pass left behind.
async fn reap_tombstone(
    options: &AsyncRetentionOptions<'_>,
    state: &mut PassState,
    facts: &RunRetentionFacts,
    mutated: &mut bool,
) -> std::io::Result<()> {
    let Some(status) = facts.status.as_ref() else {
        // Unreachable: `decide` only returns `Reap` for a candidate with a status. Stated as a
        // keep rather than an `unwrap`, because the safe answer to "I cannot tell" is "leave it".
        increment(
            &mut state.result.skipped,
            super::SkipReason::InvalidStatus.as_str(),
        );
        return Ok(());
    };
    let run_id = status.run_id.clone();

    if let Some(key) = recheck(options, facts).await {
        increment(&mut state.result.skipped, key);
        return Ok(());
    }

    *mutated = true;
    tokio::fs::remove_dir_all(&facts.run_dir).await?;
    remove_run_tombstone_marker(options.maintenance_root, &run_id).await;
    state.result.reaped_tombstones += 1;
    state.deleted_ids.push(format!("run-tombstone:{run_id}"));
    Ok(())
}

/// pi `:807-833` — retire a live run tree: marker, rename, re-check, delete.
async fn retire_run(
    options: &AsyncRetentionOptions<'_>,
    state: &mut PassState,
    request: &RunScanRequest<'_>,
    facts: &RunRetentionFacts,
    mutated: &mut bool,
) -> std::io::Result<()> {
    let Some(status) = facts.status.as_ref() else {
        increment(
            &mut state.result.skipped,
            super::SkipReason::InvalidStatus.as_str(),
        );
        return Ok(());
    };
    let run_id = status.run_id.clone();

    let tombstone_name = format!("{RUN_TOMBSTONE_PREFIX}{}", uuid::Uuid::new_v4());
    let tombstone = options.async_root.join(&tombstone_name);

    // The marker goes down FIRST (`:808`) so that a crash between here and the rename leaves a
    // dangling marker (self-healed on the next pass) rather than an unattributable tombstone. A
    // failure here therefore means "do not rename" — the tree stays exactly where it was.
    write_run_tombstone_marker(options.maintenance_root, &run_id, &tombstone, options.now).await?;
    *mutated = true;

    // pi `:812-816`. `rename(2)` within one directory, which CAN still fail — `EXDEV` if the async
    // root straddles a mount — so the marker is rolled back rather than left naming a tree that
    // was never created.
    if let Err(error) = tokio::fs::rename(&facts.run_dir, &tombstone).await {
        remove_run_tombstone_marker(options.maintenance_root, &run_id).await;
        *mutated = false;
        return Err(error);
    }

    // pi `:817-818` — re-read the status FROM THE TOMBSTONE and re-read the wait references. The
    // window between the first decision and this one is exactly where a newly armed wait, or a
    // freshly written `status.json`, would otherwise be lost.
    let recheck_facts = run_candidate_facts(*request, &tombstone_name).await?;
    if let Some(key) = recheck(options, &recheck_facts).await {
        // pi `:820-823`. `fs.existsSync` is false on any error, so an unreadable original path
        // takes the rename-back branch here too; a rename-back that then fails propagates, leaving
        // the tombstone AND its marker — the designed intermediate state, which the next pass
        // completes.
        if !tokio::fs::try_exists(&facts.run_dir).await.unwrap_or(false) {
            tokio::fs::rename(&tombstone, &facts.run_dir).await?;
            *mutated = false;
        }
        remove_run_tombstone_marker(options.maintenance_root, &run_id).await;
        if *mutated {
            state.cursor_commit_unsafe = true;
        }
        increment(&mut state.result.skipped, key);
        return Ok(());
    }

    tokio::fs::remove_dir_all(&tombstone).await?;
    remove_run_tombstone_marker(options.maintenance_root, &run_id).await;
    state.result.deleted_runs += 1;
    state.deleted_ids.push(format!("run:{run_id}"));
    Ok(())
}

/// The SECOND decision, taken after a destructive action has been staged: re-read the wait
/// subscriptions and re-run [`super::policy::skip_reason`] (pi `:794-797`, `:818-828`).
///
/// Returns the `skipped` key to record, or `None` when the candidate is still reapable.
///
/// It is [`super::policy::skip_reason`] and deliberately NOT [`decide`]: the freshly renamed tree
/// is named `.deleting-run-*`, and `decide` would apply the tombstone grace to it — see that
/// function's own doc for why that would make a run with a recent directory mtime
/// permanently unreapable.
async fn recheck(options: &AsyncRetentionOptions<'_>, facts: &RunRetentionFacts) -> Option<String> {
    let Some(fresh_wait_ids) = wait_run_ids(options.wait_subscriptions_dir).await else {
        // pi `:795`, `:819` — an unreadable subscription set at this point aborts THIS candidate,
        // not the pass, and is recorded under the `recheck-` prefix so it is distinguishable from
        // the pass-level abort at `:753`.
        return Some(format!(
            "{RECHECK_REASON_PREFIX}{}",
            skip_reasons::WAIT_REFERENCES_UNKNOWN
        ));
    };
    super::policy::skip_reason(
        facts,
        options.now,
        options.retention_ms,
        options.protected_run_ids,
        &fresh_wait_ids,
    )
    .map(super::SkipReason::recheck_key)
}

/// Monotonic elapsed milliseconds.
///
/// [`Instant`], never a difference of [`crate::time::now_epoch_millis`] reads: upstream needs
/// `Math.max(0, …)` (`:689-690`) because a wall clock can step backwards mid-pass, and a monotonic
/// clock removes the need rather than clamping after the fact.
fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::path::PathBuf;

    use super::super::lock::LOCK_STALE_MS;
    use super::*;
    use crate::background::reconcile::Liveness;
    use crate::background::{RunMode, RunStatus};
    use crate::identity::ResultFileName;

    /// 60 days: comfortably past [`ASYNC_RETENTION_MS`], so a run backdated to it is reapable on
    /// the DEFAULT policy rather than on a tuned-down window.
    const OLD_AGE_MS: i64 = 60 * 24 * 60 * 60 * 1000;

    /// One scope's four roots plus a fixed clock, so every test reads the same way.
    struct Scope {
        _temp: tempfile::TempDir,
        async_root: PathBuf,
        results_dir: PathBuf,
        wait_dir: PathBuf,
        maintenance_root: PathBuf,
        now: i64,
    }

    impl Scope {
        async fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let async_root = temp.path().join("async").join("cwd-key");
            let results_dir = temp.path().join("results").join("cwd-key");
            let wait_dir = temp.path().join("wait-subscriptions").join("cwd-key");
            for dir in [&async_root, &results_dir, &wait_dir] {
                tokio::fs::create_dir_all(dir).await.unwrap();
            }
            let maintenance_root = super::super::maintenance_root(&async_root);
            Self {
                _temp: temp,
                async_root,
                results_dir,
                wait_dir,
                maintenance_root,
                now: crate::time::now_epoch_millis(),
            }
        }

        fn identity(&self) -> RetentionLockIdentity {
            RetentionLockIdentity {
                pid: std::process::id(),
                hostname: "test-host".to_string(),
                process_start_identity: None,
                liveness: |_| Liveness::Alive,
                start_identity_of: |_| None,
            }
        }

        /// A run directory with a `status.json`, aged `age_ms` before this scope's clock.
        ///
        /// Both the logical timestamps AND both mtimes are backdated, because
        /// `statusTimestamp` (`:174-185`) is `max(ended_at ?? last_update, max(mtime(dir),
        /// mtime(status.json)))` — leaving either mtime at "now" would make the run `recent`
        /// however old its recorded fields were. `status.json` is backdated BEFORE the directory,
        /// since writing into a directory updates the directory's own mtime.
        async fn seed_run(&self, token: &str, state: RunState, age_ms: i64) -> PathBuf {
            let at = self.now - age_ms;
            let dir = self.async_root.join(token);
            tokio::fs::create_dir_all(&dir).await.unwrap();
            let mut status =
                RunStatus::queued(RunId::from_token(token.to_string()), RunMode::Single, None);
            status.state = state;
            status.started_at = at;
            status.last_update = at;
            status.ended_at = state.is_terminal().then_some(at);
            self.write_status(&dir, &status).await;
            backdate(&dir, at);
            dir
        }

        async fn write_status(&self, dir: &Path, status: &RunStatus) {
            let path = dir.join("status.json");
            tokio::fs::write(&path, serde_json::to_vec(status).unwrap())
                .await
                .unwrap();
            backdate(&path, status.ended_at.unwrap_or(status.last_update));
        }

        /// A wait-subscription record for `run_id`, in a session that is NOT this one — the case
        /// that matters, since the whole guard exists to see ANOTHER instance's subscription.
        async fn seed_wait_subscription(&self, token: &str, run_id: &str) {
            let record = format!(
                r#"{{"version":1,"token":"{token}","sessionId":"another-session",
                   "targetKind":"async","runId":"{run_id}","requestedId":"{run_id}",
                   "createdAt":1,"expiresAt":{}}}"#,
                self.now + 60_000
            );
            tokio::fs::write(
                self.wait_dir
                    .join(format!("{token}{}", ResultFileName::EXTENSION)),
                record,
            )
            .await
            .unwrap();
        }

        fn options<'a>(
            &'a self,
            protected: &'a BTreeSet<RunId>,
            identity: &'a RetentionLockIdentity,
        ) -> AsyncRetentionOptions<'a> {
            AsyncRetentionOptions::new(
                &self.async_root,
                &self.results_dir,
                &self.wait_dir,
                &self.maintenance_root,
                protected,
                identity,
                self.now,
            )
        }

        /// The scan request one pass builds, so a test can gather a candidate's facts through
        /// the SAME gatherer the sweep uses rather than hand-assembling `RunRetentionFacts`.
        fn scan_request(&self) -> RunScanRequest<'_> {
            RunScanRequest {
                async_root: &self.async_root,
                results_dir: &self.results_dir,
                maintenance_root: Some(&self.maintenance_root),
                after: None,
                budget: ASYNC_RETENTION_BATCH_SIZE,
            }
        }

        async fn sweep(&self) -> AsyncRetentionResult {
            let protected = BTreeSet::new();
            let identity = self.identity();
            cleanup_async_retention(&self.options(&protected, &identity)).await
        }

        async fn entries(&self) -> Vec<String> {
            let mut names = Vec::new();
            let mut read = tokio::fs::read_dir(&self.async_root).await.unwrap();
            while let Some(entry) = read.next_entry().await.unwrap() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
            names.sort();
            names
        }
    }

    fn backdate(path: &Path, millis: i64) {
        filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(millis / 1000, 0))
            .unwrap();
    }

    fn token(index: usize) -> String {
        format!("run-{index:04}")
    }

    // ---------------------------------------------------------------------------------------
    // The pass itself
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_sweep_reaps_only_what_the_policy_selects() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        scope.seed_run("fresh", RunState::Complete, 0).await;

        let report = scope.sweep().await;

        assert!(report.acquired);
        assert_eq!(report.deleted_runs, 1);
        assert_eq!(report.scanned, 2);
        assert_eq!(report.skipped.get("recent").copied(), Some(1));
        assert_eq!(scope.entries().await, vec![".async-retention", "fresh"]);
        assert!(
            report.accounts_for_every_candidate(),
            "one reap plus one skip must sum to scanned"
        );
    }

    #[tokio::test]
    async fn a_paused_run_is_never_reaped() {
        // R-SA-084: `Paused` is explicitly NOT terminal, so an ancient paused run is a keep.
        let scope = Scope::new().await;
        scope.seed_run("paused", RunState::Paused, OLD_AGE_MS).await;

        let report = scope.sweep().await;

        assert_eq!(report.deleted_runs, 0);
        assert_eq!(report.skipped.get("non-terminal").copied(), Some(1));
        assert!(scope.async_root.join("paused").exists());
    }

    #[tokio::test]
    async fn a_run_whose_status_cannot_be_read_is_kept() {
        // Row 1 of `runSkipReason` (`:289`): no readable `status.json` is `invalid-status`, a
        // KEEP. The earlier name for this test said `unknown-age`, which is row 13 and a DIFFERENT
        // guard — removing `status.json` never reaches it, because row 1 returns first. Row 13's
        // own polarity ("an age this pass cannot establish is never an old age") is pinned in
        // `policy::a_run_with_an_unreadable_mtime_is_kept_not_reaped`; it is not reachable through
        // the sweep, because a status this pass managed to READ came from a file it can also
        // `stat`.
        let scope = Scope::new().await;
        let dir = scope
            .seed_run("vanishing", RunState::Complete, OLD_AGE_MS)
            .await;
        tokio::fs::remove_file(dir.join("status.json"))
            .await
            .unwrap();

        let report = scope.sweep().await;

        assert_eq!(report.deleted_runs, 0);
        assert_eq!(report.skipped.get("invalid-status").copied(), Some(1));
        assert!(
            dir.exists(),
            "'I cannot tell how old this is' is never 'delete it'"
        );
    }

    #[tokio::test]
    async fn a_protected_run_is_never_reaped_however_old_it_is() {
        let scope = Scope::new().await;
        scope.seed_run("held", RunState::Complete, OLD_AGE_MS).await;

        let protected: BTreeSet<RunId> = [RunId::from_token("held".to_string())].into();
        let identity = scope.identity();
        let report = cleanup_async_retention(&scope.options(&protected, &identity)).await;

        assert_eq!(report.deleted_runs, 0);
        assert_eq!(report.skipped.get("runtime-reference").copied(), Some(1));
        assert!(scope.async_root.join("held").exists());
    }

    #[tokio::test]
    async fn a_run_referenced_by_a_foreign_wait_subscription_is_never_reaped() {
        let scope = Scope::new().await;
        scope
            .seed_run("awaited", RunState::Complete, OLD_AGE_MS)
            .await;
        scope
            .seed_wait_subscription("3f2504e0-4f89-41d3-9a0c-0305e82c3301", "awaited")
            .await;

        let report = scope.sweep().await;

        assert_eq!(report.deleted_runs, 0);
        assert_eq!(report.skipped.get("wait-reference").copied(), Some(1));
        assert!(scope.async_root.join("awaited").exists());
    }

    #[tokio::test]
    async fn an_unparseable_wait_subscription_halts_the_whole_pass() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        tokio::fs::write(scope.wait_dir.join("broken.json"), b"{".as_slice())
            .await
            .unwrap();

        let report = scope.sweep().await;

        assert!(report.acquired);
        assert_eq!(report.scanned, 0, "the loop is never entered");
        assert_eq!(
            report.skipped.get("wait-references-unknown").copied(),
            Some(1)
        );
        assert_eq!(report.deleted_runs, 0);
        assert!(scope.async_root.join("orphan").exists());
    }

    #[tokio::test]
    async fn a_symlinked_async_root_entry_is_never_even_a_candidate() {
        // The FIRST line of defence, and the one the sweep alone can reach: `worker.mjs:63`'s
        // `entry.isDirectory()` reads the dirent type, which is false for a symlink, so a
        // symlinked entry never enters the window at all. This test used to be named for the
        // `unsafe-run-path` guard, which it cannot reach for exactly that reason — that guard is
        // the SECOND line of defence and is pinned by
        // `an_unsafe_run_path_raises_the_guard_and_deletes_nothing` below.
        let scope = Scope::new().await;
        let dir = scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        let decoy = scope._temp.path().join("decoy");
        tokio::fs::create_dir_all(&decoy).await.unwrap();
        tokio::fs::write(decoy.join("keep-me"), b"{}".as_slice())
            .await
            .unwrap();

        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = scope.options(&protected, &identity);

        tokio::fs::remove_dir_all(&dir).await.unwrap();
        tokio::fs::symlink(&decoy, &dir).await.unwrap();

        let report = cleanup_async_retention(&options).await;

        assert_eq!(report.scanned, 0, "the listing filter rejected it outright");
        assert_eq!(report.deleted_runs, 0);
        assert!(
            tokio::fs::symlink_metadata(&dir)
                .await
                .unwrap()
                .file_type()
                .is_symlink(),
            "the symlink — and therefore its target — is untouched"
        );
        assert!(
            decoy.join("keep-me").exists(),
            "and nothing followed the link into the target"
        );
    }

    #[tokio::test]
    async fn an_unsafe_run_path_raises_the_guard_and_deletes_nothing() {
        // pi `:761-766` — the re-`lstat` at commit time, for the tree that WAS a directory when
        // the window was built and is a symlink by the time the commit reaches it. A whole-sweep
        // test can never produce those facts (the scan rejects a symlink before it becomes a
        // candidate), so the candidate is built from the real gatherer and handed to the real
        // `process_candidate` with the swap performed in between — which is precisely the
        // interleaving the guard exists for.
        //
        // `symlink_metadata`, never `metadata`: the latter follows the link, reports a directory,
        // and hands the decoy's contents to `remove_dir_all`.
        let scope = Scope::new().await;
        let run_dir = scope
            .seed_run("swapped", RunState::Complete, OLD_AGE_MS)
            .await;
        let request = scope.scan_request();
        let facts = run_candidate_facts(request, "swapped").await.unwrap();
        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = scope.options(&protected, &identity);
        assert_eq!(
            decide(
                &facts,
                options.now,
                options.retention_ms,
                options.tombstone_grace_ms,
                &protected,
                &BTreeSet::new()
            ),
            RetentionDecision::Tombstone,
            "the policy would otherwise retire this tree, so only the lstat guard can spare it"
        );

        let decoy = scope._temp.path().join("decoy");
        tokio::fs::create_dir_all(&decoy).await.unwrap();
        tokio::fs::write(decoy.join("keep-me"), b"{}".as_slice())
            .await
            .unwrap();
        tokio::fs::remove_dir_all(&run_dir).await.unwrap();
        tokio::fs::symlink(&decoy, &run_dir).await.unwrap();

        let mut state = PassState::default();
        let mut mutated = false;
        process_candidate(
            &options,
            &mut state,
            &request,
            facts,
            &BTreeSet::new(),
            &mut mutated,
        )
        .await
        .unwrap();

        assert_eq!(
            state.result.skipped.get("unsafe-run-path").copied(),
            Some(1)
        );
        assert_eq!(state.result.deleted_runs, 0);
        assert!(
            !mutated,
            "nothing was staged, so the cursor stays committable"
        );
        assert!(
            decoy.join("keep-me").exists(),
            "`metadata` here would have followed the link and deleted the target"
        );
        assert!(
            tokio::fs::symlink_metadata(&run_dir)
                .await
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    // ---------------------------------------------------------------------------------------
    // Batching and the cursor
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_sweep_processes_in_batches_and_the_cursor_advances() {
        let scope = Scope::new().await;
        for index in 0..250 {
            scope
                .seed_run(&token(index), RunState::Complete, OLD_AGE_MS)
                .await;
        }

        let protected = BTreeSet::new();
        let identity = scope.identity();
        let mut options = scope.options(&protected, &identity);
        options.batch_size = 100;

        let first = cleanup_async_retention(&options).await;
        assert_eq!(first.scanned, 100, "upstream's own `scanned <= batchSize`");
        assert_eq!(first.deleted_runs, 100);
        assert_eq!(
            first.source_exhausted.get("runs").copied(),
            Some(false),
            "the budget was reached, so there is more next pass"
        );

        // Without a PERSISTED cursor this second pass would re-scan the same 100 names and reap
        // nothing new, and the root would never converge.
        let second = cleanup_async_retention(&options).await;
        assert_eq!(second.deleted_runs, 100);

        let third = cleanup_async_retention(&options).await;
        assert_eq!(third.deleted_runs, 50);
        assert_eq!(
            scope.entries().await,
            vec![".async-retention"],
            "three bounded passes converge on an empty root"
        );
    }

    #[tokio::test]
    async fn the_batch_size_can_only_ever_be_made_smaller() {
        // pi `:656` — `min(ASYNC_RETENTION_BATCH_SIZE, max(1, trunc(batchSize)))`. A caller asking
        // for 10_000 gets 100; a caller asking for 0 gets 1.
        let scope = Scope::new().await;
        for index in 0..150 {
            scope
                .seed_run(&token(index), RunState::Complete, OLD_AGE_MS)
                .await;
        }
        let protected = BTreeSet::new();
        let identity = scope.identity();
        let mut options = scope.options(&protected, &identity);
        options.batch_size = 10_000;

        let report = cleanup_async_retention(&options).await;
        assert_eq!(report.scanned, ASYNC_RETENTION_BATCH_SIZE);

        options.batch_size = 0;
        let clamped_up = cleanup_async_retention(&options).await;
        assert_eq!(clamped_up.scanned, 1);
    }

    #[tokio::test]
    async fn a_thousand_orphaned_run_directories_are_fully_reapable_across_passes() {
        // The load-shedding property, stated structurally rather than as a wall-clock bound: no
        // single pass may consider more than one batch, and repeated passes must still converge on
        // an empty root. A wall-clock threshold would be the flake shape this crate's `tokio`
        // `test-util` dev-dependency already exists to avoid.
        let scope = Scope::new().await;
        for index in 0..1_000 {
            scope
                .seed_run(&token(index), RunState::Complete, OLD_AGE_MS)
                .await;
        }
        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = scope.options(&protected, &identity);

        let mut passes = 0;
        let mut reaped = 0;
        while passes < 64 {
            let report = cleanup_async_retention(&options).await;
            assert!(
                report.scanned <= ASYNC_RETENTION_BATCH_SIZE,
                "one pass must never consider more than one batch"
            );
            assert!(report.accounts_for_every_candidate());
            reaped += report.deleted_runs;
            passes += 1;
            if report.scanned == 0 {
                break;
            }
        }

        assert_eq!(reaped, 1_000);
        assert_eq!(scope.entries().await, vec![".async-retention"]);
    }

    #[tokio::test(start_paused = true)]
    async fn a_pass_evaluates_at_most_one_batch_and_returns_without_sleeping() {
        // On a paused clock, virtual time only advances when the runtime has nothing ready to run.
        // A pass that slept between batches would therefore move it; a bounded pass cannot.
        let scope = Scope::new().await;
        for index in 0..150 {
            scope
                .seed_run(&token(index), RunState::Complete, OLD_AGE_MS)
                .await;
        }
        let protected = BTreeSet::new();
        let identity = scope.identity();
        let mut options = scope.options(&protected, &identity);
        options.batch_size = 10;

        let before = tokio::time::Instant::now();
        let report = cleanup_async_retention(&options).await;

        assert_eq!(report.scanned, 10);
        assert_eq!(
            tokio::time::Instant::now().duration_since(before),
            std::time::Duration::ZERO,
            "the pass is bounded, not paced — `ASYNC_RETENTION_DELAY_MS` is a SCHEDULE delay"
        );
    }

    // ---------------------------------------------------------------------------------------
    // Tombstones and crash safety
    // ---------------------------------------------------------------------------------------

    /// Hand-build the state an interrupted delete leaves: a `.deleting-run-*` tree with a valid
    /// `status.json` inside it, plus the marker that attributes it back to its run id.
    async fn seed_interrupted_delete(scope: &Scope, token: &str, with_marker: bool) -> PathBuf {
        let dir = scope.seed_run(token, RunState::Complete, OLD_AGE_MS).await;
        let tombstone = scope
            .async_root
            .join(format!("{RUN_TOMBSTONE_PREFIX}{token}-abandoned"));
        tokio::fs::rename(&dir, &tombstone).await.unwrap();
        if with_marker {
            write_run_tombstone_marker(
                &scope.maintenance_root,
                &RunId::from_token(token.to_string()),
                &tombstone,
                scope.now,
            )
            .await
            .unwrap();
        }
        tombstone
    }

    #[tokio::test]
    async fn an_interrupted_delete_leaves_a_prefixed_tombstone_the_next_pass_reaps() {
        let scope = Scope::new().await;
        let tombstone = seed_interrupted_delete(&scope, "abandoned", true).await;

        let report = scope.sweep().await;

        assert_eq!(report.reaped_tombstones, 1);
        assert_eq!(report.deleted_runs, 0, "it is completed, not re-retired");
        assert!(!tombstone.exists());
        assert_eq!(
            scope.entries().await,
            vec![".async-retention"],
            "and the marker is cleaned up with it"
        );
        assert!(report.accounts_for_every_candidate());
    }

    #[tokio::test]
    async fn a_tombstone_whose_marker_is_missing_is_skipped_not_deleted() {
        let scope = Scope::new().await;
        let tombstone = seed_interrupted_delete(&scope, "orphaned", false).await;

        let report = scope.sweep().await;

        assert_eq!(report.reaped_tombstones, 0);
        assert_eq!(report.skipped.get("run-tombstone-marker").copied(), Some(1));
        assert!(
            tombstone.exists(),
            "after the rename the marker is the only thing mapping a tree back to its run id, so \
             an unattributable tombstone is fail-closed"
        );
    }

    #[tokio::test]
    async fn a_tombstone_inside_the_grace_is_kept() {
        // Reaching `tombstone-grace` at all requires a retention window shorter than the grace:
        // the tombstone tree's own mtime is one of `statusTimestamp`'s inputs, so a tree young
        // enough to be inside a 24 h grace is `recent` under the default 30-day window and is
        // spared one guard earlier. Shortening the window is what isolates THIS guard.
        let scope = Scope::new().await;
        let token = "in-grace";
        let dir = scope.seed_run(token, RunState::Complete, 0).await;
        let tombstone = scope
            .async_root
            .join(format!("{RUN_TOMBSTONE_PREFIX}{token}"));
        tokio::fs::rename(&dir, &tombstone).await.unwrap();
        write_run_tombstone_marker(
            &scope.maintenance_root,
            &RunId::from_token(token.to_string()),
            &tombstone,
            scope.now,
        )
        .await
        .unwrap();

        let protected = BTreeSet::new();
        let identity = scope.identity();
        let mut options = scope.options(&protected, &identity);
        options.retention_ms = 0;

        let report = cleanup_async_retention(&options).await;

        assert_eq!(report.skipped.get("tombstone-grace").copied(), Some(1));
        assert_eq!(report.reaped_tombstones, 0);
        assert!(tombstone.exists());
    }

    #[tokio::test]
    async fn a_wait_armed_before_the_pass_spares_a_tombstone_at_the_first_decision() {
        // A subscription that is already on disk when the window is built is seen by the FIRST
        // decision (`runSkipReason` row 5), so the tombstone is spared as `wait-reference` and the
        // re-check is never reached. This test was once named for the re-check; it cannot reach
        // it, and the two `recheck-` tests below do.
        let scope = Scope::new().await;
        let tombstone = seed_interrupted_delete(&scope, "raced", true).await;
        scope
            .seed_wait_subscription("3f2504e0-4f89-41d3-9a0c-0305e82c3302", "raced")
            .await;

        let report = scope.sweep().await;

        assert_eq!(report.reaped_tombstones, 0);
        assert_eq!(
            report.skipped.get("wait-reference").copied(),
            Some(1),
            "the FIRST decision already sees it, so it never reaches the re-check"
        );
        assert!(
            !report
                .skipped
                .keys()
                .any(|key| key.starts_with(RECHECK_REASON_PREFIX)),
            "and no `recheck-` key is raised: {:?}",
            report.skipped
        );
        assert!(tombstone.exists());
    }

    #[tokio::test]
    async fn a_tombstone_recheck_spares_the_tree_instead_of_deleting_it() {
        // pi `:794-797` — `reap_tombstone`'s own re-read of the wait set, the last guard before an
        // irreversible `rm -rf`, and the reason upstream re-reads the directory per candidate
        // instead of trusting the set it read before the loop.
        //
        // The whole sweep cannot produce this interleaving: both reads happen inside one call, so
        // the only way to arm a wait BETWEEN them is to drive the real `reap_tombstone` with facts
        // gathered before the subscription landed. Nothing here is hand-rolled — the facts come
        // from the production gatherer and the arm under test is the production one.
        let scope = Scope::new().await;
        let tombstone = seed_interrupted_delete(&scope, "raced", true).await;
        let tombstone_name = format!("{RUN_TOMBSTONE_PREFIX}raced-abandoned");
        let request = scope.scan_request();
        let facts = run_candidate_facts(request, &tombstone_name).await.unwrap();

        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = scope.options(&protected, &identity);
        assert_eq!(
            decide(
                &facts,
                options.now,
                options.retention_ms,
                options.tombstone_grace_ms,
                &protected,
                &BTreeSet::new()
            ),
            RetentionDecision::Reap,
            "the first decision really does select this tree for deletion"
        );

        // Armed only now — after the first decision, before the second.
        scope
            .seed_wait_subscription("3f2504e0-4f89-41d3-9a0c-0305e82c3304", "raced")
            .await;

        let mut state = PassState::default();
        let mut mutated = false;
        reap_tombstone(&options, &mut state, &facts, &mut mutated)
            .await
            .unwrap();

        assert_eq!(
            state.result.skipped.get("recheck-wait-reference").copied(),
            Some(1),
            "the `recheck-` prefix is what distinguishes a second-decision keep from a first: \
             {:?}",
            state.result.skipped
        );
        assert_eq!(state.result.reaped_tombstones, 0);
        assert!(!mutated, "the `rm -rf` never ran, so nothing was staged");
        assert!(
            tombstone.exists(),
            "the tree survives, which is the entire purpose of the second read"
        );
        assert!(
            super::super::run_tombstone_marker_path(
                &scope.maintenance_root,
                &RunId::from_token("raced".to_string())
            )
            .exists(),
            "and its marker stays, so the next pass can still attribute the tree"
        );
    }

    #[tokio::test]
    async fn a_recheck_after_the_rename_rolls_the_tree_back_under_its_own_name() {
        // pi `:817-828` — the rollback arm. The rename is already done when the second decision
        // says "keep", so the tree must be renamed BACK, the marker removed, and the skip recorded
        // under the `recheck-` prefix. Driven exactly as the tombstone re-check above: real facts,
        // gathered before the subscription landed, handed to the real `retire_run`.
        let scope = Scope::new().await;
        let run_dir = scope
            .seed_run("raced", RunState::Complete, OLD_AGE_MS)
            .await;
        let run_id = RunId::from_token("raced".to_string());
        let request = scope.scan_request();
        let facts = run_candidate_facts(request, "raced").await.unwrap();

        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = scope.options(&protected, &identity);
        assert_eq!(
            decide(
                &facts,
                options.now,
                options.retention_ms,
                options.tombstone_grace_ms,
                &protected,
                &BTreeSet::new()
            ),
            RetentionDecision::Tombstone
        );

        scope
            .seed_wait_subscription("3f2504e0-4f89-41d3-9a0c-0305e82c3305", "raced")
            .await;

        let mut state = PassState::default();
        let mut mutated = false;
        retire_run(&options, &mut state, &request, &facts, &mut mutated)
            .await
            .unwrap();

        assert_eq!(
            state.result.skipped.get("recheck-wait-reference").copied(),
            Some(1),
            "{:?}",
            state.result.skipped
        );
        assert_eq!(state.result.deleted_runs, 0);
        assert!(
            !mutated,
            "the tree went back under its own name, so the mutation is un-staged"
        );
        assert!(
            !state.cursor_commit_unsafe,
            "and a fully rolled-back candidate never poisons the cursor commit"
        );
        assert!(
            run_dir.exists(),
            "the run is addressable under its own id again"
        );
        assert!(
            !super::super::run_tombstone_marker_path(&scope.maintenance_root, &run_id).exists(),
            "and the marker minted for the abandoned rename is gone"
        );
        assert_eq!(
            scope.entries().await,
            vec![".async-retention", "raced"],
            "no `.deleting-run-*` tree is left behind"
        );
    }

    #[tokio::test]
    async fn a_failed_rename_rolls_the_tombstone_marker_back_and_leaves_the_tree() {
        // pi `:812-816` — `retire_run`'s rename-failure arm, which had NO test at all.
        //
        // Forcing it needs no permission trick and no seam: the rename TARGET is
        // `options.async_root/.deleting-run-<uuid>`, so an `async_root` that does not exist makes
        // `rename(2)` fail with `ENOENT` while the SOURCE — addressed by `facts.run_dir` — stays
        // perfectly real. That is the production race the arm is written for: the scope directory
        // removed out from under a pass that had already listed it.
        //
        // The invariant is that a marker is written BEFORE the rename and rolled back when the
        // rename fails — a marker naming a tree that was never created would block that run's
        // reap on every future pass until the self-heal caught it.
        let scope = Scope::new().await;
        let run_dir = scope
            .seed_run("doomed", RunState::Complete, OLD_AGE_MS)
            .await;
        let run_id = RunId::from_token("doomed".to_string());
        let request = scope.scan_request();
        let facts = run_candidate_facts(request, "doomed").await.unwrap();

        let vanished_root = scope._temp.path().join("async").join("cwd-key-removed");
        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = AsyncRetentionOptions {
            async_root: &vanished_root,
            ..scope.options(&protected, &identity)
        };

        let mut state = PassState::default();
        let mut mutated = false;
        let error = retire_run(&options, &mut state, &request, &facts, &mut mutated)
            .await
            .expect_err("a rename into a root that does not exist cannot succeed");

        assert!(
            errno::is_absent(&error),
            "and it is an ABSENT fault, which is why `:836` keeps it out of the report's \
             `errors`: {error}"
        );
        let marker = super::super::run_tombstone_marker_path(&scope.maintenance_root, &run_id);
        assert!(
            marker.parent().unwrap().exists(),
            "the markers directory exists, which proves the marker really was written before the \
             rename was attempted"
        );
        assert!(!marker.exists(), "and the rollback unlinked it");
        assert!(
            !mutated,
            "the rollback un-stages the mutation, so this candidate does not poison the cursor"
        );
        assert!(!state.cursor_commit_unsafe);
        assert_eq!(state.result.deleted_runs, 0);
        assert!(state.deleted_ids.is_empty());
        assert!(
            run_dir.join("status.json").exists(),
            "the run tree is exactly where it was, contents and all"
        );
        assert!(
            !scope
                .entries()
                .await
                .iter()
                .any(|name| name.starts_with(RUN_TOMBSTONE_PREFIX)),
            "and no tombstone was left behind"
        );
    }

    #[tokio::test]
    async fn a_crash_mid_sweep_leaves_a_re_evaluable_tree() {
        // Drive one pass, interrupt it by hand at its designed intermediate state (marker written,
        // tree renamed, delete not yet performed), then run to convergence.
        let scope = Scope::new().await;
        seed_interrupted_delete(&scope, "crashed", true).await;
        scope.seed_run("healthy", RunState::Complete, 0).await;

        let mut reaped = 0;
        for _ in 0..4 {
            let report = scope.sweep().await;
            reaped += report.reaped_tombstones + report.deleted_runs;
        }

        assert_eq!(reaped, 1);
        assert_eq!(
            scope.entries().await,
            vec![".async-retention", "healthy"],
            "the interrupted tree converges and the live one is untouched"
        );
    }

    // ---------------------------------------------------------------------------------------
    // The cross-instance lock, through the pass
    // ---------------------------------------------------------------------------------------

    async fn seed_foreign_lock(scope: &Scope, hostname: &str, pid: u32, started_at: i64) {
        let lock = lock_dir(&scope.maintenance_root);
        tokio::fs::create_dir_all(&lock).await.unwrap();
        tokio::fs::write(
            lock.join("owner.json"),
            serde_json::to_vec(&RetentionLockOwner {
                version: LockOwnerVersion,
                token: "other-instance".to_string(),
                pid,
                hostname: hostname.to_string(),
                started_at,
                process_start_identity: None,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_second_instance_is_refused_the_lock_and_reports_lock_busy() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        seed_foreign_lock(&scope, "test-host", std::process::id(), scope.now).await;

        let report = scope.sweep().await;

        assert!(!report.acquired);
        assert_eq!(report.skipped.get("lock-busy").copied(), Some(1));
        assert_eq!(report.scanned, 0);
        assert!(scope.async_root.join("orphan").exists());
    }

    #[tokio::test]
    async fn a_stale_lock_is_broken_and_the_pass_proceeds() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        seed_foreign_lock(&scope, "test-host", 424_242, scope.now).await;

        let protected = BTreeSet::new();
        let identity = RetentionLockIdentity {
            liveness: |_| Liveness::Dead,
            ..scope.identity()
        };
        let report = cleanup_async_retention(&scope.options(&protected, &identity)).await;

        assert!(report.acquired, "a dead owner on this host is a stale lock");
        assert_eq!(report.deleted_runs, 1);
    }

    #[tokio::test]
    async fn a_lock_owned_by_another_host_is_never_broken() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        // A dead-pid probe AND a year past the stale floor: this machine's pid table still says
        // nothing about another machine's, which is what keeps an NFS-shared scope safe.
        seed_foreign_lock(&scope, "elsewhere", 424_242, scope.now).await;

        let protected = BTreeSet::new();
        let identity = RetentionLockIdentity {
            liveness: |_| Liveness::Dead,
            ..scope.identity()
        };
        let mut options = scope.options(&protected, &identity);
        options.now = scope.now + LOCK_STALE_MS * 365;
        let report = cleanup_async_retention(&options).await;

        assert!(!report.acquired);
        assert_eq!(report.skipped.get("lock-busy").copied(), Some(1));
        assert!(scope.async_root.join("orphan").exists());
    }

    #[tokio::test]
    async fn two_scopes_never_share_a_lock_a_cursor_or_a_log() {
        // The `[CYRUP-DELTA]` that `maintenance_root` exists for: pi's
        // `path.dirname(asyncDirRoot)` default would be `<run_scratch>/async` here — shared by
        // EVERY working directory on the machine, so one project's sweep would block another's
        // forever and one project's cursor would advance past another's entries.
        let left = Scope::new().await;
        let right = Scope::new().await;

        assert_ne!(
            lock_dir(&left.maintenance_root),
            lock_dir(&right.maintenance_root)
        );
        assert!(
            left.maintenance_root.starts_with(&left.async_root),
            "the maintenance root is keyed by cwd for free by nesting inside the async root"
        );

        left.seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        right
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        seed_foreign_lock(&left, "test-host", std::process::id(), left.now).await;

        let blocked = left.sweep().await;
        let unblocked = right.sweep().await;

        assert!(!blocked.acquired);
        assert!(
            unblocked.acquired,
            "one scope's held lock must not block another scope's pass"
        );
        assert_eq!(unblocked.deleted_runs, 1);
    }

    // ---------------------------------------------------------------------------------------
    // The `running` repair hop
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_crashed_running_run_is_repaired_so_it_can_ever_be_reaped() {
        // Without this hop a run whose runner was SIGKILLed claims `Running` forever, is
        // `non-terminal` on every pass, and is NEVER reapable — which is precisely the orphan
        // class this module exists for.
        let scope = Scope::new().await;
        let dir = scope
            .seed_run("crashed", RunState::Running, OLD_AGE_MS)
            .await;
        let mut status: RunStatus =
            serde_json::from_slice(&tokio::fs::read(dir.join("status.json")).await.unwrap())
                .unwrap();
        status.pid = Some(424_242);
        scope.write_status(&dir, &status).await;
        backdate(&dir, scope.now - OLD_AGE_MS);

        let report = scope.sweep().await;

        assert_eq!(report.repaired_runs, 1);
        assert!(
            !report.skipped.contains_key("non-terminal"),
            "the run is terminal on disk after the hop, not before it: {:?}",
            report.skipped
        );
        let repaired: RunStatus =
            serde_json::from_slice(&tokio::fs::read(dir.join("status.json")).await.unwrap())
                .unwrap();
        assert!(repaired.state.is_terminal());
    }

    // ---------------------------------------------------------------------------------------
    // The boundary, the report and the log
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn the_sweep_never_touches_the_results_dir() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;

        let payload = scope.results_dir.join("orphan.json");
        let index = scope
            .results_dir
            .join("result-index")
            .join("observers")
            .join("mission")
            .join("other.json");
        let replay = scope
            .results_dir
            .join("completion-replay")
            .join("orphan.json");
        let archive = scope.results_dir.join("output-archives").join("orphan.txt");
        for path in [&payload, &index, &replay, &archive] {
            tokio::fs::create_dir_all(path.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(path, b"{}".as_slice()).await.unwrap();
        }

        let report = scope.sweep().await;

        assert_eq!(report.deleted_runs, 1, "the RUN half really did run");
        assert_eq!(report.deleted_results, 0);
        for path in [&payload, &index, &replay, &archive] {
            assert!(
                path.exists(),
                "{} belongs to result_index/completion_replay, never to this module",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn the_report_accounts_for_every_candidate() {
        // One reap, one tombstone reap, one skip and one induced error, in a single pass —
        // every bucket of the identity, exercised at once.
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        scope.seed_run("fresh", RunState::Complete, 0).await;
        seed_interrupted_delete(&scope, "abandoned", true).await;

        // The induced error: a DIRECTORY sitting where this run's tombstone marker belongs, so the
        // marker write fails and the candidate leaves the loop through `:834-837`. The marker is
        // written BEFORE the rename precisely so a failure here leaves the run tree where it was —
        // which is also why this candidate raises no `commit-failure`.
        let doomed = scope
            .seed_run("doomed", RunState::Complete, OLD_AGE_MS)
            .await;
        tokio::fs::create_dir_all(super::super::run_tombstone_marker_path(
            &scope.maintenance_root,
            &RunId::from_token("doomed".to_string()),
        ))
        .await
        .unwrap();

        let report = scope.sweep().await;

        assert_eq!(report.scanned, 4);
        assert_eq!(report.deleted_runs, 1);
        assert_eq!(report.reaped_tombstones, 1);
        assert_eq!(report.skipped.get("recent").copied(), Some(1));
        assert_eq!(report.errored_candidates, 1);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert!(
            report.accounts_for_every_candidate(),
            "scanned {} vs {:?}",
            report.scanned,
            report.skipped
        );
        assert!(
            doomed.exists(),
            "a candidate whose marker could not be written is never renamed, let alone deleted"
        );
        assert!(
            !report.skipped.contains_key("commit-failure"),
            "nothing was mutated, so the cursor is still safe to commit"
        );
    }

    #[tokio::test]
    async fn a_run_removed_before_the_pass_is_never_a_candidate_and_is_never_a_fault() {
        // A run tree that is already gone when the window is built is simply not listed: the pass
        // reports nothing, counts nothing and — the part that matters — raises no error for a
        // directory it never had. (The earlier name claimed this exercised `:836`'s `isNotFound`
        // filter inside the candidate loop; the loop is never entered here. That filter's input,
        // an absent-classified `io::Error` out of the candidate path, is asserted in
        // `a_failed_rename_rolls_the_tombstone_marker_back_and_leaves_the_tree`.)
        let scope = Scope::new().await;
        let doomed = scope
            .seed_run("doomed", RunState::Complete, OLD_AGE_MS)
            .await;
        tokio::fs::remove_dir_all(&doomed).await.unwrap();

        let report = scope.sweep().await;

        assert!(report.acquired, "the pass really ran");
        assert_eq!(report.scanned, 0);
        assert_eq!(report.errored_candidates, 0);
        assert!(report.errors.is_empty());
        assert_eq!(report.deleted_runs, 0);
        assert!(
            report
                .skipped
                .keys()
                .all(|reason| is_pass_level_skip(reason)),
            "no candidate-level skip can be raised for a candidate that never existed: {:?}",
            report.skipped
        );
        assert!(report.accounts_for_every_candidate());
    }

    #[tokio::test]
    async fn a_pass_that_cannot_reach_its_maintenance_root_sweeps_nothing() {
        // The lock, the cursor and the markers all live under the maintenance root. A pass that
        // cannot create it cannot take the cross-instance lock, and a pass without that lock must
        // not touch a SHARED async root at all — so the only correct outcome is to report and stop.
        //
        // A regular FILE where the directory belongs forces it, with no permission trick: root or
        // not, `create_dir_all` cannot turn a file into a directory.
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;
        tokio::fs::write(&scope.maintenance_root, b"not a directory".as_slice())
            .await
            .unwrap();

        let report = scope.sweep().await;

        assert!(!report.acquired);
        assert_eq!(
            report.skipped.get("maintenance-root-unavailable").copied(),
            Some(1)
        );
        assert_eq!(report.scanned, 0);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(report.deleted_runs, 0);
        assert!(
            scope.async_root.join("orphan").exists(),
            "a pass that could not take the lock must delete nothing"
        );
    }

    #[tokio::test]
    async fn a_discovery_fault_is_reported_instead_of_being_read_as_an_empty_root() {
        // The other half of `scan.rs`'s errno policy, at pass level: an async root whose listing
        // fails with something that is NOT absent-or-denied propagates out of the scan, and the
        // pass records `discovery-failure` rather than silently concluding there was nothing to do
        // — which would then advance the cursor over entries it never saw.
        let scope = Scope::new().await;
        let loop_a = scope._temp.path().join("loop-a");
        let loop_b = scope._temp.path().join("loop-b");
        tokio::fs::symlink(&loop_b, &loop_a).await.unwrap();
        tokio::fs::symlink(&loop_a, &loop_b).await.unwrap();

        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = AsyncRetentionOptions {
            async_root: &loop_a,
            ..scope.options(&protected, &identity)
        };
        let report = cleanup_async_retention(&options).await;

        assert!(
            report.acquired,
            "the lock lives in the maintenance root, which is fine"
        );
        assert_eq!(report.skipped.get("discovery-failure").copied(), Some(1));
        assert_eq!(report.scanned, 0);
        assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
        assert_eq!(
            super::super::read_cursor(&scope.maintenance_root)
                .await
                .run_after,
            None,
            "and a pass that could not discover anything never advances the cursor"
        );
    }

    #[tokio::test]
    async fn the_maintenance_log_is_written_even_when_the_lock_is_busy() {
        // pi `finish()` (`:691`) runs on EVERY return path including `:703`. That unconditionality
        // is what makes `/subagents-doctor` able to say "another instance holds the lock" instead
        // of silently rendering a zero.
        let scope = Scope::new().await;
        seed_foreign_lock(&scope, "test-host", std::process::id(), scope.now).await;

        let report = scope.sweep().await;
        assert!(!report.acquired);

        match super::super::read_last_maintenance_line(&scope.maintenance_root).await {
            super::super::MaintenanceLogRead::Line(line) => {
                assert!(!line.acquired);
                assert_eq!(line.skipped.get("lock-busy").copied(), Some(1));
            }
            other => panic!("the pass must always leave a record, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn every_pass_records_itself_for_the_doctor_to_read() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;

        scope.sweep().await;
        scope.sweep().await;

        let text =
            tokio::fs::read_to_string(super::super::maintenance_log_path(&scope.maintenance_root))
                .await
                .unwrap();
        assert_eq!(text.lines().count(), 2, "one line per pass");

        match super::super::read_last_maintenance_line(&scope.maintenance_root).await {
            super::super::MaintenanceLogRead::Line(line) => {
                assert_eq!(
                    line.deleted_runs, 0,
                    "the SECOND pass had nothing left to do"
                );
                assert!(line.acquired);
            }
            other => panic!("expected the last line, got {other:?}"),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Cancellation
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn a_cancelled_pass_stops_at_a_checkpoint_and_deletes_nothing() {
        let scope = Scope::new().await;
        scope
            .seed_run("orphan", RunState::Complete, OLD_AGE_MS)
            .await;

        let cancel = CancelToken::new();
        cancel.cancel();
        let protected = BTreeSet::new();
        let identity = scope.identity();
        let options = AsyncRetentionOptions {
            cancel: Some(&cancel),
            ..scope.options(&protected, &identity)
        };

        let report = cleanup_async_retention(&options).await;

        assert!(report.cancelled);
        assert_eq!(report.skipped.get("cancelled").copied(), Some(1));
        assert_eq!(report.deleted_runs, 0);
        assert!(scope.async_root.join("orphan").exists());
        assert!(
            !tokio::fs::try_exists(lock_dir(&scope.maintenance_root))
                .await
                .unwrap(),
            "a cancelled pass still releases its lock"
        );
    }
}
