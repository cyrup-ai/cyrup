//! The workflow checklist projection and formatters (SCOPE_3e SUBTASK2) — pi
//! `workflows/workflow-checklist.ts` (436 LOC), complete.
//!
//! A pure projection: graph + steps + host steps + trace + preflight ⇒ a phase/item checklist. No
//! I/O, and no clock — `now` is an input. Consumes `background::workflow_graph`'s
//! node/phase/snapshot types and SUBTASK0's [`HostStepNode`] rather than redeclaring either;
//! consumes SUBTASK1's [`workflow_preflight_lane_for_runtime_key`] exactly as upstream imports it
//! (`workflow-checklist.ts:3`, `as laneFor`).
//!
//! Only [`project_workflow_checklist`] and the four formatters upstream exports are `pub`;
//! `stateLabel`, `formatWorkflowChecklistItem` and `formatDurationText` are module-private
//! upstream (`:387`, `:410`, `:416`) and stay private here.

use super::display_text::{sanitize_display_text, truncate_display};
use super::host_step::{HostStepMonitorKind, HostStepNode, HostStepVerdict};
use super::preflight::{WorkflowTraceLike, workflow_preflight_lane_for_runtime_key};
use super::types::{WorkflowPreflight, WorkflowPreflightLane};
use crate::background::{
    WorkflowGraphNode, WorkflowGraphSnapshot, WorkflowNodeKind, WorkflowNodeStatus,
};

/// pi `MAX_TEXT` (`workflow-checklist.ts:115`) — the display slice every [`text`] result gets,
/// through §A.4's [`truncate_display`] (UTF-16 units). This one *truncates*, unlike preflight's
/// reject-on-overflow rule — the two behaviours are §A.4's two families and are never conflated.
const MAX_TEXT: usize = 160;

/// pi `WorkflowChecklistState` (`workflow-checklist.ts:5`).
///
/// Seven states, not the six a run has: `Blocked` and `Queued` are checklist-only — a `Queued`
/// item is declared and waiting its turn, a `Blocked` one is held by an explicit signal
/// ([`explicit_blocked`]). Neither is a [`crate::background::RunState`], because neither is a
/// state the run itself is ever in; they are properties of an item's position in the graph.
///
/// There is no `Rejected` and no `Detached`: [`checklist_state`] folds `rejected` into `Blocked`
/// and `detached` into `Paused` (`:174-176`), which is why this cannot be
/// [`crate::workflows::WorkflowChildState`] (eight variants, both of those distinct).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkflowChecklistState {
    /// Finished successfully.
    Complete,
    /// Currently executing (rendered as `active`).
    Running,
    /// Declared and waiting its turn — the default for any unrecognized status word.
    Queued,
    /// Held by an explicit signal (budget, watchdog, acceptance, review, staleness).
    Blocked,
    /// Finished with a failure.
    Failed,
    /// Interrupted (or detached) — soft.
    Paused,
    /// Explicitly stopped (or cancelled).
    Stopped,
}

impl WorkflowChecklistState {
    /// pi `TERMINAL_STATES` (`workflow-checklist.ts:116`) = `{complete, blocked, failed, paused,
    /// stopped}` — **`blocked` counts as terminal here**; `queued` and `running` do not.
    ///
    /// This is the checklist's own set, over the checklist's own type. It mirrors the shape of
    /// [`crate::workflows::WorkflowChildState::is_terminal_for_inventory`] but the two predicates
    /// are over unrelated types and must never be unified (SCOPE_3e §0.12; that method's own doc
    /// carries the same warning).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        match self {
            Self::Complete | Self::Blocked | Self::Failed | Self::Paused | Self::Stopped => true,
            Self::Running | Self::Queued => false,
        }
    }

    /// The canonical lowercase status word — what `mergeNodeStep` feeds back into `stepItem`'s
    /// `status: string` upstream (§0.13's round-trip). Every one of these seven words is also an
    /// alias in [`status_only_state`], so [`checklist_state`] accepts its own output.
    fn as_status_word(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

/// pi `WorkflowChecklistStep["context"]` (`workflow-checklist.ts:16`): `"fresh" | "fork"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowChecklistContext {
    /// The child ran on a fresh context.
    Fresh,
    /// The child ran on a forked context.
    Fork,
}

impl WorkflowChecklistContext {
    /// The display word the two item formatters parenthesize.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Fork => "fork",
        }
    }
}

/// pi `WorkflowChecklistStep["acceptance"]["reviewResult"]` (`workflow-checklist.ts:33`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistReviewResult {
    /// The nested review status word.
    pub status: Option<String>,
}

/// pi `WorkflowChecklistStep["acceptance"]` (`workflow-checklist.ts:33`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistAcceptance {
    /// The acceptance status word.
    pub status: Option<String>,
    /// The acceptance-attached review result — the FALLBACK arm of `explicitBlocked`'s review
    /// chain (`:143`), consulted only when `review.status` is absent.
    pub review_result: Option<WorkflowChecklistReviewResult>,
}

/// pi `WorkflowChecklistStep["review"]` (`workflow-checklist.ts:34`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistReview {
    /// The review status word.
    pub status: Option<String>,
}

/// pi `WorkflowChecklistStep["watchdog"]` (`workflow-checklist.ts:35`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistWatchdog {
    /// The watchdog phase word (`"stale"` is the block signal).
    pub phase: Option<String>,
}

/// One step row a caller projects — pi `WorkflowChecklistStep` (`workflow-checklist.ts:7-36`).
///
/// `status` stays a `String` rather than "improving" to [`WorkflowChecklistState`]: the enum
/// cannot represent `"accepted"` or `"canceled"`, which arrive from real graph and trace data,
/// and `mergeNodeStep` feeds a derived state's word back through this field (§0.13's round-trip).
/// `stopped` is carried as upstream declares it but — exactly as upstream — no state derivation
/// reads it (`explicitBlocked`/`checklistState`'s `Pick` lists omit it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistStep {
    /// The step's workflow key.
    pub key: Option<String>,
    /// An alternative workflow key field ([`step_key`]'s second arm).
    pub workflow_key: Option<String>,
    /// The step's run id ([`step_key`]'s third arm).
    pub run_id: Option<String>,
    /// The display label.
    pub label: Option<String>,
    /// The display description (a label fallback).
    pub description: Option<String>,
    /// The declared phase.
    pub phase: Option<String>,
    /// The child agent name.
    pub agent: Option<String>,
    /// The raw status word (see the struct doc for why this is not the enum).
    pub status: String,
    /// The child's context mode.
    pub context: Option<WorkflowChecklistContext>,
    /// The live activity state (`"needs_attention"` is a block signal).
    pub activity_state: Option<String>,
    /// Epoch-millis the step started.
    pub started_at: Option<f64>,
    /// Epoch-millis the step ended.
    pub ended_at: Option<f64>,
    /// Explicit elapsed milliseconds (wins over the timestamps).
    pub duration_ms: Option<f64>,
    /// The tool currently executing.
    pub current_tool: Option<String>,
    /// Epoch-millis the current tool started.
    pub current_tool_started_at: Option<f64>,
    /// The path the current tool is working on.
    pub current_path: Option<String>,
    /// Count of assistant turns.
    pub turn_count: Option<f64>,
    /// Count of tool calls.
    pub tool_count: Option<f64>,
    /// The named output this step registers.
    pub output_name: Option<String>,
    /// The step's error text.
    pub error: Option<String>,
    /// The tool budget blocked this child.
    pub tool_budget_blocked: Option<bool>,
    /// The turn budget was exceeded.
    pub turn_budget_exceeded: Option<bool>,
    /// The child timed out.
    pub timed_out: Option<bool>,
    /// The child was stopped (carried, never read by the state derivation — see the struct doc).
    pub stopped: Option<bool>,
    /// The acceptance verdict surface.
    pub acceptance: Option<WorkflowChecklistAcceptance>,
    /// The review verdict surface.
    pub review: Option<WorkflowChecklistReview>,
    /// The watchdog surface.
    pub watchdog: Option<WorkflowChecklistWatchdog>,
}

/// One trace row the checklist reads — pi `WorkflowChecklistTraceEntry`
/// (`workflow-checklist.ts:38-49`). Its own shape, distinct from
/// [`crate::workflows::WorkflowScriptTraceEntry`]: `operation` and `state` are raw strings here
/// because this is a *view* input assembled by the caller, not the executor's persisted record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistTraceEntry {
    /// The operation word (`"run"`/`"host"`); an absent operation is KEPT by [`trace_sources`].
    pub operation: Option<String>,
    /// The workflow key.
    pub key: String,
    /// The state word.
    pub state: String,
    /// The child agent name.
    pub agent: Option<String>,
    /// The launched child's run id.
    pub run_id: Option<String>,
    /// The display phase.
    pub phase: Option<String>,
    /// The display label.
    pub label: Option<String>,
    /// Generated `runs.lanes` provenance.
    pub generated_lane_key: Option<String>,
    /// Elapsed milliseconds.
    pub duration_ms: Option<f64>,
    /// The error text.
    pub error: Option<String>,
}

impl WorkflowTraceLike for WorkflowChecklistTraceEntry {
    /// An absent operation reads as `""` — never equal to `"run"`, exactly as upstream's
    /// `undefined !== "run"` behaves in the preflight advisory functions. (The checklist's own
    /// [`trace_sources`] keeps no-operation entries; that is a different filter with a different
    /// rule, deliberately.)
    fn operation(&self) -> &str {
        self.operation.as_deref().unwrap_or("")
    }

    fn key(&self) -> &str {
        &self.key
    }

    fn phase(&self) -> Option<&str> {
        self.phase.as_deref()
    }

    fn generated_lane_key(&self) -> Option<&str> {
        self.generated_lane_key.as_deref()
    }

    /// The checklist trace row has no warning column; the annotator's "preserve an existing
    /// warning" arm is therefore never taken for this type, and an attached warning has nowhere
    /// to land — matching upstream, where `WorkflowChecklistTraceEntry` likewise carries no
    /// `warning` field and the annotator is only ever applied to script trace rows.
    fn warning(&self) -> Option<&str> {
        None
    }

    fn set_warning(&mut self, _warning: String) {}
}

/// The item kind tag — pi `WorkflowChecklistItem["kind"]` (`workflow-checklist.ts:66`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowChecklistItemKind {
    /// A child-agent item.
    Child,
    /// A host-step item.
    Host,
}

/// One checklist row — pi `WorkflowChecklistItem` (`workflow-checklist.ts:52-75`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowChecklistItem {
    /// The item's key (sanitized display text).
    pub key: String,
    /// The item's label (sanitized display text).
    pub label: String,
    /// The phase this item belongs to.
    pub phase: String,
    /// The derived state.
    pub state: WorkflowChecklistState,
    /// The child agent name.
    pub agent: Option<String>,
    /// The child's context mode.
    pub context: Option<WorkflowChecklistContext>,
    /// Epoch-millis the item started.
    pub started_at: Option<f64>,
    /// Elapsed milliseconds.
    pub duration_ms: Option<f64>,
    /// The tool currently executing.
    pub current_tool: Option<String>,
    /// Epoch-millis the current tool started.
    pub current_tool_started_at: Option<f64>,
    /// The path the current tool is working on.
    pub current_path: Option<String>,
    /// Count of tool calls.
    pub tool_count: Option<u64>,
    /// The named output this item registers.
    pub output_name: Option<String>,
    /// The item's error text.
    pub error: Option<String>,
    /// The advisory preflight lane resolved for this item.
    pub preflight: Option<WorkflowPreflightLane>,
    /// The item kind tag (`Host` for host steps and `"host"` trace rows).
    pub kind: Option<WorkflowChecklistItemKind>,
    /// The host monitor kind.
    pub monitor_kind: Option<HostStepMonitorKind>,
    /// The host provider.
    pub provider: Option<String>,
    /// The host role.
    pub role: Option<String>,
    /// The host verdict.
    pub verdict: Option<HostStepVerdict>,
    /// The host target.
    pub target: Option<String>,
    /// The host reason code.
    pub reason_code: Option<String>,
    /// The host observation staleness.
    pub stale: Option<bool>,
    /// The host report path.
    pub report_path: Option<String>,
}

/// One phase group — pi `WorkflowChecklistPhase` (`workflow-checklist.ts:77-91`).
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowChecklistPhase {
    /// The phase key (upstream sets it to the label).
    pub key: String,
    /// The phase label.
    pub label: String,
    /// The rolled-up phase state ([`phase_state_cascade`]).
    pub state: WorkflowChecklistState,
    /// The items, in insertion order.
    pub items: Vec<WorkflowChecklistItem>,
    /// Total items.
    pub total: usize,
    /// Items in `Complete`.
    pub done: usize,
    /// Items in `Running`.
    pub running: usize,
    /// Items in `Queued`.
    pub queued: usize,
    /// Items in `Blocked`.
    pub blocked: usize,
    /// Items in `Failed`.
    pub failed: usize,
    /// Items in `Paused`.
    pub paused: usize,
    /// Items in `Stopped`.
    pub stopped: usize,
    /// Whether the phase holds more than one item.
    pub parallel: bool,
}

/// The full projection — pi `WorkflowChecklistProjection` (`workflow-checklist.ts:93-104`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistProjection {
    /// The non-empty phases, in insertion order.
    pub phases: Vec<WorkflowChecklistPhase>,
    /// Total items across phases.
    pub total: usize,
    /// Items in `Complete`.
    pub done: usize,
    /// Items in `Running`.
    pub running: usize,
    /// Items in `Queued`.
    pub queued: usize,
    /// Items in `Blocked`.
    pub blocked: usize,
    /// Items in `Failed`.
    pub failed: usize,
    /// Items in `Paused`.
    pub paused: usize,
    /// Items in `Stopped`.
    pub stopped: usize,
    /// The first lowest-priority item, absent when it is `Complete` (the projection's own
    /// `bottleneck.state !== "complete"` guard, `:384` — the formatter never re-checks).
    pub bottleneck: Option<WorkflowChecklistItem>,
}

impl Default for WorkflowChecklistState {
    /// The projection default is `Queued` — `normalizedStatus`'s own fallback.
    fn default() -> Self {
        Self::Queued
    }
}

/// The projection input — pi `WorkflowChecklistInput` (`workflow-checklist.ts:106-113`).
/// (Upstream's `steps` union with `AsyncJobStep` is a TS-side cast; the one Rust shape is
/// [`WorkflowChecklistStep`].)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkflowChecklistInput {
    /// The workflow graph snapshot, when one exists.
    pub graph: Option<WorkflowGraphSnapshot>,
    /// The step rows.
    pub steps: Vec<WorkflowChecklistStep>,
    /// The host-step rows.
    pub host_steps: Vec<HostStepNode>,
    /// The declared preflight, when any.
    pub preflight: Option<WorkflowPreflight>,
    /// The trace rows.
    pub trace: Vec<WorkflowChecklistTraceEntry>,
    /// The clock, as an input — this module never reads one.
    pub now: Option<f64>,
}

/// pi `text` (`workflow-checklist.ts:118-122`): sanitize
/// ([`sanitize_display_text`] — the ANSI-stripping normalizer, NOT preflight's cheaper JSON-input
/// rule), collapse whitespace, trim, then **truncate** to [`MAX_TEXT`] UTF-16 units via §A.4's
/// [`truncate_display`]. An empty result yields `fallback`, not `""`.
fn text(value: Option<&str>, fallback: Option<&str>) -> Option<String> {
    let Some(value) = value else {
        return fallback.map(str::to_string);
    };
    let sanitized = sanitize_display_text(value);
    let clean = sanitized.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        fallback.map(str::to_string)
    } else {
        Some(truncate_display(&clean, MAX_TEXT))
    }
}

/// pi `keyText` (`workflow-checklist.ts:124-126`): [`text`] with a non-optional fallback.
fn key_text(value: Option<&str>, fallback: &str) -> String {
    text(value, Some(fallback)).unwrap_or_else(|| fallback.to_string())
}

/// pi `normalizedStatus` (`workflow-checklist.ts:128-130`): lowercased, defaulting to `"queued"`.
fn normalized_status(value: Option<&str>) -> String {
    value.map_or_else(|| "queued".to_string(), str::to_lowercase)
}

/// pi `finite` (`workflow-checklist.ts:132-134`).
fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

/// pi `count` (`workflow-checklist.ts:136-139`): finite ▸ non-negative ▸ round. For a
/// non-negative value `Math.round` and [`f64::round`] agree (half away from zero IS half up), so
/// the integral result is carried as `u64`.
fn count(value: Option<f64>) -> Option<u64> {
    let number = finite(value)?;
    if number >= 0.0 {
        Some(number.round() as u64)
    } else {
        None
    }
}

/// The borrowed surface both state derivations read — pi's structural
/// `Pick<WorkflowChecklistStep, …> & { verdict?: unknown; stale?: unknown }`
/// (`workflow-checklist.ts:141`, `:157`): steps have the budget/acceptance/review/watchdog
/// signals, host items contribute `verdict`/`stale`.
#[derive(Clone, Copy, Default)]
struct StateSource<'a> {
    status: Option<&'a str>,
    activity_state: Option<&'a str>,
    tool_budget_blocked: Option<bool>,
    turn_budget_exceeded: Option<bool>,
    timed_out: Option<bool>,
    acceptance_status: Option<&'a str>,
    acceptance_review_status: Option<&'a str>,
    review_status: Option<&'a str>,
    watchdog_phase: Option<&'a str>,
    verdict: Option<&'a str>,
    stale: Option<bool>,
}

/// A step's [`StateSource`] view.
fn step_source(step: &WorkflowChecklistStep) -> StateSource<'_> {
    StateSource {
        status: Some(&step.status),
        activity_state: step.activity_state.as_deref(),
        tool_budget_blocked: step.tool_budget_blocked,
        turn_budget_exceeded: step.turn_budget_exceeded,
        timed_out: step.timed_out,
        acceptance_status: step
            .acceptance
            .as_ref()
            .and_then(|acceptance| acceptance.status.as_deref()),
        acceptance_review_status: step
            .acceptance
            .as_ref()
            .and_then(|acceptance| acceptance.review_result.as_ref())
            .and_then(|review| review.status.as_deref()),
        review_status: step
            .review
            .as_ref()
            .and_then(|review| review.status.as_deref()),
        watchdog_phase: step
            .watchdog
            .as_ref()
            .and_then(|watchdog| watchdog.phase.as_deref()),
        verdict: None,
        stale: None,
    }
}

/// pi `explicitBlocked` (`workflow-checklist.ts:141-155`) — the 11-way disjunction, literally.
/// `review` is `review.status ?? acceptance.reviewResult.status` — the fallback chain reaches two
/// levels into `acceptance` only when `review.status` is absent (`:143`).
fn explicit_blocked(source: &StateSource<'_>) -> bool {
    let acceptance = normalized_status(source.acceptance_status);
    let review = normalized_status(source.review_status.or(source.acceptance_review_status));
    source.tool_budget_blocked == Some(true)
        || source.turn_budget_exceeded == Some(true)
        || source.activity_state == Some("needs_attention")
        || source.timed_out == Some(true)
        || source.watchdog_phase == Some("stale")
        || source.stale == Some(true)
        || source.verdict == Some("inconclusive")
        || acceptance == "rejected"
        || acceptance == "blockers"
        || review == "blockers"
        || review == "review-required"
}

/// The status-word half of [`checklist_state`] — all seventeen aliases with `Queued` as the
/// default (`workflow-checklist.ts:158-181`). Takes an already-normalized (lowercased) word.
fn status_only_state(status: &str) -> WorkflowChecklistState {
    match status {
        "complete" | "completed" | "done" | "pass" | "accepted" => WorkflowChecklistState::Complete,
        "running" | "started" | "active" => WorkflowChecklistState::Running,
        "failed" | "error" | "fail" => WorkflowChecklistState::Failed,
        "blocked" | "rejected" | "partial" | "needs_attention" => WorkflowChecklistState::Blocked,
        "paused" | "detached" => WorkflowChecklistState::Paused,
        "stopped" | "cancelled" | "canceled" => WorkflowChecklistState::Stopped,
        _ => WorkflowChecklistState::Queued,
    }
}

/// pi `checklistState` (`workflow-checklist.ts:157-182`): [`explicit_blocked`] runs FIRST (⇒
/// `Blocked`), then the lowercased status match with `Queued` as the default.
fn checklist_state(source: &StateSource<'_>) -> WorkflowChecklistState {
    if explicit_blocked(source) {
        return WorkflowChecklistState::Blocked;
    }
    status_only_state(&normalized_status(source.status))
}

/// pi `duration` (`workflow-checklist.ts:184-191`): explicit `durationMs` wins (clamped at 0);
/// else `None` without a `startedAt`; else `now - startedAt` for a `Running` item, or
/// `(endedAt ?? now) - startedAt` otherwise; always clamped at 0.
fn duration_of(
    duration_ms: Option<f64>,
    started_at: Option<f64>,
    ended_at: Option<f64>,
    now: Option<f64>,
    state: WorkflowChecklistState,
) -> Option<f64> {
    if let Some(explicit) = finite(duration_ms) {
        return Some(explicit.max(0.0));
    }
    let started_at = finite(started_at)?;
    let end = if state == WorkflowChecklistState::Running {
        now
    } else {
        finite(ended_at).or(now)
    };
    end.map(|end| (end - started_at).max(0.0))
}

/// pi `stepKey` (`workflow-checklist.ts:193-195`): `key ?? workflowKey ?? runId`.
fn step_key(step: &WorkflowChecklistStep) -> Option<&str> {
    step.key
        .as_deref()
        .or(step.workflow_key.as_deref())
        .or(step.run_id.as_deref())
}

/// pi `stepItem` (`workflow-checklist.ts:197-216`). `label: None` is pi's undefined argument,
/// which takes the default chain `step.label ?? step.description ?? stepKey(step) ?? step.agent
/// ?? key`; note [`duration_of`] is called with `now: None` (`:198`) — a live duration is
/// [`apply_now`]'s job, never this builder's.
fn step_item(
    step: &WorkflowChecklistStep,
    index: usize,
    phase: &str,
    key: &str,
    label: Option<&str>,
    preflight: Option<&WorkflowPreflightLane>,
) -> WorkflowChecklistItem {
    let state = checklist_state(&step_source(step));
    let label = label
        .or(step.label.as_deref())
        .or(step.description.as_deref())
        .or_else(|| step_key(step))
        .or(step.agent.as_deref())
        .unwrap_or(key);
    WorkflowChecklistItem {
        key: key_text(Some(key), &format!("step-{}", index + 1)),
        label: key_text(Some(label), key),
        phase: phase.to_string(),
        state,
        agent: text(step.agent.as_deref(), None),
        context: step.context,
        started_at: finite(step.started_at),
        duration_ms: duration_of(step.duration_ms, step.started_at, step.ended_at, None, state),
        current_tool: text(step.current_tool.as_deref(), None),
        current_tool_started_at: finite(step.current_tool_started_at),
        current_path: text(step.current_path.as_deref(), None),
        tool_count: count(step.tool_count),
        output_name: text(step.output_name.as_deref(), None),
        error: text(step.error.as_deref(), None),
        preflight: preflight.cloned(),
        kind: None,
        monitor_kind: None,
        provider: None,
        role: None,
        verdict: None,
        target: None,
        reason_code: None,
        stale: None,
        report_path: None,
    }
}

/// pi `hostItem` (`workflow-checklist.ts:218-236`). `key: None` is pi's defaulted `host.id`.
fn host_item(host: &HostStepNode, phase: &str, key: Option<&str>) -> WorkflowChecklistItem {
    let key = key.unwrap_or(&host.id);
    let stale = host.freshness.as_ref().and_then(|freshness| freshness.stale);
    let source = StateSource {
        status: Some(host.state.as_str()),
        verdict: host.verdict.map(HostStepVerdict::as_str),
        stale,
        ..StateSource::default()
    };
    let state = checklist_state(&source);
    WorkflowChecklistItem {
        key: key_text(Some(key), "host-step"),
        label: key_text(Some(&host.label), "host step"),
        phase: phase.to_string(),
        state,
        agent: None,
        context: None,
        started_at: None,
        duration_ms: None,
        current_tool: None,
        current_tool_started_at: None,
        current_path: None,
        tool_count: None,
        output_name: None,
        error: text(host.detail.as_deref(), None),
        preflight: None,
        kind: Some(WorkflowChecklistItemKind::Host),
        monitor_kind: Some(host.monitor_kind),
        provider: text(host.provider.as_deref(), None),
        role: text(host.role.as_deref(), None),
        verdict: host.verdict,
        target: text(host.target.as_deref(), None),
        reason_code: text(host.reason_code.as_deref(), None),
        stale,
        report_path: text(host.report_path.as_deref(), None),
    }
}

/// pi `graphNodes` (`workflow-checklist.ts:238-250`): flatten the tree — a group recurses into
/// its children **but is emitted itself when it has none** (`:243`); an empty group is still a
/// row. Everything else is emitted directly.
fn graph_nodes(graph: Option<&WorkflowGraphSnapshot>) -> Vec<&WorkflowGraphNode> {
    fn visit<'a>(node: &'a WorkflowGraphNode, out: &mut Vec<&'a WorkflowGraphNode>) {
        match node.kind {
            WorkflowNodeKind::ParallelGroup | WorkflowNodeKind::DynamicParallelGroup => {
                let children = node.children.as_deref().unwrap_or(&[]);
                for child in children {
                    visit(child, out);
                }
                if children.is_empty() {
                    out.push(node);
                }
            }
            WorkflowNodeKind::Step | WorkflowNodeKind::Agent => out.push(node),
        }
    }
    let mut out = Vec::new();
    if let Some(graph) = graph {
        for node in &graph.nodes {
            visit(node, &mut out);
        }
    }
    out
}

/// The field-wise half of [`trace_sources`]' cumulative merge: later non-absent fields overwrite
/// the accumulated row; absent fields do NOT clear earlier values; `state` is preserved for a
/// `reused` entry. An explicit per-field `if let` — a struct-update `..` expression would
/// overwrite with `None` (SCOPE_3e §0.13).
fn merge_trace_entry(
    accumulated: &mut WorkflowChecklistTraceEntry,
    entry: &WorkflowChecklistTraceEntry,
    keep_state: bool,
) {
    if let Some(operation) = &entry.operation {
        accumulated.operation = Some(operation.clone());
    }
    if !keep_state {
        accumulated.state = entry.state.clone();
    }
    if let Some(agent) = &entry.agent {
        accumulated.agent = Some(agent.clone());
    }
    if let Some(run_id) = &entry.run_id {
        accumulated.run_id = Some(run_id.clone());
    }
    if let Some(phase) = &entry.phase {
        accumulated.phase = Some(phase.clone());
    }
    if let Some(label) = &entry.label {
        accumulated.label = Some(label.clone());
    }
    if let Some(generated) = &entry.generated_lane_key {
        accumulated.generated_lane_key = Some(generated.clone());
    }
    if let Some(duration_ms) = entry.duration_ms {
        accumulated.duration_ms = Some(duration_ms);
    }
    if let Some(error) = &entry.error {
        accumulated.error = Some(error.clone());
    }
}

/// pi `traceSources` (`workflow-checklist.ts:252-261`) — a **cumulative field merge**, not a
/// dedupe (§0.13).
///
/// Skip when: the operation is present AND not `run`/`host` (so an entry with NO operation is
/// kept — the `!== undefined` guard), OR the key is empty, OR the normalized state is
/// `delivered`/`missed`. Merge: later entries overwrite field-by-field over the accumulated row,
/// and a `reused` entry updates every field EXCEPT `state`. Order: insertion order of first
/// appearance (JS `Map`; a `Vec` here for the same reason as 3d's builders, SCOPE_3d §0.16).
fn trace_sources(trace: &[WorkflowChecklistTraceEntry]) -> Vec<WorkflowChecklistTraceEntry> {
    let mut latest: Vec<WorkflowChecklistTraceEntry> = Vec::new();
    for entry in trace {
        let state = normalized_status(Some(&entry.state));
        let operation_excluded = entry
            .operation
            .as_deref()
            .is_some_and(|operation| operation != "run" && operation != "host");
        if operation_excluded || entry.key.is_empty() || state == "delivered" || state == "missed"
        {
            continue;
        }
        if let Some(existing) = latest.iter_mut().find(|existing| existing.key == entry.key) {
            merge_trace_entry(existing, entry, state == "reused");
        } else {
            latest.push(entry.clone());
        }
    }
    latest
}

/// pi `traceItem` (`workflow-checklist.ts:263-268`): an item for a trace key no other pass
/// covered. The phase fallback chain is `preflight.key ?? generatedLaneKey ?? phase` ⇒
/// `"Workflow"`; a raw `"started"` state (exact match, not normalized) maps to `"running"`; an
/// `operation == "host"` row is tagged [`WorkflowChecklistItemKind::Host`].
fn trace_item(
    entry: &WorkflowChecklistTraceEntry,
    index: usize,
    preflight: Option<&WorkflowPreflightLane>,
) -> WorkflowChecklistItem {
    let phase = key_text(
        preflight
            .map(|lane| lane.key.as_str())
            .or(entry.generated_lane_key.as_deref())
            .or(entry.phase.as_deref()),
        "Workflow",
    );
    let status = if entry.state == "started" {
        "running".to_string()
    } else {
        entry.state.clone()
    };
    let step = WorkflowChecklistStep {
        key: Some(entry.key.clone()),
        label: entry.label.clone(),
        phase: Some(phase.clone()),
        agent: entry.agent.clone(),
        status,
        duration_ms: entry.duration_ms,
        error: entry.error.clone(),
        ..WorkflowChecklistStep::default()
    };
    let label = entry.label.clone().unwrap_or_else(|| entry.key.clone());
    let mut item = step_item(&step, index, &phase, &entry.key, Some(&label), preflight);
    if entry.operation.as_deref() == Some("host") {
        item.kind = Some(WorkflowChecklistItemKind::Host);
    }
    item
}

/// An empty phase group — pi `phaseFor`'s creation arm (`workflow-checklist.ts:270-277`).
fn new_phase(label: &str) -> WorkflowChecklistPhase {
    WorkflowChecklistPhase {
        key: label.to_string(),
        label: label.to_string(),
        state: WorkflowChecklistState::Queued,
        items: Vec::new(),
        total: 0,
        done: 0,
        running: 0,
        queued: 0,
        blocked: 0,
        failed: 0,
        paused: 0,
        stopped: 0,
        parallel: false,
    }
}

/// pi `phaseFor`'s ensure-exists half — used when seeding phases from the graph, which registers
/// the phase (and its insertion position) even before any item lands in it.
fn ensure_phase(phases: &mut Vec<WorkflowChecklistPhase>, label: &str) {
    if !phases.iter().any(|phase| phase.label == label) {
        phases.push(new_phase(label));
    }
}

/// pi `add` (`workflow-checklist.ts:279-281`). The phase list is a `Vec` with linear lookup —
/// phase order is observable (`[...phases.values()]`), so this is an insertion-ordered map, never
/// a `BTreeMap`/`HashMap` (SCOPE_3d §0.16, restated by SCOPE_3e's DoD).
fn add_item(
    phases: &mut Vec<WorkflowChecklistPhase>,
    label: &str,
    item: WorkflowChecklistItem,
) {
    if let Some(phase) = phases.iter_mut().find(|phase| phase.label == label) {
        phase.items.push(item);
        return;
    }
    let mut phase = new_phase(label);
    phase.items.push(item);
    phases.push(phase);
}

/// The [`WorkflowNodeStatus`] wire word — what upstream's raw `node.status` string carries.
fn node_status_word(status: WorkflowNodeStatus) -> &'static str {
    match status {
        WorkflowNodeStatus::Pending => "pending",
        WorkflowNodeStatus::Running => "running",
        WorkflowNodeStatus::Completed => "completed",
        WorkflowNodeStatus::Failed => "failed",
        WorkflowNodeStatus::Paused => "paused",
        WorkflowNodeStatus::Stopped => "stopped",
        WorkflowNodeStatus::Detached => "detached",
    }
}

/// pi `mergeNodeStep`'s status cascade (`workflow-checklist.ts:285-289`) — a four-way precedence,
/// one pure function returning one enum (§A.1, §0.13), never a ternary chain inside the item
/// builder:
///
/// 1. node is terminal && step is NOT ⇒ the node's state (the graph knows it ended);
/// 2. a trace row exists && step is NOT terminal ⇒ the trace state, with `"started"` ⇒
///    `"running"`;
/// 3. step is `"pending"` && node is not ⇒ the node's status;
/// 4. otherwise ⇒ the step's own status.
///
/// Upstream's cascade yields a raw status *string* that `stepItem` re-derives through
/// `checklistState`; returning the enum and feeding its canonical word back through the same
/// derivation is behaviour-identical, because every alias set maps its own canonical word to
/// itself and `explicitBlocked` reads only the flags, never the status.
fn merge_node_status(
    node_state: WorkflowChecklistState,
    step_state: WorkflowChecklistState,
    trace: Option<&WorkflowChecklistTraceEntry>,
    step_status: &str,
    node_status: WorkflowNodeStatus,
) -> WorkflowChecklistState {
    // Arm 1 — the graph knows it ended.
    if node_state.is_terminal() && !step_state.is_terminal() {
        return node_state;
    }
    // Arm 2 — the live trace word, `"started"` promoted to `"running"`.
    if let Some(trace) = trace
        && !step_state.is_terminal()
    {
        let word = if trace.state == "started" {
            "running".to_string()
        } else {
            normalized_status(Some(&trace.state))
        };
        return status_only_state(&word);
    }
    // Arm 3 — a pending step yields to a non-pending node.
    if normalized_status(Some(step_status)) == "pending"
        && node_status_word(node_status) != "pending"
    {
        return status_only_state(node_status_word(node_status));
    }
    // Arm 4 — the step's own status.
    status_only_state(&normalized_status(Some(step_status)))
}

/// pi `mergeNodeStep` (`workflow-checklist.ts:283-292`): the status comes from
/// [`merge_node_status`]; every remaining field is a plain `??` chain over step ▸ trace ▸ node.
fn merge_node_step(
    node: &WorkflowGraphNode,
    step: &WorkflowChecklistStep,
    phase: &str,
    trace: Option<&WorkflowChecklistTraceEntry>,
    preflight: Option<&WorkflowPreflightLane>,
) -> WorkflowChecklistItem {
    let step_state = checklist_state(&step_source(step));
    let node_source = StateSource {
        status: Some(node_status_word(node.status)),
        acceptance_status: node.acceptance_status.as_deref(),
        ..StateSource::default()
    };
    let node_state = checklist_state(&node_source);
    let merged_state =
        merge_node_status(node_state, step_state, trace, &step.status, node.status);

    let mut merged = step.clone();
    merged.status = merged_state.as_status_word().to_string();
    merged.key = Some(node.id.clone());
    merged.label = step.label.clone().or_else(|| Some(node.label.clone()));
    merged.phase = step.phase.clone().or_else(|| Some(phase.to_string()));
    merged.agent = step.agent.clone().or_else(|| node.agent.clone());
    merged.output_name = step
        .output_name
        .clone()
        .or_else(|| node.output_name.clone());
    merged.error = step
        .error
        .clone()
        .or_else(|| trace.and_then(|trace| trace.error.clone()))
        .or_else(|| node.error.clone());
    merged.acceptance = step.acceptance.clone().or_else(|| {
        node.acceptance_status
            .as_ref()
            .map(|status| WorkflowChecklistAcceptance {
                status: Some(status.clone()),
                review_result: None,
            })
    });
    merged.duration_ms = step
        .duration_ms
        .or_else(|| trace.and_then(|trace| trace.duration_ms));

    let label = step.label.clone().or_else(|| Some(node.label.clone()));
    step_item(
        &merged,
        node.flat_index.unwrap_or(0),
        phase,
        &node.id,
        label.as_deref(),
        preflight,
    )
}

/// pi `priority` (`workflow-checklist.ts:294-296`) — a total order over the seven states.
fn priority(item: &WorkflowChecklistItem) -> u8 {
    match item.state {
        WorkflowChecklistState::Blocked => 0,
        WorkflowChecklistState::Failed => 1,
        WorkflowChecklistState::Running => 2,
        WorkflowChecklistState::Paused => 3,
        WorkflowChecklistState::Stopped => 4,
        WorkflowChecklistState::Queued => 5,
        WorkflowChecklistState::Complete => 6,
    }
}

/// pi `applyNow` (`workflow-checklist.ts:298-300`): fill `durationMs` from `now` only when it is
/// still absent and `startedAt` is present — idempotent, so the projection's second pass over
/// already-applied items is a no-op.
fn apply_now(item: WorkflowChecklistItem, now: Option<f64>) -> WorkflowChecklistItem {
    if item.duration_ms.is_none()
        && let Some(started_at) = item.started_at
        && let Some(now) = now
    {
        let mut item = item;
        item.duration_ms = Some((now - started_at).max(0.0));
        return item;
    }
    item
}

/// The phase-state cascade of pi `finalize` (`workflow-checklist.ts:312`) — one pure function
/// returning one enum, not a nested ternary (§A.1): `blocked ▸ failed ▸ running ▸ paused ▸
/// stopped ▸ queued ▸ complete`.
fn phase_state_cascade(phase: &WorkflowChecklistPhase) -> WorkflowChecklistState {
    if phase.blocked > 0 {
        return WorkflowChecklistState::Blocked;
    }
    if phase.failed > 0 {
        return WorkflowChecklistState::Failed;
    }
    if phase.running > 0 {
        return WorkflowChecklistState::Running;
    }
    if phase.paused > 0 {
        return WorkflowChecklistState::Paused;
    }
    if phase.stopped > 0 {
        return WorkflowChecklistState::Stopped;
    }
    if phase.queued > 0 {
        return WorkflowChecklistState::Queued;
    }
    WorkflowChecklistState::Complete
}

/// pi `finalize` (`workflow-checklist.ts:302-313`): the seven counts, `parallel = total > 1`, and
/// the phase state via [`phase_state_cascade`].
fn finalize_phase(phase: &mut WorkflowChecklistPhase) {
    let count_state = |state: WorkflowChecklistState| {
        phase.items.iter().filter(|item| item.state == state).count()
    };
    phase.total = phase.items.len();
    phase.done = count_state(WorkflowChecklistState::Complete);
    phase.running = count_state(WorkflowChecklistState::Running);
    phase.queued = count_state(WorkflowChecklistState::Queued);
    phase.blocked = count_state(WorkflowChecklistState::Blocked);
    phase.failed = count_state(WorkflowChecklistState::Failed);
    phase.paused = count_state(WorkflowChecklistState::Paused);
    phase.stopped = count_state(WorkflowChecklistState::Stopped);
    phase.parallel = phase.total > 1;
    phase.state = phase_state_cascade(phase);
}

/// Linear lookup into the deduped trace — pi's `traceByKey` `Map` (keys are unique after
/// [`trace_sources`], so a `find` is the same lookup).
fn trace_lookup<'a>(
    trace: &'a [WorkflowChecklistTraceEntry],
    key: &str,
) -> Option<&'a WorkflowChecklistTraceEntry> {
    trace.iter().find(|entry| entry.key == key)
}

/// pi `projectWorkflowChecklist` (`workflow-checklist.ts:315-386`) — four passes, in order:
///
/// 0. seed phases from `graph.phases` (title through [`key_text`] ⇒ `"Workflow"`; the FIRST
///    node-id assignment wins in `phaseByNode`, `:338`);
/// 1. graph nodes (`:341-357`) — a host node becomes a [`host_item`] and is REMOVED from the
///    leftover-host map; else ALL unused steps matching `node.id` merge (each consumed, each its
///    own item); else a synthetic step built from the node itself;
/// 2. leftover steps (`:359-367`) — any step index pass 1 did not consume;
/// 3. leftover hosts (`:369`) — phase = the host's label, fallback `"Host"` (NOT `"Workflow"`);
/// 4. uncovered trace (`:370-374`) — trace keys in none of the graph/step/host key sets.
///
/// Then: drop empty phases, [`apply_now`] every item again (idempotent), [`finalize_phase`] each
/// phase, count per state over the flattened items, and pick the bottleneck as the FIRST
/// lowest-[`priority`] item — [`Iterator::min_by_key`], which documents "the first element is
/// returned" on ties, mirroring the stable `sort(...)[0]` upstream (§0.13); emitted only when its
/// state is not `Complete`.
#[must_use]
pub fn project_workflow_checklist(input: &WorkflowChecklistInput) -> WorkflowChecklistProjection {
    let mut phases: Vec<WorkflowChecklistPhase> = Vec::new();
    let steps = &input.steps;
    let nodes = graph_nodes(input.graph.as_ref());
    let trace = trace_sources(&input.trace);
    let preflight = input.preflight.as_ref();

    // Pass 0 — seed phases (and the node→phase assignment, first wins) from the graph.
    let mut phase_by_node: std::collections::HashMap<&str, String> =
        std::collections::HashMap::new();
    if let Some(graph) = input.graph.as_ref() {
        for phase in &graph.phases {
            let title = key_text(Some(&phase.title), "Workflow");
            ensure_phase(&mut phases, &title);
            for node_id in &phase.node_ids {
                phase_by_node
                    .entry(node_id.as_str())
                    .or_insert_with(|| title.clone());
            }
        }
    }

    let mut steps_by_key: std::collections::HashMap<&str, Vec<usize>> =
        std::collections::HashMap::new();
    for (index, step) in steps.iter().enumerate() {
        let Some(key) = step_key(step) else { continue };
        steps_by_key.entry(key).or_default().push(index);
    }

    let mut used_steps: std::collections::HashSet<usize> = std::collections::HashSet::new();
    // Insertion-ordered host map (pi `new Map(hostSteps.map(h => [h.id, h]))` — a duplicate id
    // replaces the value but keeps the first position).
    let mut host_by_id: Vec<&HostStepNode> = Vec::new();
    for host in &input.host_steps {
        if let Some(slot) = host_by_id.iter_mut().find(|existing| existing.id == host.id) {
            *slot = host;
        } else {
            host_by_id.push(host);
        }
    }
    let graph_keys: std::collections::HashSet<&str> =
        nodes.iter().map(|node| node.id.as_str()).collect();
    let step_keys: std::collections::HashSet<&str> = steps_by_key.keys().copied().collect();
    let host_keys: std::collections::HashSet<String> =
        host_by_id.iter().map(|host| host.id.clone()).collect();

    // Pass 1 — graph nodes.
    for node in &nodes {
        // [CYRUP-DELTA, shape] pi `node.hostStep ?? hostById.get(node.id)` (`:341`). cyrup's
        // `WorkflowGraphNode` carries no inline `hostStep`, so only the second arm exists here.
        // This is behaviour-identical for every graph cyrup can currently produce (nothing
        // writes the field); SCOPE_3f restores the first arm in the same expression when it adds
        // `host_step` to the node.
        let host = host_by_id
            .iter()
            .copied()
            .find(|host| host.id == node.id);
        let phase_fallback = host.map_or(node.label.as_str(), |host| host.label.as_str());
        let phase_label = key_text(
            phase_by_node
                .get(node.id.as_str())
                .map(String::as_str)
                .or(node.phase.as_deref())
                .or(Some(phase_fallback)),
            "Workflow",
        );
        if let Some(host) = host {
            let item = host_item(host, &phase_label, Some(&node.id));
            add_item(&mut phases, &phase_label, item);
            host_by_id.retain(|existing| existing.id != node.id);
            continue;
        }
        let matches: Vec<usize> = steps_by_key
            .get(node.id.as_str())
            .map(|indices| {
                indices
                    .iter()
                    .copied()
                    .filter(|index| !used_steps.contains(index))
                    .collect()
            })
            .unwrap_or_default();
        if !matches.is_empty() {
            for index in matches {
                used_steps.insert(index);
                let Some(step) = steps.get(index) else { continue };
                let trace_entry =
                    trace_lookup(&trace, step_key(step).unwrap_or(node.id.as_str()));
                let lane = workflow_preflight_lane_for_runtime_key(
                    preflight,
                    &node.id,
                    &[Some(phase_label.as_str())],
                );
                let item = apply_now(
                    merge_node_step(node, step, &phase_label, trace_entry, lane),
                    input.now,
                );
                add_item(&mut phases, &phase_label, item);
            }
            continue;
        }
        let synthetic = WorkflowChecklistStep {
            key: Some(node.id.clone()),
            label: Some(node.label.clone()),
            phase: Some(phase_label.clone()),
            agent: node.agent.clone(),
            status: node_status_word(node.status).to_string(),
            output_name: node.output_name.clone(),
            error: node.error.clone(),
            acceptance: node
                .acceptance_status
                .as_ref()
                .map(|status| WorkflowChecklistAcceptance {
                    status: Some(status.clone()),
                    review_result: None,
                }),
            ..WorkflowChecklistStep::default()
        };
        let trace_entry = trace_lookup(&trace, &node.id);
        let lane = workflow_preflight_lane_for_runtime_key(
            preflight,
            &node.id,
            &[Some(phase_label.as_str())],
        );
        let item = apply_now(
            merge_node_step(node, &synthetic, &phase_label, trace_entry, lane),
            input.now,
        );
        add_item(&mut phases, &phase_label, item);
    }

    // Pass 2 — leftover steps.
    for (index, step) in steps.iter().enumerate() {
        if used_steps.contains(&index) {
            continue;
        }
        let key = key_text(step_key(step), &format!("step-{}", index + 1));
        let trace_entry = trace_lookup(&trace, &key);
        let lane = workflow_preflight_lane_for_runtime_key(
            preflight,
            &key,
            &[
                step.phase.as_deref(),
                trace_entry.and_then(|entry| entry.generated_lane_key.as_deref()),
                trace_entry.and_then(|entry| entry.phase.as_deref()),
            ],
        );
        let phase_label = key_text(
            lane.map(|lane| lane.key.as_str())
                .or(step.phase.as_deref())
                .or_else(|| trace_entry.and_then(|entry| entry.generated_lane_key.as_deref()))
                .or_else(|| trace_entry.and_then(|entry| entry.phase.as_deref())),
            "Workflow",
        );
        let label = step
            .label
            .as_deref()
            .or(step.key.as_deref())
            .or(step.agent.as_deref());
        let item = apply_now(
            step_item(step, index, &phase_label, &key, label, lane),
            input.now,
        );
        add_item(&mut phases, &phase_label, item);
    }

    // Pass 3 — leftover hosts: phase = the host's label, fallback "Host" (NOT "Workflow").
    for host in &host_by_id {
        let label = key_text(Some(&host.label), "Host");
        let item = host_item(host, &label, None);
        add_item(&mut phases, &label, item);
    }

    // Pass 4 — uncovered trace keys, at the CURRENT phase count per iteration (`phases.size`).
    for entry in &trace {
        if graph_keys.contains(entry.key.as_str())
            || step_keys.contains(entry.key.as_str())
            || host_keys.contains(&entry.key)
        {
            continue;
        }
        let lane = workflow_preflight_lane_for_runtime_key(
            preflight,
            &entry.key,
            &[
                entry.generated_lane_key.as_deref(),
                entry.phase.as_deref(),
            ],
        );
        let item = trace_item(entry, phases.len(), lane);
        let phase_label = item.phase.clone();
        add_item(&mut phases, &phase_label, item);
    }

    // Finalize.
    let mut finalized: Vec<WorkflowChecklistPhase> = phases
        .into_iter()
        .filter(|phase| !phase.items.is_empty())
        .collect();
    for phase in &mut finalized {
        let items = std::mem::take(&mut phase.items);
        phase.items = items
            .into_iter()
            .map(|item| apply_now(item, input.now))
            .collect();
        finalize_phase(phase);
    }
    let all: Vec<&WorkflowChecklistItem> = finalized
        .iter()
        .flat_map(|phase| phase.items.iter())
        .collect();
    let count_state = |state: WorkflowChecklistState| {
        all.iter().filter(|item| item.state == state).count()
    };
    let bottleneck = all
        .iter()
        .copied()
        .min_by_key(|item| priority(item))
        .filter(|item| item.state != WorkflowChecklistState::Complete)
        .cloned();
    WorkflowChecklistProjection {
        total: all.len(),
        done: count_state(WorkflowChecklistState::Complete),
        running: count_state(WorkflowChecklistState::Running),
        queued: count_state(WorkflowChecklistState::Queued),
        blocked: count_state(WorkflowChecklistState::Blocked),
        failed: count_state(WorkflowChecklistState::Failed),
        paused: count_state(WorkflowChecklistState::Paused),
        stopped: count_state(WorkflowChecklistState::Stopped),
        bottleneck,
        phases: finalized,
    }
}

/// pi `stateLabel` (`workflow-checklist.ts:387-389`, module-private): renames **only**
/// `running` → `"active"`; every other state prints its own word.
fn state_label(state: WorkflowChecklistState) -> &'static str {
    match state {
        WorkflowChecklistState::Running => "active",
        WorkflowChecklistState::Complete => "complete",
        WorkflowChecklistState::Queued => "queued",
        WorkflowChecklistState::Blocked => "blocked",
        WorkflowChecklistState::Failed => "failed",
        WorkflowChecklistState::Paused => "paused",
        WorkflowChecklistState::Stopped => "stopped",
    }
}

/// pi `formatWorkflowChecklistSummary` (`workflow-checklist.ts:391-394`): `""` when empty; else
/// `{done}/{total} done` plus every non-zero count, joined ` · `.
#[must_use]
pub fn format_workflow_checklist_summary(projection: &WorkflowChecklistProjection) -> String {
    if projection.total == 0 {
        return String::new();
    }
    let mut parts = vec![format!("{}/{} done", projection.done, projection.total)];
    for (count, word) in [
        (projection.running, "active"),
        (projection.queued, "queued"),
        (projection.blocked, "blocked"),
        (projection.failed, "failed"),
        (projection.paused, "paused"),
        (projection.stopped, "stopped"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {word}"));
        }
    }
    parts.join(" · ")
}

/// pi `formatWorkflowChecklistPhase` (`workflow-checklist.ts:396-399`): the same counts, except
/// `done` appears only when `total > 1 && done > 0`; bare `{label}` when no count qualifies.
#[must_use]
pub fn format_workflow_checklist_phase(phase: &WorkflowChecklistPhase) -> String {
    let mut counts: Vec<String> = Vec::new();
    if phase.total > 1 && phase.done > 0 {
        counts.push(format!("{} done", phase.done));
    }
    for (count, word) in [
        (phase.running, "active"),
        (phase.queued, "queued"),
        (phase.blocked, "blocked"),
        (phase.failed, "failed"),
        (phase.paused, "paused"),
        (phase.stopped, "stopped"),
    ] {
        if count > 0 {
            counts.push(format!("{count} {word}"));
        }
    }
    if counts.is_empty() {
        phase.label.clone()
    } else {
        format!("{} {}", phase.label, counts.join(" · "))
    }
}

/// The shared identity of the two item formatters: `[label, agent if != label]` joined ` · `,
/// falling back to `item.key` when that join is empty.
fn item_identity(item: &WorkflowChecklistItem) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !item.label.is_empty() {
        parts.push(&item.label);
    }
    if let Some(agent) = &item.agent
        && agent != &item.label
    {
        parts.push(agent);
    }
    let joined = parts.join(" · ");
    if joined.is_empty() {
        item.key.clone()
    } else {
        joined
    }
}

/// Lowercase `Output:` at ASCII word boundaries — pi's `\bOutput:` → `output:` replacement in
/// both item formatters. A manual scan (this crate carries no `regex`): the match requires the
/// preceding character NOT to be an ASCII alphanumeric or underscore, so `NoOutput:` is
/// untouched.
fn lowercase_output_label(error: &str) -> String {
    const NEEDLE: &str = "Output:";
    fn is_word_char(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }
    let mut output = String::with_capacity(error.len());
    let mut previous: Option<char> = None;
    let mut index = 0usize;
    while index < error.len() {
        let Some(rest) = error.get(index..) else { break };
        if rest.starts_with(NEEDLE) && !previous.is_some_and(is_word_char) {
            output.push_str("output:");
            index += NEEDLE.len();
            previous = Some(':');
            continue;
        }
        let Some(c) = rest.chars().next() else { break };
        output.push(c);
        previous = Some(c);
        index += c.len_utf8();
    }
    output
}

/// pi `formatWorkflowChecklistBottleneck`'s options (`workflow-checklist.ts:401`).
///
/// pi `:404-405`: `options.includeOutput ?? true`, `options.includeError ?? true`. BOTH default
/// TRUE, so a `#[derive(Default)]` (which would give `false`) would invert the whole function —
/// hence the hand-written impl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BottleneckOptions {
    /// Render the `out:{name}` cell.
    pub include_output: bool,
    /// Render the `error:{text}` cell.
    pub include_error: bool,
}

impl Default for BottleneckOptions {
    fn default() -> Self {
        Self {
            include_output: true,
            include_error: true,
        }
    }
}

/// pi `formatWorkflowChecklistBottleneck` (`workflow-checklist.ts:401-408`). `None` for an absent
/// item (upstream's `undefined`); the projection has already guaranteed a present bottleneck is
/// not `Complete`.
#[must_use]
pub fn format_workflow_checklist_bottleneck(
    item: Option<&WorkflowChecklistItem>,
    options: &BottleneckOptions,
) -> Option<String> {
    let item = item?;
    let identity = item_identity(item);
    let mut details: Vec<String> = Vec::new();
    if let Some(context) = item.context {
        details.push(format!("({})", context.as_str()));
    }
    if let Some(tool) = &item.current_tool {
        let duration = item
            .duration_ms
            .map(|ms| format!(" {}", format_duration_text(ms)))
            .unwrap_or_default();
        details.push(format!("{tool}{duration}"));
    }
    if item.current_tool.is_none()
        && let Some(path) = &item.current_path
    {
        details.push(path.clone());
    }
    if item.current_tool.is_none()
        && let Some(ms) = item.duration_ms
    {
        details.push(format_duration_text(ms));
    }
    if let Some(tools) = item.tool_count {
        details.push(format!("{tools} tools"));
    }
    if options.include_output
        && let Some(name) = &item.output_name
    {
        details.push(format!("out:{name}"));
    }
    if options.include_error
        && let Some(error) = &item.error
    {
        details.push(format!("error:{}", lowercase_output_label(error)));
    }
    let mut cells = vec![identity];
    cells.extend(details);
    Some(cells.join(" · "))
}

/// pi `formatWorkflowChecklistItem` (`workflow-checklist.ts:410-414`, module-private — NOT one of
/// the exported formatters, §1's export correction).
fn format_workflow_checklist_item(item: &WorkflowChecklistItem) -> String {
    let identity = item_identity(item);
    let mut details: Vec<String> = Vec::new();
    if item.kind.is_some()
        && let Some(kind) = item.monitor_kind
    {
        details.push(kind.as_str().to_string());
    }
    if let Some(provider) = &item.provider {
        details.push(format!("provider:{provider}"));
    }
    if let Some(role) = &item.role {
        details.push(format!("role:{role}"));
    }
    if let Some(target) = &item.target {
        details.push(target.clone());
    }
    if let Some(tool) = &item.current_tool {
        details.push(tool.clone());
    }
    if item.current_tool.is_none()
        && let Some(path) = &item.current_path
    {
        details.push(path.clone());
    }
    if let Some(ms) = item.duration_ms {
        details.push(format_duration_text(ms));
    }
    if let Some(tools) = item.tool_count {
        details.push(format!("{tools} tools"));
    }
    if let Some(name) = &item.output_name {
        details.push(format!("out:{name}"));
    }
    if let Some(path) = &item.report_path {
        details.push(format!("out:{path}"));
    }
    if item.stale == Some(true) {
        details.push("stale".to_string());
    }
    if let Some(code) = &item.reason_code {
        details.push(format!("reason:{code}"));
    }
    if let Some(error) = &item.error {
        details.push(format!("error:{}", lowercase_output_label(error)));
    }
    let context = item
        .context
        .map(|context| format!(" ({})", context.as_str()))
        .unwrap_or_default();
    let state = if item.state == WorkflowChecklistState::Complete {
        String::new()
    } else {
        format!(" · {}", state_label(item.state))
    };
    let detail_suffix = if details.is_empty() {
        String::new()
    } else {
        format!(" · {}", details.join(" · "))
    };
    format!("{identity}{context}{state}{detail_suffix}")
}

/// pi `formatDurationText` (`workflow-checklist.ts:416-419`, module-private):
/// `seconds = max(0, round(ms / 1000))`; under 60 ⇒ `{n}s`, else `{m}m {s}s`.
fn format_duration_text(ms: f64) -> String {
    let seconds = (ms / 1000.0).round().max(0.0) as u64;
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

/// The six state markers of pi `formatWorkflowChecklistText` (`:424`, `:428`) — one exhaustive
/// map for both the phase and item rows.
fn state_marker(state: WorkflowChecklistState) -> &'static str {
    match state {
        WorkflowChecklistState::Complete => "✓",
        WorkflowChecklistState::Running => "⠼",
        WorkflowChecklistState::Blocked => "!",
        WorkflowChecklistState::Failed => "✗",
        WorkflowChecklistState::Paused | WorkflowChecklistState::Stopped => "■",
        WorkflowChecklistState::Queued => "◦",
    }
}

/// pi `formatWorkflowChecklistText` (`workflow-checklist.ts:421-436`): a **line vector**, not a
/// joined string (§0.13) — every caller pushes the lines into a larger buffer. `indent` is
/// positional (pi defaults it to `""`); `include_items` is pi's `options.includeItems`, which
/// defaults to `true`.
///
/// The subtle coupling at `:433`: `include_items == false` is the ONLY thing that turns the
/// bottleneck's error on — when items are listed the error is already visible on its own row.
/// `include_output` is NOT passed at that call site, so it takes its [`BottleneckOptions`]
/// default — `true` — which is exactly why that `Default` impl cannot be derived.
#[must_use]
pub fn format_workflow_checklist_text(
    projection: &WorkflowChecklistProjection,
    indent: &str,
    include_items: bool,
) -> Vec<String> {
    if projection.total == 0 {
        return Vec::new();
    }
    let mut lines = vec![format!(
        "{indent}Workflow checklist: {}",
        format_workflow_checklist_summary(projection)
    )];
    for phase in &projection.phases {
        lines.push(format!(
            "{indent}  {} {}",
            state_marker(phase.state),
            format_workflow_checklist_phase(phase)
        ));
        if !include_items {
            continue;
        }
        for item in &phase.items {
            lines.push(format!(
                "{indent}    {} {}",
                state_marker(item.state),
                format_workflow_checklist_item(item)
            ));
        }
    }
    let bottleneck = format_workflow_checklist_bottleneck(
        projection.bottleneck.as_ref(),
        &BottleneckOptions {
            include_error: !include_items,
            ..BottleneckOptions::default()
        },
    );
    if let Some(bottleneck) = bottleneck.filter(|text| !text.is_empty()) {
        lines.push(format!("{indent}  bottleneck · {bottleneck}"));
    }
    lines
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
    use crate::background::{WorkflowPhase, WorkflowRunMode};
    use crate::workflows::host_step::{HostStepFreshness, HostStepKind, HostStepState, HostStepVersion};

    fn step(key: &str, status: &str) -> WorkflowChecklistStep {
        WorkflowChecklistStep {
            key: Some(key.to_string()),
            status: status.to_string(),
            ..WorkflowChecklistStep::default()
        }
    }

    fn node(id: &str, status: WorkflowNodeStatus) -> WorkflowGraphNode {
        WorkflowGraphNode {
            id: id.to_string(),
            kind: WorkflowNodeKind::Step,
            agent: None,
            phase: None,
            label: id.to_string(),
            status,
            flat_index: Some(0),
            step_index: Some(0),
            item_key: None,
            output_name: None,
            structured: None,
            acceptance_status: None,
            error: None,
            dynamic: None,
            children: None,
        }
    }

    fn graph(nodes: Vec<WorkflowGraphNode>, phases: Vec<WorkflowPhase>) -> WorkflowGraphSnapshot {
        WorkflowGraphSnapshot {
            run_id: "run".to_string(),
            mode: WorkflowRunMode::Workflow,
            phases,
            nodes,
            current_node_id: None,
        }
    }

    fn host(id: &str, state: HostStepState) -> HostStepNode {
        HostStepNode {
            version: HostStepVersion,
            kind: HostStepKind,
            monitor_kind: HostStepMonitorKind::Ci,
            id: id.to_string(),
            label: format!("{id} label"),
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
            updated_at: serde_json::Number::from(0),
            deadline_at: None,
        }
    }

    /// All seventeen aliases, `Queued` as the default, and the §0.13 round-trip: every canonical
    /// word maps to itself.
    #[test]
    fn checklist_state_aliases_and_round_trip() {
        for (word, expected) in [
            ("complete", WorkflowChecklistState::Complete),
            ("completed", WorkflowChecklistState::Complete),
            ("done", WorkflowChecklistState::Complete),
            ("pass", WorkflowChecklistState::Complete),
            ("accepted", WorkflowChecklistState::Complete),
            ("running", WorkflowChecklistState::Running),
            ("started", WorkflowChecklistState::Running),
            ("active", WorkflowChecklistState::Running),
            ("failed", WorkflowChecklistState::Failed),
            ("error", WorkflowChecklistState::Failed),
            ("fail", WorkflowChecklistState::Failed),
            ("blocked", WorkflowChecklistState::Blocked),
            ("rejected", WorkflowChecklistState::Blocked),
            ("partial", WorkflowChecklistState::Blocked),
            ("needs_attention", WorkflowChecklistState::Blocked),
            ("paused", WorkflowChecklistState::Paused),
            ("detached", WorkflowChecklistState::Paused),
            ("stopped", WorkflowChecklistState::Stopped),
            ("cancelled", WorkflowChecklistState::Stopped),
            ("canceled", WorkflowChecklistState::Stopped),
            ("mystery", WorkflowChecklistState::Queued),
        ] {
            assert_eq!(status_only_state(word), expected, "{word}");
        }
        // The round-trip: checklist_state accepts its own output words.
        for state in [
            WorkflowChecklistState::Complete,
            WorkflowChecklistState::Running,
            WorkflowChecklistState::Queued,
            WorkflowChecklistState::Blocked,
            WorkflowChecklistState::Failed,
            WorkflowChecklistState::Paused,
            WorkflowChecklistState::Stopped,
        ] {
            assert_eq!(status_only_state(state.as_status_word()), state);
        }
        // Case-insensitivity comes from normalizedStatus.
        assert_eq!(
            checklist_state(&StateSource {
                status: Some("Completed"),
                ..StateSource::default()
            }),
            WorkflowChecklistState::Complete
        );
    }

    /// The 11-way disjunction, including the `review.status ?? acceptance.reviewResult.status`
    /// fallback chain.
    #[test]
    fn explicit_blocked_covers_all_signals() {
        let base = StateSource {
            status: Some("running"),
            ..StateSource::default()
        };
        assert!(!explicit_blocked(&base));
        assert!(explicit_blocked(&StateSource {
            tool_budget_blocked: Some(true),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            turn_budget_exceeded: Some(true),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            activity_state: Some("needs_attention"),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            timed_out: Some(true),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            watchdog_phase: Some("stale"),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            stale: Some(true),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            verdict: Some("inconclusive"),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            acceptance_status: Some("Rejected"),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            acceptance_status: Some("blockers"),
            ..base
        }));
        assert!(explicit_blocked(&StateSource {
            review_status: Some("review-required"),
            ..base
        }));
        // The fallback arm: no review.status, but acceptance.reviewResult.status = blockers.
        assert!(explicit_blocked(&StateSource {
            acceptance_review_status: Some("blockers"),
            ..base
        }));
        // review.status PRESENT wins over the fallback: "approved" does not block.
        assert!(!explicit_blocked(&StateSource {
            review_status: Some("approved"),
            acceptance_review_status: Some("blockers"),
            ..base
        }));
    }

    /// `TERMINAL_STATES` — `blocked` is terminal here; `queued`/`running` are not.
    #[test]
    fn terminal_set_matches_upstream() {
        assert!(WorkflowChecklistState::Blocked.is_terminal());
        assert!(WorkflowChecklistState::Paused.is_terminal());
        assert!(!WorkflowChecklistState::Queued.is_terminal());
        assert!(!WorkflowChecklistState::Running.is_terminal());
    }

    /// §0.13's cumulative merge: field-wise overwrites, no clearing, `reused` keeps state,
    /// entries with NO operation are kept, `delivered`/`missed` are skipped.
    #[test]
    fn trace_sources_is_a_cumulative_field_merge() {
        let trace = vec![
            WorkflowChecklistTraceEntry {
                operation: Some("run".to_string()),
                key: "lane".to_string(),
                state: "started".to_string(),
                agent: Some("coder".to_string()),
                ..WorkflowChecklistTraceEntry::default()
            },
            // No operation — KEPT (the `!== undefined` guard); overwrites state, not agent.
            WorkflowChecklistTraceEntry {
                key: "lane".to_string(),
                state: "completed".to_string(),
                duration_ms: Some(1200.0),
                ..WorkflowChecklistTraceEntry::default()
            },
            // A reused entry updates fields but PRESERVES the accumulated state.
            WorkflowChecklistTraceEntry {
                operation: Some("run".to_string()),
                key: "lane".to_string(),
                state: "reused".to_string(),
                label: Some("Lane".to_string()),
                ..WorkflowChecklistTraceEntry::default()
            },
            // Steer operations and delivered/missed states are excluded.
            WorkflowChecklistTraceEntry {
                operation: Some("steer".to_string()),
                key: "lane".to_string(),
                state: "completed".to_string(),
                ..WorkflowChecklistTraceEntry::default()
            },
            WorkflowChecklistTraceEntry {
                operation: Some("run".to_string()),
                key: "lane".to_string(),
                state: "Delivered".to_string(),
                ..WorkflowChecklistTraceEntry::default()
            },
            WorkflowChecklistTraceEntry {
                operation: Some("run".to_string()),
                key: String::new(),
                state: "started".to_string(),
                ..WorkflowChecklistTraceEntry::default()
            },
        ];
        let merged = trace_sources(&trace);
        assert_eq!(merged.len(), 1);
        let row = &merged[0];
        assert_eq!(row.state, "completed", "reused preserved the accumulated state");
        assert_eq!(row.agent.as_deref(), Some("coder"), "absent fields do not clear");
        assert_eq!(row.duration_ms, Some(1200.0));
        assert_eq!(row.label.as_deref(), Some("Lane"), "reused still updates other fields");
    }

    /// The four-way status cascade.
    #[test]
    fn merge_node_status_is_a_four_way_cascade() {
        let trace = WorkflowChecklistTraceEntry {
            key: "k".to_string(),
            state: "started".to_string(),
            ..WorkflowChecklistTraceEntry::default()
        };
        // Arm 1: terminal node beats a non-terminal step.
        assert_eq!(
            merge_node_status(
                WorkflowChecklistState::Complete,
                WorkflowChecklistState::Running,
                Some(&trace),
                "running",
                WorkflowNodeStatus::Completed,
            ),
            WorkflowChecklistState::Complete
        );
        // Arm 2: live trace word, started ⇒ running.
        assert_eq!(
            merge_node_status(
                WorkflowChecklistState::Queued,
                WorkflowChecklistState::Queued,
                Some(&trace),
                "pending",
                WorkflowNodeStatus::Pending,
            ),
            WorkflowChecklistState::Running
        );
        // Arm 3: pending step yields to a non-pending node.
        assert_eq!(
            merge_node_status(
                WorkflowChecklistState::Queued,
                WorkflowChecklistState::Queued,
                None,
                "pending",
                WorkflowNodeStatus::Running,
            ),
            WorkflowChecklistState::Running
        );
        // Arm 4: the step's own status.
        assert_eq!(
            merge_node_status(
                WorkflowChecklistState::Queued,
                WorkflowChecklistState::Complete,
                Some(&trace),
                "accepted",
                WorkflowNodeStatus::Running,
            ),
            WorkflowChecklistState::Complete
        );
    }

    /// `duration`/`applyNow`: explicit wins, running uses `now`, `stepItem` never computes a live
    /// duration (its `now` is `None`), and `apply_now` is idempotent.
    #[test]
    fn duration_and_apply_now() {
        assert_eq!(
            duration_of(Some(-5.0), None, None, None, WorkflowChecklistState::Queued),
            Some(0.0)
        );
        assert_eq!(
            duration_of(None, Some(100.0), Some(50.0), None, WorkflowChecklistState::Complete),
            Some(0.0),
            "end before start clamps at 0"
        );
        assert_eq!(
            duration_of(None, Some(100.0), None, Some(400.0), WorkflowChecklistState::Running),
            Some(300.0)
        );
        assert_eq!(
            duration_of(None, None, Some(50.0), Some(400.0), WorkflowChecklistState::Complete),
            None,
            "no startedAt ⇒ no duration"
        );
        let built = step_item(
            &WorkflowChecklistStep {
                started_at: Some(100.0),
                status: "running".to_string(),
                ..WorkflowChecklistStep::default()
            },
            0,
            "Workflow",
            "k",
            None,
            None,
        );
        assert_eq!(built.duration_ms, None, "stepItem passes now: None");
        let applied = apply_now(built, Some(350.0));
        assert_eq!(applied.duration_ms, Some(250.0));
        let reapplied = apply_now(applied.clone(), Some(9999.0));
        assert_eq!(reapplied.duration_ms, Some(250.0), "idempotent once set");
    }

    /// The four passes: a graph node merges its step, an empty parallel group is its own row, a
    /// leftover host lands under its label (fallback "Host"), and an uncovered trace key becomes
    /// a row.
    #[test]
    fn projection_runs_the_four_passes() {
        let mut group = node("step-1", WorkflowNodeStatus::Pending);
        group.kind = WorkflowNodeKind::ParallelGroup;
        group.children = Some(Vec::new());
        group.label = "Parallel task".to_string();
        let input = WorkflowChecklistInput {
            graph: Some(graph(
                vec![node("step-0", WorkflowNodeStatus::Running), group],
                vec![WorkflowPhase {
                    title: "Build".to_string(),
                    node_ids: vec!["step-0".to_string()],
                }],
            )),
            steps: vec![
                step("step-0", "running"),
                step("orphan", "queued"),
            ],
            host_steps: vec![host("ci.gate", HostStepState::Running)],
            preflight: None,
            trace: vec![WorkflowChecklistTraceEntry {
                operation: Some("run".to_string()),
                key: "trace.only".to_string(),
                state: "completed".to_string(),
                ..WorkflowChecklistTraceEntry::default()
            }],
            now: None,
        };
        let projection = project_workflow_checklist(&input);
        assert_eq!(projection.total, 5);
        let phase_labels: Vec<&str> = projection
            .phases
            .iter()
            .map(|phase| phase.label.as_str())
            .collect();
        assert_eq!(
            phase_labels,
            vec!["Build", "Parallel task", "Workflow", "ci.gate label"],
            "insertion order: seeded phase, then pass-1/2/3 phases (pass 4 reuses Workflow)"
        );
        let build = &projection.phases[0];
        assert_eq!(build.items.len(), 1);
        assert_eq!(build.items[0].state, WorkflowChecklistState::Running);
        // Pass 2's orphan step and pass 4's trace row both land in "Workflow".
        let workflow = &projection.phases[2];
        assert_eq!(workflow.items.len(), 2);
        assert_eq!(workflow.items[0].key, "orphan");
        assert_eq!(workflow.items[1].key, "trace.only");
        assert_eq!(
            workflow.items[1].state,
            WorkflowChecklistState::Complete
        );
        // Pass 3's host phase used the host label, not "Workflow".
        let host_phase = &projection.phases[3];
        assert_eq!(host_phase.items[0].kind, Some(WorkflowChecklistItemKind::Host));
        assert_eq!(host_phase.items[0].state, WorkflowChecklistState::Running);
        // The bottleneck is the FIRST lowest-priority item: both running items tie at 2; the
        // graph node was inserted first.
        let bottleneck = projection.bottleneck.as_ref().expect("bottleneck");
        assert_eq!(bottleneck.key, "step-0");
    }

    /// A host id matched by a graph node consumes the host (pass 1) and never reappears in pass
    /// 3; the checklist state folds host words (`done`+verdict, `cancelled`, `inconclusive`).
    #[test]
    fn host_nodes_consume_and_fold_states() {
        let mut done_host = host("gate", HostStepState::Done);
        done_host.verdict = Some(HostStepVerdict::Pass);
        let input = WorkflowChecklistInput {
            graph: Some(graph(vec![node("gate", WorkflowNodeStatus::Running)], Vec::new())),
            host_steps: vec![done_host],
            ..WorkflowChecklistInput::default()
        };
        let projection = project_workflow_checklist(&input);
        assert_eq!(projection.total, 1, "the host was consumed by its node");
        assert_eq!(projection.done, 1, "done+pass folds to complete");

        // Inconclusive verdict and stale freshness block.
        let mut inconclusive = host("x", HostStepState::Done);
        inconclusive.verdict = Some(HostStepVerdict::Inconclusive);
        let blocked = host_item(&inconclusive, "P", None);
        assert_eq!(blocked.state, WorkflowChecklistState::Blocked);
        let mut stale = host("y", HostStepState::Done);
        stale.freshness = Some(HostStepFreshness {
            expected_ref: "main".to_string(),
            observed_ref: None,
            stale: Some(true),
        });
        let item = host_item(&stale, "P", None);
        assert_eq!(item.state, WorkflowChecklistState::Blocked);
        assert_eq!(item.stale, Some(true));
        assert_eq!(
            host_item(&host("z", HostStepState::Cancelled), "P", None).state,
            WorkflowChecklistState::Stopped
        );
    }

    /// The four public formatters plus the private markers: character-exact rows.
    #[test]
    fn formatters_render_character_exact() {
        let input = WorkflowChecklistInput {
            steps: vec![
                WorkflowChecklistStep {
                    key: Some("build".to_string()),
                    status: "complete".to_string(),
                    agent: Some("coder".to_string()),
                    ..WorkflowChecklistStep::default()
                },
                WorkflowChecklistStep {
                    key: Some("test".to_string()),
                    status: "running".to_string(),
                    phase: Some("build".to_string()),
                    current_tool: Some("bash".to_string()),
                    duration_ms: Some(65_000.0),
                    tool_count: Some(3.0),
                    output_name: Some("report".to_string()),
                    error: Some("NoOutput: kept, Output: lowered".to_string()),
                    ..WorkflowChecklistStep::default()
                },
            ],
            ..WorkflowChecklistInput::default()
        };
        let projection = project_workflow_checklist(&input);
        assert_eq!(
            format_workflow_checklist_summary(&projection),
            "1/2 done · 1 active"
        );
        assert_eq!(
            format_workflow_checklist_summary(&WorkflowChecklistProjection::default()),
            ""
        );

        let lines = format_workflow_checklist_text(&projection, "> ", true);
        assert_eq!(lines[0], "> Workflow checklist: 1/2 done · 1 active");
        assert_eq!(lines[1], ">   ✓ Workflow");
        assert_eq!(lines[2], ">     ✓ build · coder");
        assert_eq!(lines[3], ">   ⠼ build 1 active");
        // The ITEM formatter keeps tool and duration as separate cells; only the BOTTLENECK
        // formatter merges them (`${currentTool}${durationMs ? \` …\` : ""}`).
        assert_eq!(
            lines[4],
            ">     ⠼ test · active · bash · 1m 5s · 3 tools · out:report · \
             error:NoOutput: kept, output: lowered"
        );
        // includeItems=true ⇒ the bottleneck's error is OFF (already on its own row) while
        // include_output stays at its true default.
        assert_eq!(lines[5], ">   bottleneck · test · bash 1m 5s · 3 tools · out:report");

        // includeItems=false ⇒ item rows disappear and the bottleneck error turns ON.
        let collapsed = format_workflow_checklist_text(&projection, "", false);
        assert_eq!(collapsed.len(), 4);
        assert_eq!(
            collapsed[3],
            "  bottleneck · test · bash 1m 5s · 3 tools · out:report · \
             error:NoOutput: kept, output: lowered"
        );
        assert!(format_workflow_checklist_text(&WorkflowChecklistProjection::default(), "", true)
            .is_empty());
    }

    /// `stateLabel` renames only running; `formatDurationText` switches at 60s; the phase
    /// formatter's `done` gate needs `total > 1`.
    #[test]
    fn label_duration_and_phase_rules() {
        assert_eq!(state_label(WorkflowChecklistState::Running), "active");
        assert_eq!(state_label(WorkflowChecklistState::Stopped), "stopped");
        assert_eq!(format_duration_text(59_499.0), "59s");
        assert_eq!(format_duration_text(59_500.0), "1m 0s");
        assert_eq!(format_duration_text(-5.0), "0s");
        assert_eq!(format_duration_text(125_000.0), "2m 5s");

        let mut phase = new_phase("Solo");
        phase.items.push(host_item(&host("a", HostStepState::Done), "Solo", None));
        finalize_phase(&mut phase);
        assert_eq!(
            format_workflow_checklist_phase(&phase),
            "Solo",
            "done is hidden when total <= 1"
        );
        phase
            .items
            .push(host_item(&host("b", HostStepState::Done), "Solo", None));
        finalize_phase(&mut phase);
        assert_eq!(format_workflow_checklist_phase(&phase), "Solo 2 done");
        assert!(phase.parallel);
        assert_eq!(phase.state, WorkflowChecklistState::Complete);
    }

    /// The word-boundary scan: `NoOutput:` untouched, a leading or post-boundary `Output:`
    /// lowered.
    #[test]
    fn output_label_lowering_respects_word_boundaries() {
        assert_eq!(lowercase_output_label("Output: x"), "output: x");
        assert_eq!(lowercase_output_label("NoOutput: x"), "NoOutput: x");
        assert_eq!(lowercase_output_label("9Output:"), "9Output:");
        assert_eq!(lowercase_output_label("a Output: b -Output:"), "a output: b -output:");
        assert_eq!(lowercase_output_label("é Output:"), "é output:");
    }

    /// The bottleneck formatter's option gates and identity fallback.
    #[test]
    fn bottleneck_formatter_gates_and_identity() {
        assert_eq!(
            format_workflow_checklist_bottleneck(None, &BottleneckOptions::default()),
            None
        );
        let item = WorkflowChecklistItem {
            key: "k".to_string(),
            label: "Label".to_string(),
            phase: "P".to_string(),
            state: WorkflowChecklistState::Blocked,
            agent: Some("agent".to_string()),
            context: Some(WorkflowChecklistContext::Fork),
            started_at: None,
            duration_ms: Some(2_000.0),
            current_tool: None,
            current_tool_started_at: None,
            current_path: Some("src/x.rs".to_string()),
            tool_count: None,
            output_name: Some("out".to_string()),
            error: Some("boom".to_string()),
            preflight: None,
            kind: None,
            monitor_kind: None,
            provider: None,
            role: None,
            verdict: None,
            target: None,
            reason_code: None,
            stale: None,
            report_path: None,
        };
        assert_eq!(
            format_workflow_checklist_bottleneck(Some(&item), &BottleneckOptions::default())
                .expect("some"),
            "Label · agent · (fork) · src/x.rs · 2s · out:out · error:boom"
        );
        assert_eq!(
            format_workflow_checklist_bottleneck(
                Some(&item),
                &BottleneckOptions {
                    include_output: false,
                    include_error: false,
                }
            )
            .expect("some"),
            "Label · agent · (fork) · src/x.rs · 2s"
        );
        // agent == label collapses; empty identity falls back to the key.
        let mut same = item.clone();
        same.agent = Some("Label".to_string());
        assert!(
            format_workflow_checklist_bottleneck(Some(&same), &BottleneckOptions::default())
                .expect("some")
                .starts_with("Label · (fork)")
        );
        assert!(BottleneckOptions::default().include_output);
        assert!(BottleneckOptions::default().include_error);
    }

    /// Labels flow through `sanitize_display_text` + `truncate_display(MAX_TEXT)`; `text` falls
    /// back rather than returning empty.
    #[test]
    fn text_sanitizes_truncates_and_falls_back() {
        assert_eq!(text(Some("a\u{1b}[31m b"), None).as_deref(), Some("a b"));
        assert_eq!(text(Some("   "), Some("fb")).as_deref(), Some("fb"));
        assert_eq!(text(None, Some("fb")).as_deref(), Some("fb"));
        assert_eq!(text(None, None), None);
        let long = "x".repeat(400);
        assert_eq!(text(Some(&long), None).map(|t| t.len()), Some(MAX_TEXT));
        assert_eq!(key_text(Some(""), "fallback"), "fallback");
    }

    /// The preflight lane resolution reaches the checklist: pass 2 uses `lane.key` as the phase
    /// and carries the lane on the item.
    #[test]
    fn preflight_lane_names_the_phase() {
        let preflight = crate::workflows::normalize_workflow_preflight(Some(&serde_json::json!({
            "version": 1,
            "lanes": [{ "key": "lane" }],
        })))
        .expect("valid")
        .expect("present");
        let input = WorkflowChecklistInput {
            steps: vec![step("lane.child", "running")],
            preflight: Some(preflight),
            ..WorkflowChecklistInput::default()
        };
        let projection = project_workflow_checklist(&input);
        assert_eq!(projection.phases[0].label, "lane");
        assert_eq!(
            projection.phases[0].items[0]
                .preflight
                .as_ref()
                .map(|lane| lane.key.as_str()),
            Some("lane")
        );
    }
}
