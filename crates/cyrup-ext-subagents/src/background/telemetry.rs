//! Live activity telemetry (func-SA §4.5; pi `subagent-runner.ts:1430-1581`, `shared/types.ts`)
//!
//! pi's detached runner folds each child NDJSON event into the run's `status.json` on a live cadence
//! (`updateStepFromChildEvent`, `subagent-runner.ts:2706-2861`) so a reader watching the file sees
//! `currentTool`/`recentTools`/`recentOutput`/`turnCount`/`toolCount`/`tokens`/`activityState`/
//! `lastActivityAt` per step, plus the top-level roll-ups those feed (`syncTopLevelCurrentTool`,
//! `statusPayload.toolCount`/`turnCount`/`totalTokens`). This section is the Rust port of that data
//! model, plus the pure per-event fold [`apply_child_event_to_step`] the detached runner
//! (`background/runner_main.rs`) drives from the child's real stdout events.
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::path::PathBuf;

use super::{StepStatus, WorkflowGraphSnapshot};
use crate::workflows::HostStepNode;

/// pi's `ActivityState` (`shared/types.ts:156`): a run/step that is idle-but-long-running or has
/// tripped a needs-attention control heuristic. Absent (`None` on the carrying field) is pi's
/// "neither" default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    /// The step is still actively producing events but has been running a long time (pi's
    /// `active_long_running`).
    ActiveLongRunning,
    /// The step tripped a needs-attention heuristic (repeated mutating-tool failures, idle past
    /// threshold — pi's `needs_attention`).
    NeedsAttention,
}

/// Accumulated per-step (and run-wide) token totals mirroring pi's `{ input, output, total }`
/// telemetry shape (`subagent-runner.ts:1502-1507`). Kept distinct from [`cyrup_core::Usage`] (the
/// richer, cost-bearing accounting record on [`StepStatus::usage`]) because pi's live telemetry
/// carries only these three integers on `step.tokens`/`statusPayload.totalTokens`, and reproducing
/// that exact on-the-wire shape is what a status-reading UI expects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    /// Input tokens accumulated so far.
    pub input: u64,
    /// Output tokens accumulated so far.
    pub output: u64,
    /// `input + output` accumulated so far (pi carries the redundant total explicitly).
    pub total: u64,
}

impl TokenTotals {
    /// Fold one turn's `(input, output)` into the running totals (pi's additive accumulation,
    /// `subagent-runner.ts:1502-1507`).
    pub(super) fn add(&mut self, input: u64, output: u64) {
        self.input = self.input.saturating_add(input);
        self.output = self.output.saturating_add(output);
        self.total = self.input.saturating_add(self.output);
    }
}

/// One entry in a step's recent-tool ring (pi's `recentTools` element,
/// `subagent-runner.ts:1448-1449`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentTool {
    /// The tool's name.
    pub tool: String,
    /// A compact preview of the tool's arguments (pi's `currentToolArgs`).
    pub args: String,
    /// Wall-clock epoch-millis the tool call ended.
    pub end_ms: i64,
}

/// Bounded cap on a step's `recent_tools`/`recent_output` rings so a long-running step's telemetry
/// can never grow `status.json` without limit (pi slices to a recent window per append,
/// `subagent-runner.ts:1878,1914` @v0.34.0).
const RECENT_RING_CAP: usize = 20;

/// The live per-step activity telemetry pi folds from child events (`subagent-runner.ts:2706-2861`),
/// carried on [`StepStatus`] via `#[serde(flatten)]` so these fields serialize at the SAME top level
/// of each `status.json` step object pi writes them at (`currentTool`, `recentTools`, … are direct
/// members of the step object, not a nested sub-object — `shared/types.ts:598-632`). Every field is
/// `#[serde(default)]` + skip-if-empty so (a) an older `status.json` written before this data model
/// existed still deserializes, and (b) a step that has produced no telemetry yet serializes to the
/// exact same lean object shape it did before this model was added (no test asserting an exact
/// pre-telemetry step shape regresses).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StepTelemetry {
    /// The tool currently executing in this step, if any (pi `step.currentTool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// A compact preview of the current tool's arguments (pi `step.currentToolArgs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_args: Option<String>,
    /// Epoch-millis the current tool started (pi `step.currentToolStartedAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool_started_at: Option<i64>,
    /// The filesystem path the current tool is operating on, when derivable (pi `step.currentPath`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_path: Option<String>,
    /// The most recent completed tool calls, oldest-first, capped at [`RECENT_RING_CAP`]
    /// (pi `step.recentTools`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_tools: Vec<RecentTool>,
    /// The most recent lines of assistant/tool output, oldest-first, capped at [`RECENT_RING_CAP`]
    /// (pi `step.recentOutput`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_output: Vec<String>,
    /// Count of tool calls started in this step (pi `step.toolCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    /// Count of assistant turns completed in this step (pi `step.turnCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// Accumulated token totals for this step (pi `step.tokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<TokenTotals>,
    /// The step's derived activity state (pi `step.activityState`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<ActivityState>,
    /// Epoch-millis of the most recent observed activity for this step (pi `step.lastActivityAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// The active model's thinking level for this step, when reported (pi `step.thinking`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// The file this step wrote its final output to, when a file-output handoff was configured
    /// (pi `step.outputFile`/the run-level `outputFile`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_file: Option<PathBuf>,
    /// G90: how many steering messages this step has ACCEPTED (pi `step.steerCount`,
    /// `subagent-runner.ts:1761`). Written by the runner's steer router, read by
    /// `run_status`'s per-step line — it is the only way a caller can tell that
    /// `action: "steer"` actually landed on a child rather than merely being queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_count: Option<u64>,
    /// G90: epoch-millis of this step's most recent accepted steer (pi `step.lastSteerAt`,
    /// `subagent-runner.ts:1762`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_steer_at: Option<i64>,
}

/// The run-wide activity roll-ups pi maintains on the top-level `statusPayload`
/// (`subagent-runner.ts:2085-2120`, `shared/types.ts:576-638`), carried on [`RunStatus`](crate::background::RunStatus) via
/// `#[serde(flatten)]` so they serialize at the SAME top level of `status.json` pi writes them at
/// (`status.currentTool`, `status.toolCount`, …). Same `#[serde(default)]` + skip-if-empty
/// backward-compatibility discipline as [`StepTelemetry`].
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTelemetry {
    /// The tool currently executing anywhere in the run (pi `syncTopLevelCurrentTool`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// Total tool calls started across every step (pi `statusPayload.toolCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<u64>,
    /// Highest per-step turn count observed (pi `statusPayload.turnCount`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_count: Option<u64>,
    /// Token totals summed across every step (pi `statusPayload.totalTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<TokenTotals>,
    /// The run's derived activity state (pi `statusPayload.activityState`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<ActivityState>,
    /// Epoch-millis of the most recent observed activity anywhere in the run
    /// (pi `statusPayload.lastActivityAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<i64>,
    /// The workflow-graph snapshot for this run (pi `statusPayload.workflowGraph`,
    /// `shared/types.ts:597`) — node ids, phases, group-status precedence, `currentNodeId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_graph: Option<WorkflowGraphSnapshot>,
    /// The run's host-owned monitor rows — pi `AsyncJobState.hostSteps` (`shared/types.ts:2034`),
    /// whose own doc reads *"Bounded host-owned CI/gate nodes loaded from the workflow status
    /// graph."* Serialized as `hostSteps`, the key upstream's readers already key on, and carried
    /// beside [`Self::workflow_graph`] because that is the pair every host-step consumer reads
    /// together (`tui/fleet-status.ts:434`'s `job.workflowGraph ?? job.hostSteps`).
    ///
    /// [CYRUP-DELTA] upstream does not STORE this list, it DERIVES it —
    /// `validHostStepNodes(status.workflowGraph)` (`async-status.ts:297`,
    /// `async-job-tracker.ts:469`, `run-status.ts:595`) walks the graph for nodes whose
    /// `kind === "host-step"` and lifts each one's `hostStep` payload back out. That derivation
    /// needs a graph node kind cyrup's [`crate::background::WorkflowNodeKind`] deliberately does
    /// not have (`background/workflow_graph.rs:42-53` enumerates four structural kinds, and a
    /// host monitor is a separate record in this tree, never a graph node). So the same list is
    /// carried DIRECTLY, under the same wire key, rather than a host-step graph kind being
    /// invented solely to be immediately flattened back out again by every reader.
    ///
    /// Written only through
    /// [`RunStatus::record_host_step`](crate::background::RunStatus::record_host_step), which owns
    /// the upsert-by-id rule and the [`HOST_STEP_MAX_COUNT`](crate::workflows::HOST_STEP_MAX_COUNT)
    /// bound.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_steps: Vec<HostStepNode>,
    /// G90: total steering messages accepted anywhere in this run (pi `statusPayload.steerCount`,
    /// `subagent-runner.ts:1766`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_count: Option<u64>,
    /// G90: epoch-millis of the run's most recent accepted steer (pi `statusPayload.lastSteerAt`,
    /// `subagent-runner.ts:1767`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_steer_at: Option<i64>,
}

/// Fold one child NDJSON [`crate::exec::ndjson::SubagentEvent`] into a step's live telemetry — the
/// Rust port of pi's `updateStepFromChildEvent` (`subagent-runner.ts:2706-2861`), scoped to the
/// per-step [`StepTelemetry`] fields (the top-level roll-ups are the caller's job via
/// [`RunStatus::sync_top_level_telemetry`](crate::background::RunStatus::sync_top_level_telemetry), mirroring pi's `syncTopLevelCurrentTool` living outside
/// the per-event fold). `now` is the caller's single epoch-millis reading for this event so every
/// field stamped by one event agrees on one timestamp (pi reads `Date.now()` once per call).
///
/// Only the telemetry-bearing event kinds are acted on; every other event kind (turn boundaries,
/// compaction, retries, unknown) simply bumps `last_activity_at`, exactly like pi's own fall-through.
pub fn apply_child_event_to_step(
    step: &mut StepStatus,
    event: &crate::exec::ndjson::SubagentEvent,
    now: i64,
) {
    use crate::exec::ndjson::SubagentEvent;
    match event {
        SubagentEvent::ToolExecutionStart {
            tool_name, args, ..
        } => {
            step.telemetry.tool_count =
                Some(step.telemetry.tool_count.unwrap_or(0).saturating_add(1));
            step.telemetry.current_tool = Some(tool_name.clone());
            step.telemetry.current_tool_args = Some(preview_tool_args(args));
            step.telemetry.current_tool_started_at = Some(now);
            step.telemetry.current_path = resolve_current_path(args);
        }
        SubagentEvent::ToolExecutionEnd { result, .. } => {
            if let Some(tool) = step.telemetry.current_tool.take() {
                let args = step.telemetry.current_tool_args.take().unwrap_or_default();
                push_bounded(
                    &mut step.telemetry.recent_tools,
                    RecentTool {
                        tool,
                        args,
                        end_ms: now,
                    },
                );
            }
            // A tool result carries text output pi folds into `recentOutput` (its own
            // `tool_result_end` branch, `subagent-runner.ts:1456-1460`; cyrup's wire union collapses
            // the result payload onto `tool_execution_end`, so the fold happens here).
            let text = extract_event_text(result);
            append_recent_output(&mut step.telemetry.recent_output, &text);
            step.telemetry.current_tool_args = None;
            step.telemetry.current_tool_started_at = None;
            step.telemetry.current_path = None;
        }
        SubagentEvent::MessageEnd { message }
            if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant") =>
        {
            let text = message
                .get("content")
                .map(extract_event_text)
                .unwrap_or_default();
            append_recent_output(&mut step.telemetry.recent_output, &text);
            step.telemetry.turn_count =
                Some(step.telemetry.turn_count.unwrap_or(0).saturating_add(1));
            if let Some(usage) = event.assistant_usage() {
                let mut tokens = step.telemetry.tokens.unwrap_or_default();
                tokens.add(usage.input, usage.output);
                step.telemetry.tokens = Some(tokens);
            }
        }
        _ => {}
    }
    step.telemetry.last_activity_at = Some(now);
}

/// UW-3 — the child-watchdog branch of pi's `updateStepFromChildEvent`
/// (`subagent-runner.ts:2711-2722` @v0.43.0; `:3016-3033` @v0.68.0), over one RAW child line (the
/// status event has no [`crate::exec::ndjson::SubagentEvent`] variant; it parses to `Unknown`).
///
/// `None` when `raw` is not a child-watchdog status event — the caller folds it as usual. `Some`
/// when it is, and the caller must NOT fold it further (upstream `return`s): `Some(true)` when it
/// was accepted into `step.watchdog` (the step's `lastActivityAt` is bumped here and the caller
/// bumps the run's, as upstream does on this path), `Some(false)` when it was rejected.
///
/// The identity is upstream's own for the runner: the run id, the step's agent and the step's FLAT
/// index — the same flat index the runner hands its child as `childIndex`
/// (`runner_main::executor`, `child_index: Some(ctx.step_slot.index())`), so it is the identity
/// the child was actually armed with.
pub fn apply_child_watchdog_line_to_step(
    step: &mut StepStatus,
    raw: &str,
    run_id: &str,
    flat_index: usize,
    now: i64,
) -> Option<bool> {
    use crate::watchdog::child_status::{
        ChildWatchdogIdentity, ChildWatchdogStatusEvent, accept_child_watchdog_event,
        is_child_watchdog_status_event,
    };
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    if !is_child_watchdog_status_event(&value) {
        return None;
    }
    let Ok(event) = serde_json::from_value::<ChildWatchdogStatusEvent>(value) else {
        return Some(false);
    };
    let identity = ChildWatchdogIdentity {
        run_id: Some(run_id.to_string()),
        agent: Some(step.agent.clone()),
        child_index: u64::try_from(flat_index).ok(),
    };
    let Some(next) = accept_child_watchdog_event(step.watchdog.as_ref(), &event, &identity) else {
        return Some(false);
    };
    step.watchdog = Some(next);
    step.telemetry.last_activity_at = Some(now);
    Some(true)
}

/// Push `item` onto a bounded recent-ring, dropping the oldest entry once [`RECENT_RING_CAP`] is
/// exceeded (pi keeps only a recent window, `subagent-runner.ts:1878,1914` @v0.34.0).
fn push_bounded<T>(ring: &mut Vec<T>, item: T) {
    ring.push(item);
    if ring.len() > RECENT_RING_CAP {
        let overflow = ring.len() - RECENT_RING_CAP;
        ring.drain(..overflow);
    }
}

/// Append the last few non-empty lines of `text` to a step's `recent_output` ring, bounded
/// (pi's `appendRecentStepOutput(step, text.split("\n").slice(-10))`, `subagent-runner.ts:1460`).
fn append_recent_output(ring: &mut Vec<String>, text: &str) {
    let tail: Vec<String> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .rev()
        .take(10)
        .map(str::to_string)
        .collect();
    for line in tail.into_iter().rev() {
        push_bounded(ring, line);
    }
}

/// Extract a compact preview of a tool call's arguments for `currentToolArgs`/a recent-tool entry
/// (pi's `extractToolArgsPreview`, `subagent-runner.ts:1440`). A best-effort compact JSON rendering
/// truncated so one pathological argument blob can never bloat `status.json`.
fn preview_tool_args(args: &serde_json::Value) -> String {
    const MAX: usize = 160;
    let rendered = match args {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if rendered.chars().count() > MAX {
        rendered.chars().take(MAX).collect::<String>() + "…"
    } else {
        rendered
    }
}

/// Derive the filesystem path a tool is operating on from its arguments, when one of pi's
/// well-known path-bearing argument keys is present (`resolveCurrentPath`,
/// `subagent-runner.ts:1437`). Returns `None` when no path-like argument is found.
fn resolve_current_path(args: &serde_json::Value) -> Option<String> {
    const KEYS: [&str; 6] = [
        "path",
        "file",
        "filePath",
        "file_path",
        "filename",
        "target",
    ];
    let object = args.as_object()?;
    KEYS.iter()
        .find_map(|key| object.get(*key).and_then(serde_json::Value::as_str))
        .map(str::to_string)
}

/// Extract human-readable text out of an opaque event payload `Value` — either a bare string, or a
/// `[{type:"text", text:"…"}, …]` content array (the `AssistantMessage.content`/tool-result shape),
/// mirroring pi's `extractTextFromContent` (`subagent-runner.ts`). Returns the empty string for any
/// shape carrying no text.
fn extract_event_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(serde_json::Value::as_str) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            out
        }
        _ => String::new(),
    }
}
