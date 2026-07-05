//! Typed, serializable render-payload shapes the host (`cyrup-tui`) consumes to drive the live
//! subagent surfaces (func-SA §4.6/§5.5; arch-SA §3.7; C19/C20/C21) — plus the small
//! NDJSON-folding accumulator the foreground `subagent` tool uses to build them as a child streams.
//!
//! # What this module is (crate-side of the outer layer)
//!
//! Two crate/host surfaces meet here, and this module owns the crate side of both:
//!
//! - **C19 — live foreground progress.** The foreground `subagent` tool call stays blocked while
//!   its child runs, but must not stay *silent*: as the child streams NDJSON on stdout, the tool
//!   folds each progress event ([`LiveProgressFold`]) into a [`LiveProgressSnapshot`], wraps it in a
//!   [`SubagentUpdatePayload`], and pushes it through the host [`cyrup_core::ToolUpdateSink`]
//!   (`extension.rs::run_foreground_streaming`). Mirrors pi's `emitUpdateSnapshot`/`fireUpdate`
//!   (`runs/foreground/execution.ts:478-499`), which fires an `onUpdate({content, details:{mode,
//!   results, progress}})` on every `tool_execution_start`/`tool_execution_end`/`message_end`.
//!
//! - **C20/C21 — the render payloads.** [`SubagentUpdatePayload`] is the exact `ToolUpdate.details`
//!   wire shape `cyrup-tui` deserializes (it arrives as
//!   `AgentSessionEvent::ToolExecutionUpdate.partial_result.details`, `cyrup-session-svc`) to render
//!   the **inline subagent-result surface** (C20). [`AsyncJobSnapshot`]/[`AsyncJobsPayload`] is the
//!   wire shape for the **persistent async-jobs widget** (C21) — one row per tracked background run,
//!   populated crate-side from a polled `status.json` via [`AsyncJobSnapshot::from_run_status`].
//!   Both mirror pi's `Details`/`renderSubagentResult` input (`tui/render.ts:1267-1301,940-1265`).
//!
//! # The pure render primitives already exist — the remaining outer-layer step is `cyrup-tui`-side
//!
//! The fold-to-aggregate render primitives in [`crate::tui::render`]
//! ([`render_run_header_line`](crate::tui::render::render_run_header_line),
//! [`render_progress_header`](crate::tui::render::render_progress_header),
//! [`render_background_region`](crate::tui::render::render_background_region)) are already written,
//! pure, and unit-tested. This module bridges the wire payloads to them:
//! [`render_inline_result`] renders a [`SubagentUpdatePayload`] (C20) and
//! [`render_async_jobs_widget`] renders `&[AsyncJobSnapshot]` (C21), both reusing those primitives.
//!
//! **REMAINING OUTER-LAYER STEP (owned by `cyrup-tui`, not this crate):** `cyrup-tui` must (a)
//! deserialize [`SubagentUpdatePayload`] from `AgentSessionEvent::ToolExecutionUpdate`'s
//! `partial_result.details` and paint [`render_inline_result`]'s lines into the inline tool-result
//! region on every update, and (b) subscribe the persistent async-jobs widget to the background
//! [`JobTracker`](crate::background::tracker::JobTracker) feed, build `&[AsyncJobSnapshot]` from
//! each poll (via [`AsyncJobSnapshot::from_run_status`]), and paint [`render_async_jobs_widget`].
//! `cyrup-ext-subagents` has ZERO dependency on `cyrup-tui` (arch-SA §1.1/§6.1), so it emits these
//! `ratatui::text::Line` values and typed payloads; whichever crate owns the live terminal paints
//! them. Until that host wiring lands, C20/C21 stay `outer-layer`.

use std::collections::VecDeque;

use cyrup_core::{Content, ToolUpdate};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::background::{RunId, RunMode, RunState, RunStatus};
use crate::exec::ndjson::{parse_line, SubagentEvent};
use crate::exec::SingleResult;
use crate::fork_context::ContextMode;
use crate::tui::render::{render_background_region, render_run_header_line};
use crate::tui::{RunSource, SubagentProgressSnapshot};

/// Bounded cap on a live progress fold's `recent_output` ring (pi slices `recentOutput` to a recent
/// window on every append, `runs/foreground/execution.ts:115-120`; the crate's own per-step
/// telemetry ring uses the same discipline). Oldest lines are evicted first once the cap is hit, so
/// a long-running child's fold can never grow without bound.
pub const RECENT_OUTPUT_CAP: usize = 20;

// =================================================================================================
// LiveProgressStatus / LiveProgressSnapshot (the per-run progress the inline surface renders)
// =================================================================================================

/// The lifecycle phase a [`LiveProgressSnapshot`] represents (pi `AgentProgress.status`,
/// `runs/foreground/execution.ts`). Deliberately the three phases a *foreground single run* can be
/// in from the inline surface's point of view — a still-streaming child is `Running`, a settled
/// child is `Complete` or `Failed`. (The richer `Paused`/`Queued` background lifecycle lives on
/// [`RunState`], which [`AsyncJobSnapshot`] carries for the C21 widget.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveProgressStatus {
    /// The child is still streaming NDJSON — the animated activity glyph applies.
    #[default]
    Running,
    /// The child exited cleanly (exit code 0).
    Complete,
    /// The child exited non-zero, timed out, or errored.
    Failed,
}

impl LiveProgressStatus {
    /// Map to the [`RunState`] the pure render primitives key their activity-glyph gate off
    /// (`render::is_actively_running` animates only [`RunState::Running`]). `Complete`/`Failed`
    /// map to their [`RunState`] namesakes so a settled run renders the static idle glyph.
    #[must_use]
    pub fn to_run_state(self) -> RunState {
        match self {
            LiveProgressStatus::Running => RunState::Running,
            LiveProgressStatus::Complete => RunState::Complete,
            LiveProgressStatus::Failed => RunState::Failed,
        }
    }
}

/// One run's live progress fold, reduced to the renderer-facing fields the inline surface shows
/// (pi's `snapshotProgress` output, `runs/foreground/execution.ts:134-141`): the agent, its
/// lifecycle phase, the tool it is currently running, cumulative tool/turn counts, cumulative
/// tokens (`input + output`, pi `progress.tokens`), and a bounded tail of recent output.
///
/// Serializable so it round-trips through [`SubagentUpdatePayload`] over the
/// [`cyrup_core::ToolUpdate`] `details` channel to `cyrup-tui`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProgressSnapshot {
    /// The fully-qualified persona name this run executes, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The run's lifecycle phase from the inline surface's point of view.
    pub status: LiveProgressStatus,
    /// The tool the child is currently invoking, if the most recent event started one and no
    /// `tool_execution_end` has cleared it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// Number of `tool_execution_start` events observed so far.
    pub tool_count: u32,
    /// Number of assistant `message_end` turns observed so far (pi `progress.turnCount`).
    pub turn_count: u32,
    /// Cumulative `input + output` tokens observed so far (pi `progress.tokens`).
    pub tokens: u64,
    /// A bounded tail of the child's most recent textual output lines (assistant + tool-result
    /// text), oldest-first (pi `progress.recentOutput`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_output: Vec<String>,
}

impl LiveProgressSnapshot {
    /// A single compact, dim stats line for the inline surface: the current tool (when running),
    /// the tool/turn counts, and the token total — the fields pi's `formatProgressStats`
    /// (`tui/render.ts`) folds onto the running/settled result row. Kept as a plain styled
    /// [`Line`] so the host can paint it directly beneath [`render_run_header_line`]'s header.
    #[must_use]
    pub fn stats_line(&self) -> Line<'static> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(tool) = &self.current_tool {
            parts.push(format!("⚒ {tool}"));
        }
        parts.push(format!("{} tools", self.tool_count));
        parts.push(format!("{} turns", self.turn_count));
        parts.push(format!("{} tokens", self.tokens));
        Line::from(vec![Span::styled(
            format!("  {}", parts.join(" · ")),
            Style::default().add_modifier(Modifier::DIM),
        )])
    }
}

// =================================================================================================
// LiveProgressFold (the NDJSON-stream accumulator behind the foreground live sink, C19)
// =================================================================================================

/// The in-memory accumulator the foreground `subagent` tool folds a child's raw NDJSON stdout lines
/// into as they stream (C19) — the crate-side half of pi's `processLine` progress fold
/// (`runs/foreground/execution.ts:501-586`). Cheap and allocation-light; a
/// [`LiveProgressSnapshot`] is taken off it whenever a progress-relevant event fires.
///
/// Deliberately narrower than [`crate::exec::AgentProgress`] (which additionally retains full event
/// vectors for final-output extraction / completion-guard scanning): this fold keeps only the
/// renderer-facing counters + the bounded output tail, so installing it as a live sink adds no
/// meaningful overhead to a foreground run.
#[derive(Clone, Debug, Default)]
pub struct LiveProgressFold {
    agent: Option<String>,
    current_tool: Option<String>,
    tool_count: u32,
    turn_count: u32,
    input_tokens: u64,
    output_tokens: u64,
    recent_output: VecDeque<String>,
}

impl LiveProgressFold {
    /// Start a fold for `agent` (the persona name shown in the header), with empty counters.
    #[must_use]
    pub fn new(agent: Option<String>) -> Self {
        Self { agent, ..Self::default() }
    }

    /// Fold one raw NDJSON stdout line into this state. Returns `true` iff the line parsed to a
    /// progress-relevant event (`tool_execution_start`/`tool_execution_end`/assistant
    /// `message_end`) — the exact set pi fires `fireUpdate` on
    /// (`runs/foreground/execution.ts:535,553,585`), so a caller emits a fresh
    /// [`SubagentUpdatePayload`] exactly when this returns `true` (never once per raw line).
    /// A non-JSON line, or a JSON event that carries no renderer-facing change, returns `false`.
    pub fn record_line(&mut self, raw: &str) -> bool {
        let Some(event) = parse_line(raw) else {
            return false;
        };
        match &event {
            SubagentEvent::ToolExecutionStart { tool_name, .. } => {
                self.tool_count = self.tool_count.saturating_add(1);
                self.current_tool = Some(tool_name.clone());
                true
            }
            SubagentEvent::ToolExecutionEnd { result, .. } => {
                self.current_tool = None;
                self.push_output(&extract_event_text(result));
                true
            }
            SubagentEvent::MessageEnd { message } => {
                let is_assistant = message.get("role").and_then(serde_json::Value::as_str)
                    == Some("assistant");
                if is_assistant {
                    self.turn_count = self.turn_count.saturating_add(1);
                    if let Some(usage) = event.assistant_usage() {
                        self.input_tokens = self.input_tokens.saturating_add(usage.input);
                        self.output_tokens = self.output_tokens.saturating_add(usage.output);
                    }
                    let text = message.get("content").map(extract_event_text).unwrap_or_default();
                    self.push_output(&text);
                }
                is_assistant
            }
            _ => false,
        }
    }

    /// Take a [`LiveProgressSnapshot`] of the current fold at the given lifecycle `status`.
    #[must_use]
    pub fn snapshot(&self, status: LiveProgressStatus) -> LiveProgressSnapshot {
        LiveProgressSnapshot {
            agent: self.agent.clone(),
            status,
            current_tool: self.current_tool.clone(),
            tool_count: self.tool_count,
            turn_count: self.turn_count,
            tokens: self.input_tokens.saturating_add(self.output_tokens),
            recent_output: self.recent_output.iter().cloned().collect(),
        }
    }

    /// Append the non-empty lines of `text` to the bounded `recent_output` ring, evicting oldest
    /// first past [`RECENT_OUTPUT_CAP`] (pi `appendRecentOutput`,
    /// `runs/foreground/execution.ts:115-120`).
    fn push_output(&mut self, text: &str) {
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if self.recent_output.len() >= RECENT_OUTPUT_CAP {
                self.recent_output.pop_front();
            }
            self.recent_output.push_back(trimmed.to_string());
        }
    }
}

/// Extract human-readable text from a message-`content`/tool-`result` JSON value (pi
/// `extractTextFromContent`): a bare string is itself; an array of parts joins each part's `text`;
/// an object falls back to its own `text` member. Any other shape yields `""`.
fn extract_event_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Object(_) => value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

// =================================================================================================
// SubagentUpdatePayload (the `ToolUpdate.details` wire shape — C19 stream + C20 inline surface)
// =================================================================================================

/// The serializable `details` payload the `subagent` tool attaches to every streamed
/// [`cyrup_core::ToolUpdate`] (C19) and to its final result — the exact wire shape `cyrup-tui`
/// deserializes from `AgentSessionEvent::ToolExecutionUpdate.partial_result.details` to render the
/// inline subagent-result surface (C20). Mirrors pi's `Details` object
/// (`tui/render.ts` `renderSubagentResult` input; `runs/foreground/execution.ts:483-491`):
/// `mode`/`context`/`results`/`progress`, plus the chain/parallel step-position fields the
/// multi-run renderer folds a step label off of.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentUpdatePayload {
    /// Which shape of run produced this update (single/parallel/chain).
    pub mode: RunMode,
    /// The run's *resolved* fork context (drives the `[fork]` badge, R-SA-110/111).
    #[serde(default)]
    pub context: ContextMode,
    /// One live progress entry per active run (one for SINGLE; per-task for PARALLEL/CHAIN).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub progress: Vec<LiveProgressSnapshot>,
    /// The settled per-run results, once available — empty while still running.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub results: Vec<SingleResult>,
    /// Zero-based index of the chain/parallel step currently active, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_index: Option<u32>,
    /// Total number of steps currently known, if applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<u32>,
}

impl SubagentUpdatePayload {
    /// A SINGLE-mode live update carrying one still-running progress snapshot and no settled result
    /// (pi `createPlaceholderResult`'s `status: "running"` shape, `slash/slash-live-state.ts:50`).
    #[must_use]
    pub fn single_live(context: ContextMode, progress: LiveProgressSnapshot) -> Self {
        Self {
            mode: RunMode::Single,
            context,
            progress: vec![progress],
            results: Vec::new(),
            current_step_index: None,
            total_steps: None,
        }
    }

    /// A SINGLE-mode terminal update carrying both the settled [`SingleResult`] and a final
    /// progress snapshot (pi's `emitUpdateSnapshot` at run settle,
    /// `runs/foreground/execution.ts:483-491`).
    #[must_use]
    pub fn single_final(
        context: ContextMode,
        result: SingleResult,
        progress: LiveProgressSnapshot,
    ) -> Self {
        Self {
            mode: RunMode::Single,
            context,
            progress: vec![progress],
            results: vec![result],
            current_step_index: None,
            total_steps: None,
        }
    }

    /// The `content` text for the enclosing [`cyrup_core::ToolUpdate`]: the most recent output line
    /// while running, else `"(running…)"` (pi `fireUpdate`'s `output || "(running...)"`,
    /// `runs/foreground/execution.ts:498`).
    #[must_use]
    pub fn content_text(&self) -> String {
        self.progress
            .last()
            .and_then(|p| p.recent_output.last().cloned())
            .unwrap_or_else(|| "(running…)".to_string())
    }

    /// Wrap this payload as a [`cyrup_core::ToolUpdate`] carrying `text` as content and `self`
    /// serialized under `details` (the shape `cyrup-agent` re-serializes into
    /// `partial_result.details` for `cyrup-tui`). A serialization failure degrades to no `details`
    /// rather than dropping the whole update — the content text still surfaces.
    #[must_use]
    pub fn into_tool_update(self, text: String) -> ToolUpdate {
        let details = serde_json::to_value(&self).ok();
        ToolUpdate { content: vec![Content::text(text)], details, terminate: None }
    }
}

/// Render the inline subagent-result surface (C20) for one [`SubagentUpdatePayload`], reusing the
/// pure primitives in [`crate::tui::render`]: one header line per progress entry (activity glyph +
/// agent + `[fork]` badge, R-SA-109/110/111) followed by its compact dim stats line. `tick` drives
/// the activity glyph. See the module doc for the remaining `cyrup-tui`-side painting step.
#[must_use]
pub fn render_inline_result(payload: &SubagentUpdatePayload, tick: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for entry in &payload.progress {
        let label = entry.agent.as_deref().unwrap_or("subagent");
        out.push(render_run_header_line(label, entry.status.to_run_state(), payload.context, tick));
        out.push(entry.stats_line());
    }
    out
}

// =================================================================================================
// AsyncJobSnapshot / AsyncJobsPayload (the persistent async-jobs widget wire shape — C21)
// =================================================================================================

/// One row of the persistent async-jobs widget (C21): a compact, serializable view of one tracked
/// background run, populated crate-side from that run's polled `status.json`
/// ([`AsyncJobSnapshot::from_run_status`]). The wire shape `cyrup-tui` deserializes to build the
/// widget's per-job rows (pi's `renderAsyncJobs`, `tui/render.ts:940-1265`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncJobSnapshot {
    /// This run's identity (names its `RunDir` on disk).
    pub run_id: RunId,
    /// The persona currently executing, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The run shape (single/parallel/chain).
    pub mode: RunMode,
    /// The run's current lifecycle state (drives the activity glyph and status text).
    pub state: RunState,
    /// The run's resolved fork context (drives the `[fork]` badge).
    #[serde(default)]
    pub context: ContextMode,
    /// Zero-based index of the step currently running, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_step_index: Option<u32>,
    /// Total number of steps currently known for a chain run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<u32>,
    /// The tool currently executing anywhere in the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// Total tool calls started across the run.
    pub tool_count: u32,
    /// Highest per-step turn count observed.
    pub turn_count: u32,
    /// `input + output` tokens summed across the run.
    pub tokens: u64,
    /// A bounded tail of the run's most recent output lines, when carried.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_output: Vec<String>,
}

impl AsyncJobSnapshot {
    /// Build one async-jobs row from a polled [`RunStatus`] (the crate-side of the C21 feed): reads
    /// the run's top-level activity roll-ups (`current_tool`/`tool_count`/`turn_count`/`total_tokens`,
    /// pi `syncTopLevelCurrentTool`, `subagent-runner.ts:1444-1514`) and step position. `agent` and
    /// `context` are supplied by the caller (the tracker knows the run's originating persona and
    /// resolved context, which `status.json` itself does not always carry).
    #[must_use]
    pub fn from_run_status(status: &RunStatus, agent: Option<String>, context: ContextMode) -> Self {
        let clamp_u32 = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
        let clamp_usize = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        Self {
            run_id: status.run_id.clone(),
            agent,
            mode: status.mode,
            state: status.state,
            context,
            current_step_index: status.current_step.map(clamp_usize),
            total_steps: status.chain_step_count.map(clamp_usize),
            current_tool: status.telemetry.current_tool.clone(),
            tool_count: status.telemetry.tool_count.map(clamp_u32).unwrap_or(0),
            turn_count: status.telemetry.turn_count.map(clamp_u32).unwrap_or(0),
            tokens: status.telemetry.total_tokens.as_ref().map(|t| t.total).unwrap_or(0),
            recent_output: Vec::new(),
        }
    }

    /// Bridge this row to a [`SubagentProgressSnapshot`] — the input the pure
    /// [`render_background_region`] consumes — so `cyrup-tui` can paint the C21 widget through the
    /// existing primitive without re-deriving a [`RunStatus`]. The renderer only reads
    /// `status.state` (for the activity glyph) plus the snapshot's own top-level fields, so a lean
    /// [`RunStatus`] carrying the correct `state`/step position is sufficient here.
    #[must_use]
    pub fn to_progress_snapshot(&self) -> SubagentProgressSnapshot {
        let status = RunStatus {
            state: self.state,
            current_step: self.current_step_index.map(|i| i as usize),
            chain_step_count: self.total_steps.map(|i| i as usize),
            ..RunStatus::queued(self.run_id.clone(), self.mode, None)
        };
        SubagentProgressSnapshot {
            run_id: self.run_id.clone(),
            mode: self.mode,
            context: self.context,
            source: RunSource::Async,
            status,
            current_agent: self.agent.clone(),
            current_step_index: self.current_step_index,
            total_steps: self.total_steps,
            current_tool: self.current_tool.clone(),
            turn_count: self.turn_count,
            tool_count: self.tool_count,
            recent_output: self.recent_output.last().cloned(),
            children: Vec::new(),
            last_activity_at: std::time::Instant::now(),
        }
    }
}

/// The full async-jobs widget feed (C21): every tracked background run's [`AsyncJobSnapshot`], in
/// the tracker's own order. The serializable payload `cyrup-tui` deserializes to render the widget.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsyncJobsPayload {
    /// One row per tracked background run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<AsyncJobSnapshot>,
}

/// Render the persistent async-jobs widget (C21) for a set of tracked background runs, reusing the
/// pure [`render_background_region`] primitive (bounded detail + fold-to-summary overflow,
/// R-SA-107/108). See the module doc for the remaining `cyrup-tui`-side feed-subscription +
/// painting step.
#[must_use]
pub fn render_async_jobs_widget(jobs: &[AsyncJobSnapshot], tick: usize) -> Vec<Line<'static>> {
    let snapshots: Vec<SubagentProgressSnapshot> =
        jobs.iter().map(AsyncJobSnapshot::to_progress_snapshot).collect();
    render_background_region(&snapshots, tick)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::tui::render::lines_to_plain_text;

    fn tool_start(id: &str, name: &str) -> String {
        serde_json::json!({"type": "tool_execution_start", "toolCallId": id, "toolName": name})
            .to_string()
    }

    fn tool_end(id: &str, name: &str, result: &str) -> String {
        serde_json::json!({
            "type": "tool_execution_end", "toolCallId": id, "toolName": name,
            "result": result, "isError": false
        })
        .to_string()
    }

    fn message_end(text: &str, input: u64, output: u64) -> String {
        serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "usage": {
                    "input": input, "output": output, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": input + output,
                    "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "stopReason": "stop"
            }
        })
        .to_string()
    }

    #[test]
    fn fold_counts_tools_turns_tokens_and_current_tool() {
        let mut fold = LiveProgressFold::new(Some("worker".to_string()));
        assert!(!fold.record_line("not json at all"));
        assert!(!fold.record_line(r#"{"type":"agent_start"}"#));
        assert!(fold.record_line(&tool_start("c1", "edit")));
        // After a start, the current tool is the started tool and the count incremented.
        let snap = fold.snapshot(LiveProgressStatus::Running);
        assert_eq!(snap.tool_count, 1);
        assert_eq!(snap.current_tool.as_deref(), Some("edit"));
        assert_eq!(snap.turn_count, 0);

        assert!(fold.record_line(&tool_end("c1", "edit", "wrote 3 lines")));
        assert!(fold.record_line(&message_end("Implemented the fix.", 42, 17)));

        let snap = fold.snapshot(LiveProgressStatus::Complete);
        assert_eq!(snap.tool_count, 1);
        assert_eq!(snap.turn_count, 1);
        assert_eq!(snap.tokens, 59, "tokens must be input + output");
        assert_eq!(snap.current_tool, None, "tool_execution_end clears the current tool");
        assert!(snap.recent_output.iter().any(|l| l.contains("Implemented the fix.")));
        assert!(snap.recent_output.iter().any(|l| l.contains("wrote 3 lines")));
        assert_eq!(snap.status, LiveProgressStatus::Complete);
    }

    #[test]
    fn fold_recent_output_ring_is_bounded() {
        let mut fold = LiveProgressFold::new(None);
        for i in 0..(RECENT_OUTPUT_CAP + 10) {
            fold.record_line(&message_end(&format!("line-{i}"), 1, 1));
        }
        let snap = fold.snapshot(LiveProgressStatus::Running);
        assert!(snap.recent_output.len() <= RECENT_OUTPUT_CAP, "recent_output must stay bounded");
    }

    #[test]
    fn payload_round_trips_through_tool_update_details() {
        let progress = LiveProgressSnapshot {
            agent: Some("scout".to_string()),
            status: LiveProgressStatus::Running,
            current_tool: Some("read".to_string()),
            tool_count: 3,
            turn_count: 2,
            tokens: 128,
            recent_output: vec!["some output".to_string()],
        };
        let payload = SubagentUpdatePayload::single_live(ContextMode::Fork, progress);
        let text = payload.content_text();
        assert_eq!(text, "some output");
        let update = payload.into_tool_update(text);
        let details = update.details.expect("details must serialize");
        let round: SubagentUpdatePayload =
            serde_json::from_value(details).expect("details must deserialize");
        assert_eq!(round.mode, RunMode::Single);
        assert_eq!(round.context, ContextMode::Fork);
        assert_eq!(round.progress.len(), 1);
        assert_eq!(round.progress[0].current_tool.as_deref(), Some("read"));
        assert_eq!(round.progress[0].tokens, 128);
    }

    #[test]
    fn inline_result_reuses_render_primitives_with_fork_badge() {
        let progress = LiveProgressSnapshot {
            agent: Some("researcher".to_string()),
            status: LiveProgressStatus::Running,
            current_tool: Some("web_read".to_string()),
            tool_count: 1,
            turn_count: 1,
            tokens: 10,
            recent_output: Vec::new(),
        };
        let payload = SubagentUpdatePayload::single_live(ContextMode::Fork, progress);
        let lines = render_inline_result(&payload, 0);
        let plain = lines_to_plain_text(&lines);
        assert!(plain.iter().any(|l| l.contains("researcher")), "header must name the agent: {plain:?}");
        assert!(plain.iter().any(|l| l.contains("[fork]")), "fork context must render the badge: {plain:?}");
        assert!(plain.iter().any(|l| l.contains("1 tools")), "stats line must render counts: {plain:?}");
    }

    #[test]
    fn async_job_snapshot_bridges_to_render_background_region() {
        let status = RunStatus::queued(RunId::new(), RunMode::Chain, Some(4321));
        let job = AsyncJobSnapshot::from_run_status(&status, Some("planner".to_string()), ContextMode::Fresh);
        assert_eq!(job.mode, RunMode::Chain);
        assert_eq!(job.agent.as_deref(), Some("planner"));
        let lines = render_async_jobs_widget(std::slice::from_ref(&job), 0);
        let plain = lines_to_plain_text(&lines);
        assert!(plain.iter().any(|l| l.contains("planner")), "widget row must name the agent: {plain:?}");
    }
}
