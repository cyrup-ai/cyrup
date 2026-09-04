//! The live per-attempt progress fold (R-SA-027/028) — [`AgentProgress`]'s streamed
//! `recent_output`/`tool_calls`/`usage` state and the [`ProgressSnapshotInput`] handoff used to
//! build a wire-shape progress snapshot. Split out of `exec/mod.rs`'s own "AgentProgress: the
//! live per-attempt fold" section.

use std::collections::VecDeque;

use cyrup_core::Usage;

use crate::exec::ndjson::SubagentEvent;
use crate::exec::tool_call_summary::ToolCallSummary;
use crate::exec::{RECENT_OUTPUT_CAP, RECENT_OUTPUT_TAIL_LINES, bound_output_line};

// ================================================================================================
// AgentProgress: the live per-attempt fold (R-SA-027/028)
// ================================================================================================

/// The live, in-memory progress state one attempt accumulates as its child's NDJSON stdout is
/// consumed (R-SA-027/028). This is the "still-running" shape architecture.md §4.3/R-SA-043
/// contrasts with [`crate::exec::SingleResult`]'s own compacted, terminal shape — never returned to
/// `run_sync`'s own caller directly; folded down into `SingleResult`'s summarized fields once the
/// attempt (and then the whole fallback ladder) settles.
#[derive(Debug, Clone, Default)]
pub struct AgentProgress {
    /// Running additive [`Usage`] total for THIS attempt alone (cross-attempt aggregation, which
    /// is additive across the whole ladder including failed attempts, is
    /// [`crate::exec::fallback::run_fallback_ladder`]'s own separate concern, R-SA-040) — every `MessageEnd`
    /// event's `usage` is folded in here as it is observed (R-SA-027).
    pub usage: Usage,
    /// Number of `ToolExecutionStart` events observed so far this attempt (R-SA-027).
    pub tool_count: u32,
    /// The most recently started tool's name, if any tool call has started and none more recent
    /// has superseded it (R-SA-027's "set `current_tool`").
    pub current_tool: Option<String>,
    /// Bounded ring buffer of the child's recent OUTPUT TEXT, oldest evicted first once
    /// [`RECENT_OUTPUT_CAP`] is exceeded (R-SA-028) — pi `progress.recentOutput`
    /// (`shared/types.ts:575`), seeded with the fallback ladder's attempt notes
    /// (`recentOutput: [...shared.attemptNotes]`, `runs/foreground/execution.ts:366`) and appended
    /// to by `appendRecentOutput` on each assistant `message_end` and each `tool_execution_end`.
    ///
    /// This holds EXTRACTED, human-readable text (`extractTextFromContent` over the message
    /// `content` / tool `result`), never the raw NDJSON envelope. That distinction is load-bearing
    /// rather than cosmetic: R-SA-028 describes "recent output" as a rendering/log concern, the
    /// only consumer that publishes it —
    /// [`crate::exec::SingleResult::progress`] via [`AgentProgress::snapshot`] — surfaces it to a caller as
    /// pi's `AgentProgress.recentOutput`, and a raw `{"type":"message_end","message":{...}}` line
    /// is both unrenderable and (before [`crate::exec::RECENT_OUTPUT_LINE_CHARS`]) an unbounded blob of the
    /// whole turn.
    pub recent_output: VecDeque<String>,
    /// Every `MessageEnd` event observed this attempt, in chronological (parse) order — the exact
    /// input [`crate::exec::output::extract_final_output`] (R-SA-029) needs, and what
    /// [`crate::exec::completion_guard::has_mutation_tool_call`]/[`crate::exec::completion_guard::evaluate_completion_mutation_guard`]
    /// (R-SA-034) scans alongside `tool_events` below.
    pub message_end_events: Vec<SubagentEvent>,
    /// Every `ToolExecutionEnd` event observed this attempt, in chronological order — feeds
    /// [`crate::exec::completion_guard::has_mutation_tool_call`] (R-SA-034) and the summarized `tool_calls`
    /// list [`crate::exec::SingleResult`] carries (R-SA-043).
    pub tool_end_events: Vec<SubagentEvent>,
    /// The full parsed transcript of every recognized event this attempt observed, in
    /// chronological order — needs more than the two narrower vectors above. `run_sync` reads it
    /// directly alongside `message_end_events`/`tool_end_events` for its R-SA-029/034 wiring.
    /// R-SA-030 (structured output) is deliberately NOT among its consumers any more: SUBA-S01's
    /// residual pass removed the transcript scan, because pi's structured value only ever travels
    /// through the child's capture file (`structured-output.ts:156-173`), never through prose.
    pub all_events: Vec<SubagentEvent>,
    /// The short argument preview captured when [`Self::current_tool`] STARTED (pi
    /// `progress.currentToolArgs = extractToolArgsPreview(toolArgs)`,
    /// `runs/foreground/execution.ts:794`), copied onto the [`Self::recent_tools`] entry that call
    /// produces when it ends and cleared alongside `current_tool` (`:811-812`).
    pub current_tool_args: String,
    /// Bounded ring of finished tool calls (pi `progress.recentTools`, `shared/types.ts:574`),
    /// oldest evicted first past [`crate::tui::events::RECENT_TOOLS_CAP`] — the same
    /// bound-at-push discipline (and the same rationale) as
    /// [`crate::tui::events::LiveProgressFold`]'s own ring.
    pub recent_tools: VecDeque<crate::tui::events::RecentToolCall>,
    /// When this attempt's clock started, for `durationMs` (pi's `startTime` local, captured before
    /// the child spawns and read back at `execution.ts:744`). `None` in a `Default`-constructed
    /// fold, which reports a zero duration.
    pub started_at: Option<std::time::Instant>,
}

impl AgentProgress {
    /// Fold one parsed [`SubagentEvent`] into this progress state (R-SA-027). Every `MessageEnd`
    /// event's usage is accumulated additively (never last-wins — mirrors
    /// [`crate::exec::fallback::add_usage`]'s own contract, restated here at the per-attempt granularity); every
    /// `ToolExecutionStart` increments `tool_count` and sets `current_tool`.
    ///
    /// Also feeds [`Self::recent_output`], on exactly pi's two append sites: an ASSISTANT
    /// `message_end`'s extracted content text (`appendRecentOutput(progress,
    /// assistantText.split("\n").slice(-10))`, `runs/foreground/execution.ts:651` @v0.34.0) and a
    /// finished tool call's extracted result text (`:670`). **[CYRUP-DELTA]** pi reads the result
    /// text off a separate `tool_result_end` event; cyrup's wire has no such event and carries the
    /// same payload on `ToolExecutionEnd.result` — the delta [`crate::exec::ndjson::SubagentEvent`]
    /// already documents, and the same one [`crate::tui::events::LiveProgressFold`] makes.
    pub fn record_event(&mut self, event: SubagentEvent) {
        if let Some(usage) = event.assistant_usage() {
            crate::exec::fallback::add_usage(&mut self.usage, &usage);
        }
        match &event {
            SubagentEvent::ToolExecutionStart {
                tool_name, args, ..
            } => {
                self.tool_count += 1;
                self.current_tool = Some(tool_name.clone());
                // pi `execution.ts:794`.
                self.current_tool_args =
                    crate::exec::tool_call_summary::extract_tool_args_preview(args);
            }
            SubagentEvent::MessageEnd { message } => {
                // pi `execution.ts:650-651` @v0.34.0 — ASSISTANT turns only; a user/tool-role
                // `message_end` contributes nothing to the rendered output tail.
                if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant") {
                    let text = message
                        .get("content")
                        .map(crate::tui::events::extract_event_text)
                        .unwrap_or_default();
                    self.append_recent_output(&text);
                }
                self.message_end_events.push(event.clone());
            }
            SubagentEvent::ToolExecutionEnd { result, .. } => {
                // pi `execution.ts:663-664,670` @v0.34.0.
                let result_text = crate::tui::events::extract_event_text(result);
                self.append_recent_output(&result_text);
                // pi pushes onto `recentTools` ONLY when a `currentTool` was in flight
                // (`execution.ts:804-810`), then clears it and its args (`:811-812`).
                if let Some(tool) = self.current_tool.take() {
                    if self.recent_tools.len() >= crate::tui::events::RECENT_TOOLS_CAP {
                        self.recent_tools.pop_front();
                    }
                    self.recent_tools
                        .push_back(crate::tui::events::RecentToolCall {
                            tool,
                            args: std::mem::take(&mut self.current_tool_args),
                            end_ms: u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0),
                        });
                }
                self.current_tool_args.clear();
                self.tool_end_events.push(event.clone());
            }
            _ => {}
        }
        self.all_events.push(event);
    }

    /// Number of ASSISTANT `message_end` events observed this attempt — pi's `progress.turnCount`,
    /// which it keeps in lockstep with `result.usage.turns` and increments only for an assistant
    /// message (`runs/foreground/execution.ts:825-827`).
    #[must_use]
    pub fn turn_count(&self) -> u32 {
        let turns = self
            .message_end_events
            .iter()
            .filter(|event| match event {
                SubagentEvent::MessageEnd { message } => {
                    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
                }
                _ => false,
            })
            .count();
        u32::try_from(turns).unwrap_or(u32::MAX)
    }

    /// Milliseconds elapsed since this attempt's clock started (pi `Date.now() - startTime`,
    /// `runs/foreground/execution.ts:1177`); `0` for a fold whose clock was never started.
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.started_at
            .map(|start| u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// Append one chunk of child output text to the bounded `recent_output` ring (R-SA-028) — a
    /// 1:1 port of pi's `appendRecentOutput` (`runs/foreground/execution.ts:211-217`) fused with
    /// the `.split("\n").slice(-10)` every call site applies to its argument (`:850,869`).
    ///
    /// Exactly pi's three rules, in pi's order: keep only the last
    /// [`RECENT_OUTPUT_TAIL_LINES`] lines of THIS chunk, drop the blank ones
    /// (`lines.filter((line) => line.trim())`), then evict from the front until the ring is back
    /// within [`RECENT_OUTPUT_CAP`]. Plus the one [CYRUP-DELTA] documented on
    /// [`crate::exec::RECENT_OUTPUT_LINE_CHARS`]: each surviving line is truncated to that many `char`s here
    /// rather than at snapshot time.
    ///
    /// pi keeps the ORIGINAL (untrimmed) line text and only *tests* `line.trim()` for emptiness,
    /// so leading indentation survives — reproduced here rather than pushing the trimmed form.
    pub fn append_recent_output(&mut self, text: &str) {
        let lines: Vec<&str> = text.lines().collect();
        let tail_start = lines.len().saturating_sub(RECENT_OUTPUT_TAIL_LINES);
        for line in lines.into_iter().skip(tail_start) {
            if line.trim().is_empty() {
                continue;
            }
            if self.recent_output.len() >= RECENT_OUTPUT_CAP {
                self.recent_output.pop_front();
            }
            self.recent_output.push_back(bound_output_line(line));
        }
    }

    /// Summarized `{text, expandedText}` tool-call previews observed this attempt (R-SA-043's
    /// compaction target), in chronological (request) order — one entry per `ToolExecutionStart`
    /// event, matching pi's `extractToolCallSummaries`, which walks the assistant messages'
    /// `toolCall` parts (`utils.ts:309-326`). Sourced from `ToolExecutionStart` (which carries the
    /// requested `args`), NOT `ToolExecutionEnd` (which carries only the result): a tool-call
    /// preview renders the arguments the model requested, and includes a call that started but
    /// never completed, exactly like pi's message-part walk. Repeats of the same tool are preserved
    /// (one entry per real call).
    #[must_use]
    pub fn summarized_tool_calls(&self) -> Vec<ToolCallSummary> {
        self.all_events
            .iter()
            .filter_map(|event| match event {
                SubagentEvent::ToolExecutionStart {
                    tool_name, args, ..
                } => Some(ToolCallSummary::from_call(tool_name, args)),
                _ => None,
            })
            .collect()
    }

    /// Project this fold into pi's `AgentProgress` wire shape
    /// ([`crate::tui::events::LiveProgressSnapshot`]) — the bridge `includeProgress` gates.
    ///
    /// pi needs no such projection because it has ONE object: `runSingleAttempt` builds a single
    /// mutable `progress` literal carrying both the launch context and the live counters
    /// (`runs/foreground/execution.ts:258-270` @v0.34.0), mutates its `status`/`durationMs`/
    /// `error`/`failedTool` at settle (`:907-913`), and hands that same object out as
    /// `result.progress` (`:271`). cyrup splits the two halves — the counters accumulate here, per
    /// ATTEMPT, while the launch context and the post-ladder settled facts are `run_sync` locals —
    /// so [`ProgressSnapshotInput`] carries the second half in and this method fuses them.
    ///
    /// The result is the FULL (still-uncompacted) shape, exactly like pi's object at `:271`. A
    /// caller publishing a settled run's progress must then run it through
    /// [`crate::tui::events::LiveProgressSnapshot::compact_completed`], which is what pi's
    /// `compactForegroundDetails` does one level up (`shared/utils.ts:414-421`).
    #[must_use]
    pub fn snapshot(
        &self,
        input: ProgressSnapshotInput<'_>,
    ) -> crate::tui::events::LiveProgressSnapshot {
        crate::tui::events::LiveProgressSnapshot {
            index: input.index,
            agent: Some(input.agent.to_string()),
            status: input.status,
            activity_state: input.activity_state,
            task: input.task.to_string(),
            skills: input.skills,
            // pi `progress.currentTool` survives into the returned object; `record_event` `take`s
            // it on `tool_execution_end`, so it is `Some` only for a call still in flight.
            current_tool: self.current_tool.clone(),
            recent_tools: self.recent_tools.iter().cloned().collect(),
            tool_count: self.tool_count,
            turn_count: self.turn_count(),
            // pi `progress.tokens = result.usage.input + result.usage.output`
            // (`execution.ts:646` @v0.34.0) — NOT the cache-read/write terms.
            tokens: self.usage.input.saturating_add(self.usage.output),
            model: input.model,
            thinking: input.thinking,
            input_tokens: Some(self.usage.input),
            output_tokens: Some(self.usage.output),
            duration_ms: self.duration_ms(),
            error: input.error.clone(),
            // pi `if (result.error) { …; if (progress.currentTool) progress.failedTool =
            // progress.currentTool; }` (`execution.ts:909-913` @v0.34.0) — BOTH conditions, so a
            // clean run names no failed tool and a failure with nothing in flight names none
            // either.
            failed_tool: input.error.as_ref().and_then(|_| self.current_tool.clone()),
            recent_output: self.recent_output.iter().cloned().collect(),
        }
    }
}

/// The half of pi's `progress` object that lives OUTSIDE [`AgentProgress`] in this port: the
/// launch-time descriptive fields pi writes into the literal at construction
/// (`runs/foreground/execution.ts:258-270` @v0.34.0) and the settled facts it assigns after the
/// child closes (`:907-913`). Every field is a `run_sync` local by the time
/// [`AgentProgress::snapshot`] is called.
///
/// A struct rather than nine positional arguments so the call site names each value (and so clippy's
/// `too_many_arguments` stays quiet).
pub struct ProgressSnapshotInput<'a> {
    /// pi `progress.index` ← `options.index ?? 0` (`execution.ts:259`); cyrup
    /// [`crate::exec::RunOptions::child_index`].
    pub index: u32,
    /// pi `progress.agent` ← `agent.name` (`:260`).
    pub agent: &'a str,
    /// pi `progress.task` ← the (post-fork-wrap) task text (`:262`).
    pub task: &'a str,
    /// pi `progress.skills` ← `shared.resolvedSkillNames` (`:263`) — the names that actually
    /// RESOLVED, `None` when none did (pi `resolvedSkills.length > 0 ? … : undefined`,
    /// `:1481` @HEAD).
    pub skills: Option<Vec<String>>,
    /// pi `progress.model` ← `modelArg` (`:267`), i.e. the winning model id WITH the thinking
    /// suffix [`crate::exec::apply_thinking_suffix`] appends.
    pub model: Option<String>,
    /// pi `progress.thinking` ← `resolvedThinking` (`:268`).
    pub thinking: Option<String>,
    /// pi's settled `progress.status` (`:907` / `:344` for a detach / `:828` for an interrupt).
    pub status: crate::tui::events::LiveProgressStatus,
    /// pi `progress.activityState`, owned by the live-control state machine and cleared on
    /// interrupt (`:832,854`); cyrup reads it back off the winning attempt's
    /// [`crate::exec::control::ControlMonitor`].
    pub activity_state: Option<crate::background::ActivityState>,
    /// pi `progress.error` ← the FINAL `result.error`, after every post-settlement gate
    /// (structured-output, completion guard, acceptance) has had its say (`:910`, plus the
    /// acceptance-failure assignment at `:1233-1234`).
    pub error: Option<String>,
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
    use crate::exec::RECENT_OUTPUT_LINE_CHARS;

    // ---- AgentProgress: R-SA-027/028 folding ----

    #[test]
    fn record_event_accumulates_usage_additively_across_multiple_message_end_events() {
        let mut progress = AgentProgress::default();
        let ev1 = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [],
                "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 15, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}
            }),
        };
        let ev2 = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [],
                "usage": {"input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 5, "cost": {"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}}
            }),
        };
        progress.record_event(ev1);
        progress.record_event(ev2);
        assert_eq!(progress.usage.input, 13);
        assert_eq!(progress.usage.output, 7);
        assert_eq!(progress.message_end_events.len(), 2);
    }

    #[test]
    fn record_event_increments_tool_count_and_sets_current_tool() {
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            args: serde_json::Value::Null,
        });
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c2".into(),
            tool_name: "edit".to_string(),
            args: serde_json::Value::Null,
        });
        assert_eq!(progress.tool_count, 2);
        assert_eq!(progress.current_tool.as_deref(), Some("edit"));
    }

    #[test]
    fn recent_output_buffer_is_capped_at_50_lines_oldest_evicted_first() {
        let mut progress = AgentProgress::default();
        for i in 0..(RECENT_OUTPUT_CAP + 10) {
            progress.append_recent_output(&format!("line-{i}"));
        }
        assert_eq!(progress.recent_output.len(), RECENT_OUTPUT_CAP);
        assert_eq!(
            progress.recent_output.front().map(String::as_str),
            Some("line-10")
        );
        let expected_last = format!("line-{}", RECENT_OUTPUT_CAP + 9);
        assert_eq!(
            progress.recent_output.back().map(String::as_str),
            Some(expected_last.as_str())
        );
    }

    #[test]
    fn append_recent_output_keeps_pis_last_ten_nonblank_lines_of_one_chunk() {
        // pi `appendRecentOutput(progress, text.split("\n").slice(-10))`
        // (`runs/foreground/execution.ts:651,670` @v0.34.0): one chunk contributes at most its
        // last ten lines, blank lines are dropped by `lines.filter((line) => line.trim())`, and
        // the ORIGINAL (untrimmed) text of each surviving line is what is stored.
        let mut progress = AgentProgress::default();
        let mut chunk = String::new();
        for i in 0..25 {
            chunk.push_str(&format!("l{i}\n"));
        }
        progress.append_recent_output(&chunk);
        assert_eq!(progress.recent_output.len(), RECENT_OUTPUT_TAIL_LINES);
        assert_eq!(
            progress.recent_output.front().map(String::as_str),
            Some("l15")
        );
        assert_eq!(
            progress.recent_output.back().map(String::as_str),
            Some("l24")
        );

        let mut blanks = AgentProgress::default();
        blanks.append_recent_output("a\n\n   \n  b  \n");
        assert_eq!(
            blanks.recent_output.iter().cloned().collect::<Vec<_>>(),
            vec!["a".to_string(), "  b  ".to_string()],
            "blank lines are dropped; surviving lines keep their own leading/trailing space"
        );
    }

    #[test]
    fn append_recent_output_truncates_one_enormous_line_to_pis_char_cap() {
        // pi `boundStreamedRecentOutput` (`shared/utils.ts:450-456`), applied at append time per
        // this crate's documented delta. Without it, one 10 MB tool result line would ride out on
        // `SingleResult::progress.recent_output` for an interrupt-paused run, whose `running`
        // status `compact_completed` deliberately refuses to empty.
        let mut progress = AgentProgress::default();
        let huge = "x".repeat(RECENT_OUTPUT_LINE_CHARS * 3);
        progress.append_recent_output(&huge);
        let stored = progress
            .recent_output
            .front()
            .cloned()
            .expect("one line must be stored");
        assert_eq!(
            stored.chars().count(),
            RECENT_OUTPUT_LINE_CHARS + "… [truncated]".chars().count()
        );
        assert!(stored.ends_with("… [truncated]"), "pi's suffix, verbatim");

        // A multi-byte line must be cut on a char boundary, not a byte one.
        let mut wide = AgentProgress::default();
        wide.append_recent_output(&"é".repeat(RECENT_OUTPUT_LINE_CHARS + 5));
        let stored = wide.recent_output.front().cloned().unwrap_or_default();
        assert_eq!(
            stored.chars().filter(|c| *c == 'é').count(),
            RECENT_OUTPUT_LINE_CHARS
        );

        // Exactly at the cap is NOT truncated (pi's `line.length > MAX` is strict).
        let mut exact = AgentProgress::default();
        exact.append_recent_output(&"y".repeat(RECENT_OUTPUT_LINE_CHARS));
        assert_eq!(
            exact.recent_output.front().map(String::len),
            Some(RECENT_OUTPUT_LINE_CHARS)
        );
    }

    #[test]
    fn record_event_appends_extracted_text_never_the_raw_ndjson_envelope() {
        // The regression this pins: `drive_attempt` used to push every RAW stdout line into
        // `recent_output`, so the field `SingleResult::progress` publishes as pi's `recentOutput`
        // held `{"type":"message_end",...}` JSON rather than the child's prose. pi appends
        // `extractTextFromContent(...)` at exactly two sites and nothing else.
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": [{ "type": "text", "text": "hello from the child" }]
            }),
        });
        progress.record_event(SubagentEvent::ToolExecutionEnd {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            result: serde_json::json!("tool said ok"),
            is_error: false,
        });
        // A non-assistant `message_end` contributes nothing (pi guards on `role === "assistant"`).
        progress.record_event(SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "user",
                "content": [{ "type": "text", "text": "user echo" }]
            }),
        });
        assert_eq!(
            progress.recent_output.iter().cloned().collect::<Vec<_>>(),
            vec![
                "hello from the child".to_string(),
                "tool said ok".to_string()
            ]
        );
        assert!(
            !progress
                .recent_output
                .iter()
                .any(|line| line.contains("\"type\"")),
            "no raw NDJSON envelope may reach recent_output: {:?}",
            progress.recent_output
        );
    }

    #[test]
    fn summarized_tool_calls_previews_each_started_calls_arguments_in_order() {
        // R-SA-043 / pi `extractToolCallSummaries`: one `{text, expandedText}` preview per
        // ToolExecutionStart (the request, which carries the args), in chronological order.
        let mut progress = AgentProgress::default();
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c1".into(),
            tool_name: "bash".to_string(),
            args: serde_json::json!({ "command": "ls -la" }),
        });
        progress.record_event(SubagentEvent::ToolExecutionStart {
            tool_call_id: "c2".into(),
            tool_name: "edit".to_string(),
            args: serde_json::json!({ "path": "/tmp/out.rs" }),
        });
        assert_eq!(
            progress.summarized_tool_calls(),
            vec![
                ToolCallSummary {
                    text: "$ ls -la".to_string(),
                    expanded_text: "$ ls -la".to_string(),
                },
                ToolCallSummary {
                    text: "edit /tmp/out.rs".to_string(),
                    expanded_text: "edit /tmp/out.rs".to_string(),
                },
            ]
        );
    }
}
