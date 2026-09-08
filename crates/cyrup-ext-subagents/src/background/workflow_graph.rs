//! Workflow-graph snapshot (pi `runs/shared/workflow-graph.ts:73-206`)
//!
//! A faithful port of pi's `buildWorkflowGraphSnapshot`: given a run's declared step list plus its
//! per-step results/statuses, produces the node-id/phase/group-status-precedence/dynamic-metadata/
//! `currentNodeId` snapshot a status-reading UI renders (`shared/types.ts:33-65`). Node ids are the
//! exact pi shapes: `step-<N>` for a sequential step or a group, `step-<N>-agent-<M>` for a static
//! parallel child, `step-<N>-item-<key>` for a dynamic fan-out child. This is a pure function over
//! plain data — no filesystem, no discovery — so it reproduces `workflow-graph.test.ts` scenario for
//! scenario.
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use super::{RunMode, RunStatus, StepState};

/// A workflow node's lifecycle state (pi `WorkflowNodeStatus`, `shared/types.ts:40`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeStatus {
    /// Declared but not yet started.
    Pending,
    /// Currently running.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with a failure.
    Failed,
    /// Interrupted mid-flight (soft pause).
    Paused,
    /// G77 — pi `WorkflowNodeStatus` includes `"stopped"` (`shared/types.ts:40`), and the detached
    /// runner's own graph refresh normalizes a `"stopped"` step onto it and rolls it up over its
    /// children (`subagent-runner.ts:2163-2188`). Distinct from [`Self::Failed`]: upstream's
    /// `updateNode` tail is `if (node.error && node.status !== "stopped" && node.status !==
    /// "rejected") node.status = "failed"` — i.e. a stopped node keeps its status even though the
    /// stop stamped an `error` onto it, precisely so a stop is never re-rendered as a failure.
    Stopped,
    /// Detached (fire-and-forget) and no longer tracked inline.
    Detached,
}

/// A workflow node's structural kind (pi `WorkflowGraphNode.kind`, `shared/types.ts:37`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkflowNodeKind {
    /// A single sequential step.
    Step,
    /// A static-width parallel group container.
    ParallelGroup,
    /// A dynamic (runtime-width) fan-out group container.
    DynamicParallelGroup,
    /// One concurrently-dispatched child agent within a group.
    Agent,
}

/// The run-shape tag on a [`WorkflowGraphSnapshot`] (pi `WorkflowGraphSnapshot.mode`,
/// `shared/types.ts:105`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunMode {
    /// A linear chain.
    Chain,
    /// A standalone static parallel fan-out.
    Parallel,
    /// One single agent invocation.
    Single,
    /// A script-driven workflow run — pi `WorkflowGraphSnapshot.mode` is typed `SubagentRunMode`
    /// (`shared/types.ts:105`, `:400`) and therefore carries all FOUR shapes. This enum was
    /// originally ported with three because the fourth was unreachable before
    /// [`crate::background::RunMode::Workflow`] existed; it is not a narrower type upstream.
    Workflow,
}

/// The `dynamic` metadata block on a dynamic-fan-out group node (pi `WorkflowGraphNode.dynamic`,
/// `shared/types.ts:45-51`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowDynamicMeta {
    /// The named output the fan-out expands from.
    pub source_output: String,
    /// The JSON-pointer path within that output the array lives at.
    pub source_path: String,
    /// The per-item variable name (`item` by default).
    pub item_name: String,
    /// The optional `maxItems` cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<usize>,
    /// The named output the collected results register under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collect_as: Option<String>,
}

/// One node of a [`WorkflowGraphSnapshot`] (pi `WorkflowGraphNode`, `shared/types.ts:42-74`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGraphNode {
    /// The stable node id (`step-<N>` / `step-<N>-agent-<M>` / `step-<N>-item-<key>`).
    pub id: String,
    /// The node's structural kind.
    pub kind: WorkflowNodeKind,
    /// The agent this node invokes, for a step/agent node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The declared phase this node belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// The human-readable label.
    pub label: String,
    /// The node's lifecycle status.
    pub status: WorkflowNodeStatus,
    /// The node's flat (execution-order) index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat_index: Option<usize>,
    /// The node's declared step index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
    /// The item key, for a dynamic fan-out child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_key: Option<String>,
    /// The named output this node's result registers under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_name: Option<String>,
    /// Whether this node produces a structured (schema-validated) output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured: Option<bool>,
    /// This node's acceptance-ledger status, when evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_status: Option<String>,
    /// This node's error text, when failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The dynamic-fan-out metadata, for a dynamic group node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamic: Option<WorkflowDynamicMeta>,
    /// This node's children, for a group container (present, possibly empty, only for a group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<WorkflowGraphNode>>,
}

/// A named phase grouping node ids (pi `WorkflowGraphSnapshot.phases[]`, `shared/types.ts:62`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    /// The phase title.
    pub title: String,
    /// The ids of every node in this phase, in declaration order.
    pub node_ids: Vec<String>,
}

/// The full workflow-graph snapshot (pi `WorkflowGraphSnapshot`, `shared/types.ts:76-82`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowGraphSnapshot {
    /// The run's id.
    pub run_id: String,
    /// The run shape.
    pub mode: WorkflowRunMode,
    /// The declared phases.
    pub phases: Vec<WorkflowPhase>,
    /// The nodes, in declaration/execution order.
    pub nodes: Vec<WorkflowGraphNode>,
    /// The id of the currently-active node, when one is determinable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_node_id: Option<String>,
}

/// One declared sequential step, or one static-parallel/dynamic child spec, as an input to
/// [`build_workflow_graph_snapshot`] (pi's `SequentialStep`/parallel-task/`DynamicParallelStep`
/// fields, projected to only what the graph builder reads).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowTaskSpec {
    /// The agent name.
    pub agent: Option<String>,
    /// The declared phase.
    pub phase: Option<String>,
    /// The declared label.
    pub label: Option<String>,
    /// The named output (`as`).
    pub output_name: Option<String>,
    /// Whether this task produces a structured output (`Boolean(outputSchema)`).
    pub structured: bool,
}

/// The dynamic-fan-out shape of one declared step (pi `DynamicParallelStep`, projected).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowDynamicStepSpec {
    /// `expand.from.output`.
    pub expand_from_output: String,
    /// `expand.from.path`.
    pub expand_from_path: String,
    /// `expand.item` (`item` by default).
    pub item_name: Option<String>,
    /// `expand.maxItems`.
    pub max_items: Option<usize>,
    /// The whole step's `phase`/`label` (fallbacks for the group node).
    pub step_phase: Option<String>,
    /// The whole step's `label`.
    pub step_label: Option<String>,
    /// `parallel.phase`/`parallel.label` (the per-item template's phase/label).
    pub template_phase: Option<String>,
    /// `parallel.label`.
    pub template_label: Option<String>,
    /// `collect.as`.
    pub collect_as: String,
    /// `Boolean(collect.outputSchema)`.
    pub collect_structured: bool,
}

/// One declared workflow step, as an input to [`build_workflow_graph_snapshot`] — the union
/// pi's `ChainStep` is (`isParallelStep`/`isDynamicParallelStep`/sequential, `settings.ts`).
#[derive(Clone, Debug, PartialEq)]
pub enum WorkflowInputStep {
    /// A single sequential step.
    Sequential(WorkflowTaskSpec),
    /// A static-width parallel group over these tasks.
    Parallel(Vec<WorkflowTaskSpec>),
    /// A dynamic (runtime-width) fan-out.
    Dynamic(WorkflowDynamicStepSpec),
}

/// One materialized dynamic fan-out child, keyed by its resolved `itemKey`
/// (pi `WorkflowGraphBuildInput.dynamicChildren`, `workflow-graph.ts:12`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowDynamicChild {
    /// The child's agent.
    pub agent: String,
    /// The child's label override.
    pub label: Option<String>,
    /// The child's flat index.
    pub flat_index: usize,
    /// The resolved item key (sanitized into the node id).
    pub item_key: String,
    /// The child's named output.
    pub output_name: Option<String>,
    /// Whether the child produces a structured output.
    pub structured: bool,
    /// The child's own error text.
    pub error: Option<String>,
}

/// A per-step result summary the builder reads for status derivation (pi
/// `WorkflowGraphBuildInput.results[]`, `workflow-graph.ts:8`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowResultSummary {
    /// The child's exit code (`0` = success).
    pub exit_code: Option<i32>,
    /// Whether the child detached.
    pub detached: bool,
    /// Whether the child was interrupted (soft pause).
    pub interrupted: bool,
    /// The child's error text.
    pub error: Option<String>,
    /// The child's acceptance-ledger status.
    pub acceptance_status: Option<String>,
}

/// A per-step status override the builder reads (pi `WorkflowGraphBuildInput.stepStatuses[]`,
/// `workflow-graph.ts:11`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowStepStatusInput {
    /// The raw status string (`"complete"`/`"running"`/… normalized by the builder).
    pub status: Option<String>,
    /// The step's error text.
    pub error: Option<String>,
}

/// A dynamic-group status override (pi `WorkflowGraphBuildInput.dynamicGroupStatuses`,
/// `workflow-graph.ts:13`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowDynamicGroupStatus {
    /// The forced group status (empty-skip `completed`, aggregate-failure `failed`, …).
    pub status: WorkflowNodeStatus,
    /// The group's error text.
    pub error: Option<String>,
    /// The group's acceptance-ledger status.
    pub acceptance_status: Option<String>,
}

/// The full input to [`build_workflow_graph_snapshot`] (pi `WorkflowGraphBuildInput`,
/// `workflow-graph.ts:4-14`).
#[derive(Clone, Debug, Default)]
pub struct WorkflowGraphBuildInput {
    /// The run's id.
    pub run_id: String,
    /// The run shape (`chain` by default, mirroring pi's `input.mode ?? "chain"`).
    pub mode: Option<WorkflowRunMode>,
    /// The declared steps.
    pub steps: Vec<WorkflowInputStep>,
    /// Per-flat-index result summaries.
    pub results: Vec<WorkflowResultSummary>,
    /// The currently-running flat index.
    pub current_flat_index: Option<usize>,
    /// The currently-running step index.
    pub current_step_index: Option<usize>,
    /// Per-flat-index status overrides.
    pub step_statuses: Vec<WorkflowStepStatusInput>,
    /// Materialized dynamic children, keyed by step index.
    pub dynamic_children: std::collections::BTreeMap<usize, Vec<WorkflowDynamicChild>>,
    /// Dynamic-group status overrides, keyed by step index.
    pub dynamic_group_statuses: std::collections::BTreeMap<usize, WorkflowDynamicGroupStatus>,
}

/// Normalize a raw status string to a [`WorkflowNodeStatus`] (pi `normalizeStatus`,
/// `workflow-graph.ts:16-34`) — `None` for an unrecognized value so the caller can fall through.
fn normalize_workflow_status(status: Option<&str>) -> Option<WorkflowNodeStatus> {
    match status {
        Some("complete" | "completed") => Some(WorkflowNodeStatus::Completed),
        Some("running") => Some(WorkflowNodeStatus::Running),
        Some("failed") => Some(WorkflowNodeStatus::Failed),
        Some("paused") => Some(WorkflowNodeStatus::Paused),
        // G77: `workflow-graph.ts`'s own `normalizeStatus` has no `"stopped"` case, but the
        // detached runner's graph refresh — which in cyrup is FUSED into this same builder via
        // [`build_workflow_graph_from_runner_steps`] rather than living as a second local
        // normalizer — does (`subagent-runner.ts:2163-2166`: `if (status === "running" || … ||
        // status === "stopped" || …) return status`). Without this arm a stopped step falls
        // through to the result-derived/`pending` default and the graph silently reports a stopped
        // node as pending-or-failed.
        Some("stopped") => Some(WorkflowNodeStatus::Stopped),
        Some("detached") => Some(WorkflowNodeStatus::Detached),
        Some("pending") => Some(WorkflowNodeStatus::Pending),
        _ => None,
    }
}

/// Derive a status from a per-step result (pi `resultStatus`, `workflow-graph.ts:36-41`).
fn workflow_result_status(result: Option<&WorkflowResultSummary>) -> Option<WorkflowNodeStatus> {
    let result = result?;
    if result.detached {
        return Some(WorkflowNodeStatus::Detached);
    }
    if result.interrupted {
        return Some(WorkflowNodeStatus::Paused);
    }
    Some(if result.exit_code == Some(0) {
        WorkflowNodeStatus::Completed
    } else {
        WorkflowNodeStatus::Failed
    })
}

/// Resolve a node's status (pi `nodeStatus`, `workflow-graph.ts:43-47`): step-status override,
/// then result-derived, then running-if-current, else pending.
fn workflow_node_status(input: &WorkflowGraphBuildInput, flat_index: usize) -> WorkflowNodeStatus {
    normalize_workflow_status(
        input
            .step_statuses
            .get(flat_index)
            .and_then(|s| s.status.as_deref()),
    )
    .or_else(|| workflow_result_status(input.results.get(flat_index)))
    .unwrap_or(if input.current_flat_index == Some(flat_index) {
        WorkflowNodeStatus::Running
    } else {
        WorkflowNodeStatus::Pending
    })
}

/// Push a node id under its phase, creating the phase group in first-seen order (pi `pushPhase`,
/// `workflow-graph.ts:49-57`).
fn push_workflow_phase(phases: &mut Vec<WorkflowPhase>, phase: Option<&str>, node_id: &str) {
    let Some(phase) = phase else { return };
    if let Some(group) = phases.iter_mut().find(|candidate| candidate.title == phase) {
        group.node_ids.push(node_id.to_string());
    } else {
        phases.push(WorkflowPhase {
            title: phase.to_string(),
            node_ids: vec![node_id.to_string()],
        });
    }
}

/// Summarize a parallel group's child statuses with pi's explicit precedence (pi
/// `summarizeParallelStatuses`, `workflow-graph.ts:65-74`): running > stopped > failed > paused >
/// detached > all-completed > any-completed(=running) > pending.
///
/// G77 — the `stopped` slot sits immediately after `running`, matching the detached runner's own
/// child roll-up (`subagent-runner.ts:2181-2186`: `every completed` → `some running` → `some
/// stopped` → `some rejected` → `some failed` → `some paused`), which is the roll-up this fused
/// builder serves for a background run. `workflow-graph.ts`'s standalone version predates the
/// state and has no such branch; adding it here cannot change any other caller's result, because
/// [`WorkflowNodeStatus::Stopped`] is only ever produced by a stopped step.
fn summarize_parallel_statuses(statuses: &[WorkflowNodeStatus]) -> WorkflowNodeStatus {
    if statuses.contains(&WorkflowNodeStatus::Running) {
        return WorkflowNodeStatus::Running;
    }
    if statuses.contains(&WorkflowNodeStatus::Stopped) {
        return WorkflowNodeStatus::Stopped;
    }
    if statuses.contains(&WorkflowNodeStatus::Failed) {
        return WorkflowNodeStatus::Failed;
    }
    if statuses.contains(&WorkflowNodeStatus::Paused) {
        return WorkflowNodeStatus::Paused;
    }
    if statuses.contains(&WorkflowNodeStatus::Detached) {
        return WorkflowNodeStatus::Detached;
    }
    if !statuses.is_empty() && statuses.iter().all(|s| *s == WorkflowNodeStatus::Completed) {
        return WorkflowNodeStatus::Completed;
    }
    if statuses.contains(&WorkflowNodeStatus::Completed) {
        return WorkflowNodeStatus::Running;
    }
    WorkflowNodeStatus::Pending
}

/// pi `seqLabel` (`workflow-graph.ts:59-61`): the step's trimmed label, else its agent, else
/// `Step <n>`.
fn seq_label(step: &WorkflowTaskSpec, step_index: usize) -> String {
    let trimmed = step
        .label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    trimmed
        .map(str::to_string)
        .or_else(|| step.agent.clone().filter(|a| !a.is_empty()))
        .unwrap_or_else(|| format!("Step {}", step_index + 1))
}

/// Sanitize a dynamic item key into a node-id-safe token (pi's
/// `task.itemKey.replace(/[^a-zA-Z0-9_-]/g, "-")`, `workflow-graph.ts:157`).
fn sanitize_item_key(item_key: &str) -> String {
    item_key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Build a [`WorkflowGraphSnapshot`] from declared steps + per-step results/statuses — the Rust port
/// of pi's `buildWorkflowGraphSnapshot` (`workflow-graph.ts:73-206`), reproducing its exact node
/// ids, phase grouping, group-status precedence, dynamic metadata, and `currentNodeId` selection.
#[must_use]
pub fn build_workflow_graph_snapshot(input: &WorkflowGraphBuildInput) -> WorkflowGraphSnapshot {
    let mut nodes: Vec<WorkflowGraphNode> = Vec::new();
    let mut phases: Vec<WorkflowPhase> = Vec::new();
    let mut flat_index = 0usize;
    let mut current_node_id: Option<String> = None;

    for (step_index, step) in input.steps.iter().enumerate() {
        match step {
            WorkflowInputStep::Parallel(tasks) => {
                let group_id = format!("step-{step_index}");
                let mut children: Vec<WorkflowGraphNode> = Vec::new();
                let mut child_statuses: Vec<WorkflowNodeStatus> = Vec::new();
                for (task_index, task) in tasks.iter().enumerate() {
                    let status = workflow_node_status(input, flat_index);
                    child_statuses.push(status);
                    let child_id = format!("step-{step_index}-agent-{task_index}");
                    let label = task
                        .label
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .or_else(|| task.agent.clone().filter(|a| !a.is_empty()))
                        .unwrap_or_else(|| format!("Agent {}", task_index + 1));
                    let child = WorkflowGraphNode {
                        id: child_id.clone(),
                        kind: WorkflowNodeKind::Agent,
                        agent: task.agent.clone(),
                        phase: task.phase.clone(),
                        label,
                        status,
                        flat_index: Some(flat_index),
                        step_index: Some(step_index),
                        item_key: None,
                        output_name: task.output_name.clone(),
                        structured: Some(task.structured),
                        acceptance_status: input
                            .results
                            .get(flat_index)
                            .and_then(|r| r.acceptance_status.clone()),
                        error: input
                            .step_statuses
                            .get(flat_index)
                            .and_then(|s| s.error.clone())
                            .or_else(|| {
                                input.results.get(flat_index).and_then(|r| r.error.clone())
                            }),
                        dynamic: None,
                        children: None,
                    };
                    push_workflow_phase(&mut phases, task.phase.as_deref(), &child_id);
                    if status == WorkflowNodeStatus::Running
                        || input.current_flat_index == Some(flat_index)
                    {
                        current_node_id = Some(child_id.clone());
                    }
                    children.push(child);
                    flat_index += 1;
                }
                let group_status = summarize_parallel_statuses(&child_statuses);
                if input.current_step_index == Some(step_index) && current_node_id.is_none() {
                    current_node_id = Some(group_id.clone());
                }
                nodes.push(WorkflowGraphNode {
                    id: group_id,
                    kind: WorkflowNodeKind::ParallelGroup,
                    agent: None,
                    phase: None,
                    label: if tasks.len() == 1 {
                        "Parallel task".to_string()
                    } else {
                        format!("Parallel group ({})", tasks.len())
                    },
                    status: group_status,
                    flat_index: None,
                    step_index: Some(step_index),
                    item_key: None,
                    output_name: None,
                    structured: None,
                    acceptance_status: None,
                    error: None,
                    dynamic: None,
                    children: Some(children),
                });
                continue;
            }
            WorkflowInputStep::Dynamic(dynamic) => {
                let group_id = format!("step-{step_index}");
                let materialized = input.dynamic_children.get(&step_index);
                let group_override = input.dynamic_group_statuses.get(&step_index);
                let mut children: Vec<WorkflowGraphNode> = Vec::new();
                let mut child_statuses: Vec<WorkflowNodeStatus> = Vec::new();
                if let Some(materialized) = materialized {
                    for task in materialized {
                        let status = workflow_node_status(input, task.flat_index);
                        child_statuses.push(status);
                        let child_id = format!(
                            "step-{step_index}-item-{}",
                            sanitize_item_key(&task.item_key)
                        );
                        let phase = dynamic
                            .template_phase
                            .clone()
                            .or_else(|| dynamic.step_phase.clone());
                        let label = task
                            .label
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                            .or_else(|| {
                                dynamic
                                    .template_label
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|s| !s.is_empty())
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| format!("{} {}", task.agent, task.item_key));
                        let child = WorkflowGraphNode {
                            id: child_id.clone(),
                            kind: WorkflowNodeKind::Agent,
                            agent: Some(task.agent.clone()),
                            phase: phase.clone(),
                            label,
                            status,
                            flat_index: Some(task.flat_index),
                            step_index: Some(step_index),
                            item_key: Some(task.item_key.clone()),
                            output_name: task.output_name.clone(),
                            structured: Some(task.structured),
                            acceptance_status: input
                                .results
                                .get(task.flat_index)
                                .and_then(|r| r.acceptance_status.clone()),
                            error: input
                                .step_statuses
                                .get(task.flat_index)
                                .and_then(|s| s.error.clone())
                                .or_else(|| {
                                    input
                                        .results
                                        .get(task.flat_index)
                                        .and_then(|r| r.error.clone())
                                })
                                .or_else(|| task.error.clone()),
                            dynamic: None,
                            children: None,
                        };
                        push_workflow_phase(&mut phases, phase.as_deref(), &child_id);
                        if status == WorkflowNodeStatus::Running
                            || input.current_flat_index == Some(task.flat_index)
                        {
                            current_node_id = Some(child_id.clone());
                        }
                        children.push(child);
                    }
                }
                let group_status = group_override.map(|o| o.status).unwrap_or_else(|| {
                    if children.is_empty() {
                        if input.current_step_index == Some(step_index) {
                            WorkflowNodeStatus::Running
                        } else {
                            WorkflowNodeStatus::Pending
                        }
                    } else {
                        summarize_parallel_statuses(&child_statuses)
                    }
                });
                if input.current_step_index == Some(step_index) && current_node_id.is_none() {
                    current_node_id = Some(group_id.clone());
                }
                let label = dynamic
                    .step_label
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .or_else(|| {
                        dynamic
                            .template_label
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| format!("Dynamic fanout ({})", dynamic.collect_as));
                nodes.push(WorkflowGraphNode {
                    id: group_id,
                    kind: WorkflowNodeKind::DynamicParallelGroup,
                    agent: None,
                    phase: None,
                    label,
                    status: group_status,
                    flat_index: None,
                    step_index: Some(step_index),
                    item_key: None,
                    output_name: Some(dynamic.collect_as.clone()),
                    structured: Some(dynamic.collect_structured),
                    acceptance_status: group_override.and_then(|o| o.acceptance_status.clone()),
                    error: group_override.and_then(|o| o.error.clone()),
                    dynamic: Some(WorkflowDynamicMeta {
                        source_output: dynamic.expand_from_output.clone(),
                        source_path: dynamic.expand_from_path.clone(),
                        item_name: dynamic
                            .item_name
                            .clone()
                            .unwrap_or_else(|| "item".to_string()),
                        max_items: dynamic.max_items,
                        collect_as: Some(dynamic.collect_as.clone()),
                    }),
                    children: Some(children),
                });
                if let Some(materialized) = materialized
                    && let Some(max) = materialized.iter().map(|c| c.flat_index + 1).max()
                {
                    flat_index = flat_index.max(max);
                }
                continue;
            }
            WorkflowInputStep::Sequential(seq) => {
                let status = workflow_node_status(input, flat_index);
                let id = format!("step-{step_index}");
                nodes.push(WorkflowGraphNode {
                    id: id.clone(),
                    kind: WorkflowNodeKind::Step,
                    agent: seq.agent.clone(),
                    phase: seq.phase.clone(),
                    label: seq_label(seq, step_index),
                    status,
                    flat_index: Some(flat_index),
                    step_index: Some(step_index),
                    item_key: None,
                    output_name: seq.output_name.clone(),
                    structured: Some(seq.structured),
                    acceptance_status: input
                        .results
                        .get(flat_index)
                        .and_then(|r| r.acceptance_status.clone()),
                    error: input
                        .step_statuses
                        .get(flat_index)
                        .and_then(|s| s.error.clone())
                        .or_else(|| input.results.get(flat_index).and_then(|r| r.error.clone())),
                    dynamic: None,
                    children: None,
                });
                push_workflow_phase(&mut phases, seq.phase.as_deref(), &id);
                if status == WorkflowNodeStatus::Running
                    || input.current_flat_index == Some(flat_index)
                    || input.current_step_index == Some(step_index)
                {
                    current_node_id = Some(id);
                }
                flat_index += 1;
            }
        }
    }

    WorkflowGraphSnapshot {
        run_id: input.run_id.clone(),
        mode: input.mode.unwrap_or(WorkflowRunMode::Chain),
        phases,
        nodes,
        current_node_id,
    }
}

/// Build a [`WorkflowGraphBuildInput`] from a background run's already-flattened [`crate::spawn::chain_graph::RunnerStep`] list
/// plus its live [`RunStatus`], so the detached runner (`background/runner_main.rs`) can embed a
/// live workflow-graph snapshot in `status.json`. Each [`crate::spawn::chain_graph::RunnerStep`] projects to a
/// [`WorkflowInputStep`]; per-step status/errors come straight off `status.steps`
/// ([`StepState`] normalized to the graph's own vocabulary). Note the cyrup [`crate::spawn::chain_graph::SingleStepSpec`]
/// carries no `phase`/`label`, so those degrade to `None`/agent-name here — the richer, phase/label-
/// bearing [`build_workflow_graph_snapshot`] path is exercised directly by the chain-plan layer that
/// does have that metadata.
#[must_use]
pub fn workflow_graph_from_run(
    steps: &[crate::spawn::chain_graph::RunnerStep],
    status: &RunStatus,
) -> WorkflowGraphSnapshot {
    use crate::spawn::chain_graph::RunnerStep;
    let mut input_steps: Vec<WorkflowInputStep> = Vec::new();
    let mut results: Vec<WorkflowResultSummary> = Vec::new();
    let mut step_statuses: Vec<WorkflowStepStatusInput> = Vec::new();

    for (index, step) in steps.iter().enumerate() {
        // SUBA-093 — `status.steps` is FLAT (one entry per child), so a top-level step's own entry
        // is at its flat base, not at its position in `steps`. pi keys its graph nodes on the same
        // value (`node.flatIndex` → `statusPayload.steps[node.flatIndex]`,
        // `subagent-runner.ts:2847-2854` @v0.64.0). A group node therefore still reports its FIRST
        // member's state, which is the same single word the collapsed group entry used to carry.
        let step_status = status
            .steps
            .get(crate::background::flat_index::flat_base(steps, index));
        let status_str = step_status.map(|s| match s.status {
            StepState::Pending => "pending",
            StepState::Running => "running",
            StepState::Paused => "paused",
            StepState::Complete => "complete",
            StepState::Failed => "failed",
            StepState::Stopped => "stopped",
        });
        step_statuses.push(WorkflowStepStatusInput {
            status: status_str.map(str::to_string),
            error: step_status.and_then(|s| s.error.clone()),
        });
        results.push(WorkflowResultSummary::default());
        match step {
            RunnerStep::SingleStep(spec) => {
                input_steps.push(WorkflowInputStep::Sequential(WorkflowTaskSpec {
                    agent: Some(spec.agent.clone()),
                    phase: None,
                    label: None,
                    output_name: spec.output.clone(),
                    structured: spec.structured_output_schema.is_some(),
                }));
            }
            RunnerStep::ImportAsyncRoot(spec) => {
                input_steps.push(WorkflowInputStep::Sequential(WorkflowTaskSpec {
                    agent: Some(spec.agent.clone()),
                    phase: None,
                    label: None,
                    output_name: spec.output.clone(),
                    structured: false,
                }));
            }
            RunnerStep::ParallelGroup(group) => {
                let tasks = group
                    .steps
                    .iter()
                    .map(|s| WorkflowTaskSpec {
                        agent: Some(s.agent.clone()),
                        phase: None,
                        label: None,
                        output_name: s.output.clone(),
                        structured: s.structured_output_schema.is_some(),
                    })
                    .collect();
                input_steps.push(WorkflowInputStep::Parallel(tasks));
            }
            RunnerStep::DynamicGroup(dynamic) => {
                input_steps.push(WorkflowInputStep::Dynamic(WorkflowDynamicStepSpec {
                    expand_from_output: dynamic.expand.clone(),
                    expand_from_path: String::new(),
                    item_name: None,
                    max_items: None,
                    step_phase: None,
                    step_label: None,
                    template_phase: None,
                    template_label: None,
                    collect_as: dynamic.collect.clone(),
                    collect_structured: dynamic.template.structured_output_schema.is_some(),
                }));
            }
        }
    }

    let mode = match status.mode {
        RunMode::Single => WorkflowRunMode::Single,
        RunMode::Parallel => WorkflowRunMode::Parallel,
        RunMode::Chain => WorkflowRunMode::Chain,
        RunMode::Workflow => WorkflowRunMode::Workflow,
    };

    build_workflow_graph_snapshot(&WorkflowGraphBuildInput {
        run_id: status.run_id.as_str().to_string(),
        mode: Some(mode),
        steps: input_steps,
        results,
        current_flat_index: status.current_step,
        current_step_index: status.current_step,
        step_statuses,
        dynamic_children: std::collections::BTreeMap::new(),
        dynamic_group_statuses: std::collections::BTreeMap::new(),
    })
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
    fn workflow_graph_projects_and_rolls_up_stopped() {
        assert_eq!(
            normalize_workflow_status(Some("stopped")),
            Some(WorkflowNodeStatus::Stopped)
        );
        assert_eq!(
            summarize_parallel_statuses(&[WorkflowNodeStatus::Failed, WorkflowNodeStatus::Stopped]),
            WorkflowNodeStatus::Stopped,
            "stopped outranks failed in the runner's own child roll-up"
        );
        assert_eq!(
            summarize_parallel_statuses(&[
                WorkflowNodeStatus::Running,
                WorkflowNodeStatus::Stopped
            ]),
            WorkflowNodeStatus::Running,
            "…but running still outranks stopped"
        );
        // Unchanged without a stopped child.
        assert_eq!(
            summarize_parallel_statuses(&[WorkflowNodeStatus::Failed, WorkflowNodeStatus::Paused]),
            WorkflowNodeStatus::Failed
        );
    }
}
