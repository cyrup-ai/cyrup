//! The `subagent-notify` message a completed background run produces: SUBA-090's `display`
//! predicate, the [`CompletionMessage`] payload, and the notify.ts content layout
//! (`Background task <status>: **<agent>**` header, summary, optional session line). Split out of
//! `background/watch.rs`; ports pi `runs/background/notify.ts:58-104,399-412`.

use super::classify::{ClassifiedOutcome, classify_outcome};
use super::results_watcher::LossReport;
use crate::background::ResultFile;
use crate::exec::SingleResult;
use crate::exec::output_state::SubagentOutputState;
use crate::exec::result_summary::extract_result_summary;

// =================================================================================================
// Completion notification (C6): format + deliver + delete (notify.ts / result-watcher.ts)
// =================================================================================================

/// SUBA-090 — pi's `display` predicate for a `subagent-notify` completion
/// (`v0.64.0:src/runs/background/notify.ts:402`):
///
/// ```ts
/// const display = details.some((detail) => detail.source === "foreground" || detail.status !== "completed" || detail.scheduleOrigin !== undefined);
/// ```
///
/// A plain successful background completion is injected as a NON-displayed context message — the
/// parent LLM still sees it and the turn still fires (R-SA-101), but nothing is drawn — and the
/// notice is rendered only when something needs attention: a failed/paused/stopped outcome, a
/// detached-foreground completion, or a schedule-launched run. The predicate is identical at
/// `v0.43.0:notify.ts:173` (minus the `scheduleOrigin` clause) and `v0.57.0:notify.ts:239`.
///
/// Cyrup's [`ResultFile`] carries no `source` (every completion this crate observes is an async
/// background run — detached-foreground completions are not ported), so the FIRST clause is still
/// vacuously false here.
///
/// SUBA-016 landed the THIRD: [`ResultFile::schedule_origin`] exists, and it is OR'd in below
/// exactly as this doc asked for. A scheduled run that succeeds IS displayed, because nobody was
/// watching when it fired — "your nightly sweep ran and it was fine" is the whole point of having
/// scheduled it, and a silent success is indistinguishable from a schedule that never fired.
#[must_use]
pub fn completion_notice_display(
    outcome: ClassifiedOutcome,
    schedule_origin: Option<&crate::background::ScheduleOrigin>,
) -> bool {
    outcome != ClassifiedOutcome::Completed || schedule_origin.is_some()
}

/// pi `scheduledCompletionTriggersTurn` (`notify.ts:362-365`):
/// `!(origin?.quiet === true && outcome === "completed")`.
///
/// This is what `quiet` MEANS, and it is narrower than it sounds: a quiet schedule's SUCCESSFUL
/// completion is still delivered and still displayed — it just does not wake the parent's turn.
/// Quiet is "do not interrupt me", never "do not tell me". A quiet schedule that FAILS wakes the
/// turn like any other, which is the half a looser reading would have silently dropped.
#[must_use]
pub fn scheduled_completion_triggers_turn(
    schedule_origin: Option<&crate::background::ScheduleOrigin>,
    outcome: ClassifiedOutcome,
) -> bool {
    !(schedule_origin.is_some_and(|origin| origin.quiet == Some(true))
        && outcome == ClassifiedOutcome::Completed)
}

/// The `subagent-notify` message a completed background run produces (pi `sendCompletion`,
/// `v0.64.0:src/runs/background/notify.ts:399-412`:
/// `pi.sendMessage({customType:"subagent-notify", content, display}, {triggerTurn: items.some((item) => item.triggerTurn)})`).
/// `custom_type` is fixed; `content` is built by [`format_completion_message`] to reproduce
/// notify.ts's status/summary/session-line layout character-for-character; `display` is upstream's
/// outcome-dependent predicate ([`completion_notice_display`], `notify.ts:402`); `trigger_turn` is
/// `true` for every completion cyrup can produce today (see the field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionMessage {
    /// Always `"subagent-notify"` (pi's `customType`).
    pub custom_type: String,
    /// The rendered notification body (status header, blank line, summary, optional session line).
    pub content: String,
    /// SUBA-090 — pi's `display` (`notify.ts:402` @v0.64.0): `false` for a plain successful
    /// background completion (the message is context for the LLM, not a rendered notice), `true`
    /// for a failed/paused/stopped outcome. Computed by [`completion_notice_display`] over
    /// [`classify_outcome`]; NOT a constant — the previous "Always `true` (pi's `display: true`)"
    /// claim was wrong at every upstream tag from v0.43.0 on.
    pub display: bool,
    /// `true` for every completion cyrup produces — pi's per-completion
    /// `triggerTurn: result.triggerTurn !== false` (`notify.ts:605` @v0.64.0), OR'd over the batch
    /// at `:409`; a `CompletionNotification` may carry `triggerTurn: false`, but cyrup's
    /// [`ResultFile`] has no such input, so the default (`true`) is the only value reachable. The
    /// completion re-enters the parent's normal turn/prompt path so the LLM sees and can act on the
    /// background result (R-SA-101) — this holds whether or not the notice is displayed.
    pub trigger_turn: bool,
    /// `ASYNC_NOTIFY_BUG_REPORT` F3.2 — `true` only for the ordinary value-carrying completion
    /// ([`format_completion_message`]). A delivery-FAILURE report —
    /// [`format_undeliverable_message`], [`format_missing_payload_message`] — carries information
    /// no `wait` could ever have surfaced, so it is never suppressed by the inline-answer ledger
    /// ([`crate::background::watch::InlineAnsweredSink`]).
    pub suppressible: bool,
}

/// pi's `"(no output)"` (`subagent-runner.ts:4744`, `notify.ts:242`). ONE constant, both sites.
const NO_OUTPUT: &str = "(no output)";

/// pi `CHILD_OUTPUT_PREVIEW_MAX_BYTES` (`notify.ts:132`).
const CHILD_OUTPUT_PREVIEW_MAX_BYTES: usize = 4 * 1024;

/// pi's terminal `summary` (`subagent-runner.ts:4744`), ported line for line.
///
/// # Every child gets a block, and every block names its agent
///
/// This IS the feature, not formatting. A fan-out's completion is the only channel carrying a
/// background child's text to the orchestrator; without the `{agent}:` prefix three children
/// arrive as three anonymous values it can neither attribute nor count.
///
/// Three rules that are easy to get wrong, all upstream's:
///
/// * **A child NEVER disappears.** An empty child renders `agent:\n(no output)`. The previous
///   implementation filtered empties out, so N children could render as fewer than N blocks.
/// * **`error` is a fallback ONLY for a non-zero exit** (`r.exitCode !== 0 ? r.error : undefined`).
///   A child that succeeded while carrying a stale `error` must still report its output.
/// * **`||` is JS string truthiness**: an EMPTY string falls through to the next arm. `Some("")`
///   must fall through too, so the test is "non-empty after trim", never `is_some`.
pub fn result_display_summary(result: &ResultFile) -> String {
    let total = result.results.len();
    result
        .results
        .iter()
        .enumerate()
        .map(|(index, child)| {
            format!(
                "{}{}:\n{}",
                child.agent,
                child_position(index, total),
                child_display_body(child)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// pi's `taskInfo` suffix (` (1/3)`, `notify.ts:481-484` from `taskIndex`/`totalTasks`).
///
/// Upstream's `CompletionNotification.taskIndex`/`totalTasks` (`notify.ts:103-104`) have NO
/// producer at pi HEAD `df26ebc8` — verified by grep; the fields are vestigial there. The shape is
/// exactly what a fan-out needs and is adopted here: three children of the SAME agent (`delegate`,
/// `delegate`, `delegate`) are otherwise indistinguishable, so the orchestrator cannot tell three
/// results from one repeated three times, nor notice that a fourth is missing.
///
/// Omitted for a single-child run so a plain `subagent({agent, async})` notification stays
/// byte-identical to today.
fn child_position(index: usize, total: usize) -> String {
    if total <= 1 {
        return String::new();
    }
    format!(" ({}/{total})", index + 1)
}

/// One child's rendered body: pi's `r.output || (exitCode !== 0 ? r.error : undefined) ||
/// "(no output)"` (`subagent-runner.ts:4744`) with `childInlinePreview`'s structured-output
/// fallback (`notify.ts:198-212`) spliced in where upstream's `resultPreview` would apply it.
///
/// # `(no output)` is the LAST rung, and reaching it must mean there is genuinely nothing
///
/// `NO_OUTPUT` does not deliver output — it is the admission that none was found, and every rung
/// above it exists so that admission is TRUE rather than merely convenient. A terminal that says
/// "nothing" while a file on disk holds the answer is precisely the dead end that sent the live
/// orchestrator to `bash` to `cat` that file itself.
///
/// So rung 3 is a POINTER. [`crate::artifacts::ArtifactPaths::output_path`] is documented as "the
/// child's delivered answer" and is written for every child whose artifacts are enabled (the
/// default); [`SingleResult::saved_output_path`] is the R-SA-031 handoff's own concrete file.
/// Naming either one hands the orchestrator something it can act on with its `read` tool — no
/// shell, no `python3`.
///
/// **[CYRUP-DELTA]** upstream surfaces this path only inside the workflow-gated `Child outputs:`
/// block (`notify.ts:214-238`, gated on `workflowRunId` at `buildCompletionDetails:526`), so a
/// plain pi fan-out does render a bare `(no output)` here. cyrup promotes it to the shared ladder
/// because the fan-out case is the one this task exists for, and a reachable answer must never be
/// reported as absent. Stated here rather than left as an unexplained difference.
fn child_display_body(child: &SingleResult) -> String {
    let link = child_output_path(child).map(|path| format!("\nOutput saved to: {path}"));
    if let Some(text) = inline_preview(child) {
        // CONTEXT EFFICIENCY — the body is the child's SUMMARY, never a dump. Three chatty
        // children would otherwise put three unbounded transcripts into the orchestrator's context
        // on every fan-out completion, which is what makes a useful notification unaffordable.
        //
        // SUBTASK0's contract block when the child honoured it, its output's TAIL when it did not
        // — both bounded, both ending on the conclusion rather than the preamble.
        let bounded = extract_result_summary(&text, CHILD_OUTPUT_PREVIEW_MAX_BYTES);
        return match (bounded.truncated, &link) {
            // Truncated WITH a file: the marker is not decoration, it is the contract that the
            // orchestrator can still reach the whole answer without guessing that it was cut.
            (true, Some(link)) => format!("{}\n[truncated]{link}", bounded.text),
            (true, None) => format!("{}\n[truncated]", bounded.text),
            // Untruncated: still name the file. It costs one line and is what lets a follow-up
            // read the full artifact without a `status` round-trip.
            (false, Some(link)) => format!("{}{link}", bounded.text),
            (false, None) => bounded.text,
        };
    }
    if child.exit_code != 0
        && let Some(error) = non_empty(child.error.as_deref())
    {
        // Same bounded extractor as the success path, on the ERROR text instead of
        // `final_output` — a stale error is not a contract-emitting child, so this always takes
        // the tail-truncation fallback, never the marker branch.
        return extract_result_summary(&error, CHILD_OUTPUT_PREVIEW_MAX_BYTES).text;
    }
    if let Some(path) = child_output_path(child) {
        // pi's own reference wording (`childInlinePreview`'s `referenceOnly` probe,
        // `notify.ts:201`, tests `output.trim().startsWith("Output saved to:")`), so a body this
        // renderer emits is recognised as a reference by the same predicate that detects one.
        return format!("Output saved to: {path}");
    }
    NO_OUTPUT.to_string()
}

/// The file holding this child's delivered answer when the text did not travel inline.
///
/// [`SingleResult::saved_output_path`] first — it is the explicitly requested R-SA-031 handoff and
/// therefore the caller's own chosen location — then the artifact bundle's `output_path`, which is
/// written unconditionally under the default artifact config.
fn child_output_path(child: &SingleResult) -> Option<String> {
    non_empty(child.saved_output_path.as_deref()).or_else(|| {
        child
            .artifact_paths
            .as_ref()
            .map(|paths| paths.output_path.display().to_string())
    })
}

/// pi `childInlinePreview`'s `raw` derivation (`notify.ts:198-212`), returning the text to show or
/// `None` when there is genuinely nothing.
///
/// The ordering is upstream's and is load-bearing: an `Absent` output-state or a reference-only
/// body goes STRAIGHT to structured output (the text is known not to be the answer), and only
/// then does a degenerate body fall back with the raw text as the second chance.
///
/// Returning `None` here is NOT the end of the line — [`child_display_body`]'s pointer rung runs
/// next. That matters most for the `reference_only` case: routing away from a "Output saved to:"
/// body and finding no structured output must not discard the reference, or a child whose answer
/// is on disk reports nothing.
fn inline_preview(child: &SingleResult) -> Option<String> {
    let output = child.final_output.as_deref().unwrap_or("");
    // pi `referenceOnly` (`:201`) — the body is just a pointer to a file, not the answer.
    let reference_only =
        child.saved_output_path.is_some() && output.trim_start().starts_with("Output saved to:");
    let raw = if child.output_state == SubagentOutputState::Absent || reference_only {
        structured_output_text(child.structured_output.as_ref())
    } else if is_degenerate_output(output) {
        structured_output_text(child.structured_output.as_ref()).or_else(|| non_empty(Some(output)))
    } else {
        non_empty(Some(output))
    };
    non_empty(raw.as_deref())
}

/// pi `isDegenerateOutput` (`notify.ts:194-196`) — blank, or a bare unclosed reasoning tag.
fn is_degenerate_output(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.is_empty() || trimmed == "</think>"
}

/// pi `structuredOutputText` (`notify.ts:184-192`) — `JSON.stringify(value, null, 2)`, bounded.
///
/// PRETTY (2-space), not compact: this lands in a model's context, and a wall of minified JSON is
/// exactly what the orchestrator was resorting to `python3` to unpack. `serde_json::to_string_pretty`
/// is 2-space by default, so it is upstream's output byte for byte.
///
/// Bounded by pi's own [`CHILD_OUTPUT_PREVIEW_MAX_BYTES`] through
/// [`crate::exec::output::utf8_safe_prefix`] — cyrup's existing port of `truncateUtf8Head`
/// (`exec/output.rs:1393`), which walks down to a char boundary rather than slicing bytes.
fn structured_output_text(value: Option<&serde_json::Value>) -> Option<String> {
    let text = serde_json::to_string_pretty(value?).ok()?;
    Some(crate::exec::output::utf8_safe_prefix(&text, CHILD_OUTPUT_PREVIEW_MAX_BYTES).to_string())
}

/// JS `value || next` on a string: empty (after trim) is falsy.
fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
}

/// Build the `subagent-notify` [`CompletionMessage`] for `result`, reproducing pi's `notify.ts`
/// content layout (`notify.ts:58-104`): a `Background task <status>: **<agent>**` header, a blank
/// line, the display summary (or `"(no output)"`), and — when a session file is present — a blank
/// line followed by `Session file: <path>`. `<status>` is `completed`/`failed`/`paused`/`stopped`
/// per [`classify_outcome`] (R-SA-100; a paused run is never reported as failed). The same
/// classification decides `display` ([`completion_notice_display`], SUBA-090): a plain
/// `completed` outcome is injected hidden, anything else is rendered.
#[must_use]
pub fn format_completion_message(result: &ResultFile) -> CompletionMessage {
    let outcome = classify_outcome(result);
    let status = match outcome {
        ClassifiedOutcome::Completed => "completed",
        ClassifiedOutcome::Failed => "failed",
        ClassifiedOutcome::Paused => "paused",
        // G77 — pi `notify.ts:210`'s own fourth word, rendered verbatim into the
        // `Background task <status>: **<agent>**` header.
        ClassifiedOutcome::Stopped => "stopped",
    };
    let agent = if result.agent.is_empty() {
        "unknown"
    } else {
        result.agent.as_str()
    };

    let summary = result_display_summary(result);
    let display_summary = if summary.trim().is_empty() {
        NO_OUTPUT.to_string()
    } else {
        summary
    };

    // pi's `content` array: header, "", the schedule line when there is one, "", displaySummary,
    // then (only if a session line exists) "" and the session line, joined by "\n"
    // (`notify.ts:343-356`).
    let mut lines: Vec<String> = vec![
        format!("Background task {status}: **{agent}**"),
        String::new(),
    ];
    if let Some(origin) = &result.schedule_origin {
        // pi `:341-342`, verbatim — the NAME when it has one, the id otherwise, and the id always.
        lines.push(format!(
            "Scheduled run from **{}** (schedule {}).",
            origin.name.as_deref().unwrap_or(origin.id.as_str()),
            origin.id
        ));
        lines.push(String::new());
    }
    lines.push(display_summary);
    if let Some(session_file) = &result.session_file {
        lines.push(String::new());
        lines.push(format!("Session file: {}", session_file.display()));
    }

    CompletionMessage {
        custom_type: "subagent-notify".to_string(),
        content: lines.join("\n"),
        display: completion_notice_display(outcome, result.schedule_origin.as_ref()),
        // pi: `triggerTurn: result.triggerTurn !== false && scheduledCompletionTriggersTurn(...)`
        // (`notify.ts:776`). `ResultFile` carries no `triggerTurn`, so the first conjunct is
        // always `true`; the second is SUBA-016's `quiet`.
        trigger_turn: scheduled_completion_triggers_turn(result.schedule_origin.as_ref(), outcome),
        // The one value-carrying completion shape: a `wait` that already surfaced this run's value
        // inline may suppress the standalone duplicate (`ASYNC_NOTIFY_BUG_REPORT` F3).
        suppressible: true,
    }
}

// =================================================================================================
// Loss notifications — a completion whose payload never reached the orchestrator
// =================================================================================================

/// The `subagent-notify` message for a run that completed but whose result payload was destroyed
/// before this session could deliver it.
///
/// # This recovers the answer; it does not merely admit the loss
///
/// Only the payload goes missing. The run's own directory is untouched by whatever consumed it, so
/// `status.json` still holds the terminal state, the agent names and a tail of each step's output
/// (`steps[].recentOutput`), and `events.jsonl` still holds the lifecycle. Announcing the loss
/// WITHOUT that content would tell the orchestrator only that something disappeared — which is
/// barely better than the silence this replaces. Announcing it WITH the content usually hands back
/// the answer the child actually produced.
///
/// `display: true` and `trigger_turn: true`: unlike a plain successful completion (SUBA-090), this
/// is a condition someone needs to see.
#[must_use]
pub fn format_missing_payload_message(report: &LossReport) -> CompletionMessage {
    let agent = if report.agent.is_empty() {
        "unknown"
    } else {
        report.agent.as_str()
    };
    let mut lines: Vec<String> = vec![
        format!(
            "Background task completed but its result payload was removed before delivery: **{agent}**"
        ),
        String::new(),
    ];

    if report.recovered.is_empty() {
        lines.push(NO_OUTPUT.to_string());
    } else {
        let total = report.recovered.len();
        let blocks: Vec<String> = report
            .recovered
            .iter()
            .enumerate()
            .map(|(index, step)| {
                let bounded =
                    extract_result_summary(&step.output, CHILD_OUTPUT_PREVIEW_MAX_BYTES).text;
                format!(
                    "{}{}:\n{}",
                    step.agent,
                    child_position(index, total),
                    bounded
                )
            })
            .collect();
        lines.push(blocks.join("\n\n"));
    }

    lines.push(String::new());
    lines.push(format!(
        "Recovered from run {}'s own records; its result file was removed before this session \
         could read it — most likely consumed by another cyrup instance in this directory.",
        report.run_id
    ));
    if let Some(async_dir) = &report.async_dir {
        lines.push(format!(
            "Artifacts: {} (status.json, events.jsonl)",
            async_dir.display()
        ));
    }

    CompletionMessage {
        custom_type: "subagent-notify".to_string(),
        content: lines.join("\n"),
        display: true,
        trigger_turn: true,
        // A loss report carries information no `wait` could have surfaced — never suppressed.
        suppressible: false,
    }
}

/// The `subagent-notify` message for a completion the sink refused
/// [`crate::background::watch::MAX_PROCESSING_ATTEMPTS`] times (R-SA-102's retry bound).
///
/// The payload is still on disk and still readable; what failed is delivery itself. Surfacing the
/// result's own summary alongside that fact is what keeps the retry bound from becoming a second
/// silent loss — the bound stops the retry loop, it must not stop the notification.
#[must_use]
pub fn format_undeliverable_message(result: &ResultFile) -> CompletionMessage {
    let base = format_completion_message(result);
    let mut content = base.content;
    content.push_str(
        "\n\nThis completion could not be delivered on repeated attempts; it is reported here \
         once rather than retried further.",
    );
    CompletionMessage {
        custom_type: base.custom_type,
        content,
        // Delivery trouble always warrants surfacing, whatever the run's own outcome was.
        display: true,
        trigger_turn: true,
        // A delivery-failure report is exactly what a `wait` cannot have answered — never
        // suppressed (`ASYNC_NOTIFY_BUG_REPORT` F3.2).
        suppressible: false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{child_result, result_with_children, sample_result};
    use super::*;
    use crate::background::RunState;
    use std::path::PathBuf;

    /// pi `notify.ts:210`'s status word reaches the rendered `subagent-notify` body.
    #[test]
    fn format_completion_message_renders_pis_fourth_status_word() {
        let stopped = sample_result("run-stop-3", RunState::Stopped, false);
        let message = format_completion_message(&stopped);
        assert!(
            message
                .content
                .starts_with("Background task stopped: **researcher**"),
            "{}",
            message.content
        );
        assert!(
            !message.content.contains("Background task failed"),
            "a stopped run must never be announced as failed: {}",
            message.content
        );
    }

    #[test]
    fn format_completion_message_reproduces_notify_ts_layout() {
        // Completed, with output and a session file.
        let result = result_with_children(
            "run-fmt-1",
            RunState::Complete,
            true,
            Some(PathBuf::from("/tmp/session.jsonl")),
            vec![child_result("worker", Some("Done"), 0)],
        );
        let msg = format_completion_message(&result);
        assert_eq!(msg.custom_type, "subagent-notify");
        assert!(
            !msg.display,
            "SUBA-090: a plain successful completion is not displayed (notify.ts:402 @v0.64.0)"
        );
        assert!(msg.trigger_turn);
        // SCOPE_17 — every child now gets an `{agent}:` prefix (divergence #1 in §2), including a
        // single-child run; only the `(i/n)` position suffix is omitted below `total > 1`.
        assert_eq!(
            msg.content,
            "Background task completed: **worker**\n\nworker:\nDone\n\nSession file: /tmp/session.jsonl"
        );

        // Empty output falls back to "(no output)".
        let empty = result_with_children(
            "run-fmt-2",
            RunState::Complete,
            true,
            None,
            vec![child_result("worker", None, 0)],
        );
        assert_eq!(
            format_completion_message(&empty).content,
            "Background task completed: **worker**\n\nworker:\n(no output)"
        );

        // A paused run is reported paused, never failed (R-SA-100).
        let paused = result_with_children(
            "run-fmt-3",
            RunState::Paused,
            false,
            None,
            vec![child_result("worker", Some("Paused after interrupt."), 0)],
        );
        assert_eq!(
            format_completion_message(&paused).content,
            "Background task paused: **worker**\n\nworker:\nPaused after interrupt."
        );

        // A failed run is reported failed.
        let failed = result_with_children(
            "run-fmt-4",
            RunState::Failed,
            false,
            None,
            vec![child_result("worker", Some("boom"), 1)],
        );
        assert!(
            format_completion_message(&failed)
                .content
                .starts_with("Background task failed: **worker**")
        );
    }

    // =============================================================================================
    // SUBA-090 — the `display` predicate (v0.64.0 `notify.ts:402`)
    // =============================================================================================

    /// A plain successful background completion is injected as a NON-displayed context message:
    /// upstream's `display` is `details.some(d => d.source === "foreground" || d.status !==
    /// "completed" || d.scheduleOrigin !== undefined)` (`notify.ts:402` @v0.64.0), and cyrup's
    /// `ResultFile` carries neither `source` nor `scheduleOrigin`, so only the status clause can
    /// hold — and for `completed` it does not. The turn is still triggered (R-SA-101).
    #[test]
    fn a_plain_successful_background_completion_is_not_displayed() {
        let completed = result_with_children(
            "run-display-1",
            RunState::Complete,
            true,
            Some(PathBuf::from("/tmp/session.jsonl")),
            vec![child_result("worker", Some("Done"), 0)],
        );
        let msg = format_completion_message(&completed);
        assert_eq!(classify_outcome(&completed), ClassifiedOutcome::Completed);
        assert!(
            !msg.display,
            "a `completed` status is the one outcome upstream keeps invisible"
        );
        assert!(
            msg.trigger_turn,
            "hidden is not inert: the completion still re-enters the turn loop"
        );
        assert!(!completion_notice_display(
            ClassifiedOutcome::Completed,
            None
        ));
    }

    /// Every non-`completed` status satisfies upstream's `detail.status !== "completed"` clause, so
    /// failed, paused and stopped completions are rendered — including the `state: Complete,
    /// success: false` combination `classify_outcome` reports as failed (R-SA-100).
    #[test]
    fn failed_paused_and_stopped_completions_are_displayed() {
        let failed = result_with_children(
            "run-display-2",
            RunState::Failed,
            false,
            None,
            vec![child_result("worker", Some("boom"), 1)],
        );
        let acceptance_failed = result_with_children(
            "run-display-3",
            RunState::Complete,
            false,
            None,
            vec![child_result("worker", Some("rejected"), 0)],
        );
        let paused = result_with_children(
            "run-display-4",
            RunState::Paused,
            false,
            None,
            vec![child_result("worker", Some("Paused after interrupt."), 0)],
        );
        let stopped = result_with_children(
            "run-display-5",
            RunState::Stopped,
            false,
            None,
            vec![child_result("worker", Some("stopped"), 0)],
        );
        for (label, result) in [
            ("failed", &failed),
            ("complete-but-unsuccessful", &acceptance_failed),
            ("paused", &paused),
            ("stopped", &stopped),
        ] {
            let msg = format_completion_message(result);
            assert!(
                msg.display,
                "{label}: a non-completed status must be displayed"
            );
            assert!(msg.trigger_turn, "{label}: the turn is still triggered");
        }
        for outcome in [
            ClassifiedOutcome::Failed,
            ClassifiedOutcome::Paused,
            ClassifiedOutcome::Stopped,
        ] {
            assert!(completion_notice_display(outcome, None), "{outcome:?}");
        }
    }
}
