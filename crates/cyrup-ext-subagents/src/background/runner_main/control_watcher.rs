//! Control-plane intake for the detached runner (R-SA-081/082): the shared [`ControlFlags`],
//! the control-inbox watcher task, and the steer / child-stop routers it drives. Split out of
//! `background/runner_main.rs`; ports pi `runs/background/subagent-runner.ts`.

use super::entry::run_id_from_paths;
use super::events::append_event;
use super::status::{SharedStatus, lock_status, write_shared_status};
use crate::background::child_identity::{async_status_child_identity, positional_child_identity};
use crate::background::child_stop::{
    ChildStatusWord, ChildStopMarking, ChildStopRecord, ChildStopRegistry, child_status_event,
    mark_child_stop_requested,
};
use crate::background::control;
use crate::background::{RunPaths, RunState, StepState};
use crate::jsonl::RunEventLog;
use std::sync::Arc;

/// R-SA-082: control-inbox watcher, installed with the mandatory synchronous startup check
/// performed FIRST (catches a request written in the race window before the watcher attaches),
/// then a background task forwarding every watch notification into `interrupted`.
///
/// Returns the three flags folded into a [`ControlFlags`] plus the shared soft-interrupt token
/// they pre-cancel, both of which [`run`](super::run) hands to the watcher and to [`run_inner`](super::turn_loop::run_inner).
pub(super) async fn init_control_flags(
    run_paths: &RunPaths,
) -> (ControlFlags, cyrup_core::CancelToken) {
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // R-SA-084 mid-flight interrupt (`subagent-runner.ts:1583-1609`): the run-wide SHARED soft-
    // interrupt token. The control-inbox watcher cancels it the instant an interrupt lands, which
    // tears down whatever child is running RIGHT NOW (via `run_sync`'s `opts.interrupt` race)
    // rather than only being noticed between steps — the difference between actually stopping a
    // single long-running step's child and a no-op. `ExecSingleStepExecutor` clones this same token
    // into every dispatched step's `RunOptions::interrupt`.
    let interrupt_cancel = cyrup_core::CancelToken::new();
    // The second control-inbox verb (`control/timeout.json`, pi `TimeoutRequest`): an ancestor
    // whose own deadline expired cascades one of these into every live descendant's inbox, and it
    // gets the identical synchronous-startup-check-then-watch treatment the interrupt flag does,
    // for the identical reason — a request written in the race window before the watcher attaches
    // must not be missed.
    let timed_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // G77 — the THIRD control-inbox verb (`control/stop.json`, pi `StopRequest`): an explicit
    // user/agent stop, or an ancestor's stop cascaded down. Same mandatory
    // synchronous-startup-check-then-watch treatment as the other two, for the same reason.
    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    if control::check_stop_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        interrupt_cancel.cancel();
    }
    if control::check_timeout_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
        interrupt_cancel.cancel();
    }
    if control::check_control_inbox_now(run_paths)
        .await
        .ok()
        .flatten()
        .is_some()
    {
        interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
        // An interrupt already pending at startup (written in the race window before the watcher
        // attaches) must likewise pre-cancel the shared token so the very first dispatched step's
        // child is torn down mid-flight, not merely noticed after it finishes.
        interrupt_cancel.cancel();
    }
    let control_flags = ControlFlags {
        interrupted: Arc::clone(&interrupted),
        timed_out: Arc::clone(&timed_out),
        stopped: Arc::clone(&stopped),
        child_stops: ChildStopRegistry::new(),
    };
    (control_flags, interrupt_cancel)
}

/// The two control-inbox verbs' pending flags, shared between the watcher task that SETS them and
/// the step loop that consumes them. Bundled into one struct rather than passed as two loose
/// `Arc<AtomicBool>`s so adding the timeout verb did not push [`run_inner`](super::turn_loop::run_inner) past clippy's argument
/// ceiling — and so the pair stays visibly a pair (they are always created, cloned and read
/// together, and the loop's ordering between them is load-bearing).
#[derive(Clone)]
pub(super) struct ControlFlags {
    /// A `control/interrupt.json` is pending (soft, resumable pause).
    pub(super) interrupted: Arc<std::sync::atomic::AtomicBool>,
    /// A `control/timeout.json` is pending (terminal deadline failure).
    pub(super) timed_out: Arc<std::sync::atomic::AtomicBool>,
    /// G77 — a `control/stop.json` is pending (terminal, non-resumable explicit stop). The THIRD
    /// verb, checked before the other two everywhere the three are drained together, matching pi's
    /// own inbox order (`runs/background/control-channel.ts:653-655` @v0.43.0: `consumeStopRequest` → then
    /// `consumeTimeoutRequest` → then `consumeInterruptRequest`) and `stopRunner`'s own
    /// `if (stopped || timedOut || interrupted || state !== "running") return` mutual exclusion
    /// (`subagent-runner.ts:2955-2986`).
    pub(super) stopped: Arc<std::sync::atomic::AtomicBool>,
    /// SUBA-087 — the CHILD-SCOPED stop registry (pi `childStopRequests` + `activeChildStops`,
    /// `subagent-runner.ts:2595-2596` @v0.64.0), shared by the watcher task that receives a
    /// targeted `control/stop-requests/*.json`, the executor that registers each dispatched step's
    /// stop handle, and the step loop that skips a step whose stop was queued before it started.
    /// Unlike the three flags it is not a verdict on the RUN: a child-scoped stop leaves the run
    /// `Running`.
    pub(super) child_stops: ChildStopRegistry,
}

// =================================================================================================
// install_ignored_sigusr2_handler — survive R-SA-081's best-effort wake-up signal
// =================================================================================================

/// Install a handler for `SIGUSR2` (R-SA-081's best-effort wake-up signal, sent by
/// `control::deliver_wakeup_signal` to nudge this runner's control-inbox watcher awake sooner)
/// that does nothing but drain and discard every received signal, for as long as the returned
/// task handle is kept alive.
///
/// This is REQUIRED, not defensive-programming excess: `SIGUSR2`'s default disposition on every
/// Unix target this crate ships to (Linux, macOS) is process TERMINATION. Without a registered
/// handler, `interrupt()`'s signal send would kill this runner process outright — silently
/// converting every "soft" R-SA-084 interrupt into a hard crash before `run_inner`'s own
/// cooperative `interrupted` flag ever gets a chance to observe anything, which would make a
/// `Paused` outcome unreachable by the very code path that is supposed to produce it.
///
/// The handler itself does nothing with the signal's payload — `control::watch_control_inbox`'s
/// filesystem notification (started by [`spawn_control_watcher`] immediately after this function
/// is called) and `run_inner`'s own per-iteration re-check are the actual authoritative source of
/// "an interrupt/append request landed" (DI-SA-9). This handler's only job is to exist for the
/// life of the run so the OS never falls back to terminating the process on receipt.
///
/// # Errors / fallback
///
/// If installing the signal listener itself fails (e.g. resource exhaustion), this degrades the
/// SAME way `spawn_control_watcher`'s own installation failure already degrades: the run
/// continues without the wake-up-signal fast path, relying purely on the poll-interval side of
/// `control::watch_control_inbox`'s `PollWatcher` and `run_inner`'s own per-iteration re-check —
/// never a hard failure of the run itself. In that one failure case, this function returns `None`
/// and the caller simply holds nothing (no guard needed: no handler was installed, so there is
/// nothing this crate did to make `SIGUSR2`'s default disposition worse than it already was
/// before this function was ever added).
#[cfg(unix)]
pub(super) fn install_ignored_sigusr2_handler() -> Option<SigUsr2Guard> {
    let mut stream =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined2()).ok()?;
    let handle = tokio::spawn(async move {
        loop {
            // `recv()` returning `None` means the underlying signal stream has been torn down
            // (process-wide signal-handling shutdown) — nothing further to drain in that case.
            if stream.recv().await.is_none() {
                return;
            }
        }
    });
    Some(SigUsr2Guard { handle })
}

/// RAII wrapper aborting the SIGUSR2-draining task on drop, mirroring
/// [`ControlWatcherHandle`]'s identical pattern immediately below.
#[cfg(unix)]
pub(super) struct SigUsr2Guard {
    handle: tokio::task::JoinHandle<()>,
}

#[cfg(unix)]
impl Drop for SigUsr2Guard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

// =================================================================================================
// spawn_control_watcher — background task forwarding control-inbox notifications
// =================================================================================================

/// Spawn a background task that installs [`control::watch_control_inbox`] and sets `interrupted`
/// whenever a notification arrives, for the duration of the returned [`ControlWatcherHandle`]
/// (dropping it stops the watch — the underlying `notify::PollWatcher` is dropped inside the
/// spawned task once the task itself is aborted, which happens automatically when
/// [`ControlWatcherHandle`] is dropped, since it wraps a [`tokio::task::JoinHandle`] with
/// `abort_on_drop`-equivalent semantics achieved via an explicit `Drop` impl below rather than
/// relying on any external crate).
///
/// # R-SA-082's two mechanisms, both present
///
/// This satisfies R-SA-082's "MUST watch its control inbox via both a filesystem-notification
/// mechanism and a fixed-interval poll fallback" via [`control::watch_control_inbox`]'s own
/// `notify::PollWatcher`-based implementation (that module's own doc comment explains why
/// `PollWatcher` IS simultaneously both halves: it does not depend on a native OS notification
/// backend being available, so there is no separate native-vs-poll branch to maintain here). The
/// mandatory synchronous startup check (the other half of R-SA-082) is performed by [`run`](super::run)
/// itself, BEFORE this function is called — never inside this function — matching
/// `control::check_control_inbox_now`'s own documented "caller MUST invoke this once before
/// installing any asynchronous watch" contract.
pub(super) fn spawn_control_watcher(
    run_paths: RunPaths,
    flags: ControlFlags,
    interrupt_cancel: cyrup_core::CancelToken,
    shared_status: SharedStatus,
) -> ControlWatcherHandle {
    let ControlFlags {
        interrupted,
        timed_out,
        stopped,
        child_stops,
    } = flags;
    let handle = tokio::spawn(async move {
        // G90: the steer queue's own `events.jsonl` writer. A second `RunEventLog` on the same
        // file is safe: it opens in append mode and writes each line in one `write_all`, and the
        // steering lines it writes are LIFECYCLE lines, which are never capped (pi `appendJsonl`,
        // `subagent-runner.ts:2865-2868` @v0.68.0). The diagnostic budget, which only the
        // telemetry pump's handle spends, is measured against the file as it actually is, so these
        // lines count against it exactly as upstream's shared per-path counter counts them.
        let mut events = RunEventLog::create(&run_paths.events).await.ok();
        // pi's in-memory `pendingStepSteers` (`subagent-runner.ts:1332,2071-2075` @v0.34.0): a steer that
        // arrives while its target child is still `pending` is HELD, not dropped, and re-attempted.
        //
        // [CYRUP-DELTA] pi flushes the pending queue from an explicit per-step
        // `flushPendingStepSteers(flatIndex)` hook at each dispatch site; this task instead
        // re-attempts on the SAME fixed interval pi's own `watchAsyncControlInbox` runs its poll
        // safety net at (`runs/background/control-channel.ts:625-692`). Same guarantee — a held steer lands as soon as
        // its child starts running — reached through the polling half of R-SA-082 rather than a new
        // hook threaded through three dispatch sites. It also closes a real gap: cyrup's watcher
        // previously had NO interval at all, so it depended entirely on `notify` firing.
        let mut pending: Vec<control::SteerRequest> = Vec::new();
        let mut ticker = tokio::time::interval(control::CONTROL_INBOX_POLL_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let (watcher, mut rx) = match control::watch_control_inbox(&run_paths) {
            Ok(pair) => pair,
            Err(_) => {
                // R-SA-082's watch is best-effort defense in depth on top of `run_inner`'s own
                // per-iteration `control::list_pending_appends`/interrupt re-check — a watcher
                // that fails to install (e.g. EMFILE/ENOSPC-class resource exhaustion) does not
                // strand the run: the step loop still re-checks `interrupted`/pending appends on
                // every iteration regardless of whether this watcher is alive at all. This task
                // simply has nothing further to do.
                return;
            }
        };
        // Keep the watcher alive for the lifetime of this task (dropping it would stop the watch)
        // — held in this local binding rather than discarded.
        let _watcher = watcher;
        loop {
            // G90 turned this from a bare `rx.recv()` await into a two-arm select: the interval arm
            // is what retries a HELD steer whose target child was still `pending` last time round,
            // and is also the poll safety net pi's `watchAsyncControlInbox` has always had.
            tokio::select! {
                message = rx.recv() => {
                    if message.is_none() {
                        break;
                    }
                }
                _ = ticker.tick() => {}
            }
            // G90: drain the steer queue and route it, BEFORE the interrupt/timeout checks. Order
            // matters and is pi's: a steer is non-terminal guidance for a child that is expected to
            // keep running, so it must be handed over before an interrupt landing in the same tick
            // tears that child down.
            route_steer_requests(&run_paths, &shared_status, &mut events, &mut pending).await;
            // SUBA-087: child-scoped stops are routed here too — they tear ONE child down and
            // must never flip the run-wide `stopped` flag probed just below.
            route_child_stop_requests(&run_paths, &shared_status, &mut events, &child_stops).await;
            // The control inbox now holds TWO distinct request files (`interrupt.json` and
            // `timeout.json`), so a notification is no longer self-describing: this task must ask
            // WHICH one is pending rather than blindly assuming "interrupt". Blindly setting
            // `interrupted` on a timeout delivery would tear the live child down under the wrong
            // verdict and end the run `Paused` (resumable) when it must end `Failed`/timed-out.
            //
            // Timeout is checked first, matching pi's own drain order (`runs/background/control-channel.ts:608-609`
            // @v0.34.0) and this run loop's own top-of-iteration ordering.
            let mut wake = false;
            // G77: stop is probed FIRST, matching pi's fixed drain order
            // (`runs/background/control-channel.ts:653-655`) — when a stop and a timeout/interrupt land in the same
            // tick, the run must end `Stopped`, the hardest and least-resumable of the three.
            if control::check_stop_inbox_now(&run_paths)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                stopped.store(true, std::sync::atomic::Ordering::SeqCst);
                wake = true;
            }
            if control::check_timeout_inbox_now(&run_paths)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                timed_out.store(true, std::sync::atomic::Ordering::SeqCst);
                wake = true;
            }
            if control::check_control_inbox_now(&run_paths)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                interrupted.store(true, std::sync::atomic::Ordering::SeqCst);
                wake = true;
            }
            if wake {
                // R-SA-084 mid-flight interrupt: cancelling the run-wide shared interrupt token
                // tears down whatever child is running RIGHT NOW (via `run_sync`'s
                // `opts.interrupt` race), rather than waiting for the step loop's next
                // between-steps check — the difference between a control request that actually
                // stops a single long-running step's child and one that is a no-op until the
                // (never-arriving) next step. pi does the same for both verbs (`interruptRunner`
                // signals the live children; `timeoutRunner` aborts via `timeoutAbortController`).
                interrupt_cancel.cancel();
            }
        }
    });
    ControlWatcherHandle { handle }
}

/// G90 — the runner's steer router, pi `deliverSteerRequest` + the `onSteer` inbox handler
/// (`subagent-runner.ts:1740-1790,2066-2076` @v0.34.0).
///
/// One tick: drain `<run_dir>/control/steer-requests/`, merge whatever was HELD from previous ticks,
/// and for each request in `ts` order decide per target child whether it can be handed over right
/// now. An accepted request is copied into that child's own inbox
/// ([`control::enqueue_step_steer`]), counted on the step and on the run
/// ([`crate::background::StepTelemetry::steer_count`]), and logged to `events.jsonl` as
/// `subagent.steer.requested` with pi's exact `acceptedIndexes`/`rejected` payload — so
/// `subagent({ action: "status", id })` shows the acceptance and `events.jsonl` shows the full
/// decision, which is what makes `action: "steer"` observable rather than fire-and-forget.
///
/// A request whose target is still `pending` is put back on `pending` for the next tick, matching
/// pi's `pendingStepSteers`. A request that lands while the run is not `Running` at all is held the
/// same way rather than discarded: pi returns early from `deliverSteerRequest`, and its request has
/// already been removed from the run-level queue by `consumeSteerRequests`, so holding it here is
/// strictly closer to pi's INTENT (`pendingStepSteers` exists precisely so early steers survive) —
/// and the whole queue is dropped with this task when the run ends either way.
async fn route_steer_requests(
    run_paths: &RunPaths,
    shared: &SharedStatus,
    events: &mut Option<RunEventLog>,
    pending: &mut Vec<control::SteerRequest>,
) {
    let mut queue = std::mem::take(pending);
    queue.extend(control::consume_steer_requests(&run_paths.run_dir).await);
    if queue.is_empty() {
        return;
    }
    queue.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| a.id.cmp(&b.id)));

    let mut status_dirty = false;
    for request in queue {
        // Snapshot the decision inputs under the lock, then release it — `enqueue_step_steer` is
        // `.await`-ing filesystem work and a `std::sync::Mutex` guard must never cross an await.
        let (run_state, step_states): (RunState, Vec<StepState>) = {
            let status = lock_status(shared);
            (
                status.state,
                status.steps.iter().map(|s| s.status).collect(),
            )
        };
        if run_state != RunState::Running {
            pending.push(request);
            continue;
        }
        let targets: Vec<usize> = match request.target_index {
            Some(index) => vec![index],
            None => step_states
                .iter()
                .enumerate()
                .filter(|(_, state)| **state == StepState::Running)
                .map(|(index, _)| index)
                .collect(),
        };
        // No running child yet and no explicit target: hold rather than reject, so a steer racing
        // the very first dispatch is not lost (pi's `else pendingStepSteers.push(request)`).
        if targets.is_empty() {
            pending.push(request);
            continue;
        }

        let mut accepted: Vec<usize> = Vec::new();
        let mut rejected: Vec<serde_json::Value> = Vec::new();
        let mut held = false;
        for index in targets {
            match step_states.get(index) {
                None => rejected.push(
                    serde_json::json!({ "index": index, "reason": "child index out of range" }),
                ),
                Some(StepState::Pending) => held = true,
                Some(StepState::Running) => {
                    if control::enqueue_step_steer(&run_paths.run_dir, index, &request)
                        .await
                        .is_ok()
                    {
                        accepted.push(index);
                    } else {
                        rejected.push(serde_json::json!({
                            "index": index,
                            "reason": "child inbox write failed"
                        }));
                    }
                }
                Some(other) => rejected.push(serde_json::json!({
                    "index": index,
                    "reason": format!("child is {}", crate::background::run_status::step_state_label(*other))
                })),
            }
        }
        if held && accepted.is_empty() && rejected.is_empty() {
            pending.push(request);
            continue;
        }

        let now = crate::time::now_epoch_millis();
        if !accepted.is_empty() {
            let mut status = lock_status(shared);
            for index in &accepted {
                if let Some(step) = status.steps.get_mut(*index) {
                    step.telemetry.steer_count =
                        Some(step.telemetry.steer_count.unwrap_or(0).saturating_add(1));
                    step.telemetry.last_steer_at = Some(now);
                }
            }
            let total = u64::try_from(accepted.len()).unwrap_or(0);
            status.telemetry.steer_count = Some(
                status
                    .telemetry
                    .steer_count
                    .unwrap_or(0)
                    .saturating_add(total),
            );
            status.telemetry.last_steer_at = Some(now);
            status.last_update = now;
            status_dirty = true;
        }

        let mut payload = serde_json::json!({
            "runId": run_paths
                .run_dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "requestId": request.id,
            "message": request.message,
            "acceptedIndexes": accepted,
        });
        if let Some(map) = payload.as_object_mut() {
            if let Some(source) = request.source.as_ref() {
                map.insert("source".to_string(), serde_json::json!(source));
            }
            if let Some(target) = request.target_index {
                map.insert("targetIndex".to_string(), serde_json::json!(target));
            }
            if !rejected.is_empty() {
                map.insert("rejected".to_string(), serde_json::json!(rejected));
            }
        }
        append_event(events, "subagent.steer.requested", Some(payload)).await;
    }

    if status_dirty {
        let _ = write_shared_status(run_paths, shared).await;
    }
}

/// SUBA-087 — the runner's child-scoped stop router, pi `stopChildStep` as `watchAsyncControlInbox`'s
/// `onStop` (`subagent-runner.ts:3015-3031,3900` @v0.64.0; drained at
/// `runs/background/control-channel.ts:690`).
///
/// One tick: consume every `control/stop-requests/*.json` that carries a `targetIndex` (whole-run
/// requests are left for the loop-top stop branch), oldest first, and for each:
///
/// * `childId` defaults to the step's own identity (`request.childId ??
///   childStopTargetId(request.targetIndex)`, `:3020`);
/// * `markChildStopRequested` gates on pending/running (`:2979-2991`) — a refusal is
///   `subagent.step.stop_failed` with `Child is not pending or running.` (`:3023`);
/// * an accepted request is recorded, the step's `stopRequested`/`stopRequestedAt` written, and
///   `subagent.step.stop_requested` + `subagent.child-status` `stopping` appended (`:2988-2989`);
/// * the live child's stop handle fires if there is one, else a still-`pending` step gets
///   `subagent.step.stop_queued` (`:3026-3030`) and the loop applies it at dispatch.
async fn route_child_stop_requests(
    run_paths: &RunPaths,
    shared: &SharedStatus,
    events: &mut Option<RunEventLog>,
    registry: &ChildStopRegistry,
) {
    let requests = control::consume_child_stop_requests(&run_paths.run_dir).await;
    if requests.is_empty() {
        return;
    }
    let run_id = run_id_from_paths(run_paths);
    for request in requests {
        let Some(index) = request.target_index else {
            continue;
        };
        let now = crate::time::now_epoch_millis();
        let (child_id, marking) = {
            let mut status = lock_status(shared);
            let child_id = request.child_id.clone().unwrap_or_else(|| {
                status
                    .steps
                    .get(index)
                    .map(|step| async_status_child_identity(step, index))
                    .unwrap_or_else(|| positional_child_identity(index))
            });
            let marking = mark_child_stop_requested(&mut status, index, &child_id, now);
            (child_id, marking)
        };
        match marking {
            ChildStopMarking::NotStoppable => {
                append_event(
                    events,
                    "subagent.step.stop_failed",
                    Some(serde_json::json!({
                        "runId": run_id.as_str(),
                        "stepIndex": index,
                        "childId": child_id,
                        "message": "Child is not pending or running.",
                    })),
                )
                .await;
            }
            ChildStopMarking::Requested {
                child_id,
                agent,
                was_pending,
            } => {
                registry.record(
                    index,
                    ChildStopRecord {
                        child_id: child_id.clone(),
                        requested_at: now,
                    },
                );
                // Best effort, as pi's `writeStatusPayload` is (`subagent-runner.ts:2988`): the
                // in-memory status and the registry already carry the request, and the next
                // status write republishes it — but a silent miss here would make a `stopping`
                // that the parent never sees in `status.json` unexplainable, so say so.
                if let Err(error) = write_shared_status(run_paths, shared).await {
                    tracing::warn!(
                        step_index = index,
                        child_id = %child_id,
                        %error,
                        "child-scoped stop accepted but status.json could not be written"
                    );
                }
                append_event(
                    events,
                    "subagent.step.stop_requested",
                    Some(serde_json::json!({
                        "runId": run_id.as_str(),
                        "stepIndex": index,
                        "childId": child_id,
                        "agent": agent,
                    })),
                )
                .await;
                append_event(
                    events,
                    "subagent.child-status",
                    Some(child_status_event(
                        run_id.as_str(),
                        index,
                        &child_id,
                        &agent,
                        ChildStatusWord::Stopping,
                        now,
                    )),
                )
                .await;
                if !registry.cancel_active(index) && was_pending {
                    append_event(
                        events,
                        "subagent.step.stop_queued",
                        Some(serde_json::json!({
                            "runId": run_id.as_str(),
                            "stepIndex": index,
                            "childId": child_id,
                        })),
                    )
                    .await;
                }
            }
        }
    }
}

/// RAII wrapper aborting the spawned control-inbox watcher task on drop, so a caller ([`run`](super::run))
/// never needs to remember to clean it up explicitly — the watcher's only useful lifetime is the
/// duration of [`run`](super::run)'s own step loop.
pub(super) struct ControlWatcherHandle {
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for ControlWatcherHandle {
    fn drop(&mut self) {
        self.handle.abort();
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

    use super::super::ExecSingleStepExecutor;
    use super::*;
    use crate::background::{RunId, RunMode, RunStatus};
    use crate::spawn::depth::DepthEnvelope;
    use std::collections::BTreeMap;
    use std::path::Path;

    // ---------------------------------------------------------------------------------------
    // G90 — the runner's steer router, driven as the FULL LIFECYCLE rather than one rendered
    // block: parent writes into the run queue -> router decides per child -> per-child inbox +
    // status counters + `events.jsonl` decision record.
    // ---------------------------------------------------------------------------------------

    /// A `Running` run with `agents.len()` steps, the ones named in `running` marked `Running` and
    /// the rest left `Pending`.
    fn steer_fixture(dir: &Path, agents: &[&str], running: &[usize]) -> (RunPaths, SharedStatus) {
        let paths = RunPaths::for_run(dir, dir, &RunId::from_token("steerroute01".to_string()));
        std::fs::create_dir_all(&paths.run_dir).expect("mkdir run dir");
        let mut status = RunStatus::queued(
            paths
                .run_dir
                .file_name()
                .map(|n| RunId::from_token(n.to_string_lossy().into_owned()))
                .expect("run id"),
            RunMode::Parallel,
            Some(std::process::id()),
        );
        status.state = RunState::Running;
        status.steps = agents
            .iter()
            .enumerate()
            .map(|(i, agent)| {
                let mut step = crate::background::StepStatus::pending(*agent);
                if running.contains(&i) {
                    step.status = StepState::Running;
                }
                step
            })
            .collect();
        (paths, Arc::new(std::sync::Mutex::new(status)))
    }

    /// SUBA-087 — pi `stopChildStep` (`subagent-runner.ts:3015-3031` @v0.64.0) driven as the full
    /// lifecycle: parent writes a targeted request → router gates and records → status stamps +
    /// `events.jsonl` + the live child's handle fires; a `pending` target is queued and its handle
    /// fires at registration; a terminal target is `stop_failed`. The run-wide `stopped` flag is
    /// never involved.
    ///
    /// Fails at the parent commit by construction (`StopRequest` had no `target_index`).
    #[tokio::test]
    async fn child_scoped_stop_requests_are_routed_to_one_step_and_never_the_whole_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b", "c"], &[1]);
        {
            let mut status = lock_status(&shared);
            status.steps[0].status = StepState::Complete;
        }
        let registry = ChildStopRegistry::new();
        let live = cyrup_core::CancelToken::new();
        registry.register_active(1, live.clone());

        // Three parent-side writes: a terminal target, the running target, and a pending target.
        for index in [0usize, 1, 2] {
            control::deliver_child_stop_request(&paths.run_dir, "stop-action", index, None)
                .await
                .expect("parent write");
        }

        let mut events = RunEventLog::create(&paths.events).await.ok();
        route_child_stop_requests(&paths, &shared, &mut events, &registry).await;
        drop(events);

        // The running child's handle fired; the run itself was not asked to stop.
        assert!(live.is_cancelled(), "the targeted live child is torn down");
        assert!(
            control::check_stop_inbox_now(&paths)
                .await
                .expect("probe")
                .is_none(),
            "no whole-run request exists or was left behind"
        );
        assert!(
            !control::has_pending_stop_request(&paths.run_dir).await,
            "every child-scoped request was consumed"
        );

        // Status: step 1 and step 2 carry the request stamps; step 0 is untouched.
        {
            let status = lock_status(&shared);
            assert!(!status.steps[0].stop_requested);
            assert!(status.steps[1].stop_requested);
            assert!(status.steps[1].stop_requested_at.is_some());
            assert_eq!(
                status.steps[1].status,
                StepState::Running,
                "not yet settled"
            );
            assert!(status.steps[2].stop_requested);
            assert_eq!(status.state, RunState::Running, "the run stays alive");
        }
        // Registry: 1 and 2 recorded with their positional identities; 0 refused.
        assert!(!registry.is_requested(0));
        assert_eq!(
            registry.recorded(1).map(|r| r.child_id),
            Some("step:1".to_string())
        );
        assert!(registry.is_requested(2));
        // A handle registered for the QUEUED step fires immediately (pi `registerStepStop`).
        let late = cyrup_core::CancelToken::new();
        registry.register_active(2, late.clone());
        assert!(late.is_cancelled());

        // events.jsonl carries pi's event types with the step index and child id.
        let text = tokio::fs::read_to_string(&paths.events)
            .await
            .expect("events.jsonl");
        let events: Vec<serde_json::Value> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let of = |kind: &str| -> Vec<&serde_json::Value> {
            events.iter().filter(|e| e["type"] == kind).collect()
        };
        let failed = of("subagent.step.stop_failed");
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0]["stepIndex"], 0);
        assert_eq!(failed[0]["childId"], "step:0");
        assert_eq!(failed[0]["message"], "Child is not pending or running.");
        let requested = of("subagent.step.stop_requested");
        assert_eq!(
            requested
                .iter()
                .map(|e| (
                    e["stepIndex"].as_u64(),
                    e["childId"].as_str(),
                    e["agent"].as_str()
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some(1), Some("step:1"), Some("b")),
                (Some(2), Some("step:2"), Some("c"))
            ]
        );
        let stopping = of("subagent.child-status");
        assert_eq!(stopping.len(), 2);
        assert_eq!(stopping[0]["status"], "stopping");
        assert_eq!(stopping[0]["version"], 1);
        assert_eq!(stopping[0]["source"], "async");
        assert_eq!(stopping[0]["reason"], "user");
        let queued = of("subagent.step.stop_queued");
        assert_eq!(queued.len(), 1, "only the PENDING target is queued");
        assert_eq!(queued[0]["stepIndex"], 2);
        assert!(of("subagent.run.stopped").is_empty());
    }

    /// G90, the runner's two halves must address the SAME directory.
    ///
    /// `route_steer_requests` writes an accepted request into
    /// `control::step_steer_inbox_dir(run_dir, index)`; `run_single` hands the child
    /// `steer_inbox_for(index)`. If those two ever diverge — a run-level queue dir on one side, a
    /// per-child target dir on the other; a step index on one side and a flat index on the other —
    /// each half stays individually correct and the feature is silently dead again, with no test
    /// failing. This asserts the agreement directly, at the real write site.
    #[tokio::test]
    async fn the_inbox_the_runner_writes_is_the_inbox_the_child_is_handed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b"], &[1]);
        control::request_async_steer(
            &paths.run_dir,
            "look at step two",
            None,
            Some("steer-action"),
        )
        .await
        .expect("parent write");

        let mut events = RunEventLog::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;

        // The executor built for THIS run, exactly as `run` builds it.
        let executor = ExecSingleStepExecutor {
            writer_ledgers: None,
            lease_writer: None,
            spawn_command: None,
            child_env: std::collections::HashMap::new(),
            host_available_builtins: None,
            // SUBA-021: unbudgeted on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            permission_rules: None,
            depth: DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            interrupted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            interrupt_cancel: cyrup_core::CancelToken::new(),
            child_stops: None,
            telemetry: None,
            share: None,
            artifacts_dir: None,
            artifact_config: crate::artifacts::ArtifactConfig::default(),
            transcript_source: crate::exec::child_transcript::TranscriptSource::Async,
            resolved_agents: Arc::new(BTreeMap::new()),
            orchestrator_intercom_target: None,
            run_id: None,
            inherited_session_model: None,
            inherited_session_thinking: None,
            model_scope: None,
            control: None,
            include_progress: None,
            run_dir: Some(paths.run_dir.clone()),
        };

        let handed = executor
            .steer_inbox_for(1)
            .expect("a background executor must hand its children an inbox");
        let written: Vec<_> = std::fs::read_dir(&handed)
            .unwrap_or_else(|e| {
                panic!(
                    "the runner must have written into the very directory the child is handed \
                     ({}): {e}",
                    handed.display()
                )
            })
            .filter_map(Result::ok)
            .collect();
        assert_eq!(
            written.len(),
            1,
            "the routed request must be sitting in the child's own inbox at {}",
            handed.display()
        );

        // A FOREGROUND executor has no run dir and therefore hands no inbox — the same condition
        // that makes `control_steer` refuse a foreground run outright.
        let foreground = ExecSingleStepExecutor::foreground(
            DepthEnvelope {
                current_depth: 0,
                max_depth: 5,
            },
            Arc::new(BTreeMap::new()),
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(foreground.steer_inbox_for(0).is_none());
    }

    #[tokio::test]
    async fn steer_routing_fans_an_untargeted_request_to_every_running_child() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b", "c"], &[0, 2]);
        control::request_async_steer(
            &paths.run_dir,
            "tighten the scope",
            None,
            Some("steer-action"),
        )
        .await
        .expect("parent write");

        let mut events = RunEventLog::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;

        // Every RUNNING child got its own copy; the pending one did not.
        for index in [0usize, 2] {
            let inbox = control::step_steer_inbox_dir(&paths.run_dir, index);
            let files: Vec<_> = std::fs::read_dir(&inbox)
                .unwrap_or_else(|e| panic!("child {index} inbox must exist: {e}"))
                .filter_map(Result::ok)
                .collect();
            assert_eq!(
                files.len(),
                1,
                "child {index} must receive exactly one steer"
            );
            let raw = std::fs::read_to_string(files[0].path()).expect("read");
            let request: control::SteerRequest = serde_json::from_str(&raw).expect("parse");
            assert_eq!(
                request.target_index,
                Some(index),
                "the copy must be PINNED to its child"
            );
            assert_eq!(request.message, "tighten the scope");
        }
        assert!(
            !control::step_steer_inbox_dir(&paths.run_dir, 1).exists(),
            "a pending child must NOT be handed an untargeted steer"
        );

        // The run-level queue was drained exactly once (delete-before-deliver).
        assert!(
            control::consume_steer_requests(&paths.run_dir)
                .await
                .is_empty(),
            "the run queue must be empty after routing"
        );
        assert!(pending.is_empty(), "nothing was held");

        // Counters landed on the accepted steps AND the run, and were persisted.
        let status = lock_status(&shared).clone();
        assert_eq!(status.steps[0].telemetry.steer_count, Some(1));
        assert_eq!(status.steps[1].telemetry.steer_count, None);
        assert_eq!(status.steps[2].telemetry.steer_count, Some(1));
        assert_eq!(status.telemetry.steer_count, Some(2));
        assert!(status.telemetry.last_steer_at.is_some());
        let persisted: RunStatus =
            serde_json::from_slice(&std::fs::read(&paths.status).expect("status.json written"))
                .expect("parse status.json");
        assert_eq!(persisted.telemetry.steer_count, Some(2));

        // And the decision is on the event log, with pi's payload keys.
        drop(events);
        let log = std::fs::read_to_string(&paths.events).expect("events.jsonl");
        let line = log
            .lines()
            .find(|l| l.contains("subagent.steer.requested"))
            .expect("the router must log its decision");
        let event: serde_json::Value = serde_json::from_str(line).expect("parse event");
        assert_eq!(event["acceptedIndexes"], serde_json::json!([0, 2]));
        assert_eq!(event["source"], serde_json::json!("steer-action"));
        assert_eq!(event["message"], serde_json::json!("tighten the scope"));
        assert!(
            event.get("rejected").is_none(),
            "nothing was rejected: {event}"
        );
    }

    #[tokio::test]
    async fn a_steer_aimed_at_a_pending_child_is_held_then_delivered_once_it_starts() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b"], &[0]);
        control::request_async_steer(&paths.run_dir, "wait for me", Some(1), None)
            .await
            .expect("parent write");

        let mut events = RunEventLog::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        assert_eq!(
            pending.len(),
            1,
            "a pending target must be HELD, never dropped"
        );
        assert!(
            !control::step_steer_inbox_dir(&paths.run_dir, 1).exists(),
            "nothing may be delivered while the child is pending"
        );

        // The child starts; the next tick delivers the held request without the parent resending.
        lock_status(&shared).steps[1].status = StepState::Running;
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        assert!(pending.is_empty(), "the held request must be released");
        let inbox = control::step_steer_inbox_dir(&paths.run_dir, 1);
        assert_eq!(
            std::fs::read_dir(&inbox).expect("inbox").count(),
            1,
            "the held request must land on the child that just started"
        );
        assert_eq!(lock_status(&shared).steps[1].telemetry.steer_count, Some(1));
    }

    #[tokio::test]
    async fn a_steer_aimed_at_a_finished_child_is_rejected_with_that_childs_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a", "b"], &[0]);
        lock_status(&shared).steps[1].status = StepState::Complete;
        control::request_async_steer(&paths.run_dir, "too late", Some(1), None)
            .await
            .expect("parent write");

        let mut events = RunEventLog::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        assert!(
            pending.is_empty(),
            "a terminal child is a rejection, not a hold"
        );
        assert_eq!(lock_status(&shared).telemetry.steer_count, None);

        drop(events);
        let log = std::fs::read_to_string(&paths.events).expect("events.jsonl");
        let line = log
            .lines()
            .find(|l| l.contains("subagent.steer.requested"))
            .expect("a rejection is still logged");
        let event: serde_json::Value = serde_json::from_str(line).expect("parse event");
        assert_eq!(event["acceptedIndexes"], serde_json::json!([]));
        assert_eq!(event["rejected"][0]["index"], serde_json::json!(1));
        assert_eq!(
            event["rejected"][0]["reason"],
            serde_json::json!("child is complete")
        );
    }

    #[tokio::test]
    async fn steer_requests_are_routed_in_timestamp_order_not_readdir_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (paths, shared) = steer_fixture(dir.path(), &["a"], &[0]);
        // Written newest-first on purpose: the queue file names are zero-padded by `ts`, and the
        // consumer re-sorts, so delivery order must still be oldest-first.
        for (ts, message) in [(2_000_i64, "second"), (1_000, "first")] {
            let request = control::SteerRequest {
                kind: "steer".to_string(),
                id: format!("id-{ts}"),
                ts,
                message: message.to_string(),
                mode: None,
                target_index: None,
                source: None,
            };
            control::write_steer_request_to_dir(
                &control::steer_requests_dir(&paths.run_dir),
                &request,
            )
            .await
            .expect("write");
        }

        let mut events = RunEventLog::create(&paths.events).await.ok();
        let mut pending = Vec::new();
        route_steer_requests(&paths, &shared, &mut events, &mut pending).await;
        drop(events);

        let log = std::fs::read_to_string(&paths.events).expect("events.jsonl");
        let messages: Vec<String> = log
            .lines()
            .filter(|l| l.contains("subagent.steer.requested"))
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter_map(|e| e["message"].as_str().map(str::to_string))
            .collect();
        assert_eq!(messages, vec!["first".to_string(), "second".to_string()]);
        assert_eq!(lock_status(&shared).steps[0].telemetry.steer_count, Some(2));
    }
}
