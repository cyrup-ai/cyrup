//! Run-status and fleet inspection: tracker resumption, status listings, fleet state and the
//! `status` view renderer.

use std::path::{Path, PathBuf};

use crate::background::{RunId, RunPaths, RunState, run_status};
use crate::extension::executor::SubagentExecutor;
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
    ///   from the SAME in-memory record `persist_foreground_run_history` writes to disk, so it
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

        // (4) the ordinary id/dir resolution. pi precedence (`run-status.ts:131`): a bare `id` (no
        // `dir`) resolves by id; otherwise a present `dir` resolves the directory directly.
        if !transcript {
            return match (resolved_id.as_deref(), dir) {
                (Some(id), None) => run_status::inspect_status_by_id(&async_root, &results_dir, id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "Async run not found. Provide id or dir.".to_string()),
                (_, Some(dir)) => run_status::inspect_status_by_dir(Path::new(dir), &results_dir)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "Async run not found. Provide id or dir.".to_string()),
                // Unreachable: branch (3) either returned or filled `resolved_id`.
                (None, None) => Err("Async run not found. Provide id or dir.".to_string()),
            };
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
}
