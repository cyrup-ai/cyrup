//! `action: "stop"` (G77) — deliver a TERMINAL, non-resumable stop to a background run, and its
//! no-UI `/subagents-stop` fallback listing. Ports `runs/foreground/async-stop-action.ts`
//! (`stopAsyncRun`) and `slash/slash-commands.ts`'s `stopFallbackText`.
//!
//! Moved out of `extension::executor::control` verbatim (WORKFLOW_8 SUBTASK1) — see
//! [`super`] for the scope note shared by every verb in this directory: `foreground_actions`
//! names pi's SOURCE directory (`src/runs/foreground/`), not a claim that the runs acted on are
//! themselves "foreground" — every verb here (`stop`, `steer`, `dismiss`) acts on an **async**
//! run, and `background::control`'s primitives (this file's own `control::stop`, reconciliation,
//! the on-disk request shapes) stay in `background/control.rs` untouched.

use std::path::Path;

use crate::background::control;
use crate::background::run_status;
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::tool::text::{
    STOP_FOREGROUND_RUN_REFUSAL, STOP_NESTED_RUN_REFUSAL, STOP_NO_STOPPABLE_RUN_REFUSAL,
};

impl SubagentExecutor {
    /// G77 — `action: "stop"` (pi `stopAsyncRun`, `runs/foreground/async-stop-action.ts:23-64`
    /// @v0.43.0, dispatched at `subagent-executor.ts:4771-4815`): deliver a TERMINAL,
    /// non-resumable stop to a background run.
    ///
    /// This is deliberately NOT [`Self::control_interrupt`]. An interrupt is a soft, resumable
    /// pause — the run ends [`crate::background::RunState::Paused`] and `action: "resume"` is expected to pick it back
    /// up. A stop is terminal: the run ends [`crate::background::RunState::Stopped`], every unfinished step ends
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
    /// SUBA-087 — `childId` (pi `async-stop-action.ts:48-66,68,75` @v0.64.0, threaded from
    /// `subagent-executor.ts:6163,6184`): when given, the child is resolved against the reconciled
    /// status ([`crate::background::child_identity::resolve_async_status_child`]) and gated on
    /// pending/running BEFORE anything is written — a failed resolution answers with the
    /// resolver's own sentence, a non-stoppable child with `Child '{childId}' in async run
    /// '{runId}' is {status}; stop only supports pending or running children.`, and success with
    /// `Stop requested for child {child.id} in async run {id}.` naming the RESOLVED identity. The
    /// written request then carries `targetIndex`/`childId`, and the runner stops that one step
    /// while the run stays alive. The `workflowControllers` child-stop branch
    /// (`subagent-executor.ts:6122-6155`) is the unported workflow subsystem, as below.
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
        child_id: Option<&str>,
    ) -> Result<String, String> {
        if target.is_none() && dir.is_none() {
            return Err("action='stop' requires id or dir.".to_string());
        }
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);

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
            if self.resolves_to_nested_run(id).await {
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

        let current_session =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        match control::stop(
            &async_root,
            &results_dir,
            &run_id,
            "stop-action",
            None,
            child_id,
            current_session.as_ref(),
        )
        .await
        {
            Ok(control::StopOutcome::Requested) => {
                Ok(format!("Stop requested for async run {run_id}."))
            }
            // SUBA-087 — `async-stop-action.ts:75`: the receipt names the RESOLVED child id.
            Ok(control::StopOutcome::ChildRequested { child_id }) => Ok(format!(
                "Stop requested for child {child_id} in async run {run_id}."
            )),
            Ok(control::StopOutcome::NotStoppable) => Err(format!(
                "No running or queued async run was found for '{run_id}'."
            )),
            // S4 (pi `async-stop-action.ts:34`) — upstream's exact refusal sentence. Distinct from
            // `NotStoppable`: the run may be perfectly stoppable, just not by this instance.
            Ok(control::StopOutcome::NotInActiveSession) => Err(format!(
                "Async run '{run_id}' was not found in the active session."
            )),
            // SUBA-087 — `async-stop-action.ts:51-57`: the resolver's own not-found/ambiguous
            // sentence, verbatim.
            Ok(control::StopOutcome::ChildUnresolved(message)) => Err(message),
            // SUBA-087 — `async-stop-action.ts:59-65`: the caller's own spelling of the id and
            // the status record's run id, with pi's lowercase step-status word.
            Ok(control::StopOutcome::ChildNotStoppable {
                run_id: status_run_id,
                state,
            }) => Err(format!(
                "Child '{}' in async run '{status_run_id}' is {}; stop only supports pending or \
                 running children.",
                child_id.unwrap_or_default(),
                run_status::step_state_label(state)
            )),
            Err(e) => Err(format!("Failed to stop async run {run_id}: {e}")),
        }
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
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        let runs = run_status::list_active_runs(
            &async_root,
            &results_dir,
            self.current_session_id().as_deref(),
        )
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

    use crate::background::control;
    use crate::extension::executor::SubagentExecutor;
    use crate::extension::testsupport::{FixedSessionHost, seed_orphaned_run};

    /// `stop` writes a real request file under `control/stop-requests/` for a `Running` run, and
    /// `consume_stop_request` reads-then-deletes it exactly once (pi `requestAsyncStop` /
    /// `consumeStopRequestPayload`, `runs/background/control-channel.ts:297-310,615-620` @v0.64.0).
    /// S4 — the gate pi applies at `async-stop-action.ts:34`, which cyrup's port of that same
    /// function (`control.rs`'s `:24-86` range claim) previously omitted while porting `:41`,
    /// `:47`, `:48-68` and `:75`.
    ///
    /// Pinned at the ACTION boundary (`control_stop`, not the `control::stop` primitive): the
    /// primitive's `StopOutcome::NotInActiveSession` discriminant is already pinned where it is
    /// defined, but the user-visible SENTENCE is only observable here.
    ///
    /// `async_root` is per-cwd, so every concurrent instance can address every other instance's
    /// runs by id — and every listing hands those ids out.
    #[tokio::test]
    async fn control_stop_refuses_a_run_owned_by_another_session_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = seed_orphaned_run(
            dir.path(),
            "foreignstop1",
            Some("session-OWNER"),
            Some(std::process::id()),
        );

        let intruder = SubagentExecutor::new();
        intruder.set_host_services(Arc::new(FixedSessionHost("session-INTRUDER")));
        let err = intruder
            .control_stop(dir.path(), Some("foreignstop1"), None, None)
            .await
            .expect_err("a foreign session must be refused");
        assert_eq!(
            err, "Async run 'foreignstop1' was not found in the active session.",
            "pi `async-stop-action.ts:34`'s exact sentence"
        );
        assert!(
            !control::has_pending_stop_request(&paths.run_dir).await,
            "a refused stop must leave ZERO filesystem trace in another session's run directory"
        );

        // ...and the owner can still stop it.
        let owner = SubagentExecutor::new();
        owner.set_host_services(Arc::new(FixedSessionHost("session-OWNER")));
        let ok = owner
            .control_stop(dir.path(), Some("foreignstop1"), None, None)
            .await
            .expect("the owning session may stop its own run");
        assert_eq!(ok, "Stop requested for async run foreignstop1.");
        assert!(control::has_pending_stop_request(&paths.run_dir).await);
    }

    /// The PERMISSIVE half of the class (pi `state.currentSessionId && ...`): a headless host has
    /// no session identity and must still be able to control its own runs. A strict-ified gate
    /// would lock SDK embedders out entirely.
    #[tokio::test]
    async fn control_stop_still_works_for_a_host_with_no_session_identity() {
        let dir = tempfile::tempdir().expect("tempdir");
        seed_orphaned_run(
            dir.path(),
            "headlessstop",
            Some("session-ANY"),
            Some(std::process::id()),
        );

        let executor = SubagentExecutor::new();
        let ok = executor
            .control_stop(dir.path(), Some("headlessstop"), None, None)
            .await
            .expect("a host with no session must not be locked out of control");
        assert_eq!(
            ok, "Stop requested for async run headlessstop.",
            "a host with no session must not be locked out of control"
        );
    }
}
