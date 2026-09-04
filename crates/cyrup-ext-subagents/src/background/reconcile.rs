//! Stale-run liveness reconciliation (func-SA §5.4 R-SA-088..092; arch-SA §6.5).
//!
//! This module is the SOLE place the grace-window/staleness-threshold *decision* logic that
//! consumes [`RunStatus::started_at`]/[`RunStatus::last_update`] lives (see `background/mod.rs`'s
//! own module docs, which explicitly defer this here). Every other subsystem that needs to know
//! "is this run actually still alive" — `status`/`interrupt`/`resume`/`append-step` control
//! handlers (R-SA-079, later phase's `background/control.rs`), the orchestrator's shared poller
//! (R-SA-093, later phase's `background/tracker.rs`) — calls [`reconcile`], never re-derives this
//! algorithm.
//!
//! # The algorithm (R-SA-088, verbatim sequencing)
//!
//! Given a run id (resolved to its [`crate::background::RunPaths`]):
//!
//! 1. **If a [`ResultFile`] exists on disk, it is ALWAYS authoritative.** Read it, and if
//!    `status.json` still claims a non-terminal state, repair `status.json` from the result file's
//!    own `state` and write the repair back atomically. Return the (possibly just-repaired)
//!    status. This is a one-way street: the result file is never re-derived from `status.json`,
//!    only the reverse (R-SA-077's "readers MUST treat presence of the `ResultFile` as
//!    authoritative over a stale/contradictory `status.json`").
//! 2. **Else if `status.json` is absent entirely**, and we are still within the short spawn grace
//!    window (R-SA-090, target ~1000ms) after the confirmed hop-1 spawn, synthesize and return a
//!    provisional `Queued` status ([`RunStatus::provisional`]-shaped) rather than declaring the run
//!    failed — the detached runner may simply not have gotten around to writing its own first
//!    `status.json` yet. Outside the grace window with still no file on disk, the run is treated as
//!    failed (the spawn itself must be considered lost).
//! 3. **Else if `status.json` exists but does not claim `Running` with a numeric pid** (i.e. it is
//!    `Queued`, `Paused`, or already terminal), there is nothing to reconcile — return it unchanged.
//!    A `Paused` run is a soft, resumable, deliberately non-terminal state (R-SA-084) that
//!    reconciliation must not disturb on its own; a `Queued` run with no pid yet has nothing to
//!    probe.
//! 4. **Else (`status.json` claims `Running` with a numeric pid)**, classify the pid's liveness via
//!    a real zero-signal probe ([`check_pid_liveness`], R-SA-089's three-outcome classification):
//!    - **Dead** (`ESRCH`): the runner process is gone but never got to write a terminal status —
//!      synthesize a failure (R-SA-092) and write both files.
//!    - **Alive or Unknown, but `last_update` has not advanced for longer than `stale_after`**
//!      (R-SA-091, target 24h): OS pid reuse makes indefinite "alive" trust unsound past this
//!      threshold — synthesize a failure (R-SA-092) and write both files, exactly as for `Dead`.
//!    - **Alive or Unknown, still within the staleness threshold**: the run is presumed genuinely
//!      still in progress — return `status.json` unmodified.
//!
//! `Unknown` (a permission-denied-class probe failure, e.g. under sandboxing) is deliberately
//! treated identically to `Alive` for staleness purposes — R-SA-089 is explicit that "Unknown MUST
//! NOT be treated as dead": a probe that cannot confirm liveness is not evidence of death, only of
//! an inconclusive check, so it only tips over into a synthesized failure via the SAME staleness
//! path `Alive` does, never immediately.
//!
//! # Injectable clock (testability)
//!
//! Every "how long has it been" comparison in this module goes through a plain `now:
//! std::time::SystemTime` (or, for the liveness probe, a plain `check_liveness: impl Fn(u32) ->
//! Liveness`) parameter threaded in by the caller, rather than this module calling
//! `SystemTime::now()`/`nix::sys::signal::kill` internally and unconditionally. This is what lets
//! the long-staleness test below assert "genuinely alive but 25 hours stale" reconciles to `Failed`
//! without an actual 24-hour wait in CI (arch-SA §11's stated testing approach: "simulated via a
//! fake reconciliation clock rather than actually waiting 24h in CI") while every non-test call
//! site simply passes `SystemTime::now`/[`check_pid_liveness`] and gets the real behavior for free.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::background::{ResultFile, RunPaths, RunState, RunStatus, StepState};

// =================================================================================================
// Liveness (R-SA-089)
// =================================================================================================

/// The three-outcome classification a zero-signal liveness probe (`kill(pid, 0)` on Unix) MUST
/// distinguish (R-SA-089). **Never collapse [`Liveness::Unknown`] into [`Liveness::Dead`]** — an
/// `EPERM`-class probe failure commonly means the process exists but liveness cannot be confirmed
/// under sandboxing, which is a materially different situation from "no such process".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Liveness {
    /// The probe succeeded: the process exists (and this process has permission to signal it).
    Alive,
    /// The probe failed with "no such process" (`ESRCH`): the pid is confirmed gone.
    Dead,
    /// The probe failed with anything other than `ESRCH` (most commonly `EPERM`/permission-denied
    /// under sandboxing) — the process may well still exist; this check simply cannot confirm
    /// either way.
    Unknown,
}

impl Liveness {
    /// `true` for [`Liveness::Alive`] or [`Liveness::Unknown`] — the two outcomes that do NOT, on
    /// their own, prove the process is gone (R-SA-089's "Unknown MUST NOT be treated as dead").
    /// Reconciliation's staleness check (step 4 of the algorithm) applies identically to both.
    #[must_use]
    pub fn is_possibly_alive(self) -> bool {
        matches!(self, Liveness::Alive | Liveness::Unknown)
    }
}

/// A real zero-signal liveness probe against `pid` (R-SA-089): `kill(pid, 0)` sends no actual
/// signal — the kernel performs only existence/permission checks — so this never disturbs a live
/// process, it only observes it.
///
/// - `Ok(())` → [`Liveness::Alive`]: the pid exists and this process may signal it.
/// - `Err(Errno::ESRCH)` → [`Liveness::Dead`]: the kernel has confirmed no such process exists.
/// - `Err(_)` (most commonly `Errno::EPERM` under sandboxing, or any other unexpected errno) →
///   [`Liveness::Unknown`]: the probe is inconclusive, never treated as death.
///
/// On non-Unix targets, `nix::sys::signal::kill` is unavailable; conservatively reports
/// [`Liveness::Unknown`] for every pid rather than guessing, which routes every non-Unix
/// reconciliation decision through the same staleness-threshold path `Alive` uses, never the
/// immediate-failure `Dead` path.
#[must_use]
pub fn check_pid_liveness(pid: u32) -> Liveness {
    #[cfg(unix)]
    {
        // SIGNAL 0: existence/permission check only, no signal is actually delivered.
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None) {
            Ok(()) => Liveness::Alive,
            Err(nix::errno::Errno::ESRCH) => Liveness::Dead,
            Err(_) => Liveness::Unknown,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Liveness::Unknown
    }
}

// =================================================================================================
// Grace window / staleness thresholds (R-SA-090/091)
// =================================================================================================

/// Default spawn grace window (R-SA-090: "target ~1000ms") — how long `status.json` may remain
/// absent on disk after a confirmed successful hop-1 spawn before reconciliation gives up waiting
/// for the detached runner's first write and treats the run as failed.
pub const DEFAULT_SPAWN_GRACE: Duration = Duration::from_millis(1000);

/// Default long-staleness threshold (R-SA-091: "target 24h") — how long an alive-or-unknown pid's
/// `last_update` may go without advancing before reconciliation stops trusting it and synthesizes
/// a failure, on the rationale that OS pid reuse makes indefinite "alive" trust unsound.
pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

// =================================================================================================
// Reconciliation outcome
// =================================================================================================

/// The result of one [`reconcile`] call: the (possibly repaired/synthesized) status, plus whether
/// this call itself caused a write to disk — useful for callers (R-SA-092's `run.repaired_stale`
/// event, later phase's `background/tracker.rs`/`control.rs`) that need to know whether to emit a
/// diagnostic event, without re-diffing the before/after status themselves.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconcileOutcome {
    /// The status as it stands after reconciliation — always what a caller should treat as
    /// current truth from this point on.
    pub status: RunStatus,
    /// Which, if any, repair action reconciliation took.
    pub action: ReconcileAction,
}

/// Which repair action, if any, [`reconcile`] took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    /// Nothing needed reconciling — the on-disk `status.json` (or the provisional grace-window
    /// synthesis) was returned as-is.
    NoneNeeded,
    /// A pre-existing [`ResultFile`] was found and `status.json` was repaired to match it
    /// (algorithm step 1) because it still claimed a non-terminal state.
    RepairedFromResult,
    /// A dead-or-long-stale `Running` pid was detected and a synthesized failure was written to
    /// both `status.json` and a new [`ResultFile`] (R-SA-092, algorithm step 4).
    SynthesizedFailure,
    /// SUBA-057 — the run carries a [`RunStatus::display_dismissed_at`] marker and no [`ResultFile`]
    /// repaired it away, so reconciliation declines to report it at all: pi's
    /// `if (effectiveStatus.displayDismissedAt !== undefined) return { status: null, repaired:
    /// false, resultPath }` (`stale-run-reconciler.ts:359-361` @v0.47.1).
    ///
    /// **[CYRUP-DELTA] on the carrier, not the meaning.** Upstream signals this by returning a
    /// `null` status; [`ReconcileOutcome::status`] is not an `Option` (every one of its ~20 call
    /// sites treats it as "current truth", and widening it would push an `unwrap`-shaped decision
    /// into all of them), so the *action* carries the signal and the status field still carries the
    /// dismissed record for the one reader that wants it — the single-run status view, which needs
    /// `run_id` and `display_dismissed_at` to render pi's `State: display-dismissed` report
    /// (`run-status.ts:332-345`). Callers that upstream would hand a `null` MUST test the action:
    /// [`crate::background::run_status::list_active_runs`] does, and drops the run.
    ///
    /// No write is performed on this path — dismissal is display-only and reconciliation must not
    /// advance a dismissed run's state.
    DisplayDismissed,
}

// =================================================================================================
// reconcile (R-SA-088, the algorithm itself)
// =================================================================================================

/// Runs stale-run reconciliation for one background run, per R-SA-088's exact sequencing (see
/// module docs for the full walkthrough).
///
/// # Parameters
///
/// - `paths`: the run's resolved [`RunPaths`] (status/result file locations).
/// - `spawn_confirmed_at`: wall-clock time hop-1 confirmed a successful spawn — the reference point
///   the R-SA-090 grace window is measured from when `status.json` is absent entirely. `None` if
///   the caller has no such reference (e.g. reconciling an old run resolved purely by id, long
///   after any spawn-call-site context existed) — in that case a missing `status.json` is always
///   treated as outside the grace window (nothing to be provisional about).
/// - `now`: the current wall-clock time, injected so tests can simulate arbitrary elapsed time
///   without a real wait (see module docs).
/// - `check_liveness`: the liveness probe to use, injected for the identical reason — production
///   callers pass [`check_pid_liveness`]; tests can pass a closure that returns a fixed
///   [`Liveness`] for a pid regardless of whether it is real.
/// - `grace`: the spawn grace window duration (production default [`DEFAULT_SPAWN_GRACE`]).
/// - `stale_after`: the long-staleness threshold (production default [`DEFAULT_STALE_AFTER`]).
///
/// # Errors
///
/// Returns `std::io::Error` only for a genuine I/O failure reading or writing one of the run's
/// files (a missing file is not itself an error — see algorithm step 2/3 above; a present-but-
/// unparseable file IS surfaced as an error rather than silently discarded, since a corrupted
/// status/result file is a real anomaly a caller should be able to detect rather than have masked).
pub async fn reconcile(
    paths: &RunPaths,
    spawn_confirmed_at: Option<SystemTime>,
    now: SystemTime,
    check_liveness: impl Fn(u32) -> Liveness,
    grace: Duration,
    stale_after: Duration,
) -> std::io::Result<ReconcileOutcome> {
    // Step 1: a ResultFile is ALWAYS authoritative over status.json, unconditionally, before any
    // liveness probing is even considered (R-SA-088 item 1, R-SA-077).
    if let Some(result) = read_optional_json::<ResultFile>(&paths.result).await? {
        return repair_from_result(paths, result).await;
    }

    // status.json may or may not exist yet.
    let existing_status = read_optional_json::<RunStatus>(&paths.status).await?;

    let Some(mut status) = existing_status else {
        // Step 2: status.json is absent entirely. Within the spawn grace window, this is expected
        // (the runner hasn't written its first status yet) — synthesize a provisional status
        // rather than declaring failure. Outside the grace window (or with no spawn-time reference
        // at all), there is nothing to reconcile against: the caller gets an I/O-style "not found"
        // signal via `Ok(None)`-shaped provisional-failure handling is NOT this function's job —
        // this function only reconciles state that *some* record exists for. A caller with no
        // spawn_confirmed_at reference and no status.json has no run to reconcile in the first
        // place, so we return a synthesized `Queued` provisional in the grace-window case, and a
        // synthesized `Failed` (spawn presumed lost) otherwise — both cases still produce a
        // returnable RunStatus rather than an error, mirroring "reconciliation always yields a
        // status", never an I/O-not-found error for a legitimately-in-flight run.
        return Ok(reconcile_missing_status(
            paths,
            spawn_confirmed_at,
            now,
            grace,
        ));
    };

    // SUBA-057 — pi `if (effectiveStatus.displayDismissedAt !== undefined) return { status: null,
    // repaired: false, resultPath }` (`stale-run-reconciler.ts:359-361` @v0.47.1). Sequenced here,
    // in pi's own position: AFTER the result-file branch (a real result outranks the marker and
    // erases it, `:169`) and BEFORE the liveness probe (a dismissed run is deliberately never
    // probed, never repaired and never advanced — dismissal is display-only and terminates
    // nothing).
    if status.display_dismissed_at.is_some() {
        return Ok(ReconcileOutcome {
            status,
            action: ReconcileAction::DisplayDismissed,
        });
    }

    // Step 3: status.json exists but isn't claiming Running with a numeric pid — nothing to
    // reconcile (Queued/Paused/terminal all pass through unmodified).
    let (RunState::Running, Some(pid)) = (status.state, status.pid) else {
        return Ok(ReconcileOutcome {
            status,
            action: ReconcileAction::NoneNeeded,
        });
    };

    // Step 4: claims Running with a numeric pid — probe liveness.
    let liveness = check_liveness(pid);
    let stale = is_stale(status.last_update, now, stale_after);

    let should_fail = match liveness {
        Liveness::Dead => true,
        Liveness::Alive | Liveness::Unknown => stale,
    };

    if !should_fail {
        return Ok(ReconcileOutcome {
            status,
            action: ReconcileAction::NoneNeeded,
        });
    }

    let reason = match liveness {
        Liveness::Dead => "tracked pid is no longer running (zero-signal probe: no such process)",
        Liveness::Alive => {
            "tracked pid appears alive but has not reported progress past the long-staleness \
             threshold; OS pid reuse makes indefinite trust unsound"
        }
        Liveness::Unknown => {
            "tracked pid liveness could not be confirmed (permission-denied-class probe failure) \
             and has not reported progress past the long-staleness threshold"
        }
    };

    synthesize_failure(paths, &mut status, reason).await
}

/// Convenience wrapper over [`reconcile`] using the real wall clock and the real
/// [`check_pid_liveness`] probe — the call shape every non-test production call site
/// (`background/control.rs`, `background/tracker.rs`, later phases) actually uses.
///
/// # Errors
///
/// See [`reconcile`].
pub async fn reconcile_now(
    paths: &RunPaths,
    spawn_confirmed_at: Option<SystemTime>,
) -> std::io::Result<ReconcileOutcome> {
    reconcile(
        paths,
        spawn_confirmed_at,
        SystemTime::now(),
        check_pid_liveness,
        DEFAULT_SPAWN_GRACE,
        DEFAULT_STALE_AFTER,
    )
    .await
}

// =================================================================================================
// Internal helpers
// =================================================================================================

/// Step 1's repair action: a [`ResultFile`] exists on disk, so it is authoritative. If
/// `status.json` (read fresh here, independent of whatever the caller may already have loaded)
/// still claims a non-terminal state, repair it in place to match the result file's own `state`
/// and persist the repair atomically; otherwise leave it untouched. Either way, return the
/// resulting status.
async fn repair_from_result(
    paths: &RunPaths,
    result: ResultFile,
) -> std::io::Result<ReconcileOutcome> {
    let existing = read_optional_json::<RunStatus>(&paths.status).await?;

    let needs_repair = match &existing {
        Some(status) => !status.state.is_terminal(),
        None => true,
    };

    if !needs_repair {
        // Safe: `needs_repair` is false only in the `Some(status)` arm above.
        if let Some(status) = existing {
            // SUBA-057 — pi `if (effectiveStatus.displayDismissedAt === undefined) return { status:
            // effectiveStatus, repaired: false, resultPath }` (`stale-run-reconciler.ts:357`): with a
            // result file present but the status ALREADY terminal, upstream returns early only for a
            // NON-dismissed run; a dismissed one falls through to its own `status: null` return at
            // `:359-361`. The marker therefore still hides an already-terminal dismissed run, and only
            // an actual repair (below) erases it.
            let action = if status.display_dismissed_at.is_some() {
                ReconcileAction::DisplayDismissed
            } else {
                ReconcileAction::NoneNeeded
            };
            return Ok(ReconcileOutcome { status, action });
        }
    }

    let mut repaired =
        existing.unwrap_or_else(|| RunStatus::queued(result.run_id.clone(), result.mode, None));
    repaired.pid = repaired.pid.or(None);
    for (index, step) in repaired.steps.iter_mut().enumerate() {
        if !step.status.is_terminal() {
            // G77 — pi `childState(overallState, child)` (`stale-run-reconciler.ts:145-149`): a
            // child that reports its OWN `success` wins (`true` → complete, `false` → failed),
            // otherwise the step inherits the run's overall repaired state — which is where
            // `"stopped"` propagates from (`readResultRepairData`, `:126`: `data.success ?
            // "complete" : data.state === "stopped" ? "stopped" : …`). Before this, every
            // non-terminal step of a stopped run was repaired to `Failed`, erasing the stop.
            let child_stopped = result.results.get(index).is_some_and(|child| child.stopped);
            step.status = if child_stopped || result.state == RunState::Stopped {
                StepState::Stopped
            } else if result.success {
                StepState::Complete
            } else {
                StepState::Failed
            };
        }
    }
    // Force `state` to the result's own terminal state directly rather than routing through
    // `advance_state`'s transition guard: repair-from-result is a deliberate, authoritative
    // override of whatever `status.json` currently (possibly incorrectly) claims, not a normal
    // forward-progress transition, so it must succeed unconditionally even if the guard would
    // otherwise reject the jump (e.g. a corrupted `status.json` stuck in `Queued` while the result
    // file says `Complete`).
    repaired.state = result.state;
    // SUBA-057 — pi `delete terminalStatus.displayDismissedAt` (`stale-run-reconciler.ts:169`,
    // inside `terminalStatusFromResult`). A display-dismissal hides a run the operator judged
    // orphaned; the arrival of a real `ResultFile` proves it was not, so the marker is erased and
    // the run reappears with its genuine terminal outcome. Without this line a dismissed run whose
    // result landed a moment later would stay invisible forever.
    repaired.display_dismissed_at = None;
    repaired.ended_at = repaired
        .ended_at
        .or_else(|| Some(crate::time::epoch_millis(SystemTime::now())));
    repaired.last_update = crate::time::epoch_millis(SystemTime::now());

    crate::background::atomic::write_atomic_json(&paths.status, &repaired).await?;

    Ok(ReconcileOutcome {
        status: repaired,
        action: ReconcileAction::RepairedFromResult,
    })
}

/// Step 2's grace-window handling for a `status.json` that does not exist on disk at all.
fn reconcile_missing_status(
    paths: &RunPaths,
    spawn_confirmed_at: Option<SystemTime>,
    now: SystemTime,
    grace: Duration,
) -> ReconcileOutcome {
    let within_grace = spawn_confirmed_at
        .map(|spawned| now.duration_since(spawned).unwrap_or(Duration::ZERO) < grace)
        .unwrap_or(false);

    let run_id = run_id_from_paths(paths);

    if within_grace {
        // R-SA-090: still within the grace window — provisional, not failed. Mirrors
        // `RunStatus::provisional`'s shape exactly (Queued, pid unknown since we have no record of
        // it here — the real pid, if the caller has it, is what the spawn call site itself already
        // writes via `RunStatus::provisional` before this function would ever be invoked in
        // practice).
        return ReconcileOutcome {
            status: RunStatus::queued(run_id, crate::background::RunMode::Single, None),
            action: ReconcileAction::NoneNeeded,
        };
    }

    // Outside the grace window with still no status.json on disk at all: the spawn itself must be
    // considered lost. Synthesize a Failed status (no ResultFile is written here — with no
    // status.json ever having existed, there were never any steps to mark Failed, and writing a
    // ResultFile for a run that never even confirmed its own existence on disk would risk a
    // spurious completion notification for a run identity a caller may not even recognize; the
    // synthesized-failure-plus-both-files write in R-SA-092 applies specifically to the
    // claimed-Running-with-pid case, step 4 below, not this one).
    let mut status = RunStatus::queued(run_id, crate::background::RunMode::Single, None);
    status.state = RunState::Failed;
    let now_ms = crate::time::epoch_millis(now);
    status.last_update = now_ms;
    status.ended_at = Some(now_ms);
    ReconcileOutcome {
        status,
        action: ReconcileAction::NoneNeeded,
    }
}

/// `true` once `last_update` has not advanced for longer than `stale_after` relative to `now`
/// (R-SA-091).
fn is_stale(last_update_epoch_ms: i64, now: SystemTime, stale_after: Duration) -> bool {
    let now_ms = crate::time::epoch_millis(now);
    let elapsed_ms = now_ms.saturating_sub(last_update_epoch_ms);
    let elapsed = Duration::from_millis(u64::try_from(elapsed_ms.max(0)).unwrap_or(u64::MAX));
    elapsed >= stale_after
}

/// Step 4's failure-synthesis action (R-SA-092): mark every non-terminal step `Failed`, advance the
/// overall run state to `Failed`, write the repaired `status.json`, then write a freshly synthesized
/// [`ResultFile`] — in that order, mirroring R-SA-077's own "status before result" write ordering
/// even for a synthesized (not naturally-completed) terminal transition.
///
/// A bounded tail of the runner's own captured stderr (R-SA-092's "bounded tail... for
/// diagnostics") is included in the synthesized result's `final_output` when the runner's stderr
/// log is present and readable; its absence (or an I/O error reading it) never blocks the
/// synthesis itself — diagnostics are a best-effort enrichment, not a precondition for declaring
/// the run failed.
async fn synthesize_failure(
    paths: &RunPaths,
    status: &mut RunStatus,
    reason: &str,
) -> std::io::Result<ReconcileOutcome> {
    let now = SystemTime::now();
    let now_ms = crate::time::epoch_millis(now);

    for step in &mut status.steps {
        if !step.status.is_terminal() {
            step.status = StepState::Failed;
            step.error.get_or_insert_with(|| reason.to_string());
            step.ended_at.get_or_insert(now_ms);
        }
    }
    if let Some(groups) = &mut status.parallel_groups {
        for group in groups {
            for child in &mut group.children {
                if !child.status.is_terminal() {
                    child.status = StepState::Failed;
                    child.error.get_or_insert_with(|| reason.to_string());
                    child.ended_at.get_or_insert(now_ms);
                }
            }
        }
    }

    // Force the terminal transition directly, mirroring `repair_from_result`'s rationale: a
    // stale-dead run may currently be sitting in `Running`, which normally CAN transition to
    // `Failed` via the guard — but we bypass `advance_state` here too so this single code path
    // behaves identically regardless of which (already-legal-or-not) state a corrupted status
    // happened to be caught in, rather than silently swallowing a transition-guard error in a
    // function whose entire point is "make this stale record consistent no matter what it
    // currently says".
    status.state = RunState::Failed;
    status.last_update = now_ms;
    status.ended_at = Some(now_ms);

    crate::background::atomic::write_atomic_json(&paths.status, status).await?;

    let stderr_tail = read_stderr_tail(&paths.runner_stderr_log).await;
    let mut diagnostic = format!("subagent run reconciled as stale/dead: {reason}");
    if let Some(tail) = stderr_tail.filter(|tail| !tail.is_empty()) {
        diagnostic.push_str("\n\n--- runner stderr (tail) ---\n");
        diagnostic.push_str(&tail);
    }

    let synthesized_results = synthesize_step_results(status, &diagnostic);

    let result = ResultFile {
        id: status.run_id.clone(),
        run_id: status.run_id.clone(),
        agent: status
            .steps
            .first()
            .map(|s| s.agent.clone())
            .unwrap_or_else(|| status.run_id.as_str().to_string()),
        mode: status.mode,
        state: RunState::Failed,
        success: false,
        cwd: paths.run_dir.clone(),
        session_file: status.steps.first().and_then(|s| s.session_file.clone()),
        results: synthesized_results,
    };

    crate::background::atomic::write_atomic_json(&paths.result, &result).await?;

    Ok(ReconcileOutcome {
        status: status.clone(),
        action: ReconcileAction::SynthesizedFailure,
    })
}

/// Builds one synthesized [`crate::exec::SingleResult`] per step (or, if there are no steps at all
/// yet, exactly one placeholder result for the run as a whole) so the [`ResultFile::results`]
/// vector is never left empty for a run that had genuinely started executing — a downstream reader
/// walking `results` should always find at least one entry explaining what failed and why.
fn synthesize_step_results(status: &RunStatus, diagnostic: &str) -> Vec<crate::exec::SingleResult> {
    if status.steps.is_empty() {
        return vec![placeholder_result(
            status.run_id.as_str(),
            status.mode,
            diagnostic,
        )];
    }
    status
        .steps
        .iter()
        .map(|step| crate::exec::SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            agent: step.agent.clone(),
            task: String::new(),
            exit_code: -1,
            usage: step.usage.clone(),
            model: step.model.clone(),
            attempted_models: step.attempted_models.clone(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            stopped: false,
            process_signal: None,
            error: Some(step.error.clone().unwrap_or_else(|| diagnostic.to_string())),
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            // A reconciled placeholder describes a run cyrup could not read back, not a foreign
            // process it observed.
            runner: None,
            external_process: None,
        })
        .collect()
}

fn placeholder_result(
    agent: &str,
    mode: crate::background::RunMode,
    diagnostic: &str,
) -> crate::exec::SingleResult {
    let _ = mode;
    crate::exec::SingleResult {
        // SUBA-021: no usage budget on this path (see the field doc).
        usage_budget: None,
        turn_budget: None,
        turn_budget_exceeded: false,
        wrap_up_requested: false,
        agent: agent.to_string(),
        task: String::new(),
        exit_code: -1,
        usage: cyrup_core::Usage::default(),
        model: None,
        attempted_models: Vec::new(),
        model_attempts: Vec::new(),
        final_output: None,
        structured_output: None,
        acceptance: None,
        detached: false,
        interrupted: false,
        timed_out: false,
        stopped: false,
        process_signal: None,
        error: Some(diagnostic.to_string()),
        saved_output_path: None,
        tool_calls: Vec::new(),
        output_truncated: false,
        control_events: Vec::new(),
        progress: None,
        runner: None,
        external_process: None,
    }
}

/// Reads and parses a JSON file if it exists, returning `Ok(None)` (not an error) if the path is
/// absent. A present-but-unparseable file surfaces as a genuine `io::Error` (`InvalidData`) rather
/// than being silently treated as absent, since a corrupted status/result file is a real anomaly a
/// caller should be able to detect.
async fn read_optional_json<T: serde::de::DeserializeOwned>(
    path: &Path,
) -> std::io::Result<Option<T>> {
    match tokio::fs::read(path).await {
        Ok(bytes) => {
            let value = serde_json::from_slice(&bytes)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Best-effort read of a bounded tail of the runner's captured stderr log, for the diagnostic
/// context R-SA-092 asks a synthesized failure to include. Returns `None` (never an error) if the
/// log is missing or unreadable — diagnostics enrichment must never block failure synthesis itself.
async fn read_stderr_tail(path: &Path) -> Option<String> {
    /// Cap on how much of the stderr log is retained in a synthesized diagnostic — "bounded tail",
    /// not the whole (potentially unbounded) log.
    const STDERR_TAIL_CAP_BYTES: usize = 4096;

    let bytes = tokio::fs::read(path).await.ok()?;
    let start = bytes.len().saturating_sub(STDERR_TAIL_CAP_BYTES);
    let tail_bytes = bytes.get(start..)?;
    Some(String::from_utf8_lossy(tail_bytes).into_owned())
}

/// Best-effort recovery of a [`crate::background::RunId`] from a [`RunPaths`]' own status-file
/// path (its parent directory name is always the run id, per [`crate::background::RunPaths::for_run`]'s
/// construction). Falls back to an empty-token id in the practically-unreachable case the path has
/// no parent/file-name component at all, since this helper is only ever used to label a synthesized
/// placeholder status and must never panic on a malformed path.
fn run_id_from_paths(paths: &RunPaths) -> crate::background::RunId {
    paths
        .run_dir
        .file_name()
        .map(|name| crate::background::RunId::from_token(name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| crate::background::RunId::from_token(""))
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
    use crate::background::{RunId, RunMode, StepStatus};
    use std::process::Stdio;

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

    fn running_status(run_id: RunId, pid: u32, last_update: i64) -> RunStatus {
        let mut status = RunStatus::queued(run_id, RunMode::Single, Some(pid));
        status.state = RunState::Running;
        status.last_update = last_update;
        status.steps = vec![StepStatus {
            status: crate::background::StepState::Running,
            ..StepStatus::pending("researcher")
        }];
        status
    }

    /// Spawns a real child process and immediately kills+reaps it, returning a pid that is
    /// GENUINELY dead — a real OS-level fact, not a mocked one — for the "dead pid reconciles to
    /// Failed" test.
    fn spawn_and_reap_dead_pid() -> u32 {
        let mut child = std::process::Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("`true` spawns");
        let pid = child.id();
        let status = child.wait().expect("wait reaps the child");
        assert!(status.success(), "`true` must exit 0");
        pid
    }

    /// Spawns a real, genuinely long-lived child process for the "alive" tests. Returns the child
    /// handle (kept alive for the test's duration — dropping/killing it is the caller's job) and
    /// its pid.
    fn spawn_long_lived() -> (std::process::Child, u32) {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("`sleep 30` spawns");
        let pid = child.id();
        (child, pid)
    }

    // ---------------------------------------------------------------------------------------
    // Liveness classification
    // ---------------------------------------------------------------------------------------

    #[test]
    fn check_pid_liveness_reports_dead_for_a_genuinely_reaped_pid() {
        let pid = spawn_and_reap_dead_pid();
        assert_eq!(check_pid_liveness(pid), Liveness::Dead);
    }

    #[test]
    fn check_pid_liveness_reports_alive_for_a_genuinely_running_process() {
        let (mut child, pid) = spawn_long_lived();
        assert_eq!(check_pid_liveness(pid), Liveness::Alive);
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn liveness_is_possibly_alive_excludes_only_dead() {
        assert!(Liveness::Alive.is_possibly_alive());
        assert!(Liveness::Unknown.is_possibly_alive());
        assert!(!Liveness::Dead.is_possibly_alive());
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-088 step 1: ResultFile is always authoritative
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn result_file_present_repairs_a_still_running_status_json() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);

        let status = running_status(
            run_id.clone(),
            999_999,
            crate::time::epoch_millis(SystemTime::now()),
        );
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let result = ResultFile {
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
            .expect("write result");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| Liveness::Alive,
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::RepairedFromResult);
        assert_eq!(outcome.status.state, RunState::Complete);

        // The repair must actually have been persisted to disk, not just returned in-memory.
        let reread: RunStatus = serde_json::from_slice(
            &tokio::fs::read(&paths.status)
                .await
                .expect("status.json exists"),
        )
        .expect("valid JSON");
        assert_eq!(reread.state, RunState::Complete);
    }

    /// G77 — repair-from-result must carry a STOPPED terminal state onto the still-non-terminal
    /// steps as [`StepState::Stopped`], not as `Failed`.
    ///
    /// Upstream: `readResultRepairData` derives `state = data.success ? "complete" : data.state ===
    /// "stopped" ? "stopped" : …` (`stale-run-reconciler.ts:126`) and `childState` hands that
    /// overall state to each repaired step (`:145-149`), which then writes `status: state`
    /// (`:163`). Before this, every non-terminal step of a stopped run was repaired to `Failed`,
    /// erasing the stop from the status report an orchestrator reads back.
    #[tokio::test]
    async fn a_stopped_result_file_repairs_its_steps_to_stopped_not_failed() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);

        let status = running_status(
            run_id.clone(),
            999_999,
            crate::time::epoch_millis(SystemTime::now()),
        );
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let result = ResultFile {
            id: run_id.clone(),
            run_id: run_id.clone(),
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Stopped,
            success: false,
            cwd: paths.run_dir.clone(),
            session_file: None,
            results: Vec::new(),
        };
        crate::background::atomic::write_atomic_json(&paths.result, &result)
            .await
            .expect("write result");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| Liveness::Alive,
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::RepairedFromResult);
        assert_eq!(outcome.status.state, RunState::Stopped);
        assert_eq!(
            outcome.status.steps[0].status,
            StepState::Stopped,
            "the in-flight step must be repaired to Stopped, not Failed: {:?}",
            outcome.status.steps[0]
        );

        // Persisted, not merely returned.
        let reread: RunStatus = serde_json::from_slice(
            &tokio::fs::read(&paths.status)
                .await
                .expect("status.json exists"),
        )
        .expect("valid JSON");
        assert_eq!(reread.state, RunState::Stopped);
        assert_eq!(reread.steps[0].status, StepState::Stopped);
    }

    #[tokio::test]
    async fn result_file_present_and_status_already_terminal_is_a_no_op() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);

        let mut status = running_status(
            run_id.clone(),
            1,
            crate::time::epoch_millis(SystemTime::now()),
        );
        status.state = RunState::Failed;
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let result = ResultFile {
            id: run_id.clone(),
            run_id,
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Failed,
            success: false,
            cwd: paths.run_dir.clone(),
            session_file: None,
            results: Vec::new(),
        };
        crate::background::atomic::write_atomic_json(&paths.result, &result)
            .await
            .expect("write result");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| Liveness::Dead,
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::NoneNeeded);
        assert_eq!(outcome.status.state, RunState::Failed);
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-088 step 3: not claiming Running -> nothing to reconcile
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn paused_status_is_returned_unmodified() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let mut status = running_status(run_id, 42, crate::time::epoch_millis(SystemTime::now()));
        status.state = RunState::Paused;
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| panic!("liveness must not be probed for a Paused run"),
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::NoneNeeded);
        assert_eq!(outcome.status.state, RunState::Paused);
    }

    #[tokio::test]
    async fn queued_status_with_no_pid_is_returned_unmodified() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let status = RunStatus::queued(run_id, RunMode::Single, None);
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| panic!("liveness must not be probed without a numeric pid"),
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::NoneNeeded);
        assert_eq!(outcome.status.state, RunState::Queued);
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-090: spawn grace window for a missing status.json
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn missing_status_within_grace_window_is_provisional_not_failed() {
        let (_dir, paths) = temp_paths();
        let spawned_at = SystemTime::now();

        let outcome = reconcile(
            &paths,
            Some(spawned_at),
            spawned_at + Duration::from_millis(100),
            |_| panic!("liveness must not be probed when status.json is absent"),
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::NoneNeeded);
        assert_eq!(
            outcome.status.state,
            RunState::Queued,
            "within the grace window a missing status.json must be provisional (Queued), not Failed"
        );
    }

    #[tokio::test]
    async fn missing_status_past_grace_window_is_treated_as_failed() {
        let (_dir, paths) = temp_paths();
        let spawned_at = SystemTime::now();

        let outcome = reconcile(
            &paths,
            Some(spawned_at),
            spawned_at + Duration::from_secs(10),
            |_| panic!("liveness must not be probed when status.json is absent"),
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.status.state, RunState::Failed);
    }

    // ---------------------------------------------------------------------------------------
    // Real-pid outcome tests (the three cases this task's spec explicitly asks for)
    // ---------------------------------------------------------------------------------------

    /// A genuinely-dead pid (spawned and reaped for real, not mocked) reconciles to `Failed`, and
    /// BOTH `status.json` and a synthesized `ResultFile` are written to disk (R-SA-092).
    #[tokio::test]
    async fn genuinely_dead_pid_reconciles_to_failed_and_writes_both_files() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let dead_pid = spawn_and_reap_dead_pid();

        let status = running_status(
            run_id,
            dead_pid,
            crate::time::epoch_millis(SystemTime::now()),
        );
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            check_pid_liveness, // the REAL probe, against the REAL dead pid
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::SynthesizedFailure);
        assert_eq!(outcome.status.state, RunState::Failed);
        assert!(
            outcome
                .status
                .steps
                .iter()
                .all(|s| s.status == crate::background::StepState::Failed),
            "every non-terminal step must be marked Failed"
        );

        // Both files must actually exist on disk with Failed/success:false content.
        let status_bytes = tokio::fs::read(&paths.status)
            .await
            .expect("status.json exists");
        let reread_status: RunStatus = serde_json::from_slice(&status_bytes).expect("valid JSON");
        assert_eq!(reread_status.state, RunState::Failed);

        let result_bytes = tokio::fs::read(&paths.result)
            .await
            .expect("ResultFile exists");
        let reread_result: ResultFile = serde_json::from_slice(&result_bytes).expect("valid JSON");
        assert_eq!(reread_result.state, RunState::Failed);
        assert!(!reread_result.success);
        assert!(
            !reread_result.results.is_empty(),
            "a synthesized diagnostic result must be present"
        );
    }

    /// A genuinely-alive-but-stale-past-threshold pid (a REAL long-lived child, but with a fake
    /// injected clock claiming 25 hours have passed since its last update) also reconciles to
    /// `Failed` — proving OS pid reuse cannot be trusted indefinitely (R-SA-091, A-SA-13), WITHOUT
    /// an actual 24h wait in this test.
    #[tokio::test]
    async fn genuinely_alive_but_long_stale_pid_reconciles_to_failed() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let (mut child, alive_pid) = spawn_long_lived();

        let last_update = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        let status = running_status(run_id, alive_pid, crate::time::epoch_millis(last_update));
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(), // real "now" — the STALENESS comes from last_update being old
            check_pid_liveness, // the REAL probe, against the REAL alive pid
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(
            outcome.action,
            ReconcileAction::SynthesizedFailure,
            "an alive-but-25h-stale pid must be reconciled to Failed, never trusted indefinitely"
        );
        assert_eq!(outcome.status.state, RunState::Failed);

        let _ = child.kill();
        let _ = child.wait();
    }

    /// A genuinely-alive-and-active pid (real long-lived child, `last_update` fresh — well inside
    /// the staleness threshold) is left `Running`, entirely unmodified — no files rewritten, no
    /// spurious failure synthesized.
    #[tokio::test]
    async fn genuinely_alive_and_active_pid_is_left_running_unmodified() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let (mut child, alive_pid) = spawn_long_lived();

        let status = running_status(
            run_id,
            alive_pid,
            crate::time::epoch_millis(SystemTime::now()),
        );
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");
        let before_bytes = tokio::fs::read(&paths.status)
            .await
            .expect("status.json exists");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            check_pid_liveness, // the REAL probe, against the REAL alive pid
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(outcome.action, ReconcileAction::NoneNeeded);
        assert_eq!(outcome.status.state, RunState::Running);
        assert!(
            !paths.result.exists(),
            "no ResultFile should ever be synthesized for a genuinely still-active run"
        );

        // status.json on disk must be byte-identical to before reconcile ran — reconcile must not
        // have rewritten it just because it happened to inspect it.
        let after_bytes = tokio::fs::read(&paths.status)
            .await
            .expect("status.json still exists");
        assert_eq!(
            before_bytes, after_bytes,
            "reconcile must not touch status.json at all when the run is genuinely still active"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    // ---------------------------------------------------------------------------------------
    // Unknown liveness is never treated as Dead (R-SA-089)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn unknown_liveness_within_staleness_threshold_is_not_failed() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let status = running_status(
            run_id,
            123_456,
            crate::time::epoch_millis(SystemTime::now()),
        );
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| Liveness::Unknown,
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(
            outcome.action,
            ReconcileAction::NoneNeeded,
            "Unknown liveness must never be treated as Dead on its own"
        );
        assert_eq!(outcome.status.state, RunState::Running);
    }

    #[tokio::test]
    async fn unknown_liveness_past_staleness_threshold_reconciles_to_failed() {
        let (_dir, paths) = temp_paths();
        let run_id = run_id_from_paths(&paths);
        let last_update = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
        let status = running_status(run_id, 123_456, crate::time::epoch_millis(last_update));
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .expect("write status");

        let outcome = reconcile(
            &paths,
            None,
            SystemTime::now(),
            |_| Liveness::Unknown,
            DEFAULT_SPAWN_GRACE,
            DEFAULT_STALE_AFTER,
        )
        .await
        .expect("reconcile succeeds");

        assert_eq!(
            outcome.action,
            ReconcileAction::SynthesizedFailure,
            "Unknown liveness past the staleness threshold reconciles via the SAME path as Alive"
        );
    }

    // ---------------------------------------------------------------------------------------
    // is_stale
    // ---------------------------------------------------------------------------------------

    #[test]
    fn is_stale_false_just_under_threshold() {
        let now = SystemTime::now();
        let last_update = crate::time::epoch_millis(now - Duration::from_secs(23 * 60 * 60));
        assert!(!is_stale(last_update, now, DEFAULT_STALE_AFTER));
    }

    #[test]
    fn is_stale_true_just_over_threshold() {
        let now = SystemTime::now();
        let last_update = crate::time::epoch_millis(now - Duration::from_secs(25 * 60 * 60));
        assert!(is_stale(last_update, now, DEFAULT_STALE_AFTER));
    }

    #[test]
    fn is_stale_handles_a_last_update_in_the_future_without_underflow() {
        let now = SystemTime::now();
        let last_update = crate::time::epoch_millis(now + Duration::from_secs(60));
        assert!(!is_stale(last_update, now, DEFAULT_STALE_AFTER));
    }
}
