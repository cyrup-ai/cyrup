//! SUBA-057 — `action: "dismiss"` (pi `dismissRecoveredWorkflow`,
//! `runs/foreground/async-dismiss-action.ts` @v0.47.1). Moved out of
//! `extension::executor::control` verbatim, plus the WORKFLOW_6 registry wiring (WORKFLOW_8
//! SUBTASK3) — see [`super`] for the scope note shared by every verb in this directory.
//!
//! `dismiss` is the one verb of the three that is STRICT rather than PERMISSIVE
//! ([`crate::background::delivery::SessionGate`]): a host with no session identity at all may not
//! dismiss anything, because dismissal asserts ownership rather than merely controlling a run this
//! host could already reach.

use std::path::Path;

use crate::background::atomic::write_atomic_json;
use crate::background::control;
use crate::background::run_status;
use crate::background::{RunId, RunPaths, RunState};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::tool::text::dismiss_not_running_refusal;
use crate::identity::SessionId;

impl SubagentExecutor {
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
    /// [`crate::background::RunMode`] (`background/mod.rs:242`) has three variants — `Single`/`Parallel`/`Chain` —
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
    pub async fn control_dismiss(
        &self,
        cwd: &Path,
        target: Option<&str>,
    ) -> Result<String, String> {
        // pi `:5873`: `paramsWithResolvedCwd.runId ?? paramsWithResolvedCwd.id` is resolved by the
        // caller; a missing selector is its own sentence, ahead of everything else.
        let Some(target) = target.filter(|id| !id.trim().is_empty()) else {
            return Err("action='dismiss' requires id.".to_string());
        };
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);

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
        if current_session.is_none()
            || status.session_id.as_ref().map(SessionId::as_str) != current_session.as_deref()
        {
            return Err(format!(
                "Recovered workflow '{run_id_text}' was not found in the active session."
            ));
        }

        // pi `:37-42` — BOTH lookups. `status.run_id` is canonical; `target` is the caller's own
        // spelling, which may be a unique prefix (`resolve_run_id` accepts those), and a controller
        // registered under either must block the dismiss.
        //
        // WORKFLOW_6 (`workflow_controllers.rs`) is the registry upstream means; before it landed
        // this guard was the pid probe below alone, which is why that probe's `[CYRUP-DELTA]` above
        // is kept rather than deleted: cyrup drives BACKGROUND runs from a detached process, so "a
        // live controller" has two genuine carriers here and upstream only has one. Registry first
        // (in-process, free), pid second (the detached-runner shape), one shared sentence.
        let live_controller = self.has_workflow_controller(&status.run_id)
            || self.has_workflow_controller(&RunId::from_token(target.to_string()))
            || status.pid.is_some_and(|pid| {
                crate::background::reconcile::check_pid_liveness(pid).is_possibly_alive()
            });
        if live_controller {
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
        // Resolved through the index: a promoted payload lives under `result-owned/`, so probing
        // the legacy root alone would miss every result this build writes and skip the
        // re-reconciliation upstream performs precisely when one exists.
        let has_terminal_result = match latest.session_id.as_ref() {
            Some(session_id) => paths
                .resolve_result(session_id, &latest.run_id)
                .await
                .is_some(),
            None => tokio::fs::try_exists(&paths.legacy_result_root)
                .await
                .unwrap_or(false),
        };
        if has_terminal_result {
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
        latest.display_dismissed_at = Some(crate::time::now_epoch_millis());
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
        //
        // pi `:76-77`. `JobTracker::untrack` is a `HashMap::remove` (`tracker.rs:297`) — removing
        // an absent key is a no-op, exactly like `Map.delete`, so the second call is free when the
        // two spellings coincide.
        self.tracker.untrack(&run_id);
        self.tracker.untrack(&RunId::from_token(target.to_string()));

        Ok(format!(
            "Dismissed recovered workflow {run_id_text} from the display. No running work was \
             terminated."
        ))
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

    use std::sync::Arc;

    use cyrup_core::CancelToken;

    use crate::background::RunId;
    use crate::extension::executor::SubagentExecutor;
    use crate::extension::testsupport::{FixedSessionHost, seed_orphaned_run};

    /// pi `:37-42`'s registry half (WORKFLOW_6 lands the registry; WORKFLOW_8 wires it in here): a
    /// controller registered under the CALLER's raw spelling — not the resolved canonical run id —
    /// still blocks the dismiss. `status.run_id` (canonical) is registered under nothing here, so
    /// only the SECOND lookup (`target`, the caller's own spelling) can catch this, which is what
    /// makes this the one fixture that actually exercises "either spelling" rather than the two
    /// lookups vacuously agreeing.
    #[tokio::test]
    async fn dismiss_refuses_a_controller_registered_under_the_callers_raw_spelling() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionHost("session-a")));
        // The only run under this async root, so "run0regp" is a unique resolvable prefix of it.
        seed_orphaned_run(dir.path(), "run0regprefix1", Some("session-a"), None);

        executor.register_workflow_controller(&RunId::from_token("run0regp"), CancelToken::new());

        let err = executor
            .control_dismiss(dir.path(), Some("run0regp"))
            .await
            .expect_err(
                "a controller registered under the caller's own spelling must still block dismiss",
            );
        assert_eq!(
            err, "Workflow 'run0regprefix1' still has a live controller and cannot be dismissed.",
            "the resolved CANONICAL id names the run in the refusal, even though the registry hit \
             came from the caller's own (prefix) spelling"
        );
    }

    /// pi `:32` STRICT (`!state.currentSessionId || …`): a host with NO session identity at all is
    /// refused outright — even though the run itself here ALSO carries no session, so a
    /// PERMISSIVE gate would have admitted this exact caller. This is the one arm where `dismiss`
    /// diverges from `stop`/`steer`'s PERMISSIVE class (`SessionGate::Strict` vs. `::Permissive`,
    /// `gate.rs`'s own doc: "a strict-ified `stop` breaks headless and SDK embedders... a
    /// permissive-ised `dismiss` lets a host with no identity dismiss anything in the directory").
    #[tokio::test]
    async fn dismiss_refuses_a_host_with_no_session_identity_at_all() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new(); // no host services bound at all
        seed_orphaned_run(dir.path(), "run0nohostsess", None, None);

        let err = executor
            .control_dismiss(dir.path(), Some("run0nohostsess"))
            .await
            .expect_err("a headless caller must be refused, unlike stop/steer's PERMISSIVE class");
        assert_eq!(
            err,
            "Recovered workflow 'run0nohostsess' was not found in the active session."
        );
    }
}
