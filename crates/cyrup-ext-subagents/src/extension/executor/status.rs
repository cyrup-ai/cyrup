//! Run-status and fleet inspection: tracker resumption, status listings, fleet state and the
//! `status` view renderer.

use std::path::{Path, PathBuf};

use crate::background::{RunId, RunPaths, RunState, run_status};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::foreground_history::ForegroundHistoryRun;
use crate::extension::executor::foreground_transcript;
use crate::extension::executor::paths::{default_async_root_in, default_results_dir_in};
use crate::extension::executor::requests::StatusViewSelector;
use crate::extension::host::native_impl::read_nested_children;

impl SubagentExecutor {
    /// Resume background-run tracking from disk (R-SA-093's "resume on session start" note in
    /// `on_event`'s own doc): re-discover any run directories still present under this cwd's
    /// `AsyncRoot` from a prior process and re-track them, so a restarted orchestrator does not
    /// lose visibility into still-running detached runs.
    ///
    /// Mirrors pi's `restoreActiveJobs` (`async-job-tracker.ts:490-508` @v0.43.0) exactly: only runs whose
    /// RECONCILED state is `queued` or `running` are re-tracked — a run that has already reached a
    /// terminal state (`complete`/`failed`/`paused`) by the time this process restarts is NOT
    /// re-tracked (pi's own `listAsyncRuns({ states: ["queued", "running"] })` filter), and each
    /// restored job's `events.jsonl` byte cursor is seeded from the file's CURRENT size (pi's
    /// `restoredControlEventCursor`, ENOENT → 0) so historical control events already written before
    /// this process existed are never re-tailed. A `read_dir` failure on the `AsyncRoot` itself is
    /// logged (pi's `console.error` in the listing `catch`) rather than silently swallowed.
    pub async fn resume_tracking(&self, cwd: &Path) {
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        let current_session =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        let mut entries = match tokio::fs::read_dir(&async_root).await {
            Ok(entries) => entries,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    async_root = %async_root.display(),
                    "failed to restore active async jobs: could not list AsyncRoot"
                );
                return;
            }
        };
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        async_root = %async_root.display(),
                        "failed to restore active async jobs: error reading AsyncRoot entry"
                    );
                    break;
                }
            };
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            // pi `async-status.ts:502` — the reserved index dirs (`.terminal-runs`,
            // `.active-runs`) are inside the async root but are not runs; restoring one would
            // reconcile it as a status-less run on every session start.
            if crate::background::terminal_run_index::is_reserved_async_root_entry(&name) {
                continue;
            }
            let run_id = RunId::from_token(name);
            let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

            // Only queued/running runs are restored (pi: `listAsyncRuns({ states: ["queued",
            // "running"] })`) — reconcile first so a run that is claimed-Running-but-actually-dead
            // is correctly classified as terminal (Failed) rather than spuriously re-tracked.
            let Ok(outcome) = crate::background::reconcile::reconcile_now(&paths, None).await
            else {
                continue;
            };
            if !matches!(outcome.status.state, RunState::Queued | RunState::Running) {
                continue;
            }

            // S7 — pi `async-job-tracker.ts:658`/`:709` gate both lifecycle transitions on
            // `state.currentSessionId`, PERMISSIVE: a host with no session identity restores
            // everything, a host with one restores only its own.
            //
            // `async_root` is per-cwd (`background/artifact_roots.rs:281-284`), so this listing
            // sees every concurrent instance's runs. Adopting a foreign one is not merely
            // cosmetic: `JobTracker`'s run ids are a CANDIDATE SOURCE for the result watcher
            // (pi `result-watcher.ts:641`), so an unscoped tracker would silently re-widen the
            // delivery partitioning this change exists to establish.
            if !crate::background::delivery::SessionGate::Permissive
                .admits(current_session.as_ref(), outcome.status.session_id.as_ref())
            {
                continue;
            }

            // Seed the events cursor at the file's CURRENT size (pi: `restoredControlEventCursor`)
            // so this process never re-tails control events a prior process already consumed.
            let events_cursor = match tokio::fs::metadata(&paths.events).await {
                Ok(metadata) => metadata.len(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => 0,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        events_path = %paths.events.display(),
                        "failed to stat events.jsonl while restoring async job; seeding cursor at 0"
                    );
                    0
                }
            };

            self.tracker
                .track_restored(run_id, paths, events_cursor)
                .await;
        }
    }

    // ---------------------------------------------------------------------------------------
    // Background control actions (C5): status / interrupt / resume / append-step
    //
    // Each method is the executor half of one `subagent` control action (pi
    // `subagent-executor.ts:2845-2912`), routing to the faithful [`crate::background::control`]
    // primitives + the [`crate::background::run_status`] report shape. They return
    // `Result<String, String>`: `Ok` is the rendered report/confirmation the caller shows as tool
    // content; `Err` is the user-facing failure message the tool surface turns into a `ToolError`
    // (cyrup's `ToolResult` has no `isError` flag, so a soft user-facing error is an `Err(text)`).
    // ---------------------------------------------------------------------------------------

    /// `action: "status"` (C5): render the no-id "list active runs" view, or a single run's full
    /// per-step report resolved by `id` (exact or unique prefix) or by `dir` — pi
    /// `subagent-executor.ts:2845-2863` + `run-status.ts:101-273`.
    ///
    /// # Errors
    ///
    /// Returns the not-found notice (or a resolution/reconciliation error message) as `Err`.
    pub async fn control_status(
        &self,
        cwd: &Path,
        id: Option<&str>,
        dir: Option<&str>,
        child_safe: bool,
    ) -> Result<String, String> {
        self.control_status_view(cwd, id, dir, child_safe, StatusViewSelector::default())
            .await
    }

    /// G92: `action: "status"` with pi's optional `view`/`lines`/`index` selectors
    /// (`extension/schemas.ts:232-237` + `run-status.ts:192-320` @v0.34.0). [`Self::control_status`]
    /// is this with all three absent.
    ///
    /// The branch order is pi's own, and the order is load-bearing:
    ///
    /// 1. **an unknown `view` is rejected before anything else** (`run-status.ts:192-198`) — so
    ///    `view: "flee"` reports the typo rather than silently rendering the ordinary report;
    /// 2. **`view: "fleet"` short-circuits ahead of id resolution** (`:200`) — the fleet surface is
    ///    deliberately id-free, and a caller passing both gets the fleet;
    /// 3. **no id + `view: "transcript"`** resolves to the single active run when there is exactly
    ///    one, and otherwise reports how to choose (`:213-219`);
    /// 4. only then does the ordinary id/dir resolution run.
    ///
    /// # Errors
    ///
    /// Returns the unknown-view/child-safe/not-found notices (or a resolution/reconciliation error
    /// message) as `Err`.
    /// Build the FleetView's `SubagentState` projection (pi's `state` argument to
    /// `collectFleetSnapshot`/`collectFleetStatusEntries`, `tui/fleet.ts:137`,
    /// `tui/fleet-status.ts:147`) from this executor's LIVE registries plus, optionally, the
    /// on-disk async root.
    ///
    /// `include_history` is pi's `options.asyncDirRoot !== undefined` branch (`fleet.ts:192-203`):
    /// the inspector passes `true` (it lists finished runs too), the always-on status widget passes
    /// `false` (it only ever shows active work, `fleet-status.ts:182`).
    ///
    /// Everything the live registries and the on-disk status records actually know is threaded
    /// through, including four things that used to be dropped on the floor:
    ///
    /// * **`session_id`** now comes from each run's OWN recorded
    ///   [`crate::background::RunStatus::session_id`], not from stamping the current session onto
    ///   every job. Stamping made `belongs_to_current_session` (`fleet.ts:63-65`) a tautology — no
    ///   job could ever fail it — so a run inherited from a previous session in the same process
    ///   showed up as this session's.
    /// * **`nested_children`** are resolved one level from
    ///   [`crate::background::StepStatus::nested_run_ids`] by reading each nested run's own
    ///   `status.json` ([`crate::tui::fleet_state::NestedRunView::from_run_status`]). Reading is
    ///   not reconciling: nothing is repaired, killed or re-terminalised, so this is not the
    ///   recursive reconcile `background/fleet_view.rs` declines (its delta 2). Without them
    ///   `fleet-status.ts`'s whole nested tree rendered as absent.
    /// * **`ForegroundControlView::current_tool`/`current_path`/`activity_state`/`mode`** are read
    ///   off the live control entry rather than left at their `Default`.
    /// * **`foreground_runs`** (WORKFLOW_7) — settled foreground runs this process remembers
    ///   (pi's `state.foregroundRuns`), projected from [`Self::foreground_runs_views`]. Populated
    ///   from the SAME in-memory record `foreground_history::persist` writes to disk, so it
    ///   survives a restart within this session and is restored (STRICT, session-scoped) on the
    ///   next one. `tui/fleet.rs`'s own session filter over this field was written ahead of a real
    ///   producer and was dead until now.
    pub async fn fleet_state(
        &self,
        cwd: &Path,
        include_history: bool,
        fleet_inspector_open: bool,
    ) -> crate::tui::fleet_state::FleetState {
        use crate::tui::fleet_state::{
            AsyncRunView, FleetState, ForegroundChildView, ForegroundControlView,
        };

        let services = self.host_services();
        let current_session_id = services.as_ref().and_then(|s| s.session_id());
        let parent_session_file = services.as_ref().and_then(|s| s.session_file());

        let foreground_controls: Vec<ForegroundControlView> = {
            let controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls
                .iter()
                .map(|(run_id, entry)| ForegroundControlView {
                    run_id: run_id.clone(),
                    // WORKFLOW_6 §3.3 — the entry's OWN recorded session, not a stamp:
                    // `fleet.ts:63-65`'s `belongsToCurrentSession` is a real test on this axis now,
                    // matching what the async half already does (this method's own doc, above).
                    session_id: entry.session_id.as_ref().map(|s| s.as_str().to_string()),
                    // VL-S6 — `selectedInspectAction`'s parent rung (`fleet.ts:956-958`): a
                    // workflow CHILD has no async directory of its own, so `Enter`/`H` inspects
                    // the PARENT's run. The value has been on the entry since WORKFLOW_18
                    // (`notices.rs:73`) with no reader; this is the reader.
                    parent_workflow_run_id: entry
                        .parent_workflow_run_id
                        .as_ref()
                        .map(|id| id.as_str().to_string()),
                    current_agent: entry.current_agent.clone(),
                    current_index: entry.current_index,
                    activity_state: entry.current_activity_state,
                    current_tool: entry.current_tool.clone(),
                    current_path: entry.current_path.clone(),
                    turn_count: entry.turn_count,
                    tool_count: entry.tool_count,
                    tokens: entry.tokens,
                    mode: entry.mode,
                    description: entry.description.clone(),
                    started_at: entry.started_at,
                    updated_at: entry.updated_at,
                    // The entry's own cwd, falling back to the caller's for an entry registered
                    // before this field existed.
                    cwd: entry.cwd.clone().or_else(|| Some(cwd.to_path_buf())),
                    // `ForegroundControlView::active_children` has been a permanently-empty `Vec`
                    // since it was added; project the real map now, sorted by index — which a
                    // `BTreeMap`'s iteration order already gives.
                    active_children: entry
                        .active_children
                        .values()
                        .map(ForegroundChildView::from)
                        .collect(),
                    ..ForegroundControlView::default()
                })
                .collect()
        };

        let mut tracked_jobs: Vec<AsyncRunView> = Vec::new();
        for job in self.tracker.snapshot() {
            let Some(status) = job.last_status else {
                continue;
            };
            // pi `nestedChildren` (`fleet-status.ts:193,212`), resolved from the ids each step
            // records. One level, read-only — see this method's doc.
            let nested_children = read_nested_children(&job.paths, &status).await;
            tracked_jobs.push(AsyncRunView {
                // The run's OWN recorded session, so `belongs_to_current_session` is a real test.
                session_id: status.session_id.as_ref().map(|s| s.as_str().to_string()),
                paths: job.paths,
                status,
                description: None,
                context: None,
                nested_children,
            });
        }

        let cfg = self.config_snapshot().await;
        let mut state = FleetState {
            // SUBA-048 / pi `state.artifactDirPreference` (`extension/index.ts:375`), seeded from
            // `config.artifactDir` and read by `fleetArtifactsRoot` (`fleet.ts:334-340`).
            artifact_dir_preference: cfg.artifact_dir_preference(),
            base_cwd: cwd.to_path_buf(),
            current_session_id,
            // SUBA-091 / pi `state.trustedSessionRoots` (`extension/index.ts:895-898` @v0.64.0):
            // `config.defaultSessionDir` (tilde-expanded, resolved) plus the parent session's
            // subagent session root — the roots `asyncDetail`'s session-transcript fallback
            // (`tui/fleet.ts:557`) is confined to. Before this field the fleet passed `[]` and
            // every such read was refused.
            trusted_session_roots: super::paths::trusted_session_roots(
                cfg.default_session_dir.as_deref(),
                parent_session_file.as_deref(),
            ),
            // VL-S6 / pi `state.herdrProjectPanes` (`fleet-status.ts:351-365`) — the map the
            // SessionStart restore (`extension/index.ts:980-981`) last refreshed, flattened for
            // `tui::fleet_status::project_pane_entries`. Empty until that restore has run, which
            // renders no project-pane section at all rather than an empty heading.
            herdr_project_panes: self.herdr_project_pane_snapshots(),
            parent_session_file,
            foreground_controls,
            // WORKFLOW_7 §3.3 — pi `state.foregroundRuns` (`fleet.ts:211`). Was `Vec::new()` with a
            // stated reason; the reason is gone (see this method's own doc, above).
            // `tui/fleet.rs:418`'s `belongs_to_current_session` filter — written with this
            // collection in mind and dead until now — becomes load-bearing with this line and
            // needs no change of its own.
            foreground_runs: self.foreground_runs_views(),
            tracked_jobs,
            history_jobs: Vec::new(),
            fleet_inspector_open,
            scan_error: None,
        };
        if include_history {
            let roots = cfg.roots;
            let async_root = default_async_root_in(&roots, cwd);
            let results_dir = default_results_dir_in(&roots, cwd);
            match crate::tui::fleet::collect_fleet_history(
                &async_root,
                &results_dir,
                state.current_session_id.as_deref(),
            )
            .await
            {
                Ok(history) => state.history_jobs = history,
                // pi's own `catch` (`fleet.ts:207-209`) turns a failed history scan into the
                // snapshot's `error` — rendered as a `Fleet scan warning:` line above the detail
                // pane — never into an empty roster. The live half is kept either way.
                Err(error) => {
                    tracing::debug!(target: "cyrup_ext_subagents::fleet", %error, "fleet history scan failed");
                    state.scan_error = Some(error);
                }
            }
        }
        state
    }

    /// `debug.run` — pi `inspectSubagentStatus`'s `debug.run` arms (`run-status.ts:406-410`,
    /// `:463-468`, `:472-475`, `:512-519`, `:714-721`, `:781-785` @v0.68.0): resolve the run's
    /// location WITHOUT the foreground/nested ladder `status` uses (`:406-410` calls
    /// `resolveAsyncRunLocation` directly, no session filter), run the FULL stale-run reconciler
    /// (`:475`), inspect the run's active-capacity slot with the three identities upstream passes
    /// (`:515`), read the run's process-terminal pair
    /// ([`debug_process_terminal`](crate::background::run_lifecycle_debug::debug_process_terminal),
    /// pi `:514`'s `debugProcessTerminal(asyncDir, status)`), and render
    /// [`crate::background::run_lifecycle_debug::format_run_lifecycle_debug`].
    ///
    /// The display-dismissed marker is NOT a refusal here (`:485-492` dumps over the on-disk
    /// status regardless), which is why this does not route through `inspect_paths`.
    ///
    /// # Errors
    ///
    /// Each of pi's refusal sentences, verbatim: an unresolvable id (`:463-468`), a run with no
    /// `status.json` but a result file (`:714-721`), a directory with neither (`:781-785`), the
    /// `dir` form's two `resolveAsyncRunLocation` refusals (outside the async root, `id`/`dir`
    /// disagree — `async-resume.ts:229-233`), plus the resolver's own ambiguity sentence and any
    /// genuine I/O failure.
    pub async fn control_debug_run(
        &self,
        cwd: &Path,
        id: Option<&str>,
        dir: Option<&str>,
    ) -> Result<String, String> {
        use crate::background::active_async_capacity::inspect_active_async_capacity_owner;
        use crate::background::reconcile::reconcile_now;
        use crate::background::run_lifecycle_debug::{
            DEBUG_RUN_NEEDS_STATUS_DIR, RunLifecycleDebug, debug_process_terminal,
            format_run_lifecycle_debug,
        };

        let cfg = self.config_snapshot().await;
        let async_root = default_async_root_in(&cfg.roots, cwd);
        let results_dir = default_results_dir_in(&cfg.roots, cwd);

        // pi `resolveAsyncRunLocation` (`async-resume.ts:223-259`): the `dir` form (`:227-235`)
        // takes the directory as given, asserts it is inside the async root, and refuses an `id`
        // that disagrees with its basename; the id form resolves exact-then-prefix with no
        // session filter (`None`), which is what lets an operator debug another session's stuck
        // run. Both forms yield a location whose `result_path` is `exactResultPath` (`:234`,
        // `:240`), which the no-`status.json` split below needs.
        let location = match (dir, id) {
            (Some(dir), requested) => crate::background::resolve_async_run_dir(
                Path::new(dir),
                requested,
                cwd,
                &async_root,
                &results_dir,
            )
            .map_err(|error| error.to_string())?,
            (None, Some(id)) => {
                let location =
                    crate::background::resolve_async_run_id(id, &async_root, &results_dir, None)
                        .map_err(|error| error.to_string())?;
                let Some(location) = location else {
                    // pi `:463-468`.
                    return Err("Async run not found. Provide id or dir.".to_string());
                };
                location
            }
            (None, None) => {
                return Err(
                    crate::background::run_lifecycle_debug::DEBUG_RUN_REQUIRES_TARGET.to_string(),
                );
            }
        };
        let Some(async_dir) = location.async_dir else {
            // pi `:714-721` — a result file alone is not a directory to dump.
            return Err(DEBUG_RUN_NEEDS_STATUS_DIR.to_string());
        };
        let paths = RunPaths::for_run(
            async_dir.parent().unwrap_or(&async_dir),
            &results_dir,
            &location.resolved_id,
        );

        // pi `:472-475` — `readStatus` then the FULL reconciler — over a directory that exists.
        // Upstream's `reconcileAsyncRun` returns `status: null` when no `status.json` exists
        // (`stale-run-reconciler.ts:369`) and never repairs one, so the verb then reaches
        // `:714-721` when a result file exists and the `:781-785` fall-through when none does.
        // cyrup's `reconcile_now` differs exactly there: with no `status.json` and a result it
        // repairs one INTO the directory (`background/reconcile.rs`, `repair_from_result`,
        // `existing: None => needs_repair`) and files it in the active-run index. A diagnostic
        // must not create the record it reports, so the split is made on the file's existence
        // BEFORE the reconciler runs, and the reconciler runs only over a `status.json` that was
        // already there.
        let status_existed = tokio::fs::try_exists(&paths.status)
            .await
            .map_err(|error| error.to_string())?;
        if !status_existed {
            return Err(if location.result_path.is_some() {
                DEBUG_RUN_NEEDS_STATUS_DIR.to_string()
            } else {
                "Status file not found.".to_string()
            });
        }
        let status = reconcile_now(&paths, None)
            .await
            .map_err(|error| error.to_string())?
            .status;

        // pi `:515` — `{ runId, sessionId, asyncDir }` against `{ rootDir, liveWorkflowRunIds,
        // abandonedSlotReleaseAfterMs }`, the latter resolved ONCE by `capacity_options`.
        let options = Self::capacity_options(&cfg, self.live_workflow_run_ids());
        let capacity = inspect_active_async_capacity_owner(
            &status.run_id,
            status.session_id.as_ref(),
            Some(&paths.run_dir),
            &options,
        )
        .await
        .map_err(|error| error.to_string())?;
        // pi `:514` — the sidecar and the status overlay, read against `:53`'s expectation.
        let terminal = debug_process_terminal(
            &crate::background::RunDir::for_existing(&paths.run_dir),
            &status,
        )
        .await;

        Ok(format_run_lifecycle_debug(&RunLifecycleDebug {
            status: &status,
            paths: &paths,
            sidecar: terminal.sidecar.as_ref(),
            overlay: terminal.overlay.as_ref(),
            capacity: &capacity,
        }))
    }

    pub async fn control_status_view(
        &self,
        cwd: &Path,
        id: Option<&str>,
        dir: Option<&str>,
        child_safe: bool,
        selector: StatusViewSelector<'_>,
    ) -> Result<String, String> {
        let StatusViewSelector { view, lines, index } = selector;
        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);

        // (1) pi `run-status.ts:192-198`.
        if let Some(view) = view
            && view != "fleet"
            && view != "transcript"
        {
            return Err(format!(
                "Unknown status view: {view}. Valid: fleet, transcript."
            ));
        }
        // (2) pi `run-status.ts:200`.
        if view == Some("fleet") {
            let runs = run_status::list_active_runs(
                &async_root,
                &results_dir,
                self.current_session_id().as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;
            return crate::background::fleet_view::format_fleet(
                &self.foreground_fleet_entries(),
                &runs,
                child_safe,
                crate::time::now_epoch_millis(),
            );
        }

        // (3) pi `run-status.ts:202-231`: the no-id branch, which `view: "transcript"` narrows.
        let transcript = view == Some("transcript");
        let mut resolved_id: Option<String> = id.map(str::to_string);
        if resolved_id.is_none() && dir.is_none() {
            if child_safe {
                return Err(
                    "Child-safe subagent status requires an id when no foreground run is active."
                        .to_string(),
                );
            }
            // SCOPE_10 — pi `run-status.ts:367-371`. Runs BEFORE the `listAsyncRuns` fallback
            // below (`:381`): a live foreground run in THIS session is the answer to a bare
            // `view: "transcript"`, and only if there is none does the async listing get a turn.
            let foreground = if transcript {
                crate::identity::SessionId::parse_opt(self.current_session_id().as_deref())
                    .and_then(|current| self.most_recent_live_foreground_run(&current))
            } else {
                None
            };
            if let Some(run_id) = foreground {
                // pi `return inspectSubagentStatus({ ...params, id: foreground.runId }, deps)`
                // (`:371`) — a RE-ENTRY with the resolved id, which lands in the by-id foreground
                // arm of branch (4) below. Assigning `resolved_id` is that re-entry without the
                // second pass over branches already taken.
                resolved_id = Some(run_id);
            } else {
                let runs = run_status::list_active_runs(
                    &async_root,
                    &results_dir,
                    self.current_session_id().as_deref(),
                )
                .await
                .map_err(|e| e.to_string())?;
                if !transcript {
                    // SCOPE_11 — pi `run-status.ts:390-393`, the ONE place upstream renders armed
                    // subscriptions: `[formatAsyncRunList(runs), waitSubscriptions].filter(Boolean)
                    // .join("\n\n")`. Not the by-id form below, not the by-dir form, and not the
                    // fleet view above — upstream renders it here and nowhere else.
                    //
                    // No session filtering happens here and none is needed: `armed()` is
                    // session-scoped BY CONSTRUCTION (its only two writers are `arm`, which stamps the
                    // live session, and `restore`, which is gated on it). `filter(Boolean)` is the
                    // `None` drop.
                    let armed = self
                        .wait_subscriptions()
                        .map(|manager| manager.armed())
                        .unwrap_or_default();
                    let subscriptions =
                        crate::background::wait_subscriptions::format_wait_subscriptions(
                            &armed,
                            crate::time::now_epoch_millis(),
                        );
                    let runs = run_status::format_run_list(&runs);
                    return Ok(match subscriptions {
                        Some(subscriptions) => format!("{runs}\n\n{subscriptions}"),
                        None => runs,
                    });
                }
                match runs.as_slice() {
                    [only] => resolved_id = Some(only.status.run_id.as_str().to_string()),
                    [] => return Err("No active async run transcript is available.".to_string()),
                    many => {
                        return Err(format!(
                            "Transcript view requires an id when {} active async runs exist. Use \
                         subagent({{ action: \"status\", view: \"fleet\" }}) to choose one.",
                            many.len()
                        ));
                    }
                }
            }
        }

        // (4) the ordinary id/dir resolution. pi precedence (`run-status.ts:131`): a bare `id` (no
        // `dir`) resolves by id; otherwise a present `dir` resolves the directory directly.
        if !transcript {
            // VL-S11b — pi `run-status.ts:421-424`: `const run = deps.state?.foregroundRuns?.get(
            // resolved.id); if (run) return formatRememberedForegroundStatus(run);`
            //
            // The SECOND foreground arm, and the one a DETACHED run needs. `foreground_controls`
            // and `foreground_runs` are disjoint by construction (`executor/mod.rs`: the history
            // entry is created at the exact point the live control is removed), and a detached run
            // is in the history map by definition — `run_foreground_impl` remembers the receipt
            // and then settles the control. So the SCOPE_10 branch below, which resolves through
            // `foreground_controls` only, cannot see it, and without this arm the id falls
            // straight through to `Async run not found. Provide id or dir.`
            //
            // Placed ahead of the async resolution for upstream's own reason: a foreground run has
            // no `status.json` to reconcile, so asking the async resolver about it can only fail.
            //
            // pi `:425`'s ternary has a transcript half (`formatRememberedForegroundTranscript`)
            // with no cyrup counterpart — `foreground_transcript.rs` renders the LIVE control
            // only, off `active_children`, and a remembered run carries no event stream to render.
            // `view: "transcript"` on a detached id therefore still falls through; the status view
            // this arm serves is what `/subagents-detach`'s own success sentence names.
            if let Some(id) = resolved_id.as_deref()
                && dir.is_none()
                && let Some(run) = self.remembered_foreground_run(id)
            {
                return Ok(format_remembered_foreground_status(&run));
            }

            // S6 — the renderer is HANDED the two live registries it may not lock across an
            // `.await`; see `run_status::RunStatusRenderDeps`.
            let deps = self.run_status_render_deps();
            return match (resolved_id.as_deref(), dir) {
                (Some(id), None) => {
                    run_status::inspect_status_by_id(&async_root, &results_dir, id, &deps)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "Async run not found. Provide id or dir.".to_string())
                }
                (_, Some(dir)) => {
                    run_status::inspect_status_by_dir(Path::new(dir), &results_dir, &deps)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "Async run not found. Provide id or dir.".to_string())
                }
                // Unreachable: branch (3) either returned or filled `resolved_id`.
                (None, None) => Err("Async run not found. Provide id or dir.".to_string()),
            };
        }

        // SCOPE_10 — pi `run-status.ts:412-416`: `resolveSubagentRunId(...).kind === "foreground"`
        // is answered by the `foreground_controls` registry itself
        // (`nested_control.rs`'s `resolve_live_foreground_run`, exact-then-unique-prefix, the same
        // rule every other selector honours). A live foreground run has no `status.json` to
        // reconcile, so this MUST run before the async resolution below — without it the id falls
        // through to `Async run not found. Provide id or dir.`
        if let Some(id) = resolved_id.as_deref()
            && dir.is_none()
            && let Some(control_run_id) = self.resolve_live_foreground_run(id)
        {
            // Cloned out of the lock section: the entry derives `Clone` for exactly this, and
            // everything below is `async`/IO.
            let control = {
                let controls = self
                    .foreground_controls
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                controls.get(&control_run_id).cloned()
            };
            if let Some(control) = control {
                let state = self.live_foreground_transcript_state(cwd).await;
                return foreground_transcript::format_live_foreground_transcript(
                    &control,
                    &control_run_id,
                    &state,
                    index,
                    lines,
                );
            }
        }

        let (status, paths) = match (resolved_id.as_deref(), dir) {
            (Some(id), None) => run_status::reconcile_by_id(&async_root, &results_dir, id).await,
            (_, Some(dir)) => run_status::reconcile_by_dir(Path::new(dir), &results_dir).await,
            (None, None) => Ok(None),
        }
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Async run not found. Provide id or dir.".to_string())?;

        // S5 — pi `run-status.ts:494`/`:521`, PERMISSIVE. A transcript is the run's full event
        // stream, so an ungated `id`/`dir` lookup lets any instance read another instance's
        // conversation out of the shared per-cwd root. Upstream's message, verbatim.
        if !crate::background::delivery::SessionGate::Permissive.admits(
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref()).as_ref(),
            status.session_id.as_ref(),
        ) {
            return Err(
                "Transcript view is only available for async runs owned by the current session."
                    .to_string(),
            );
        }

        crate::background::fleet_view::format_async_run_transcript(
            &status,
            &paths,
            index,
            lines,
            &self.transcript_session_roots(cwd, &roots),
        )
    }

    /// SCOPE_12 — `action: "inspect"`: one run's (or one child's) transcript tail and final
    /// output, read back out of the canonical artifacts.
    ///
    /// A pure dispatcher's counterpart, shaped exactly like [`Self::control_status_view`]: this
    /// method owns resolving the roots, the current session and the clock from the executor, and
    /// [`crate::background::inspect_rpc::build_inspect_reply`] owns everything downstream of that.
    ///
    /// # The trusted roots are the TRANSCRIPT view's
    ///
    /// Upstream's inspect confines its session reads to `state.trustedSessionRoots`
    /// (`inspect-rpc.ts:374`) — cyrup's [`crate::extension::executor::paths::trusted_session_roots`]
    /// — unioned with `status.sessionRoot`, which cyrup's [`crate::background::RunStatus`] does not
    /// carry. This passes [`Self::transcript_session_roots`] instead, and the choice is recorded
    /// rather than left to inference: inspect and `view: "transcript"` dereference the SAME
    /// recorded `sessionFile` from the SAME per-cwd artifact roots, so confining them differently
    /// would mean one surface could read a child transcript the other refuses. The containment
    /// MECHANISM is the one gate either way
    /// ([`crate::background::fleet_view::read_session_messages_tail`]).
    ///
    /// # The request id
    ///
    /// [CYRUP-DELTA] upstream's `requestId` correlates a widget reply with the slash command that
    /// asked for it. A TOOL CALL is self-correlating — the result returns to its own call — so
    /// there is no caller-supplied token here and a fixed one is stamped. The slash entry point
    /// ([`crate::background::inspect_rpc::handle_inspect_rpc_args`]) still carries the real thing.
    pub async fn control_inspect(
        &self,
        cwd: &Path,
        async_id: &str,
        child_id: Option<&str>,
        lines: Option<i64>,
    ) -> crate::background::inspect_rpc::InspectReply {
        /// Matches `is_valid_request_id`, so the reply is never rewritten to `invalid`.
        const TOOL_REQUEST_ID: &str = "tool";

        let roots = self.config_snapshot().await.roots;
        let async_root = default_async_root_in(&roots, cwd);
        let results_dir = default_results_dir_in(&roots, cwd);
        let current_session =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        crate::background::inspect_rpc::build_inspect_reply(
            &crate::background::inspect_rpc::InspectRequest {
                request_id: TOOL_REQUEST_ID.to_string(),
                async_id: async_id.to_string(),
                child_id: child_id.map(str::to_string),
                lines,
            },
            &crate::background::inspect_rpc::InspectDeps {
                async_root: &async_root,
                results_dir: &results_dir,
                current_session: current_session.as_ref(),
                trusted_roots: &self.transcript_session_roots(cwd, &roots),
                // ONE clock read, threaded through the replay filter and its re-verification.
                now: crate::time::now_epoch_millis(),
            },
        )
        .await
    }

    /// S6 — pi's `deps` subset the single-run status renderer reads
    /// (`run-status.ts:609-613`), materialised from this executor's two live registries.
    ///
    /// Built HERE rather than read inside the renderer for the reason
    /// [`run_status::RunStatusRenderDeps`] states at length: both registries are
    /// `std::sync::Mutex`es and the renderer is `async`, so the projection crosses the boundary
    /// as a value and no lock is ever held across an `.await`.
    fn run_status_render_deps(&self) -> run_status::RunStatusRenderDeps {
        let foreground_controls: Vec<run_status::LiveWorkflowControlCandidate> = {
            let controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            controls
                .iter()
                .map(|(run_id, entry)| run_status::LiveWorkflowControlCandidate {
                    run_id: run_id.clone(),
                    session_id: entry.session_id.clone(),
                    parent_workflow_run_id: entry.parent_workflow_run_id.clone(),
                    // `active_children` is a `BTreeMap`, so these keys are already ascending
                    // and pi's `:687` numeric sort is a no-op — see the candidate's own doc.
                    child_indexes: entry.active_children.keys().copied().collect(),
                })
                .collect()
        };
        run_status::RunStatusRenderDeps {
            current_session: crate::identity::SessionId::parse_opt(
                self.current_session_id().as_deref(),
            ),
            live_workflow_run_ids: self.live_workflow_run_ids(),
            foreground_controls,
        }
    }

    /// pi `run-status.ts:368-370` — the live foreground control a bare `view: "transcript"`
    /// resolves to, session-filtered and most-recently-updated first.
    ///
    /// # `:368` IS expressible as `Some(&current)` equality, and S6's `:611` is not
    ///
    /// The asymmetry is upstream's and is worth naming: `:367`'s `deps.state?.currentSessionId`
    /// truthiness guard makes the current session known-`Some` before `:368` compares, so the
    /// `None`/`None` row that makes S6's comparison unrepresentable as a
    /// [`crate::background::delivery::SessionGate`] simply cannot arise here. The caller performs
    /// that hoist by passing a `&SessionId`.
    ///
    /// # `[CYRUP-DELTA, unrepresentable]` — `state.lastForegroundControlId` has no port
    ///
    /// Upstream's `:369` prefers the control the host last activated
    /// (`controls.find((c) => c.runId === deps.state?.lastForegroundControlId)`) and only then
    /// falls back to `:370`'s `updatedAt` DESC sort. cyrup writes no such field — upstream's five
    /// writers are `extension/index.ts:472,954`, `subagent-executor.ts:545,551` and
    /// `async-job-tracker.ts:759`, none of which has a cyrup counterpart — so the selection
    /// degenerates to `:370` alone. That is a DEGRADATION, not an equivalence: with two live
    /// controls in one session the host's own last-activated one may not be the most recently
    /// updated. [`ForegroundControlEntry::updated_at`] is bumped on every control-event
    /// transition, so the fallback is the sharpest signal in the tree today; porting the field is
    /// a separate task with its own writers.
    ///
    /// The run-id tie-break is cyrup's: upstream's `sort` is stable over a `Map`'s insertion
    /// order, and a `HashMap` has none, so ties resolve ascending by id rather than
    /// unpredictably.
    fn most_recent_live_foreground_run(
        &self,
        current: &crate::identity::SessionId,
    ) -> Option<String> {
        let controls = self
            .foreground_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        controls
            .iter()
            // `:368` — the control list is SESSION-FILTERED.
            .filter(|(_, entry)| entry.session_id.as_ref() == Some(current))
            .max_by(|(left_id, left), (right_id, right)| {
                left.updated_at
                    .cmp(&right.updated_at)
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(run_id, _)| run_id.clone())
    }

    /// The `deps.state` subset [`foreground_transcript::format_live_foreground_transcript`] reads
    /// (pi `run-status.ts:265` plus the `:249` gate's left-hand side).
    async fn live_foreground_transcript_state(
        &self,
        cwd: &Path,
    ) -> foreground_transcript::LiveForegroundTranscriptState {
        let services = self.host_services();
        foreground_transcript::LiveForegroundTranscriptState {
            current_session: crate::identity::SessionId::parse_opt(
                self.current_session_id().as_deref(),
            ),
            parent_session_file: services.as_ref().and_then(|s| s.session_file()),
            base_cwd: cwd.to_path_buf(),
            artifact_dir_preference: self.config_snapshot().await.artifact_dir_preference(),
        }
    }

    /// The live foreground runs the fleet view renders (pi `[...state.foregroundControls.values()]`,
    /// `fleet-view.ts:318`), projected onto [`crate::background::fleet_view::ForegroundFleetEntry`].
    fn foreground_fleet_entries(&self) -> Vec<crate::background::fleet_view::ForegroundFleetEntry> {
        let controls = self
            .foreground_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut entries: Vec<_> = controls
            .iter()
            .map(
                |(run_id, entry)| crate::background::fleet_view::ForegroundFleetEntry {
                    run_id: run_id.clone(),
                    current_agent: entry.current_agent.clone(),
                    current_index: entry.current_index,
                    activity_state: entry.current_activity_state,
                    session_id: entry.session_id.clone(),
                    parent_workflow_run_id: entry.parent_workflow_run_id.clone(),
                },
            )
            .collect();
        // pi sorts by `updatedAt` descending (`fleet-view.ts:236`); cyrup's registry carries no
        // per-entry timestamp, so run id gives the same STABLE ordering a `HashMap` iteration
        // cannot (an unordered fleet listing would make the rendered text non-deterministic).
        entries.sort_by(|a, b| a.run_id.cmp(&b.run_id));
        entries
    }

    /// The trusted roots a `view: "transcript"` session-JSONL read is confined to (pi
    /// `trustedSessionRootsForStatus`, `subagent-executor.ts:402-407` @v0.43.0). A recorded `sessionFile` is
    /// data a CHILD wrote, so it is never dereferenced outside these roots — see
    /// [`crate::background::fleet_view`]'s containment gate.
    fn transcript_session_roots(&self, cwd: &Path, roots: &crate::paths::Roots) -> Vec<PathBuf> {
        let mut session_roots = vec![
            default_async_root_in(roots, cwd),
            crate::artifacts::project_subagents_dir(cwd),
            crate::artifacts::temp_artifacts_dir(cwd),
        ];
        session_roots.dedup();
        session_roots
    }

    /// VL-S11b — pi `deps.state?.foregroundRuns?.get(resolved.id)` (`run-status.ts:421`), with the
    /// exact-then-UNIQUE-prefix rule every other selector in this crate honours
    /// ([`SubagentExecutor::resolve_live_foreground_run`]'s own, restated over the OTHER map).
    ///
    /// Cloned out of the lock: the caller renders asynchronously and this is a `std::sync::Mutex`.
    /// An ambiguous prefix declines, for the same reason the live resolver's does — the accurate
    /// diagnosis for two matching runs is the async resolver's `AmbiguousRunId`, not a silently
    /// chosen one.
    fn remembered_foreground_run(&self, selector: &str) -> Option<ForegroundHistoryRun> {
        let runs = self
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, run)) = runs.iter().find(|(id, _)| id.as_str() == selector) {
            return Some(run.clone());
        }
        let mut prefix = runs
            .iter()
            .filter(|(id, _)| id.as_str().starts_with(selector));
        let (_, first) = prefix.next()?;
        if prefix.next().is_some() {
            return None;
        }
        Some(first.clone())
    }
}

/// pi `deps.state.foregroundRuns` as `bg_wait` consumes it (`subagent-wait.ts:226`) — the
/// executor's projection of its remembered-run map onto the shape `background/wait` is able to
/// name.
///
/// A [`std::sync::Weak`] for the same reason `ExecutorSubscriptionSessions` and
/// `ExecutorForegroundProbe` are (`extension/executor/wait_subscriptions.rs`): the hook is handed
/// to a wait that may outlive the turn, and a detached child must not keep the executor alive. A
/// dropped executor snapshots as "no remembered runs", which makes every wait on one report the
/// run as having disappeared rather than blocking on a map nobody can write to any more.
struct ExecutorDetachedForegroundRuns {
    executor: std::sync::Weak<SubagentExecutor>,
}

impl crate::background::wait::DetachedForegroundRunsSource for ExecutorDetachedForegroundRuns {
    fn snapshot(&self) -> Vec<crate::background::wait::DetachedForegroundRun> {
        let Some(executor) = std::sync::Weak::upgrade(&self.executor) else {
            return Vec::new();
        };
        let runs = executor
            .foreground_runs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        runs.values()
            .map(|run| crate::background::wait::DetachedForegroundRun {
                run_id: run.run_id.as_str().to_string(),
                session_id: run.session_id.as_str().to_string(),
                children: run
                    .children
                    .iter()
                    .map(|child| crate::background::wait::DetachedForegroundChild {
                        index: child.index,
                        agent: child.agent.clone(),
                        status: child.status.clone(),
                        // `[CYRUP-DELTA]`, already recorded and not re-stated: a remembered child
                        // carries no live activity state or current tool (`wait_subscriptions`'
                        // module doc, and `ExecutorForegroundProbe`'s own `// Not persisted` note
                        // on the identical projection). Upstream's supervisor-attention test
                        // (`subagent-wait.ts:242`) therefore cannot fire for a cyrup detached run;
                        // it is ported in full because the record format is shared with the
                        // subscription manager, which restores records written by any producer.
                        activity_state: None,
                        current_tool: None,
                        transcript_path: child.transcript_path.clone(),
                    })
                    .collect(),
            })
            .collect()
    }
}

impl SubagentExecutor {
    /// VL-S11b — the hook `extension/wait_tool.rs` attaches to every `bg_wait` call's
    /// [`crate::background::wait::WaitDeps`], so a wait's candidate set is upstream's full one
    /// (async runs PLUS remembered detached foreground runs, `subagent-wait.ts:581-584`).
    ///
    /// `self: &Arc<Self>` because the hook must hold a [`std::sync::Weak`]; every production
    /// caller already holds the executor in an [`Arc`](std::sync::Arc).
    #[must_use]
    pub fn detached_foreground_hook(
        self: &std::sync::Arc<Self>,
    ) -> crate::background::wait::DetachedForegroundHook {
        crate::background::wait::DetachedForegroundHook::new(std::sync::Arc::new(
            ExecutorDetachedForegroundRuns {
                executor: std::sync::Arc::downgrade(self),
            },
        ))
    }
}

/// pi `rememberedForegroundChildOutput` (`run-status.ts:192-203`): prefer the artifact/saved
/// output FILE when it is still on disk, and fall back to the remembered inline snapshot.
///
/// The file is authoritative because `compact_child` drops `final_output` from a persisted record
/// that has an output path (`foreground_history/persist.rs`) — exactly upstream's own
/// `...(!outputPath && child.finalOutput ? … : {})`. A read failure falls through silently, as
/// upstream's `catch` does.
fn remembered_foreground_child_output(
    artifact_output_path: Option<&Path>,
    saved_output_path: Option<&Path>,
    final_output: Option<&str>,
) -> String {
    if let Some(path) = artifact_output_path.or(saved_output_path)
        && let Ok(text) = std::fs::read_to_string(path)
        && !text.trim().is_empty()
    {
        return text.trim().to_string();
    }
    final_output.unwrap_or_default().to_string()
}

/// pi `formatRememberedForegroundStatus` (`run-status.ts:205-243`) — what
/// `subagent({ action: "status", id })` renders for a run that is no longer LIVE but is still
/// remembered, which is the only shape a detached run ever has.
///
/// Narrowed to the fields
/// [`ForegroundHistoryChild`](crate::extension::executor::foreground_history) carries. Three of
/// upstream's per-child parts have no cyrup field and are therefore absent rather than faked:
/// `sessionName` (pi `:216`; cyrup's history child records the agent only), `exit ${exitCode}`
/// (`:218`) and `acceptance: ${status}` (`:220`) — the same field-set narrowing that record's own
/// "scoped out, deliberately" note already states, and none of them changes which branch the
/// trailing recovery line takes.
fn format_remembered_foreground_status(run: &ForegroundHistoryRun) -> String {
    let run_id = run.run_id.as_str();
    let mut lines = vec![
        format!("Run: {run_id}"),
        "State: remembered foreground".to_string(),
        format!("Mode: {}", crate::formatters::run_mode_label(run.mode)),
        format!(
            "Updated: {}",
            crate::time::format_iso8601_millis(run.updated_at)
        ),
        format!("Cwd: {}", run.cwd.display()),
    ];
    for child in &run.children {
        // pi `:215` — the FIRST non-blank line of the child's output, capped at 160 chars.
        let output = remembered_foreground_child_output(
            child.artifact_output_path.as_deref(),
            child.saved_output_path.as_deref(),
            child.final_output.as_deref(),
        );
        let preview = output
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(|line| line.chars().take(160).collect::<String>());
        let mut parts = vec![format!(
            "{}. {} {}",
            child.index + 1,
            child.agent,
            child.status
        )];
        if let Some(error) = child.error.as_ref() {
            parts.push(format!("error: {error}"));
        }
        if let Some(preview) = preview.filter(|p| !p.is_empty()) {
            parts.push(format!("output: {preview}"));
        }
        lines.push(parts.join(", "));
        if let Some(path) = child.session_file.as_ref() {
            lines.push(format!("  Session: {}", path.display()));
        }
        if let Some(path) = child.transcript_path.as_ref() {
            lines.push(format!("  Transcript: {}", path.display()));
        }
        if let Some(path) = child.artifact_output_path.as_ref() {
            lines.push(format!("  Output: {}", path.display()));
        }
        if let Some(path) = child
            .saved_output_path
            .as_ref()
            .filter(|saved| Some(*saved) != child.artifact_output_path.as_ref())
        {
            lines.push(format!("  Saved output: {}", path.display()));
        }
        if let Some(warning) = child.output_save_error.as_ref() {
            lines.push(format!("  Output warning: {warning}"));
        }
        if let Some(warning) = child.transcript_error.as_ref() {
            lines.push(format!("  Transcript warning: {warning}"));
        }
    }
    lines.push(String::new());
    lines.push(format!(
        "Status: subagent({{ action: \"status\", id: \"{run_id}\" }})"
    ));
    if run.children.len() == 1 {
        lines.push(format!(
            "Transcript: subagent({{ action: \"status\", id: \"{run_id}\", view: \"transcript\" }})"
        ));
    } else {
        lines.push(format!(
            "Transcript: subagent({{ action: \"status\", id: \"{run_id}\", index: 0, view: \
             \"transcript\" }})"
        ));
    }
    // pi `:236-242` — the detached arm WINS over the resume arm, because a still-detached child
    // must not be replaced by a revival of its own session file.
    let detached = run.children.iter().any(|child| child.status == "detached");
    let resumable = run
        .children
        .iter()
        .find(|child| child.session_file.as_ref().is_some_and(|p| p.exists()));
    if detached {
        lines.push(format!(
            "Recovery: reply to the supervisor request first, then wait with {}({{ id: \
             \"{run_id}\" }}); do not resume or launch a replacement while any child remains \
             detached.",
            crate::extension::wait_tool::WAIT_TOOL_NAME
        ));
    } else if let Some(child) = resumable {
        lines.push(if run.children.len() == 1 {
            format!(
                "Revive: subagent({{ action: \"resume\", id: \"{run_id}\", message: \"...\" }})"
            )
        } else {
            format!(
                "Revive child: subagent({{ action: \"resume\", id: \"{run_id}\", index: {}, \
                 message: \"...\" }})",
                child.index
            )
        });
    } else {
        lines.push("Resume: unavailable; no child session file was persisted.".to_string());
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    // ---------------------------------------------------------------------------------------
    // C5 control-action dispatch smoke tests (executor glue; read-only, no spawn, no home writes)
    //
    // These drive the real `SubagentExecutor::control_*` methods over a fresh temp cwd whose async
    // root has never been created, so every path is a pure read that returns the expected empty /
    // not-found rendering without spawning any process or touching the user's `~/.cyrup` tree. The
    // full per-run rendering + primitive behavior is covered by `background::run_status`'s own tests
    // against explicit temp roots.
    // ---------------------------------------------------------------------------------------

    /// SUBA-091 — `fleet_state` seeds pi's `state.trustedSessionRoots`
    /// (`extension/index.ts:895-898` @v0.64.0) from the configured `default_session_dir`. With no
    /// host services bound there is no parent session file, so that rung is the only one; with
    /// nothing configured the list is pi's initial `[]` (`:447`). Pre-fix the field did not exist
    /// and the fleet inspector passed a literal empty slice regardless of configuration.
    #[tokio::test]
    async fn fleet_state_seeds_trusted_session_roots_from_the_configured_default_session_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let configured = dir.path().join("subagent-sessions");
        let executor =
            SubagentExecutor::with_config(crate::registration::SubagentExtensionConfig {
                default_session_dir: Some(configured.clone()),
                ..crate::registration::SubagentExtensionConfig::default()
            });
        let state = executor.fleet_state(dir.path(), false, false).await;
        assert_eq!(state.trusted_session_roots, vec![configured]);

        let bare = SubagentExecutor::new()
            .fleet_state(dir.path(), false, false)
            .await;
        assert!(bare.trusted_session_roots.is_empty());
    }

    #[tokio::test]
    async fn control_status_no_id_over_a_fresh_cwd_lists_no_active_runs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let text = executor
            .control_status(dir.path(), None, None, false)
            .await
            .expect("status list is Ok even with no runs");
        assert_eq!(text, "No active async runs.");
    }

    #[tokio::test]
    async fn control_status_unknown_id_is_the_not_found_notice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_status(dir.path(), Some("deadbeef0000"), None, false)
            .await
            .expect_err("an unknown id is a not-found error");
        assert_eq!(err, "Async run not found. Provide id or dir.");
    }

    /// pi `run-status.ts:104-110`: the child-safe fanout tool (`deps.nested` truthy) hard-errors on
    /// a no-id status call instead of listing the cwd's active runs — a fanout child has no
    /// business enumerating its parent's whole async root. Regression proof: pre-fix,
    /// `control_status` had no `child_safe` parameter at all and always fell through to
    /// `list_active_runs`, which would have made this assert `Ok("No active async runs.")` instead.
    #[tokio::test]
    async fn control_status_child_safe_no_id_hard_errors_instead_of_listing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        let err = executor
            .control_status(dir.path(), None, None, true)
            .await
            .expect_err("child-safe no-id status must hard-error, not list runs");
        assert_eq!(
            err,
            "Child-safe subagent status requires an id when no foreground run is active."
        );
    }

    // ---------------------------------------------------------------------------------------
    // SCOPE_10 SUBTASK3 — the live-foreground transcript, its `:249` gate and its `:368` selector.
    //
    // Every test below fails on the pre-SCOPE_10 tree for one shared reason: a foreground run id
    // was not resolvable at all here (`run_status::resolve_run_id` scans only ASYNC run
    // directories), so `view: "transcript"` answered every one of these inputs with
    // `Async run not found. Provide id or dir.` and the no-id branch never consulted
    // `foreground_controls`.
    // ---------------------------------------------------------------------------------------

    use crate::extension::executor::foreground_control::ForegroundChildEntry;
    use crate::extension::executor::notices::ForegroundControlEntry;
    use crate::extension::testsupport::FixedSessionIdHost;
    use std::sync::Arc;

    fn child(index: usize, agent: &str) -> ForegroundChildEntry {
        ForegroundChildEntry {
            index,
            agent: agent.to_string(),
            session_name: None,
            description: None,
            started_at: 0,
            updated_at: 0,
            current_activity_state: None,
            current_tool: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            tokens: None,
            interrupt: cyrup_core::CancelToken::new(),
            steer: None,
        }
    }

    fn register_control(
        executor: &SubagentExecutor,
        run_id: &str,
        session: Option<&str>,
        updated_at: i64,
        children: &[usize],
    ) {
        let entry = ForegroundControlEntry {
            detach: None,
            interrupt: cyrup_core::CancelToken::new(),
            current_agent: None,
            current_index: None,
            current_activity_state: None,
            mode: crate::background::RunMode::Single,
            description: None,
            current_tool: None,
            current_path: None,
            turn_count: None,
            tool_count: None,
            tokens: None,
            started_at: 0,
            updated_at,
            session_id: session.and_then(crate::identity::SessionId::parse),
            parent_workflow_run_id: None,
            workflow_key: None,
            cwd: None,
            session_name: None,
            active_children: children
                .iter()
                .map(|index| (*index, child(*index, "scout")))
                .collect(),
        };
        executor
            .foreground_controls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(run_id.to_string(), entry);
    }

    fn transcript_view<'a>() -> StatusViewSelector<'a> {
        StatusViewSelector {
            view: Some("transcript"),
            lines: None,
            index: None,
        }
    }

    /// pi `run-status.ts:249-251`, STRICT: a foreground run owned by ANOTHER session is refused
    /// with upstream's verbatim sentence rather than rendered. `foreground_controls` is a
    /// per-process registry, but two cyrup instances share one cwd and one artifact root, and the
    /// transcript is the child's whole conversation.
    #[tokio::test]
    async fn a_foreground_transcript_of_a_foreign_run_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some("session-a".to_string()),
            file: None,
        }));
        register_control(&executor, "fg0000000001", Some("session-b"), 0, &[0]);

        let err = executor
            .control_status_view(
                dir.path(),
                Some("fg0000000001"),
                None,
                false,
                transcript_view(),
            )
            .await
            .expect_err("a foreign foreground run is refused");
        assert_eq!(
            err,
            "Foreground run 'fg0000000001' is not owned by the current session."
        );
    }

    /// `:249`'s FIRST arm, `!state.currentSessionId` — the arm that distinguishes
    /// `SessionGate::Strict` from `Permissive`. A host with no session identity is refused even
    /// though the control it is asking about records no session either, because with no identity
    /// there is nothing to establish ownership WITH.
    #[tokio::test]
    async fn a_foreground_transcript_with_no_session_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        // No host services bound at all: `current_session_id()` is `None`.
        let executor = SubagentExecutor::new();
        register_control(&executor, "fg0000000001", None, 0, &[0]);

        let err = executor
            .control_status_view(
                dir.path(),
                Some("fg0000000001"),
                None,
                false,
                transcript_view(),
            )
            .await
            .expect_err("a sessionless host cannot own a foreground run");
        assert_eq!(
            err,
            "Foreground run 'fg0000000001' is not owned by the current session."
        );
    }

    /// `:368` — the no-id transcript branch consults `foreground_controls` FIRST, and the list it
    /// consults is SESSION-FILTERED. The foreign control below is deliberately the most recently
    /// updated one, so an unfiltered `:370` sort would pick it.
    #[tokio::test]
    async fn the_transcript_control_list_is_session_filtered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some("session-a".to_string()),
            file: None,
        }));
        register_control(&executor, "fgown0000001", Some("session-a"), 10, &[0]);
        register_control(&executor, "fgforeign001", Some("session-b"), 999, &[0]);

        let report = executor
            .control_status_view(dir.path(), None, None, false, transcript_view())
            .await
            .expect("the session's own live foreground run answers a bare transcript request");
        assert!(report.contains("Run: fgown0000001"), "{report}");
        assert!(report.contains("State: live foreground"), "{report}");
        assert!(!report.contains("fgforeign001"), "{report}");
    }

    /// `:370` — among the session's OWN live controls, the most recently updated wins.
    ///
    /// [CYRUP-DELTA] upstream's `:369` first prefers `state.lastForegroundControlId`, which cyrup
    /// does not record; see `most_recent_live_foreground_run`'s own note. This test pins the
    /// fallback that remains, which is the whole selection here.
    #[tokio::test]
    async fn the_no_id_transcript_prefers_the_most_recently_updated_live_control() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some("session-a".to_string()),
            file: None,
        }));
        register_control(&executor, "fgolder00001", Some("session-a"), 10, &[0]);
        register_control(&executor, "fgnewer00001", Some("session-a"), 20, &[0]);

        let report = executor
            .control_status_view(dir.path(), None, None, false, transcript_view())
            .await
            .expect("a live foreground run answers a bare transcript request");
        assert!(report.contains("Run: fgnewer00001"), "{report}");
    }

    /// `:259` — a control with no active child REPORTS that, as a rendered line, instead of
    /// failing. The distinction matters: "there is nothing to show yet" is an answer, and an
    /// error would send the caller looking for a broken run.
    #[tokio::test]
    async fn a_live_foreground_transcript_with_no_active_child_reports_it_rather_than_failing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some("session-a".to_string()),
            file: None,
        }));
        register_control(&executor, "fg0000000001", Some("session-a"), 0, &[]);

        let report = executor
            .control_status_view(
                dir.path(),
                Some("fg0000000001"),
                None,
                false,
                transcript_view(),
            )
            .await
            .expect("no active child is a rendered notice, not an error");
        assert_eq!(
            report,
            "Run: fg0000000001\nState: live foreground\nTranscript unavailable: no active \
             foreground child."
        );
    }

    /// `:260-262` — with several children live and no `index`, the refusal ENUMERATES the
    /// indexes, so the caller's next call is a copy-edit rather than a guess.
    #[tokio::test]
    async fn a_live_foreground_transcript_requires_an_index_when_several_children_are_active() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some("session-a".to_string()),
            file: None,
        }));
        register_control(&executor, "fg0000000001", Some("session-a"), 0, &[0, 2]);

        let err = executor
            .control_status_view(
                dir.path(),
                Some("fg0000000001"),
                None,
                false,
                transcript_view(),
            )
            .await
            .expect_err("an ambiguous child selection is refused");
        assert_eq!(
            err,
            "Transcript view requires index for foreground run 'fg0000000001'. Active child \
             indexes: 0, 2."
        );

        // …and naming one of them renders it.
        let report = executor
            .control_status_view(
                dir.path(),
                Some("fg0000000001"),
                None,
                false,
                StatusViewSelector {
                    index: Some(2),
                    ..transcript_view()
                },
            )
            .await
            .expect("a named child renders");
        assert!(report.contains("Child: 2 (scout)"), "{report}");
    }

    /// VL-S11b — `subagent({ action: "status", id })` finds a run that is in `foreground_runs`
    /// but NOT in `foreground_controls`, which is by construction the shape every DETACHED run
    /// has (`run_foreground_impl` remembers the receipt and then drops the live control).
    ///
    /// **Gutting mutation this fails on:** delete the second foreground arm (the
    /// `remembered_foreground_run` branch). The id then falls through to the async resolver, which
    /// has no `status.json` for a foreground run, and the call returns
    /// `Err("Async run not found. Provide id or dir.")` — so both the `expect` and every
    /// `contains` below fire.
    #[tokio::test]
    async fn status_by_id_finds_a_remembered_foreground_run_that_is_no_longer_live() {
        let dir = tempfile::tempdir().expect("tempdir");
        let executor = SubagentExecutor::new();
        executor.set_host_services(std::sync::Arc::new(
            crate::extension::testsupport::FixedSessionIdHost {
                id: Some("session-detach".to_string()),
                file: None,
            },
        ));

        // The receipt a detach publishes, remembered exactly as `run_foreground_impl` does.
        let receipt = crate::extension::executor::foreground::detach_receipt(
            "scout",
            "hold the line",
            crate::extension::executor::detach::DetachReason::UserRequest,
            false,
        );
        let run_id = RunId::from_token("fgdetached0001".to_string());
        executor.remember_foreground_run(
            &run_id,
            crate::background::RunMode::Single,
            dir.path(),
            &[&receipt.result],
        );
        // The live-control map is EMPTY: the two maps are disjoint, which is the whole premise.
        assert_eq!(
            executor.resolve_live_foreground_run("fgdetached0001"),
            None,
            "a detached run must not be in `foreground_controls`"
        );

        let text = executor
            .control_status(dir.path(), Some("fgdetached0001"), None, false)
            .await
            .expect("a remembered foreground run resolves by id");
        assert!(text.contains("Run: fgdetached0001"), "{text}");
        assert!(text.contains("State: remembered foreground"), "{text}");
        assert!(text.contains("Mode: single"), "{text}");
        assert!(text.contains("1. scout detached"), "{text}");
        // pi `:238` — the detached arm wins, and it names the REGISTERED wait tool.
        assert!(
            text.contains(&format!(
                "Recovery: reply to the supervisor request first, then wait with {}({{ id: \
                 \"fgdetached0001\" }})",
                crate::extension::wait_tool::WAIT_TOOL_NAME
            )),
            "{text}"
        );

        // A UNIQUE PREFIX resolves it too, matching every other selector in this crate.
        let by_prefix = executor
            .control_status(dir.path(), Some("fgdetached"), None, false)
            .await
            .expect("a unique prefix resolves a remembered foreground run");
        assert!(by_prefix.contains("Run: fgdetached0001"), "{by_prefix}");
    }

    /// S6's executor half: the deps handed to the renderer carry each control's session, its
    /// parent workflow id and its ACTIVE CHILD INDEXES, already ascending — which is what lets
    /// `run_status.rs` skip upstream's `:687` numeric sort.
    #[test]
    fn the_run_status_render_deps_project_active_child_indexes_in_ascending_order() {
        let executor = SubagentExecutor::new();
        register_control(&executor, "fg0000000001", Some("session-a"), 0, &[5, 0, 2]);
        let deps = executor.run_status_render_deps();
        assert_eq!(deps.foreground_controls.len(), 1);
        assert_eq!(deps.foreground_controls[0].child_indexes, vec![0, 2, 5]);
        assert_eq!(
            deps.foreground_controls[0].session_id,
            crate::identity::SessionId::parse("session-a")
        );
    }
}
