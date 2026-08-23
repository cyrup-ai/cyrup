//! The live-run control verbs: interrupt, stop, dismiss, steer, resume and append-step.

use std::path::Path;

use crate::background::{run_status, RunId, RunMode, RunPaths, RunState, StepState};
use crate::background::atomic::write_atomic_json;
use crate::background::control::{self, AppendOutcome, InterruptOutcome, ResumeOutcome};
use crate::discovery::types::AgentReadScope;
use crate::error::SubagentError;
use crate::fork_context::ContextMode;
use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root, default_results_dir};
use crate::extension::executor::requests::BackgroundStepsSpec;
use crate::extension::tool::text::{
    dismiss_not_running_refusal, STEER_ACK_POLL_INTERVAL, STEER_ACK_TIMEOUT,
    STEER_FOREGROUND_RUN_REFUSAL, STOP_FOREGROUND_RUN_REFUSAL, STOP_NESTED_RUN_REFUSAL,
    STOP_NO_STOPPABLE_RUN_REFUSAL,
};

impl SubagentExecutor {

    /// `action: "interrupt"` (C5): deliver a soft, resumable interrupt (R-SA-084 — a *pause*
    /// request, never a kill) to the target async run, or, with no id, to the most-recently-updated
    /// running run in this cwd's async root — pi `subagent-executor.ts:2871-2911`.
    ///
    /// # Errors
    ///
    /// Returns `Err` if no interrupt-capable run is found, if the target is not Running (R-SA-079),
    /// or if the underlying delivery fails.
    pub async fn control_interrupt(&self, cwd: &Path, target: Option<&str>) -> Result<String, String> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let run_id = match target {
            Some(explicit) => explicit.to_string(),
            None => {
                // No id: interrupt the most-recently-updated running run (the list is already sorted
                // running-first, most-recent-first), mirroring pi's "defaults to the most recently
                // active controllable run" contract for interrupt.
                let runs = run_status::list_active_runs(&async_root, &results_dir, self.current_session_id().as_deref())
                    .await
                    .map_err(|e| e.to_string())?;
                runs.iter()
                    .find(|run| run.status.state == RunState::Running)
                    .map(|run| run.status.run_id.as_str().to_string())
                    .ok_or_else(|| "No interrupt-capable run found in this session.".to_string())?
            }
        };
        match control::interrupt(&async_root, &results_dir, &run_id, "interrupt-action", None).await {
            Ok(InterruptOutcome::Delivered | InterruptOutcome::AlreadyPending) => {
                Ok(format!("Interrupt requested for async run {run_id}."))
            }
            Ok(InterruptOutcome::NotRunning) => Err(format!(
                "No running async run with an interrupt-capable pid was found for '{run_id}'."
            )),
            Err(e) => Err(e.to_string()),
        }
    }

    /// G77 — `action: "stop"` (pi `stopAsyncRun`, `runs/foreground/async-stop-action.ts:23-64`
    /// @v0.43.0, dispatched at `subagent-executor.ts:4771-4815`): deliver a TERMINAL,
    /// non-resumable stop to a background run.
    ///
    /// This is deliberately NOT [`Self::control_interrupt`]. An interrupt is a soft, resumable
    /// pause — the run ends [`RunState::Paused`] and `action: "resume"` is expected to pick it back
    /// up. A stop is terminal: the run ends [`RunState::Stopped`], every unfinished step ends
    /// [`crate::background::StepState::Stopped`] with
    /// [`crate::background::control::STOP_MESSAGE`], the whole descendant subtree is stopped with
    /// it, and `action: "resume"` MUST refuse it (`async-resume.ts:406`).
    ///
    /// pi's guards, in pi's order (`subagent-executor.ts:4771-4815`):
    ///
    /// * `targetRunId = params.runId ?? params.id` — `runId` first, same precedence as `interrupt`;
    /// * with neither `id` nor `dir`, the exact refusal `"action='stop' requires id or dir."`
    ///   (`:4789`). There is deliberately NO "most recently active run" default here — upstream has
    ///   one for `interrupt` and not for `stop`, because a stop is unrecoverable;
    /// * a `nested` run resolves to [`STOP_NESTED_RUN_REFUSAL`] (`:4796`);
    /// * a `foreground` run resolves to [`STOP_FOREGROUND_RUN_REFUSAL`] (`:4797`);
    /// * with `dir`, the id is `location.resolvedId ?? targetRunId ?? basename(dir)` (`:4782`);
    /// * an id that names no async run of this session at all reaches
    ///   [`STOP_NO_STOPPABLE_RUN_REFUSAL`] (`:4812`, upstream's `stopAsyncRun` → `null` fallback);
    /// * the reconciled state must be `running` or `queued`, else the exact refusal text
    ///   `"No running or queued async run was found for '{id}'."` with `isError: true`
    ///   (`async-stop-action.ts:41-47`);
    /// * success is the exact text `"Stop requested for async run {id}."` (`:56`);
    /// * a delivery failure is `"Failed to stop async run {id}: {message}"` (`:60`).
    ///
    /// Upstream's `action === "stop"` block spells SEVEN distinct literals of its own (`:4776`,
    /// `:4783`, `:4789`, `:4796`, `:4797`, `:4801`, `:4812`) and delegates three more to
    /// `stopAsyncRun`. This function now reproduces five of the seven plus two of the three:
    ///
    /// | upstream string | here |
    /// |---|---|
    /// | `action='stop' requires id or dir.` (`:4789`) | yes |
    /// | `action='stop' supports current-session top-level async runs only.` (`:4796`) | yes |
    /// | `action='stop' supports async runs only. Use action='interrupt' for foreground runs.` (`:4797`) | yes |
    /// | `No running or queued async run was found for '{id}'.` (`:4783`, `async-stop-action.ts:41`) | yes |
    /// | `No stoppable async run found in this session.` (`:4812`) | yes |
    /// | `Stop requested for async run {id}.` (`async-stop-action.ts:54`) | yes |
    /// | `Failed to stop async run {id}: {message}` (`async-stop-action.ts:60`) | yes |
    /// | `Stop requested for async workflow {id}.` (`:4776`) | **unported subsystem** |
    /// | `Workflow {id} is not controlled by this extension runtime; reload recovery cannot stop it safely.` (`:4801`) | **unported subsystem** |
    /// | `Async run '{id}' was not found in the active session.` (`async-stop-action.ts:34`) | **unported subsystem** |
    ///
    /// The two `Workflow …` strings are the `workflowControllers` fast path and the `mode ===
    /// "workflow"` reload-recovery refusal. Both are gated on upstream's fourth run mode
    /// (`SubagentRunMode = "single" | "parallel" | "chain" | "workflow"`, `shared/types.ts:231`) and
    /// its `state.workflowControllers` registry (`shared/types.ts:1590`); [`crate::background::RunMode`] has
    /// three variants and this crate has no controller registry, so both branches would be dead code
    /// today. They enter scope with the WorkflowScript runtime, not before.
    ///
    /// `Async run '{id}' was not found in the active session.` is `stopAsyncRun`'s session-scope
    /// guard (`status?.sessionId !== state.currentSessionId`). [`crate::background::RunStatus`] records no
    /// session id at all, so there is nothing to compare; it enters scope with per-run session
    /// attribution in the async store.
    ///
    /// # Errors
    ///
    /// Returns `Err` with whichever of the refusal/failure texts above applies.
    pub async fn control_stop(
        &self,
        cwd: &Path,
        target: Option<&str>,
        dir: Option<&str>,
    ) -> Result<String, String> {
        if target.is_none() && dir.is_none() {
            return Err("action='stop' requires id or dir.".to_string());
        }
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);

        // pi `:4795-4797`, in pi's own order: `resolveSubagentRunId` classifies the selector first,
        // and the `nested` and `foreground` kinds each get their OWN sentence before anything
        // touches the async store. Both are id-addressed only — upstream's `dir` form returns from
        // the `params.dir` branch at `:4783` long before this classification runs.
        let mut resolved_async_id: Option<String> = None;
        if let Some(id) = target
            && dir.is_none()
        {
            // pi `:4796`: the selector named a run nested inside another run's subtree. Real id,
            // wrong scope — never reported as a missing async run.
            if self.resolves_to_nested_run(id) {
                return Err(STOP_NESTED_RUN_REFUSAL.to_string());
            }
            // pi `:4797`: a live FOREGROUND run is refused with its own sentence pointing at
            // `interrupt`, never silently treated as a missing async run.
            if self.is_live_foreground_run(id) {
                return Err(STOP_FOREGROUND_RUN_REFUSAL.to_string());
            }
            // pi `:4812` via `stopAsyncRun` → `getAsyncStopTarget` → `undefined`
            // (`async-stop-action.ts:18-20`: no `dir` location and `state.asyncJobs.get(runId)` is
            // absent). cyrup's on-disk analogue of "not a tracked async job of this session" is
            // `resolve_run_id` finding no run directory / status / result for the selector.
            // A safe-token/ambiguity failure surfaces as its own message, matching upstream's
            // `catch { return { text: error.message } }` around `resolveSubagentRunId` (`:4790-4795`).
            match run_status::resolve_run_id(&async_root, &results_dir, id)
                .await
                .map_err(|e| e.to_string())?
            {
                // pi `:4806`: `resolved?.kind === "async" ? resolved.id : targetRunId` — the run
                // that actually gets stopped (and gets named in the confirmation) is the RESOLVED
                // id, so a unique run-id PREFIX stops the run it names instead of being reported
                // missing under its own abbreviation.
                Some(resolved) => resolved_async_id = Some(resolved.as_str().to_string()),
                None => return Err(STOP_NO_STOPPABLE_RUN_REFUSAL.to_string()),
            }
        }

        // pi `:4782`: `location.resolvedId ?? targetRunId ?? path.basename(location.asyncDir ??
        // params.dir)`.
        let run_id = match dir {
            Some(dir_arg) => run_status::reconcile_by_dir(Path::new(dir_arg), &results_dir)
                .await
                .map_err(|e| e.to_string())?
                .map(|(status, _)| status.run_id.as_str().to_string())
                .or_else(|| target.map(str::to_string))
                .or_else(|| {
                    Path::new(dir_arg)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                })
                .ok_or_else(|| "action='stop' requires id or dir.".to_string())?,
            None => resolved_async_id.unwrap_or_else(|| target.unwrap_or_default().to_string()),
        };

        match control::stop(&async_root, &results_dir, &run_id, "stop-action", None).await {
            Ok(control::StopOutcome::Requested) => {
                Ok(format!("Stop requested for async run {run_id}."))
            }
            Ok(control::StopOutcome::NotStoppable) => Err(format!(
                "No running or queued async run was found for '{run_id}'."
            )),
            Err(e) => Err(format!("Failed to stop async run {run_id}: {e}")),
        }
    }

    /// SUBA-057 — `action: "dismiss"` (pi `dismissRecoveredWorkflow`,
    /// `runs/foreground/async-dismiss-action.ts:11-85` @v0.47.1, dispatched at
    /// `runs/foreground/subagent-executor.ts:5872-5885`).
    ///
    /// Clears a **reload-orphaned** run from the display. It terminates nothing: its entire effect
    /// is to stamp [`crate::background::RunStatus::display_dismissed_at`], which the three readers landed alongside
    /// the field already honour ([`crate::background::reconcile::reconcile`],
    /// [`run_status::list_active_runs`], and the single-run status view). Before this method the
    /// field had no producer, so a run whose runner is gone but whose `status.json` still claims
    /// `Running` — and which reconciliation therefore cannot advance, because it has no pid to
    /// probe — stayed in `/subagents-fleet` and `{action:"status"}` forever with no supported way
    /// to clear it.
    ///
    /// Upstream's five refusals are ported in upstream's own order, each with its exact sentence:
    ///
    /// 1. `:18-22` no async dir on disk → *"…has no disk status to dismiss."*
    /// 2. `:24-30` no readable `status.json` → *"…is not a recovered workflow."*
    /// 3. `:31-36` not the active session → *"…was not found in the active session."*
    /// 4. `:37-42` a live controller → *"…still has a live controller and cannot be dismissed."*
    /// 5. `:43-48` not `running` → *"…is `<state>`, not running."*
    ///
    /// then the result-file re-reconcile (`:50-63`), the stamp + atomic write (`:65-66`), the
    /// post-write re-reconcile (`:67-74`), and the job-map eviction (`:76-79`).
    ///
    /// # [CYRUP-DELTA] on refusal 2's `mode` half
    ///
    /// Upstream's second refusal is `!status || status.mode !== "workflow"` (`:25`). cyrup's
    /// [`RunMode`] (`background/mod.rs:242`) has three variants — `Single`/`Parallel`/`Chain` —
    /// and no `Workflow`, because upstream's fourth `SubagentRunMode` member
    /// (`shared/types.ts:231`) belongs to the `workflowScript` run shape, which this crate has not
    /// ported. Porting the mode test literally would mean adding a variant nothing constructs, so
    /// the gate would refuse **every** run and `dismiss` would be unreachable — a verb that cannot
    /// fire is a worse port than one whose narrowing predicate is absent. Only the `!status` half
    /// is ported; the `mode` half is recorded here and lands with the `workflowScript` mode.
    ///
    /// # [CYRUP-DELTA] on refusal 4's carrier
    ///
    /// Upstream tests `state.workflowControllers.has(runId)` (`:37`) — an in-process
    /// `AbortController` map, because upstream's workflow runs are driven inside the extension
    /// host. cyrup drives every background run from a **detached runner process**
    /// (`background/spawn_detached.rs`), so its controller is that process and the test that
    /// carries the same meaning is a zero-signal liveness probe of the recorded pid
    /// ([`crate::background::reconcile::check_pid_liveness`]). A run with no recorded pid has no
    /// controller and is dismissible, which is exactly the reload-orphaned case the verb exists
    /// for.
    ///
    /// # Errors
    ///
    /// Returns each of the five refusals above, or a resolution/read/write failure, as `Err`.
    pub async fn control_dismiss(&self, cwd: &Path, target: Option<&str>) -> Result<String, String> {
        // pi `:5873`: `paramsWithResolvedCwd.runId ?? paramsWithResolvedCwd.id` is resolved by the
        // caller; a missing selector is its own sentence, ahead of everything else.
        let Some(target) = target.filter(|id| !id.trim().is_empty()) else {
            return Err("action='dismiss' requires id.".to_string());
        };
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);

        // pi `:5877-5883` (`resolveSubagentRunId`) then `:18-22` (`!asyncDir`). A selector that
        // resolves to no async run at all is upstream's "no disk status" case, so it gets that
        // sentence rather than the not-a-workflow one.
        let resolved = run_status::resolve_run_id(&async_root, &results_dir, target)
            .await
            .map_err(|e| e.to_string())?;
        let Some(run_id) = resolved else {
            return Err(format!(
                "Recovered workflow '{target}' has no disk status to dismiss."
            ));
        };
        let run_id_text = run_id.as_str().to_string();
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        // pi `:24-30` — `readStatus(asyncDir)` returning nothing is refused before any other
        // property of the run is consulted. Read RAW here, not through the reconciliation gate:
        // upstream's `readStatus` is a plain file read, and reconciling first would let the
        // liveness probe rewrite the very record the refusals below are about to judge.
        let Some(status) = control::read_status_file(&paths.status)
            .await
            .map_err(|e| e.to_string())?
        else {
            return Err(format!("Run '{run_id_text}' is not a recovered workflow."));
        };

        // pi `:31-36`: `!state.currentSessionId || status.sessionId !== state.currentSessionId`.
        // Both halves, including the "this host has no session at all" one.
        let current_session = self.current_session_id();
        if current_session.is_none() || status.session_id != current_session {
            return Err(format!(
                "Recovered workflow '{run_id_text}' was not found in the active session."
            ));
        }

        // pi `:37-42` — see the [CYRUP-DELTA] above for why the carrier is a pid probe.
        if status
            .pid
            .is_some_and(|pid| crate::background::reconcile::check_pid_liveness(pid).is_possibly_alive())
        {
            return Err(format!(
                "Workflow '{run_id_text}' still has a live controller and cannot be dismissed."
            ));
        }

        // pi `:43-48`.
        if status.state != RunState::Running {
            return Err(dismiss_not_running_refusal(&run_id_text, status.state));
        }

        // pi `:50-63`: only when a terminal result file is already on disk does upstream
        // re-reconcile before stamping — the result is authoritative and may have finished the run
        // between the read above and now, in which case there is nothing orphaned to dismiss.
        let mut latest = status;
        if tokio::fs::try_exists(&paths.result).await.unwrap_or(false) {
            let reconciled = crate::background::reconcile::reconcile_now(&paths, None)
                .await
                .map_err(|e| e.to_string())?;
            if reconciled.status.state != RunState::Running {
                return Err(dismiss_not_running_refusal(
                    &run_id_text,
                    reconciled.status.state,
                ));
            }
            latest = reconciled.status;
        }

        // pi `:65-66`: `{ ...latestStatus, displayDismissedAt: Date.now() }` written atomically.
        latest.display_dismissed_at = Some(crate::background::now_epoch_millis_pub());
        write_atomic_json(&paths.status, &latest)
            .await
            .map_err(|e| format!("Failed to dismiss async run {run_id_text}: {e}"))?;

        // pi `:67-74`: re-reconcile and refuse if the run turned out not to be running after all.
        //
        // Upstream's `reconcileAsyncRun` returns `status: null` for a record carrying the marker
        // (`stale-run-reconciler.ts:359-361`), so its `if (repaired && …)` guard is vacuous on the
        // happy path. cyrup's carrier for that `null` is
        // [`crate::background::reconcile::ReconcileAction::DisplayDismissed`], so the action — not
        // the status — is what must be tested here, exactly as that variant's own doc requires.
        let repaired = crate::background::reconcile::reconcile_now(&paths, None)
            .await
            .map_err(|e| e.to_string())?;
        if repaired.action != crate::background::reconcile::ReconcileAction::DisplayDismissed
            && repaired.status.state != RunState::Running
        {
            return Err(dismiss_not_running_refusal(
                &run_id_text,
                repaired.status.state,
            ));
        }

        // pi `:76-79`: `state.asyncJobs.delete(...)` / `state.fleetJobs?.delete(...)`. cyrup's
        // single in-memory job map is the [`JobTracker`]; the fleet widget has no separate map of
        // its own — it renders from `list_active_runs`, which already drops the dismissed run.
        //
        // (Upstream's `updateActiveRunIndex(asyncDir, "complete")` at `:75` has no counterpart:
        // `background/active-run-index.ts` is unported crate-wide, so there is no index to update.)
        self.tracker.untrack(&run_id);

        Ok(format!(
            "Dismissed recovered workflow {run_id_text} from the display. No running work was \
             terminated."
        ))
    }

    /// G77 — pi's no-UI `/subagents-stop` fallback (`slash/slash-commands.ts:206-217,774`
    /// @v0.43.0's `stopFallbackText`): with no explicit id and no overlay seam, list the stoppable
    /// targets and the exact commands that stop each, rather than guessing one.
    ///
    /// Upstream's target list is `discoverStopTargets` = current-session queued/running async runs
    /// (`formatAsyncStopTarget`, `:168-178`) PLUS scheduled runs (`scheduledStopTargets`, `:180-196`).
    /// The `schedule.*` family is unported (it is the one part of pi's `SUBAGENT_ACTIONS` this
    /// crate's schema deliberately omits, "MUST NOT be advertised until their manager exists"), so
    /// the scheduled half contributes nothing here and only the async half renders — which is
    /// exactly what upstream's own `scheduledStopTargets` `catch { return []; }` produces for a
    /// runtime with no schedule store.
    pub async fn format_stop_targets(&self, cwd: &Path) -> Result<String, String> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let runs = run_status::list_active_runs(&async_root, &results_dir, self.current_session_id().as_deref())
            .await
            .map_err(|e| e.to_string())?;
        if runs.is_empty() {
            return Ok(
                "No active current-session async runs or scheduled subagent runs to stop."
                    .to_string(),
            );
        }
        let mut lines = vec!["Subagent stop targets:".to_string(), String::new()];
        for run in &runs {
            let id = run.status.run_id.as_str();
            lines.push(format!(
                "- {id} · {} · {}",
                run_status::run_mode_label(run.status.mode),
                run_status::progress_label(&run.status)
            ));
            lines.push(format!(
                "  {} · {}",
                run_status::run_state_label(run.status.state),
                run.dir.display()
            ));
            lines.push(format!(
                "  stop async run: subagent({{ action: \"stop\", id: \"{id}\" }})"
            ));
            lines.push(format!("  slash: /subagents-stop {id}"));
        }
        Ok(lines.join("\n"))
    }

    /// G90 — `action: "steer"` (pi `subagent-executor.ts:570-626,3194-3220` @v0.34.0): queue
    /// NON-TERMINAL guidance for a still-live background child.
    ///
    /// This is deliberately NOT [`Self::control_resume`]. `resume` interrupts the child first and
    /// then delivers a follow-up (or revives a finished one from its transcript); `steer` never
    /// interrupts and never respawns — it drops a request into the run's control inbox
    /// ([`crate::background::control::request_async_steer`]) and the runner hands it to the running
    /// child. That is why the two verbs coexist upstream, and why the confirmation text below says
    /// "queued": the parent's job ends at the inbox.
    ///
    /// pi's guards, in pi's order:
    ///
    /// * `message` is required, falling back to `task`, and must be non-blank (`:3195-3196`);
    /// * `id` or `dir` is required (`:3208`);
    /// * the run must reconcile to `Running` or `Queued` (`:585-590`);
    /// * an explicit `index` must be in range (`:592-598`) and must name a child that is `running`
    ///   or `pending` (`:600-607`);
    /// * with no `index`, a multi-child run with nothing running yet is refused with pi's
    ///   "Provide index to steer a queued child." (`:609-617`).
    ///
    /// # Errors
    ///
    /// Returns each of the above refusals, or a resolution/reconciliation/write failure, as `Err`.
    // SUBA-049 raised this from 7 to 8 arguments by adding `mode`. The alternative — an options
    // struct — would be a cyrup-original shape for a function whose whole contract is pi's
    // `steerAsyncRun` input, and this is the crate's established treatment for exactly that
    // (`build_attempt_spawn_plan_with_read_requirement` carries the same allowance for the same
    // reason).
    #[allow(clippy::too_many_arguments)]
    pub async fn control_steer(
        &self,
        cwd: &Path,
        target: Option<&str>,
        dir: Option<&str>,
        message: Option<&str>,
        task: Option<&str>,
        index: Option<usize>,
        mode: Option<&str>,
    ) -> Result<String, String> {
        // SUBA-049 — pi `mode` (`extension/schemas.ts:283` @v0.43.0), validated HERE rather than by
        // serde so an unrecognised value is a sentence the model can act on. Upstream's schema
        // `enum` does the rejecting there; cyrup's schema carries the same enum, but a tool call
        // that bypasses schema validation must still be refused rather than silently defaulted to
        // the INTERRUPTING mode — quietly upgrading `follow_up` to `steer` is the worst available
        // failure for this parameter.
        let mode = match mode.map(str::trim).filter(|m| !m.is_empty()) {
            None => None,
            Some(raw) => Some(control::SteerDeliveryMode::parse(raw).ok_or_else(|| {
                format!("Unknown steer mode '{raw}'. Valid: steer, follow_up, auto.")
            })?),
        };
        let message = message
            .or(task)
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .ok_or_else(|| "action='steer' requires message.".to_string())?;
        if target.is_none() && dir.is_none() {
            return Err("action='steer' requires id or dir.".to_string());
        }
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let (status, paths) = match (target, dir) {
            (Some(id), None) => {
                // pi classifies an id-addressed selector with `resolveSubagentRunId` BEFORE it
                // touches the async store (`subagent-executor.ts:3211` @v0.34.0) and branches on
                // all three outcomes, with a DISTINCT refusal for each. cyrup collapsed two of
                // them into the `steerAsyncRun` "no live run directory" text, which told a caller
                // who had named a live FOREGROUND run — or a typo — that an async run existed but
                // had lost its directory. Both are restored here, in pi's own order.
                if self.is_live_foreground_run(id) {
                    // pi `:3217`, rebranded (`Pi child sessions` → `Cyrup child sessions`,
                    // matching this crate's standing rebrand of pi's user-facing product noun —
                    // see `control_steer`'s own success text).
                    return Err(STEER_FOREGROUND_RUN_REFUSAL.to_string());
                }
                // pi `:3218`: the selector resolved to NOTHING — neither a foreground run nor an
                // async one. Distinct from "resolved, but its run dir is gone", which the shared
                // `ok_or_else` below still reports with `steerAsyncRun`'s own text (pi `:3580`).
                if run_status::resolve_run_id(&async_root, &results_dir, id)
                    .await
                    .map_err(|e| e.to_string())?
                    .is_none()
                {
                    return Err(format!("No async run found for '{id}'."));
                }
                run_status::reconcile_by_id(&async_root, &results_dir, id).await
            }
            (_, Some(dir)) => run_status::reconcile_by_dir(Path::new(dir), &results_dir).await,
            (None, None) => Ok(None),
        }
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            format!(
                "Async run '{}' has no live run directory to steer.",
                target.or(dir).unwrap_or("")
            )
        })?;

        let run_id = status.run_id.as_str().to_string();
        if !matches!(status.state, RunState::Running | RunState::Queued) {
            return Err(format!(
                "Async run '{run_id}' is not running or queued and cannot be steered."
            ));
        }
        let steps = &status.steps;
        if let Some(index) = index {
            let Some(step) = steps.get(index) else {
                return Err(format!(
                    "Async run '{run_id}' has {} children. Index {index} is out of range.",
                    steps.len()
                ));
            };
            if !matches!(step.status, StepState::Running | StepState::Pending) {
                return Err(format!(
                    "Async run '{run_id}' child {index} is {} and cannot be steered.",
                    run_status::step_state_label(step.status)
                ));
            }
        } else if steps.len() > 1
            && !steps.iter().any(|s| s.status == StepState::Running)
        {
            return Err(format!(
                "Async run '{run_id}' has no running child yet. Provide index to steer a queued \
                 child."
            ));
        }

        let (_, request_id) = control::request_async_steer_with_mode(
            &paths.run_dir,
            message,
            mode,
            index,
            Some("steer-action"),
        )
        .await
        .map_err(|e| e.to_string())?;

        // SUBA-049 — WAIT FOR THE CHILD'S ANSWER. This is the whole item: before it, the function
        // returned here with "Steering queued …", which was true of the file drop and said nothing
        // about delivery. A steer that reached a child mid-tool and never got a turn boundary, one
        // refused by a child whose host cannot inject messages, and one acted on immediately were
        // three identical successes.
        //
        // pi's own budget: `waitForSteeringAction({ …, timeoutMs: input.ackTimeoutMs ?? 3_000 })`
        // (`runs/foreground/async-steering-action.ts`). Upstream waits on `status.steering`, which
        // its RUNNER folds acks into; cyrup's parent reads the ack files directly and narrows them
        // to this request — see [`control::take_steer_acks`]'s `[CYRUP-DELTA]`.
        let outcome = Self::await_steer_ack(&paths.run_dir, &request_id, index).await;
        let state = match outcome.as_ref() {
            // pi's `stateText` (`async-steering-action.ts`'s final `return`). No acknowledgment
            // inside the budget is upstream's `pending`, NOT a failure: the request is on disk and a
            // child that reaches a safe point later still takes it.
            None => "pending",
            Some(ack) => ack.state.as_str(),
        };
        let text = format!("Steering {state} for async run {run_id} (request {request_id}).");
        match outcome {
            // pi sets `isError` for `failed`/`partial` only; `pending` and `queued` are ordinary
            // successes because the request is still live.
            Some(ack) if ack.state == control::SteerAckState::Failed => {
                Err(format!("{text} {}", ack.message))
            }
            _ => Ok(text),
        }
    }

    /// SUBA-049 — poll this run's steer-acknowledgment directory for `request_id` until one arrives
    /// or [`STEER_ACK_TIMEOUT`] elapses.
    ///
    /// Returns the LAST acknowledgment seen for the request, not the first, and that matters: the
    /// lifecycle can produce two (`queued` then `delivered`) and the later one is the outcome.
    /// [`control::take_steer_acks`] already returns them in lifecycle order — see
    /// `steer_ack_write_path` for why the file name encodes it.
    ///
    /// `index` narrows a fan-out's answers to the addressed child; with no index every running
    /// child is a legitimate answerer and the first one back is taken, which is upstream's own
    /// `targetIndexes` semantics collapsed to the single answer this surface reports.
    async fn await_steer_ack(
        run_dir: &Path,
        request_id: &str,
        index: Option<usize>,
    ) -> Option<control::SteerAck> {
        let deadline = std::time::Instant::now() + STEER_ACK_TIMEOUT;
        let mut latest: Option<control::SteerAck> = None;
        loop {
            for ack in control::take_steer_acks(run_dir, Some(request_id)).await {
                if index.is_some_and(|want| want != ack.index) {
                    continue;
                }
                latest = Some(ack);
            }
            if latest.is_some() || std::time::Instant::now() >= deadline {
                return latest;
            }
            tokio::time::sleep(STEER_ACK_POLL_INTERVAL).await;
        }
    }

    /// `action: "resume"` (C5): the R-SA-085/086 fork — steer a still-running run's live child, or
    /// revive a terminal run from its persisted transcript — pi `subagent-executor.ts:2865`/
    /// `801-1031`. Requires a follow-up `message` (falling back to `task`) and a run `id`.
    ///
    /// The running-selection branch interrupts the live child, then DELIVERS the follow-up over the
    /// broker to that child's deterministic registered bridge target — pi
    /// `deliverSubagentIntercomMessageEvent(events, target.intercomTarget, …)`
    /// (`subagent-executor.ts:848-878`). The child WAS activated as a bridge participant at its spawn
    /// (the subagents spawn overlay writes `CYRUP_SUBAGENT_ORCHESTRATOR_TARGET`/`_RUN_ID`/
    /// `_CHILD_AGENT`/`_CHILD_INDEX`/`_INTERCOM_SESSION_NAME`, so the child's `IntercomExtension`
    /// registered `contact_supervisor` + a broker presence under
    /// `resolve_subagent_intercom_target(run_id, agent, index)`), so this arm recovers that same
    /// target from the reconciled run status (`steps[step_index].agent` + the step index) and steers
    /// it via the [`crate::tui::intercom::SteerChannel`] threaded in by
    /// `SubagentsExtension::with_channels`. pi's "intercom target is not registered" guidance is
    /// returned ONLY as the genuine delivery-FAILED fallback (no live broker, or no registered
    /// receiver at that target) — the caller then waits for the pause and retries, hitting the
    /// terminal-revival branch. The terminal-revival branch respawns a fresh detached child seeded
    /// from the transcript, running the run's REAL resolved persona (T0.1/C13), and hard-fails (no
    /// silent fresh-session fallback) when no transcript exists.
    ///
    /// # Errors
    ///
    /// Returns `Err` for a missing message/id, the delivery-failed intercom-unregistered live-steer
    /// notice, a no-transcript revival, or any resolution/spawn failure.
    pub async fn control_resume(
        &self,
        cwd: &Path,
        target: Option<&str>,
        message: Option<&str>,
        task: Option<&str>,
        index: Option<usize>,
    ) -> Result<String, String> {
        let follow_up = message.or(task).map(str::trim).unwrap_or_default();
        if follow_up.is_empty() {
            return Err("action='resume' requires message.".to_string());
        }
        let Some(run_id) = target else {
            return Err("action='resume' requires id.".to_string());
        };
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        match control::resume(&async_root, &results_dir, run_id, index).await {
            Ok(ResumeOutcome::SteerRunning { step_index }) => {
                // pi (`subagent-executor.ts:848-878`): interrupt the live child, then DELIVER the
                // follow-up over the broker to that child's registered bridge target. Recover the
                // child's deterministic target from the reconciled run status — the resumed step's
                // REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let source_paths = RunPaths::for_run(
                    &async_root,
                    &results_dir,
                    &RunId::from_token(run_id.to_string()),
                );
                // pi `interruptLiveAsyncResumeTarget` (`background/async-resume.ts:53-56`):
                // re-reconcile and REQUIRE `status.state === "running"` with a numeric pid before
                // even attempting to interrupt — a reconciliation failure, a run that is no longer
                // Running, or a Running status with no known runner pid all abort the WHOLE resume
                // with this exact diagnostic, rather than silently falling through to steer a child
                // that was never confirmed interruptible.
                let status = match control::reconcile_before_control_op(&source_paths).await {
                    Ok(status) if status.state == RunState::Running && status.pid.is_some() => {
                        status
                    }
                    _ => {
                        return Err(format!(
                            "Async run {run_id} is live but no interrupt-capable runner pid was \
                             found."
                        ));
                    }
                };
                // Recover the child's deterministic target from the reconciled run status — the
                // resumed step's REAL agent + its flat index reproduce the SAME
                // `resolve_subagent_intercom_target(run_id, agent, index)` string the child
                // registered its broker presence under at spawn.
                let (child_target, child_agent) = match status.steps.get(step_index) {
                    Some(step) => (
                        Some(crate::spawn::intercom_target::resolve_subagent_intercom_target(
                            run_id,
                            &step.agent,
                            step_index,
                        )),
                        Some(step.agent.clone()),
                    ),
                    None => (None, None),
                };
                // Interrupt the live child (genuine), matching pi's interrupt-then-deliver order
                // (`subagent-executor.ts:846-859`): a FAILED interrupt is returned as the error
                // result immediately, before any follow-up delivery is attempted — it must never be
                // silently swallowed and fall through to steering a child that may still be running
                // its prior turn.
                if let Err(e) =
                    control::interrupt(&async_root, &results_dir, run_id, "async-resume", None)
                        .await
                {
                    return Err(format!("Failed to interrupt async run {run_id}: {e}"));
                }
                // pi's follow-up header includes the resolved agent name (`subagent-executor.ts:863`:
                // `Follow-up for async run ${target.runId} (${target.agent}):`).
                let follow_up_message = match &child_agent {
                    Some(agent) => format!("Follow-up for async run {run_id} ({agent}):\n\n{follow_up}"),
                    None => format!("Follow-up for async run {run_id}:\n\n{follow_up}"),
                };
                // pi's `deliverSubagentIntercomMessageEvent` bounds EVERY caller (including this
                // live-child follow-up steer, `subagent-executor.ts:860`) to a 500ms default timeout
                // race — the caller's own turn is never blocked longer than that waiting on a
                // delivery ack (`result-intercom.ts:325-358`). Race the raw `SteerChannel::steer`
                // call against that same bound rather than awaiting it unbounded.
                let delivered = match &child_target {
                    Some(target) => {
                        crate::tui::intercom::steer_with_default_timeout(
                            self.steer.as_ref(),
                            target.clone(),
                            follow_up_message,
                        )
                        .await
                    }
                    None => false,
                };
                if delivered {
                    // pi's delivered-follow-up confirmation (`subagent-executor.ts:868-871`).
                    Ok(format!(
                        "Interrupted live async child, then delivered follow-up.\n\
                         Run: {run_id}\n\
                         Intercom target: {}",
                        child_target.unwrap_or_default()
                    ))
                } else {
                    // Delivery-FAILED fallback ONLY (no live broker, or no registered receiver at the
                    // target) — pi's exact intercom-unregistered guidance
                    // (`subagent-executor.ts:873-877`).
                    let target_line = child_target
                        .map(|t| format!("Intercom target: {t}\n"))
                        .unwrap_or_default();
                    Err(format!(
                        "Async child appears live but its intercom target is not registered.\n\
                         Run: {run_id}\n\
                         {target_line}Wait for completion, then retry action='resume'."
                    ))
                }
            }
            Ok(ResumeOutcome::RespawnFromTranscript { step_index, session_file }) => self
                .revive_from_transcript(cwd, run_id, step_index, &session_file, follow_up)
                .await
                .map_err(|e| e.to_string()),
            Err(SubagentError::ResumeNoTranscript) => Err(format!(
                "Resume unavailable: async run '{run_id}' has no persisted transcript to revive \
                 from."
            )),
            // G77 — pi `async-resume.ts:406` @v0.43.0 throws with this exact sentence, and the
            // thrown message is what the caller surfaces. `SubagentError::ResumeStopped`'s own
            // `Display` IS that sentence, so this arm exists to pin it: a stopped run must never
            // fall into the no-transcript wording above (its children usually DO have transcripts)
            // and must never be silently revived.
            Err(e @ SubagentError::ResumeStopped(_)) => Err(e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// The terminal-revival spawn half of [`Self::control_resume`] (R-SA-085): read the source run's
    /// reconciled status to recover the revived step's agent, resolve its REAL persona (never a
    /// placeholder, C13), and spawn a fresh detached background single-run seeded from
    /// `session_file` (`executeAsyncSingle` in pi, `subagent-executor.ts:987`).
    async fn revive_from_transcript(
        &self,
        cwd: &Path,
        source_run_id: &str,
        step_index: usize,
        session_file: &Path,
        follow_up: &str,
    ) -> Result<String, SubagentError> {
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        let source_paths = RunPaths::for_run(
            &async_root,
            &results_dir,
            &RunId::from_token(source_run_id.to_string()),
        );
        let status = control::reconcile_before_control_op(&source_paths).await?;
        let agent = status
            .steps
            .get(step_index)
            .map(|step| step.agent.clone())
            .ok_or_else(|| {
                SubagentError::AgentNotFound(format!("no step at index {step_index} to revive"))
            })?;
        // pi `effectiveCwd = target.cwd ?? requestCwd` (`subagent-executor.ts:890`, fed by
        // `target.cwd` = `status.cwd ?? result.cwd`, `background/async-resume.ts:373`): the revived
        // child's persona discovery AND its actual spawn cwd prefer the ORIGINAL run's own working
        // directory (persisted onto the reconciled status by `finish_run`,
        // `background/runner_main.rs`) over whatever cwd happens to be current at resume time —
        // never silently reroute a revived agent into a different directory than the one it was
        // originally invoked from.
        let effective_cwd = status.cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
        let resolved_agents =
            self.resolve_plan_personas(&effective_cwd, [agent.clone()], AgentReadScope::Both)?;
        let revived_task =
            Self::build_revived_async_task(source_run_id, &agent, session_file, follow_up);
        let step = SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.clone(),
            task: revived_task,
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: Some(session_file.to_path_buf()),
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: Some(ContextMode::Fork),
            agent_scope: None,
        };
        let new_id = self
            .spawn_background_steps(
                &effective_cwd,
                BackgroundStepsSpec {
                    // SUBA-021: unbudgeted on this path (see the field doc).
                    usage_budget: None,
                    turn_budget: None,
                    steps: vec![RunnerStep::SingleStep(step)],
                    mode: RunMode::Single,
                    session_file: Some(session_file.to_path_buf()),
                    resolved_agents,
                    // The revival's follow-up is its `{task}`; a single revived run has no chain dir.
                    original_task: follow_up.to_string(),
                    chain_dir: None,
                    // SUBA-N05: a revival carries no per-call `control` object of its own — pi's
                    // revive path likewise resolves `resolveControlConfig(deps.config.control,
                    // input.params.control)` where `params` is the ACTION's params
                    // (`subagent-executor.ts:1179` @v0.34.0), and cyrup's `resume` action exposes no
                    // `control` field. The extension-level `subagents.control` block still applies.
                    control: Some(crate::exec::control::resolve_control_config(
                        self.config_snapshot().await.control.as_ref(),
                        None,
                    )),
                    // SUBA-N06: a revival exposes no `includeProgress` param either, for the same
                    // reason — the `resume` action's params carry no such field.
                    include_progress: None,
                    // SUBA-N03: the `resume` action's params carry none of the SINGLE-mode
                    // overrides either — a revival re-runs an EXISTING run's persona against a
                    // follow-up, and pi's revive path likewise forwards no `output`/`skill`/
                    // `share`/`sessionDir`/`artifacts`/`timeoutMs`. The revived run's own session
                    // file (above) is what keeps its transcript continuous.
                    run_id: RunId::new(),
                    timeout_ms: None,
                    share: None,
                    artifacts_dir: None,
                    artifact_config: crate::artifacts::ArtifactConfig::default(),
                },
            )
            .await?;
        // pi's confirmation (`subagent-executor.ts:1019-1029`): a source label ("foreground" /
        // "async" / "nested" — cyrub's `control::resume` only ever revives an async source today, so
        // this is always "async" here), then the intercom-target line ONLY when a real bridge is
        // wired (pi `intercomBridge.active`), matching `NoTransportSteerChannel::is_active` ==
        // `false` degrading to omitting the line entirely rather than showing a target nothing will
        // ever deliver to.
        let intercom_target_line = if self.steer.is_active() {
            let target = crate::spawn::intercom_target::resolve_subagent_intercom_target(
                new_id.as_str(),
                &agent,
                0,
            );
            format!("Intercom target: {target} (if registered)\n")
        } else {
            String::new()
        };
        Ok(format!(
            "Revived async subagent from {source_run_id}.\n\
             Revived run: {new_id}\n\
             Agent: {agent}\n\
             Session: {}\n\
             {intercom_target_line}Status if needed: subagent({{ action: \"status\", id: \"{new_id}\" }})",
            session_file.display()
        ))
    }

    /// pi `buildRevivedAsyncTask` (`background/async-resume.ts:526-539`): the revival framing wrapped
    /// AROUND the orchestrator's raw follow-up, rather than sending the follow-up verbatim as the
    /// revived child's `{task}` — the revived agent otherwise has no way to know it is being resumed
    /// from a stored transcript rather than starting fresh.
    fn build_revived_async_task(
        source_run_id: &str,
        agent: &str,
        session_file: &Path,
        follow_up: &str,
    ) -> String {
        let lines: Vec<String> = vec![
            "You are reviving a previous subagent conversation.".to_string(),
            String::new(),
            format!("Original run: {source_run_id}"),
            format!("Original agent: {agent}"),
            format!("Original session file: {}", session_file.display()),
            String::new(),
            "Use the stored session context as background. Answer the orchestrator's follow-up \
             below. Do not assume the original child process is still alive."
                .to_string(),
            String::new(),
            "Follow-up:".to_string(),
            follow_up.to_string(),
        ];
        lines.join("\n")
    }

    /// `action: "append-step"` (C5): validate and enqueue exactly one new step onto a running async
    /// chain (R-SA-094/095/096) — pi `subagent-executor.ts:2868`/`508-686`. The appended agent is
    /// resolved through real discovery first (fail-fast on an unknown agent, matching pi's
    /// `buildAsyncRunnerSteps`), then the step is enqueued via [`crate::background::control::append_step`].
    ///
    /// # Errors
    ///
    /// Returns `Err` for a missing id, a chain that is not exactly one step, an unknown agent, or a
    /// primitive-level rejection (wrong mode/state, output-name collision).
    pub async fn control_append_step(
        &self,
        cwd: &Path,
        target: Option<&str>,
        chain: &[serde_json::Value],
    ) -> Result<String, String> {
        let Some(run_id) = target else {
            return Err("action='append-step' requires id.".to_string());
        };
        if chain.len() != 1 {
            return Err("action='append-step' requires chain with exactly one step.".to_string());
        }
        let Some(step_val) = chain.first() else {
            return Err("action='append-step' requires chain with exactly one step.".to_string());
        };
        let Some(agent) = step_val.get("agent").and_then(serde_json::Value::as_str) else {
            return Err("action='append-step' chain step requires an 'agent' field.".to_string());
        };
        let task = step_val
            .get("task")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        let output = step_val
            .get("output")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        // pi validates every appended agent exists before enqueuing (`buildAsyncRunnerSteps` errors
        // on an unknown agent name); resolve it via real discovery for the same fail-fast behavior.
        self.resolve_agent(cwd, agent, AgentReadScope::Both)
            .map_err(|e| format!("Cannot append step to run '{run_id}': {e}"))?;
        let step = SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task,
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        };
        let async_root = default_async_root(cwd);
        let results_dir = default_results_dir(cwd);
        match control::append_step(
            &async_root,
            &results_dir,
            run_id,
            vec![RunnerStep::SingleStep(step)],
        )
        .await
        {
            Ok(AppendOutcome::Enqueued { .. }) => {
                let paths = RunPaths::for_run(
                    &async_root,
                    &results_dir,
                    &RunId::from_token(run_id.to_string()),
                );
                let pending = control::count_pending_appends(&paths.append_dir)
                    .await
                    .unwrap_or(1);
                Ok(format!(
                    "Append queued for chain run {run_id}: 1 step. It becomes eligible after the \
                     chain's already-queued steps finish. Pending appends: {pending}."
                ))
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    #[tokio::test]
    async fn control_interrupt_with_no_run_reports_none_capable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_interrupt(dir.path(), None)
            .await
            .expect_err("no runs -> no interrupt-capable run");
        assert_eq!(err, "No interrupt-capable run found in this session.");
    }

    #[tokio::test]
    async fn control_resume_requires_a_message_then_an_id() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // Empty follow-up is rejected before anything else (pi `resume` requires `message`).
        let no_msg = executor
            .control_resume(dir.path(), Some("run00000000"), None, None, None)
            .await
            .expect_err("resume requires a message");
        assert_eq!(no_msg, "action='resume' requires message.");
        // With a message but no id, resume requires an id selector.
        let no_id = executor
            .control_resume(dir.path(), None, Some("carry on"), None, None)
            .await
            .expect_err("resume requires an id");
        assert_eq!(no_id, "action='resume' requires id.");
    }

    /// pi `buildRevivedAsyncTask` (`background/async-resume.ts:526-539`): a revived child's `{task}`
    /// must be the follow-up WRAPPED in the revival framing (source run/agent/session-file context
    /// plus an explicit "you are reviving..." preamble), never the orchestrator's raw follow-up text
    /// verbatim — the revived agent otherwise has no way to know it is resuming from a stored
    /// transcript rather than starting fresh.
    #[test]
    fn build_revived_async_task_wraps_the_follow_up_in_pi_s_revival_framing() {
        let task = SubagentExecutor::build_revived_async_task(
            "run00099",
            "researcher",
            Path::new("/tmp/session-abc.jsonl"),
            "please continue",
        );
        assert_eq!(
            task,
            "You are reviving a previous subagent conversation.\n\
             \n\
             Original run: run00099\n\
             Original agent: researcher\n\
             Original session file: /tmp/session-abc.jsonl\n\
             \n\
             Use the stored session context as background. Answer the orchestrator's follow-up \
             below. Do not assume the original child process is still alive.\n\
             \n\
             Follow-up:\n\
             please continue"
        );
        assert_ne!(
            task, "please continue",
            "the revived task must NOT be the raw follow-up passed through verbatim"
        );
    }

    #[tokio::test]
    async fn control_append_step_validates_shape_before_touching_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        // Missing id.
        let no_id = executor
            .control_append_step(dir.path(), None, &[])
            .await
            .expect_err("append-step requires id");
        assert_eq!(no_id, "action='append-step' requires id.");
        // Wrong-cardinality chain (must be exactly one step).
        let bad_chain = executor
            .control_append_step(dir.path(), Some("run00000000"), &[])
            .await
            .expect_err("append-step requires exactly one chain step");
        assert_eq!(
            bad_chain,
            "action='append-step' requires chain with exactly one step."
        );
    }

}
