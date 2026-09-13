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
use crate::background::flat_index::pending_step_statuses_for;
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

    let Some(status) = publish_initial_status(&config, run_paths).await else {
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
    let (telemetry_tx, telemetry_rx) = tokio::sync::mpsc::unbounded_channel::<TelemetryMsg>();
    let telemetry_task =
        spawn_telemetry_task(run_paths.clone(), Arc::clone(&shared_status), telemetry_rx);

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
        &mut events,
    )
    .await;

    // `run_inner` has returned, so its executor (holding the last live-telemetry sender) is dropped
    // and the telemetry task observes all-senders-dropped and finishes — await it so no late
    // telemetry status write races the terminal record `finish_run` writes.
    let _ = telemetry_task.await;

    let duration_ms = (crate::time::now_epoch_millis() - overall_started_at).max(0);
    let (terminal_state, results, final_error) =
        settle_loop_outcome(loop_outcome, &config, &mut events, duration_ms).await;

    // Recover the final live status (its accumulated per-step telemetry + workflow-graph snapshot)
    // so the terminal `status.json` `finish_run` writes preserves everything the pump accumulated.
    let final_status = lock_status(&shared_status).clone();

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

    Ok(())
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
    status.chain_step_count = Some(config.steps.len());
    // SUBA-093 — one entry per FLAT child, not per top-level step: a `ParallelGroup` declares one
    // `RunStatus::steps` entry per member (pi `subagent-runner.ts:2618-2652` @v0.64.0), which is
    // what makes a `tasks[]` fan-out's members individually addressable.
    status.steps = config
        .steps
        .iter()
        .flat_map(pending_step_statuses_for)
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
