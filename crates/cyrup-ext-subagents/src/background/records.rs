//! StepStatus / ParallelGroupStatus (func-SA §4.5)
//! RunStatus (func-SA §4.5, R-SA-075/076/077/090/094)
//! ResultFile (func-SA §4.5, R-SA-077/166)
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::path::PathBuf;

use cyrup_core::{ModelId, Usage};

use crate::exec::SingleResult;
use crate::identity::{CompletionOwnerId, SessionId};

use super::{
    RunId, RunMode, RunState, RunStateTransitionError, RunTelemetry, StepState, StepTelemetry,
    TokenTotals,
};

/// Per-step status entry inside a [`RunStatus`] (func-SA §4.5).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepStatus {
    /// The fully-qualified agent name this step invokes.
    pub agent: String,
    /// This step's own lifecycle state.
    pub status: StepState,
    /// Path to the persisted session transcript this step's child was run against, once known —
    /// the seed `resume` (R-SA-085) reads back from when reviving a terminal run.
    pub session_file: Option<PathBuf>,
    /// The model actually used for the attempt that finished (or is currently running).
    pub model: Option<ModelId>,
    /// Every model attempted for this step, in fallback-ladder order (R-SA-038).
    pub attempted_models: Vec<ModelId>,
    /// Token/cost usage accumulated for this step so far.
    pub usage: Usage,
    /// Assistant turns this step ran — pi's `Usage.turns` (`shared/types.ts:262`), kept beside
    /// [`Self::usage`] because [`cyrup_core::Usage`] deliberately has no turn concept (see
    /// [`crate::exec::SingleResult::turns`]). Written by `record_step_outcome` when the step
    /// settles; omitted from the wire while zero so a `status.json` written before this field
    /// existed still round-trips.
    #[serde(default, skip_serializing_if = "crate::exec::is_zero_u64")]
    pub turns: u64,
    /// pi's status-payload `step.contextOverflow` (`shared/types.ts:1933`, written by
    /// `setOptionalProperty(requiredStatusStep(…), "contextOverflow", singleResult.contextOverflow)`
    /// at `subagent-runner.ts:3668`/`:4083`/`:4570`, read back at `:2238`/`:3738`/`:4194`/`:4853`).
    ///
    /// Distinct from [`crate::exec::SingleResult::context_overflow`] only in surface: this is what
    /// `subagent({action:"status"})` and the fleet view read, and without it an overflowing async
    /// run reports the flag in its result file and nowhere an operator looks.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub context_overflow: bool,
    /// pi's status-payload `step.timeoutRecovery` (`shared/types.ts:1917`, written by
    /// `setOptionalProperty(requiredStatusStep(…), "timeoutRecovery", singleResult.timeoutRecovery)`
    /// at `subagent-runner.ts:3750`/`:4165`/`:4645`) — the **full summary**, deliberately: a
    /// status step is local state, so it keeps the session/transcript/artifact paths. A job or
    /// wait view must narrow it to
    /// [`crate::exec::mutation_evidence::TimeoutRecoveryProjection`] on the way out (upstream's
    /// `Omit<…,"timeoutRecovery"> & { timeoutRecovery?: TimeoutRecoveryProjection }`,
    /// `shared/types.ts:1962-1965`) — never republish this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_recovery: Option<crate::exec::mutation_evidence::TimeoutRecoverySummary>,
    /// Where this step's live child transcript is (or will be) written — pi
    /// `AsyncStatus.steps[].transcriptPath`, written at declaration by
    /// `resolveAsyncStepTranscriptPath` (`subagent-runner.ts:1806-1821`, applied at `:1965-1986`)
    /// and overwritten post-step from `singleResult.transcriptPath ?? step.transcriptPath`
    /// (`:3829`). Stamped BEFORE the child spawns, so the FleetView pane and `/subagents status`
    /// can open the file while the step is still `Running`; `None` when the run has no artifacts
    /// dir, `enabled: false`, or `include_transcript: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<PathBuf>,
    /// The transcript writer's latched failure for this step — pi `step.transcriptError`
    /// (`subagent-runner.ts:3830`), from [`crate::exec::SingleResult::transcript_error`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_error: Option<String>,
    /// The step's own error message, if `status == Failed`.
    pub error: Option<String>,
    /// Run-ids of any further background runs this step itself spawned (R-SA-104's nested
    /// descendants) — populated only for a step whose agent itself delegated to a background
    /// subagent. Kept as bare [`RunId`]s rather than full nested [`RunStatus`] snapshots so this
    /// struct stays cheap to (de)serialize on every status write; a reader that needs the nested
    /// run's own status re-reads it via [`RunPaths::nested`](crate::background::RunPaths::nested).
    pub nested_run_ids: Vec<RunId>,
    /// Wall-clock start time (epoch milliseconds) once this step began running.
    pub started_at: Option<i64>,
    /// Wall-clock end time (epoch milliseconds) once this step reached a terminal or paused
    /// state.
    pub ended_at: Option<i64>,
    /// SUBA-087 — pi `step.stopRequested?: boolean` (`shared/types.ts:1882` @v0.64.0): a
    /// child-scoped stop (`subagent({action:"stop", childId})`) has been recorded against THIS step
    /// by the runner (`markChildStopRequested`, `subagent-runner.ts:2979-2991`). Set before the
    /// child is torn down and kept once it is, so a reader can tell a step that was asked to stop
    /// from one that stopped for any other reason.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop_requested: bool,
    /// SUBA-087 — pi `step.stopRequestedAt?: number` (`shared/types.ts:1883`): epoch milliseconds
    /// of the request above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_requested_at: Option<i64>,
    /// SUBA-087 — pi `step.stopped?: boolean` (`shared/types.ts:1904`): this step ended
    /// [`StepState::Stopped`] because a stop — run-wide (`stopRunner`, `subagent-runner.ts:3842`)
    /// or child-scoped (`markChildStopped`, `:2998`) — terminated it. Distinct from the state so a
    /// status reader that only knows the older `status` word still sees the flag pi's readers key
    /// on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped: bool,
    /// The stable workflow lane key this step belongs to — pi `AsyncStatus.steps[].workflowKey`
    /// (`shared/types.ts:1878`), read by `workflowChildSummary`'s step pass
    /// (`workflow-child-summary.ts:83-84`).
    ///
    /// `None` for every non-workflow run. This is the FIRST rung of
    /// [`crate::background::child_identity::identity_from_parts`], which had been unreachable
    /// since that module was written; landing it means a caller can name a workflow child by its
    /// key rather than by a positional `step:<index>` that shifts when the graph does.
    ///
    /// No `deserialize_with`: [`crate::workflows::WorkflowKey`] deserializes THROUGH its own
    /// parser (SCOPE_3d §0.9), matching [`crate::identity::SessionId`] and every other validated
    /// newtype in this crate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_key: Option<crate::workflows::WorkflowKey>,
    /// This step's own child run id, when the step launched a real run — pi
    /// `AsyncStatus.steps[].runId` (`shared/types.ts:1888`), the SECOND rung of the same identity
    /// ladder, one of the four `launchResolved` witnesses (`workflow-child-summary.ts:91`), and
    /// the value `WaitCompletionChild.runId` carries (via
    /// [`crate::exec::SingleResult::child_run_id`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// The external runner this step's child executed under, when it was not a native child — pi
    /// `AsyncJobStep.runner?: ExternalCliRunnerStatus | ExternalJobRunnerStatus`
    /// (`shared/types.ts:1315`), carried onto the step summary as `runner?` (`async-status.ts:51`)
    /// and read by `children.list`'s external-runner rung (`retained-children.ts:54`), which
    /// refuses to call such a child resumable. Filled at settle time from the child's own
    /// [`SingleResult::runner`]; `None` for every native child and omitted from the wire, so a
    /// status written before this field existed still round-trips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner: Option<crate::runner::status::ExternalCliRunnerStatus>,
    /// Human-readable display name for the child session, when derived at launch — pi
    /// `AsyncStatus.steps[].sessionName` (`shared/types.ts:1868`) /
    /// `WorkflowChildSummary.children[].sessionName` (`shared/types.ts:200-201`), bounded to 256
    /// UTF-8 bytes by the summary builder (`workflow-child-summary.ts:97`) — bounded THERE, not
    /// here: this field is local state, and rejecting a long name on read would lose a value pi
    /// keeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    /// pi `WorkflowStatusStep.interrupted` (`workflow-settlement.ts:18`) — NOT on the base
    /// `AsyncStatus["steps"][number]`; the settlement family adds it.
    ///
    /// Written by [`crate::workflows::apply_detached_child_settlement`] and read by
    /// [`crate::workflows::classify_workflow_settlement`] — which is why it cannot be folded into
    /// [`Self::stopped`] or into [`StepState::Paused`]: the classifier distinguishes a step that
    /// FAILED with an interruption from one that merely failed, and that distinction is what
    /// selects `interrupted-child` over `failed-child`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub interrupted: bool,
    /// pi `WorkflowStatusStep.outputPathMapping` (`:17`) — the requested→saved remap, stamped by
    /// the detach reconciler and carried into the public child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path_mapping: Option<crate::workflows::WorkflowOutputPathMapping>,
    /// Live activity telemetry folded from this step's child events (pi
    /// `subagent-runner.ts:2706-2861`) — flattened so its members serialize at the same top level
    /// of the `status.json` step object pi writes them at (`shared/types.ts:598-632`).
    #[serde(flatten, default)]
    pub telemetry: StepTelemetry,
}

impl StepStatus {
    /// A freshly declared, not-yet-dispatched step for `agent`.
    #[must_use]
    pub fn pending(agent: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            status: StepState::Pending,
            session_file: None,
            model: None,
            attempted_models: Vec::new(),
            usage: Usage::default(),
            turns: 0,
            context_overflow: false,
            timeout_recovery: None,
            transcript_path: None,
            transcript_error: None,
            error: None,
            nested_run_ids: Vec::new(),
            started_at: None,
            ended_at: None,
            stop_requested: false,
            stop_requested_at: None,
            stopped: false,
            workflow_key: None,
            run_id: None,
            runner: None,
            session_name: None,
            interrupted: false,
            output_path_mapping: None,
            telemetry: StepTelemetry::default(),
        }
    }
}

/// Status of one concurrently-dispatched member of a `ParallelGroup`/`DynamicGroup` step
/// (func-SA §4.5's `RunStatus.parallel_groups`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParallelGroupStatus {
    /// The FLAT index the owning group starts at in [`RunStatus::steps`] — pi's `group.start`
    /// (`subagent-runner.ts:2621,2657` @v0.64.0).
    ///
    /// SUBA-093 changed what this number means: `RunStatus::steps` now carries one entry per
    /// `ParallelGroup` MEMBER rather than one entry for the whole group, so this is the group's
    /// base slot, not its position in the run's `RunnerStep` list. Nothing outside
    /// [`crate::background::runner_main`]'s own terminal sweeps reads it, and those compare it
    /// against a flat cursor, so the two moved together.
    pub group_step_index: usize,
    /// Status of each concurrently-dispatched child within this group, in fixed dispatch-order
    /// (R-SA-051: result/status ordering is always preserved, never reordered by completion
    /// order).
    ///
    /// A settled-detail record, written once when the group finishes. For a `ParallelGroup` the
    /// LIVE per-member status is in [`RunStatus::steps`] itself after SUBA-093 — this vector
    /// remains for the consumers (`registration::cost`, `background::tracker`,
    /// `tui::fleet_status`) that already walk it.
    pub children: Vec<StepStatus>,
}

/// The on-disk `status.json` record for one background run (func-SA §4.5; arch-SA §3.6).
///
/// Written via [`atomic::write_atomic_json`](crate::background::atomic::write_atomic_json) (R-SA-076) at every observable state change
/// (R-SA-075) by the detached runner (`background/runner_main.rs`, later phase); read by the
/// orchestrator's poller (`background/tracker.rs`) and by `status`/`interrupt`/`resume`/
/// `append-step` control handlers (`background/control.rs`) after first running stale-run
/// reconciliation (R-SA-079, `background/reconcile.rs`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunStatus {
    /// This run's identity.
    pub run_id: RunId,
    /// The ORCHESTRATOR session that launched this run (pi `AsyncStatus.sessionId`,
    /// `shared/types.ts:1249`, written by the runner from `config.sessionId` at
    /// `subagent-runner.ts:2088`).
    ///
    /// Recorded so a LATER session reading the same async root can tell whose runs these are: pi's
    /// `listAsyncRuns` drops every on-disk run whose `sessionId` differs from the caller's
    /// (`async-status.ts:432`), which is what keeps `/subagents-fleet` and the active-run listings
    /// scoped to the current session instead of showing every run the project ever launched.
    /// `None` for a run launched with no resolvable parent-session anchor (a headless or
    /// unpersisted orchestrator), and for a status synthesized by reconciliation.
    ///
    /// Since the session-scoping work this identity ALSO gates every control operation
    /// (`background/control.rs`'s stop/steer/interrupt, pi `async-stop-action.ts:34`), not just
    /// listings — multiple cyrup instances share one per-cwd `async_root`
    /// (`background/artifact_roots.rs:281-284`), so without it any instance can act on any
    /// other's runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// The orchestrator PROCESS that launched this run (pi `completionOwnerId`,
    /// `shared/completion-owner.ts:10-14`).
    ///
    /// The second half of the delivery-ownership pair: a session id can outlive the process that
    /// minted it (it is written to disk and read back later), so session identity alone cannot
    /// decide who may consume a completion. A distinct TYPE from [`RunStatus::session_id`] so the
    /// two can never be transposed at a call site.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_owner_id: Option<CompletionOwnerId>,
    /// Which shape of run this is.
    pub mode: RunMode,
    /// Current overall lifecycle state (monotone-forward, see [`RunState`]).
    pub state: RunState,
    /// The detached runner's own OS pid, once known. `None` only in the brief window between
    /// spawn-call-site provisional-status synthesis (R-SA-090) and the runner's own first real
    /// status write, if the caller chooses not to pre-populate a known pid (the reference spawn
    /// call site always knows the pid immediately from `Child::id()`, so in practice this is
    /// `Some` from the very first write; the `Option` exists for the reconciliation code path,
    /// `background/reconcile.rs`, which may synthesize a status for a run whose pid was never
    /// successfully recorded).
    pub pid: Option<u32>,
    /// The working directory this run's steps actually execute in (pi `AsyncStatus.cwd`,
    /// `shared/types.ts:592`), set once by the detached runner at its very first status write and
    /// never changed thereafter (a run's cwd is fixed for its whole lifetime). `resume`'s
    /// terminal-revival branch (R-SA-085, `Self::revive_from_transcript`) reads this back so a
    /// revived child spawns in the SAME directory the original run did, rather than whatever cwd
    /// happens to be current at resume time (pi `target.cwd ?? requestCwd`,
    /// `background/async-resume.ts:323,345,373` @v0.34.0 + `subagent-executor.ts:890`). `None` only for a
    /// synthesized/repaired status that never had a chance to observe the real value (mirrors
    /// `pid`'s own `None` carve-out immediately above).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// The run-wide persisted session-transcript path, once known (pi `AsyncStatus.sessionFile`,
    /// `shared/types.ts:636`, populated at `subagent-runner.ts:2411` once the run's effective
    /// session file is resolved). Read back by the SAME terminal-revival branch as
    /// [`Self::cwd`] as the run-level fallback beneath a resolved step's own `sessionFile`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    /// Wall-clock spawn-confirmed time (epoch milliseconds).
    pub started_at: i64,
    /// Wall-clock terminal time (epoch milliseconds), set once `state` becomes `Complete`/
    /// `Failed`.
    pub ended_at: Option<i64>,
    /// Wall-clock time of the most recent status write — the value long-staleness reconciliation
    /// (R-SA-091) compares against its threshold.
    pub last_update: i64,
    /// Index of the step currently running (or about to run), if any.
    pub current_step: Option<usize>,
    /// Total number of steps currently known for a `Chain` run (grows as append-requests are
    /// consumed, R-SA-095).
    pub chain_step_count: Option<usize>,
    /// Count of still-unconsumed chain-append requests, recomputed by directory listing each time
    /// it changes (R-SA-094) — never a separately maintained counter that could drift from the
    /// actual `append-requests/` directory contents. This module does not itself recompute the
    /// count (that requires a filesystem read, owned by `background/control.rs`); it only carries
    /// the field.
    pub pending_appends: Option<usize>,
    /// Per-step status, in step order.
    pub steps: Vec<StepStatus>,
    /// Per-parallel-group child status, for any `ParallelGroup`/`DynamicGroup` steps.
    pub parallel_groups: Option<Vec<ParallelGroupStatus>>,
    /// SUBA-057 — pi `AsyncStatus.displayDismissedAt?: number` (`shared/types.ts:1293-1294`
    /// @v0.47.1, whose own doc comment reads *"Display-only dismissal marker for a reload-orphaned
    /// workflow."*). Epoch milliseconds, stamped by
    /// [`SubagentExecutor::control_dismiss`](crate::extension::SubagentExecutor::control_dismiss)
    /// (pi `dismissRecoveredWorkflow`, `runs/foreground/async-dismiss-action.ts:66`).
    ///
    /// **Display-only, and terminates nothing.** Its whole contract is that three readers honour it
    /// and one erases it:
    ///
    /// * [`crate::background::reconcile::reconcile`] returns
    ///   [`ReconcileAction::DisplayDismissed`](crate::background::reconcile::ReconcileAction::DisplayDismissed)
    ///   for a dismissed run instead of a normal outcome — pi's `return { status: null, … }`
    ///   (`stale-run-reconciler.ts:359-361`);
    /// * [`crate::background::run_status::list_active_runs`] drops it from the active set (pi
    ///   `async-status.ts:455-458`), which is what makes it vanish from `/subagents-fleet` and
    ///   `{action:"status"}`;
    /// * the single-run status view renders the `State: display-dismissed` report instead (pi
    ///   `run-status.ts:332-345`);
    /// * and a genuine terminal repair from a [`ResultFile`] **clears** it (pi `delete
    ///   terminalStatus.displayDismissedAt`, `stale-run-reconciler.ts:169`), so a dismissed run
    ///   whose result lands later comes back with its real outcome rather than staying hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_dismissed_at: Option<i64>,
    /// pi `AsyncStatus.error` (`shared/types.ts`) — the RUN-LEVEL error, written by
    /// [`crate::workflows::promote_settled_paused_workflow`] (the
    /// [`crate::workflows::UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION`] diagnostic has no other
    /// home) and by [`crate::workflows::plan_workflow_settlement`]'s
    /// [`crate::workflows::EVIDENCE_PERSISTENCE_FAILED`] promotion.
    ///
    /// **Distinct from [`StepStatus::error`]**, which is per-step and already existed — do not
    /// confuse the two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// pi `AsyncStatus.toolCallId` (`shared/types.ts:387`) — the host tool-call id, retained when
    /// it differs from [`Self::run_id`]. Read by
    /// [`crate::workflows::with_workflow_children`]'s `status.toolCallId ?? status.runId`
    /// fallback, and is the value [`crate::background::result_index::ResultWrite::tool_call_id`]
    /// wants (`background/runner_main/finish.rs` currently hard-codes that write parameter to
    /// `None`; this field is where a future caller would source it from).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// pi `AsyncStatus.workflowChildren` (`workflow-settlement.ts:231`) — the run's own live/final
    /// child inventory, mirrored onto [`crate::background::ResultFile::workflow_children`] at
    /// settlement. An in-flight workflow reports its inventory with its `inventory_complete` flag
    /// still `false` before it settles; only [`crate::workflows::with_workflow_children`] ever
    /// flips that flag to settled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_children: Option<crate::workflows::WorkflowChildSummary>,
    /// pi `AsyncStatus.workflowReceiptPath` — the exact reference returned by successful current
    /// workflow receipt publication. `None` unless a receipt was actually written; deliberately
    /// does NOT gain a sibling `stopped` field — cyrup carries the stop verdict on
    /// [`Self::state`] alone (`status.stopped ? true : undefined` becomes
    /// `(state == RunState::Stopped).then_some(true)` at every reader).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_receipt_path: Option<std::path::PathBuf>,
    /// pi `AsyncStatus.parallelHandoff` (`shared/types.ts:920`, mirrored onto `Details` `:1468`
    /// and the result record `:1994`) — the compact reference to the parallel-handoff manifest a
    /// `worktree: true` fan-out wrote, set at each of upstream's four write sites
    /// (`subagent-runner.ts:3293,:3308,:4427,:4810`).
    ///
    /// Not decorative. It is HOW a caller learns the `handoffPath` to pass to a later
    /// `worktree.cleanup` / `lane.*` verb — `extension/schemas.ts:298` @v0.68.0 describes that
    /// parameter as *"Existing manifest for worktree/lane actions"*, and
    /// `subagent-executor.ts:6247` refuses with *"requires handoffPath from parallelHandoff.path
    /// or async status"*. It is also the first of the two path sources
    /// [`crate::background::async_retention`]'s `hasUnresolvedRunHandoff` consults, which could
    /// not be ported while this field was absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_handoff: Option<crate::handoff::HandoffReference>,
    /// Run-wide live activity roll-ups + the workflow-graph snapshot (pi's top-level
    /// `statusPayload` telemetry, `subagent-runner.ts:2085-2120`) — flattened so its members
    /// serialize at the same top level of `status.json` pi writes them at.
    #[serde(flatten, default)]
    pub telemetry: RunTelemetry,
}

impl RunStatus {
    /// Constructs the initial `Queued` status for a freshly minted run, before the detached
    /// runner has written anything of its own. `started_at`/`last_update` are both set to "now".
    #[must_use]
    pub fn queued(run_id: RunId, mode: RunMode, pid: Option<u32>) -> Self {
        let now = crate::time::now_epoch_millis();
        Self {
            run_id,
            // The launching session is the runner's to record (it is the only process that knows
            // the anchor); a status built here carries none until it does.
            session_id: None,
            // Stamped by the orchestrator at spawn (`extension/executor/background.rs`), not here:
            // this constructor runs before the run has an owner recorded.
            completion_owner_id: None,
            mode,
            state: RunState::Queued,
            pid,
            cwd: None,
            session_file: None,
            started_at: now,
            ended_at: None,
            last_update: now,
            current_step: None,
            chain_step_count: None,
            pending_appends: None,
            steps: Vec::new(),
            parallel_groups: None,
            // SUBA-057: a freshly minted run has never been display-dismissed.
            display_dismissed_at: None,
            error: None,
            tool_call_id: None,
            workflow_children: None,
            workflow_receipt_path: None,
            parallel_handoff: None,
            telemetry: RunTelemetry::default(),
        }
    }

    /// Record one host-step transition onto [`RunStatus::telemetry`], UPSERTING BY ID — pi's
    /// `job.hostSteps = validHostStepNodes(status.workflowGraph)` refresh
    /// (`async-job-tracker.ts:469`), reached here one transition at a time because cyrup's
    /// producer is a callback stream rather than a graph rebuild (see
    /// [`RunTelemetry::host_steps`]).
    ///
    /// ⚠ UPSERT, never push. The scripted engine emits each `runs.host` call TWICE — a `Running`
    /// node when the op starts (`workflows/scripted/engine.rs:1259`) and a terminal one when it
    /// settles (`:1320`/`:1367`) — both carrying the same [`crate::workflows::HostStepNode::id`].
    /// A blind push would put two rows with one id into `status.json`, which is exactly what
    /// [`crate::workflows::assert_unique_host_step_ids`] (pi Rule 11) exists to reject, and would
    /// make the projection render one monitor as two children.
    ///
    /// A NEW id beyond [`HOST_STEP_MAX_COUNT`](crate::workflows::HOST_STEP_MAX_COUNT) is dropped
    /// rather than appended: upstream's own readers slice to that bound (`validHostStepNodes`,
    /// `host-step-status.ts:141`) and its receipt builder THROWS past it, so a status that
    /// carried more could never be projected or receipted anyway. The engine refuses the 33rd `runs.host` call outright, so the bound is
    /// defence for a status written by something else — an upsert onto an ALREADY-recorded id is
    /// therefore never refused, only a genuinely new row.
    ///
    /// `last_update` is deliberately NOT touched: this is a telemetry fold, and the caller that
    /// persists the status is the one that decides the write moment (R-SA-075), exactly as
    /// [`Self::sync_top_level_telemetry`] leaves the timestamp alone.
    pub fn record_host_step(&mut self, node: &crate::workflows::HostStepNode) {
        let host_steps = &mut self.telemetry.host_steps;
        // `position` then `get_mut`, not `iter_mut().find()`: the length test in the `None` arm
        // needs the borrow back, and an index-first match is the form that gives it back.
        match host_steps.iter().position(|step| step.id == node.id) {
            Some(index) => {
                if let Some(existing) = host_steps.get_mut(index) {
                    existing.clone_from(node);
                }
            }
            None if host_steps.len() < crate::workflows::HOST_STEP_MAX_COUNT => {
                host_steps.push(node.clone());
            }
            None => {}
        }
    }

    /// Roll the per-step telemetry of the step at `flat_index` up into the top-level
    /// [`RunStatus::telemetry`] fields, mirroring pi's `syncTopLevelCurrentTool` +
    /// `statusPayload.toolCount`/`turnCount`/`totalTokens`/`lastActivityAt` maintenance
    /// (`subagent-runner.ts:2085-2120`). Recomputes `current_tool` from whichever step is currently
    /// running (a step with a live `current_tool`), sums `tool_count`/`total_tokens` across every
    /// step, and takes the max `turn_count` — so the roll-up is always internally consistent with
    /// the per-step fields rather than a separately drifting counter.
    pub fn sync_top_level_telemetry(&mut self, flat_index: usize) {
        self.current_step = Some(flat_index);
        let mut tool_total: u64 = 0;
        let mut turn_max: u64 = 0;
        let mut tokens = TokenTotals::default();
        let mut current_tool: Option<String> = None;
        let mut last_activity: Option<i64> = self.telemetry.last_activity_at;
        for step in &self.steps {
            tool_total = tool_total.saturating_add(step.telemetry.tool_count.unwrap_or(0));
            turn_max = turn_max.max(step.telemetry.turn_count.unwrap_or(0));
            if let Some(step_tokens) = step.telemetry.tokens {
                tokens.add(step_tokens.input, step_tokens.output);
            }
            if current_tool.is_none()
                && let Some(tool) = &step.telemetry.current_tool
            {
                current_tool = Some(tool.clone());
            }
            if let Some(activity) = step.telemetry.last_activity_at {
                last_activity = Some(last_activity.map_or(activity, |prev| prev.max(activity)));
            }
        }
        self.telemetry.current_tool = current_tool;
        self.telemetry.tool_count = (tool_total > 0).then_some(tool_total);
        self.telemetry.turn_count = (turn_max > 0).then_some(turn_max);
        self.telemetry.total_tokens = (tokens.total > 0).then_some(tokens);
        self.telemetry.last_activity_at = last_activity;
    }

    /// Constructs the **provisional, synthesized** status the spawn call site MUST supply for the
    /// R-SA-090 grace window: covers the short interval immediately after a confirmed successful
    /// hop-1 spawn but before the detached runner itself has had a chance to write its own real
    /// `status.json`. A reader (`background/reconcile.rs`) that finds no `status.json` on disk yet
    /// falls back to synthesizing a value equivalent to this constructor's output rather than
    /// declaring the run failed outright, provided it is still within the grace-window duration —
    /// the actual elapsed-time comparison against that duration is `reconcile.rs`'s job (this
    /// constructor only shapes the value, it does not itself know "how long ago" `started_at` was
    /// relative to "now").
    ///
    /// Identical to [`RunStatus::queued`] except documented as specifically the provisional-status
    /// shape a caller synthesizes rather than one persisted to disk by the runner itself — kept as
    /// a distinct, separately named constructor (rather than reusing `queued` silently) so call
    /// sites read as self-documenting about which of the two situations (real initial state vs.
    /// synthesized grace-window placeholder) they are in.
    #[must_use]
    pub fn provisional(run_id: RunId, mode: RunMode, pid: u32) -> Self {
        Self::queued(run_id, mode, Some(pid))
    }

    /// Attempts to advance `self.state` to `next`, updating `last_update` (and `ended_at` if
    /// `next` is terminal) on success. Returns [`RunStateTransitionError`] — leaving `self`
    /// entirely unmodified — if the transition is not permitted by
    /// [`RunState::can_transition_to`]. This is the sole sanctioned way to mutate `self.state`
    /// (see the enforcement note on [`RunState`] itself).
    ///
    /// # Errors
    ///
    /// Returns [`RunStateTransitionError`] if `next` is not reachable from `self.state`.
    pub fn advance_state(&mut self, next: RunState) -> Result<(), RunStateTransitionError> {
        let advanced = self.state.try_advance(next)?;
        self.state = advanced;
        let now = crate::time::now_epoch_millis();
        self.last_update = now;
        if advanced.is_terminal() {
            self.ended_at = Some(now);
        }
        Ok(())
    }

    /// Touches `last_update` to "now" without changing `state` — for status writes triggered by
    /// sub-state-level progress (tool activity milestones, model fallback, chain-append
    /// acceptance) that R-SA-075 requires a persisted update for even though the overall `state`
    /// enum value itself is unchanged.
    pub fn touch(&mut self) {
        self.last_update = crate::time::now_epoch_millis();
    }
}

/// The on-disk terminal result record for one finished background run (func-SA §4.5; arch-SA
/// §3.6). Written **exactly once**, at process exit, strictly **after** the final `status.json`
/// write (R-SA-077) — presence of this file in `ResultsDir` is the single authoritative "truly
/// done" signal for the run, decoupled from (and outranking) whatever `status.json` currently
/// claims.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultFile {
    /// Identical to `run_id` — func-SA §4.5 documents both fields on the wire schema (the
    /// duplication mirrors the ported source format rather than being invented here); kept as two
    /// fields rather than collapsed to preserve exact on-disk shape compatibility.
    pub id: RunId,
    /// This run's identity.
    pub run_id: RunId,
    /// The top-level agent name for the run (for a `Chain`/`Parallel` run, the entry-point
    /// agent).
    pub agent: String,
    /// Which shape of run this was.
    pub mode: RunMode,
    /// The run's final lifecycle state at the moment this file was written — always terminal
    /// (`Complete`/`Failed`) or `Paused` (an interrupt-induced early stop is still recorded via a
    /// result file per R-SA-084's soft-pause semantics, not withheld until an eventual resume).
    pub state: RunState,
    /// Whether the run is considered an overall success. Distinct from `state == Complete`
    /// because a `Complete` run whose steps individually failed acceptance is still `success:
    /// false` — this field is the OR'd/derived signal R-SA-100 requires downstream consumers to
    /// have available without re-deriving it from `steps`.
    pub success: bool,
    /// The working directory the run executed in.
    pub cwd: PathBuf,
    /// Path to the top-level persisted session transcript, if any (fork-context or a
    /// session-persisting fresh run).
    pub session_file: Option<PathBuf>,
    /// The LAUNCHING orchestrator's session — pi `sessionId: config.sessionId`
    /// (`subagent-runner.ts`'s result payload).
    ///
    /// Gates delivery at pi `result-watcher.ts:408` (a result with no session id is never
    /// delivered to anyone) and is the PARTITION KEY for
    /// `result-index/sessions/` ([`crate::background::result_index`]). Without it, every cyrup
    /// instance sharing this cwd consumes and deletes every other instance's results — the
    /// defect this field exists to fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// The launching PROCESS — pi `completionOwnerId`, the second half of `ownsCompletion`
    /// (`result-delivery-ownership.ts:22-26`).
    ///
    /// Distinct TYPE from [`ResultFile::session_id`] so the two can never be transposed in
    /// `owns(session, owner)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_owner_id: Option<CompletionOwnerId>,
    /// Per-child results, in the same fixed order as [`RunStatus::steps`] (R-SA-051 ordering
    /// preserved).
    pub results: Vec<SingleResult>,
    /// pi `status.workflowChildren` (`workflow-settlement.ts:231`) — the FINAL child inventory of
    /// a workflow run, stamped complete by settlement (its `inventory_complete` flag set) and by
    /// nothing else.
    ///
    /// `None` for every non-workflow run. Read by a future wait-completion projector onto
    /// `WaitCompletion.workflowChildren`, which additionally REJECTS a summary whose
    /// `workflow_run_id` does not match the run it was found on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_children: Option<crate::workflows::WorkflowChildSummary>,
    /// pi `publicResult.workflowReceipt` (`workflow-settlement.ts:235`) — `{ path, receipt }`,
    /// present only when a receipt was actually written (upstream deletes the key otherwise,
    /// `:238`; here the key is built conditionally and never inserted-then-removed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_receipt: Option<crate::workflows::WorkflowReceiptRef>,
    /// SUBA-016 — pi `scheduleOrigin` (`runs/background/scheduled-runs.ts:461`,
    /// `runs/background/notify.ts:59`): which SCHEDULE fired this run, when one did.
    ///
    /// `None` for every run a human or a model launched, which is all of them before SUBA-016.
    /// `Some` only for a run produced by
    /// [`crate::background::scheduled_runs::ScheduleLauncher::launch`], and it is what makes a
    /// scheduled completion distinguishable from any other — the one thing
    /// `background/watch/message.rs`'s `completion_notice_display` doc asked for by name
    /// ("When either input lands, OR it in here").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_origin: Option<ScheduleOrigin>,
}

/// pi `ScheduleOrigin` (`runs/background/notify.ts:59`, built at `scheduled-runs.ts:461`) — the
/// schedule that fired a run, carried on its result so the completion can name it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleOrigin {
    /// The schedule's id.
    pub id: String,
    /// Its display name, when it has one distinct from the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// pi `quiet?` — a successful completion of a QUIET schedule does not wake the parent's turn
    /// (`notify.ts:363-365`'s `scheduledCompletionTriggersTurn`). It is still DELIVERED and still
    /// displayed: quiet means "do not interrupt me", never "do not tell me".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<bool>,
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

    #[test]
    fn run_status_queued_starts_in_queued_state_with_no_end_time() {
        let status = RunStatus::queued(RunId::new(), RunMode::Single, Some(4242));
        assert_eq!(status.state, RunState::Queued);
        assert_eq!(status.pid, Some(4242));
        assert!(status.ended_at.is_none());
        assert_eq!(status.started_at, status.last_update);
        assert!(status.steps.is_empty());
    }

    #[test]
    fn run_status_provisional_matches_queued_shape() {
        let run_id = RunId::new();
        let provisional = RunStatus::provisional(run_id.clone(), RunMode::Chain, 99);
        assert_eq!(provisional.run_id, run_id);
        assert_eq!(provisional.mode, RunMode::Chain);
        assert_eq!(provisional.state, RunState::Queued);
        assert_eq!(provisional.pid, Some(99));
    }

    /// The recorder's two rules, which the scripted engine's emission pattern makes load-bearing:
    /// the SAME id collapses last-write-wins (the engine emits every `runs.host` call twice, a
    /// `Running` node then a terminal one), and a NEW id past `HOST_STEP_MAX_COUNT` is dropped
    /// rather than appended — a status carrying 33 rows could be neither projected
    /// (`validHostStepNodes` slices to 32) nor receipted (`build_workflow_receipt` throws).
    #[test]
    fn record_host_step_upserts_by_id_and_stops_at_the_host_step_cap() {
        let node =
            |id: &str, state: crate::workflows::HostStepState| crate::workflows::HostStepNode {
                version: crate::workflows::HostStepVersion,
                kind: crate::workflows::HostStepKind,
                monitor_kind: crate::workflows::HostStepMonitorKind::Command,
                id: id.to_string(),
                label: id.to_string(),
                role: None,
                provider: None,
                state,
                verdict: None,
                reason_code: None,
                detail: None,
                target: None,
                freshness: None,
                report_path: None,
                exit_code: None,
                updated_at: serde_json::Number::from(1),
                deadline_at: None,
            };
        let mut status = RunStatus::queued(RunId::new(), RunMode::Workflow, None);

        status.record_host_step(&node("gate", crate::workflows::HostStepState::Running));
        status.record_host_step(&node("gate", crate::workflows::HostStepState::Done));
        assert_eq!(
            status.telemetry.host_steps.len(),
            1,
            "the engine's Running-then-terminal pair is ONE row, not two"
        );
        assert_eq!(
            status.telemetry.host_steps[0].state,
            crate::workflows::HostStepState::Done,
            "last write wins"
        );
        assert!(
            crate::workflows::assert_unique_host_step_ids(&status.telemetry.host_steps, "status")
                .is_ok(),
            "a pushed duplicate would fail the receipt this same list is built from"
        );

        for index in 0..crate::workflows::HOST_STEP_MAX_COUNT + 4 {
            status.record_host_step(&node(
                &format!("gate-{index}"),
                crate::workflows::HostStepState::Done,
            ));
        }
        assert_eq!(
            status.telemetry.host_steps.len(),
            crate::workflows::HOST_STEP_MAX_COUNT,
            "the cap bounds NEW ids"
        );
        status.record_host_step(&node("gate", crate::workflows::HostStepState::Error));
        assert_eq!(
            status.telemetry.host_steps.len(),
            crate::workflows::HOST_STEP_MAX_COUNT,
            "an upsert onto a recorded id is never refused by the cap"
        );
        assert_eq!(
            status.telemetry.host_steps[0].state,
            crate::workflows::HostStepState::Error
        );
    }

    /// The field is `skip_serializing_if = "Vec::is_empty"` + `default`, so a run that never
    /// called `runs.host` writes the exact `status.json` shape it wrote before this field
    /// existed, and a status written by an older build still reads back.
    #[test]
    fn host_steps_are_absent_from_the_wire_when_a_run_produced_none() {
        let status = RunStatus::queued(RunId::new(), RunMode::Single, None);
        let json = serde_json::to_value(&status).expect("serializes");
        assert!(
            json.get("hostSteps").is_none(),
            "an empty list must not add a key: {json}"
        );
        let round: RunStatus = serde_json::from_value(json).expect("round-trips");
        assert!(round.telemetry.host_steps.is_empty());
    }

    #[test]
    fn run_status_advance_state_updates_last_update_and_leaves_ended_at_unset_for_non_terminal() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        let before = status.last_update;
        std::thread::sleep(std::time::Duration::from_millis(2));

        status
            .advance_state(RunState::Running)
            .expect("Queued -> Running is legal");

        assert_eq!(status.state, RunState::Running);
        assert!(status.last_update >= before);
        assert!(
            status.ended_at.is_none(),
            "Running is not terminal, ended_at must stay unset"
        );
    }

    #[test]
    fn run_status_advance_state_sets_ended_at_on_terminal_transition() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        status.advance_state(RunState::Running).expect("legal");
        status.advance_state(RunState::Complete).expect("legal");

        assert_eq!(status.state, RunState::Complete);
        assert!(status.ended_at.is_some());
    }

    #[test]
    fn run_status_advance_state_rejects_illegal_transition_and_leaves_state_unmodified() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        let before = status.clone();

        let result = status.advance_state(RunState::Complete);

        assert!(result.is_err(), "Queued -> Complete must be rejected");
        assert_eq!(
            status, before,
            "a rejected transition must leave the status entirely unmodified"
        );
    }

    #[test]
    fn run_status_advance_state_rejects_transition_out_of_terminal_state() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        status.advance_state(RunState::Running).expect("legal");
        status.advance_state(RunState::Failed).expect("legal");

        let result = status.advance_state(RunState::Running);
        assert!(
            result.is_err(),
            "a terminal Failed run must never transition again, even back to Running"
        );
    }

    #[test]
    fn run_status_touch_advances_last_update_without_changing_state() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Single, Some(1));
        status.advance_state(RunState::Running).expect("legal");
        let before = status.last_update;
        std::thread::sleep(std::time::Duration::from_millis(2));

        status.touch();

        assert_eq!(status.state, RunState::Running, "touch never changes state");
        assert!(status.last_update >= before);
    }

    #[test]
    fn run_status_round_trips_through_json() {
        let mut status = RunStatus::queued(RunId::new(), RunMode::Parallel, Some(7));
        status.steps.push(StepStatus::pending("researcher"));
        status.chain_step_count = Some(3);
        status.pending_appends = Some(0);

        let json = serde_json::to_string(&status).expect("serializes");
        let back: RunStatus = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, status);
    }

    // ---------------------------------------------------------------------------------------
    // StepStatus / ParallelGroupStatus
    // ---------------------------------------------------------------------------------------

    #[test]
    fn step_status_pending_has_no_timestamps_or_usage_yet() {
        let step = StepStatus::pending("reviewer");
        assert_eq!(step.agent, "reviewer");
        assert_eq!(step.status, StepState::Pending);
        assert!(step.started_at.is_none());
        assert!(step.ended_at.is_none());
        assert!(step.attempted_models.is_empty());
        assert!(step.nested_run_ids.is_empty());
    }

    /// The live-transcript stamps ride `status.json` under pi's own key names
    /// (`transcriptPath` / `transcriptError`, `subagent-runner.ts:3829-3830`), are omitted while
    /// absent, and a `status.json` written before the fields existed still reads back as `None`.
    #[test]
    fn step_transcript_fields_round_trip_under_pis_wire_names_and_default_when_absent() {
        let mut step = StepStatus::pending("worker");
        let wire = serde_json::to_value(&step).expect("serializes");
        assert!(wire.get("transcriptPath").is_none(), "{wire}");
        assert!(wire.get("transcriptError").is_none(), "{wire}");

        step.transcript_path = Some(PathBuf::from("/art/run_worker_0_transcript.jsonl"));
        step.transcript_error = Some("Failed to write child transcript '/art/x': EIO".into());
        let wire = serde_json::to_value(&step).expect("serializes");
        assert_eq!(wire["transcriptPath"], "/art/run_worker_0_transcript.jsonl");
        assert_eq!(
            wire["transcriptError"],
            "Failed to write child transcript '/art/x': EIO"
        );
        let back: StepStatus = serde_json::from_value(wire).expect("round-trips");
        assert_eq!(back, step);

        // Pre-field JSON: only the keys an older writer emitted.
        let legacy: StepStatus = serde_json::from_value(serde_json::json!({
            "agent": "worker", "status": "pending", "sessionFile": null, "model": null,
            "attemptedModels": [], "usage": cyrup_core::Usage::default(), "error": null,
            "nestedRunIds": [], "startedAt": null, "endedAt": null
        }))
        .expect("a status written before the transcript fields still parses");
        assert_eq!(legacy.transcript_path, None);
        assert_eq!(legacy.transcript_error, None);
    }

    /// A `SingleResult` decoded from a payload written BEFORE the transcript fields existed —
    /// only the keys an older writer emitted — which is itself the pre-field-JSON half of the
    /// contract below.
    fn pre_transcript_single_result() -> crate::exec::SingleResult {
        serde_json::from_value(serde_json::json!({
            "agent": "worker",
            "task": "",
            "exitCode": 0,
            "usage": cyrup_core::Usage::default(),
            "model": null,
            "attemptedModels": [],
            "modelAttempts": [],
            "finalOutput": null,
            "structuredOutput": null,
            "acceptance": null,
            "detached": false,
            "interrupted": false,
            "timedOut": false,
            "error": null,
            "toolCalls": [],
            "outputTruncated": false,
            "controlEvents": [],
            "progress": null,
        }))
        .expect("a result payload written before the transcript fields still decodes")
    }

    /// Same contract on the per-child `SingleResult` inside a `ResultFile` (pi
    /// `SingleResult.transcriptPath` / `transcriptError`, `subagent-runner.ts:276-277`).
    #[test]
    fn single_result_transcript_fields_round_trip_under_pis_wire_names_and_default_when_absent() {
        let mut result = pre_transcript_single_result();
        let wire = serde_json::to_value(&result).expect("serializes");
        assert!(wire.get("transcriptPath").is_none(), "{wire}");
        assert!(wire.get("transcriptError").is_none(), "{wire}");

        result.transcript_path = Some(PathBuf::from("/art/run_worker_0_transcript.jsonl"));
        result.transcript_error =
            Some("Failed to initialize child transcript '/art/x': EACCES".into());
        let wire = serde_json::to_value(&result).expect("serializes");
        assert_eq!(wire["transcriptPath"], "/art/run_worker_0_transcript.jsonl");
        assert_eq!(
            wire["transcriptError"],
            "Failed to initialize child transcript '/art/x': EACCES"
        );
        let back: crate::exec::SingleResult = serde_json::from_value(wire).expect("round-trips");
        assert_eq!(back, result);

        let mut legacy = serde_json::to_value(&result).expect("serializes");
        let object = legacy.as_object_mut().expect("object");
        object.remove("transcriptPath");
        object.remove("transcriptError");
        let legacy: crate::exec::SingleResult = serde_json::from_value(legacy)
            .expect("a result written before the fields still parses");
        assert_eq!(legacy.transcript_path, None);
        assert_eq!(legacy.transcript_error, None);
    }

    #[test]
    fn parallel_group_status_round_trips_through_json() {
        let group = ParallelGroupStatus {
            group_step_index: 2,
            children: vec![StepStatus::pending("a"), StepStatus::pending("b")],
        };
        let json = serde_json::to_string(&group).expect("serializes");
        let back: ParallelGroupStatus = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, group);
    }

    // ---------------------------------------------------------------------------------------
    // ResultFile
    // ---------------------------------------------------------------------------------------

    #[test]
    fn result_file_round_trips_through_json() {
        let run_id = RunId::new();
        let result = ResultFile {
            schedule_origin: None,
            id: run_id.clone(),
            run_id,
            agent: "researcher".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp/project"),
            session_file: None,
            session_id: None,
            completion_owner_id: None,
            results: Vec::new(),
            workflow_children: None,
            workflow_receipt: None,
        };
        let json = serde_json::to_string(&result).expect("serializes");
        let back: ResultFile = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, result);
    }

    // ---------------------------------------------------------------------------------------
    // StepStatus — the SCOPE_3d workflow identity fields
    // ---------------------------------------------------------------------------------------

    /// SCOPE_3d — the three step identity fields (and `SingleResult::child_run_id`) are additive:
    /// a `status.json`/result payload written before they existed still decodes (`None`), a
    /// populated one round-trips under pi's wire names, and an empty step's wire shape is
    /// unchanged (no new key leaks into a lean step object).
    #[test]
    fn workflow_identity_fields_decode_from_legacy_payloads_and_round_trip() {
        // Legacy decode: the serialized form of a pending step has none of the three keys —
        // by construction, so this IS the pre-SCOPE_3d shape.
        let legacy = serde_json::to_value(StepStatus::pending("researcher")).expect("serializes");
        let object = legacy.as_object().expect("a step is an object");
        for key in ["workflowKey", "runId", "sessionName"] {
            assert!(
                !object.contains_key(key),
                "{key} must be omitted while absent"
            );
        }
        let decoded: StepStatus = serde_json::from_value(legacy).expect("legacy decode");
        assert_eq!(decoded.workflow_key, None);
        assert_eq!(decoded.run_id, None);
        assert_eq!(decoded.session_name, None);

        // Populated round-trip, under pi's `AsyncStatus.steps[]` wire names — and the
        // `workflowKey` read routes through `WorkflowKey::parse` (§0.9), so a corrupt key on
        // disk fails to decode rather than smuggling an unvalidated string in.
        let mut step = StepStatus::pending("researcher");
        step.workflow_key = crate::workflows::WorkflowKey::parse("lane.a").ok();
        step.run_id = Some(RunId::from_token("wfchild00001"));
        step.session_name = Some("Lane A".to_string());
        let json = serde_json::to_value(&step).expect("serializes");
        assert_eq!(json["workflowKey"], serde_json::json!("lane.a"));
        assert_eq!(json["runId"], serde_json::json!("wfchild00001"));
        assert_eq!(json["sessionName"], serde_json::json!("Lane A"));
        let back: StepStatus = serde_json::from_value(json.clone()).expect("round-trips");
        assert_eq!(back, step);
        let mut corrupt = json;
        corrupt["workflowKey"] = serde_json::json!(".not-a-key");
        assert!(
            serde_json::from_value::<StepStatus>(corrupt).is_err(),
            "an invalid workflowKey on disk must not deserialize"
        );

        // The `SingleResult` mirror decodes from a legacy payload the same way.
        let stopped = crate::exec::SingleResult {
            child_run_id: Some(RunId::from_token("wfchild00001")),
            ..serde_json::from_value(serde_json::json!({
                "agent": "researcher",
                "task": "",
                "exitCode": 0,
                "usage": cyrup_core::Usage::default(),
                "model": null,
                "attemptedModels": [],
                "modelAttempts": [],
                "finalOutput": null,
                "structuredOutput": null,
                "acceptance": null,
                "detached": false,
                "interrupted": false,
                "timedOut": false,
                "error": null,
                "toolCalls": [],
                "outputTruncated": false,
                "controlEvents": [],
                "progress": null,
            }))
            .expect("a pre-SCOPE_3d result payload still decodes (child_run_id defaults)")
        };
        let round: crate::exec::SingleResult =
            serde_json::from_value(serde_json::to_value(&stopped).expect("serializes"))
                .expect("round-trips");
        assert_eq!(round.child_run_id, stopped.child_run_id);
    }
}
