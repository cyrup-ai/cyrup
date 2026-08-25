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
//!   (`runs/foreground/execution.ts:805-826`), which fires an `onUpdate({content, details:{mode,
//!   results, progress}})` on every `tool_execution_start`/`tool_execution_end`/`message_end`.
//!
//! - **C20/C21 — the render payloads.** [`SubagentUpdatePayload`] is the exact `ToolUpdate.details`
//!   wire shape `cyrup-tui` deserializes (it arrives as
//!   `AgentSessionEvent::ToolExecutionUpdate.partial_result.details`, `cyrup-session-svc`) to render
//!   the **inline subagent-result surface** (C20). [`AsyncJobSnapshot`]/[`AsyncJobsPayload`] is the
//!   wire shape for the **persistent async-jobs widget** (C21) — one row per tracked background run,
//!   populated crate-side from a polled `status.json` via [`AsyncJobSnapshot::from_run_status`].
//!   Both mirror pi's `Details`/`renderSubagentResult` input (`tui/render.ts:1678-2029`).
//!
//! # The pure render primitives already exist — the remaining outer-layer step is `cyrup-tui`-side
//!
//! The fold-to-aggregate render primitives in [`crate::tui::render`]
//! ([`render_run_header_line`],
//! [`render_progress_header`](crate::tui::render::render_progress_header),
//! [`render_background_region`]) are already written,
//! pure, and unit-tested. This module bridges the wire payloads to them:
//! [`render_inline_result`] renders a [`SubagentUpdatePayload`] (C20) and
//! [`render_async_jobs_widget`] renders `&[AsyncJobSnapshot]` (C21), both reusing those primitives.
//!
//! **C20 IS WIRED.** [`render_inline_result`] is driven by the extension's own
//! [`NativeExtension::render_result`](cyrup_ext::native::NativeExtension::render_result)
//! (`crate::extension`), which the host calls on `AgentSessionEvent::ToolExecutionEnd` for the
//! `subagent` tool once `init` has declared `register_tool_renderer("subagent")` —
//! `cyrup-tui/src/app.rs:4276-4296`, pi's `renderResult` seam
//! (`pi-subagents/src/extension/index.ts:569` @v0.43.0). The lines are converted to plain text
//! there (`tui::render::lines_to_plain_text`) because the renderer contract carries a serialized
//! widget tree across the boundary rather than live `ratatui` values.
//!
//! **C21 IS NOT, AND CANNOT BE FROM THIS CRATE.** [`render_async_jobs_widget`] renders pi's
//! PERSISTENT widget, which upstream installs with `ctx.ui.setWidget(WIDGET_KEY, …)`
//! (`tui/render.ts:1265-1273`). cyrup's extension surface has no such capability: neither
//! `cyrup_ext::native::InitApi` nor `HostCtx` exposes a widget slot, and the renderer contract
//! covers only tool rows and custom messages. So this function's only caller remains its own test,
//! and it stays that way until `cyrup-ext` grows a `set_widget` capability — a change outside this
//! crate. The `render_result` seam is deliberately NOT abused to fake it: upstream's own
//! `renderSubagentResult` sends an async start down the plain-text branch (`:1413-1423`), it does
//! not draw a jobs widget there.

use std::collections::VecDeque;

use cyrup_core::{Content, ToolUpdate};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde::{Deserialize, Serialize};

use crate::background::{ActivityState, RunId, RunMode, RunState, RunStatus};
use crate::exec::ndjson::{parse_line, SubagentEvent};
use crate::exec::SingleResult;
use crate::fork_context::ContextMode;
use crate::tui::render::{render_background_region, render_run_header_line};
use crate::tui::{RunSource, SubagentProgressSnapshot};

/// Bounded cap on a live progress fold's `recent_output` ring (pi slices `recentOutput` to a recent
/// window on every append, `runs/foreground/execution.ts:211-216`; the crate's own per-step
/// telemetry ring uses the same discipline). Oldest lines are evicted first once the cap is hit, so
/// a long-running child's fold can never grow without bound.
pub const RECENT_OUTPUT_CAP: usize = 20;

/// Bounded cap on a live progress fold's `recent_tools` ring — pi's `MAX_STREAMED_RECENT_TOOLS`
/// (`pi-subagents/src/shared/utils.ts:435`, applied by `boundStreamedRecentTools` at `:444-447`).
///
/// **[CYRUP-DELTA]** pi lets the LIVE `progress.recentTools` array grow without bound and slices it
/// to the last 32 entries only when a snapshot is taken (`snapshotProgress`, `execution.ts:230-237`).
/// This fold evicts at push time instead, which yields the identical 32-entry tail on every
/// snapshot while making a long, tool-heavy child's in-memory fold O(1) rather than O(tool calls).
/// The distinction is invisible on the wire: no consumer reads past the tail pi already slices to,
/// and a SETTLED snapshot empties the ring entirely
/// ([`LiveProgressSnapshot::compact_completed`]).
pub const RECENT_TOOLS_CAP: usize = 32;

// =================================================================================================
// LiveProgressStatus / LiveProgressSnapshot (the per-run progress the inline surface renders)
// =================================================================================================

/// The lifecycle phase a [`LiveProgressSnapshot`] represents — a 1:1 port of pi's
/// `AgentProgress["status"]` union (`pi-subagents/src/shared/types.ts:578` @v0.43.0,
/// `"pending" | "running" | "completed" | "failed" | "detached"`), including its exact wire
/// spellings. (The richer `Paused`/`Queued` BACKGROUND lifecycle lives on [`RunState`], which
/// [`AsyncJobSnapshot`] carries for the C21 widget; this enum is the FOREGROUND child's own.)
///
/// `Complete` serializes as `"completed"` (pi's spelling) rather than the Rust variant's own
/// lower-cased name — the variant keeps its original identifier so existing in-crate matches are
/// untouched, while the wire shape is pi's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LiveProgressStatus {
    /// Queued for execution; no child process has started streaming yet (pi `"pending"`, the
    /// initial state a not-yet-started parallel task carries).
    Pending,
    /// The child is still streaming NDJSON — the animated activity glyph applies.
    #[default]
    Running,
    /// The child exited cleanly (exit code 0).
    #[serde(rename = "completed")]
    Complete,
    /// The child exited non-zero, timed out, or errored.
    Failed,
    /// The child handed off to intercom coordination and is no longer this run's to settle
    /// (pi `progress.status = "detached"`, `runs/foreground/execution.ts:449`).
    Detached,
}

impl LiveProgressStatus {
    /// Map to the [`RunState`] the pure render primitives key their activity-glyph gate off
    /// (`render::is_actively_running` animates only [`RunState::Running`]). `Complete`/`Failed`
    /// map to their [`RunState`] namesakes so a settled run renders the static idle glyph;
    /// `Pending` is [`RunState::Queued`] and `Detached` is [`RunState::Paused`] (a detached child
    /// is awaiting an out-of-band reply, which is exactly the paused/idle glyph's meaning).
    #[must_use]
    pub fn to_run_state(self) -> RunState {
        match self {
            LiveProgressStatus::Pending => RunState::Queued,
            LiveProgressStatus::Running => RunState::Running,
            LiveProgressStatus::Complete => RunState::Complete,
            LiveProgressStatus::Failed => RunState::Failed,
            LiveProgressStatus::Detached => RunState::Paused,
        }
    }

    /// Whether this phase is pi's `"running"` — the ONE status
    /// [`LiveProgressSnapshot::compact_completed`] refuses to compact (pi
    /// `compactCompletedProgress`'s `if (progress.status === "running") return progress;`,
    /// `pi-subagents/src/shared/utils.ts:330`).
    #[must_use]
    pub fn is_running(self) -> bool {
        matches!(self, LiveProgressStatus::Running)
    }
}

/// One entry of [`LiveProgressSnapshot::recent_tools`] — a 1:1 port of pi's
/// `AgentProgress["recentTools"][number]` (`pi-subagents/src/shared/types.ts:587` @v0.43.0,
/// `{ tool: string; args: string; endMs: number }`), pushed once per `tool_execution_end`
/// (`runs/foreground/execution.ts:803-810`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentToolCall {
    /// The tool that just finished (pi `progress.currentTool` at `tool_execution_end` time).
    pub tool: String,
    /// The short argument preview captured when the call STARTED (pi `progress.currentToolArgs`,
    /// itself `extractToolArgsPreview(toolArgs)`, `execution.ts:794`); empty when the start carried
    /// no previewable argument.
    pub args: String,
    /// Wall-clock epoch-millis at which the call finished (pi `endMs: now`, `execution.ts:808`).
    pub end_ms: u64,
}

/// One run's progress snapshot — a port of pi's `AgentProgress` (`shared/types.ts:562-587`) as
/// produced by `snapshotProgress` (`runs/foreground/execution.ts:230-237`) for a LIVE update and
/// by `compactCompletedProgress` (`shared/utils.ts:329-345`) for a SETTLED one.
///
/// Two consumers, one shape (exactly as upstream):
///
/// - the C19/C20 live stream, where it rides [`SubagentUpdatePayload`] over the
///   [`cyrup_core::ToolUpdate`] `details` channel to `cyrup-tui`; and
/// - a settled run's [`crate::exec::SingleResult::progress`], populated only when the caller asked
///   for it via `includeProgress` (pi `progress: params.includeProgress ? allProgress : undefined`,
///   `runs/foreground/subagent-executor.ts:3444,3819` @v0.43.0), and always passed through
///   [`Self::compact_completed`] first.
///
/// Every field beyond the original renderer-facing six is `#[serde(default)]` and skipped when
/// empty/`None`, so a payload that carries none of them serializes byte-for-byte as it did before
/// they existed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveProgressSnapshot {
    /// This child's flat index within its run (pi `AgentProgress.index`, `shared/types.ts:563`) — `0` for
    /// a SINGLE run, the task position for a PARALLEL fan-out.
    #[serde(default)]
    pub index: u32,
    /// The fully-qualified persona name this run executes, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The run's lifecycle phase from the inline surface's point of view.
    pub status: LiveProgressStatus,
    /// pi `AgentProgress.activityState` (`shared/types.ts:566`): the live-control attention classification
    /// this child most recently transitioned to, when one was raised at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_state: Option<ActivityState>,
    /// The task text this child was launched with (pi `AgentProgress.task`, `shared/types.ts:567`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub task: String,
    /// The resolved skill names injected into this child's prompt (pi `AgentProgress.skills`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<Vec<String>>,
    /// The tool the child is currently invoking, if the most recent event started one and no
    /// `tool_execution_end` has cleared it yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tool: Option<String>,
    /// The bounded tail of finished tool calls (pi `AgentProgress.recentTools`). Always EMPTY on a
    /// settled snapshot — see [`Self::compact_completed`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_tools: Vec<RecentToolCall>,
    /// Number of `tool_execution_start` events observed so far.
    pub tool_count: u32,
    /// Number of assistant `message_end` turns observed so far (pi `progress.turnCount`).
    pub turn_count: u32,
    /// Cumulative `input + output` tokens observed so far (pi `progress.tokens`).
    pub tokens: u64,
    /// The resolved launch model for this child (pi `AgentProgress.model`, `shared/types.ts:580`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The resolved thinking level for this child (pi `AgentProgress.thinking`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// Split input-token total (pi `AgentProgress.inputTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Split output-token total (pi `AgentProgress.outputTokens`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Wall-clock milliseconds this child has been running (pi `AgentProgress.durationMs`, stamped
    /// at settle as `Date.now() - startTime`, `execution.ts:1177`).
    #[serde(default)]
    pub duration_ms: u64,
    /// The run's terminal error text, when it failed (pi `progress.error = result.error`,
    /// `execution.ts:1179`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The tool that was in flight when the run failed (pi `progress.failedTool =
    /// progress.currentTool`, `execution.ts:1181`, set ONLY when there was both an error and a
    /// tool in flight).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_tool: Option<String>,
    /// A bounded tail of the child's most recent textual output lines (assistant + tool-result
    /// text), oldest-first (pi `progress.recentOutput`). Always EMPTY on a settled snapshot — see
    /// [`Self::compact_completed`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_output: Vec<String>,
}

impl LiveProgressSnapshot {
    /// Derive the settled render entry for one [`SingleResult`] — what pi's `renderSingleCompact`
    /// reads directly off `r` when the result carries no `progress` of its own
    /// (`tui/render.ts:1275-1308` @v0.34.0: the glyph comes from `r.exitCode`/`r.detached`, the
    /// counters from `r.usage`/`r.toolCalls`).
    ///
    /// The status is pi's own terminal classification, in pi's own precedence order
    /// (`renderSingleCompact`'s `resultGlyph`/`resultStatusLine` inputs, and `runSinglePath`'s
    /// detached/interrupted/exit-code ladder, `subagent-executor.ts:3849-3869`): detached wins,
    /// then a zero exit is complete and anything else failed. `recent_tools`/`recent_output` are
    /// left empty exactly as [`Self::compact_completed`] leaves them on a settled snapshot.
    #[must_use]
    pub fn from_settled_result(result: &SingleResult) -> Self {
        let status = if result.detached || result.interrupted {
            LiveProgressStatus::Detached
        } else if result.exit_code == 0 {
            LiveProgressStatus::Complete
        } else {
            LiveProgressStatus::Failed
        };
        Self {
            agent: (!result.agent.is_empty()).then(|| result.agent.clone()),
            status,
            task: result.task.clone(),
            tool_count: u32::try_from(result.tool_calls.len()).unwrap_or(u32::MAX),
            tokens: result.usage.input.saturating_add(result.usage.output),
            model: result.model.as_ref().map(ToString::to_string),
            input_tokens: Some(result.usage.input),
            output_tokens: Some(result.usage.output),
            error: result.error.clone(),
            ..Self::default()
        }
    }

    /// pi `compactCompletedProgress` (`pi-subagents/src/shared/utils.ts:329-345`) — the ONLY shape
    /// a SETTLED run's progress is ever published in, and the reason a chatty child cannot inflate
    /// a returned result without bound.
    ///
    /// A still-`running` snapshot is returned untouched (pi's first line). Any other status is
    /// rebuilt from the eleven fields pi's literal names — `index`, `agent`, `status`,
    /// `activityState`, `task`, `skills`, `toolCount`, `tokens`, `durationMs`, `error`,
    /// `failedTool` — with `recentTools`/`recentOutput` reset to empty and EVERY other field
    /// dropped, because pi constructs a fresh object listing exactly those keys. So the per-run
    /// growth terms (the tool-history ring and the output tail) are gone, and what remains is a
    /// fixed-size record whose only unbounded member is the task text the CALLER supplied.
    ///
    /// Dropping `currentTool`/`turnCount`/`model`/`thinking`/`inputTokens`/`outputTokens` here is
    /// not an oversight — it is pi's own object literal, reproduced key-for-key. `failedTool`
    /// survives, which is how a failed run still names the tool that was in flight after
    /// `currentTool` itself is dropped.
    #[must_use]
    pub fn compact_completed(self) -> Self {
        if self.status.is_running() {
            return self;
        }
        Self {
            index: self.index,
            agent: self.agent,
            status: self.status,
            activity_state: self.activity_state,
            task: self.task,
            skills: self.skills,
            tool_count: self.tool_count,
            tokens: self.tokens,
            duration_ms: self.duration_ms,
            error: self.error,
            failed_tool: self.failed_tool,
            recent_tools: Vec::new(),
            recent_output: Vec::new(),
            // Every remaining key is absent from pi's literal.
            current_tool: None,
            turn_count: 0,
            model: None,
            thinking: None,
            input_tokens: None,
            output_tokens: None,
        }
    }
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
/// (`runs/foreground/execution.ts:830-1023`). Cheap and allocation-light; a
/// [`LiveProgressSnapshot`] is taken off it whenever a progress-relevant event fires.
///
/// Deliberately narrower than [`crate::exec::AgentProgress`] (which additionally retains full event
/// vectors for final-output extraction / completion-guard scanning): this fold keeps only the
/// renderer-facing counters + the bounded output tail, so installing it as a live sink adds no
/// meaningful overhead to a foreground run.
#[derive(Clone, Debug, Default)]
pub struct LiveProgressFold {
    agent: Option<String>,
    /// This child's flat index within its run (pi `progress.index`, seeded from `options.index`,
    /// `runs/foreground/execution.ts:358`).
    index: u32,
    /// The launch task text (pi `progress.task`, `execution.ts:361`).
    task: String,
    /// The resolved skill names (pi `progress.skills`, `execution.ts:362`).
    skills: Option<Vec<String>>,
    /// The resolved launch model/effort (pi `progress.model`/`progress.thinking`,
    /// `execution.ts:367-368`).
    model: Option<String>,
    thinking: Option<String>,
    current_tool: Option<String>,
    /// The argument preview captured at the current tool's START (pi `progress.currentToolArgs`,
    /// `execution.ts:794`), copied onto the `recent_tools` entry when it ends.
    current_tool_args: String,
    /// The bounded finished-tool ring (pi `progress.recentTools`), capped at [`RECENT_TOOLS_CAP`].
    recent_tools: VecDeque<RecentToolCall>,
    tool_count: u32,
    turn_count: u32,
    input_tokens: u64,
    output_tokens: u64,
    recent_output: VecDeque<String>,
    /// When this fold started, for `durationMs` (pi's `startTime` local, `execution.ts:404-411`).
    /// `None` in a `Default`-constructed fold, which reports a zero duration.
    started_at: Option<std::time::Instant>,
}

impl LiveProgressFold {
    /// Start a fold for `agent` (the persona name shown in the header), with empty counters and the
    /// duration clock started (pi's `startTime`, captured before the child spawns).
    #[must_use]
    pub fn new(agent: Option<String>) -> Self {
        Self {
            agent,
            started_at: Some(std::time::Instant::now()),
            ..Self::default()
        }
    }

    /// Seed the launch-time descriptive fields pi writes into `progress` at construction
    /// (`runs/foreground/execution.ts:425-442`) and never mutates thereafter: this child's flat
    /// `index`, its `task` text, its resolved `skills`, and its resolved `model`/`thinking`.
    /// Chained onto [`Self::new`] by callers that know them; callers that do not (the raw
    /// NDJSON-only folds in tests) simply leave them at their empty defaults.
    #[must_use]
    pub fn with_launch_context(
        mut self,
        index: u32,
        task: impl Into<String>,
        skills: Option<Vec<String>>,
        model: Option<String>,
        thinking: Option<String>,
    ) -> Self {
        self.index = index;
        self.task = task.into();
        self.skills = skills;
        self.model = model;
        self.thinking = thinking;
        self
    }

    /// Fold one raw NDJSON stdout line into this state. Returns `true` iff the line parsed to a
    /// progress-relevant event (`tool_execution_start`/`tool_execution_end`/assistant
    /// `message_end`) — the exact set pi fires `fireUpdate` on
    /// (`runs/foreground/execution.ts:562,608,626,660,693,718` @v0.34.0), so a caller emits a fresh
    /// [`SubagentUpdatePayload`] exactly when this returns `true` (never once per raw line).
    /// A non-JSON line, or a JSON event that carries no renderer-facing change, returns `false`.
    pub fn record_line(&mut self, raw: &str) -> bool {
        let Some(event) = parse_line(raw) else {
            return false;
        };
        match &event {
            SubagentEvent::ToolExecutionStart { tool_name, args, .. } => {
                self.tool_count = self.tool_count.saturating_add(1);
                self.current_tool = Some(tool_name.clone());
                // pi `progress.currentToolArgs = extractToolArgsPreview(toolArgs)`
                // (`runs/foreground/execution.ts:794`).
                self.current_tool_args =
                    crate::exec::tool_call_summary::extract_tool_args_preview(args);
                true
            }
            SubagentEvent::ToolExecutionEnd { result, .. } => {
                // pi pushes the finished call onto `recentTools` ONLY when a `currentTool` was in
                // flight (`execution.ts:804-810`), then clears both it and its args (`:811-812`).
                if let Some(tool) = self.current_tool.take() {
                    if self.recent_tools.len() >= RECENT_TOOLS_CAP {
                        self.recent_tools.pop_front();
                    }
                    self.recent_tools.push_back(RecentToolCall {
                        tool,
                        args: std::mem::take(&mut self.current_tool_args),
                        end_ms: u64::try_from(crate::time::now_epoch_millis())
                            .unwrap_or(0),
                    });
                }
                self.current_tool_args.clear();
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

    /// Take a [`LiveProgressSnapshot`] of the current fold at the given lifecycle `status` (pi
    /// `snapshotProgress`, `runs/foreground/execution.ts:230-237`). The returned snapshot is the
    /// FULL, still-running shape; a caller publishing a SETTLED run's progress must additionally
    /// run it through [`LiveProgressSnapshot::compact_completed`], exactly as pi's
    /// `compactForegroundDetails` does (`shared/utils.ts:414-421`).
    #[must_use]
    pub fn snapshot(&self, status: LiveProgressStatus) -> LiveProgressSnapshot {
        LiveProgressSnapshot {
            index: self.index,
            agent: self.agent.clone(),
            status,
            activity_state: None,
            task: self.task.clone(),
            skills: self.skills.clone(),
            current_tool: self.current_tool.clone(),
            recent_tools: self.recent_tools.iter().cloned().collect(),
            tool_count: self.tool_count,
            turn_count: self.turn_count,
            tokens: self.input_tokens.saturating_add(self.output_tokens),
            model: self.model.clone(),
            thinking: self.thinking.clone(),
            input_tokens: Some(self.input_tokens),
            output_tokens: Some(self.output_tokens),
            duration_ms: self.duration_ms(),
            error: None,
            failed_tool: None,
            recent_output: self.recent_output.iter().cloned().collect(),
        }
    }

    /// Milliseconds elapsed since [`Self::new`] started this fold's clock (pi `Date.now() -
    /// startTime`, `runs/foreground/execution.ts:1177`); `0` for a `Default`-constructed fold that
    /// never started one.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.started_at
            .map(|start| u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// Fold one PARENT-side attempt note (a model-fallback or startup-retry note) into this run's
    /// `recent_output` ring.
    ///
    /// pi seeds each attempt's `progress.recentOutput` from the accumulated `attemptNotes` when the
    /// attempt is constructed (`runs/foreground/execution.ts:432`) and streams that object live, so
    /// the note explaining a relaunch is visible for the whole relaunched attempt. This fold is
    /// driven by the child's NDJSON instead, and a parent-side note never appears there — so the
    /// executor hands it over directly (see [`crate::exec::LiveEventSink::emit_note`]).
    ///
    /// Deliberately touches ONLY `recent_output`: a note is not a turn, a tool call or a token, and
    /// folding it as one would corrupt every counter on the surface it is meant to explain.
    pub fn record_attempt_note(&mut self, note: &str) {
        self.push_output(note);
    }

    /// Append the non-empty lines of `text` to the bounded `recent_output` ring, evicting oldest
    /// first past [`RECENT_OUTPUT_CAP`] (pi `appendRecentOutput`,
    /// `runs/foreground/execution.ts:211-216`).
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
///
/// `pub(crate)` because [`crate::exec::AgentProgress::append_recent_output`] folds the SAME
/// extraction into its own `recent_output` ring — pi runs one `extractTextFromContent` over both
/// the assistant `message_end` content and the `tool_execution_end` result before appending
/// (`runs/foreground/execution.ts:651,670` @v0.34.0), so a second private copy here would be two
/// implementations of one upstream function drifting apart.
pub(crate) fn extract_event_text(value: &serde_json::Value) -> String {
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
    /// The run's identity (pi `Details.runId`, `shared/types.ts:506` — optional there too). Carried
    /// on the SETTLED result so a renderer/consumer can name the run; absent on a live update,
    /// where the run id is not yet meaningful to the row being drawn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
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
    /// (pi `createPlaceholderResult`'s `status: "running"` shape, `slash/slash-live-state.ts:51-76`).
    #[must_use]
    pub fn single_live(context: ContextMode, progress: LiveProgressSnapshot) -> Self {
        Self {
            mode: RunMode::Single,
            run_id: None,
            context,
            progress: vec![progress],
            results: Vec::new(),
            current_step_index: None,
            total_steps: None,
        }
    }

    /// A SINGLE-mode terminal update carrying both the settled [`SingleResult`] and a final
    /// progress snapshot (pi's `emitUpdateSnapshot` at run settle,
    /// `runs/foreground/execution.ts:806-820`).
    #[must_use]
    pub fn single_final(
        context: ContextMode,
        result: SingleResult,
        progress: LiveProgressSnapshot,
    ) -> Self {
        Self {
            mode: RunMode::Single,
            run_id: None,
            context,
            progress: vec![progress],
            results: vec![result],
            current_step_index: None,
            total_steps: None,
        }
    }

    /// The `content` text for the enclosing [`cyrup_core::ToolUpdate`]: the most recent output line
    /// while running, else `"(running…)"` (pi `fireUpdate`'s `output || "(running...)"`,
    /// `runs/foreground/execution.ts:822-827`).
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
/// pure primitives in [`crate::tui::render`]: one header line per rendered entry (activity glyph +
/// agent + `[fork]` badge, R-SA-109/110/111) followed by its compact dim stats line. `tick` drives
/// the activity glyph.
///
/// Which entries are rendered mirrors pi's `renderSubagentResult`, whose input is the whole
/// `Details` object (`tui/render.ts:1678-1712` @v0.43.0) and which reads the SETTLED run out of
/// `d.results[…]` — `renderSingleCompact(d, r, …)` takes `r` from `results`, and only reaches for
/// `r.progress` for the still-running detail lines (`:1275-1298`). So:
///
/// * a LIVE update (`progress` populated, `results` empty — the C19 stream) renders its progress
///   entries, and
/// * a SETTLED result (`results` populated) renders one entry per RESULT, derived by
///   [`LiveProgressSnapshot::from_settled_result`], preferring a matching live progress entry when
///   the payload carried one so a settled snapshot's real tool/turn counters survive.
///
/// Rendering only `progress` — which this function used to do — meant a settled tool result drew
/// NOTHING at all, because pi's own settle payload carries `results` and gates `progress` on the
/// caller's `includeProgress` flag (`subagent-executor.ts:3819`).
#[must_use]
pub fn render_inline_result(payload: &SubagentUpdatePayload, tick: usize) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    if payload.results.is_empty() {
        for entry in &payload.progress {
            let label = entry.agent.as_deref().unwrap_or("subagent");
            out.push(render_run_header_line(
                label,
                entry.status.to_run_state(),
                payload.context,
                tick,
            ));
            out.push(entry.stats_line());
        }
        return out;
    }
    for (index, result) in payload.results.iter().enumerate() {
        let entry = payload
            .progress
            .get(index)
            .filter(|p| !p.status.is_running())
            .cloned()
            .unwrap_or_else(|| LiveProgressSnapshot::from_settled_result(result));
        let label = if result.agent.is_empty() {
            entry.agent.clone().unwrap_or_else(|| "subagent".to_string())
        } else {
            result.agent.clone()
        };
        out.push(render_run_header_line(
            &label,
            entry.status.to_run_state(),
            payload.context,
            tick,
        ));
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
    /// pi `syncTopLevelCurrentTool`, `subagent-runner.ts:2453-2460`) and step position. `agent` and
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
            ..LiveProgressSnapshot::default()
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
            ..LiveProgressSnapshot::default()
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
