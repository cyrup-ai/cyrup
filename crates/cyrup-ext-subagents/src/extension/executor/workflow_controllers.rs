//! The live in-process workflow-controller registry (WORKFLOW_6 §2) — pi's
//! `state.workflowControllers: Map<string, AbortController>` plus the two facts every cyrup
//! consumer needs and upstream re-reads off the run's on-disk status instead.
//!
//! Port of `state.workflowControllers` (`subagent-executor.ts:5095-5098` insert, `:5272` rollback,
//! `:5796` settlement `finally`; `extension/index.ts:1038-1041` abort-then-clear teardown).
//!
//! `workflowChildStops` — the SIBLING registry pi creates alongside this one — was out of scope
//! here and landed as its own map in [`super::workflow_child_stops`] (WORKFLOW_18), fed by the
//! `register_stop_child` seam on [`crate::workflows::scripted::RunWorkflowScriptOptions`] that the
//! `route_workflow_mode` call site now supplies. The two maps stay separate for upstream's own
//! reason: a controller aborts a WORKFLOW, a stop handle stops ONE CHILD of one. The only place
//! they meet is `abort_and_clear_workflow_controllers` below, which tears both down together
//! exactly as `extension/index.ts:1038-1042` does.
//!
//! pi's `topLevelResume` is likewise NOT ported here as a standalone member
//! (`is_top_level_resume`): it is a pure read with no in-scope caller yet — its one real consumer
//! (WORKFLOW_10's per-session capacity gate) does not exist in this build. Porting it ahead of
//! that caller would be dead code; land it, verbatim, alongside WORKFLOW_10's own call site.
//!
//! `state.workflowControllers?.has(runId)` (`has_workflow_controller`, below) was deferred for the
//! same reason at WORKFLOW_6 landing time. WORKFLOW_7's `active_workflow_error`
//! (`workflow_steering.rs`) is the in-scope caller that gives it one, so it is ported here.

use std::collections::HashSet;

use cyrup_core::CancelToken;

use crate::background::RunId;
use crate::extension::executor::SubagentExecutor;

/// One live in-process workflow shell — pi's `AbortController` value in
/// `state.workflowControllers` (`shared/types.ts:2270`), plus the two facts every cyrup consumer
/// of the registry needs and upstream reads off the run's on-disk status instead.
///
/// Upstream's comment is the invariant: *"Live in-process workflow controllers. Durable status
/// remains on disk after settlement."* An entry here means "this process is driving that workflow
/// RIGHT NOW" — never "that workflow exists".
#[derive(Clone)]
pub(crate) struct WorkflowController {
    /// pi's `AbortController`. This is the SAME token `run_workflow_script` was handed as
    /// `RunWorkflowScriptOptions::cancel` (`routing.rs`'s workflow dispatch), so firing it
    /// genuinely aborts the engine and every child it is awaiting, rather than setting a flag
    /// nobody reads.
    abort: CancelToken,
    /// The session that launched this workflow. Upstream re-reads it from `status.sessionId` on
    /// every check (`run-status.ts:610`, `workflow-foreground-steering.ts:27`); an in-process
    /// registry can simply carry it, which is one fewer `status.json` read per gate.
    session_id: Option<crate::identity::SessionId>,
    /// Epoch millis at insert — WORKFLOW_9's detach reconciliation needs an age, and it is free
    /// here.
    started_at: i64,
}

impl WorkflowController {
    pub(crate) fn new(abort: CancelToken, session_id: Option<crate::identity::SessionId>) -> Self {
        Self {
            abort,
            session_id,
            started_at: crate::time::now_epoch_millis(),
        }
    }

    /// pi `controller.abort(...)`. Idempotent — `CancellationToken::cancel` is.
    pub(crate) fn abort(&self) {
        self.abort.cancel();
    }

    pub(crate) fn is_aborted(&self) -> bool {
        self.abort.is_cancelled()
    }

    pub(crate) fn session_id(&self) -> Option<&crate::identity::SessionId> {
        self.session_id.as_ref()
    }

    pub(crate) fn started_at(&self) -> i64 {
        self.started_at
    }
}

impl SubagentExecutor {
    /// pi `subagent-executor.ts:5095-5098` — insert at workflow LAUNCH, before the first child.
    /// Returns the controller so the caller can hold the abort handle without a second lookup.
    pub(crate) fn register_workflow_controller(
        &self,
        run_id: &RunId,
        abort: CancelToken,
    ) -> WorkflowController {
        let session_id =
            crate::identity::SessionId::parse_opt(self.current_session_id().as_deref());
        let controller = WorkflowController::new(abort, session_id);
        self.workflow_controllers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(run_id.clone(), controller.clone());
        // Observability only: how many workflow shells this process is now driving, read through
        // the same registry WORKFLOW_10's capacity gate will consult. The lock above was a
        // temporary bound to the `insert` statement and is already released, so this is a fresh,
        // independent acquisition — never nested under it (`std::sync::Mutex` is not reentrant).
        tracing::debug!(
            run_id = %run_id,
            live_workflow_controllers = self.live_workflow_run_ids().len(),
            "registered workflow controller"
        );
        controller
    }

    /// pi `subagent-executor.ts:5272` (rollback) and `:5796` (the settlement `finally`) — the SAME
    /// removal on both paths, which is why there is one function and not two.
    ///
    /// ⚠ Must run in a `finally`-equivalent position. Upstream's own comment at `:5789` —
    /// *"Idempotent cleanup only"* — is the contract: a leaked entry makes WORKFLOW_10 over-count
    /// live workflows forever and makes WORKFLOW_8's dismiss refusal permanent for that id.
    pub(crate) fn settle_workflow_controller(&self, run_id: &RunId) -> Option<WorkflowController> {
        self.workflow_controllers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id)
    }

    /// pi `new Set(state.workflowControllers?.keys() ?? [])` — the `liveWorkflowRunIds` set
    /// WORKFLOW_10 feeds to `inspectActiveAsyncCapacityOwner`. Returns a [`HashSet`], the set type
    /// those call sites want, not a `Vec` they would each have to re-collect.
    pub(crate) fn live_workflow_run_ids(&self) -> HashSet<RunId> {
        self.workflow_controllers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// pi `state.workflowControllers?.has(runId)` (`workflow-foreground-steering.ts:23`) —
    /// WORKFLOW_7's first gate (`active_workflow_error`), checked BEFORE any disk read: a workflow
    /// this process is not driving right now never costs a `status.json` parse.
    pub(crate) fn has_workflow_controller(&self, run_id: &RunId) -> bool {
        self.workflow_controllers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(run_id)
    }

    /// Resolve `id` against the live workflow-controller registry, EXACT MATCH ONLY (WORKFLOW_7
    /// §1.5's routing gate). Never prefix-matched: prefix matching is `foreground_controls`'s own
    /// affordance (`mod.rs:139-143`'s field doc gives the reason), and a workflow id that
    /// prefix-matched a child id here would route a workflow steer at the wrong target.
    pub(crate) fn live_workflow_run_id_for(&self, id: &str) -> Option<RunId> {
        self.workflow_controllers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .find(|run_id| run_id.as_str() == id)
            .cloned()
    }

    /// pi `extension/index.ts:1038-1042` — abort every live controller, THEN clear, THEN clear the
    /// SIBLING child-stop map.
    ///
    /// Order is upstream's and is load-bearing: *"Workflow continuations retain their launch
    /// context; abort them before teardown so a reload cannot launch through a stale context."*
    /// Clearing first would drop the only handle able to stop them.
    ///
    /// WORKFLOW_18 — `state.workflowChildStops?.clear()` (`extension/index.ts:1042`) is the very
    /// next statement upstream, and it belongs here rather than at the `teardown_session` call
    /// site: a teardown that cleared only this map would leave every stop handle holding its run's
    /// whole `Arc<RunShared>` for the life of the process
    /// (see [`super::workflow_child_stops`]'s lifetime contract).
    pub(crate) fn abort_and_clear_workflow_controllers(&self) {
        let mut controllers = self
            .workflow_controllers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = crate::time::now_epoch_millis();
        for (run_id, controller) in controllers.iter() {
            // Observability for the abort-before-clear teardown sweep: which workflow, whose
            // session, and how long it had been running — the three facts `WorkflowController`
            // exists to carry so an in-process abort does not have to re-read `status.json` for
            // them (the same three a future WORKFLOW_9 detach-reconciliation report would want).
            tracing::debug!(
                run_id = %run_id,
                session_id = ?controller.session_id().map(crate::identity::SessionId::as_str),
                age_ms = now.saturating_sub(controller.started_at()),
                already_aborted = controller.is_aborted(),
                "aborting live workflow controller at session teardown"
            );
            controller.abort();
        }
        controllers.clear();
        // Released BEFORE the child-stop map is taken: `std::sync::Mutex` is not reentrant and
        // these are two independent locks, so the second acquisition must be a separate,
        // un-nested statement — the same point this file's `register_workflow_controller` makes
        // about its own `live_workflow_run_ids()` call.
        drop(controllers);
        self.clear_workflow_child_stops();
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

    /// Register then settle: the registry key/remove pair is exact — no leak, no double-remove
    /// panic, and the returned controller shares the SAME abort token as the caller's.
    #[test]
    fn register_then_settle_round_trips_the_same_controller() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        let abort = CancelToken::new();

        let controller = executor.register_workflow_controller(&run_id, abort.clone());
        assert!(executor.live_workflow_run_ids().contains(&run_id));
        assert!(!controller.is_aborted());

        abort.cancel();
        assert!(
            controller.is_aborted(),
            "the registered controller must share the caller's own token"
        );

        let settled = executor
            .settle_workflow_controller(&run_id)
            .expect("a registered controller settles once");
        assert!(settled.is_aborted());
        assert!(!executor.live_workflow_run_ids().contains(&run_id));

        // Idempotent: settling an already-settled (or never-registered) id is `None`, not a panic.
        assert!(executor.settle_workflow_controller(&run_id).is_none());
    }

    /// `live_workflow_run_ids` reflects exactly what THIS process is driving right now — never a
    /// settled id.
    #[test]
    fn live_workflow_run_ids_tracks_registration_and_settlement() {
        let executor = SubagentExecutor::new();
        let a = RunId::new();
        let b = RunId::new();
        executor.register_workflow_controller(&a, CancelToken::new());
        executor.register_workflow_controller(&b, CancelToken::new());

        let live = executor.live_workflow_run_ids();
        assert!(live.contains(&a) && live.contains(&b));
        assert_eq!(live.len(), 2);

        executor.settle_workflow_controller(&a);
        let live = executor.live_workflow_run_ids();
        assert!(!live.contains(&a));
        assert!(live.contains(&b));
    }

    /// Teardown aborts every live controller BEFORE clearing the map — the order upstream's own
    /// comment names as load-bearing.
    #[tokio::test]
    async fn abort_and_clear_workflow_controllers_aborts_before_clearing() {
        let executor = SubagentExecutor::new();
        let a = RunId::new();
        let b = RunId::new();
        let abort_a = CancelToken::new();
        let abort_b = CancelToken::new();
        executor.register_workflow_controller(&a, abort_a.clone());
        executor.register_workflow_controller(&b, abort_b.clone());

        executor.abort_and_clear_workflow_controllers();

        assert!(
            abort_a.is_cancelled(),
            "every live controller must be aborted"
        );
        assert!(
            abort_b.is_cancelled(),
            "every live controller must be aborted"
        );
        assert!(
            executor.live_workflow_run_ids().is_empty(),
            "the map must be cleared after abort"
        );
    }

    /// `has_workflow_controller` is a plain registry membership probe: `true` only while THIS
    /// process holds the controller, never before registration or after settlement.
    #[test]
    fn has_workflow_controller_tracks_registration_and_settlement() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        assert!(!executor.has_workflow_controller(&run_id));

        executor.register_workflow_controller(&run_id, CancelToken::new());
        assert!(executor.has_workflow_controller(&run_id));

        executor.settle_workflow_controller(&run_id);
        assert!(!executor.has_workflow_controller(&run_id));
    }

    /// `live_workflow_run_id_for` is EXACT MATCH ONLY — a prefix of a live workflow id must not
    /// resolve, unlike `foreground_controls`'s own prefix-tolerant lookup.
    #[test]
    fn live_workflow_run_id_for_is_exact_match_only() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        executor.register_workflow_controller(&run_id, CancelToken::new());

        assert_eq!(
            executor.live_workflow_run_id_for(run_id.as_str()),
            Some(run_id.clone())
        );
        let prefix = &run_id.as_str()[..8];
        assert_eq!(
            executor.live_workflow_run_id_for(prefix),
            None,
            "a prefix of a live workflow id must not resolve — exact match only"
        );
        assert_eq!(executor.live_workflow_run_id_for("not-a-workflow"), None);
    }

    /// `register_workflow_controller` records the CURRENT session id, not a stamp captured once —
    /// the same discipline `ForegroundControlEntry::session_id` follows (WORKFLOW_6 SUBTASK1).
    #[test]
    fn register_workflow_controller_carries_the_current_session_id() {
        let executor = SubagentExecutor::new();
        executor.set_host_services(std::sync::Arc::new(
            crate::extension::testsupport::FixedSessionIdHost {
                id: Some("session-a".to_string()),
                file: None,
            },
        ));
        let run_id = RunId::new();
        let controller = executor.register_workflow_controller(&run_id, CancelToken::new());
        assert_eq!(
            controller.session_id().map(|s| s.as_str()),
            Some("session-a")
        );
        assert!(controller.started_at() > 0);
    }
}
