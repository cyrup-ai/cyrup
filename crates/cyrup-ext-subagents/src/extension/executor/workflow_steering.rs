//! Workflow-scoped foreground steering (WORKFLOW_7 SUBTASK1) — the resolver that lets
//! `action: "steer"` address a live workflow shell's own foreground child, and the routing that
//! wires it ahead of the blanket foreground refusal.
//!
//! Port of [`runs/foreground/workflow-foreground-steering.ts`
//! (`@57278d82`)](../../../../../../workspace/pi-subagents/src/runs/foreground/workflow-foreground-steering.ts):
//! `activeWorkflowError` (`:21-30`), `controlIsLiveInWorkflow` (`:32-36`) and
//! `resolveWorkflowForegroundSteeringTarget` (`:38-65`), plus the ONE delivery arm this crate can
//! actually reach today, `steerWorkflowForegroundTarget`'s index-defaulting + optional-`steer`
//! refusal (`:128-169`).
//!
//! # Scope: the resolver, the routing, AND the delivery
//!
//! WORKFLOW_14 completed this file. A foreground WORKFLOW child now carries a
//! [`ForegroundChildSteerHandle`](crate::extension::executor::foreground_control::ForegroundChildSteerHandle)
//! on its [`ForegroundChildEntry`](crate::extension::executor::foreground_control::ForegroundChildEntry),
//! because the workflow owns a real run directory (WORKFLOW_13) for the child's inbox to live in,
//! so the delivery arm below writes a real steer request and waits for the child's real
//! acknowledgment.
//!
//! Delivery is pi's own shape: `await child.steer({message, mode})` (`:149`) — the DIRECT line to
//! the child. Upstream calls the live in-process session object; cyrup's child is a spawned OS
//! process, so its direct line is writing into the very inbox directory the child was spawned
//! watching ([`ForegroundChildSteerHandle::deliver`](crate::extension::executor::foreground_control::ForegroundChildSteerHandle::deliver)).
//!
//! ⚠ NOT `request_async_steer_with_mode`. That writes to the background runner's
//! `steer-requests/` INTAKE queue, drained only by `runner_main::control_watcher`'s watch loop. A
//! foreground workflow runs in THIS process and has no such loop, so a request left there is routed
//! nowhere and the child never sees it. The ack half IS shared with the async path
//! (`await_steer_ack`), because an out-of-process child can only answer on disk — that is cyrup's
//! documented SUBA-049 delta from upstream, which gets its outcome synchronously.
//!
//! One arm stays refused, and correctly: a plain foreground SINGLE run still has no run directory,
//! so `STEER_FOREGROUND_RUN_REFUSAL` is unchanged and upstream's optional-`steer` sentence
//! (`workflow-foreground-steering.ts:141`'s `if (!child.steer) return managementError(...)`) is
//! kept for a child whose handle is `None`. `ForegroundChildControl.steer` IS optional upstream;
//! this file narrows that refusal to its true scope rather than deleting it.

use std::path::Path;
use std::sync::PoisonError;

use crate::background::{RunDir, RunId, RunMode, RunState};
use crate::extension::executor::SubagentExecutor;
use crate::extension::executor::foreground_control::ChildSteerReadiness;
use crate::extension::executor::notices::ForegroundControlEntry;
use crate::extension::tool::text::CHILD_SESSION_NOT_RUNNING_YET;
use crate::identity::SessionId;

/// pi `WorkflowForegroundSteeringTarget` (`workflow-foreground-steering.ts:11-15`).
pub(crate) struct WorkflowForegroundSteeringTarget {
    /// The resolved live child's control entry — a CLONE, not a borrow: `foreground_controls` is
    /// a `std::sync::Mutex` and this value outlives the lock section (`notices.rs`'s
    /// `ForegroundControlEntry` derives `Clone` for exactly this).
    control: ForegroundControlEntry,
    /// The `foreground_controls` key the entry was found under. `ForegroundControlEntry` does not
    /// carry its own run id — pi's `ForegroundRunControl.runId` is a field, cyrup's is the MAP KEY
    /// (`mod.rs:132`) — and every refusal below interpolates it, so it is carried here.
    control_run_id: String,
    // pi's `sourceRunId` (the id a delivered steering RECEIPT is attributed to, `:54` vs `:64`) is
    // deliberately NOT a field here: this task's delivery arm never reaches a successful receipt
    // (§0.4 — there is no transport yet), so nothing in-tree would ever READ it, and an unread field
    // is exactly the dead-code shape §5's DoD forbids. WORKFLOW_14 shipped the delivery receipt and
    // it is keyed by `control_run_id`, which this struct already carries — so there is STILL no
    // reader for a separate source run id, and the field stays deferred to the first task that has
    // one.
}

/// The one string `:23`, `:26` and `:62` all return. Factored so the three sites cannot drift.
fn no_live_child(workflow_run_id: &RunId) -> String {
    format!("Workflow '{workflow_run_id}' has no live foreground child.")
}

/// pi `controlIsLiveInWorkflow` (`workflow-foreground-steering.ts:32-36`) — three terms, and all
/// three are required:
///
/// ```ts
/// return control.parentWorkflowRunId === workflowRunId
///     && control.sessionId === sessionId
///     && (control.activeChildren?.size ?? 0) > 0;
/// ```
///
/// Dropping the SESSION term is the bug this whole programme exists to prevent: two cyrup
/// instances sharing a cwd both register foreground controls, and a workflow id is only unique
/// per-process. Dropping the `active_children` term steers a control entry whose child has already
/// finished.
fn control_is_live_in_workflow(
    control: &ForegroundControlEntry,
    workflow_run_id: &RunId,
    session_id: &SessionId,
) -> bool {
    control.parent_workflow_run_id.as_ref() == Some(workflow_run_id)
        && control.session_id.as_ref() == Some(session_id)
        && !control.active_children.is_empty()
}

impl SubagentExecutor {
    /// pi `activeWorkflowError` (`workflow-foreground-steering.ts:21-30`). `Ok(())` is upstream's
    /// `undefined`; every `Err` is one of its verbatim strings.
    ///
    /// The order is upstream's and is load-bearing: the session-identity gate (`:22`) runs FIRST,
    /// hoisted out of the `:28` comparison it makes safe — by the time `:28` runs, `current` is
    /// known `Some`. THEN registry liveness (`:23`, the WORKFLOW_6 registry — checked before any
    /// disk read). THEN the on-disk status read (`:24-27`). THEN the session-ownership gate
    /// (`:28`).
    ///
    /// `pub(crate)` since WORKFLOW_18: the stop-side action
    /// ([`SubagentExecutor::stop_workflow_child_action`]) runs these SAME four gates rather than a
    /// second copy of them — the fourth is `SessionGate::Strict`, and stopping another session's
    /// child is exactly the cross-session hazard it exists to prevent. Widened visibility only;
    /// not one gate, not one sentence, and not the order has changed.
    pub(crate) async fn active_workflow_error(
        &self,
        workflow_run_id: &RunId,
        async_root: &Path,
    ) -> Result<(), String> {
        // `:22` — pi `if (!state.currentSessionId)`. This is `SessionGate::Strict`'s "no current
        // session means refuse" arm, hoisted so the `:28` comparison below runs with a known-`Some`
        // current session.
        let current = SessionId::parse_opt(self.current_session_id().as_deref())
            .ok_or_else(|| "Workflow steering requires an active parent session.".to_string())?;

        // `:23` — the WORKFLOW_6 registry, checked BEFORE any disk read.
        if !self.has_workflow_controller(workflow_run_id) {
            return Err(no_live_child(workflow_run_id));
        }

        // `:24-27` — pi `readStatus(path.join(asyncDirRoot, workflowRunId))`. A missing/unparseable
        // status, a non-workflow mode, or a settled state all collapse to the SAME refusal upstream
        // gives them: from the caller's side they are one fact.
        let status_path = RunDir::new(async_root, workflow_run_id).status();
        let status = crate::background::control::read_status_file(&status_path)
            .await
            .ok()
            .flatten();
        let live = status.as_ref().is_some_and(|s| {
            s.mode == RunMode::Workflow && matches!(s.state, RunState::Running | RunState::Queued)
        });
        if !live {
            return Err(no_live_child(workflow_run_id));
        }

        // `:28` — STRICT. Distinct from `control_steer`'s async gate one function away, which is
        // PERMISSIVE (`control.rs:602-612`, pi `async-steering-action.ts:48`). Do not unify them:
        // the two arms differ only on a `None` current session, and `:22` above already owns that
        // case.
        let owner = status.as_ref().and_then(|s| s.session_id.as_ref());
        if !crate::background::delivery::SessionGate::Strict.admits(Some(&current), owner) {
            return Err(format!(
                "Workflow '{workflow_run_id}' was not found in the active session."
            ));
        }
        Ok(())
    }

    /// pi `resolveWorkflowForegroundSteeringTarget` (`workflow-foreground-steering.ts:38-65`).
    /// Both branches run [`Self::active_workflow_error`] FIRST; the child branch additionally
    /// re-checks [`control_is_live_in_workflow`], because `parentWorkflowRunId` alone does not
    /// prove the control is in *this session* or still has a live child.
    pub(crate) async fn resolve_workflow_foreground_steering_target(
        &self,
        child_run_id: Option<&str>,
        workflow_run_id: Option<&RunId>,
        async_root: &Path,
    ) -> Result<WorkflowForegroundSteeringTarget, String> {
        // `:45-54` — the caller addressed a CHILD.
        if let Some(child_run_id) = child_run_id {
            let control = {
                let controls = self
                    .foreground_controls
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                controls.get(child_run_id).cloned()
            };
            // `:46-47` — not a control entry at all, or one with no workflow owner. Resolved as
            // ONE `let-else` producing BOTH values together, rather than checking `control` twice.
            let Some((control, workflow_run_id)) = control.and_then(|c| {
                let workflow_run_id = c.parent_workflow_run_id.clone()?;
                Some((c, workflow_run_id))
            }) else {
                return Err(format!(
                    "Foreground run '{child_run_id}' is not a live workflow-owned child."
                ));
            };
            // `:49-50` — the workflow's own four gates, before the per-control one.
            self.active_workflow_error(&workflow_run_id, async_root)
                .await?;
            let current =
                SessionId::parse_opt(self.current_session_id().as_deref()).ok_or_else(|| {
                    "Workflow steering requires an active parent session.".to_string()
                })?;
            // `:51-53`
            if !control_is_live_in_workflow(&control, &workflow_run_id, &current) {
                return Err(format!(
                    "Foreground run '{child_run_id}' is not a live workflow-owned child in the \
                     active session."
                ));
            }
            return Ok(WorkflowForegroundSteeringTarget {
                control,
                control_run_id: child_run_id.to_string(),
            });
        }

        // `:57-64` — the caller addressed the WORKFLOW. pi's `:58` "requires a workflow or child
        // run id" is unreachable here: `control_steer` already refused a call with neither id nor
        // dir (`control.rs`'s own guard) before this resolver is ever reached.
        let Some(workflow_run_id) = workflow_run_id else {
            return Err("Workflow steering requires a workflow or child run id.".to_string());
        };
        self.active_workflow_error(workflow_run_id, async_root)
            .await?;
        let current = SessionId::parse_opt(self.current_session_id().as_deref())
            .ok_or_else(|| "Workflow steering requires an active parent session.".to_string())?;

        // `:61` — every live control in THIS workflow and THIS session. Collected with the key,
        // because cyrup's run id is the map key, not a field (see `WorkflowForegroundSteeringTarget`'s
        // own doc).
        let mut matches: Vec<(String, ForegroundControlEntry)> = {
            let controls = self
                .foreground_controls
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            controls
                .iter()
                .filter(|(_, c)| control_is_live_in_workflow(c, workflow_run_id, &current))
                .map(|(k, c)| (k.clone(), c.clone()))
                .collect()
        };
        // Upstream iterates a `Map`, whose order is insertion order; a `HashMap` has none, and the
        // `:63` refusal interpolates a COUNT so the order only matters for the `len == 1` arm,
        // where it cannot. Sorted anyway so a multi-child refusal is reproducible.
        matches.sort_by(|a, b| a.0.cmp(&b.0));

        match matches.len() {
            0 => Err(no_live_child(workflow_run_id)), // `:62`
            1 => match matches.into_iter().next() {
                Some((control_run_id, control)) => Ok(WorkflowForegroundSteeringTarget {
                    control,
                    control_run_id,
                }),
                None => Err(no_live_child(workflow_run_id)),
            },
            n => Err(format!(
                "Workflow '{workflow_run_id}' has {n} live foreground children; steer a child run \
                 id instead."
            )), // `:63`
        }
    }

    /// pi `steerWorkflowForegroundTarget` (`workflow-foreground-steering.ts:128-169`), narrowed to
    /// what this task can deliver: resolve the WORKFLOW-addressed target (this crate's only
    /// reachable route today, see `control.rs`'s routing), default `index` exactly as upstream
    /// does at `:135-140` (sorted active indexes; the sole index when there is one, else require an
    /// explicit index), and land on upstream's own optional-`steer` refusal.
    pub(crate) async fn steer_workflow_foreground(
        &self,
        workflow_run_id: &RunId,
        // WORKFLOW_14: seam discharged. `ForegroundChildEntry` carries the steer handle
        // (`foreground.rs`'s `register_foreground_controls`) and the child was spawned against the
        // matching `RunOptions` steer paths (`build_foreground_run_options`), so both are live
        // parameters now rather than documented placeholders.
        message: &str,
        mode: Option<crate::background::control::SteerDeliveryMode>,
        index: Option<usize>,
        async_root: &Path,
    ) -> Result<String, String> {
        let target = self
            .resolve_workflow_foreground_steering_target(None, Some(workflow_run_id), async_root)
            .await?;
        // pi `:133-137` — `activeChildren` is a `BTreeMap`, so `.keys()` is ALREADY the sorted
        // order upstream builds with `.sort((l, r) => l - r)`.
        let active: Vec<usize> = target.control.active_children.keys().copied().collect();
        let index = match index {
            Some(index) => Some(index),
            None if active.len() == 1 => active.first().copied(),
            None => None,
        };
        let Some(index) = index else {
            return Err(match active.len() {
                0 => format!(
                    "Foreground run '{}' has no live child session.",
                    target.control_run_id
                ),
                n => format!(
                    "Foreground run '{}' has {n} live child sessions; provide index.",
                    target.control_run_id
                ),
            });
        };
        // pi `:139` and `:141` in ONE lookup — a `contains_key` followed by a `get` would give the
        // same string two unreachable-from-each-other homes.
        //
        // ⚠ `index` here keys `active_children`, which is the child's index within its OWN
        // foreground run (always `0` for the single-child run a workflow child is).
        // `handle.index` below is its flat index within the WORKFLOW. Two namespaces; the handle
        // carries its own so they cannot be conflated.
        let Some(child) = target.control.active_children.get(&index) else {
            return Err(format!(
                "Foreground run '{}' child {index} is not live.",
                target.control_run_id
            ));
        };
        // The child's own steer handle — `None` only for a non-workflow foreground child, which
        // cannot reach this function at all (`resolve_workflow_foreground_steering_target` requires
        // `parent_workflow_run_id`). Upstream's optional-`steer` string is kept for that unreachable
        // arm rather than deleted: `ForegroundChildControl.steer` IS optional upstream.
        let Some(handle) = child.steer.as_ref() else {
            return Err(format!(
                "Foreground run '{}' child {index} does not support steering.",
                target.control_run_id
            ));
        };

        // pi `:3860-3861`, consumed BEFORE the write. The control entry registers before the
        // spawned child reaches its runtime, so "there is a live control" does not imply "there is
        // a child to read this". Classifying first is what keeps three different facts — booting,
        // unsteerable, merely busy — from all rendering as one queued receipt that expires.
        match handle.readiness().await {
            ChildSteerReadiness::Ready => {}
            // RETRYABLE, and said as such: this surface has no poll to hand it to (pi's
            // `steerWorkflowChildByKey` does, and that is `WorkflowRunHost::steer`), so the caller
            // is the retry loop. Returning the bare sentence rather than a receipt is upstream's
            // own shape — the reason is the whole answer, and no request id exists yet to correlate.
            ChildSteerReadiness::NotRunningYet => {
                return Err(CHILD_SESSION_NOT_RUNNING_YET.to_string());
            }
            // pi's `catch` arm at `:3867-3869`, reached here at capability-publish time rather
            // than call time because cyrup's child publishes the answer instead of throwing it.
            // Terminal: a host that cannot inject messages will not start being able to.
            ChildSteerReadiness::Unsupported => {
                return Err(format!(
                    "Steering failed for foreground run {} (request -): child {index} cannot be \
                     steered.",
                    target.control_run_id
                ));
            }
        }

        // pi `:149` — `await child.steer({message, mode})`. The child's OWN inbox, addressed
        // directly, because a foreground workflow runs in THIS process: there is no runner watch
        // loop to drain an intake queue on its behalf. `source` distinguishes this route in the
        // request record.
        let request_id = handle
            .deliver(message, mode, "workflow-steer-action")
            .await
            .map_err(|e| e.to_string())?;

        // SUBA-049's whole point, and it applies identically here: a queued file drop is not a
        // delivery. No ack inside the budget is NOT a failure — the request is on disk and a child
        // that reaches a safe point later still takes it.
        //
        // ⚠ The no-ack word is `queued`, NOT the async arm's `pending`, and the two surfaces stay
        // split on purpose. pi's foreground receipt has no `pending` word at all
        // (`workflow-foreground-steering.ts:158`: `deliveryStatus: outcome.state === "delivered" ?
        // "delivered" : "queued"`), and its no-ack contract at `:112-114` is stated as *"no
        // acknowledgment leaves an honest, unaddressed QUEUED receipt"*. `pending` belongs to the
        // ASYNC surface (`foreground_actions/steer.rs`), where it is upstream's own text and stays.
        // Two words for two surfaces, each its own upstream's; a third would be invented.
        //
        // The SENTENCE names the child index as well as the run, which pi's does not: this arm
        // defaults `index` over a multi-child `active_children` map a dozen lines up, so a receipt
        // that named only the run would not say which child took the guidance.
        let outcome = Self::await_steer_ack(&handle.run_dir, &request_id, Some(handle.index)).await;
        let state = match outcome.as_ref() {
            None => "queued",
            Some(ack) => ack.state.as_str(),
        };
        let text = format!(
            "Steering {state} for workflow {} child {index} (request {request_id}).",
            target.control_run_id
        );
        // The async arm's own classification, kept identical: a `failed` ack is an error RESULT,
        // `queued`/`delivered` are ordinary successes because the request is still live.
        // (The script-facing `WorkflowRunHost::steer` returns `Failed` as a receipt VALUE instead —
        // it has a `WorkflowSteerResult::error` to carry it; this surface has only a sentence.)
        match outcome {
            Some(ack) if ack.state == crate::background::control::SteerAckState::Failed => {
                Err(format!("{text} {}", ack.message))
            }
            _ => Ok(text),
        }
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
    use crate::extension::executor::foreground_control::ForegroundChildEntry;
    use crate::extension::testsupport::FixedSessionIdHost;
    use cyrup_core::CancelToken;
    use std::sync::Arc;

    fn control_entry(
        session_id: Option<&str>,
        parent_workflow_run_id: Option<&RunId>,
        active_children: std::collections::BTreeMap<usize, ForegroundChildEntry>,
    ) -> ForegroundControlEntry {
        ForegroundControlEntry {
            interrupt: CancelToken::new(),
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
            updated_at: 0,
            session_id: session_id.and_then(SessionId::parse),
            parent_workflow_run_id: parent_workflow_run_id.cloned(),
            workflow_key: None,
            cwd: None,
            session_name: None,
            active_children,
        }
    }

    /// One live child at `active_children` key 0, carrying `steer` (i.e. a WORKFLOW child) when a
    /// handle is supplied and none (a plain foreground child) when it is not.
    fn one_active_child(
        steer: Option<crate::extension::executor::foreground_control::ForegroundChildSteerHandle>,
    ) -> std::collections::BTreeMap<usize, ForegroundChildEntry> {
        let mut children = std::collections::BTreeMap::new();
        children.insert(
            0,
            ForegroundChildEntry {
                index: 0,
                agent: "scout".to_string(),
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
                interrupt: CancelToken::new(),
                steer,
            },
        );
        children
    }

    fn with_session(executor: &SubagentExecutor, session_id: &str) {
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some(session_id.to_string()),
            file: None,
        }));
    }

    /// `.expect_err()` without requiring `T: Debug` — `WorkflowForegroundSteeringTarget` carries a
    /// `ForegroundControlEntry`, which derives no `Debug` (it holds a raw `CancelToken`), so the
    /// ordinary `Result::expect_err` bound cannot be satisfied by the resolver's `Ok` type.
    fn must_err<T>(result: Result<T, String>, context: &str) -> String {
        match result {
            Ok(_) => panic!("{context}: expected Err, got Ok"),
            Err(err) => err,
        }
    }

    /// Writes a `status.json` reflecting a live, RUNNING workflow run, owned by `session_id`
    /// (or by no session at all, when `session_id` is `None`) — so `active_workflow_error`'s
    /// on-disk read (`:24-27`) has something real to observe.
    fn write_running_workflow_status(
        async_root: &std::path::Path,
        run_id: &RunId,
        session_id: Option<&str>,
    ) {
        let run_dir = async_root.join(run_id.as_str());
        std::fs::create_dir_all(&run_dir).expect("mkdir run dir");
        let mut status = crate::background::RunStatus::queued(
            run_id.clone(),
            crate::background::RunMode::Workflow,
            None,
        );
        status.state = crate::background::RunState::Running;
        status.session_id = session_id.and_then(SessionId::parse);
        std::fs::write(
            run_dir.join("status.json"),
            serde_json::to_vec(&status).expect("serialize status"),
        )
        .expect("write status.json");
    }

    /// Publish the child's own steering capability record, exactly as a booted child's
    /// `prompt_runtime` does (`STEER_CAPABILITY_ENV` → `publish_capability`). Nothing delivers
    /// without one: `ForegroundChildSteerHandle::readiness` reads THIS file to tell a child that has
    /// not reached its runtime from one whose host cannot inject at all.
    async fn publish_capability(run_dir: &std::path::Path, index: usize, supported: bool) {
        crate::background::control::write_steer_capability_at(
            &crate::background::control::steer_capability_path(run_dir, index),
            &crate::background::control::SteerCapability {
                kind: "steer-capability".to_string(),
                protocol_version: 1,
                index,
                // A real pid, because `write_steer_capability_at` refuses a zero one — the same
                // guard that makes a stale record from a dead process detectable.
                pid: 4242,
                ready_at: 1,
                supported,
            },
        )
        .await
        .expect("the child's capability must publish");
    }

    /// How many steer requests are sitting in `inbox`. `0` for a directory that was never created,
    /// which is the assertion every refusal arm needs: a refused steer writes NOTHING.
    fn requests_in(inbox: &std::path::Path) -> usize {
        std::fs::read_dir(inbox)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Gate `:22` — no active parent session refuses BEFORE the registry is even consulted.
    #[tokio::test]
    async fn active_workflow_error_with_no_session_refuses_first() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        // No session bound at all, and no controller registered either — if the session gate ran
        // second, this would report "no live foreground child" instead.
        let err = executor
            .active_workflow_error(&workflow_run_id, dir.path())
            .await
            .expect_err("no session must refuse");
        assert_eq!(err, "Workflow steering requires an active parent session.");
    }

    /// Gate `:23` — a session exists but this process holds no controller for the id: refused
    /// BEFORE any disk read (no `status.json` exists under `dir.path()` at all here).
    #[tokio::test]
    async fn active_workflow_error_with_no_registered_controller_refuses_before_any_disk_read() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();

        let err = executor
            .active_workflow_error(&workflow_run_id, dir.path())
            .await
            .expect_err("no live controller must refuse");
        assert_eq!(err, no_live_child(&workflow_run_id));
    }

    /// Gate `:28` — a live controller exists, and `status.json` genuinely reflects a live
    /// workflow, but its recorded session differs from the caller's: STRICT refusal.
    #[tokio::test]
    async fn active_workflow_error_refuses_a_workflow_owned_by_another_session() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());

        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-b"));

        let err = executor
            .active_workflow_error(&workflow_run_id, dir.path())
            .await
            .expect_err("a foreign-session workflow must refuse");
        assert_eq!(
            err,
            format!("Workflow '{workflow_run_id}' was not found in the active session.")
        );
    }

    /// The full resolver, workflow-addressed branch: a live controller, a live on-disk status
    /// owned by THIS session, and exactly one control whose `parent_workflow_run_id`/`session_id`
    /// match with a non-empty `active_children` — resolves to that sole control.
    #[tokio::test]
    async fn resolve_workflow_foreground_steering_target_resolves_the_sole_live_child() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());

        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-a"));

        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "child-1".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(None),
                ),
            );
        }

        let target = executor
            .resolve_workflow_foreground_steering_target(None, Some(&workflow_run_id), dir.path())
            .await
            .expect("must resolve the sole live child");
        assert_eq!(target.control_run_id, "child-1");
    }

    /// Two live controls in the SAME workflow/session refuse with the exact multi-child sentence,
    /// naming the count.
    #[tokio::test]
    async fn resolve_workflow_foreground_steering_target_refuses_multiple_live_children() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());

        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-a"));

        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "child-1".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(None),
                ),
            );
            controls.insert(
                "child-2".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(None),
                ),
            );
        }

        let err = must_err(
            executor
                .resolve_workflow_foreground_steering_target(
                    None,
                    Some(&workflow_run_id),
                    dir.path(),
                )
                .await,
            "two live children must refuse",
        );
        assert_eq!(
            err,
            format!(
                "Workflow '{workflow_run_id}' has 2 live foreground children; steer a child run id \
                 instead."
            )
        );
    }

    /// The child-addressed branch: an unknown id, and a live id with no `parent_workflow_run_id`,
    /// both refuse with the SAME "is not a live workflow-owned child" sentence.
    #[tokio::test]
    async fn resolve_workflow_foreground_steering_target_child_branch_refuses_a_non_workflow_run() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");

        let err = must_err(
            executor
                .resolve_workflow_foreground_steering_target(
                    Some("does-not-exist"),
                    None,
                    dir.path(),
                )
                .await,
            "an unknown child id must refuse",
        );
        assert_eq!(
            err,
            "Foreground run 'does-not-exist' is not a live workflow-owned child."
        );

        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "plain-run".to_string(),
                control_entry(Some("session-a"), None, one_active_child(None)),
            );
        }
        let err = must_err(
            executor
                .resolve_workflow_foreground_steering_target(Some("plain-run"), None, dir.path())
                .await,
            "a plain (non-workflow) foreground run must refuse the same way",
        );
        assert_eq!(
            err,
            "Foreground run 'plain-run' is not a live workflow-owned child."
        );
    }

    /// DoD manual-confirmation 2, mechanised: a workflow child WITH a steer handle must actually
    /// deliver — the request file lands in the child's OWN inbox (the directory it was spawned
    /// watching), and the answer is a `Steering ...` sentence, never the optional-`steer` refusal.
    ///
    /// This is the case the pre-WORKFLOW_14 tree could not reach at all, and the one an earlier
    /// revision got wrong in a way no assertion caught: writing to the runner's `steer-requests/`
    /// intake queue (drained only by `runner_main::control_watcher`, which a foreground workflow
    /// does not run) instead of `steer-targets/<index>/`. Asserting the PATH, not just the return
    /// string, is what makes that regression impossible to reintroduce silently.
    #[tokio::test]
    async fn steer_workflow_foreground_delivers_into_the_childs_own_inbox() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());
        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-a"));

        // The workflow's own run dir (WORKFLOW_13) and this child's flat index within it.
        let run_dir = dir.path().join("wf-run");
        let inbox = crate::background::control::step_steer_inbox_dir(&run_dir, 0);
        let handle = crate::extension::executor::foreground_control::ForegroundChildSteerHandle {
            inbox_dir: inbox.clone(),
            run_dir: run_dir.clone(),
            index: 0,
        };
        // The child has reached its runtime and its host can inject — without this record the arm
        // below answers `CHILD_SESSION_NOT_RUNNING_YET` instead, and correctly so.
        publish_capability(&run_dir, 0, true).await;
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "child-1".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(Some(handle)),
                ),
            );
        }

        let answer = executor
            .steer_workflow_foreground(
                &workflow_run_id,
                "tighten the scope",
                None,
                None,
                dir.path(),
            )
            .await
            .expect("a child WITH a steer handle must deliver, not refuse");

        // No ack can arrive (the capability record is a fixture; no child process is watching the
        // inbox), so `queued` is the correct outcome — the request is on disk and a child reaching
        // a safe point later still takes it. `queued`, NOT the async surface's `pending`: pi's
        // foreground receipt has no `pending` word at all (`:158`).
        assert!(
            answer.starts_with("Steering queued for workflow child-1 child 0 (request "),
            "got: {answer}"
        );
        assert!(
            !answer.contains("does not support steering"),
            "got: {answer}"
        );

        // THE path assertion: the request is in the child's own inbox.
        let written: Vec<_> = std::fs::read_dir(&inbox)
            .expect("the child's inbox must exist and be readable")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json"))
            .collect();
        assert_eq!(
            written.len(),
            1,
            "exactly one request in {inbox:?}: {written:?}"
        );

        // And NOT in the runner intake queue, which nothing would ever drain here.
        let intake = crate::background::control::steer_requests_dir(&run_dir);
        assert!(
            !intake.exists() || std::fs::read_dir(&intake).map(|d| d.count()).unwrap_or(0) == 0,
            "nothing may be written to the runner intake queue {intake:?}"
        );

        // The payload is addressed to this child and carries the message verbatim.
        let body = std::fs::read_to_string(inbox.join(&written[0])).expect("read request");
        let parsed: serde_json::Value = serde_json::from_str(&body).expect("request is json");
        assert_eq!(parsed["message"], "tighten the scope");
        assert_eq!(parsed["targetIndex"], 0);
    }

    /// §4.3, the fact this surface used to be unable to state. A live control whose child has NOT
    /// yet published a capability record is a child that has not reached its runtime — upstream's
    /// `CHILD_SESSION_NOT_RUNNING_YET` (`subagent-executor.ts:3860-3861`), which
    /// `steerWorkflowChildByKey` POLLS on (`:4501-4502`) rather than treating as a refusal.
    ///
    /// Before the readiness gate, this case and "the child cannot be steered at all" and "the child
    /// is merely busy" were indistinguishable: all three dropped a file, waited out
    /// `STEER_ACK_TIMEOUT` and reported the same queued receipt. The second assertion is the one
    /// that matters most — NOTHING is written, so the classification happens before the request
    /// exists and a retry cannot pile up duplicates in a booting child's inbox.
    #[tokio::test]
    async fn steer_workflow_foreground_reports_an_unbooted_child_as_not_running_yet() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());
        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-a"));

        let run_dir = dir.path().join("wf-run");
        let inbox = crate::background::control::step_steer_inbox_dir(&run_dir, 0);
        let handle = crate::extension::executor::foreground_control::ForegroundChildSteerHandle {
            inbox_dir: inbox.clone(),
            run_dir: run_dir.clone(),
            index: 0,
        };
        // Deliberately NO `publish_capability` — the control registered, the child has not booted.
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "child-1".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(Some(handle)),
                ),
            );
        }

        let err = must_err(
            executor
                .steer_workflow_foreground(&workflow_run_id, "hello", None, None, dir.path())
                .await,
            "an unbooted child must report the retryable reason",
        );
        assert_eq!(err, "Child session is not running yet.");
        assert_eq!(
            requests_in(&inbox),
            0,
            "the classification happens BEFORE the write, so a retry cannot duplicate requests"
        );
    }

    /// The other half of §4.3, and the one a timeout can never tell you: a child whose host cannot
    /// inject messages at all published `supported: false`, which is terminal. Waiting it out would
    /// turn a knowable failure into a queued receipt that expires — the exact conflation the
    /// capability record was added to remove (`control.rs`'s `SteerCapability` doc).
    #[tokio::test]
    async fn steer_workflow_foreground_refuses_a_child_whose_host_cannot_inject() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());
        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-a"));

        let run_dir = dir.path().join("wf-run");
        let inbox = crate::background::control::step_steer_inbox_dir(&run_dir, 0);
        let handle = crate::extension::executor::foreground_control::ForegroundChildSteerHandle {
            inbox_dir: inbox.clone(),
            run_dir: run_dir.clone(),
            index: 0,
        };
        publish_capability(&run_dir, 0, false).await;
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "child-1".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(Some(handle)),
                ),
            );
        }

        let err = must_err(
            executor
                .steer_workflow_foreground(&workflow_run_id, "hello", None, None, dir.path())
                .await,
            "an unsupported child must refuse, never queue",
        );
        assert_eq!(
            err,
            "Steering failed for foreground run child-1 (request -): child 0 cannot be steered."
        );
        assert_ne!(
            err, "Child session is not running yet.",
            "`supported: false` is terminal; conflating it with the retryable reason would make the \
             caller retry forever"
        );
        assert_eq!(requests_in(&inbox), 0, "a refusal writes nothing");
    }

    /// The delivery arm's index defaulting, and the ONE arm that still refuses: with no `index` and
    /// exactly one active child, the sole index is used — and a child carrying NO steer handle
    /// (`steer: None`, i.e. not a workflow child) lands on upstream's own optional-`steer` refusal
    /// rather than a cyrup-invented sentence. WORKFLOW_14 narrowed this refusal to exactly this
    /// case; a workflow child now carries `Some` and delivers instead.
    #[tokio::test]
    async fn steer_workflow_foreground_defaults_the_sole_index_and_refuses_unsupported_steering() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let workflow_run_id = RunId::new();
        executor.register_workflow_controller(&workflow_run_id, CancelToken::new());

        write_running_workflow_status(dir.path(), &workflow_run_id, Some("session-a"));
        {
            let mut controls = executor
                .foreground_controls
                .lock()
                .expect("foreground_controls lock");
            controls.insert(
                "child-1".to_string(),
                control_entry(
                    Some("session-a"),
                    Some(&workflow_run_id),
                    one_active_child(None),
                ),
            );
        }

        let err = executor
            .steer_workflow_foreground(&workflow_run_id, "hello", None, None, dir.path())
            .await
            .expect_err("a child with no steer handle must refuse, never silently succeed");
        assert_eq!(
            err,
            "Foreground run 'child-1' child 0 does not support steering."
        );
    }
}
