//! The ONE headless-capable workflow launch path — extracted from
//! [`crate::extension::tool::SubagentTool::route_workflow_mode`] so that a surface with no tool
//! call, no UI sink and no live session read can still produce a REAL `RunMode::Workflow` run.
//!
//! # Why this exists
//!
//! `route_workflow_mode` was the only production emitter of [`crate::background::RunMode::Workflow`],
//! and every WORKFLOW_17-21 behaviour rides it: the controller registration that makes
//! `action: "interrupt"` reach a live workflow, `register_stop_child`, the `on_emit` forwarder, the
//! `on_host_step` accumulator, and the terminal receipt write without which a run would strand in
//! `Running` forever. SUBA-016's scheduled runs need all of that and none of the tool-call inputs,
//! so the block was extracted VERBATIM rather than re-implemented — a second launch path would
//! diverge on the first of those five behaviours that someone forgot.
//!
//! The split is by PHASE, not by caller:
//!
//! * [`prepare_workflow_run`] mints the run id, creates the run directory and publishes a `Running`
//!   `status.json`. It returns as soon as the run is addressable on disk, which is what lets a
//!   scheduled fire record `async_id`/`async_dir` and return while the run is still going.
//! * [`drive_workflow_run`] registers the controller, runs the engine and settles. A foreground
//!   tool call awaits it; a scheduled fire spawns it.
//!
//! `[CYRUP-DELTA]` pi has no analog of this split: its scheduled fire goes through
//! `deps.launch(executionParams(schedule), …)` into the ordinary async spawn path
//! (`scheduled-runs.ts:876`), because upstream CAN run a `workflowScript` asynchronously. cyrup
//! cannot — `routing.rs`'s async refusal is permanent and
//! `background/state.rs:22-37` records that the background runner never emits
//! `RunMode::Workflow` — so "detached" here means *this process, a spawned tokio task*, which is
//! exactly what the `pid`/`register_workflow_controller` pair above already assumes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, TerminateHint, ToolError, ToolResult, ToolUpdateSink};

use crate::background::RunId;
use crate::extension::executor::SubagentExecutor;
use crate::identity::SessionId;

/// Everything [`prepare_workflow_run`] needs that differs between a tool call and a scheduled fire.
pub(crate) struct PrepareWorkflowRun<'a> {
    /// The resolved config this dispatch already snapshotted — passed in rather than re-read so
    /// the run directory and the timeout ladder cannot resolve against two different snapshots.
    pub(crate) cfg: &'a crate::registration::SubagentExtensionConfig,
    /// The directory the workflow (and every child it launches) executes in.
    pub(crate) cwd: &'a Path,
    /// The session this run is attributed to.
    ///
    /// A VALUE, not a live read. The foreground caller passes
    /// `SessionId::parse_opt(executor.current_session_id().as_deref())`, which is exactly what the
    /// inline block did; a scheduled fire passes the identity PINNED at its binding
    /// ([`crate::background::scheduled_runs::ScheduleSessionSnapshot`]), because
    /// `current_session_id` is documented to read live off the P-1 backend on every call and a
    /// fire that read it three times could stamp three different identities onto one run.
    pub(crate) session_id: Option<SessionId>,
    /// The originating tool call, when there is one.
    ///
    /// `None` for a scheduled fire: there is no tool call, and `with_workflow_children`
    /// (`workflows/settlement.rs:102-105`) falls back to the run id, which the inline block's own
    /// comment already documents as the legal degradation.
    pub(crate) tool_call_id: Option<String>,
    /// The launching PROCESS, for delivery ownership.
    ///
    /// `None` on the foreground path, preserving the extracted block byte for byte: a tool call
    /// delivers its own result as the `ToolResult` it returns, so nothing ever consults the owner.
    /// A scheduled fire has no caller to return to and reaches its session through the completion
    /// pipeline, which is gated on this pair — see
    /// [`crate::background::delivery::OwnershipSnapshot`].
    pub(crate) completion_owner_id: Option<crate::identity::CompletionOwnerId>,
}

/// A workflow run that is addressable on disk and has not started executing.
pub(crate) struct PreparedWorkflowRun {
    workflow_run_id: RunId,
    run_dir_name: crate::identity::RunDirName,
    run_dir: PathBuf,
    async_root: PathBuf,
    status: Arc<Mutex<crate::background::RunStatus>>,
}

impl PreparedWorkflowRun {
    /// The run id every child will carry as its `parent_workflow_run_id`, and the key
    /// `action: "interrupt"` addresses.
    pub(crate) fn run_id(&self) -> &RunId {
        &self.workflow_run_id
    }

    /// The run directory `status.json` was just published into.
    pub(crate) fn run_dir(&self) -> &Path {
        &self.run_dir
    }
}

/// Everything [`drive_workflow_run`] needs that differs between a tool call and a scheduled fire.
pub(crate) struct DriveWorkflowRun<'a> {
    /// The config snapshot [`prepare_workflow_run`] was given.
    pub(crate) cfg: &'a crate::registration::SubagentExtensionConfig,
    /// The same cwd.
    pub(crate) cwd: &'a Path,
    /// The script body.
    pub(crate) script: &'a str,
    /// The resolved foreground timeout.
    pub(crate) timeout_ms: Option<u64>,
    /// WORKFLOW_20's mission scratchpad, when this dispatch bound a mission.
    ///
    /// Always `None` for a scheduled fire: upstream's `executionParams` sets `mission: false`
    /// (`scheduled-runs.ts:459`), and a schedule that bound a mission would keep writing into it
    /// long after the mission closed.
    pub(crate) state: Option<Arc<dyn crate::workflows::scripted::WorkflowStateStore>>,
    /// The progress sink. A scheduled fire passes `Box::new(|_| {})`: there is no UI listening,
    /// and [`crate::extension::executor::workflow::WorkflowRunHost`] requires one.
    pub(crate) on_update: ToolUpdateSink,
    /// The abort token `register_workflow_controller` is keyed on.
    pub(crate) cancel: CancelToken,
}

/// Phase 1 — mint the run, create its directory, publish `Running`.
///
/// # Errors
///
/// The run directory could not be created, the status could not advance, or the status write
/// failed. All three are environment faults and all three leave nothing half-built.
pub(crate) async fn prepare_workflow_run(
    request: PrepareWorkflowRun<'_>,
) -> Result<PreparedWorkflowRun, ToolError> {
    let PrepareWorkflowRun {
        cfg,
        cwd,
        session_id,
        tool_call_id,
        completion_owner_id,
    } = request;

    // WORKFLOW_6/WORKFLOW_3 §3d — ONE workflow run id for the whole call, minted BEFORE the
    // engine runs. It is three things at once and all three need it up front:
    //   1. the `workflow_controllers` key for the lifetime of this call (pi `:5095-5098`),
    //   2. every child's `parent_workflow_run_id` (pi `prepareWorkflowChildLaunchParams`,
    //      `:5633`/`:5900`, whose `parentWorkflowRunId: workflowRunId` reaches the child's
    //      control entry at `:7019`),
    //   3. the receipt's directory name and the id the receipt cross-checks itself against.
    // Pre-WORKFLOW_6 it was minted after `run_workflow_script` returned, which made (1) and (2)
    // impossible.
    let workflow_run_id = crate::background::RunId::new();
    let run_dir_name = crate::identity::RunDirName::for_run(&workflow_run_id);

    // The run dir the receipt was already being written into (`attach_workflow_receipt`), created up
    // front now, because it is a real run directory from this point on — not a drop box.
    // `ensure_accessible_dir` is REQUIRED, not defensive: `write_atomic_json` has no implicit
    // `mkdir -p` (`atomic.rs:105-107`).
    let async_root = crate::extension::executor::paths::default_async_root_in(&cfg.roots, cwd);
    let run_dir = run_dir_name.resolve_in(&async_root);
    crate::background::ensure_accessible_dir(&run_dir)
        .await
        .map_err(|e| ToolError::new(format!("workflow run directory could not be created: {e}")))?;

    // THE FIRST PRODUCTION `RunMode::Workflow` IN THE CRATE.
    let mut status = crate::background::RunStatus::queued(
        workflow_run_id.clone(),
        crate::background::RunMode::Workflow,
        // §2.1 — this process drives it; reconcile.rs step 4 must be able to probe us.
        Some(std::process::id()),
    );
    // The injected identity - see `PrepareWorkflowRun::session_id` for why this is a value
    // and not a live read.
    status.session_id = session_id;
    status.completion_owner_id = completion_owner_id;
    status.cwd = Some(cwd.to_path_buf());
    // §2.3 — the parent tool call id when there IS one. `with_workflow_children`
    // (settlement.rs:102-105) falls back to the run id when this is `None`, which is the legal
    // degradation a scheduled fire takes.
    status.tool_call_id = tool_call_id;
    // A workflow's step count is DISCOVERED, not declared — exactly what `state.rs:31`'s own doc says
    // distinguishes it from a Chain. Leave it None; §3.3 fills `steps` as children settle.
    status.chain_step_count = None;
    // ⚠ §2.5 — MUST happen before the engine runs. `Queued -> Complete` is not a legal transition,
    // so an instantly-succeeding workflow would fail its terminal write from `Queued`.
    status
        .advance_state(crate::background::RunState::Running)
        .map_err(|e| ToolError::new(e.to_string()))?;
    crate::background::atomic::write_atomic_json(&run_dir.join("status.json"), &status)
        .await
        .map_err(|e| ToolError::new(format!("workflow status could not be written: {e}")))?;

    // ONE record, shared. The host refreshes `steps` in place as children settle (§3.3) and
    // §3.2's terminal write starts from whatever the host last published — never from a
    // second, divergent copy that would silently drop the `last_update` the host advanced.
    // `std::sync::Mutex`: every access here is a short synchronous clone with no `.await`
    // inside the critical section (the host's own `publish_steps` is written to guarantee it).
    let status = std::sync::Arc::new(std::sync::Mutex::new(status));
    Ok(PreparedWorkflowRun {
        workflow_run_id,
        run_dir_name,
        run_dir,
        async_root,
        status,
    })
}

/// Phase 2 — register the controller, run the engine, settle.
///
/// # Errors
///
/// Every failure the foreground workflow arm already produced: an engine failure (carrying its
/// partial trace, children, console and emits), a receipt that could not be built, or a terminal
/// status that could not be written.
pub(crate) async fn drive_workflow_run(
    executor: &Arc<SubagentExecutor>,
    prepared: PreparedWorkflowRun,
    request: DriveWorkflowRun<'_>,
) -> Result<ToolResult, ToolError> {
    let PreparedWorkflowRun {
        workflow_run_id,
        run_dir_name,
        run_dir,
        async_root,
        status,
    } = prepared;
    let DriveWorkflowRun {
        cfg,
        cwd,
        script,
        timeout_ms,
        state,
        on_update,
        cancel,
    } = request;

    // pi `const controller = new AbortController(); state.workflowControllers.set(workflowRunId,
    // controller)` — with cyrup's ONE abort mechanism, which is the token already threaded into
    // `RunWorkflowScriptOptions::cancel` below, so an abort through the registry aborts the real
    // engine run.
    let _controller = executor.register_workflow_controller(&workflow_run_id, cancel.clone());

    let host = std::sync::Arc::new(crate::extension::executor::workflow::WorkflowRunHost::new(
        std::sync::Arc::clone(executor),
        cwd.to_path_buf(),
        // Moved, not cloned — `WorkflowRunHost::new` does the `Arc<Mutex<…>>` wrap that lets
        // each child borrow a forwarding share of this one sink.
        on_update,
        workflow_run_id.clone(),
        std::sync::Arc::clone(&status),
        run_dir.clone(),
        async_root.clone(),
    ));

    // Field order mirrors `RunWorkflowScriptOptions`' own declaration order, so this block can
    // be diffed against the struct line by line. The struct has no `Default`, so all thirteen
    // are named.
    let outcome = crate::workflows::scripted::run_workflow_script(
        crate::workflows::scripted::RunWorkflowScriptOptions {
            script: script.to_string(),
            // No resource provenance is wired in this build.
            one_use_permit: None,
            timeout_ms,
            // The workspace's ONE abort flag — no subsystem invents its own.
            cancel: Some(cancel),
            continue_after_abort_when_children_settled: None,
            // -> `DEFAULT_GLOBAL_CONCURRENCY_LIMIT` (`spawn/parallel.rs:36`, 20), which is
            // exactly what `count_requested_subagent_spawns` bills the session for.
            global_concurrency_limit: None,
            host: std::sync::Arc::clone(&host)
                as std::sync::Arc<dyn crate::workflows::scripted::WorkflowScriptHost>,
            // WORKFLOW_20 — `Some` exactly when this dispatch bound a mission, from the ONE
            // derivation above. `None` means `globalThis.state` is never installed in the
            // guest realm (`prelude.js:578`, via the engine's own `options.state.is_some()`),
            // which is why the analyzer already refused the script above rather than letting
            // the guest discover a bare `ReferenceError`; the engine's verbatim "Workflow
            // state is unavailable without a mission." op refusal stays unreachable defence in
            // depth behind both.
            state,
            // WORKFLOW_18 — pi `registerStopChild` (`subagent-executor.ts:5691-5694`). The
            // engine calls this with `Some(stop)` once before the run and `None` at
            // settlement, and BOTH arms are load-bearing: the `Some` arm is the only way an
            // operator can reach a single child of a live workflow (`control_stop`'s workflow
            // branch), and the `None` arm is what RELEASES the engine's captured run state —
            // the closure holds the whole `Arc<RunShared>`, so a registry that only ever
            // inserted would pin every child result, trace entry and console line of this run
            // for the life of the process.
            register_stop_child: Some({
                let executor = std::sync::Arc::clone(executor);
                let run_id = workflow_run_id.clone();
                std::sync::Arc::new(
                    move |stop: Option<crate::workflows::scripted::WorkflowStopChild>| match stop {
                        Some(stop) => executor.register_workflow_child_stop(&run_id, stop),
                        None => executor.clear_workflow_child_stop(&run_id),
                    },
                )
            }),
            // `None`, NOT a caller-side journal. The engine OWNS the trace and returns it
            // COMPLETE on both arms (`WorkflowScriptResult::trace`, and
            // `WorkflowScriptError::partial.trace` on failure). `on_trace` is pure telemetry —
            // its own doc says it "never decides workflow outcomes" — so accumulating from it
            // would be a second copy of data that is already handed back.
            on_trace: None,
            // `runs.lanes` still WORKS; only the advisory plan callback is undelivered.
            on_lane_plan: None,
            // WORKFLOW_21 — live `emit()` forwarding. This was `None` for TWO reasons, and both
            // are now answered rather than merely overruled.
            //
            // (a) WAS: `emit` handed the callback the WHOLE accumulated snapshot every time, so
            // a per-value forwarder re-forwarded everything on every emit, quadratically. GONE:
            // [`WorkflowEmitCallback`] is now `Fn(Value, usize)` — ONE value and its zero-based
            // index — which is the engine delta this wiring waited on. The index is what makes
            // the stream idempotent across the engine's own rollback: a failed append was not
            // persisted, so index N is free again.
            //
            // (b) WAS, and STILL IS: this is the ONE callback whose failure ABORTS the run
            // (`engine.rs`'s `emit` pops the value, records a fatal and returns `Err`). The
            // answer is not that the risk went away — it is that this forwarder HAS NO ERROR
            // PATH. A UI sink that cannot take an update is not a reason to kill a workflow,
            // and a progress sink has no concept of rejecting the emitted VALUE, which is the
            // only failure that would legitimately abort. So a poisoned lock DROPS the update
            // and the closure returns `Ok(())` unconditionally. ⚠ The next reader's instinct
            // will be to propagate — do not. Every `Err` returned from here is a dropped UI
            // update converted into a dead workflow.
            //
            // ⚠ This future is polled ON THE ISOLATE THREAD, inside a live deno op: the bridge
            // impl forwards `emit` straight to `RunShared::emit` without hopping through
            // `RunShared::main_handle` the way every other host call does. The whole body is
            // therefore one `std::sync::Mutex` lock and one synchronous `FnMut(ToolUpdate)`
            // call — the same discipline `child_sink` keeps, never held across an `.await`.
            // Anything slower here stalls the workflow AND the children already running, and a
            // `tokio::sync::Mutex` "to be safe" would put an `.await` inside the op. Routing
            // these through the `TelemetryEvent` drain (where `on_host_step` lives) is the
            // other tempting wrong fix: that queue exists for INFALLIBLE telemetry and would
            // silently discard the abort semantics upstream and cyrup both implement.
            //
            // The wording matches the settled `Emitted:` line this same function renders below,
            // down to the 200-char bound, so the live line and the receipt read alike.
            on_emit: Some({
                let sink = host.update_sink();
                std::sync::Arc::new(move |value: serde_json::Value, index: usize| {
                    let sink = std::sync::Arc::clone(&sink);
                    Box::pin(async move {
                        let preview =
                            crate::workflows::scripted::format_workflow_json_preview(&value, 200)
                                .unwrap_or_else(|| "undefined".to_string());
                        if let Ok(mut sink) = sink.lock() {
                            sink(cyrup_core::ToolUpdate {
                                content: vec![cyrup_core::Content::text(format!(
                                    "Workflow emit #{index}: {preview}"
                                ))],
                                // No `SubagentUpdatePayload` shape describes a workflow emit,
                                // and `render_subagent_result` falls back to the first text
                                // block when `details` does not parse as one — so `None` is
                                // both honest and renderable.
                                details: None,
                                terminate: cyrup_core::TerminateHint::Unspecified,
                            });
                        }
                        Ok(())
                    })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = Result<(), String>> + Send>,
                        >
                })
            }),
            // WORKFLOW_19 — `supports_host()` is now true, so host steps are produced and this
            // is the one way they reach the RUN. Neither `WorkflowScriptResult` nor
            // `WorkflowScriptPartial` carries a `host_steps` field, so unlike `on_trace`
            // (which is `None` precisely BECAUSE the trace comes back complete) leaving this
            // callback unsupplied silently drops every host step on the floor.
            //
            // Like `on_emit` since WORKFLOW_21, `on_host_step` delivers ONE node per call — but
            // unlike it, off the isolate thread through the telemetry drain, because a host
            // step is pure telemetry with no failure semantics. Accumulating is correct here —
            // and it is also the only option, per the note above.
            //
            // The accumulator is the RUN's own `status` record, not a side vector. It was a
            // side vector, read once at settlement for `BuildWorkflowReceipt::host_steps`, and
            // that is precisely why a host step reached the receipt but never the run: the
            // status is the ONLY artifact a later reader has (the receipt is a workflow-shaped
            // document, and `collect_fleet_history`/`RunStatus` readers do not open it). One
            // `RunStatus::record_host_step` per transition puts the nodes on
            // `status.telemetry.host_steps`, which `settle_foreground_workflow` then clones
            // into the terminal `status.json` on BOTH arms — and the receipt reads the same
            // list back out below, so the two evidence surfaces cannot disagree.
            //
            // ⚠ UPSERT BY ID, never push — `record_host_step` owns that rule and its own doc
            // argues it (the engine emits each command twice under one id, and
            // `assert_unique_host_step_ids` rejects the duplicate, turning a successful
            // workflow into a `ToolError` at `settle_foreground_workflow`'s receipt `map_err`).
            on_host_step: Some({
                let status = std::sync::Arc::clone(&status);
                std::sync::Arc::new(move |node: &crate::workflows::HostStepNode| {
                    // A poisoned lock is RECOVERED, not skipped: the guarded value is the run
                    // record a panicking peer cannot have left torn, and dropping the node
                    // would lose exactly the evidence both readers exist to carry.
                    status
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .record_host_step(node);
                })
            }),
        },
    )
    .await;

    // WORKFLOW_6 §4.4 — pi's settlement `finally` (`subagent-executor.ts:5789-5799`):
    // "Idempotent cleanup only". Unconditional and ahead of the Ok/Err match, because a
    // workflow that FAILED is exactly as settled as one that succeeded — leaving the entry
    // behind would make WORKFLOW_10 count a dead workflow forever and make WORKFLOW_8's
    // dismiss refusal permanent for this id.
    executor.settle_workflow_controller(&workflow_run_id);
    // WORKFLOW_18 — upstream's settlement tail deletes from BOTH maps unconditionally
    // (`subagent-executor.ts:5926-5927`), and so does this one. It is NOT a substitute for the
    // registrar's `None` arm above (that one runs inside the engine, deliberately ahead of any
    // drain that could block) and it is not redundant with it either: the `None` arm is skipped
    // on a panic path, and a stop handle left behind pins the run's whole `Arc<RunShared>`.
    // `clear_workflow_child_stop` is idempotent precisely so both can run.
    executor.clear_workflow_child_stop(&workflow_run_id);

    // WORKFLOW_19 — snapshot the accumulated evidence ONCE, here, for BOTH settlement arms.
    //
    // The engine flushes and joins its telemetry drain before `run_workflow_script` returns
    // (`engine.rs`'s `TelemetryEvent::Flush` + the bounded join on `telemetry_drain`), so every
    // node the run produced has already been delivered by this point — there is no late arrival
    // to race.
    let host_steps: Vec<crate::workflows::HostStepNode> = status
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .telemetry
        .host_steps
        .clone();

    match outcome {
        Ok(result) => {
            let preview =
                crate::workflows::scripted::format_workflow_json_preview(&result.value, 1_000)
                    .unwrap_or_else(|| "undefined".to_string());
            let mut text = format!(
                "Workflow completed with {} child run(s). Return: {preview}",
                result.children.len()
            );
            if !result.emits.is_empty() {
                let rendered: Vec<String> = result
                    .emits
                    .iter()
                    .map(|value| {
                        crate::workflows::scripted::format_workflow_json_preview(value, 200)
                            .unwrap_or_else(|| "undefined".to_string())
                    })
                    .collect();
                text.push_str(&format!(" Emitted: {}", rendered.join(", ")));
            }
            text.push_str(&format!(" Trace: {} event(s).", result.trace.len()));
            // §3.2: settle via the composer (first production caller of plan_workflow_settlement).
            let details = settle_foreground_workflow(
                serde_json::json!({
                    // The inline surface's typed payloads
                    // (`IntercomPayload::from_group_children`) are keyed to
                    // `StepResult`/`SingleResult`, not to `WorkflowScriptChildResult`, and
                    // no conversion exists — so v1 attaches the children as plain JSON
                    // rather than inventing a lossy `StepResult` shim that a follow-up
                    // task would have to unpick. `WorkflowScriptChildResult` derives
                    // `Serialize` with `rename_all = "camelCase"`, so this is upstream's
                    // own wire shape. `RunMode::Workflow` already exists and is already
                    // rendered as "workflow" elsewhere, so the mode label is the real one,
                    // not a placeholder.
                    "mode": "workflow",
                    // Cloned: `&result.children` is also borrowed below, for the receipt.
                    "children": result.children.clone(),
                    "trace": result.trace.len(),
                    "emits": result.emits,
                    "console": result.console,
                }),
                &status,
                crate::background::RunState::Complete,
                &run_dir,
                &run_dir_name,
                crate::workflows::WorkflowReceiptState::Complete,
                &result.children,
                &host_steps,
                &result.trace,
                text.clone(),
            )
            .await
            .map_err(ToolError::new)?;
            Ok(ToolResult {
                content: vec![cyrup_core::Content::text(text)],
                details: Some(details),
                terminate: TerminateHint::Unspecified,
                ..Default::default()
            })
        }
        Err(error) => {
            // The partial IS the point of this error type: a failed workflow still yields its
            // trace, children, console and emits, and they must reach the caller.
            let mut message = error.message.clone();
            let detached: Vec<&str> = error
                .partial
                .children
                .iter()
                .filter(|c| c.detached)
                .map(|c| c.key.as_str())
                .collect();
            // SCOPE_8's detach hook, gated on upstream's OWN gate. pi calls
            // `promotePausedWorkflowIfSettled` at `subagent-executor.ts:5763` "right after a
            // workflow fails with `errorKind: 'detached-child'` and is therefore parked at
            // `paused`", and the error kind is exactly what the engine already hangs on this
            // rejection (`engine.rs:3019`, from `deliver_launch`'s detached arm). The looser
            // `!detached.is_empty()` test below is NOT the same question and must not be used
            // for it: a child that asked its supervisor a question and then exited CLEANLY
            // carries `detached: true` and never rejects its launch (`deliver_launch` only
            // rejects `!ok`), so a workflow that failed for an unrelated reason alongside such
            // a child is an ordinary failure, not a paused detach.
            let parked_on_detach = error.error_kind
                == Some(crate::workflows::scripted::WorkflowScriptErrorKind::DetachedChild);
            if !detached.is_empty() {
                message.push_str(&format!(" Detached child run(s): {}.", detached.join(", ")));
                message.push_str(if parked_on_detach {
                    " The run parks at paused and is reconciled from each detached child's own \
                     settled result."
                } else {
                    " This build cannot resume a detached workflow."
                });
            }
            message.push_str(&format!(
                " Partial: {} child run(s), {} trace event(s).",
                error.partial.children.len(),
                error.partial.trace.len()
            ));
            // The detach hook's evidence, snapshotted BEFORE the settle so the `Arc<Host>` is
            // not borrowed across it. Empty unless a child really detached.
            let detached_children = if parked_on_detach {
                host.detached_children()
            } else {
                Vec::new()
            };
            // §3.2 failure arm: trace now reaches the composer (with_workflow_children reads it).
            //
            // `Paused`, not `Failed`, for a detached-child rejection — pi's own shape, and the
            // state `apply_detached_child_settlement` is gated on (`settlement.rs:279-282`).
            // `Running -> Paused` is a legal transition (`background/state.rs:168`), so the
            // note this arm used to carry ("this build cannot represent it as a run state") was
            // never true of the state machine; what was missing was the second half — the
            // reconciler that takes the run back OUT of `paused`, immediately below. Nothing
            // observes the paused status in between: `settle_foreground_workflow`'s plan emits
            // no completion event for a non-terminal state (`settlement.rs:715-728`) and this
            // tool call has not returned.
            let (terminal, receipt_state) = if parked_on_detach {
                (
                    crate::background::RunState::Paused,
                    crate::workflows::WorkflowReceiptState::Paused,
                )
            } else {
                (
                    crate::background::RunState::Failed,
                    crate::workflows::WorkflowReceiptState::Failed,
                )
            };
            match settle_foreground_workflow(
                serde_json::json!({}),
                &status,
                terminal,
                &run_dir,
                &run_dir_name,
                receipt_state,
                &error.partial.children,
                // The SAME evidence on the failure arm. A workflow that throws out of
                // `runs.host` settles here, and that is exactly the case the terminal host
                // step matters most in — the partial carries no host steps of its own.
                &host_steps,
                &error.partial.trace,
                message.clone(),
            )
            .await
            {
                Ok(mut details) => {
                    let reconciled = reconcile_detached_workflow_children(
                        executor,
                        cfg,
                        cwd,
                        &workflow_run_id,
                        &detached_children,
                        &error.partial.trace,
                    )
                    .await;
                    if !reconciled.is_empty()
                        && let Some(map) = details.as_object_mut()
                    {
                        map.insert(
                            "reconciledFromDetachedChildren".to_string(),
                            serde_json::json!(reconciled),
                        );
                    }
                    Err(ToolError::new(message).with_details(details))
                }
                Err(e) => Err(ToolError::new(format!("{message} {e}"))),
            }
        }
    }
}

/// §3d rules 1-3: build the receipt from `children`, persist it, and fold
/// `workflowRunId`/`workflowReceipt` onto `base_details` — or, when persistence fails (an
/// ENVIRONMENT fault), log/omit the receipt rather than failing the workflow, mirroring
/// upstream's success-arm `try/catch` around `writeWorkflowReceipt`
/// (`subagent-executor.ts:5728-5730`).
///
/// A malformed child (a bad/duplicate key, a resumable child with no run id) is a DIFFERENT
/// kind of fault — a real defect in the engine's own output — and is surfaced as an `Err`
/// rather than swallowed.
/// The ONE terminal writer for a foreground workflow (WORKFLOW_3 §3c's production caller).
///
/// Order is WORKFLOW_3's: advance the status, build the receipt, persist it, hand the persistence
/// OUTCOME (path or error) to [`crate::workflows::plan_workflow_settlement`], then write the status
/// the plan produced. The composer owns the `evidence-persistence-failed` promotion, the
/// `workflow_children` projection and the `workflow_receipt_path` stamp; this function owns only
/// the two writes and the details map.
#[allow(clippy::too_many_arguments)]
async fn settle_foreground_workflow(
    mut base_details: serde_json::Value,
    status: &std::sync::Arc<std::sync::Mutex<crate::background::RunStatus>>,
    terminal: crate::background::RunState,
    run_dir: &Path,
    run_dir_name: &crate::identity::RunDirName,
    receipt_state: crate::workflows::WorkflowReceiptState,
    children: &[crate::workflows::WorkflowScriptChildResult],
    host_steps: &[crate::workflows::HostStepNode],
    trace: &[crate::workflows::WorkflowScriptTraceEntry],
    summary: String,
) -> Result<serde_json::Value, String> {
    // 1. Terminal status FIRST — `plan_workflow_settlement` DERIVES `success` from `status.state`
    //    (`settlement.rs:709`) and never advances it itself. Snapshot out of the shared record so
    //    no std guard is alive across the awaits below.
    let settled = {
        let mut guard = status
            .lock()
            .map_err(|_| "workflow status lock was poisoned".to_string())?;
        // The engine's returned `children` is the COMPLETE settled list — authoritative over
        // whatever `publish_steps` last managed to write.
        let mut steps = crate::workflows::workflow_step_statuses(children);
        // Each row keeps the `ended_at` `publish_steps` stamped when its child settled; a row
        // `publish_steps` never saw (the engine settled it in the same tick) is stamped now.
        crate::workflows::carry_step_settle_times(
            &guard.steps,
            &mut steps,
            crate::time::now_epoch_millis(),
        );
        guard.steps = steps;
        guard.current_step = guard.steps.len().checked_sub(1);
        guard.advance_state(terminal).map_err(|e| e.to_string())?;
        guard.clone()
    };

    // 2. Build the receipt. `workflow_children: None` stays — the PLAN fills it
    //    (`settlement.rs:692-701`), which is the whole reason the composer exists.
    let receipt =
        crate::workflows::build_workflow_receipt(crate::workflows::BuildWorkflowReceipt {
            workflow_run_id: run_dir_name,
            state: receipt_state,
            children,
            // WORKFLOW_19 — the nodes `on_host_step` accumulated, upserted by id so Rule 11's
            // `assert_unique_host_step_ids` (which would otherwise fail the whole receipt, and
            // with it the workflow) sees one terminal entry per command.
            host_steps,
            workflow_children: None,
            resource: None, // `one_use_permit: None` in this build
            terminal_outcome: None,
            created_at: None,
        })
        .map_err(|error| format!("workflow completed but its receipt is invalid: {error}"))?;

    // 3. Persist it, and carry the OUTCOME rather than swallowing it.
    let (receipt_path, receipt_persistence_error) =
        match crate::workflows::write_workflow_receipt(run_dir, &receipt) {
            Ok(path) => (Some(path), None),
            Err(error) => (None, Some(error.to_string())),
        };

    // 4. WORKFLOW_3's composer. Its first production call site.
    let plan =
        crate::workflows::plan_workflow_settlement(crate::workflows::PlanWorkflowSettlement {
            status: &settled,
            summary,
            trace,
            receipt: Some(receipt),
            receipt_path,
            receipt_persistence_error,
            resolution: None,
            terminal_outcome: None,
            now: None,
            event_metadata: serde_json::Map::new(),
        });

    // 5. The terminal status write — now carrying `workflow_children` and `workflow_receipt_path`,
    //    which `control.rs:305` / `wait.rs:602` / `run_status.rs:367` /
    //    `wait_completions/project.rs:39,184` have been reading for nothing.
    crate::background::atomic::write_atomic_json(&run_dir.join("status.json"), &plan.status)
        .await
        .map_err(|e| format!("workflow terminal status could not be written: {e}"))?;

    if let Some(map) = base_details.as_object_mut() {
        map.insert(
            "workflowRunId".to_string(),
            serde_json::json!(plan.status.run_id.as_str()),
        );
        if let (Some(receipt), Some(path)) = (&plan.receipt, &plan.receipt_path) {
            map.insert(
                "workflowReceipt".to_string(),
                serde_json::json!({ "path": path, "receipt": receipt }),
            );
        }
        if let Some(children) = &plan.status.workflow_children {
            map.insert("workflowChildren".to_string(), serde_json::json!(children));
        }
        // Always empty until WORKFLOW_15 records per-child resumability
        // (`workflow_recovery_actions` filters on `entry.resume.is_resumable()`,
        // `settlement.rs:413-432`). Surfaced anyway so the wiring is provably correct now and
        // starts reporting the moment WORKFLOW_15 lands.
        if !plan.recovery.is_empty() {
            map.insert("recovery".to_string(), serde_json::json!(plan.recovery));
        }
    }
    Ok(base_details)
}

/// SCOPE_8's detach hook — pi's `onDetachedExit` closure (`subagent-executor.ts:3951-3976`,
/// fired by `execution.ts:2364` when a child detached through `detachForeground` eventually
/// terminates), in the one shape cyrup's foreground lifecycle actually has.
///
/// # Why this is the firing moment, and why there is no earlier one
///
/// Upstream needs a callback because `detachForeground` returns the tool call EARLY and leaves
/// the child running; the exit it is waiting for happens after the workflow is already parked.
/// cyrup's INTERCOM detach — the only one a workflow child can take — does the opposite: a
/// blocking `contact_supervisor` ask fires `spawn_clarify` and the drive loop KEEPS DRIVING
/// (`exec/drive_attempt.rs:345-367`), the human's answer riding back to the still-alive child over
/// the broker rather than this stdout pipe. So the detached child's exit is observed
/// synchronously, inside
/// [`WorkflowScriptHost::launch`](crate::workflows::scripted::WorkflowScriptHost::launch), and
/// the two upstream moments — "the workflow parks at paused" and "its detached child exits" —
/// collapse into this one settlement. The `Paused` status the caller just wrote is the input;
/// this is the write that supersedes it.
///
/// **Scoped to the intercom producer on purpose, and the scope is narrower than it looks.**
/// VL-S11b added cyrup's second producer, `/subagents-detach`, which DOES return early and hands
/// its child to a continuation task (`extension/executor/foreground.rs`'s producer split). A
/// workflow child is reachable from it: `register_foreground_controls` stamps `mode: Single` on
/// EVERY entry it publishes, including a workflow child's (it only adds
/// `parent_workflow_run_id`/`workflow_key` on top), so the handler's `mode != Single` refusal (pi
/// `slash-commands.ts:990`) does not fire and a human who names that child's run id detaches it.
///
/// What then reaches this hook is the RECEIPT, not a terminal result — `launch` cannot tell the
/// two apart, since both are `SingleResult { detached: true }` returned from
/// `run_foreground_streaming`. The reconciliation it drives is therefore provisional for a user
/// detach: correct about "this child is detached and the workflow parks at paused", but written
/// before the child exited. The real exit is observed by the producer's own continuation, whose
/// reconciler is upstream's other one, `updateRememberedForegroundChild`
/// (`subagent-executor.ts:850-899`, ported as `reconcile_detached_foreground_child`), and which
/// updates `foreground_runs` — not this workflow status. Upstream closes that gap by routing the
/// exit back through `resolveDetachedWorkflowChild` (`subagent-executor.ts:4078-4081`); cyrup's
/// continuation has no workflow handle to route to, so a user-detached WORKFLOW child stays
/// `paused` until something else settles it. A user-detached plain `/run` single — the case
/// `/subagents-detach` exists for, and the only one its success sentence describes — is fully
/// reconciled.
///
/// Sequential, in launch order, never concurrent: each call re-reads `status.json` and writes
/// it back, and only the LAST open child's settlement promotes the workflow out of `paused`
/// (`promote_settled_paused_workflow`'s `still_open` scan). Interleaving the read-modify-write
/// would let one child's settlement clobber another's.
///
/// Returns the child run ids that really reconciled — `Ok(false)` means the reconciler refused
/// (no status, not a paused workflow, or no matching step), which is its idempotency guarantee
/// and not a fault. An `Err` is logged and swallowed for the reason `finish.rs` treats its own
/// index write that way: the workflow's `status.json` and receipt are already durable, and a
/// reconciliation that could not publish must not turn into a second, different tool error on
/// top of the workflow failure that is already being reported.
async fn reconcile_detached_workflow_children(
    executor: &Arc<SubagentExecutor>,
    cfg: &crate::registration::SubagentExtensionConfig,
    cwd: &Path,
    workflow_run_id: &RunId,
    detached: &[crate::extension::executor::workflow::DetachedWorkflowChild],
    trace: &[crate::workflows::WorkflowScriptTraceEntry],
) -> Vec<String> {
    if detached.is_empty() {
        return Vec::new();
    }
    // The SAME roots arithmetic the dispatch used to create the run directory, so the
    // reconciler resolves the tree the run really wrote to.
    let run_paths = crate::background::RunPaths::for_run(
        &crate::extension::executor::paths::default_async_root_in(&cfg.roots, cwd),
        &crate::extension::executor::paths::default_results_dir_in(&cfg.roots, cwd),
        workflow_run_id,
    );
    // SCOPE_8 §Y-1 shape (a): one snapshot for the whole sweep, taken here so no `std` guard is
    // alive across the `.await`s below.
    let live_controls = executor.live_foreground_controls();
    let mut reconciled = Vec::new();
    for child in detached {
        match crate::extension::executor::workflow_detach::reconcile_detached_workflow_child_completion(
            crate::extension::executor::workflow_detach::DetachedWorkflowChildCompletion {
                run_paths: &run_paths,
                child_run_id: child.run_id.as_str(),
                result: &child.result,
                workflow_key: child.workflow_key.as_ref(),
                live_controls: &live_controls,
                trace,
            },
        )
        .await
        {
            Ok(true) => reconciled.push(child.run_id.as_str().to_string()),
            Ok(false) => tracing::debug!(
                workflow_run_id = %workflow_run_id,
                child_run_id = %child.run_id,
                "detached-child reconciliation found nothing to settle; the workflow status is \
                 already past `paused` or names no such step"
            ),
            Err(error) => tracing::warn!(
                workflow_run_id = %workflow_run_id,
                child_run_id = %child.run_id,
                %error,
                "failed to reconcile a detached workflow child; the workflow's paused status \
                 and receipt are already durable on disk"
            ),
        }
    }
    reconciled
}
