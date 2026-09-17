//! SUBA-016 part B — the executor half of scheduled runs: the launch seam, the spawn-budget gate,
//! and the manager's lifecycle slot.
//!
//! # Why any of this lives up here
//!
//! `background/` sits BELOW `extension/executor/`, exactly as
//! [`crate::extension::executor::wait_subscriptions`]'s own header says. A fired schedule needs
//! three things `background/scheduled_runs/` cannot name: the executor's late-bound P-1
//! `HostServices` slot (for the session identity it pins), the per-session spawn budget, and the
//! headless workflow launch path. So the manager takes them as an injected
//! [`ScheduleLauncher`], and this module is the one implementation.
//!
//! # A fired schedule's result really reaches the session
//!
//! A foreground workflow delivers itself as the [`cyrup_core::ToolResult`] it returns. A scheduled
//! one has no caller to return to, so — exactly as `workflow_detach/mod.rs:44-52` puts it —
//! **writing the result file IS the emit**: [`publish_scheduled_result`] writes a terminal
//! [`crate::background::ResultFile`] through the session-partitioned index, the results watcher
//! observes it, and the completion is admitted only for the session stamped on it. Without that
//! write a schedule would be a job that ran where nobody could see it.

use std::path::Path;
use std::sync::{Arc, PoisonError, Weak};

use crate::background::scheduled_runs::{
    ScheduleLaunchOutcome, ScheduleLaunchRequest, ScheduleLauncher, ScheduleSessionSnapshot,
    ScheduleStore, ScheduledRunDeps, ScheduledRunManager, ScheduledRunSlot,
    scheduled_run_store_path,
};
use crate::extension::executor::SubagentExecutor;
use crate::identity::SessionId;

/// The production [`ScheduleLauncher`] — a real `RunMode::Workflow` run in THIS process.
///
/// A [`Weak`] handle on the executor, for the reason `wait_subscriptions.rs`'s
/// `ExecutorSubscriptionSessions` gives verbatim: the executor owns the manager, the manager owns
/// this, so a strong handle is a cycle. A dropped executor reads as "cannot launch", which is the
/// correct degradation — the fire records `FailedLaunch` and the schedule survives.
struct ExecutorScheduleLauncher {
    executor: Weak<SubagentExecutor>,
    /// `SubagentExtensionConfig::max_subagent_spawns_per_session`, resolved at install.
    ///
    /// Snapshotted with the rest of the session's config, like every other consumer: a fire bills
    /// against the cap this session was installed under, not one that changed on disk mid-run.
    /// Runtime GRANTS are unaffected — they live on the budget counters, not on this value, so
    /// `grant-spawn-budget` still raises the effective cap.
    max_spawns: u32,
}

#[async_trait::async_trait]
impl ScheduleLauncher for ExecutorScheduleLauncher {
    fn reserve_spawn_slot(&self) -> Result<(), String> {
        let Some(executor) = Weak::upgrade(&self.executor) else {
            return Err("The subagent executor is no longer available.".to_string());
        };
        executor.reserve_subagent_spawns(1, self.max_spawns)
    }

    async fn launch(
        &self,
        request: ScheduleLaunchRequest<'_>,
    ) -> Result<ScheduleLaunchOutcome, String> {
        let Some(executor) = Weak::upgrade(&self.executor) else {
            return Err("The subagent executor is no longer available.".to_string());
        };
        let cfg = executor.config_snapshot().await;
        let cwd = request.schedule.cwd.clone();
        let script = request.schedule.target.workflow_script.clone();
        let timeout_ms = request
            .schedule
            .timeout_ms
            .and_then(|ms| u64::try_from(ms).ok());
        let session_id = request.session.session_id().cloned();
        // pi `executionParams`' `scheduleOrigin: { id, name?, quiet? }` (`scheduled-runs.ts:461`).
        // Built HERE because this is the only place that knows both the schedule and the run.
        let origin = crate::background::ScheduleOrigin {
            id: request.schedule.id.as_str().to_string(),
            name: (!request.schedule.name.is_empty()).then(|| request.schedule.name.clone()),
            quiet: request.quiet.then_some(true),
        };

        let prepared = crate::extension::executor::workflow_launch::prepare_workflow_run(
            crate::extension::executor::workflow_launch::PrepareWorkflowRun {
                cfg: &cfg,
                cwd: &cwd,
                // §SUBTASK2 — the PINNED identity, never a live read. A fire that called
                // `current_session_id()` here, again for the `sessionOnly` gate and a third time
                // for the completion owner could observe three different identities across one
                // launch, on a session switch.
                session_id: session_id.clone(),
                // There is no tool call. `with_workflow_children` falls back to the run id.
                tool_call_id: None,
                // A scheduled fire has no caller to return a `ToolResult` to, so its result
                // reaches the session through the completion pipeline — which is gated on this
                // pair together with `session_id`.
                completion_owner_id: Some(crate::identity::current_completion_owner_id()),
            },
        )
        .await
        .map_err(|error| error.to_string())?;

        let run_id = prepared.run_id().clone();
        let run_dir = prepared.run_dir().to_path_buf();
        // `register_workflow_controller` (inside `drive_workflow_run`) keys this token by run id,
        // which is what makes a fired run reachable by `action: "interrupt"` like any other.
        let cancel = cyrup_core::CancelToken::new();
        let cfg_for_task = cfg.clone();
        let executor_for_task = Arc::clone(&executor);
        let run_id_for_task = run_id.clone();
        let completion = Box::pin(async move {
            let outcome = crate::extension::executor::workflow_launch::drive_workflow_run(
                &executor_for_task,
                prepared,
                crate::extension::executor::workflow_launch::DriveWorkflowRun {
                    cfg: &cfg_for_task,
                    cwd: &cwd,
                    script: &script,
                    timeout_ms,
                    // Upstream's `executionParams` sets `mission: false` (`:459`): a schedule binds
                    // no mission, so there is no durable scratchpad and `globalThis.state` is
                    // never installed in the guest realm.
                    state: None,
                    // No operator is watching a scheduled fire, and `WorkflowRunHost::new`
                    // requires a sink. `ToolUpdateSink` is
                    // `Box<dyn FnMut(ToolUpdate) + Send + 'static>`, so this is legal and free.
                    on_update: Box::new(|_| {}),
                    cancel,
                },
            )
            .await;
            // The workflow's ANSWER — the text `drive_workflow_run` composed for the tool result
            // nobody is here to receive. Carried onto the published result so the completion
            // notice shows what the run produced instead of `(no output)`.
            let summary = match &outcome {
                Ok(result) => first_text(&result.content)
                    .unwrap_or_else(|| "Scheduled workflow completed.".to_string()),
                Err(error) => error.to_string(),
            };
            publish_scheduled_result(ScheduledResultPublication {
                cfg: &cfg_for_task,
                cwd: &cwd,
                run_id: &run_id_for_task,
                session_id: session_id.as_ref(),
                origin: &origin,
                success: outcome.is_ok(),
                summary: &summary,
            })
            .await;
            outcome.map(|_| ()).map_err(|error| error.to_string())
        });

        Ok(ScheduleLaunchOutcome {
            // The run's own id, which is also its directory name — so `schedule.delete`'s guard
            // (which opens `async_dir/status.json` and cross-checks `runId == asyncId`) cannot be
            // made vacuous by the two disagreeing.
            async_id: run_id.as_str().to_string(),
            async_dir: run_dir,
            completion: Some(completion),
        })
    }
}

/// The first text block of a tool result, if it has one.
fn first_text(content: &[cyrup_core::Content]) -> Option<String> {
    content.iter().find_map(|part| match part {
        cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
        _ => None,
    })
}

/// The addressing and metadata one scheduled result write needs.
struct ScheduledResultPublication<'a> {
    cfg: &'a crate::registration::SubagentExtensionConfig,
    cwd: &'a Path,
    run_id: &'a crate::background::RunId,
    session_id: Option<&'a SessionId>,
    origin: &'a crate::background::ScheduleOrigin,
    success: bool,
    summary: &'a str,
}

/// Write the fired run's terminal [`crate::background::ResultFile`], so the completion pipeline
/// delivers it to the session that owns it — and to no other.
///
/// Best effort, and logged rather than propagated: the run's `status.json` and receipt are already
/// durable, so a result that could not be indexed is a missed NOTIFICATION, not a lost run. That
/// is the same trade `runner_main/finish.rs` makes for its own index write.
async fn publish_scheduled_result(request: ScheduledResultPublication<'_>) {
    let Some(session_id) = request.session_id else {
        // `write_async_result_file` refuses an unattributable result, and it is right to: an
        // unindexed result is invisible to every reader and would only ever be swept by age.
        tracing::debug!(
            run_id = %request.run_id,
            "a scheduled run finished with no pinned session; its result is not published"
        );
        return;
    };
    let async_root =
        crate::extension::executor::paths::default_async_root_in(&request.cfg.roots, request.cwd);
    let results_dir =
        crate::extension::executor::paths::default_results_dir_in(&request.cfg.roots, request.cwd);
    let run_dir = crate::identity::RunDirName::for_run(request.run_id).resolve_in(&async_root);

    // ONE `SingleResult`, carrying the workflow's answer, through the crate's existing
    // placeholder constructor rather than a second forty-field literal — see
    // `background::reconcile::placeholder_result`'s own doc for why it is `pub(crate)`.
    let label = request
        .origin
        .name
        .as_deref()
        .unwrap_or(request.origin.id.as_str());
    let mut child = crate::background::reconcile::placeholder_result(
        label,
        crate::background::RunMode::Workflow,
        request.summary,
    );
    child.exit_code = i32::from(!request.success);
    child.output_state = crate::exec::output_state::SubagentOutputState::Present;
    if request.success {
        child.final_output = Some(request.summary.to_string());
        child.error = None;
    }

    let payload = crate::background::ResultFile {
        id: request.run_id.clone(),
        run_id: request.run_id.clone(),
        agent: label.to_string(),
        mode: crate::background::RunMode::Workflow,
        state: if request.success {
            crate::background::RunState::Complete
        } else {
            crate::background::RunState::Failed
        },
        success: request.success,
        cwd: request.cwd.to_path_buf(),
        session_file: None,
        session_id: Some(session_id.clone()),
        completion_owner_id: Some(crate::identity::current_completion_owner_id()),
        results: vec![child],
        workflow_children: None,
        workflow_receipt: None,
        // THE point of the field: this is what makes the completion name its schedule, what makes
        // a SUCCESSFUL scheduled run displayed at all (`completion_notice_display`), and what
        // carries `quiet` to `scheduled_completion_triggers_turn`.
        schedule_origin: Some(request.origin.clone()),
    };
    if let Err(error) = crate::background::result_index::write_async_result_file(
        &crate::background::result_index::ResultWrite {
            results_dir: &results_dir,
            session_id,
            run_id: request.run_id,
            written_at: crate::time::now_epoch_millis(),
            async_dir: Some(&run_dir),
            tool_call_id: None,
        },
        &payload,
    )
    .await
    {
        tracing::warn!(
            run_id = %request.run_id,
            %error,
            "a scheduled run finished but its result could not be published; the run's status and \
             receipt are already durable on disk"
        );
    }
}

impl SubagentExecutor {
    /// The manager bound to this session, if scheduled runs are installed.
    ///
    /// Resolved LATE, on every dispatch: the tool arm must reach whichever manager is current now,
    /// not whichever one existed when the tool was built.
    #[must_use]
    pub fn scheduled_runs(&self) -> Option<Arc<ScheduledRunManager>> {
        self.scheduled_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// pi `scheduledRunManager.bindSession(ctx)` + `restore()` (`scheduled-runs.ts:535-537`,
    /// `:743`), collapsed onto cyrup's one `SessionStart` edge.
    ///
    /// Re-entrant across sessions: an existing manager is DISPOSED and replaced, so the old tick
    /// cannot keep firing against a directory — or an identity — this session no longer uses.
    ///
    /// The [`ScheduleSessionSnapshot`] is taken HERE, once, and is what every gate below reads.
    /// That is §SUBTASK2's whole point: [`SubagentExecutor::current_session_id`] reads live off
    /// the P-1 backend on every call, so a fire that consulted it three times could split across
    /// a session switch.
    pub async fn install_scheduled_runs(
        self: &Arc<Self>,
        cwd: &Path,
    ) -> Option<Arc<ScheduledRunManager>> {
        let cfg = self.config_snapshot().await;
        if !cfg.scheduled_runs_enabled() {
            self.dispose_scheduled_runs();
            return None;
        }
        let session = ScheduleSessionSnapshot::new(
            SessionId::parse_opt(self.current_session_id().as_deref()),
            self.host_services()
                .and_then(|services| services.session_file())
                .as_deref(),
        );
        let store_root = cfg.scheduled_runs_store_root();
        let root = scheduled_run_store_path(cwd, session.session_id(), store_root.as_deref());
        // `project_cwd` is `None` exactly when a `storeRoot` is configured — pi `:960`. The root is
        // then a sandbox the operator chose, and `assertScheduleRoot`'s containment checks have
        // no project to contain it within.
        let project_cwd = store_root.is_none().then(|| cwd.to_path_buf());
        let manager = ScheduledRunManager::new(ScheduledRunDeps {
            store: ScheduleStore::new(root, project_cwd),
            fire: crate::background::scheduled_runs::ScheduleFireContext {
                session,
                launcher: Arc::new(ExecutorScheduleLauncher {
                    executor: Arc::downgrade(self),
                    max_spawns: cfg.max_subagent_spawns_per_session,
                }),
            },
            cwd: cwd.to_path_buf(),
            max_pending: cfg.scheduled_runs_max_pending(),
        });
        if let Some(previous) = self
            .scheduled_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(Arc::clone(&manager))
        {
            previous.dispose();
        }
        manager.restore().await;
        Some(manager)
    }

    /// pi `scheduledRunManager.stop()` (`scheduled-runs.ts:539`), on `SessionShutdown`.
    ///
    /// Clears the slot as well as disposing, so a shut-down session cannot create a schedule
    /// through a manager whose tick is already gone.
    pub fn dispose_scheduled_runs(&self) {
        if let Some(manager) = self
            .scheduled_runs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            manager.dispose();
        }
    }
}

/// The slot type, named here so the executor's field declaration reads in this module's terms.
pub(crate) type Slot = ScheduledRunSlot;

impl SubagentExecutor {
    /// The slot itself, for the async-retention stage, which must resolve the manager LATE.
    ///
    /// A handle rather than a snapshot for the reason `AsyncRetentionSchedule`'s own doc gives: a
    /// protected-run set captured at install time protects the runs that were live then and
    /// leaves every run fired since unprotected.
    pub(crate) fn scheduled_run_slot(&self) -> Slot {
        Arc::clone(&self.scheduled_runs)
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

    use std::path::PathBuf;

    use super::*;
    use crate::background::scheduled_runs::{
        ScheduleId, ScheduleRunRecord, ScheduleRunState, ScheduledRunAction,
        ScheduledRunActionParams,
    };
    use crate::extension::testsupport::{FixedSessionHost, arm_scoped_missions};

    /// A scripted child that leaves a MARKER on disk and then settles cleanly.
    ///
    /// This is route (a) from the task brief and it is chosen over a `return 1;` script on
    /// purpose: a workflow body that spawns nothing proves the engine evaluated some JavaScript,
    /// while this one proves the fire reached `exec`'s real spawn path and a separate OS process
    /// really started. The marker is written by that process and by nothing in this test, so it
    /// cannot be faked by an in-memory record.
    ///
    /// The NDJSON is the child protocol's success shape (`exec/ndjson.rs`): `agent_start`, one
    /// assistant `message_end`, `agent_settled`, exit 0 — the mirror of
    /// `routing_tests::write_detaching_child_binary`'s failure shape.
    fn write_marking_child_binary(dir: &Path, marker: &Path) -> PathBuf {
        let script = dir.join("marking-child.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
printf 'the scheduled run reached a real child process\\n' > '{}'\n\
printf '%s\\n' '{{\"type\":\"agent_start\"}}'\n\
printf '%s\\n' '{{\"type\":\"message_end\",\"message\":{{\"role\":\"assistant\",\
\"content\":[{{\"type\":\"text\",\"text\":\"swept\"}}]}}}}'\n\
printf '%s\\n' '{{\"type\":\"agent_settled\"}}'\n\
exit 0\n",
                marker.display()
            ),
        )
        .expect("write the scripted child");
        std::fs::set_permissions(
            &script,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )
        .expect("make the scripted child executable");
        script
    }

    /// Poll the schedule's own history until its run leaves `Running`.
    async fn await_settled(manager: &ScheduledRunManager, id: &ScheduleId) -> ScheduleRunRecord {
        for _ in 0..600 {
            let history = manager.store().history(id).await.expect("history");
            if let Some(run) = history
                .iter()
                .find(|run| run.state != ScheduleRunState::Running)
            {
                return run.clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("a fired run never settled");
    }

    /// Every [`crate::background::ResultFile`] under this cwd's results dir.
    async fn published_results(
        executor: &SubagentExecutor,
        cwd: &Path,
    ) -> Vec<crate::background::ResultFile> {
        let roots = executor.config_snapshot().await.roots.clone();
        let dir = crate::extension::executor::paths::default_results_dir_in(&roots, cwd);
        let mut found = Vec::new();
        let mut stack = vec![dir];
        while let Some(next) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&next) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(bytes) = std::fs::read(&path)
                    && let Ok(result) =
                        serde_json::from_slice::<crate::background::ResultFile>(&bytes)
                {
                    found.push(result);
                }
            }
        }
        found
    }

    /// **THE LAUNCHER'S OWN REACHABILITY PROOF** — a schedule an agent creates really becomes a
    /// running process.
    ///
    /// `background/scheduled_runs/`'s own rows all drive that module's `StubLauncher` and
    /// therefore prove only that the trigger DECIDES to fire; [`ExecutorScheduleLauncher`] itself
    /// had no row in its own file. This one drives it through the production entry points and
    /// nothing else:
    ///
    /// 1. [`SubagentExecutor::install_scheduled_runs`] builds the manager, which is the ONLY
    ///    construction site of [`ExecutorScheduleLauncher`] (no launcher is injected here);
    /// 2. [`ScheduledRunManager::handle_action`] with the real `schedule.create` /
    ///    `schedule.run` verbs;
    /// 3. the fire runs the schedule's `workflowScript` in the real engine, which spawns a real
    ///    child through `exec`'s real spawn path.
    ///
    /// The evidence is all on the FILESYSTEM and all written by production code:
    ///
    /// * a marker file written by the CHILD PROCESS itself — nothing in this test touches it, so
    ///   a launch that returned `Ok` without spawning cannot produce it;
    /// * the fired run's own `status.json`, in a terminal state, `mode: workflow`, attributed to
    ///   the live session;
    /// * the `ResultFile` [`publish_scheduled_result`] indexed, carrying this schedule's
    ///   `scheduleOrigin` — the write that IS the emit for a run with no caller to return to.
    ///
    /// A `multi_thread` runtime because the launch's completion future is `tokio::spawn`ed and
    /// this test blocks on its on-disk effects.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_scheduled_run_really_spawns_a_process_through_the_production_launcher() {
        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = dir.path();
        let marker = cwd.join("the-child-ran");
        let script = write_marking_child_binary(cwd, &marker);

        let executor = Arc::new(SubagentExecutor::new());
        arm_scoped_missions(&executor, cwd).await;
        // A live session id: a scheduled result is session-partitioned and an unattributable one
        // is refused by `write_async_result_file`.
        executor.set_host_services(Arc::new(FixedSessionHost("session-sched")));
        executor.capture_parent_session_anchor();
        {
            let mut cfg = executor.config_cell().lock().await;
            cfg.spawn_command = Some(crate::spawn::SpawnCommand {
                binary: script,
                base_args: Vec::new(),
            });
        }

        // PRODUCTION ENTRY POINT 1 — this is what builds `ExecutorScheduleLauncher`.
        let manager = executor
            .install_scheduled_runs(cwd)
            .await
            .expect("scheduled runs are enabled by default");

        assert!(
            !marker.exists(),
            "precondition: nothing has spawned before the schedule fires"
        );

        // PRODUCTION ENTRY POINT 2 — the real `schedule.create` verb.
        manager
            .handle_action(
                ScheduledRunAction::Create,
                &ScheduledRunActionParams {
                    id: Some("nightly".to_string()),
                    at: Some("+1h".to_string()),
                    workflow_script: Some(
                        "await runs.run(\"sweep\", { agent: \"worker\", task: \"sweep the tree\", \
                         model: \"sonnet\" });\nreturn \"swept\";"
                            .to_string(),
                    ),
                    ..Default::default()
                },
            )
            .await
            .expect("schedule.create");

        // PRODUCTION ENTRY POINT 3 — the real `schedule.run` verb, which reaches
        // `trigger::launch` and thence the injected-at-install production launcher.
        let fired = manager
            .handle_action(
                ScheduledRunAction::Run,
                &ScheduledRunActionParams {
                    id: Some("nightly".to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("schedule.run");
        assert!(
            !fired.is_error,
            "the fire must not report a failed launch: {}",
            fired.text
        );

        let id = ScheduleId::parse("nightly").expect("id");
        let settled = await_settled(&manager, &id).await;
        assert_eq!(
            settled.state,
            ScheduleRunState::Completed,
            "the fired run must really have completed: {settled:?}"
        );

        // EVIDENCE 1 — a separate OS process ran, and wrote this.
        let written = std::fs::read_to_string(&marker).unwrap_or_else(|error| {
            panic!(
                "the scheduled run never spawned a child process — the marker at {} is absent \
                 ({error}). A launcher that returns Ok without spawning passes every other \
                 scheduled-run row in this crate and fails exactly here.",
                marker.display()
            )
        });
        assert_eq!(
            written.trim(),
            "the scheduled run reached a real child process"
        );

        // EVIDENCE 2 — the run's own status.json, written by the runner, not by this test.
        let async_dir = settled
            .async_dir
            .clone()
            .expect("an attached async run names its directory");
        // The SAME roots arithmetic production used, so this reads the tree the fire really
        // wrote to rather than a path the test invented.
        let cfg = executor.config_snapshot().await;
        let async_root = crate::extension::executor::paths::default_async_root_in(&cfg.roots, cwd);
        assert!(
            async_dir.starts_with(&async_root),
            "the fired run must live under THIS project's async root ({}), not somewhere the \
             schedule invented: {}",
            async_root.display(),
            async_dir.display()
        );
        let status = crate::background::control::read_status_file(&async_dir.join("status.json"))
            .await
            .expect("status read")
            .expect("a fired run really wrote a status.json");
        assert_eq!(status.mode, crate::background::RunMode::Workflow);
        assert!(
            status.state.is_terminal(),
            "the fired run's status must be terminal on disk, not left Running: {:?}",
            status.state
        );
        assert_eq!(
            status
                .session_id
                .as_ref()
                .map(crate::identity::SessionId::as_str),
            Some("session-sched"),
            "the run is attributed to the session that was live when it fired"
        );
        assert_eq!(
            settled.async_id.as_deref(),
            Some(status.run_id.as_str()),
            "the schedule's recorded async id and the run's own id must agree — `schedule.delete`'s \
             active-run guard cross-checks exactly this pair"
        );

        // EVIDENCE 3 — `publish_scheduled_result`'s write, which IS the emit for a run nobody is
        // waiting on. Without it the run happened where no session could ever see it.
        let results = published_results(&executor, cwd).await;
        let published = results
            .iter()
            .find(|result| {
                result
                    .schedule_origin
                    .as_ref()
                    .is_some_and(|origin| origin.id == "nightly")
            })
            .expect("the fired run must publish a result carrying its scheduleOrigin");
        assert!(published.success, "{published:?}");
        assert_eq!(published.state, crate::background::RunState::Complete);
        assert_eq!(published.mode, crate::background::RunMode::Workflow);
        assert_eq!(
            published
                .session_id
                .as_ref()
                .map(crate::identity::SessionId::as_str),
            Some("session-sched"),
            "an unattributable scheduled result is invisible to every reader"
        );
        assert_eq!(
            published.run_id.as_str(),
            status.run_id.as_str(),
            "the published result names the run that actually ran"
        );

        // EVIDENCE 4 — the budget the schedule path is documented to charge really moved.
        let cap = cfg.max_subagent_spawns_per_session;
        assert_eq!(
            executor.spawn_budget_snapshot(cap).used,
            1,
            "the fired run is billed to this session exactly once"
        );

        manager.dispose();
    }
}
