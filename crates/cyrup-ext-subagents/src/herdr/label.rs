//! The metadata text — and **the privacy rule**, which is absolute.
//!
//! > Explicit launch/workflow label only; raw prompts never enter pane metadata.
//!
//! — pi's own words, on the field that carries it: `tmp/pi-subagents` @ `v0.68.0`
//! `src/integrations/herdr-status.ts:32`.
//!
//! herdr's sidebar is a **shared surface**. A pane's metadata is visible to whoever is looking at
//! that terminal, it survives into rollups, and — because [`super::AGENT`] reports under a source
//! herdr never expires (`tmp/herdr/src/terminal/state.rs:18-25`) — it can outlive the process that
//! wrote it. What a user typed into cyrup is not something cyrup may publish there, however
//! convenient a label it would make.
//!
//! # What may be sent, exhaustively
//!
//! - **A workflow node's `label`** — [`crate::background::WorkflowGraphNode::label`], generated
//!   from the *workflow definition*, never from the user's task text. [`workflow_task_label`]
//!   selects one.
//!
//!   What that label can actually be TODAY is narrower than the type allows, and the narrow
//!   version is the one to reason about: `status.telemetry.workflow_graph` has exactly one
//!   production populator — `background/runner_main/status.rs:44-46` calling
//!   [`crate::background::workflow_graph_from_run`] — and that builder sets `label: None` and
//!   `phase: None` on every `WorkflowTaskSpec` (`background/workflow_graph.rs:752,:763,:775`),
//!   `step_label: None`/`template_label: None` on every dynamic step (`:789,:791`) and an empty
//!   `dynamic_children` (`:814`), because `SingleStepSpec` carries no phase or label at all. So
//!   every published node label degrades to **the agent name**, or to `Step N`/`Agent N` when
//!   there is not even one of those. Phase titles and authored step labels are a capability of
//!   the richer `build_workflow_graph_snapshot` builder, which today has no production caller —
//!   wiring it is what would make them reachable.
//! - **An explicit launch label**, when a launch site chose to name itself.
//! - **Agent names** and **counts** — how many subagents, which agents, how many project panes.
//!
//! # What may never be sent
//!
//! - [`crate::background::RunHistoryEntry::task`] (`background/run_history.rs:24`), the
//!   200-char capture of the user's task text, and every other prompt field.
//! - `AsyncRunView::description` — pi's `AsyncJobState.description`, a free-text run description
//!   whose provenance is the launch call. [`RunLabel::from_async_run`] does not read it, and the
//!   test at the bottom of this file fails if it ever starts to.
//! - Any child transcript, tool output, or rendered control-notice body
//!   (`extension/executor/notices.rs`'s `format_control_notice_message` interpolates the child's
//!   own text). The *reason token* may go on [`super::state::StateReport::message`]; the rendered
//!   body may not go anywhere.
//!
//! # Two independent bounds, both applied here
//!
//! herdr caps every presentation field and token value at 80 characters and strips control
//! characters before storage (`socket-api.mdx:792`). pi caps client-side at
//! [`MAX_TASK_LABEL_CHARS`] and [`MAX_TITLE_TASK_CHARS`] (`herdr-status.ts:11-12`). This port
//! applies **pi's** caps, deliberately: doing it client-side means the cyrup side of the contract
//! is testable without a herdr running, which in this container is the only way it is testable at
//! all. Every string also goes through [`crate::workflows::sanitize_display_text`], the in-tree
//! port of pi's `sanitizeDisplayText` — so an escape sequence in a workflow label cannot repaint
//! someone else's terminal.
//!
//! # The other half of the seam
//!
//! Everything here is destined for `pane.report_metadata`
//! ([`cyrup_herdr::schema::panes::PaneReportMetadataParams`]) and **nothing here may travel on
//! `pane.report_agent`**. herdr is explicit that `state` "affects waits, notifications, and
//! rollups" and that display-only values go through metadata (`socket-api.mdx:717-718`). See
//! [`super::state`] for the other side.

use crate::background::{WorkflowGraphNode, WorkflowGraphSnapshot, WorkflowNodeStatus};
use crate::tui::fleet_state::AsyncRunView;
use crate::workflows::{preview_display_text, sanitize_display_text};

/// pi `MAX_TASK_LABEL_CHARS` (`herdr-status.ts:11`) — the summary's task clause, in UTF-16 units.
pub const MAX_TASK_LABEL_CHARS: usize = 80;

/// pi `MAX_TITLE_TASK_CHARS` (`herdr-status.ts:12`) — the tab-title suffix, which shares a
/// terminal's title bar and gets a tighter budget.
pub const MAX_TITLE_TASK_CHARS: usize = 42;

/// pi `MAX_WORKFLOW_LABEL_NODES` (`herdr-status.ts:13`) — the walk visits at most this many nodes.
pub const MAX_WORKFLOW_LABEL_NODES: usize = 128;

/// pi `MAX_WORKFLOW_LABEL_DEPTH` (`herdr-status.ts:14`) — the walk descends at most this deep.
///
/// Both bounds exist because a workflow graph is user-authored and a dynamic fan-out can nest: the
/// label is a decoration, so it may not become an unbounded traversal on the hot path.
pub const MAX_WORKFLOW_LABEL_DEPTH: usize = 8;

/// The maximum agent names named in the summary before it elides — pi `agents.slice(0, 3)`
/// (`herdr-status.ts:160`).
const MAX_NAMED_AGENTS: usize = 3;

/// pi `boundedTaskLabel` (`herdr-status.ts:72-78`): sanitize, trim, drop if empty, then cut to
/// `max_chars` with an ellipsis.
///
/// **This is the only door.** Every string that reaches pane metadata goes through it, so the
/// bound and the control-character strip cannot be forgotten at one call site.
#[must_use]
pub fn bounded_task_label(value: &str, max_chars: usize) -> Option<String> {
    let normalized = sanitize_display_text(value);
    let normalized = normalized.trim();
    if normalized.is_empty() {
        return None;
    }
    Some(preview_display_text(normalized, max_chars))
}

/// pi `workflowTaskLabel` (`herdr-status.ts:79-98`): an explicit launch label if there is one,
/// otherwise the label of the workflow node that is currently the point of the run.
///
/// The node is chosen in upstream's order — the graph's own `current_node_id`, else the first
/// `Running` node, else the first `Pending` one — over a bounded pre-order walk
/// ([`MAX_WORKFLOW_LABEL_DEPTH`], [`MAX_WORKFLOW_LABEL_NODES`]).
///
/// `launch_label` is the *explicit* label a launch site chose for itself. It is not, and must
/// never be, the run's task text: this function bounds and sanitizes whatever it is handed, but
/// no bound can turn a prompt into a non-prompt. See the [module docs](self).
#[must_use]
pub fn workflow_task_label(
    launch_label: Option<&str>,
    graph: Option<&WorkflowGraphSnapshot>,
) -> Option<String> {
    if let Some(explicit) = launch_label.and_then(|v| bounded_task_label(v, MAX_TASK_LABEL_CHARS)) {
        return Some(explicit);
    }
    let graph = graph?;
    let mut nodes: Vec<&WorkflowGraphNode> = Vec::new();
    collect_nodes(&graph.nodes, 0, &mut nodes);

    let current = graph
        .current_node_id
        .as_deref()
        .and_then(|id| nodes.iter().find(|node| node.id == id).copied());
    let active = current
        .or_else(|| {
            nodes
                .iter()
                .find(|node| node.status == WorkflowNodeStatus::Running)
                .copied()
        })
        .or_else(|| {
            nodes
                .iter()
                .find(|node| node.status == WorkflowNodeStatus::Pending)
                .copied()
        })?;
    bounded_task_label(&active.label, MAX_TASK_LABEL_CHARS)
}

/// pi's `collect` closure (`herdr-status.ts:84-94`): pre-order, depth- and count-bounded. Both
/// guards are checked before *and* inside the loop, exactly as upstream, so a wide node cannot
/// overshoot the node budget.
fn collect_nodes<'a>(
    values: &'a [WorkflowGraphNode],
    depth: usize,
    out: &mut Vec<&'a WorkflowGraphNode>,
) {
    if depth > MAX_WORKFLOW_LABEL_DEPTH || out.len() >= MAX_WORKFLOW_LABEL_NODES {
        return;
    }
    for value in values {
        if out.len() >= MAX_WORKFLOW_LABEL_NODES {
            return;
        }
        out.push(value);
        if let Some(children) = &value.children {
            collect_nodes(children, depth + 1, out);
        }
    }
}

/// One active run, projected down to exactly the fields that may be published — pi
/// `HerdrStatusRun` (`herdr-status.ts:28-36`) minus its attention fields, which belong to
/// [`super::state`] rather than to the label.
///
/// The `task_label` is private and there is no setter: the only ways to obtain one are
/// [`Self::new`] and [`Self::from_async_run`], both of which route through
/// [`workflow_task_label`]. That is this module's structural half of the privacy rule — pi
/// achieves the same by having `replaceRuns` strip the caller's `taskLabel` and re-derive it
/// (`herdr-status.ts:304-306`), which is a discipline rather than a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunLabel {
    /// The agents this run invokes, in step order, for the summary's `(a, b, c)` clause.
    agents: Vec<String>,
    /// The bounded, sanitized workflow/launch label, when the run has one.
    task_label: Option<String>,
}

impl RunLabel {
    /// Project a run from its agent list and its workflow graph, applying the privacy rule.
    #[must_use]
    pub fn new(
        agents: Vec<String>,
        launch_label: Option<&str>,
        graph: Option<&WorkflowGraphSnapshot>,
    ) -> Self {
        Self {
            agents,
            task_label: workflow_task_label(launch_label, graph),
        }
    }

    /// Project one row of the fleet's own active-run view.
    ///
    /// Reads exactly three things: the run's step agents, and — through
    /// [`workflow_task_label`] — the run's workflow graph. It does **not** read
    /// `AsyncRunView::description`, and no launch label is passed, because nothing on this
    /// projection is known to be one. The privacy test at the bottom of this file drives this
    /// constructor with a prompt sentinel in every free-text field and asserts none of them
    /// reaches the wire text.
    #[must_use]
    pub fn from_async_run(run: &AsyncRunView) -> Self {
        Self::new(
            run.status
                .steps
                .iter()
                .map(|step| step.agent.clone())
                .collect(),
            None,
            run.status.telemetry.workflow_graph.as_ref(),
        )
    }

    /// The run's bounded label, if it has one.
    #[must_use]
    pub fn task_label(&self) -> Option<&str> {
        self.task_label.as_deref()
    }

    /// The agents this run invokes.
    #[must_use]
    pub fn agents(&self) -> &[String] {
        &self.agents
    }

    /// pi's per-run subagent count (`herdr-status.ts:154`): `max(1, agents.length)` — a run with
    /// no named agent is still one subagent.
    fn subagent_count(&self) -> usize {
        self.agents.len().max(1)
    }
}

/// The two display-only strings the bridge publishes through `pane.report_metadata`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetadataText {
    /// The `summary` token and the `idle`/`working`/`done` state labels — pi's `label(true)`
    /// (`herdr-status.ts:156-167`), which all three keys share (`:224-226`).
    pub summary: String,
    /// The `title-suffix` token — pi's `titleSuffix()` (`herdr-status.ts:169-176`).
    pub title_suffix: Option<String>,
}

/// Render the pane's metadata text, or `None` when nothing is running.
///
/// `None` is pi's clear path (`herdr-status.ts:203-215`): with no active runs the bridge does not
/// publish an empty label, it clears the keys it owns. The caller turns that into
/// `clear_state_labels` plus a `tokens` patch of JSON nulls.
///
/// `project_panes` is the count of project panes this cyrup session has opened. It is `0` until
/// the project-pane manager exists, and at `0` the clause renders nothing — pi does exactly the
/// same when its callback is absent (`herdr-status.ts:162-163`, `?? 0`). No stub, no coupling.
///
/// `attention` is how many runs are asking for the human; any non-zero count adds the warning
/// mark. The *reason* is not rendered here — it travels on
/// [`super::state::StateReport::message`], because it is semantic and this is presentation.
#[must_use]
pub fn metadata_text(
    runs: &[RunLabel],
    project_panes: usize,
    attention: usize,
) -> Option<MetadataText> {
    if runs.is_empty() {
        return None;
    }
    let summary = summary_text(runs, project_panes, attention);
    Some(MetadataText {
        title_suffix: title_suffix(runs, attention),
        summary,
    })
}

/// pi `label(includeAttention = true)` (`herdr-status.ts:156-167`).
fn summary_text(runs: &[RunLabel], project_panes: usize, attention: usize) -> String {
    let agents = active_agent_names(runs);
    let count: usize = runs.iter().map(RunLabel::subagent_count).sum();
    let who = if agents.is_empty() {
        String::new()
    } else {
        let shown = agents
            .iter()
            .take(MAX_NAMED_AGENTS)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let more = if agents.len() > MAX_NAMED_AGENTS {
            ", …"
        } else {
            ""
        };
        format!(" ({shown}{more})")
    };
    let panes = if project_panes > 0 {
        let plural = if project_panes == 1 { "" } else { "s" };
        format!(" · {project_panes} pane{plural}")
    } else {
        String::new()
    };
    let task = active_task_label(runs).map_or_else(String::new, |task| format!(" · {task}"));
    let warn = if attention > 0 { " ⚠" } else { "" };
    let plural = if count == 1 { "" } else { "s" };
    format!("⏳ {count} subagent{plural}{who}{panes}{task}{warn}")
}

/// pi `titleSuffix()` (`herdr-status.ts:169-176`): the task if there is one, else the single
/// agent's name when there is exactly one of each, else the count.
fn title_suffix(runs: &[RunLabel], attention: usize) -> Option<String> {
    if runs.is_empty() {
        return None;
    }
    let agents = active_agent_names(runs);
    let count: usize = runs.iter().map(RunLabel::subagent_count).sum();
    let task = active_task_label(runs).and_then(|t| bounded_task_label(t, MAX_TITLE_TASK_CHARS));
    let target = match task {
        Some(task) => task,
        None => match agents.as_slice() {
            [_only] if count == 1 => count.to_string(),
            _ => count.to_string(),
        },
    };
    let warn = if attention > 0 { "⚠" } else { "" };
    Some(format!("⏳{target}{warn}"))
}

/// pi `activeAgentNames()` (`herdr-status.ts:152`): every run's agents, de-duplicated, in first-
/// seen order.
fn active_agent_names(runs: &[RunLabel]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for run in runs {
        for agent in &run.agents {
            if !names.iter().any(|seen| seen == agent) {
                names.push(agent.clone());
            }
        }
    }
    names
}

/// pi `activeTaskLabel()` (`herdr-status.ts:155`): the most recently added run that has a label
/// (upstream reverses the map's insertion order and takes the first hit).
fn active_task_label(runs: &[RunLabel]) -> Option<&str> {
    runs.iter().rev().find_map(RunLabel::task_label)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use crate::background::{
        RunId, RunMode, RunState, RunStatus, StepState, StepStatus, WorkflowRunMode,
    };

    fn node(id: &str, label: &str, status: WorkflowNodeStatus) -> WorkflowGraphNode {
        WorkflowGraphNode {
            id: id.to_string(),
            kind: crate::background::WorkflowNodeKind::Step,
            agent: None,
            phase: None,
            label: label.to_string(),
            status,
            flat_index: None,
            step_index: None,
            item_key: None,
            output_name: None,
            structured: None,
            acceptance_status: None,
            error: None,
            dynamic: None,
            children: None,
        }
    }

    fn graph(nodes: Vec<WorkflowGraphNode>, current: Option<&str>) -> WorkflowGraphSnapshot {
        WorkflowGraphSnapshot {
            run_id: "r1".to_string(),
            mode: WorkflowRunMode::Chain,
            phases: Vec::new(),
            nodes,
            current_node_id: current.map(str::to_string),
        }
    }

    fn step(agent: &str) -> StepStatus {
        let mut step = StepStatus::pending(agent);
        step.status = StepState::Running;
        step
    }

    fn running_run(steps: Vec<StepStatus>) -> RunStatus {
        let mut status =
            RunStatus::queued(RunId::from_token("r1".to_string()), RunMode::Chain, None);
        status.steps = steps;
        status.state = RunState::Running;
        status
    }

    // ---------------------------------------------------------------------------------------
    // THE PRIVACY RULE
    // ---------------------------------------------------------------------------------------

    /// **The load-bearing test of this module.** A run whose every free-text field carries the
    /// user's prompt, and whose workflow graph carries a safe node label: the prompt must not
    /// appear in any byte of the published text, and the node label must.
    ///
    /// Gutted by any of: reading `AsyncRunView::description` in [`RunLabel::from_async_run`];
    /// passing the run's task text as `launch_label`; falling back to the run description when
    /// the workflow graph yields nothing. Each makes the sentinel appear and this assertion fail.
    ///
    /// **Scope.** The node label here (`"review the diff"`) is an arbitrary string, which is what
    /// this layer's input type allows and what [`workflow_task_label`]'s selection has to handle.
    /// It is NOT what production emits: the only production graph builder sets `label: None` on
    /// every spec, so a real published label is the agent name or `Step N` — see this module's
    /// doc. The privacy rule over the shape production really produces is asserted on the
    /// production producer, in `super::runtime`'s `raw_prompts_never_reach_the_published_label`,
    /// whose fixture builds its graph with `workflow_graph_from_run`.
    #[test]
    fn raw_prompts_never_enter_pane_metadata() {
        const SENTINEL: &str = "REFACTOR-THE-BILLING-SECRETS-PROMPT";

        let mut status = running_run(vec![step("reviewer")]);
        status.error = Some(SENTINEL.to_string());
        status.telemetry.workflow_graph = Some(graph(
            vec![node(
                "step-1",
                "review the diff",
                WorkflowNodeStatus::Running,
            )],
            None,
        ));
        let view = AsyncRunView {
            paths: crate::background::RunPaths::for_run(
                std::path::Path::new("/tmp/async"),
                std::path::Path::new("/tmp/results"),
                &RunId::from_token("r1".to_string()),
            ),
            status,
            session_id: None,
            // pi's `AsyncJobState.description` — free text from the launch call, and exactly the
            // field a careless label would reach for.
            description: Some(SENTINEL.to_string()),
            context: None,
            nested_children: Vec::new(),
        };

        let label = RunLabel::from_async_run(&view);
        let text = metadata_text(&[label], 0, 0).expect("one active run");
        let published = format!(
            "{}\n{}",
            text.summary,
            text.title_suffix.unwrap_or_default()
        );

        assert!(
            !published.contains(SENTINEL),
            "the prompt reached pane metadata: {published}"
        );
        assert!(
            published.contains("review the diff"),
            "the workflow node label is what may be published: {published}"
        );
    }

    /// The bound and the control-character strip are on the one door every label goes through, so
    /// an escape sequence in a user-authored workflow label cannot repaint someone's terminal.
    #[test]
    fn every_label_is_sanitized_and_bounded() {
        let sneaky = "\u{1b}[2Jwiped\u{1b}]0;retitled\u{07}";
        assert_eq!(bounded_task_label(sneaky, 80).as_deref(), Some("wiped"));
        assert_eq!(bounded_task_label("   ", 80), None);
        assert_eq!(bounded_task_label("", 80), None);

        let long = "x".repeat(200);
        let bounded = bounded_task_label(&long, MAX_TASK_LABEL_CHARS).expect("non-empty");
        assert_eq!(bounded.chars().count(), MAX_TASK_LABEL_CHARS);
        assert!(bounded.ends_with("..."));

        let title = bounded_task_label(&long, MAX_TITLE_TASK_CHARS).expect("non-empty");
        assert_eq!(title.chars().count(), MAX_TITLE_TASK_CHARS);
    }

    /// The caps reach the rendered text, not only the helper: a 200-character workflow label is
    /// cut to 80 in the summary and to 42 in the title suffix.
    #[test]
    fn the_caps_reach_the_published_text() {
        let long = "w".repeat(200);
        let run = RunLabel::new(
            vec!["a".to_string()],
            None,
            Some(&graph(
                vec![node("step-1", &long, WorkflowNodeStatus::Running)],
                None,
            )),
        );
        let text = metadata_text(&[run], 0, 0).expect("one run");
        assert!(text.summary.contains(&format!("{}...", "w".repeat(77))));
        assert!(!text.summary.contains(&"w".repeat(81)));
        let suffix = text.title_suffix.expect("a suffix");
        assert!(suffix.contains(&format!("{}...", "w".repeat(39))));
        assert!(!suffix.contains(&"w".repeat(43)));
    }

    // ---------------------------------------------------------------------------------------
    // NODE SELECTION
    // ---------------------------------------------------------------------------------------

    /// Upstream's order: `current_node_id`, else first `Running`, else first `Pending`.
    #[test]
    fn the_active_node_is_chosen_in_upstreams_order() {
        let nodes = vec![
            node("step-1", "done step", WorkflowNodeStatus::Completed),
            node("step-2", "running step", WorkflowNodeStatus::Running),
            node("step-3", "pending step", WorkflowNodeStatus::Pending),
        ];
        assert_eq!(
            workflow_task_label(None, Some(&graph(nodes.clone(), Some("step-3")))).as_deref(),
            Some("pending step"),
            "current_node_id wins outright"
        );
        assert_eq!(
            workflow_task_label(None, Some(&graph(nodes.clone(), None))).as_deref(),
            Some("running step"),
            "then the first running node"
        );
        let no_running = vec![
            node("step-1", "done step", WorkflowNodeStatus::Completed),
            node("step-3", "pending step", WorkflowNodeStatus::Pending),
        ];
        assert_eq!(
            workflow_task_label(None, Some(&graph(no_running, None))).as_deref(),
            Some("pending step"),
            "then the first pending node"
        );
        assert_eq!(
            workflow_task_label(None, Some(&graph(vec![], None))),
            None,
            "an empty graph has no label"
        );
        assert_eq!(workflow_task_label(None, None), None);
        assert_eq!(
            workflow_task_label(Some("release prep"), Some(&graph(nodes, None))).as_deref(),
            Some("release prep"),
            "an explicit launch label outranks the graph"
        );
    }

    /// A `current_node_id` naming a node beyond the walk's budget falls through to the status
    /// scan rather than yielding nothing — upstream's `??` chain, not an early return.
    #[test]
    fn an_unreachable_current_node_falls_through() {
        let nodes = vec![node("step-1", "running step", WorkflowNodeStatus::Running)];
        assert_eq!(
            workflow_task_label(None, Some(&graph(nodes, Some("step-does-not-exist")))).as_deref(),
            Some("running step")
        );
    }

    /// The two walk bounds, proven by a graph that violates both: 12 levels deep, 200 nodes wide.
    /// The label must come from inside the budget, and the walk must terminate.
    #[test]
    fn the_walk_is_depth_and_count_bounded() {
        // Depth: a chain 12 deep whose only Running node is at level 10, past the depth budget.
        let mut deep = node("leaf", "too deep to see", WorkflowNodeStatus::Running);
        for level in (0..12).rev() {
            let mut parent = node(
                &format!("step-{level}"),
                &format!("level {level}"),
                WorkflowNodeStatus::Completed,
            );
            parent.children = Some(vec![deep]);
            deep = parent;
        }
        assert_eq!(
            workflow_task_label(None, Some(&graph(vec![deep], None))),
            None,
            "nothing inside the depth budget is running or pending"
        );

        // Count: 200 completed siblings followed by one running node the budget never reaches.
        let mut wide: Vec<WorkflowGraphNode> = (0..200)
            .map(|i| {
                node(
                    &format!("step-{i}"),
                    &format!("step {i}"),
                    WorkflowNodeStatus::Completed,
                )
            })
            .collect();
        wide.push(node("late", "past the budget", WorkflowNodeStatus::Running));
        assert_eq!(
            workflow_task_label(None, Some(&graph(wide, None))),
            None,
            "the 201st node is never visited"
        );

        // And the budgets do not cut off anything inside them.
        let mut inside = node("leaf", "just inside", WorkflowNodeStatus::Running);
        for level in (0..MAX_WORKFLOW_LABEL_DEPTH).rev() {
            let mut parent = node(
                &format!("step-{level}"),
                &format!("level {level}"),
                WorkflowNodeStatus::Completed,
            );
            parent.children = Some(vec![inside]);
            inside = parent;
        }
        assert_eq!(
            workflow_task_label(None, Some(&graph(vec![inside], None))).as_deref(),
            Some("just inside")
        );
    }

    // ---------------------------------------------------------------------------------------
    // RENDERING
    // ---------------------------------------------------------------------------------------

    /// Nothing running publishes nothing — the caller's signal to clear the keys it owns instead
    /// of leaving a stale `⏳` on the sidebar.
    #[test]
    fn an_empty_fleet_publishes_no_text() {
        assert_eq!(metadata_text(&[], 3, 1), None);
    }

    /// The summary's shape, clause by clause.
    #[test]
    fn the_summary_renders_every_clause() {
        let one = RunLabel::new(vec!["reviewer".to_string()], Some("ship the release"), None);
        let text = metadata_text(&[one], 0, 0).expect("one run");
        assert_eq!(text.summary, "⏳ 1 subagent (reviewer) · ship the release");
        assert_eq!(text.title_suffix.as_deref(), Some("⏳ship the release"));

        let many = vec![
            RunLabel::new(vec!["a".to_string(), "b".to_string()], None, None),
            RunLabel::new(
                vec!["c".to_string(), "d".to_string()],
                Some("second label"),
                None,
            ),
        ];
        let text = metadata_text(&many, 2, 1).expect("two runs");
        assert_eq!(
            text.summary,
            "⏳ 4 subagents (a, b, c, …) · 2 panes · second label ⚠"
        );
        assert_eq!(text.title_suffix.as_deref(), Some("⏳second label⚠"));
    }

    /// A run with no named agent still counts as one subagent, and one pane is singular.
    #[test]
    fn counts_and_plurals_follow_upstream() {
        let anonymous = RunLabel::new(Vec::new(), None, None);
        let text = metadata_text(&[anonymous], 1, 0).expect("one run");
        assert_eq!(text.summary, "⏳ 1 subagent · 1 pane");
        assert_eq!(
            text.title_suffix.as_deref(),
            Some("⏳1"),
            "no single named agent falls back to the count"
        );
    }

    /// The pane clause is absent at zero — the shape this ships in until the project-pane manager
    /// lands, and the shape pi itself renders when its callback is missing.
    #[test]
    fn zero_project_panes_render_nothing() {
        let run = RunLabel::new(vec!["a".to_string()], None, None);
        let text = metadata_text(&[run], 0, 0).expect("one run");
        assert!(!text.summary.contains("pane"), "{}", text.summary);
    }

    /// The task clause comes from the most recently added labelled run.
    #[test]
    fn the_newest_labelled_run_names_the_task() {
        let runs = vec![
            RunLabel::new(vec!["a".to_string()], Some("older"), None),
            RunLabel::new(vec!["b".to_string()], Some("newer"), None),
            RunLabel::new(vec!["c".to_string()], None, None),
        ];
        let text = metadata_text(&runs, 0, 0).expect("three runs");
        assert!(text.summary.ends_with("· newer"), "{}", text.summary);
    }

    /// Agent names de-duplicate across runs and keep first-seen order.
    #[test]
    fn agent_names_deduplicate_in_first_seen_order() {
        let runs = vec![
            RunLabel::new(vec!["b".to_string(), "a".to_string()], None, None),
            RunLabel::new(vec!["a".to_string()], None, None),
        ];
        let text = metadata_text(&runs, 0, 0).expect("two runs");
        assert_eq!(text.summary, "⏳ 3 subagents (b, a)");
    }

    /// `from_async_run` names the run's step agents.
    #[test]
    fn an_async_run_contributes_its_step_agents() {
        let status = running_run(vec![step("planner"), step("builder")]);
        let view = AsyncRunView {
            paths: crate::background::RunPaths::for_run(
                std::path::Path::new("/tmp/async"),
                std::path::Path::new("/tmp/results"),
                &RunId::from_token("r1".to_string()),
            ),
            status,
            session_id: None,
            description: None,
            context: None,
            nested_children: Vec::new(),
        };
        let label = RunLabel::from_async_run(&view);
        assert_eq!(
            label.agents(),
            ["planner".to_string(), "builder".to_string()]
        );
        assert_eq!(label.task_label(), None);
    }
}
