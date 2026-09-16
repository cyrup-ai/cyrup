//! SCOPE_10 — the projection itself: pi `runs/shared/async-status-projection.ts:173-385,546-565`
//! (`@7fe9dee1`), everything reachable from `projectAsyncStatusSnapshot`.
//!
//! The input is [`crate::tui::fleet_state::AsyncRunView`] — cyrup's nearest thing to upstream's
//! in-memory `AsyncJobState` (`shared/types.ts:1968-2028`) and the shape the fleet surfaces
//! already produce. No new job record is invented for this file: a second in-memory job model
//! would immediately disagree with the first about which runs exist.
//!
//! # The four behaviours a re-derivation gets wrong
//!
//! These are ported deliberately, not incidentally:
//!
//! 1. **[`enforce_byte_limit`] is a BINARY SEARCH over retained runs** (`:369-385`), not a
//!    truncate. It sets `omitted.byteLimitExceeded` BEFORE searching and settles on `lower`.
//!    Slicing to the first run that fits gives a strictly smaller retained set for the same input.
//! 2. **[`AsyncStatusSnapshotState::from_wire`] never errors** — an unrecognised status word is
//!    `partial` (`:177`).
//! 3. **[`project_run`]'s child budget is reserve-then-emit-reversed** (`:340-355`): host steps
//!    are reserved FIRST out of `maxChildrenPerNode`, ordinary children take what is left, and
//!    the emitted order is `[...ordinary, ...hostSteps]`.
//! 4. **`endedAt` is emitted only for a terminal node** (`:238`/`:275`/`:337`) — a running node
//!    with a recorded end time must not carry one.
//!
//! # `[CYRUP-DELTA]`s
//!
//! * **`job.agents` does not exist.** Upstream's job record carries the agent roster directly;
//!   cyrup's carries [`crate::background::StepStatus::agent`] per step, so [`label_for_agents`]
//!   folds the step agents in step order. Same list, one hop further away.
//! * **`step.label` / `step.phase` collapse to the agent.** [`crate::background::StepStatus`]
//!   carries no user-facing label — the same delta [`crate::background::fleet_view`] and
//!   [`crate::background::run_status`] already record. A workflow GRAPH node does carry one, and
//!   [`project_workflow_graph_node`] passes it through, so the collapse is confined to the
//!   on-disk step list.
//! * **`step.children` has no source, and loses nothing.** Upstream's `AsyncJobStep.children` is
//!   a `NestedRunSummary[]`; cyrup's [`crate::background::StepStatus::nested_run_ids`] holds
//!   bare ids, and the RESOLVED summaries live on
//!   [`crate::tui::fleet_state::AsyncRunView::nested_children`] at the run level — which is
//!   exactly what upstream's own `job.nestedChildren` arm (`:347`) projects. The same nested runs
//!   therefore appear, once, under the run rather than twice.
//! * **`job.hostSteps` is STORED, not derived.** Upstream lifts it back out of
//!   `status.workflowGraph`'s `host-step` nodes; cyrup's graph has no such node kind, so the run
//!   carries the list directly on [`crate::background::RunTelemetry::host_steps`] under the same
//!   `hostSteps` wire key. Same input, one derivation fewer — see [`host_steps_for_run`].
//! * **`NestedRunSummary.children` / `.endedAt` have no source.** cyrup's
//!   [`crate::tui::fleet_state::NestedRunView`] is a one-level projection with no end timestamp
//!   (`tui/fleet_state.rs:70-100`), so the nested recursion bottoms out after one hop and a
//!   terminal nested run's `endedAt` falls back through upstream's own ladder to `lastUpdate`.

use std::collections::BTreeSet;

use super::types::{
    AsyncStatusSnapshot, AsyncStatusSnapshotActivity, AsyncStatusSnapshotCaps,
    AsyncStatusSnapshotHostStep, AsyncStatusSnapshotKind, AsyncStatusSnapshotNode,
    AsyncStatusSnapshotOmitted, AsyncStatusSnapshotOptions, AsyncStatusSnapshotState, public_count,
    public_optional_text, public_text, public_time, resolve_caps,
};
use super::{ASYNC_STATUS_SNAPSHOT_KIND, ASYNC_STATUS_SNAPSHOT_VERSION};
use crate::background::run_status::{run_mode_label, run_state_label, step_state_label};
use crate::background::{
    ActivityState, RunMode, RunStatus, StepStatus, WorkflowGraphNode, WorkflowGraphSnapshot,
    WorkflowNodeKind, WorkflowNodeStatus,
};
use crate::tui::fleet_state::{AsyncRunView, NestedRunView, NestedStepView};
use crate::workflows::{HOST_STEP_MAX_COUNT, HostStepNode, HostStepState, HostStepVerdict};

/// pi `ProjectionContext` (`:113-116`) — the caps plus the one mutable omission ledger every
/// projector shares. Threaded by `&mut` rather than returned, exactly as upstream mutates one
/// object: the counters must survive across sibling subtrees.
struct ProjectionContext {
    caps: AsyncStatusSnapshotCaps,
    omitted: AsyncStatusSnapshotOmitted,
}

/// The serde word [`ActivityState`] renders as (`background/telemetry.rs:21-22`,
/// `#[serde(rename_all = "snake_case")]`) — upstream's `activityState` arrives as that string, so
/// `activityFor`'s `typeof source.activityState === "string"` test is "is it `Some`" here.
fn activity_state_word(state: ActivityState) -> &'static str {
    match state {
        ActivityState::ActiveLongRunning => "active_long_running",
        ActivityState::NeedsAttention => "needs_attention",
    }
}

/// The six fields `activityFor` (`:195-216`) reads off whatever record it was handed — a run, a
/// step, or a nested child. One explicit record rather than three structurally-typed call sites,
/// because Rust has no structural typing and duplicating the reader three times is how the three
/// copies drift.
#[derive(Clone, Debug, Default)]
struct ActivityInput {
    activity_state: Option<ActivityState>,
    current_tool: Option<String>,
    last_activity_at: Option<i64>,
    current_tool_started_at: Option<i64>,
    turn_count: Option<u64>,
    tool_count: Option<u64>,
}

/// pi's `AsyncJobStep | NestedStepSummary` union parameter to `projectStep` (`:225`), as one
/// explicit input record for the same reason [`ActivityInput`] is one.
///
/// `label` is the TRUTHY label only: upstream's `"label" in step && step.label ? step.label :
/// step.agent` treats an empty label as absent, so a caller that has none — or has an empty one —
/// leaves this `None` and [`project_step`] falls through to `agent`.
#[derive(Clone, Debug, Default)]
struct StepInput<'a> {
    /// pi `step.workflowKey ?? step.runId`, already stringified. `None` falls through to
    /// `step:{index}`.
    id: Option<String>,
    /// pi `step.label`, when truthy.
    label: Option<String>,
    /// pi `step.agent`.
    agent: String,
    /// pi `step.status`, as the wire word [`AsyncStatusSnapshotState::from_wire`] normalizes.
    status: String,
    started_at: Option<i64>,
    ended_at: Option<i64>,
    activity: ActivityInput,
    /// pi `step.children` — see this module's `[CYRUP-DELTA]` on `step.children`; always empty
    /// for a cyrup-sourced step, and carried so the ported branch keeps its shape.
    children: &'a [NestedRunView],
}

/// pi `kindForMode` (`:184-186`).
fn kind_for_mode(mode: Option<RunMode>) -> AsyncStatusSnapshotKind {
    match mode {
        Some(RunMode::Workflow) => AsyncStatusSnapshotKind::Workflow,
        _ => AsyncStatusSnapshotKind::Subagent,
    }
}

/// pi `labelForAgents` (`:188-193`): up to three agents, comma-joined, with a `, +N more` tail.
fn label_for_agents(agents: &[String], fallback: &str, max_length: usize) -> String {
    if agents.is_empty() {
        return public_text(Some(fallback), fallback, max_length);
    }
    let visible = agents
        .iter()
        .take(3)
        .map(|agent| public_text(Some(agent), "agent", max_length))
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = if agents.len() > 3 {
        format!(", +{} more", agents.len() - 3)
    } else {
        String::new()
    };
    crate::workflows::truncate_display(&format!("{visible}{suffix}"), max_length)
}

/// pi `activityFor` (`:195-216`) — `undefined` when every member is absent (`:216`).
fn activity_for(
    source: &ActivityInput,
    ctx: &ProjectionContext,
) -> Option<AsyncStatusSnapshotActivity> {
    let activity = AsyncStatusSnapshotActivity {
        state: source.activity_state.map(|state| {
            public_text(
                Some(activity_state_word(state)),
                "unknown",
                ctx.caps.max_string_length,
            )
        }),
        current_tool: public_optional_text(
            source.current_tool.as_deref(),
            ctx.caps.max_string_length,
        ),
        last_activity_at: public_time(source.last_activity_at),
        current_tool_started_at: public_time(source.current_tool_started_at),
        turn_count: public_count(source.turn_count),
        tool_count: public_count(source.tool_count),
    };
    if activity.is_empty() {
        None
    } else {
        Some(activity)
    }
}

/// pi `appendBoundedChildren` (`:218-222`): take what fits under `maxChildrenPerNode`, count the
/// rest. `remaining` is computed against what `children` ALREADY holds, so a second call on the
/// same vector continues the same budget.
fn append_bounded_children(
    children: &mut Vec<AsyncStatusSnapshotNode>,
    source: Vec<AsyncStatusSnapshotNode>,
    ctx: &mut ProjectionContext,
) {
    let remaining = ctx
        .caps
        .max_children_per_node
        .saturating_sub(children.len());
    let source_len = source.len();
    children.extend(source.into_iter().take(remaining));
    ctx.omitted.children += source_len.saturating_sub(remaining);
}

/// pi `projectStep` (`:225-250`).
fn project_step(
    step: &StepInput<'_>,
    index: usize,
    depth: usize,
    ctx: &mut ProjectionContext,
) -> AsyncStatusSnapshotNode {
    let state = AsyncStatusSnapshotState::from_wire(&step.status);
    let started_at = public_time(step.started_at);
    let ended_at = public_time(step.ended_at);
    // `:230` — the ladder is `endedAt ?? lastActivityAt ?? startedAt`, in that order.
    let updated_at = ended_at
        .or(public_time(step.activity.last_activity_at))
        .or(started_at);
    let activity = activity_for(&step.activity, ctx);
    let fallback_id = format!("step:{index}");
    let mut node = AsyncStatusSnapshotNode {
        id: public_text(step.id.as_deref(), &fallback_id, ctx.caps.max_string_length),
        kind: AsyncStatusSnapshotKind::Step,
        label: public_text(
            Some(step.label.as_deref().unwrap_or(step.agent.as_str())),
            "step",
            ctx.caps.max_string_length,
        ),
        state,
        started_at,
        updated_at,
        ended_at: (state.is_terminal() && ended_at.is_some())
            .then_some(ended_at)
            .flatten(),
        activity,
        host_step: None,
        children: None,
    };
    if depth < ctx.caps.max_depth && !step.children.is_empty() {
        let nested: Vec<AsyncStatusSnapshotNode> = step
            .children
            .iter()
            .enumerate()
            .map(|(child_index, child)| project_nested_run(child, child_index, depth + 1, ctx))
            .collect();
        let mut bounded: Vec<AsyncStatusSnapshotNode> = Vec::new();
        append_bounded_children(&mut bounded, nested, ctx);
        if !bounded.is_empty() {
            node.children = Some(bounded);
        }
    } else if !step.children.is_empty() {
        ctx.omitted.children += step.children.len();
    }
    node
}

/// pi `workflowGraphStepStatus` (`async-status-projection.ts:449-458`) — the ONE helper of the
/// unreachable `projectAsyncWorkflowRows` half that IS reachable from here, via
/// [`project_workflow_graph_node`]. `completed -> complete`, `detached -> paused`, everything
/// else verbatim; [`AsyncStatusSnapshotState::from_wire`] then folds `pending` onto `queued`.
fn workflow_graph_step_status(status: WorkflowNodeStatus) -> &'static str {
    match status {
        WorkflowNodeStatus::Completed => "complete",
        WorkflowNodeStatus::Detached => "paused",
        WorkflowNodeStatus::Pending => "pending",
        WorkflowNodeStatus::Running => "running",
        WorkflowNodeStatus::Failed => "failed",
        WorkflowNodeStatus::Paused => "paused",
        WorkflowNodeStatus::Stopped => "stopped",
    }
}

/// pi `workflowGraphStageNodes` (`runs/shared/workflow-graph.ts:17-30`) — flatten a graph to its
/// STAGE nodes: descend through group containers, keep everything else.
///
/// [CYRUP-DELTA, unrepresentable] upstream's `node.kind !== "host-step"` guard (`:26`) is vacuous
/// here: [`WorkflowNodeKind`] has no host-step variant (`background/workflow_graph.rs:45-58`) —
/// a host monitor is a [`HostStepNode`], a separate record, never a graph node. The guard is
/// kept as the exhaustive `match` below rather than deleted, so the day a host-step kind is added
/// the compiler asks what to do with it.
fn workflow_graph_stage_nodes(graph: Option<&WorkflowGraphSnapshot>) -> Vec<&WorkflowGraphNode> {
    fn visit<'a>(node: &'a WorkflowGraphNode, stages: &mut Vec<&'a WorkflowGraphNode>) {
        match node.kind {
            WorkflowNodeKind::ParallelGroup | WorkflowNodeKind::DynamicParallelGroup => {
                for child in node.children.iter().flatten() {
                    visit(child, stages);
                }
            }
            WorkflowNodeKind::Step | WorkflowNodeKind::Agent => stages.push(node),
        }
    }
    let mut stages = Vec::new();
    for node in graph.map(|g| g.nodes.as_slice()).unwrap_or_default() {
        visit(node, &mut stages);
    }
    stages
}

/// pi `projectWorkflowGraphNode` (`:252-260`) — a graph stage rendered through the step
/// projector, keyed by the node id and indexed by its flat execution index.
fn project_workflow_graph_node(
    node: &WorkflowGraphNode,
    index: usize,
    depth: usize,
    ctx: &mut ProjectionContext,
) -> AsyncStatusSnapshotNode {
    let step = StepInput {
        id: Some(node.id.clone()),
        // `:254` — `agent: node.agent ?? node.label`, and `:255`'s `label: node.label`.
        label: Some(node.label.clone()).filter(|label| !label.is_empty()),
        agent: node
            .agent
            .clone()
            .filter(|agent| !agent.is_empty())
            .unwrap_or_else(|| node.label.clone()),
        status: workflow_graph_step_status(node.status).to_string(),
        started_at: None,
        ended_at: None,
        activity: ActivityInput::default(),
        children: &[],
    };
    project_step(&step, node.flat_index.unwrap_or(index), depth, ctx)
}

/// pi `projectNestedRun` (`:262-288`).
fn project_nested_run(
    child: &NestedRunView,
    index: usize,
    depth: usize,
    ctx: &mut ProjectionContext,
) -> AsyncStatusSnapshotNode {
    let state = AsyncStatusSnapshotState::from_wire(&child.state);
    let started_at = public_time(child.started_at);
    // [CYRUP-DELTA, unrepresentable] `NestedRunSummary.endedAt` has no field on
    // `NestedRunView`, so `:265`'s `lastUpdate ?? endedAt ?? lastActivityAt ?? startedAt` ladder
    // loses its second rung and the fourth (`lastActivityAt`) is the nested STEP's field, not the
    // run's. What survives is `lastUpdate ?? startedAt`, which is upstream's first and last rung.
    let ended_at = None;
    let updated_at = public_time(child.last_update).or(started_at);
    let activity = activity_for(
        &ActivityInput {
            activity_state: child.activity_state,
            current_tool: child.current_tool.clone(),
            ..ActivityInput::default()
        },
        ctx,
    );
    let fallback_id = format!("nested:{index}");
    let mode_label = child.mode.map_or("subagent", run_mode_label);
    let mut node = AsyncStatusSnapshotNode {
        id: public_text(Some(&child.id), &fallback_id, ctx.caps.max_string_length),
        kind: kind_for_mode(child.mode),
        label: match child.agent.as_deref().filter(|agent| !agent.is_empty()) {
            Some(agent) => public_text(Some(agent), "subagent", ctx.caps.max_string_length),
            None => label_for_agents(&child.agents, mode_label, ctx.caps.max_string_length),
        },
        state,
        started_at,
        updated_at,
        ended_at: (state.is_terminal()).then_some(ended_at).flatten(),
        activity,
        host_step: None,
        children: None,
    };
    if depth < ctx.caps.max_depth {
        let nested_steps: Vec<AsyncStatusSnapshotNode> = child
            .steps
            .iter()
            .enumerate()
            .map(|(step_index, step)| {
                project_step(&nested_step_input(step), step_index, depth + 1, ctx)
            })
            .collect();
        // `child.children` has no `NestedRunView` field — see this module's delta list — so
        // upstream's second source contributes nothing and the concatenation is the step list.
        let mut bounded: Vec<AsyncStatusSnapshotNode> = Vec::new();
        append_bounded_children(&mut bounded, nested_steps, ctx);
        if !bounded.is_empty() {
            node.children = Some(bounded);
        }
    } else {
        ctx.omitted.children += child.steps.len();
    }
    node
}

/// pi `NestedStepSummary` as [`project_step`]'s input (`:225`'s second union arm).
fn nested_step_input(step: &NestedStepView) -> StepInput<'static> {
    StepInput {
        // A nested step summary carries neither `workflowKey` nor `runId` upstream either, so the
        // `step:{index}` fallback is upstream's own answer here, not a cyrup substitution.
        id: None,
        label: None,
        agent: step.agent.clone(),
        status: step.status.clone(),
        started_at: step.started_at,
        ended_at: None,
        activity: ActivityInput {
            activity_state: step.activity_state,
            current_tool: step.current_tool.clone(),
            last_activity_at: step.last_activity_at,
            ..ActivityInput::default()
        },
        children: &[],
    }
}

/// One on-disk [`StepStatus`] as [`project_step`]'s input (`:225`'s first union arm,
/// `AsyncJobStep`).
fn status_step_input(step: &StepStatus) -> StepInput<'static> {
    StepInput {
        // `:227` — `workflowKey` first, then `runId`. Both are typed newtypes here.
        id: step
            .workflow_key
            .as_ref()
            .map(|key| key.as_str().to_string())
            .or_else(|| step.run_id.as_ref().map(|id| id.as_str().to_string())),
        // See this module's `step.label` delta: there is no field to read.
        label: None,
        agent: step.agent.clone(),
        status: step_state_label(step.status).to_string(),
        started_at: step.started_at,
        ended_at: step.ended_at,
        activity: ActivityInput {
            activity_state: step.telemetry.activity_state,
            current_tool: step.telemetry.current_tool.clone(),
            last_activity_at: step.telemetry.last_activity_at,
            current_tool_started_at: step.telemetry.current_tool_started_at,
            turn_count: step.telemetry.turn_count,
            tool_count: step.telemetry.tool_count,
        },
        children: &[],
    }
}

/// pi `hostStepSnapshotState` (`:290-296`) — the host-step vocabulary folded onto the snapshot's.
///
/// The `error || verdict === "fail"` arm is ordered AFTER `cancelled` deliberately: a cancelled
/// step that also recorded a failing verdict reports as stopped, because it was stopped.
#[must_use]
pub fn host_step_snapshot_state(
    state: HostStepState,
    verdict: Option<HostStepVerdict>,
) -> AsyncStatusSnapshotState {
    match state {
        HostStepState::Pending => AsyncStatusSnapshotState::Queued,
        HostStepState::Running => AsyncStatusSnapshotState::Running,
        HostStepState::Cancelled => AsyncStatusSnapshotState::Stopped,
        HostStepState::Error => AsyncStatusSnapshotState::Failed,
        HostStepState::Done => match verdict {
            Some(HostStepVerdict::Fail) => AsyncStatusSnapshotState::Failed,
            None | Some(HostStepVerdict::Inconclusive) => AsyncStatusSnapshotState::Partial,
            Some(HostStepVerdict::Pass) => AsyncStatusSnapshotState::Complete,
        },
    }
}

/// pi `hostStepReportName` (`runs/shared/host-step-status.ts:228-231`): the last non-empty
/// segment of the report path, split on BOTH separators, so a Windows-style path recorded by a
/// remote monitor still names its report.
fn host_step_report_name(report_path: Option<&str>) -> Option<&str> {
    report_path?
        .split(['/', '\\'])
        .rfind(|segment| !segment.is_empty())
}

/// pi `projectHostStep` (`:298-323`).
///
/// # This has no production caller in cyrup, and is ported anyway
///
/// [`host_steps_for_run`] explains why there is no input. Porting it unreachable-but-correct is
/// the crate's established answer to exactly this shape — see
/// [`crate::background::inspect_rpc`]'s `FAILED_OUTPUT_ARTIFACT_PREFIX` note, which ports a
/// provably-dead branch for the same reason: the branch is part of the FORMAT, and a format with
/// a hole in it is not a port. Its behaviour is pinned by this module's own tests.
#[must_use]
pub fn project_host_step(
    host_step: &HostStepNode,
    caps: &AsyncStatusSnapshotCaps,
) -> AsyncStatusSnapshotNode {
    let state = host_step_snapshot_state(host_step.state, host_step.verdict);
    let detail = public_optional_text(host_step.detail.as_deref(), caps.max_string_length);
    let report = public_optional_text(
        host_step_report_name(host_step.report_path.as_deref()),
        caps.max_string_length,
    );
    let updated_at = host_step.updated_at.as_i64();
    AsyncStatusSnapshotNode {
        id: public_text(Some(&host_step.id), "host-step", caps.max_string_length),
        kind: AsyncStatusSnapshotKind::HostStep,
        label: public_text(Some(&host_step.label), "host step", caps.max_string_length),
        state,
        started_at: None,
        // `:318` emits `updatedAt` UNCONDITIONALLY (it is not optional on a host step) and `:319`
        // repeats it as `endedAt` once terminal.
        updated_at,
        ended_at: state.is_terminal().then_some(updated_at).flatten(),
        activity: None,
        host_step: Some(AsyncStatusSnapshotHostStep {
            kind: host_step.monitor_kind,
            state: host_step.state,
            provider: host_step
                .provider
                .as_deref()
                .filter(|p| !p.is_empty())
                .map(|p| public_text(Some(p), "provider", caps.max_string_length)),
            role: host_step
                .role
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(|r| public_text(Some(r), "role", caps.max_string_length)),
            verdict: host_step.verdict,
            reason_code: host_step
                .reason_code
                .as_deref()
                .filter(|r| !r.is_empty())
                .map(|r| public_text(Some(r), "reason", caps.max_string_length)),
            detail,
            target: host_step
                .target
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(|t| public_text(Some(t), "target", caps.max_string_length)),
            stale: host_step.freshness.as_ref().and_then(|f| f.stale),
            report,
        }),
        children: None,
    }
}

/// pi `validHostStepList(job.hostSteps)` (`:132-143`, called at `:348`).
///
/// The input is [`crate::background::RunTelemetry::host_steps`], which the foreground workflow
/// dispatch fills one transition at a time from the scripted engine's `on_host_step` callback
/// (`extension/tool/routing.rs`, via [`crate::background::RunStatus::record_host_step`]) and
/// `settle_foreground_workflow` writes into the terminal `status.json`. So a run that called
/// `runs.host` carries its monitor rows here, and every other run carries none.
///
/// Upstream's list arrives as `HostStepNode[] | WorkflowGraphSnapshot`; the graph arm
/// (`validHostStepNodes`) is the one cyrup does not have, for the reason
/// [`crate::background::RunTelemetry::host_steps`] records — cyrup's graph has no `host-step`
/// node kind, so the list is stored rather than re-derived. The ARRAY arm is this function, and
/// it is the arm `projectRun` (`:348`) actually takes for a job record.
///
/// [CYRUP-DELTA] upstream's per-element `parseHostStepNode`/`catch` — *"Renderers must not turn
/// malformed host data into a fake child row"* — has no work to do here: a `HostStepNode` in a
/// `RunStatus` was already parsed by serde at the file boundary (`version`/`kind` are parse
/// outcomes, `workflows/host_step.rs:15-44`), so a malformed row cannot reach this slice. Only
/// the `slice(0, HOST_STEP_MAX_COUNT)` bound survives, and it survives for the same reason
/// upstream applies it after parsing: a status written by anything other than
/// [`crate::background::RunStatus::record_host_step`] — an older build, a hand-edited file — is
/// not bound by that recorder's own cap.
fn host_steps_for_run(job: &AsyncRunView) -> &[HostStepNode] {
    let host_steps = job.status.telemetry.host_steps.as_slice();
    let bounded = host_steps.len().min(HOST_STEP_MAX_COUNT);
    host_steps.get(..bounded).unwrap_or(&[])
}

/// The agent roster [`label_for_agents`] folds for a run — see this module's `job.agents` delta.
fn agents_for_run(status: &RunStatus) -> Vec<String> {
    status.steps.iter().map(|step| step.agent.clone()).collect()
}

/// pi `projectRun` (`:325-363`).
fn project_run(job: &AsyncRunView, ctx: &mut ProjectionContext) -> AsyncStatusSnapshotNode {
    let status = &job.status;
    let state = AsyncStatusSnapshotState::from_wire(run_state_label(status.state));
    let started_at = public_time(Some(status.started_at));
    let updated_at = public_time(Some(status.last_update)).or(started_at);
    let activity = activity_for(
        &ActivityInput {
            activity_state: status.telemetry.activity_state,
            current_tool: status.telemetry.current_tool.clone(),
            last_activity_at: status.telemetry.last_activity_at,
            // [CYRUP-DELTA, unrepresentable] `RunTelemetry` keeps `currentToolStartedAt` per STEP
            // (`background/telemetry.rs:96`), not run-wide; upstream's job record carries the
            // run-level copy. Absent rather than guessed from the current step.
            current_tool_started_at: None,
            turn_count: status.telemetry.turn_count,
            tool_count: status.telemetry.tool_count,
        },
        ctx,
    );
    let mut node = AsyncStatusSnapshotNode {
        id: public_text(
            Some(status.run_id.as_str()),
            "async",
            ctx.caps.max_string_length,
        ),
        kind: kind_for_mode(Some(status.mode)),
        label: label_for_agents(
            &agents_for_run(status),
            run_mode_label(status.mode),
            ctx.caps.max_string_length,
        ),
        state,
        started_at,
        updated_at,
        // `:337` — `endedAt: updatedAt`, NOT `status.ended_at`. cyrup does carry a real
        // `RunStatus::ended_at`, and substituting it here would be a divergence, not an
        // improvement: upstream's contract for this key is "the last update of a terminal run",
        // and a JS reader of this document compares it against exactly that.
        ended_at: state.is_terminal().then_some(updated_at).flatten(),
        activity,
        host_step: None,
        children: None,
    };
    if ctx.caps.max_depth > 0 {
        let step_children: Vec<AsyncStatusSnapshotNode> = status
            .steps
            .iter()
            .enumerate()
            .map(|(index, step)| project_step(&status_step_input(step), index, 1, ctx))
            .collect();
        // `:342` — a graph stage already represented by a LOADED step is not repeated.
        let loaded_keys: BTreeSet<&str> = status
            .steps
            .iter()
            .filter_map(|step| step.workflow_key.as_ref().map(|key| key.as_str()))
            .collect();
        let graph_stages = if status.mode == RunMode::Workflow {
            workflow_graph_stage_nodes(status.telemetry.workflow_graph.as_ref())
        } else {
            Vec::new()
        };
        let graph_children: Vec<AsyncStatusSnapshotNode> = graph_stages
            .iter()
            .filter(|node| !loaded_keys.contains(node.id.as_str()))
            .enumerate()
            .map(|(index, node)| project_workflow_graph_node(node, index, 1, ctx))
            .collect();
        let nested_children: Vec<AsyncStatusSnapshotNode> = job
            .nested_children
            .iter()
            .enumerate()
            .map(|(index, child)| project_nested_run(child, index, 1, ctx))
            .collect();
        let host_step_children: Vec<AsyncStatusSnapshotNode> = host_steps_for_run(job)
            .iter()
            .map(|host_step| project_host_step(host_step, &ctx.caps))
            .collect();
        // `:350-355` — RESERVE-then-emit-reversed: host steps take their slots out of the budget
        // FIRST (a host monitor is the row an operator is waiting on), ordinary children take
        // what is left, and the EMITTED order is `[...ordinary, ...hostSteps]`.
        let mut ordinary_children = step_children;
        ordinary_children.extend(graph_children);
        ordinary_children.extend(nested_children);
        let host_retained = ctx.caps.max_children_per_node.min(host_step_children.len());
        let ordinary_retained = ctx
            .caps
            .max_children_per_node
            .saturating_sub(host_retained)
            .min(ordinary_children.len());
        ctx.omitted.children += (ordinary_children.len() - ordinary_retained)
            + (host_step_children.len() - host_retained);
        let mut bounded: Vec<AsyncStatusSnapshotNode> = ordinary_children
            .into_iter()
            .take(ordinary_retained)
            .collect();
        bounded.extend(host_step_children.into_iter().take(host_retained));
        if !bounded.is_empty() {
            node.children = Some(bounded);
        }
    } else {
        // `:357-362` — nothing is projected, so everything that WOULD have been is counted.
        let loaded_keys: BTreeSet<&str> = status
            .steps
            .iter()
            .filter_map(|step| step.workflow_key.as_ref().map(|key| key.as_str()))
            .collect();
        let graph_count = if status.mode == RunMode::Workflow {
            workflow_graph_stage_nodes(status.telemetry.workflow_graph.as_ref())
                .iter()
                .filter(|node| !loaded_keys.contains(node.id.as_str()))
                .count()
        } else {
            0
        };
        ctx.omitted.children += status.steps.len()
            + graph_count
            + job.nested_children.len()
            + host_steps_for_run(job).len();
    }
    node
}

/// pi `snapshotBytes` (`:365-367`).
///
/// A failure to serialize reports ZERO — "it fits" — rather than a sentinel that would truncate
/// the document to nothing. It cannot happen for this shape (no non-string map keys, no
/// non-finite floats), and the non-destructive answer is the right one for a report if it ever
/// does.
fn snapshot_bytes(snapshot: &AsyncStatusSnapshot) -> usize {
    serde_json::to_vec(snapshot).map_or(0, |bytes| bytes.len())
}

/// pi `enforceByteLimit` (`:369-385`) — a BINARY SEARCH over the retained run count, not a
/// truncate.
///
/// The invariant the search keeps is `lower` = "known to fit", `upper` = "not known to fit", with
/// `retained = ceil((lower + upper) / 2)` so the probe always moves. `omitted.byteLimitExceeded`
/// is set before the first probe, and `omitted.runs` is rebased onto its PRE-search value at
/// every probe rather than accumulated — the cap-drop count and the budget-drop count are the
/// same counter, and double-counting it across probes would leave a nonsense total.
fn enforce_byte_limit(snapshot: &mut AsyncStatusSnapshot) {
    if snapshot_bytes(snapshot) <= snapshot.caps.max_serialized_bytes {
        return;
    }
    snapshot.omitted.byte_limit_exceeded = true;
    let runs = std::mem::take(&mut snapshot.runs);
    let initial_omitted_runs = snapshot.omitted.runs;
    let mut lower = 0usize;
    let mut upper = runs.len().saturating_sub(1);
    while lower < upper {
        // `Math.ceil((lower + upper) / 2)`, without the overflow `(lower + upper)` would risk:
        // `midpoint` is the floor, and the `+ 1` is the ceiling's correction for an odd sum.
        let retained = lower.midpoint(upper) + usize::from(!(lower + upper).is_multiple_of(2));
        snapshot.runs = runs.iter().take(retained).cloned().collect();
        snapshot.omitted.runs = initial_omitted_runs + runs.len() - retained;
        if snapshot_bytes(snapshot) <= snapshot.caps.max_serialized_bytes {
            lower = retained;
        } else {
            upper = retained - 1;
        }
    }
    snapshot.runs = runs.iter().take(lower).cloned().collect();
    snapshot.omitted.runs = initial_omitted_runs + runs.len() - lower;
}

/// pi `projectAsyncStatusSnapshot` (`:546-565`) — *"Project already-loaded async status facts into
/// the bounded public snapshot shape."*
///
/// Sorting is `updatedAt ?? startedAt ?? 0` DESCENDING, tie-broken by run id ASCENDING (`:549-553`).
/// Upstream's tie-break is `localeCompare`; cyrup run ids are ASCII tokens
/// (`background/run_id.rs`), so `str::cmp` is the same order with no collation table to reproduce.
///
/// `omitted.runs` is incremented by the cap overflow BEFORE slicing (`:554`), and
/// [`enforce_byte_limit`] then adds to that same counter.
#[must_use]
pub fn project_async_status_snapshot<'a, I>(
    jobs: I,
    options: &AsyncStatusSnapshotOptions,
) -> AsyncStatusSnapshot
where
    I: IntoIterator<Item = &'a AsyncRunView>,
{
    let caps = resolve_caps(options);
    let mut ctx = ProjectionContext {
        caps,
        omitted: AsyncStatusSnapshotOmitted::default(),
    };
    let mut sorted: Vec<&AsyncRunView> = jobs.into_iter().collect();
    sorted.sort_by(|left, right| {
        let sort_key = |job: &AsyncRunView| {
            public_time(Some(job.status.last_update))
                .or(public_time(Some(job.status.started_at)))
                .unwrap_or(0)
        };
        sort_key(right).cmp(&sort_key(left)).then_with(|| {
            left.status
                .run_id
                .as_str()
                .cmp(right.status.run_id.as_str())
        })
    });
    ctx.omitted.runs += sorted.len().saturating_sub(caps.max_runs);
    let runs: Vec<AsyncStatusSnapshotNode> = sorted
        .iter()
        .take(caps.max_runs)
        .map(|job| project_run(job, &mut ctx))
        .collect();
    let mut snapshot = AsyncStatusSnapshot {
        kind: ASYNC_STATUS_SNAPSHOT_KIND.to_string(),
        version: ASYNC_STATUS_SNAPSHOT_VERSION,
        generated_at: options
            .generated_at
            .unwrap_or_else(crate::time::now_epoch_millis),
        caps,
        omitted: ctx.omitted,
        runs,
    };
    enforce_byte_limit(&mut snapshot);
    snapshot
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::testfixtures::run_view;
    use super::*;
    use crate::workflows::{HostStepKind, HostStepMonitorKind, HostStepVersion};

    fn options(max_serialized_bytes: Option<i64>) -> AsyncStatusSnapshotOptions {
        AsyncStatusSnapshotOptions {
            // A FIXED clock: `generatedAt` is the one field whose width would otherwise wobble
            // between a document and the probe it is compared against.
            generated_at: Some(1_700_000_000_000),
            max_serialized_bytes,
            ..AsyncStatusSnapshotOptions::default()
        }
    }

    /// `:549-553` — `updatedAt ?? startedAt ?? 0` DESCENDING, ties broken by run id ASCENDING.
    /// The tie-break is not decoration: without it a `HashMap`-sourced job list renders in a
    /// different order on every process, and a widget keyed on position flickers.
    #[test]
    fn the_snapshot_sorts_by_updated_at_desc_then_async_id() {
        let jobs = [
            run_view("run0bbb00000", Some("s"), 20),
            run_view("run0aaa00000", Some("s"), 20),
            run_view("run0ccc00000", Some("s"), 90),
        ];
        let snapshot = project_async_status_snapshot(jobs.iter(), &options(None));
        let ids: Vec<&str> = snapshot.runs.iter().map(|run| run.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["run0ccc00000", "run0aaa00000", "run0bbb00000"],
            "newest first, then ascending id among equals"
        );
    }

    /// `:173-178` — both rewrites and the fall-through. The fall-through is the important one: a
    /// snapshot is a report, and a report that errors on an unfamiliar status word is worse than
    /// one that says `partial`.
    #[test]
    fn the_snapshot_normalizes_completed_and_pending_and_falls_back_to_partial() {
        assert_eq!(
            AsyncStatusSnapshotState::from_wire("completed"),
            AsyncStatusSnapshotState::Complete
        );
        assert_eq!(
            AsyncStatusSnapshotState::from_wire("pending"),
            AsyncStatusSnapshotState::Queued
        );
        assert_eq!(
            AsyncStatusSnapshotState::from_wire("rejected"),
            AsyncStatusSnapshotState::Rejected
        );
        assert_eq!(
            AsyncStatusSnapshotState::from_wire("something-this-build-never-heard-of"),
            AsyncStatusSnapshotState::Partial
        );

        // And through a real projection: a NESTED run carries its state as a free string
        // (`tui/fleet_state.rs`'s own note on why), so it is the one input that can actually
        // present an unrecognised word.
        let mut job = run_view("run0aaa00000", Some("s"), 10);
        job.nested_children = vec![
            NestedRunView {
                id: "nested-completed".to_string(),
                state: "completed".to_string(),
                ..NestedRunView::default()
            },
            NestedRunView {
                id: "nested-unknown".to_string(),
                state: "wedged".to_string(),
                ..NestedRunView::default()
            },
        ];
        let snapshot = project_async_status_snapshot([&job], &options(None));
        let children = snapshot.runs[0].children.as_ref().expect("nested children");
        assert_eq!(children[0].state, AsyncStatusSnapshotState::Complete);
        assert_eq!(children[1].state, AsyncStatusSnapshotState::Partial);
    }

    /// The `...(x ? { x } : {})` spread contract (`:232-237` and every sibling literal): an absent
    /// optional is an ABSENT KEY, never `null`. A `null` here is a port defect — a TypeScript
    /// reader declaring `startedAt?: number` accepts a missing key and rejects a null one.
    #[test]
    fn the_snapshot_omits_absent_optionals_rather_than_serializing_null() {
        let mut job = run_view("run0aaa00000", Some("s"), 10);
        job.status.steps = vec![StepStatus::pending("scout")];
        let snapshot = project_async_status_snapshot([&job], &options(None));
        let json = serde_json::to_string(&snapshot).expect("serialize");
        assert!(
            !json.contains("null"),
            "no key may serialize as null: {json}"
        );

        // A QUEUED step has no activity, no timestamps and is not terminal, so all four of the
        // node's optionals are absent rather than present-and-empty.
        let step = &snapshot.runs[0].children.as_ref().expect("steps")[0];
        assert!(step.activity.is_none() && step.started_at.is_none() && step.ended_at.is_none());
        let value = serde_json::to_value(step).expect("to value");
        let object = value.as_object().expect("node is an object");
        for absent in [
            "startedAt",
            "updatedAt",
            "endedAt",
            "activity",
            "hostStep",
            "children",
        ] {
            assert!(
                !object.contains_key(absent),
                "{absent} must be absent: {json}"
            );
        }
    }

    /// `:369-385` — the byte budget is fitted by BINARY SEARCH over the retained runs, and the
    /// result is the MAXIMUM that fits, not merely fewer. A naive "slice until it fits" pass
    /// retains a strictly smaller set for the same input, which is why maximality is asserted
    /// here rather than just `runs.len() < jobs.len()`.
    #[test]
    fn the_snapshot_byte_limit_binary_searches_the_retained_runs() {
        let jobs: Vec<AsyncRunView> = (0..12)
            .map(|i| run_view(&format!("run0{i:08}"), Some("s"), 100 - i64::from(i)))
            .collect();
        // A budget measured from the document itself, so the test does not hard-code a size that
        // a future field would invalidate.
        let unbounded = project_async_status_snapshot(jobs.iter(), &options(Some(1 << 20)));
        assert_eq!(unbounded.runs.len(), 12);
        assert!(!unbounded.omitted.byte_limit_exceeded);
        let mut four = unbounded.clone();
        four.runs.truncate(4);
        let cap = i64::try_from(serde_json::to_vec(&four).expect("serialize").len())
            .expect("a test document fits in an i64");

        let bounded = project_async_status_snapshot(jobs.iter(), &options(Some(cap)));
        assert!(
            bounded.omitted.byte_limit_exceeded,
            "the flag is set the moment the budget binds, before the search runs"
        );
        let kept = bounded.runs.len();
        assert!(
            kept > 0 && kept < 12,
            "the budget bound the document: {kept}"
        );
        assert_eq!(
            bounded.omitted.runs,
            12 - kept,
            "the byte-budget drops are added to the SAME counter the cap uses"
        );
        assert!(
            serde_json::to_vec(&bounded).expect("serialize").len()
                <= bounded.caps.max_serialized_bytes,
            "what the search returned actually fits"
        );

        // Maximality: one more run does NOT fit. This is the assertion a truncate fails.
        let mut probe = bounded.clone();
        probe.runs = unbounded.runs.iter().take(kept + 1).cloned().collect();
        probe.omitted.runs = 12 - (kept + 1);
        assert!(
            serde_json::to_vec(&probe).expect("serialize").len()
                > bounded.caps.max_serialized_bytes,
            "the search must settle on the LARGEST retained set that fits, not the first"
        );
    }

    /// `:350-355`, the DEGENERATE half of the reserve: with no host steps the reservation is zero
    /// and the ordinary children get the whole budget. The reserving half — a run that DOES carry
    /// host steps — is `host_steps_reserve_their_slots_first_and_are_emitted_last` below.
    #[test]
    fn the_snapshot_bounds_children_per_node_and_counts_what_it_dropped() {
        let mut job = run_view("run0aaa00000", Some("s"), 10);
        job.status.steps = (0..11)
            .map(|i| StepStatus::pending(format!("agent-{i}")))
            .collect();
        let snapshot = project_async_status_snapshot([&job], &options(None));
        let children = snapshot.runs[0].children.as_ref().expect("children");
        assert_eq!(children.len(), 8, "DEFAULT_MAX_CHILDREN_PER_NODE");
        assert_eq!(snapshot.omitted.children, 3);
        assert_eq!(children[0].label, "agent-0", "in step order");
    }

    /// `:350-355`, the half that only a run with real host steps can exercise: host steps take
    /// their slots out of `maxChildrenPerNode` FIRST (a host monitor is the row an operator is
    /// waiting on), the ordinary children take what is left, and the EMITTED order is
    /// `[...ordinary, ...hostSteps]` — reserved first, emitted last.
    ///
    /// A re-derivation gets this backwards in two different ways, and both are asserted here: it
    /// gives the ordinary children the whole budget and appends host steps past the cap (the
    /// budget would then be 8 + 2), or it emits the host steps first because they were reserved
    /// first.
    #[test]
    fn host_steps_reserve_their_slots_first_and_are_emitted_last() {
        let mut job = run_view("run0aaa00000", Some("s"), 10);
        job.status.mode = RunMode::Workflow;
        job.status.steps = (0..11)
            .map(|i| StepStatus::pending(format!("agent-{i}")))
            .collect();
        for id in ["ci", "gate"] {
            job.status.record_host_step(&HostStepNode {
                version: HostStepVersion,
                kind: HostStepKind,
                monitor_kind: HostStepMonitorKind::Command,
                id: id.to_string(),
                label: format!("{id} monitor"),
                role: None,
                provider: None,
                state: HostStepState::Done,
                verdict: Some(HostStepVerdict::Pass),
                reason_code: None,
                detail: None,
                target: None,
                freshness: None,
                report_path: None,
                exit_code: None,
                updated_at: serde_json::Number::from(77),
                deadline_at: None,
            });
        }

        let snapshot = project_async_status_snapshot([&job], &options(None));
        let children = snapshot.runs[0].children.as_ref().expect("children");
        assert_eq!(
            children.len(),
            8,
            "DEFAULT_MAX_CHILDREN_PER_NODE bounds the COMBINED set, not the ordinary half"
        );
        let kinds: Vec<AsyncStatusSnapshotKind> = children.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            vec![
                AsyncStatusSnapshotKind::Step,
                AsyncStatusSnapshotKind::Step,
                AsyncStatusSnapshotKind::Step,
                AsyncStatusSnapshotKind::Step,
                AsyncStatusSnapshotKind::Step,
                AsyncStatusSnapshotKind::Step,
                AsyncStatusSnapshotKind::HostStep,
                AsyncStatusSnapshotKind::HostStep,
            ],
            "six ordinary children then the two reserved host steps"
        );
        let ids: Vec<&str> = children[6..].iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["ci", "gate"], "in recorded order");
        assert_eq!(
            snapshot.omitted.children, 5,
            "eleven steps minus the six that fit — the two host steps were NOT dropped"
        );
    }

    /// The bound `host_steps_for_run` keeps for a status it did not write: a `status.json` from an
    /// older build (or a hand-edited one) can carry more than `HOST_STEP_MAX_COUNT` rows, and
    /// upstream slices rather than trusting them (`host-step-status.ts:141`).
    #[test]
    fn the_projection_slices_a_status_carrying_more_than_the_host_step_cap() {
        let mut job = run_view("run0aaa00000", Some("s"), 10);
        job.status.mode = RunMode::Workflow;
        job.status.telemetry.host_steps = (0..HOST_STEP_MAX_COUNT + 3)
            .map(|i| HostStepNode {
                version: HostStepVersion,
                kind: HostStepKind,
                monitor_kind: HostStepMonitorKind::Ci,
                id: format!("gate-{i}"),
                label: format!("gate {i}"),
                role: None,
                provider: None,
                state: HostStepState::Running,
                verdict: None,
                reason_code: None,
                detail: None,
                target: None,
                freshness: None,
                report_path: None,
                exit_code: None,
                updated_at: serde_json::Number::from(5),
                deadline_at: None,
            })
            .collect();

        let snapshot = project_async_status_snapshot([&job], &options(None));
        let children = snapshot.runs[0].children.as_ref().expect("children");
        assert!(
            children
                .iter()
                .all(|c| c.kind == AsyncStatusSnapshotKind::HostStep),
            "host steps reserved the whole budget"
        );
        assert_eq!(
            snapshot.omitted.children,
            HOST_STEP_MAX_COUNT - children.len(),
            "only the 32 SLICED rows are counted against the budget — the 3 beyond the cap were              never candidates"
        );
    }

    /// `:290-323`, the host-step projector, unit-tested at its own boundary: the state/verdict
    /// fold's seven arms, the `endedAt`-only-when-terminal rule, and `report` being the report's
    /// BASENAME rather than its path. Its INPUT reaching a real run is
    /// [`host_steps_for_run`]'s job and is proven end-to-end by
    /// `extension::tool::routing::tests::a_workflow_host_step_reaches_the_async_status_snapshot`.
    #[test]
    fn the_host_step_projection_folds_state_and_verdict_and_names_only_the_report_file() {
        let caps = resolve_caps(&AsyncStatusSnapshotOptions::default());
        let node = |state: HostStepState, verdict: Option<HostStepVerdict>| HostStepNode {
            version: HostStepVersion,
            kind: HostStepKind,
            monitor_kind: HostStepMonitorKind::Ci,
            id: "gate-1".to_string(),
            label: "CI gate".to_string(),
            role: None,
            provider: Some("github".to_string()),
            state,
            verdict,
            reason_code: None,
            detail: None,
            target: None,
            freshness: None,
            report_path: Some("reports/nested/ci-run.md".to_string()),
            exit_code: None,
            updated_at: serde_json::Number::from(42),
            deadline_at: None,
        };

        // The fold (`:291-295`), every arm.
        assert_eq!(
            host_step_snapshot_state(HostStepState::Pending, None),
            AsyncStatusSnapshotState::Queued
        );
        assert_eq!(
            host_step_snapshot_state(HostStepState::Running, None),
            AsyncStatusSnapshotState::Running
        );
        assert_eq!(
            host_step_snapshot_state(HostStepState::Cancelled, Some(HostStepVerdict::Fail)),
            AsyncStatusSnapshotState::Stopped,
            "a cancelled step reports as stopped even with a failing verdict recorded"
        );
        assert_eq!(
            host_step_snapshot_state(HostStepState::Error, None),
            AsyncStatusSnapshotState::Failed
        );
        assert_eq!(
            host_step_snapshot_state(HostStepState::Done, Some(HostStepVerdict::Pass)),
            AsyncStatusSnapshotState::Complete
        );
        assert_eq!(
            host_step_snapshot_state(HostStepState::Done, Some(HostStepVerdict::Inconclusive)),
            AsyncStatusSnapshotState::Partial
        );
        assert_eq!(
            host_step_snapshot_state(HostStepState::Done, None),
            AsyncStatusSnapshotState::Partial
        );

        let running = project_host_step(&node(HostStepState::Running, None), &caps);
        assert_eq!(running.kind, AsyncStatusSnapshotKind::HostStep);
        assert_eq!(running.updated_at, Some(42));
        assert!(
            running.ended_at.is_none(),
            "a running node must not carry an endedAt"
        );
        let settled = project_host_step(
            &node(HostStepState::Done, Some(HostStepVerdict::Pass)),
            &caps,
        );
        assert_eq!(settled.ended_at, Some(42));
        let metadata = settled.host_step.as_ref().expect("host step metadata");
        assert_eq!(metadata.report.as_deref(), Some("ci-run.md"));
        assert_eq!(metadata.provider.as_deref(), Some("github"));
        assert!(metadata.role.is_none() && metadata.stale.is_none());
    }
}
