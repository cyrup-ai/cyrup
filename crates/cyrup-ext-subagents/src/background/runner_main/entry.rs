//! Hop-2 runner entry point ([`run`]/[`run_with`]): startup sequence (initial `RunStatus` write
//! R-SA-075, control-inbox + watcher wiring R-SA-081/082) and the no-bypass tail that funnels
//! every exit into [`super::finish::finish_run`] (R-SA-077). Split out of
//! `background/runner_main.rs`; ports pi `runs/background/subagent-runner.ts`.

use super::config::{RunnerConfig, effective_run_paths, load_runner_config};
use super::control_watcher::{
    init_control_flags, install_ignored_sigusr2_handler, spawn_control_watcher,
};
use super::events::open_run_events;
use super::finish::{WorkflowResultFields, finish_run, settle_loop_outcome};
use super::status::{SharedStatus, TelemetryMsg, lock_status, spawn_telemetry_task};
use super::turn_loop::run_inner;
use crate::background::atomic::write_atomic_json;
use crate::background::flat_index::{
    pending_step_statuses_for, resolve_async_step_transcript_path,
};
use crate::background::{RunId, RunPaths, RunState, RunStatus};
use crate::error::SubagentError;
use std::path::Path;
use std::sync::Arc;

// =================================================================================================
// run — the hop-2 main loop entry point
// =================================================================================================

/// Run the hop-2 detached-runner main loop against the one-shot config at `config_path`
/// (R-SA-073..077).
///
/// `run_paths` locates every well-known file this run writes to
/// ([`crate::background::RunPaths::for_run`], resolved by the caller — `crates/cyrup/src/
/// subagent_runner_cmd.rs` — from the config path's own parent directory structure, per
/// func-SA §4.5's fixed `<AsyncRoot>/<run_id>/` layout, and passed in explicitly here rather than
/// re-derived, since this module has no opinion on where `AsyncRoot`/`ResultsDir` themselves live
/// — that is `registration::SubagentExtensionConfig`'s concern).
///
/// This function's own top-level control flow is exactly the shape documented in this module's
/// header comment: read+delete config -> write initial `Running` status -> spawn the control-inbox
/// watcher -> the step loop -> compute terminal state -> [`finish_run`] (status THEN result, on
/// every exit path) -> return. Every fallible step inside is funneled through [`run_inner`] so a
/// SINGLE tail call to [`finish_run`] is the only place either file gets written to its terminal
/// form, regardless of which branch produced that terminal state (steps-exhausted happy path,
/// mid-loop interrupt, or an internal error surfaced by `run_inner` itself).
///
/// # Errors
///
/// This function itself is effectively infallible from the CALLER's point of view — every
/// internal failure is captured into a terminal `Failed` [`RunStatus`]/[`ResultFile`](crate::background::ResultFile) pair rather
/// than propagated as a `Result::Err`, since there is no one left to hand an `Err` to once this
/// process is the detached runner (R-SA-078: "the orchestrator MUST NOT assume a live IPC channel
/// to the runner" — there is no return channel for a `Result` to travel back through). The
/// `Result` return type exists purely so `crates/cyrup/src/subagent_runner_cmd.rs` can log a
/// diagnostic and choose its own process exit code; it carries NO information this function's own
/// on-disk writes have not already durably recorded.
pub async fn run(config_path: &Path, run_paths: &RunPaths) -> Result<(), SubagentError> {
    run_with(config_path, run_paths, RunnerOverrides::default()).await
}

/// What a caller driving the runner IN-PROCESS may decide for it.
///
/// A struct rather than a parameter list because both fields answer the same question — "which
/// ambient input does this run use instead of the process's" — and the set is open: the previous
/// shape would have grown a third positional argument the moment the cascade needed its root.
///
/// Every field `None` is byte-for-byte the real detached runner's behaviour.
#[derive(Debug, Default, Clone)]
pub struct RunnerOverrides {
    /// The binary each dispatched step execs. `None` resolves from the inherited environment.
    pub spawn_command: Option<crate::spawn::SpawnCommand>,
    /// The filesystem roots this run resolves against, including the one the cascade reads
    /// mid-run. `None` resolves from the process environment.
    pub roots: Option<crate::paths::Roots>,
    /// Extra entries for each dispatched step's CHILD environment, layered exactly as
    /// [`crate::exec::RunOptions::child_env`] is.
    ///
    /// Completes the set: without it a caller driving the runner in-process could name the binary
    /// and the roots but not reach the child's own environment, which is where a fixture's
    /// out-of-band capture path lives.
    pub child_env: std::collections::HashMap<String, String>,
}

/// [`run`] with the step-dispatch binary supplied in-process.
///
/// The REAL detached runner is its own process reached through a `RunnerConfig` on disk, so it has
/// no in-process caller to hand anything down — that is why [`run`] passes `None` and why
/// `RunnerConfig` deliberately carries no such field (a config file able to redirect which binary
/// executes is a hazard, the same reason `SubagentExtensionConfig::spawn_command` is
/// `#[serde(skip)]`).
///
/// A caller driving this function IN-PROCESS, however, *is* the runner, and for it the injection is
/// both possible and correct: it substitutes the scripted fixture binary without moving
/// `CYRUP_SUBAGENT_BINARY` on a process every other concurrent test shares.
pub async fn run_with(
    config_path: &Path,
    run_paths: &RunPaths,
    overrides: RunnerOverrides,
) -> Result<(), SubagentError> {
    let RunnerOverrides {
        spawn_command,
        roots,
        child_env,
    } = overrides;
    // Resolved ONCE here, then carried: the cascade's own read is mid-run, and re-deriving it there
    // is what used to force a caller to move `CYRUP_SUBAGENTS_TEMP_ROOT` on the whole process.
    let roots = roots.unwrap_or_else(crate::paths::Roots::from_env);
    let Some(config) = load_runner_config(config_path, run_paths).await else {
        return Ok(());
    };

    let effective_paths = effective_run_paths(&config);
    let run_paths: &RunPaths = effective_paths.as_ref().unwrap_or(run_paths);

    ensure_run_directories(run_paths).await;

    // pi `subagent-runner.ts:5241-5243` — the REVIVAL lease, acquired by the RUNNER, before any
    // child session can be touched, on the revival path only. Its position is upstream's: after
    // the config is in hand, before the run announces itself.
    let mut lease = match acquire_revival_lease(&config, &roots).await {
        Ok(lease) => lease,
        Err(error) => {
            refuse_run(&config, run_paths, &error).await;
            return Ok(());
        }
    };

    let Some(status) = publish_initial_status(&config, run_paths).await else {
        // A lease taken a moment ago and a run that never started: release it here rather than
        // leaving a live-looking claim behind for the next revival to have to prove stale.
        release_lease(&mut lease, run_paths).await;
        return Ok(());
    };

    // Install a SIGUSR2 handler BEFORE anything else that could race an interrupt delivery
    // (R-SA-081's wake-up signal, sent by `control::deliver_wakeup_signal`): on both Linux and
    // macOS, SIGUSR2's default disposition is process TERMINATION. Without an installed handler,
    // the very act of a caller trying to softly interrupt this run would instead kill the runner
    // outright — the opposite of R-SA-084's "interrupt is soft, not fatal" guarantee, and the
    // interrupt would never even reach `run_inner`'s own cooperative `interrupted` check. The
    // signal's payload itself is not consulted for anything: `control::watch_control_inbox`'s
    // filesystem-notification mechanism (installed by `spawn_control_watcher` immediately below)
    // is the actual, authoritative "an interrupt/append request landed" signal per DI-SA-9
    // (file-based control, never live IPC) — SIGUSR2 exists purely to nudge that watcher/poll
    // loop awake sooner than its next scheduled tick, so this handle's only job is to keep
    // existing for this function's whole lifetime (held via `_sigusr2_guard`) so the OS routes
    // the signal to a registered handler instead of applying its default terminate action; a
    // received signal is otherwise fully drained/ignored.
    #[cfg(unix)]
    let _sigusr2_guard = install_ignored_sigusr2_handler();

    let Some(status) = ensure_control_inbox_dir(&config, run_paths, status).await else {
        release_lease(&mut lease, run_paths).await;
        return Ok(());
    };

    let (control_flags, interrupt_cancel) = init_control_flags(run_paths).await;

    let mut events = open_run_events(&config, run_paths).await;

    // The run's overall start (for `durationMs` on the terminal run event, pi's
    // `runEndedAt - overallStartTime`), captured before `status` is moved into the shared handle.
    let overall_started_at = status.started_at;

    // Move the initial `Running` status into the shared handle BOTH the step loop and the live-
    // telemetry pump mutate (pi's single `statusPayload`, folded from the per-child event handler
    // AND the 1s `activityTimer`, `subagent-runner.ts:1962`).
    let shared_status: SharedStatus = Arc::new(std::sync::Mutex::new(status));

    // R-SA-082's watcher is installed HERE rather than immediately after the two synchronous
    // startup checks above, because G90's steer routing needs the shared status handle (it accepts
    // a steer only against a currently-`Running` step and records the acceptance on that step). The
    // synchronous startup checks stay where they were — they are what closes the pre-watcher race
    // window, and nothing between them and this line can deliver a control request.
    let _watcher_task = spawn_control_watcher(
        run_paths.clone(),
        control_flags.clone(),
        interrupt_cancel.clone(),
        Arc::clone(&shared_status),
    );

    // The live-telemetry channel: each dispatched step's `RunOptions::live_events` sink forwards
    // raw child NDJSON lines here (tagged with the step's flat index); the telemetry task folds
    // each into the addressed step's `StepTelemetry` + the top-level roll-ups and writes
    // status.json on both a per-event AND a 1s cadence.
    // The run's per-step writer-process ledgers. Created here, beside the telemetry channel and
    // for the same structural reason: both are side channels every dispatched step writes into and
    // this function reads back at the close. This one becomes
    // `process-terminal-candidate.json`'s `writers`/`expectedWriters` maps — the REAL ones, over
    // cyrup's real OS children, where upstream writes all-empty maps because its children run
    // in-process (`subagent-runner.ts:5168-5190`, its own comment at `:5169`).
    let writer_ledgers: super::executor::WriterProcessLedgers =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::BTreeMap::new()));
    let (telemetry_tx, telemetry_rx) = tokio::sync::mpsc::unbounded_channel::<TelemetryMsg>();
    let telemetry_task =
        spawn_telemetry_task(run_paths.clone(), Arc::clone(&shared_status), telemetry_rx);
    // The lease's writer channel, beside the telemetry one and for the same structural reason: an
    // observation made inside a synchronous sink callback has to reach an `async` writer. The
    // HANDLE moves into this task, which is what keeps one owner for the whole claim; it comes
    // back out when every sender has been dropped, which is when `run_inner` has returned.
    let (lease_tx, lease_task) = match lease.take() {
        Some(handle) => {
            let (tx, task) = spawn_lease_writer_task(handle);
            (Some(tx), Some(task))
        }
        // No lease, no channel: a dispatch that reported into a channel nobody drains would be
        // recording a writer state no lease exists to carry, and the executor's own sink gate
        // reads this `None` to skip installing the reporter at all.
        None => (None, None),
    };

    // The step loop itself, all failure modes funneled to a single Result the tail below always
    // routes through `finish_run`.
    let loop_outcome = run_inner(
        &roots,
        &child_env,
        spawn_command.as_ref(),
        &config,
        run_paths,
        &shared_status,
        &control_flags,
        &interrupt_cancel,
        telemetry_tx,
        std::sync::Arc::clone(&writer_ledgers),
        lease_tx,
        &mut events,
    )
    .await;

    // `run_inner` has returned, so its executor (holding the last live-telemetry sender) is dropped
    // and the telemetry task observes all-senders-dropped and finishes — await it so no late
    // telemetry status write races the terminal record `finish_run` writes.
    let _ = telemetry_task.await;
    // Every sender is gone with `run_inner`'s executor, so the lease task has drained and is
    // handing the handle back. From here the runner owns it again, for the release below.
    let mut lease = match lease_task {
        Some(task) => task.await.ok(),
        None => None,
    };

    let duration_ms = (crate::time::now_epoch_millis() - overall_started_at).max(0);
    let (terminal_state, results, final_error) =
        settle_loop_outcome(loop_outcome, &config, &mut events, duration_ms).await;

    // Recover the final live status (its accumulated per-step telemetry + workflow-graph snapshot)
    // so the terminal `status.json` `finish_run` writes preserves everything the pump accumulated.
    let final_status = lock_status(&shared_status).clone();
    // Captured before `final_status` is moved into `finish_run`: the process-terminal candidate
    // declares one entry per FLAT step, including steps that never dispatched a child, so a run
    // whose every step was skipped still presents a non-empty `expectedWriters` map and is
    // distinguishable from a runner that died before declaring anything at all (which is what
    // `initialize_process_terminal`'s empty candidate means).
    let flat_step_count = final_status.steps.len();

    finish_run(
        run_paths,
        final_status,
        terminal_state,
        results,
        config.cwd.clone(),
        config.session_file.clone(),
        final_error.unwrap_or_default(),
        // This is the ordinary chain/parallel/single runner tail — never a workflow terminal
        // write (WORKFLOW_3 §3c: a future async workflow arm is the first caller to supply a
        // non-default value here, via `apply_workflow_settlement_plan`).
        WorkflowResultFields::default(),
    )
    .await;

    // pi `subagent-runner.ts:5168-5190` — the candidate FIRST, carrying the lease token of the
    // handle this runner still holds. It has to be first for two reasons, and each is load-bearing:
    // the stamp below matches on this record's token (`process-terminal.ts:136`), and a candidate
    // written after the release could not distinguish "released, unacknowledged" from "never
    // stamped".
    write_own_process_terminal_candidate(
        &config,
        run_paths,
        &writer_ledgers,
        flat_step_count,
        lease.as_ref().map(|handle| handle.token().clone()),
    )
    .await;

    // pi `subagent-runner.ts:5278-5293` — release the lease, then STAMP the acknowledgement onto
    // the candidate, and only then finalize. The order is the whole reason
    // `canonical-session-release-unverified` (`process-terminal.ts:275-276`) is a distinguishable
    // arm: `sessionProjection` (`:150-159`) requires both that the lease is FREE and that a run
    // which held a token recorded an ACKNOWLEDGED release, and neither can be true if the release
    // happens after the proof is written. Getting this backwards makes every revived run's proof
    // `unknown`.
    release_and_record_lease(&mut lease, run_paths).await;

    // pi's runner writes its process-terminal candidate at `subagent-runner.ts:5168-5190` and the
    // PARENT finalizes on `proc.once("close")` (`async-execution.ts:778-790`). cyrup's orchestrator
    // cannot do the second half: `spawn_detached_runner_with_command` spawns a genuinely detached
    // process in its own group with stdio to files and drops the `Child` handle without ever
    // awaiting it (`crates/cyrup/src/subagent_runner_cmd.rs:1-7`), so there is no `close` event and
    // no parent left by the time this process ends. The runner therefore observes its OWN close —
    // here, AFTER `finish_run` has written the terminal `status.json` and `ResultFile` (R-SA-077's
    // ordering, which must not grow a third write between them) and before this function returns.
    finalize_own_process_terminal(&config, run_paths, &roots, &mut events).await;

    Ok(())
}

/// pi `acquireSessionLease(config.revivalLease)` (`subagent-runner.ts:5241-5242`) — the runner's
/// own claim on the session file it is about to rewrite.
///
/// `Ok(None)` is every ordinary launch: no request, no lease, and nothing pretends otherwise.
///
/// # Errors
///
/// Whatever [`acquire_session_lease`](crate::background::session_lease::acquire_session_lease)
/// refuses with — most importantly the two `SessionLeaseConflictError` sentences, which are what
/// an operator or a delegating agent actually reads when a second revival of one session file is
/// turned away.
async fn acquire_revival_lease(
    config: &RunnerConfig,
    roots: &crate::paths::Roots,
) -> Result<
    Option<crate::background::session_lease::SessionLeaseHandle>,
    crate::background::session_lease::SessionLeaseError,
> {
    let Some(request) = config.revival_lease.as_ref() else {
        return Ok(None);
    };
    crate::background::session_lease::acquire_session_lease(
        request,
        &crate::background::session_leases_root_in(roots),
        &crate::background::session_lease::SessionLeaseOptions::default(),
    )
    .await
    .map(Some)
}

/// A run that never started because its session file is already leased — pi
/// `persistPreProceedStartupFailure` (`async-execution.ts:617-652`), which is upstream's record
/// for exactly this class of failure: the runner never got as far as a child.
///
/// The record it writes is upstream's, field for field: `state: "failed"`, the message as
/// `error`, and a `processTerminal` of `state: "not-started"` carrying this run's id and the
/// launch's minted instance (`:636-640`). That triple is not decoration — it is the input to the
/// capacity pool's early-failure carve-out (`active-async-capacity.ts:222-226`,
/// [`runner_release_verdict`](crate::background::active_async_capacity::runner_release_verdict)),
/// which releases the slot immediately instead of making a run that can never produce a proof sit
/// out the whole abandoned timeout.
///
/// The sidecar is deliberately left at `pending`: nothing finalizes here, because a run that never
/// started has no close to observe, and a fabricated `observed` would be the kind of proof that
/// lies. `status.processTerminal` says `not-started`; the sidecar says `pending`; both are true.
///
/// Routed through [`finish_run`], never a bare `return`: R-SA-077's "status.json THEN ResultFile,
/// on every exit path" is enforced by construction here (`runner_main/mod.rs`'s own module doc
/// states it), and a refusal that skipped both would leave the orchestrator's `status` verb
/// reading a run that never writes anything at all — which is precisely the "cannot tell a crash
/// from a slow start" failure this whole batch exists to remove.
async fn refuse_run(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    error: &crate::background::session_lease::SessionLeaseError,
) {
    tracing::warn!(
        run_id = %config.run_id,
        %error,
        "refusing to start: the canonical session lease for this revival could not be acquired"
    );
    let mut status =
        RunStatus::queued(config.run_id.clone(), config.mode, Some(std::process::id()));
    // pi `:634` — the run-level message, which is what an operator and a delegating agent read.
    status.error = Some(error.to_string());
    // pi `:636-640`.
    status.process_terminal = config.runner_process_instance_id.as_ref().map(|instance| {
        crate::background::process_terminal::ProcessTerminal::NotStarted {
            base: crate::background::process_terminal::ProcessTerminalBase::new(
                config.run_id.clone(),
                instance.clone(),
            ),
        }
    });
    finish_run(
        run_paths,
        status,
        RunState::Failed,
        Vec::new(),
        config.cwd.clone(),
        config.session_file.clone(),
        error.to_string(),
        WorkflowResultFields::default(),
    )
    .await;
}

/// pi `subagent-runner.ts:5280-5292` — the `finally` that releases the lease and STAMPS whether
/// the release was acknowledged onto the candidate.
///
/// The stamp is the only place `revivalLeaseReleaseAcknowledged` is ever written. It matches on
/// the token the candidate already carries (`process-terminal.ts:136`), so on the ordinary tail —
/// where [`write_own_process_terminal_candidate`] has just written that token — it WRITES, and
/// `finalizeProcessTerminal`'s `canonical-session-release-unverified` rung (`:275-276`) clears.
/// On the two pre-loop refusal paths ([`release_lease`]) no candidate carries a token yet and the
/// guard correctly declines: there is no run to prove anything about, and a run that is refused
/// its lease never held one to release.
///
/// An unacknowledged release is NOT an error path — it leaves the candidate saying "a token was
/// held and its release could not be confirmed", which is a different statement from "no token was
/// held", and the ladder keeps the two apart.
async fn release_and_record_lease(
    lease: &mut Option<crate::background::session_lease::SessionLeaseHandle>,
    run_paths: &RunPaths,
) {
    let Some(handle) = lease.as_mut() else {
        return;
    };
    let token = handle.token().clone();
    let acknowledged = handle.release().await;
    if !acknowledged {
        tracing::warn!(
            lease_dir = %handle.lease_dir().display(),
            "the session revival lease was not observably released; this run's close will be reported as unproven"
        );
    }
    // pi `markProcessTerminalCandidateLeaseRelease(config.asyncDir, lease.owner.token,
    // acknowledged)` (`:5288`).
    crate::background::process_terminal::mark_process_terminal_candidate_lease_release(
        &crate::background::RunDir::for_existing(&run_paths.run_dir),
        &token,
        acknowledged,
    )
    .await;
}

/// [`release_and_record_lease`] for the two pre-loop refusal paths, where the run never reached
/// its candidate write — only a claim that must not be left behind.
async fn release_lease(
    lease: &mut Option<crate::background::session_lease::SessionLeaseHandle>,
    run_paths: &RunPaths,
) {
    release_and_record_lease(lease, run_paths).await;
}

/// Drain [`crate::background::session_lease::WriterUpdate`]s into the lease handle, and hand the
/// handle back once every sender is gone.
///
/// Modelled on [`spawn_telemetry_task`]: one task owns the resource, the dispatch sites own
/// senders, and the resource comes back at the join. One owner is what keeps the four-attempt
/// claim, every writer update and the release on a single handle — `updateWriter` re-reads and
/// re-checks the token on every call (`session-lease.ts:242-245`), which two concurrent holders
/// would race.
fn spawn_lease_writer_task(
    mut lease: crate::background::session_lease::SessionLeaseHandle,
) -> (
    super::executor::LeaseWriterSender,
    tokio::task::JoinHandle<crate::background::session_lease::SessionLeaseHandle>,
) {
    let (tx, mut rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::background::session_lease::WriterUpdate>();
    let task = tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            // A refused update means the lease was broken as stale and re-taken out from under
            // this running process — `updateWriter`'s own throw (`session-lease.ts:244`). There is
            // nothing to do about it here and nothing the step loop could do either; the run's
            // close will then report unproven, which is the true statement.
            if let Err(error) = lease.update_writer(update).await {
                tracing::warn!(%error, "session revival lease writer state could not be updated");
            }
        }
        lease
    });
    (tx, task)
}

/// pi `subagent-runner.ts:5168-5190` — write this run's process-terminal candidate.
///
/// Called BEFORE the lease release, exactly as upstream's is: upstream's candidate write sits in
/// the body of `runConfiguredSubagent` and its release sits in that function's `finally`
/// (`:5280-5292`), so by the time `markProcessTerminalCandidateLeaseRelease` (`:5288`) runs, the
/// candidate on disk already carries `revivalLeaseToken` and the stamp MATCHES. Inverting the two
/// makes that stamp a no-op — it reads the candidate and returns on the token guard
/// (`process-terminal.ts:136`) — and the acknowledgement then only ever reaches disk if some later
/// writer happens to carry it. Keep this call first.
///
/// A launch that carried no [`RunnerConfig::runner_process_instance_id`] (a `runner-config.json`
/// written by a build older than the field) writes NOTHING: there is no identity any reader holds,
/// and a proof keyed on an invented one would be worse than none.
async fn write_own_process_terminal_candidate(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    writer_ledgers: &super::executor::WriterProcessLedgers,
    flat_step_count: usize,
    revival_lease_token: Option<crate::background::session_lease::LeaseToken>,
) {
    let Some(instance) = config.runner_process_instance_id.clone() else {
        return;
    };
    let run_dir = crate::background::RunDir::for_existing(&run_paths.run_dir);

    // The maps are cyrup's own: one entry per FLAT step index, `expectedWriters` counting the
    // children that were LAUNCHED and `writers` holding the closes that were OBSERVED. Steps that
    // never dispatched a child still get an explicit zero-count entry, because
    // `finalizeProcessTerminal`'s `:272` rung treats a declared index with a non-zero count and no
    // records as inconsistent while a declared ZERO is consistent with an empty list — which is how
    // a skipped step stays distinguishable from an unproven one.
    let ledgers = writer_ledgers
        .lock()
        .map(|ledgers| ledgers.clone())
        .unwrap_or_default();
    let mut writers = std::collections::BTreeMap::new();
    let mut expected_writers = std::collections::BTreeMap::new();
    for flat_index in 0..flat_step_count {
        let key = flat_index.to_string();
        let ledger = ledgers.get(&flat_index).cloned().unwrap_or_default();
        writers.insert(key.clone(), ledger.exits);
        expected_writers.insert(key, ledger.launched);
    }
    let candidate = crate::background::process_terminal::ProcessTerminalCandidate {
        version: crate::background::process_terminal::ProcessTerminalVersion,
        run_id: config.run_id.clone(),
        runner_process_instance_id: instance,
        writers,
        expected_writers: Some(expected_writers),
        // `[CYRUP-DELTA]` — pi sets this from `config.revivalLease?.sessionFile`
        // (`subagent-runner.ts:5182`), so its candidate names a session file ONLY on the revival
        // path. cyrup carries the run's own `session_file` whenever it has one, so that
        // `sessionProjection` (`process-terminal.ts:150-159`) can record that the file this run
        // wrote was free at the moment the close was observed for a FORKED run as well as a
        // revived one — a statement upstream's shape simply cannot make.
        //
        // It is NOT a strictly stronger statement of the same fact, and an earlier revision of
        // this comment said it was. The field is also the INPUT to the ladder's refusal rung
        // (`process-terminal.ts:273-274`), so broadening it broadens what can be REFUSED: a lease
        // directory left behind by an unrelated revival that was `SIGKILL`ed would, under
        // upstream's bare `state !== "free"` rung, make this healthy never-revived run's close
        // report `unknown / canonical-session-lease-active` — and every later fork of the same
        // transcript with it, until someone happened to revive it. That is paid for in the rung
        // itself: `finalize::lease_claim_is_live` runs upstream's own four staleness rungs
        // (`session-lease.ts:174-181`) over the record, so only a lease that still genuinely
        // claims the file refuses. See that function's `[CYRUP-DELTA]`.
        session_file: config.session_file.clone(),
        // pi `revivalLeaseToken: config.revivalLeaseToken` (`subagent-runner.ts:5184-5185`). The
        // ACKNOWLEDGEMENT is not written here — `mark_process_terminal_candidate_lease_release`
        // stamps it onto this record a moment later (`:5288`), which is why this write has to come
        // first. A run that held NO lease leaves both absent, and `finalizeProcessTerminal`'s
        // `canonical-session-release-unverified` rung (`:275-276`) then has nothing to check —
        // which is a different statement from "a token was held and its release was not
        // acknowledged", and the two must stay distinguishable.
        revival_lease_token,
        revival_lease_release_acknowledged: None,
    };
    if let Err(error) =
        crate::background::process_terminal::write_process_terminal_candidate(&run_dir, &candidate)
            .await
    {
        tracing::warn!(
            run_id = %config.run_id,
            %error,
            "failed to write the process-terminal candidate; the run's close will be reported as \
             unproven"
        );
    }
}

/// Judge the runner's own close — the tail of [`run_with`], split out so that function's shape
/// stays readable.
///
/// Runs AFTER [`write_own_process_terminal_candidate`] and after the lease release, over whatever
/// those two left on disk.
///
/// # The close observation, and why `signal` is always `None`
///
/// * `exit_code` is `Some(0)`, and that is a TRUE statement rather than a convenience:
///   `crates/cyrup/src/subagent_runner_cmd.rs:216-223` maps `run(..)` to `Ok(()) => 0`, and
///   [`run_with`] returns `Ok(())` on every path that reaches `finish_run` — its own doc says so
///   (*"every internal failure is captured into a terminal `Failed` … rather than propagated"*). A
///   run whose STEPS failed still exits 0; that failure is the run's outcome, carried by
///   [`RunState`](crate::background::RunState), not the runner's.
/// * `signal` is `None`, always. A runner that died of a signal never reached this line, so its
///   sidecar stays `pending` forever — which is exactly the crash semantics the artifact exists to
///   express, and the reason a crash is now distinguishable from a slow start. **Do not invent a
///   way to make this non-`None`**: there is none, and a fabricated signal name would be a
///   diagnostic that lies.
async fn finalize_own_process_terminal(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    roots: &crate::paths::Roots,
    events: &mut Option<crate::jsonl::BoundedJsonlWriter>,
) {
    let Some(instance) = config.runner_process_instance_id.clone() else {
        return;
    };
    let run_dir = crate::background::RunDir::for_existing(&run_paths.run_dir);

    let close = crate::background::process_terminal::RunnerCloseObservation {
        process_instance_id: instance,
        close_observed_at: crate::time::now_epoch_millis(),
        exit_code: Some(0),
        signal: None,
    };
    let lease_root = crate::background::session_leases_root_in(roots);
    let _ = crate::background::process_terminal::finalize_process_terminal(
        &run_dir,
        &config.run_id,
        &close,
        &lease_root,
        events,
    )
    .await;
}

/// ensureAccessibleDir-equivalent on the RUNNER side (C7's "create the dirs on both sides"):
/// guarantee the run dir (parent of every intermediate status/events write) and the results dir
/// (parent of the terminal ResultFile) both exist up front. `finish_run` re-ensures the results
/// dir as a final guard on every exit path, but creating them here keeps the happy-path
/// status/events writes from failing on a missing directory too.
pub(super) async fn ensure_run_directories(run_paths: &RunPaths) {
    let _ = crate::background::ensure_accessible_dir(&run_paths.run_dir).await;
    let _ = crate::background::ensure_accessible_dir(&run_paths.results_dir).await;
}

/// R-SA-075: initial status.json (state=Running, pid=own pid), written BEFORE any step work.
///
/// `None` means the status could not be published and the terminal `Failed` record has already
/// been written by [`finish_run`] — the caller returns without running a single step.
pub(super) async fn publish_initial_status(
    config: &RunnerConfig,
    run_paths: &RunPaths,
) -> Option<RunStatus> {
    let mut status =
        RunStatus::queued(config.run_id.clone(), config.mode, Some(std::process::id()));
    // pi `...(config.sessionId ? { sessionId: config.sessionId } : {})` (`subagent-runner.ts:2088`):
    // stamp the ORCHESTRATOR session onto the run's own `status.json`, so a later reader can scope
    // the async root to one session (`async-status.ts:432`).
    //
    // SUBA-031: the config field is the primary source and is pi's own (`sessionId:
    // ctx.currentSessionId`, `async-execution.ts:1042`). The inherited
    // `CYRUP_SUBAGENT_PARENT_SESSION` anchor survives only as the fallback, because it is published
    // by `cyrup-permission-system` and is therefore absent whenever that extension is not loaded —
    // which used to leave every background run unattributed, and a session-scoped listing must drop
    // an unattributed run (pi's `!==` against `undefined`).
    // `SessionId::parse` IS the non-empty filter now — the `.filter(|id| !id.is_empty())` that
    // used to live here was one of three independent copies of that rule, and the drift between
    // them is what let an empty-string session read as attributed.
    status.session_id = crate::identity::SessionId::parse_opt(config.session_id.as_deref())
        .or_else(|| {
            crate::identity::SessionId::parse_opt(
                crate::background::parent_anchor::resolve_parent_session_anchor().as_deref(),
            )
        });
    // The launching PROCESS, carried from the orchestrator through the one-shot config. Never
    // minted here: `current_completion_owner_id()` in this process would return the RUNNER's
    // identity, and the runner is not who consumes the completion.
    status.completion_owner_id = config.completion_owner_id.clone();
    // pi `processTerminal: { version: 1, state: "pending", runId, runnerProcessInstanceId }` on the
    // initial status (`async-execution.ts:839`, and `subagent-runner.ts:2102` on the runner's own
    // first write). The identity is the ORCHESTRATOR's mint, carried in the one-shot config — never
    // one this process chose, which would agree with nothing the launch recorded.
    //
    // A `pending` proof that is never replaced IS the crash signal: a runner killed by a signal
    // never reaches its close observation, so this record stands unchanged and every reader can
    // tell "died" from "still starting", which `state` alone could not.
    status.process_terminal = config.runner_process_instance_id.as_ref().map(|instance| {
        crate::background::process_terminal::ProcessTerminal::Pending {
            base: crate::background::process_terminal::ProcessTerminalBase::new(
                config.run_id.clone(),
                instance.clone(),
            ),
        }
    });
    status.chain_step_count = Some(config.steps.len());
    // SUBA-093 — one entry per FLAT child, not per top-level step: a `ParallelGroup` declares one
    // `RunStatus::steps` entry per member (pi `subagent-runner.ts:2618-2652` @v0.64.0), which is
    // what makes a `tasks[]` fan-out's members individually addressable.
    status.steps = config
        .steps
        .iter()
        .flat_map(pending_step_statuses_for)
        .enumerate()
        .map(|(flat_index, mut step)| {
            // pi `transcriptPath: resolveAsyncStepTranscriptPath(...)` on every declared status
            // step (`subagent-runner.ts:1965-1986`): stamped BEFORE any child spawns, so a status
            // reader can open the live transcript of a step that is still `Running` — the file
            // that step's own `run_sync` creates at exactly this path.
            step.transcript_path =
                resolve_async_step_transcript_path(config, &step.agent, flat_index);
            step
        })
        .collect();
    // Queued -> Running is always legal (RunState::can_transition_to).
    if status.advance_state(RunState::Running).is_err() {
        // Unreachable in practice (a freshly `queued` status can always advance to Running), but
        // this crate never unwraps a Result — if the transition guard were ever tightened in a
        // way that made this fail, degrade to a terminal Failed record rather than panicking.
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            "internal error: Queued -> Running transition was rejected".to_string(),
            WorkflowResultFields::default(),
        )
        .await;
        return None;
    }
    if let Err(err) = write_atomic_json(&run_paths.status, &status).await {
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            format!("failed to write initial status.json: {err}"),
            WorkflowResultFields::default(),
        )
        .await;
        return None;
    }

    // SCOPE_9/SUBTASK5 — the ACTIVE arm of pi's `updateActiveRunIndex` router
    // (`active-run-index.ts:81-95`). This is the one call site that files a marker rather than
    // releasing one: the run has just become `Running`, and until it does, "which runs are in
    // flight" can only be answered by reading every run directory in the shared per-cwd async
    // root. Issued AFTER the status write so the marker never advertises a run whose own record
    // failed to land. Best-effort and logged, the contract every index call site in this crate
    // shares.
    if let Err(err) =
        crate::background::active_run_index::update_active_run_index(&run_paths.run_dir, &status)
            .await
    {
        tracing::warn!(
            run_id = %status.run_id,
            error = %err,
            "failed to write the async active-run index marker; the run itself is unaffected"
        );
    }
    Some(status)
}

/// The control-inbox directory (`<run_dir>/control/`) MUST exist before
/// `spawn_control_watcher` installs its `notify::PollWatcher` below: that watcher targets the
/// DIRECTORY, not the (not-yet-existing, created-on-first-interrupt) file itself, since
/// watching a not-yet-existing file path is unreliable across platforms (see
/// `control::watch_control_inbox`'s own doc). Watching a directory that does not exist YET
/// fails to install at all on every platform this crate ships to — and `spawn_control_watcher`
/// degrades that failure to a silent no-op (by design, so a watcher failure never strands the
/// run), which would silently make EVERY interrupt delivered after this point unobservable:
/// `run_inner`'s own per-iteration re-check only re-scans pending chain-append requests
/// (R-SA-096), it has no independent interrupt-file poll fallback of its own — the `interrupted`
/// flag is set SOLELY by this watcher task. Creating the directory here, unconditionally,
/// before the watcher is installed, closes that gap.
///
/// This MUST route through `finish_run` on failure, matching every other pre-loop fallible step
/// immediately above (never a bare `?`, found bypassing `finish_run` entirely in second-pass
/// adversarial review): a bare `?` here would return `Err` straight out of `run` itself, leaving
/// `status.json` permanently stuck at the `Running` record already written above and NO
/// `ResultFile` ever written — directly contradicting this function's own documented "effectively
/// infallible from the caller's point of view" contract (every internal failure captured into a
/// terminal on-disk record, never propagated) and silently violating R-SA-077's ordering
/// invariant by skipping BOTH writes rather than merely reordering them.
///
/// The `status` published by [`publish_initial_status`] travels through this function so the
/// failure path can spend it on the terminal record; `None` means it already has.
pub(super) async fn ensure_control_inbox_dir(
    config: &RunnerConfig,
    run_paths: &RunPaths,
    status: RunStatus,
) -> Option<RunStatus> {
    if let Err(err) = tokio::fs::create_dir_all(
        run_paths
            .control_inbox
            .parent()
            .unwrap_or(&run_paths.run_dir),
    )
    .await
    {
        finish_run(
            run_paths,
            status,
            RunState::Failed,
            Vec::new(),
            config.cwd.clone(),
            config.session_file.clone(),
            format!("failed to create control-inbox directory: {err}"),
            WorkflowResultFields::default(),
        )
        .await;
        return None;
    }
    Some(status)
}

/// Best-effort recovery of a [`RunId`] from `run_paths`' own `run_dir` path (its final component
/// is always the run id, per [`RunPaths::for_run`]'s construction) — used only on the
/// no-config-available error paths above, where no [`RunnerConfig::run_id`] exists to read.
pub(super) fn run_id_from_paths(run_paths: &RunPaths) -> RunId {
    run_paths
        .run_dir
        .file_name()
        .map(|name| RunId::from_token(name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| RunId::from_token("unknown-run"))
}
