//! The live in-process workflow CHILD-STOP registry (WORKFLOW_18) — pi's
//! `state.workflowChildStops: Map<string, (childId: string, message?: string) => boolean>`
//! (`shared/types.ts:2315-2316`; constructed `subagent-executor.ts:4958`, defaulted `:5226`,
//! written by the registrar at `:5691-5694`, deleted at settlement `:5927`, cleared at teardown
//! `extension/index.ts:1042`).
//!
//! This is the SIBLING of [`super::workflow_controllers`]'s `state.workflowControllers`, and it is
//! deliberately a SEPARATE map for the reason upstream keeps them separate: a controller aborts a
//! WORKFLOW, a stop handle stops ONE CHILD of one. Conflating them would make "stop child b" abort
//! the whole run — the all-or-nothing granularity this task exists to end.
//!
//! # This module writes no stop logic
//!
//! The engine already builds the complete stop closure and hands it to a registrar
//! (`workflows/scripted/engine.rs`'s `register_stop_child` seam): under the run lock it refuses an
//! unknown or already-settled key, inserts a `stopped_child_result` into `children`, cancels the
//! child's own stop token, pushes a `Stopped` trace entry and bumps the run. This module STORES
//! that handle and CALLS it. The `bool` it returns is the engine's own truth, not a re-derivation.
//!
//! # ⚠ The lifetime contract
//!
//! The registrar is called with `Some(stop)` once before the run and `None` at settlement — and the
//! engine's own comment says why the `None` arm is placed before anything that can block: *"release
//! the stop closure BEFORE anything that can block. It captures the run state, so a slow drain
//! would otherwise keep the whole partial reachable from the embedder."*
//!
//! So [`SubagentExecutor::clear_workflow_child_stop`] MUST **remove** the entry, never tombstone it
//! or flip a flag. A registry that only ever inserts pins the run's `Arc<RunShared>` — every child
//! result, every trace entry, every console line — for the life of the process. Upstream's own
//! registrar spells this as `delete` (`subagent-executor.ts:5693`), and its settlement tail deletes
//! from BOTH maps unconditionally (`:5926-5927`) rather than trusting the registrar to have run.
//!
//! Two more constraints inherited from the same seam:
//!
//! * the handle is **synchronous** and is invoked from arbitrary host threads on the run-mutex
//!   path — no `.await`, no file I/O, and no lock ordering against `foreground_controls` may be
//!   introduced around it. Every method below therefore releases this registry's lock before
//!   calling (or dropping) a handle;
//! * the returned `bool` is **advisory**: `false` means "nothing live under that key", not an
//!   error.

use std::path::Path;

use crate::background::RunId;
use crate::extension::executor::SubagentExecutor;
use crate::workflows::scripted::WorkflowStopChild;

impl SubagentExecutor {
    /// pi's registrar `Some` arm (`subagent-executor.ts:5692`:
    /// `deps.state.workflowChildStops?.set(workflowRunId, stop)`) — insert at workflow LAUNCH,
    /// before the first child, from the `register_stop_child` callback the workflow dispatch
    /// supplies.
    ///
    /// Re-registering the same run id replaces the handle rather than stacking a second one: the
    /// engine hands out exactly one closure per run, so a second `Some` for a live id can only be a
    /// re-entry of the same seam, and the newest closure is the one that owns the live run state.
    pub(crate) fn register_workflow_child_stop(&self, run_id: &RunId, stop: WorkflowStopChild) {
        self.workflow_child_stops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(run_id.clone(), stop);
    }

    /// pi's registrar `None` arm (`subagent-executor.ts:5693`:
    /// `deps.state.workflowChildStops?.delete(workflowRunId)`) and its settlement tail (`:5927`).
    ///
    /// ⚠ **`remove`, never a tombstone** — see the module doc's lifetime contract. This is the one
    /// method whose *implementation*, not just its behaviour, is part of this task's definition of
    /// done: the removed handle is the only thing still holding the engine's run state once the run
    /// has settled.
    ///
    /// Idempotent, like [`SubagentExecutor::settle_workflow_controller`]: the engine's `None` arm
    /// and the dispatch site's belt-and-braces cleanup both call it for the same id, and a run that
    /// panicked before the `None` arm ran is cleaned up by the latter alone.
    pub(crate) fn clear_workflow_child_stop(&self, run_id: &RunId) {
        // Bound to a `let`, so the removed `Arc<dyn Fn…>` — which captures the engine's whole
        // `RunShared` — outlives the guard: the guard is a temporary of this statement and is
        // released at its semicolon, while `removed` is dropped on the line below, with no lock
        // held. Dropping the last reference to a run's state is embedder-reachable work, and the
        // module doc's "no lock ordering around a handle" rule covers releasing one as much as
        // calling one.
        let removed = self
            .workflow_child_stops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(run_id);
        drop(removed);
    }

    /// Call the live handle for `run_id`, if this process holds one: pi
    /// `stopChild(resolution.child.id, message)` (`subagent-executor.ts:6694`).
    ///
    /// `key` is the workflow LAUNCH KEY — the same string `runs.run("a", …)` names and the engine
    /// keys `launches`/`children`/`child_stop_tokens` by. `reason` is the stop message; `None`
    /// takes the engine's own default (`"Workflow child '{key}' stopped by user."`), which is what
    /// `control_stop` passes because neither it nor upstream's stop action carries a caller reason.
    ///
    /// Returns the engine's own verdict, verbatim: `true` when the key named a live launch that was
    /// stopped, `false` for an unknown key, an already-settled one, or a run this process holds no
    /// handle for at all. The three collapse into one `false` on purpose — from the caller's side
    /// they are one fact, exactly the collapse `no_live_child` makes one file away.
    pub(crate) fn stop_workflow_child(
        &self,
        run_id: &RunId,
        key: &str,
        reason: Option<&str>,
    ) -> bool {
        // Cloned OUT of the map, and the lock released, BEFORE the handle runs. The closure takes
        // the engine's own run mutex; holding this registry's lock across that call would establish
        // an ordering between two unrelated mutexes that every other caller of either would then
        // have to honour.
        let stop = self
            .workflow_child_stops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .cloned();
        match stop {
            Some(stop) => stop(key, reason),
            None => false,
        }
    }

    /// pi `state.workflowChildStops?.clear()` (`extension/index.ts:1042`) — the teardown half,
    /// called from [`SubagentExecutor::abort_and_clear_workflow_controllers`] so the two sibling
    /// maps are torn down together and a session teardown cannot leave the leak the module doc
    /// describes.
    ///
    /// There is no abort-then-clear ordering to honour here, unlike the controller map: a stop
    /// handle stops a CHILD on request and holds no authority that outlives the map entry, so
    /// upstream simply clears it after aborting the controllers.
    pub(crate) fn clear_workflow_child_stops(&self) {
        // Taken out whole, then dropped outside the lock — same reasoning as
        // `clear_workflow_child_stop`, multiplied by however many workflows this session was
        // driving.
        let drained = {
            let mut stops = self
                .workflow_child_stops
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *stops)
        };
        drop(drained);
    }

    /// WORKFLOW_18 §2.3 — the `action: "stop"` workflow-child action, the stop-side twin of
    /// [`SubagentExecutor::steer_workflow_foreground`]: pi's `action === "stop"` child branch
    /// (`subagent-executor.ts:6668-6704`).
    ///
    /// The four gates are NOT re-implemented here. `active_workflow_error` is run first and whole,
    /// in its own file, for the reason the spec makes non-negotiable: its fourth gate is
    /// `SessionGate::Strict`, and stopping another session's child is exactly the cross-session
    /// hazard that gate exists to prevent. Upstream gates this path on registry membership ALONE
    /// (`:6669`); running the full steering gate here is a deliberate cyrup hardening, not an
    /// oversight.
    ///
    /// One consequence is worth naming rather than hiding: `active_workflow_error`'s refusals are
    /// worded for steering (`"Workflow steering requires an active parent session."`, and
    /// `no_live_child`'s `"Workflow '{id}' has no live foreground child."`). They are upstream's
    /// verbatim sentences for "this process is not driving that workflow right now", which is the
    /// same fact whichever verb asked — so they are reused as-is rather than forked into a second,
    /// drifting copy.
    ///
    /// # Errors
    ///
    /// Returns `Err` with `active_workflow_error`'s own refusal when the workflow is not a live,
    /// this-session workflow, or with the no-live-child sentence when the engine refuses the key.
    pub(crate) async fn stop_workflow_child_action(
        &self,
        workflow_run_id: &RunId,
        key: &str,
        reason: Option<&str>,
        async_root: &Path,
    ) -> Result<String, String> {
        self.active_workflow_error(workflow_run_id, async_root)
            .await?;
        if self.stop_workflow_child(workflow_run_id, key, reason) {
            Ok(format!("Stopped workflow {workflow_run_id} child '{key}'."))
        } else {
            // `false` = unknown key, OR already settled, OR no handle for this run (the engine's
            // own gate, plus the registry miss). One message, because from the caller's side they
            // are one fact — the same collapse `no_live_child` makes.
            Err(format!(
                "Workflow '{workflow_run_id}' has no live child '{key}'."
            ))
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

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use cyrup_core::CancelToken;
    use serde_json::{Map, Value, json};

    use super::*;
    use crate::extension::testsupport::FixedSessionIdHost;
    use crate::identity::SessionId;
    use crate::workflows::WorkflowScriptChildResult;
    use crate::workflows::scripted::{
        RunWorkflowScriptOptions, WorkflowLaunchAdmission, WorkflowScriptHost,
        WorkflowStopChildRegistrar, run_workflow_script,
    };

    fn with_session(executor: &SubagentExecutor, session_id: &str) {
        executor.set_host_services(Arc::new(FixedSessionIdHost {
            id: Some(session_id.to_string()),
            file: None,
        }));
    }

    /// Every `(key, reason)` a [`recording_stop`] handle was called with, in order.
    type StopCalls = Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>;

    /// A stop handle that records every `(key, reason)` it was called with and answers with a
    /// fixed verdict — standing in for the engine's closure, whose `bool` this registry only ever
    /// forwards.
    fn recording_stop(calls: &StopCalls, verdict: bool) -> WorkflowStopChild {
        let calls = Arc::clone(calls);
        Arc::new(move |key: &str, reason: Option<&str>| {
            calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((key.to_string(), reason.map(ToString::to_string)));
            verdict
        })
    }

    /// Writes a `status.json` reflecting a live, RUNNING workflow run owned by `session_id`, so
    /// `active_workflow_error`'s on-disk gate has something real to observe. Mirrors
    /// `workflow_steering.rs`'s helper of the same name.
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

    /// A live, this-session workflow: controller registered AND a running `status.json` on disk,
    /// which is everything `active_workflow_error`'s four gates read.
    fn live_workflow(executor: &SubagentExecutor, async_root: &std::path::Path) -> RunId {
        with_session(executor, "session-a");
        let run_id = RunId::new();
        executor.register_workflow_controller(&run_id, CancelToken::new());
        write_running_workflow_status(async_root, &run_id, Some("session-a"));
        run_id
    }

    /// Register then clear: the registry key/remove pair is exact, and the CLEARED id no longer
    /// reaches its handle — the leak check §1 makes part of the definition of done. The handle
    /// here always answers `true`, so a `false` after the clear can only mean the entry is GONE,
    /// never that a tombstoned handle answered.
    #[test]
    fn clear_workflow_child_stop_removes_the_handle_rather_than_tombstoning_it() {
        let executor = SubagentExecutor::new();
        let run_id = RunId::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));

        executor.register_workflow_child_stop(&run_id, recording_stop(&calls, true));
        assert!(executor.stop_workflow_child(&run_id, "a", None));

        executor.clear_workflow_child_stop(&run_id);
        assert!(
            !executor.stop_workflow_child(&run_id, "a", None),
            "a cleared run id must not reach its handle — the handle itself always answers true"
        );
        // Idempotent: clearing an already-cleared (or never-registered) id is a no-op, not a panic.
        executor.clear_workflow_child_stop(&run_id);
        executor.clear_workflow_child_stop(&RunId::new());

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            1,
            "only the pre-clear call may reach the handle: {calls:?}"
        );
    }

    /// The registry forwards the engine's verdict verbatim — both ways — and never invents one.
    #[test]
    fn stop_workflow_child_forwards_the_engine_verdict_and_the_reason() {
        let executor = SubagentExecutor::new();
        let stopped = RunId::new();
        let refused = RunId::new();
        let stopped_calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let refused_calls = Arc::new(std::sync::Mutex::new(Vec::new()));

        executor.register_workflow_child_stop(&stopped, recording_stop(&stopped_calls, true));
        executor.register_workflow_child_stop(&refused, recording_stop(&refused_calls, false));

        assert!(executor.stop_workflow_child(&stopped, "a", Some("not needed")));
        assert!(!executor.stop_workflow_child(&refused, "a", Some("not needed")));
        // A run this process holds no handle for is the same `false`, not a panic or a hang.
        assert!(!executor.stop_workflow_child(&RunId::new(), "a", None));

        assert_eq!(
            stopped_calls.lock().unwrap().as_slice(),
            [("a".to_string(), Some("not needed".to_string()))],
            "key and reason must reach the handle untouched"
        );
        // `None` stays `None` all the way down: the DEFAULT message is the engine's to choose
        // (`"Workflow child '{key}' stopped by user."`), never this registry's.
        assert!(executor.stop_workflow_child(&stopped, "b", None));
        assert_eq!(stopped_calls.lock().unwrap()[1], ("b".to_string(), None));
    }

    /// Session teardown clears BOTH sibling maps — pi `extension/index.ts:1041-1042`. A cleared
    /// controller map with a live child-stop map is exactly the leak §1 describes.
    #[test]
    fn session_teardown_clears_the_child_stop_map_with_the_controllers() {
        let executor = SubagentExecutor::new();
        let a = RunId::new();
        let b = RunId::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        executor.register_workflow_controller(&a, CancelToken::new());
        executor.register_workflow_controller(&b, CancelToken::new());
        executor.register_workflow_child_stop(&a, recording_stop(&calls, true));
        executor.register_workflow_child_stop(&b, recording_stop(&calls, true));

        executor.abort_and_clear_workflow_controllers();

        assert!(executor.live_workflow_run_ids().is_empty());
        assert!(
            !executor.stop_workflow_child(&a, "a", None)
                && !executor.stop_workflow_child(&b, "b", None),
            "teardown must clear the child-stop map too, not just the controller map"
        );
        assert!(calls.lock().unwrap().is_empty());
    }

    /// Gate reuse, arm 1: no active parent session refuses BEFORE the registry is consulted — the
    /// handle is registered and would answer `true`, so only the gate can produce this refusal.
    #[tokio::test]
    async fn stop_workflow_child_action_refuses_without_an_active_session() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = RunId::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        executor.register_workflow_child_stop(&run_id, recording_stop(&calls, true));

        let err = executor
            .stop_workflow_child_action(&run_id, "a", None, dir.path())
            .await
            .expect_err("no session must refuse");
        assert_eq!(err, "Workflow steering requires an active parent session.");
        assert!(calls.lock().unwrap().is_empty(), "the gate runs FIRST");
    }

    /// Gate reuse, arm 2: a workflow this process is not driving is refused before any disk read.
    #[tokio::test]
    async fn stop_workflow_child_action_refuses_an_unregistered_workflow() {
        let executor = SubagentExecutor::new();
        with_session(&executor, "session-a");
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = RunId::new();

        let err = executor
            .stop_workflow_child_action(&run_id, "a", None, dir.path())
            .await
            .expect_err("an unregistered workflow must refuse");
        assert_eq!(
            err,
            format!("Workflow '{run_id}' has no live foreground child.")
        );
    }

    /// Gate reuse, arm 3 — the one the spec calls non-negotiable: `SessionGate::Strict`. Another
    /// session's live workflow is refused even though THIS process holds both its controller and
    /// its stop handle.
    #[tokio::test]
    async fn stop_workflow_child_action_refuses_another_sessions_workflow() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        with_session(&executor, "session-a");
        let run_id = RunId::new();
        executor.register_workflow_controller(&run_id, CancelToken::new());
        write_running_workflow_status(dir.path(), &run_id, Some("session-b"));
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        executor.register_workflow_child_stop(&run_id, recording_stop(&calls, true));

        let err = executor
            .stop_workflow_child_action(&run_id, "a", None, dir.path())
            .await
            .expect_err("a cross-session stop must refuse");
        assert_eq!(
            err,
            format!("Workflow '{run_id}' was not found in the active session.")
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "the strict session gate must run before the handle"
        );
    }

    /// The two answers a gated, live workflow can give, in the spec's own words.
    #[tokio::test]
    async fn stop_workflow_child_action_reports_the_engine_verdict() {
        let executor = SubagentExecutor::new();
        let dir = tempfile::tempdir().expect("tempdir");
        let run_id = live_workflow(&executor, dir.path());
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));

        executor.register_workflow_child_stop(&run_id, recording_stop(&calls, true));
        assert_eq!(
            executor
                .stop_workflow_child_action(&run_id, "a", Some("not needed"), dir.path())
                .await,
            Ok(format!("Stopped workflow {run_id} child 'a'."))
        );

        // An unknown or already-settled key is ONE refusal, because it is one fact.
        executor.register_workflow_child_stop(&run_id, recording_stop(&calls, false));
        assert_eq!(
            executor
                .stop_workflow_child_action(&run_id, "gone", None, dir.path())
                .await,
            Err(format!("Workflow '{run_id}' has no live child 'gone'."))
        );
    }

    /// A launcher that holds one named key until its own stop token fires and settles every other
    /// key immediately — so a stop delivered through this registry lands on a child that is
    /// genuinely in flight, and the children after it still run.
    struct HoldingHost {
        hold_key: String,
        started: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl WorkflowScriptHost for HoldingHost {
        async fn launch(
            &self,
            key: &str,
            _params: Map<String, Value>,
            cancel: CancelToken,
            _admission: WorkflowLaunchAdmission,
        ) -> Result<WorkflowScriptChildResult, String> {
            self.started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(key.to_string());
            if key == self.hold_key {
                // Bounded, so a regression that never delivers the stop fails the test instead of
                // hanging the suite.
                tokio::select! {
                    () = tokio::time::sleep(std::time::Duration::from_secs(20)) => {}
                    () = cancel.cancelled() => {}
                }
            }
            Ok(WorkflowScriptChildResult {
                key: key.to_string(),
                ok: true,
                output: format!("output of {key}"),
                ..Default::default()
            })
        }

        async fn status(
            &self,
            key_or_run_id: &str,
            _cancel: CancelToken,
        ) -> Result<WorkflowScriptChildResult, String> {
            Ok(WorkflowScriptChildResult {
                key: key_or_run_id.to_string(),
                ok: true,
                output: "running".into(),
                ..Default::default()
            })
        }
    }

    /// End to end, through the REAL engine and the REAL registrar (§3's observable contract):
    /// a live key stops and yields the engine's `stopped_child_result`, the workflow itself keeps
    /// running and completes, a settled key and an unknown key are both `false`, and settlement
    /// runs the registrar's `None` arm so the map holds nothing afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_workflow_stops_its_own_child_and_keeps_running() {
        let executor = Arc::new(SubagentExecutor::new());
        let run_id = RunId::new();
        let registered = Arc::new(AtomicUsize::new(0));
        let cleared = Arc::new(AtomicUsize::new(0));

        // The same two arms `routing.rs` supplies, written the same way — this test is only
        // meaningful if the registrar under test is the one production uses.
        let register_stop_child: WorkflowStopChildRegistrar = {
            let executor = Arc::clone(&executor);
            let run_id = run_id.clone();
            let registered = Arc::clone(&registered);
            let cleared = Arc::clone(&cleared);
            Arc::new(move |stop: Option<WorkflowStopChild>| match stop {
                Some(stop) => {
                    registered.fetch_add(1, Ordering::SeqCst);
                    executor.register_workflow_child_stop(&run_id, stop);
                }
                None => {
                    cleared.fetch_add(1, Ordering::SeqCst);
                    executor.clear_workflow_child_stop(&run_id);
                }
            })
        };

        let started = Arc::new(std::sync::Mutex::new(Vec::new()));
        let host = Arc::new(HoldingHost {
            hold_key: "a".to_string(),
            started: Arc::clone(&started),
        });
        // `b` runs AFTER `a` settles, so its `ok` is the proof the workflow survived the stop.
        let script = r#"
try { await runs.run("a", { agent: "worker", task: "A" }); }
catch (error) { console.log("a threw", String(error && error.message || error)); }
const b = await runs.run("b", { agent: "worker", task: "B" });
return { bOk: b.ok };
"#;
        let run = tokio::spawn(run_workflow_script(RunWorkflowScriptOptions {
            script: script.to_string(),
            one_use_permit: None,
            timeout_ms: Some(120_000),
            cancel: None,
            continue_after_abort_when_children_settled: None,
            global_concurrency_limit: None,
            host,
            state: None,
            register_stop_child: Some(register_stop_child),
            on_trace: None,
            on_lane_plan: None,
            on_emit: None,
            on_host_step: None,
        }));

        // The handle is registered before the script runs, but `a` is not a LIVE LAUNCH until the
        // script reaches `runs.run("a", …)` — until then the engine's own gate answers `false`.
        // Poll for the transition rather than sleeping a guessed interval.
        let mut stopped = false;
        for _ in 0..1_000 {
            if executor.stop_workflow_child(&run_id, "a", Some("not needed")) {
                stopped = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(stopped, "the live key 'a' must stop through the registry");

        // Already settled — the engine's `children.contains_key(key)` gate, through the same
        // handle that answered `true` a moment ago.
        assert!(
            !executor.stop_workflow_child(&run_id, "a", Some("not needed")),
            "a settled key must answer false"
        );
        assert!(
            !executor.stop_workflow_child(&run_id, "never-launched", None),
            "an unknown key must answer false"
        );

        let result = run.await.expect("join").expect("the workflow completes");

        // The workflow itself still completes, and the child after the stopped one still ran.
        assert_eq!(result.value, json!({ "bOk": true }));
        assert_eq!(
            started.lock().unwrap().as_slice(),
            ["a".to_string(), "b".to_string()]
        );

        // The engine's own evidence: `stopped: true`, carrying the reason this registry forwarded.
        let a = result
            .children
            .iter()
            .find(|child| child.key == "a")
            .expect("the stopped child is still reported");
        assert!(a.stopped, "the stopped child's result must say so");
        assert!(!a.ok);
        assert_eq!(a.output, "not needed");
        assert_eq!(a.error.as_deref(), Some("not needed"));
        assert!(
            result
                .children
                .iter()
                .any(|child| child.key == "b" && child.ok)
        );

        // Settlement ran the registrar's `None` arm exactly once, and the map is empty after it.
        assert_eq!(registered.load(Ordering::SeqCst), 1);
        assert_eq!(cleared.load(Ordering::SeqCst), 1);
        assert!(
            !executor.stop_workflow_child(&run_id, "a", None),
            "the handle must be gone once the run has settled"
        );
    }
}
