//! Final-output extraction, file-only output-path stat-snapshot handoff, and UTF-8-safe output
//! truncation (func-SA §5.2/§6.3.3; arch-SA §6.3.3; R-SA-024/025/029/031/042).
//!
//! # Scope
//!
//! This module owns exactly three algorithms, all operating strictly on already-parsed/already-
//! collected data — it spawns nothing itself and owns no subprocess lifecycle:
//!
//! 1. **Final-output extraction** ([`extract_final_output`], R-SA-029) — a two-level reverse scan
//!    over a completed attempt's [`crate::exec::ndjson::SubagentEvent::MessageEnd`] events: most
//!    recent non-error assistant message first, and within that message's content parts, an
//!    acceptance-report-shaped part beats a later plain-text part.
//! 2. **File-only output-path handoff** ([`snapshot_output_file`] / [`resolve_output_handoff`],
//!    R-SA-025/031) — a stat-snapshot (mtime + size) taken before the child spawns, compared
//!    after it exits, to decide whether the child wrote its own output file (verbatim win) or
//!    whether the orchestrator must persist its own captured `final_output` there instead. This
//!    is deliberately a heuristic, not a lock (R-SA-031's own text is explicit on this point).
//! 3. **UTF-8-safe truncation** ([`truncate_output`], R-SA-042) — a byte/line budget applied to
//!    the delivered output, cutting only at a UTF-8 character boundary and recording the
//!    truncation fact (never silently dropping bytes without a trace).
//!
//! Output-path steering (R-SA-024) has TWO injectors upstream, and this module ports both:
//!
//! * [`inject_single_output_instruction`] — the TASK-side one (`injectSingleOutputInstruction`,
//!   `single-output.ts:99-102`). This is the LIVE one: `exec/mod.rs::build_task_text` calls it on
//!   every run that has a configured output path, exactly as `subagent-executor.ts:3674` does.
//! * [`inject_output_path_system_prompt`] — the SYSTEM-PROMPT-side one
//!   (`injectOutputPathSystemPrompt`, `single-output.ts:104-108`), which upstream applies at
//!   `execution.ts:1443` and `api/preflight.ts:313`. This one is LIVE too:
//!   `exec/mod.rs::build_attempt_spawn_plan` composes it onto the persona body that becomes the
//!   child's `--system-prompt`/`--append-system-prompt`, at the same point in the composition
//!   order `execution.ts:1443` occupies.
//!
//! The two are complementary, not alternatives — upstream's foreground single run applies both to
//! the same run, and so does this crate.
//!
//! This module has ZERO dependency on `cyrup-agent` — every message/content shape it inspects is
//! the same opaque `serde_json::Value` [`crate::exec::ndjson::SubagentEvent`] already exposes,
//! never a typed `AgentMessage`/`Content` re-import (arch-SA §2.1/§1.1, restated at every module
//! boundary in this crate).

use std::path::Path;

use crate::discovery::types::AgentDefinition;
use crate::exec::ndjson::SubagentEvent;

// ============================================================================================
// R-SA-029: Final-output extraction
// ============================================================================================

/// A fenced-code-block scan match: the language tag (lowercased) and the fenced body, used by
/// [`looks_like_acceptance_report`] to test each fenced block in a text part independently of the
/// others (pi-subagents' own `getFinalOutput`, `pi-subagents/src/shared/utils.ts:280-307`, is the
/// direct source of truth this function ports verbatim — see that function's doc references
/// below for the exact three detection rules).
///
/// `pub(crate)` (not private) so [`crate::exec::structured`]'s R-SA-030 structured-output
/// extraction can reuse this exact fenced-block scanner — a fenced ` ```json `/`jsonc`/`json5`
/// block is the same shape both R-SA-029's acceptance-report detection and R-SA-030's
/// structured-output extraction key off, so this is one shared scanner, not two independent
/// reimplementations of the same fence-matching state machine.
pub(crate) struct FencedBlock<'a> {
    pub(crate) lang: &'a str,
    pub(crate) body: &'a str,
}

/// Scan `text` for every fenced code block (` ```lang\n...\n``` `), returning each one's lowercased
/// language tag and body. A hand-rolled scanner rather than a regex crate dependency: the shape
/// being matched (a line starting with three backticks, optionally followed by a language tag,
/// terminated by a line that is exactly three backticks) is simple enough that a byte-line walk is
/// both correct and avoids adding a new dependency to this crate for one narrow use.
pub(crate) fn fenced_blocks(text: &str) -> Vec<FencedBlock<'_>> {
    let mut blocks = Vec::new();
    let mut open_fence_end: Option<usize> = None;
    let mut lang_start = 0usize;
    let mut body_start = 0usize;

    for line in LineSpans::new(text) {
        if let Some(_fence_end) = open_fence_end {
            if line.trimmed == "```" {
                // Closing fence: body is everything between body_start and this line's start.
                let body = text.get(body_start..line.start).unwrap_or_default();
                let lang = text.get(lang_start..lang_start).unwrap_or_default();
                // lang was captured at open time; re-slice from the stored range below instead.
                let _ = lang;
                open_fence_end = None;
                blocks.push(FencedBlock {
                    lang: pending_lang(text, lang_start),
                    body,
                });
            }
            continue;
        }

        if let Some(rest) = line.trimmed.strip_prefix("```") {
            lang_start = line.start + 3;
            body_start = line.end_incl_newline;
            open_fence_end = Some(line.start);
            let _ = rest;
        }
    }

    blocks
}

/// Recover the language-tag slice recorded by [`fenced_blocks`] at fence-open time. Kept as a
/// tiny free function (rather than storing a second lifetime-tied slice inline) so
/// [`fenced_blocks`]'s loop body stays a straightforward state machine.
fn pending_lang(text: &str, lang_start: usize) -> &str {
    let rest = text.get(lang_start..).unwrap_or_default();
    let end = rest.find('\n').unwrap_or(rest.len());
    rest.get(..end).unwrap_or_default().trim()
}

/// One line's byte-offset span within its parent `&str`, including whether it ends the fence
/// marker exactly (used only by [`fenced_blocks`]'s hand-rolled scan).
struct LineSpan<'a> {
    trimmed: &'a str,
    start: usize,
    end_incl_newline: usize,
}

/// A minimal line iterator that (unlike `str::lines`) also reports each line's starting byte
/// offset and the offset immediately after its trailing newline — [`fenced_blocks`] needs both to
/// slice out a fenced body without reconstructing offsets via repeated `find`.
struct LineSpans<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> LineSpans<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, pos: 0 }
    }
}

impl<'a> Iterator for LineSpans<'a> {
    type Item = LineSpan<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.text.len() {
            return None;
        }
        let start = self.pos;
        let rest = self.text.get(start..).unwrap_or_default();
        let (line_end, next_pos) = match rest.find('\n') {
            Some(nl) => (start + nl, start + nl + 1),
            None => (self.text.len(), self.text.len()),
        };
        let trimmed = self
            .text
            .get(start..line_end)
            .unwrap_or_default()
            .trim_end_matches('\r');
        self.pos = next_pos;
        Some(LineSpan {
            trimmed,
            start,
            end_incl_newline: next_pos,
        })
    }
}

/// The acceptance-report-shaped JSON keys R-SA-029 names explicitly: a fenced `json`/`jsonc`/
/// `json5` block counts as acceptance-report-shaped only if it contains `"criteriaSatisfied"`
/// AND at least one of these companion keys — mirroring pi-subagents' own
/// `getFinalOutput` regex pair verbatim (`pi-subagents/src/shared/utils.ts:280-307`), not a
/// looser or stricter reinterpretation.
///
/// `pub(crate)` (not private) so `exec/acceptance.rs`'s self-report `Claimed` vs. `Attested`
/// floor classification can reuse this exact key list rather than duplicating it — one shared
/// definition of "what counts as companion evidence", consulted by both R-SA-029's extraction
/// and this crate's acceptance-ledger evaluation.
pub(crate) const ACCEPTANCE_REPORT_COMPANION_KEYS: &[&str] = &[
    "changedFiles",
    "testsAddedOrUpdated",
    "commandsRun",
    "validationOutput",
    "residualRisks",
    "noStagedFiles",
    "diffSummary",
    "reviewFindings",
    "manualNotes",
];

/// The two fence language tags an acceptance report may carry. G79 widened the PARSER to
/// `acceptance[-_]report` everywhere (`acceptance.ts:702,774,792` @v0.43.0); this probe has to
/// recognize the same set or the two disagree about whether a report exists at all.
pub(crate) const ACCEPTANCE_REPORT_FENCE_LANGS: &[&str] =
    &["acceptance-report", "acceptance_report"];

/// The snake_case spelling of a camelCase acceptance-report key, derived MECHANICALLY rather than
/// kept as a second hardcoded list, so this can never drift from
/// [`ACCEPTANCE_REPORT_COMPANION_KEYS`].
///
/// G79 gave every field of `ACCEPTANCE_REPORT_FIELDS` a snake_case alias (`acceptance.ts:486-508`).
/// Applied to the nine companion keys plus `criteriaSatisfied`, this transformation reproduces
/// upstream's table entry-for-entry (`criteriaSatisfied`→`criteria_satisfied`,
/// `testsAddedOrUpdated`→`tests_added_or_updated`, `noStagedFiles`→`no_staged_files`, …).
fn snake_case_alias(camel: &str) -> String {
    let mut out = String::with_capacity(camel.len() + 4);
    for ch in camel.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// Does `body` mention the JSON key `camel`, in either the canonical camelCase spelling or the
/// snake_case alias G79 accepts? A bare substring test on the body text, exactly like the
/// camelCase-only check it replaces — this widens WHICH spellings count, not how they are matched.
fn body_mentions_report_key(body: &str, camel: &str) -> bool {
    text_mentions_report_key(body, camel)
}

/// [`body_mentions_report_key`], exposed for `exec/acceptance.rs`'s self-report `Claimed` vs.
/// `Attested` floor, which scans the whole text rather than one fenced block's body. Shared so the
/// two can never disagree about which spellings count as companion evidence.
pub(crate) fn text_mentions_report_key(text: &str, camel: &str) -> bool {
    text.contains(&format!("\"{camel}\""))
        || text.contains(&format!("\"{}\"", snake_case_alias(camel)))
}

/// R-SA-029: does `text` "look like" a fenced acceptance-report, per the three independent
/// detection rules the requirement enumerates (any one matching is sufficient)?
///
/// 1. A fenced block whose language tag is literally `acceptance-report` (case-insensitive),
///    regardless of body content.
/// 2. A fenced `json`/`jsonc`/`json5` block whose body contains the literal key
///    `"criteriaSatisfied"` AND at least one of [`ACCEPTANCE_REPORT_COMPANION_KEYS`] — a bare
///    substring test on the body text (matching pi-subagents' own regex-over-body-text approach,
///    not a full JSON parse: a body that is not yet valid JSON, or has trailing commentary, must
///    still be detected the same way pi-subagents detects it).
/// 3. The literal marker text `ACCEPTANCE_REPORT:` (case-insensitive), anywhere in `text` — not
///    necessarily inside a fenced block at all.
#[must_use]
pub fn looks_like_acceptance_report(text: &str) -> bool {
    for block in fenced_blocks(text) {
        let lang = block.lang.to_ascii_lowercase();
        // G79: both `acceptance-report` and `acceptance_report` (`acceptance.ts:702`).
        if ACCEPTANCE_REPORT_FENCE_LANGS.contains(&lang.as_str()) {
            return true;
        }
        // G79: every key also answers to its snake_case alias (`acceptance.ts:486-508`).
        if matches!(lang.as_str(), "json" | "jsonc" | "json5")
            && body_mentions_report_key(block.body, "criteriaSatisfied")
            && ACCEPTANCE_REPORT_COMPANION_KEYS
                .iter()
                .any(|key| body_mentions_report_key(block.body, key))
        {
            return true;
        }
    }
    text.to_ascii_uppercase().contains("ACCEPTANCE_REPORT:")
}

/// One plain-text content part extracted from a [`SubagentEvent::MessageEnd`]'s `message.content`
/// array, in original (non-reversed) order within that message — the `Vec<&ContentPart>` arch-SA
/// §6.3.3 names, built as owned `String`s here since the underlying `serde_json::Value` is itself
/// a temporary borrow of the message value. Callers are responsible for the `role == "assistant"`
/// gate (matching [`SubagentEvent::is_error_or_aborted_message`]'s own convention) BEFORE calling
/// this — it inspects only `content`, not `role`, so it says nothing about whether `message` is
/// itself an assistant turn at all.
fn assistant_text_parts(message: &serde_json::Value) -> Vec<String> {
    let Some(content) = message.get("content").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    content
        .iter()
        .filter_map(|part| {
            let is_text = part.get("type").and_then(serde_json::Value::as_str) == Some("text");
            if !is_text {
                return None;
            }
            part.get("text")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// R-SA-029 (MUST) — final plain-text output extraction.
///
/// Reverse-scans `events` (assumed to be exactly the [`SubagentEvent::MessageEnd`] events
/// observed over the course of one attempt, in the chronological order they were parsed off the
/// child's stdout) from last to first. For each such event:
///
/// - Skip it entirely if [`SubagentEvent::is_error_or_aborted_message`] flags it (a message
///   flagged error/aborted can never contribute the winning text, even as a last-resort fallback).
/// - Otherwise, walk that message's content parts (in *original*, non-reversed order — arch-SA
///   §6.3.3's "build a `Vec<&ContentPart>` in original order for the winning message"), skipping
///   any part that is empty or whitespace-only (R-SA-029's explicit "cannot mask earlier
///   meaningful text" clause), and:
///   - If any surviving part in this message looks like an acceptance-report
///     ([`looks_like_acceptance_report`]), return the FIRST such part (in original order) —
///     found here in only the OTHER of two `MessageEnd`s.
///   - Otherwise, remember this message's LAST non-empty text part as a fallback candidate, but
///     keep scanning earlier messages: a chronologically earlier message's acceptance-report part
///     still outranks this message's plain fallback, since the acceptance-report search itself
///     visits messages newest-first and returns on the FIRST (i.e. most recent) hit.
///
/// If no message anywhere in the reverse scan contains an acceptance-report-shaped part, the
/// function falls back to the most recent non-error message's last non-empty text part — which is
/// exactly the first fallback candidate recorded during the scan, since the scan visits messages
/// newest-first.
///
/// Returns `None` if no non-error `MessageEnd` event contributes any non-empty text part at all.
#[must_use]
pub fn extract_final_output(events: &[SubagentEvent]) -> Option<String> {
    let mut fallback: Option<String> = None;

    for event in events.iter().rev() {
        let SubagentEvent::MessageEnd { message } = event else {
            continue;
        };
        // R-SA-029 scans "parsed assistant `MessageEnd` events" specifically — a `MessageEnd`
        // wrapping a non-assistant message (e.g. a `user`/`toolResult` echo, per `AgentMessage`'s
        // `role` tag) is not an assistant message at all and must never contribute text, exactly
        // mirroring `SubagentEvent::is_error_or_aborted_message`'s own role gate.
        if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        if event.is_error_or_aborted_message() {
            continue;
        }

        let parts = assistant_text_parts(message);
        let non_empty: Vec<&String> = parts
            .iter()
            .filter(|part| !part.trim().is_empty())
            .collect();
        if non_empty.is_empty() {
            continue;
        }

        // Rule: an acceptance-report-shaped part anywhere in this message wins immediately,
        // taking the FIRST such part in original (non-reversed) content order.
        if let Some(report_part) = non_empty
            .iter()
            .find(|part| looks_like_acceptance_report(part))
        {
            return Some((*report_part).clone());
        }

        // No acceptance-report shape in this message: record its last non-empty text part as the
        // running fallback, but only if this is the FIRST (most recent) message to reach this
        // point — an earlier (older) message's fallback text must never override a more recent
        // message's fallback text.
        if fallback.is_none() {
            let last = non_empty.last().map(|s| (*s).clone()).unwrap_or_default();
            fallback = Some(last);
        }
    }

    fallback
}

// ============================================================================================
// Exit-0 re-diagnosis: detectSubagentError + trailing assistant errorMessage + empty-output
// classification (pi `detectSubagentError` `utils.ts:481-523`; the assistantError state machine +
// empty-output check in `execution.ts:556-790`). Tier T3, group A.
//
// pi re-diagnoses a child that EXITED ZERO for latent failures its process exit code alone did not
// surface — a trailing failed tool/provider call, a still-set assistant `errorMessage`, or an
// empty/cold-start response — flipping the run to a failure (and, for the empty-output case, a
// *retryable* one so the model-fallback ladder advances). These are pure functions over the parsed
// event stream; `exec/mod.rs`'s per-attempt driver wires them into the `AttemptSignal` so the
// ladder's retry decision (`is_retryable_model_failure`) observes the re-diagnosed error.
// ============================================================================================

/// The exact message pi surfaces (and the model-fallback classifier treats as retryable via its
/// `cold.?start`/`empty response`/`no output` patterns, `model-fallback.ts:129-131`) when a
/// zero-exit attempt produced no usable final text — a likely model cold-start or empty response
/// (`execution.ts:788`).
pub const EMPTY_OUTPUT_ERROR: &str =
    "Subagent produced no output (possible model cold-start or empty response).";

/// The paused-success sentinel pi delivers for a soft-interrupted run (`execution.ts:848-852` @v0.34.0) —
/// exit 0, cleared error, this text as the final output.
pub const INTERRUPTED_FINAL_OUTPUT: &str = "Interrupted. Waiting for explicit next action.";

/// A re-diagnosed subagent failure discovered by [`detect_subagent_error`] on an otherwise
/// exit-zero run — a faithful port of pi's `ErrorInfo` (`utils.ts:481-523`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSubagentError {
    /// The exit code to attribute to the run — either parsed out of the failing tool/bash output
    /// (`exit(?:ed)? … (\d+)`) or `1` when no numeric code was recoverable.
    pub exit_code: i32,
    /// The failing tool's name (pi `toolName`), or `"tool"`/`"bash"` when unnamed — becomes the
    /// `<errorType> failed …` prefix of the surfaced error message.
    pub error_type: String,
    /// The first 200 characters of the failing tool/bash output, if any (pi `details.slice(0, 200)`).
    pub details: Option<String>,
}

impl DetectedSubagentError {
    /// The surfaced error message, exactly matching pi's `execution.ts:776-778`:
    /// `"<errorType> failed (exit <code>): <details>"` when details are present, otherwise
    /// `"<errorType> failed with exit code <code>"`.
    #[must_use]
    pub fn message(&self) -> String {
        match &self.details {
            Some(details) => format!(
                "{} failed (exit {}): {details}",
                self.error_type, self.exit_code
            ),
            None => format!(
                "{} failed with exit code {}",
                self.error_type, self.exit_code
            ),
        }
    }
}

/// The `fatalPatterns` bash-output substrings pi treats as a hard failure even at a zero (or
/// absent) parsed exit code (`utils.ts:442-451`), lowercased for a case-insensitive `contains`
/// test (pi's patterns are all `/…/i` regexes that reduce to a plain substring here; `killed|`
/// `terminated` is the one alternation, split into its two members).
const FATAL_BASH_PATTERNS: &[&str] = &[
    "command not found",
    "permission denied",
    "no such file or directory",
    "segmentation fault",
    "killed",
    "terminated",
    "out of memory",
    "connection refused",
    "timeout",
];

/// Skip ASCII whitespace in `bytes` starting at `pos`, returning the first non-whitespace offset.
fn skip_ascii_ws(bytes: &[u8], mut pos: usize) -> usize {
    while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
        pos += 1;
    }
    pos
}

/// Port of pi's exit-code regex `exit(?:ed)?\s*(?:with\s*)?(?:code|status)?\s*[:\s]?\s*(\d+)`
/// (`utils.ts:416/431`), case-insensitive, returning the first matched numeric code. Hand-rolled
/// (no `regex` dependency, matching this crate's `is_retryable_model_failure`/`regexlite`
/// convention): the pattern is anchored at `exit`/`exited`, so only occurrences of that literal
/// need be probed, each followed by the fixed optional `with`/`code`/`status`/`:` scaffold before
/// the digits.
fn parse_exit_code(text: &str) -> Option<i32> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut search_from = 0;
    while let Some(rel) = lower.get(search_from..).and_then(|s| s.find("exit")) {
        let start = search_from + rel;
        let mut pos = start + "exit".len();
        // Optional "ed" (exit -> exited).
        if lower.get(pos..).is_some_and(|s| s.starts_with("ed")) {
            pos += "ed".len();
        }
        pos = skip_ascii_ws(bytes, pos);
        // Optional "with".
        if lower.get(pos..).is_some_and(|s| s.starts_with("with")) {
            pos += "with".len();
            pos = skip_ascii_ws(bytes, pos);
        }
        // Optional "code" | "status".
        if lower.get(pos..).is_some_and(|s| s.starts_with("code")) {
            pos += "code".len();
        } else if lower.get(pos..).is_some_and(|s| s.starts_with("status")) {
            pos += "status".len();
        }
        pos = skip_ascii_ws(bytes, pos);
        // Optional single ":" separator ([:\s]? — the `\s` alternative is subsumed by the
        // surrounding `\s*`).
        if bytes.get(pos) == Some(&b':') {
            pos += 1;
        }
        pos = skip_ascii_ws(bytes, pos);
        // Required (\d+).
        let digits_start = pos;
        while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
            pos += 1;
        }
        if pos > digits_start
            && let Some(code) = lower
                .get(digits_start..pos)
                .and_then(|d| d.parse::<i32>().ok())
        {
            return Some(code);
        }
        // No digits followed this `exit`; probe the next occurrence (pi's non-global `.match`
        // likewise falls through to the next anchor position).
        search_from = start + "exit".len();
    }
    None
}

/// The first 200 characters of `text` (pi `.slice(0, 200)`), cut on a UTF-8 character boundary
/// (JS slices by UTF-16 code unit; taking whole chars is the closest boundary-safe equivalent for
/// diagnostic text).
fn first_200_chars(text: &str) -> String {
    text.chars().take(200).collect()
}

/// Extract the first `{"type":"text"}` text out of a [`SubagentEvent::ToolExecutionEnd`]'s `result`
/// value — pi's `msg.content.find((c) => c.type === "text")?.text` (`utils.ts:414/427`), adapted to
/// cyrup's real wire shape where a tool result serializes as
/// `{"content":[{"type":"text","text":…}],"details":…,"terminate":…}` (`cyrup-agent`
/// `result_value_of`, `agent.rs:113-115`). Tolerant of the shapes a real/scripted child can emit: a
/// bare string result, a `{content:[…]}` object, or a bare `[…]` array of content parts.
pub(crate) fn extract_tool_result_text(result: &serde_json::Value) -> Option<String> {
    fn first_text_part(parts: &[serde_json::Value]) -> Option<String> {
        parts.iter().find_map(|part| {
            if part.get("type").and_then(serde_json::Value::as_str) == Some("text") {
                part.get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            } else {
                None
            }
        })
    }
    match result {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(parts) => first_text_part(parts),
        serde_json::Value::Object(_) => result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .and_then(|parts| first_text_part(parts))
            .or_else(|| {
                result
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
        _ => None,
    }
}

/// One entry in the flat "messages" projection [`detect_subagent_error`] scans — the cyrup analog
/// of pi's interleaved `result.messages` (assistant `message_end` + `toolResult` `tool_result_end`).
/// cyrup's wire has no `tool_result_end` message; a tool result arrives as a
/// [`SubagentEvent::ToolExecutionEnd`], so that variant plays pi's `role === "toolResult"` role.
struct DiagMessage {
    /// pi's per-message `hasText` for the assistant-anchor scan: an assistant message with at least
    /// one non-empty text part.
    is_assistant_with_text: bool,
    /// `Some` iff this entry is a tool result (a `ToolExecutionEnd`), carrying exactly what pi's
    /// `toolResult`-role branch inspects.
    tool_result: Option<DiagToolResult>,
}

struct DiagToolResult {
    tool_name: String,
    is_error: bool,
    text: Option<String>,
}

/// Port of pi's `detectSubagentError` (`utils.ts:481-523`): re-diagnose a run for a trailing
/// tool/provider failure that has *no subsequent assistant text recovering from it*.
///
/// The reverse scan starts strictly *after* the last assistant message that carried real text — a
/// tool/bash error the agent went on to speak about (i.e. it "recovered") is deliberately NOT
/// treated as a run failure, matching pi's `lastAssistantTextIndex`/`scanStart` gate. Within the
/// tail region, in reverse:
/// - an explicitly `isError` tool result fails the run (its parsed exit code, or `1`);
/// - a `bash` result whose text parses a non-zero exit code fails the run;
/// - a `bash` result matching any [`FATAL_BASH_PATTERNS`] substring fails the run (exit `1`).
///
/// Returns `None` when nothing in the tail region indicates a failure.
#[must_use]
pub fn detect_subagent_error(events: &[SubagentEvent]) -> Option<DetectedSubagentError> {
    let messages: Vec<DiagMessage> = events
        .iter()
        .filter_map(|event| match event {
            SubagentEvent::MessageEnd { message } => {
                let is_assistant =
                    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant");
                let is_assistant_with_text = is_assistant
                    && assistant_text_parts(message)
                        .iter()
                        .any(|part| !part.trim().is_empty());
                Some(DiagMessage {
                    is_assistant_with_text,
                    tool_result: None,
                })
            }
            SubagentEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } => Some(DiagMessage {
                is_assistant_with_text: false,
                tool_result: Some(DiagToolResult {
                    tool_name: tool_name.clone(),
                    is_error: *is_error,
                    text: extract_tool_result_text(result),
                }),
            }),
            _ => None,
        })
        .collect();

    let scan_start = messages
        .iter()
        .rposition(|m| m.is_assistant_with_text)
        .map_or(0, |idx| idx + 1);

    for message in messages.get(scan_start..).unwrap_or_default().iter().rev() {
        let Some(tool_result) = &message.tool_result else {
            continue; // pi: `if (msg.role !== "toolResult") continue;`
        };

        if tool_result.is_error {
            let details = tool_result.text.clone();
            let exit_code = details.as_deref().and_then(parse_exit_code).unwrap_or(1);
            return Some(DetectedSubagentError {
                exit_code,
                error_type: if tool_result.tool_name.is_empty() {
                    "tool".to_string()
                } else {
                    tool_result.tool_name.clone()
                },
                details: details.as_deref().map(first_200_chars),
            });
        }

        if tool_result.tool_name != "bash" {
            continue;
        }
        let Some(output) = &tool_result.text else {
            continue;
        };
        if let Some(code) = parse_exit_code(output)
            && code != 0
        {
            return Some(DetectedSubagentError {
                exit_code: code,
                error_type: "bash".to_string(),
                details: Some(first_200_chars(output)),
            });
        }
        let lowered = output.to_ascii_lowercase();
        if FATAL_BASH_PATTERNS
            .iter()
            .any(|pattern| lowered.contains(pattern))
        {
            return Some(DetectedSubagentError {
                exit_code: 1,
                error_type: "bash".to_string(),
                details: Some(first_200_chars(output)),
            });
        }
    }

    None
}

/// Port of pi's live `assistantError` state machine (`execution.ts:476,940,945` @v0.43.0): the trailing,
/// still-uncleared assistant `errorMessage`, if any.
///
/// Each assistant `message_end` with a non-empty `errorMessage` sets the trailing error; a
/// subsequent *clean terminal stop* (`stopReason === "stop"`, no tool-call part, no `errorMessage`,
/// and real text) clears it — modelling pi's "the agent produced a real answer after the transient
/// provider error, so treat the run as recovered". A pure fold over the event stream (pi computes
/// it live only because it also drives control events off it; the terminal value is a pure function
/// of the ordered messages).
#[must_use]
pub fn trailing_assistant_error(events: &[SubagentEvent]) -> Option<String> {
    let mut assistant_error: Option<String> = None;
    for event in events {
        let SubagentEvent::MessageEnd { message } = event else {
            continue;
        };
        if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        let error_message = message
            .get("errorMessage")
            .and_then(serde_json::Value::as_str)
            .filter(|s| !s.is_empty());
        if let Some(err) = error_message {
            assistant_error = Some(err.to_string());
        }
        if is_terminal_assistant_stop(event) {
            let has_text = assistant_text_parts(message)
                .iter()
                .any(|part| !part.trim().is_empty());
            if error_message.is_none() && has_text {
                assistant_error = None;
            }
        }
    }
    assistant_error
}

/// Whether `event` is a *terminal assistant stop*: an assistant `message_end` with
/// `stopReason === "stop"` and no `toolCall` content part (pi `execution.ts:575-578`). This is the
/// signal pi (and, per Tier T3 group A, cyrup's `drive_attempt`) uses to open the final-stop
/// grace-drain window, and the anchor [`trailing_assistant_error`]'s clear condition keys off.
#[must_use]
pub fn is_terminal_assistant_stop(event: &SubagentEvent) -> bool {
    let SubagentEvent::MessageEnd { message } = event else {
        return false;
    };
    if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return false;
    }
    if message
        .get("stopReason")
        .and_then(serde_json::Value::as_str)
        != Some("stop")
    {
        return false;
    }
    let has_tool_call = message
        .get("content")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("type").and_then(serde_json::Value::as_str) == Some("toolCall")
            })
        });
    !has_tool_call
}

/// Whether a `message_end` event carries a non-empty assistant `errorMessage` — pi's truthy
/// `evt.message.errorMessage` test (`execution.ts:580`), i.e. an empty string counts as "no error".
/// Used by the final-stop grace-drain to decide whether a forced-drained terminal stop was *clean*
/// (`forcedDrainAfterFinalSuccess`, `execution.ts:1080-1097`).
#[must_use]
pub fn message_end_has_error_message(event: &SubagentEvent) -> bool {
    let SubagentEvent::MessageEnd { message } = event else {
        return false;
    };
    message
        .get("errorMessage")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|s| !s.is_empty())
}

/// SUBA-089 — the `errorMessage` of every `message_end` message in `events`, in order, for
/// [`crate::exec::fallback::AttemptSignal::message_errors`]. pi's `messageError(message)`
/// (`runs/shared/model-fallback.ts:524-528` @v0.64.0) reads the field off ANY message object
/// regardless of role and keeps any string, untrimmed — the trim happens at comparison time in
/// `isRetryableModelFailureAttempt` — so this does the same: no role filter, no trimming, no
/// empty-string filter (an empty `errorMessage` can never equal a non-empty error there anyway).
/// Non-string values are skipped, like `typeof value === "string"`.
#[must_use]
pub fn message_error_messages(events: &[SubagentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SubagentEvent::MessageEnd { message } => message
                .get("errorMessage")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            _ => None,
        })
        .collect()
}

// ============================================================================================
// R-SA-024/025/031: File-only output-path handoff
// ============================================================================================

/// A stat-snapshot of an output-path file's mtime/size, taken once before a child is spawned
/// (R-SA-031). Deliberately carries only the two fields the heuristic compares — never a file
/// lock, never a file handle held open across the child's lifetime (R-SA-031's explicit "MUST NOT
/// use a lock" constraint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFileSnapshot {
    /// `None` if the path did not exist at snapshot time.
    state: Option<(std::time::SystemTime, u64)>,
}

impl OutputFileSnapshot {
    /// Whether the snapshotted path existed at snapshot time.
    #[must_use]
    pub fn existed(&self) -> bool {
        self.state.is_some()
    }
}

/// R-SA-031: snapshot `output_path`'s mtime + size before spawning a child, if a path is
/// configured at all. Advisory only — a stat failure for any reason other than "the path does not
/// exist yet" (e.g. a permissions error) still degrades to a non-existent snapshot rather than
/// propagating an error here, matching pi-subagents' own `captureSingleOutputSnapshot`
/// (`pi-subagents/src/runs/shared/single-output.ts:147-156`): the snapshot's only job is to be
/// compared against post-exit state by [`resolve_output_handoff`], which itself surfaces any
/// genuine read/write failure directly rather than through this snapshot step. Returns `None` if
/// no output path is configured at all (nothing to snapshot).
#[must_use]
pub fn snapshot_output_file(output_path: Option<&Path>) -> Option<OutputFileSnapshot> {
    let path = output_path?;
    let state = std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok().map(|mtime| (mtime, meta.len())));
    Some(OutputFileSnapshot { state })
}

/// The outcome of reconciling a file-only/output-path handoff after a child has exited
/// (R-SA-031).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputHandoff {
    /// The output file changed since the pre-spawn snapshot (or did not exist at snapshot time
    /// but exists now) — the child wrote it itself. Its content, read back verbatim, MUST be used
    /// as the delivered output.
    ChildWrote { content: String },
    /// The output file did not change — the orchestrator persisted its own captured
    /// `final_output` to `output_path` and that is what was delivered. `written` is `true` unless
    /// the persist itself failed (see `error`, which is then `Some`).
    OrchestratorWrote {
        written: bool,
        error: Option<String>,
    },
}

/// R-SA-025 (MUST) — file-only output requires a path. Call this before any subprocess is
/// spawned; a `Some` return means the run MUST fail fast with
/// [`crate::error::SubagentError::OutputPathRequired`] and no child may be launched.
#[must_use]
pub fn validate_file_only_requires_path(
    output_mode: crate::discovery::types::OutputMode,
    output_path: Option<&Path>,
) -> Option<crate::error::SubagentError> {
    let is_file_only = matches!(output_mode, crate::discovery::types::OutputMode::FileOnly);
    if is_file_only && output_path.is_none() {
        Some(crate::error::SubagentError::OutputPathRequired)
    } else {
        None
    }
}

/// R-SA-031 (MUST) — reconcile the output-path handoff after the child has exited: compare the
/// current on-disk state of `output_path` against `before`, and either read back the child's own
/// write verbatim or persist the orchestrator's own `captured_output` to that path.
///
/// This is a pure stat-snapshot heuristic, never a lock (R-SA-031's own text): two children racing
/// on the same `output_path` is the caller's responsibility to avoid, not something this function
/// detects or guards against. "Changed" is `true` if:
/// - the path did not exist at snapshot time but exists now, or
/// - the path's mtime OR size differs from the snapshot, or
/// - `before` itself is `None` (no snapshot was taken, e.g. because [`snapshot_output_file`] was
///   never called for this attempt) — treated as "assume changed" so a first-ever run with no
///   prior snapshot still prefers reading back whatever is on disk over blindly overwriting it.
///
/// A stat/read failure classified as "the path genuinely does not exist" is NOT an error — it
/// simply means the child did not write the file, so the orchestrator persists its own output.
/// Any OTHER stat/read failure (permissions, I/O fault) falls back to the orchestrator's captured
/// output as well, but is surfaced via [`OutputHandoff::OrchestratorWrote`]'s `error` field so the
/// caller can still report it (never a panic, never a silently swallowed genuine I/O fault).
#[must_use]
pub fn resolve_output_handoff(
    output_path: &Path,
    captured_output: &str,
    before: Option<OutputFileSnapshot>,
) -> OutputHandoff {
    let after = std::fs::metadata(output_path)
        .ok()
        .and_then(|meta| meta.modified().ok().map(|mtime| (mtime, meta.len())));

    let changed = match (before.and_then(|snap| snap.state), after) {
        (None, None) => false,    // never existed, still doesn't: unchanged
        (None, Some(_)) => true,  // didn't exist before, exists now: the child wrote it
        (Some(_), None) => false, // existed before, gone now: nothing to read back
        (Some(before_state), Some(after_state)) => before_state != after_state,
    };
    // No snapshot was ever taken for this attempt: treat as "assume changed" (see doc comment).
    let changed = changed || before.is_none();

    if changed {
        match std::fs::read_to_string(output_path) {
            Ok(content) => return OutputHandoff::ChildWrote { content },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Raced away between the metadata() check and the read — fall through to
                // persisting the orchestrator's own output below rather than erroring.
            }
            Err(err) => {
                return persist_orchestrator_output(output_path, captured_output, Some(err));
            }
        }
    }

    persist_orchestrator_output(output_path, captured_output, None)
}

/// Persist `captured_output` to `output_path`, creating parent directories as needed, for the
/// "orchestrator wrote it" branch of [`resolve_output_handoff`]. `prior_error`, if present, is a
/// read failure that happened first and is folded into the final error message rather than
/// discarded, so a caller sees the FULL story (why the read-back was abandoned, and whether the
/// subsequent persist also failed) rather than only the last failure.
fn persist_orchestrator_output(
    output_path: &Path,
    captured_output: &str,
    prior_error: Option<std::io::Error>,
) -> OutputHandoff {
    let persist_result = output_path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(output_path, captured_output));

    match (prior_error, persist_result) {
        (None, Ok(())) => OutputHandoff::OrchestratorWrote {
            written: true,
            error: None,
        },
        (Some(read_err), Ok(())) => OutputHandoff::OrchestratorWrote {
            written: true,
            error: Some(format!(
                "failed to read back changed output file, persisted orchestrator output \
                 instead: {read_err}"
            )),
        },
        (None, Err(write_err)) => OutputHandoff::OrchestratorWrote {
            written: false,
            error: Some(format!("failed to persist output file: {write_err}")),
        },
        (Some(read_err), Err(write_err)) => OutputHandoff::OrchestratorWrote {
            written: false,
            error: Some(format!(
                "failed to read back changed output file ({read_err}); persisting orchestrator \
                 output also failed: {write_err}"
            )),
        },
    }
}

// ============================================================================================
// Saved-output reference message (pi `formatSavedOutputReference`, single-output.ts:128-138)
// ============================================================================================

/// The "output saved to a file" reference pi surfaces to the caller once a step/run with an
/// `output` file path finishes cleanly — the `bytes`/`lines` are measured over the FULL (untruncated)
/// persisted content, and `message` is the exact human-readable line appended to (or, in
/// `outputMode: "file-only"`, substituted for) the delivered output. Faithful port of pi-subagents'
/// `SavedOutputReference` (`single-output.ts:128-138`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedOutputReference {
    /// The absolute on-disk path the output was saved to.
    pub path: std::path::PathBuf,
    /// Byte length of the full (untruncated) persisted content.
    pub bytes: usize,
    /// Line count of the full (untruncated) persisted content, counted pi's single-output way
    /// (newline separators, plus one more unless the text ends in a newline; empty text is 0 lines).
    pub lines: usize,
    /// The `Output saved to: <path> (<size>, <n> line(s)). Read this file if needed.` message.
    pub message: String,
}

/// pi `formatByteSize` (`single-output.ts:116-126`): `"<n> B"` under 1024, else a 1-decimal
/// `KB`/`MB`/`GB`/`TB` value WITH a space before the unit — deliberately distinct from
/// [`format_bytes`]'s no-space `"12.3KB"` truncation-marker form (pi keeps two different byte
/// formatters for these two surfaces; this one is the saved-output-reference form).
fn format_byte_size(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let units = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64 / 1024.0;
    let mut unit_index = 0usize;
    while value >= 1024.0 && unit_index < units.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }
    format!(
        "{value:.1} {}",
        units.get(unit_index).copied().unwrap_or("TB")
    )
}

/// pi single-output `countLines` (`single-output.ts:110-114`): count `\r\n`/`\r`/`\n` separators, plus
/// one more unless the text ends in a `\r`/`\n`; empty text is 0 lines. Deliberately NOT
/// [`count_lines`] (the truncation-marker line count, which counts `split('\n')` segments and so
/// differs for trailing-newline text) — this matches the exact counter pi uses for the saved-output
/// reference.
fn count_reference_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let bytes = text.as_bytes();
    let mut separators = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'\r') => {
                separators += 1;
                if matches!(bytes.get(i + 1), Some(b'\n')) {
                    i += 1;
                }
            }
            Some(b'\n') => separators += 1,
            _ => {}
        }
        i += 1;
    }
    let ends_with_newline = text.ends_with('\r') || text.ends_with('\n');
    separators + usize::from(!ends_with_newline)
}

/// pi `formatSavedOutputReference` (`single-output.ts:128-138`): build the [`SavedOutputReference`]
/// for `saved_path` measured over `full_output`. `saved_path` is expected to already be absolute
/// (the caller resolves a relative `output` against the run/chain cwd before spawning — pi's own
/// `resolveSingleOutputPath`); it is used verbatim as the reported/message path so the message names
/// the exact file the child (or orchestrator) wrote.
#[must_use]
pub fn format_saved_output_reference(saved_path: &Path, full_output: &str) -> SavedOutputReference {
    let bytes = full_output.len();
    let lines = count_reference_lines(full_output);
    let line_word = if lines == 1 { "line" } else { "lines" };
    let message = format!(
        "Output saved to: {} ({}, {lines} {line_word}). Read this file if needed.",
        saved_path.display(),
        format_byte_size(bytes),
    );
    SavedOutputReference {
        path: saved_path.to_path_buf(),
        bytes,
        lines,
        message,
    }
}

// ============================================================================================
// R-SA-024: argv/system-prompt steering for file-only output mode
// ============================================================================================

/// G82 — source: `formatOutputPathInstruction`'s `capabilities` parameter
/// (`pi-subagents/src/runs/shared/single-output.ts:79-82,85`). Upstream passes the resolved
/// agent/step config straight in (`injectOutputPathSystemPrompt(systemPrompt, outputPath, agent)`,
/// `execution.ts:1443`), reading only its `tools`/`mcpDirectTools`; this crate's equivalent view of
/// exactly those two fields is [`AgentDefinition`], so the capability is expressed as
/// `Option<&AgentDefinition>` and answered by
/// [`crate::exec::completion_guard::has_mutation_tool_capability`].
///
/// `None` means "capabilities unknown", which upstream's `!capabilities ||
/// hasMutationToolCapability(...)` treats as write-capable — the direct-write instruction.
pub type OutputInstructionCapabilities<'a> = Option<&'a AgentDefinition>;

/// G82 — source: `formatOutputPathInstruction(outputPath, capabilities)`
/// (`pi-subagents/src/runs/shared/single-output.ts:84-97`). The shared body of BOTH injection
/// forms, and the reason this function exists rather than a single hard-coded string: its
/// `delivery` line has TWO branches.
///
/// ```text
/// const delivery = !capabilities || hasMutationToolCapability(capabilities.tools, capabilities.mcpDirectTools)
///     ? `Write your findings to exactly this path: ${outputPath}`
///     : [
///         "Return the complete artifact in your final response.",
///         `The runtime will persist it to exactly this path: ${outputPath}`,
///         "Do not call contact_supervisor merely because no write-capable tool is available.",
///     ].join("\n");
/// ```
///
/// The second branch is the one this crate was missing: an agent whose entire resolved tool
/// allowlist is read-only CANNOT write the file, so instructing it to do so produced a run that
/// either escalated to `contact_supervisor` (which the third line exists to forbid) or failed the
/// output handoff outright. The runtime persists the artifact for it instead — which
/// [`resolve_output_handoff`]'s `OrchestratorWrote` branch already does.
///
/// The two trailing lines are identical in both branches and are what make the path authoritative.
#[must_use]
pub fn format_output_path_instruction(
    output_path: &Path,
    capabilities: OutputInstructionCapabilities<'_>,
) -> String {
    let write_capable =
        capabilities.is_none_or(crate::exec::completion_guard::has_mutation_tool_capability);
    let delivery = if write_capable {
        format!(
            "Write your findings to exactly this path: {}",
            output_path.display()
        )
    } else {
        format!(
            "Return the complete artifact in your final response.\n\
             The runtime will persist it to exactly this path: {}\n\
             Do not call contact_supervisor merely because no write-capable tool is available.",
            output_path.display()
        )
    };
    format!(
        "{delivery}\n\
         This path is authoritative for this run.\n\
         Ignore any other output filename or output path mentioned elsewhere, including output \
         destinations in the base agent prompt, system prompt, or task instructions."
    )
}

/// R-SA-024 (MUST, file-only half) — build the authoritative output-path override instruction to
/// inject into the child's **system prompt** (not merely conveyed via argv), so the child's own
/// write-tool behavior is steered at generation time. Mirrors pi-subagents' own
/// `injectOutputPathSystemPrompt`'s instruction body
/// (`pi-subagents/src/runs/shared/single-output.ts:104-108`).
///
/// Returns `None` if no output path is configured (nothing to inject). `capabilities` selects the
/// delivery branch — see [`format_output_path_instruction`].
#[must_use]
pub fn build_output_path_system_prompt_instruction(
    output_path: Option<&Path>,
    capabilities: OutputInstructionCapabilities<'_>,
) -> Option<String> {
    let path = output_path?;
    Some(format!(
        "Runtime output path override:\n{}",
        format_output_path_instruction(path, capabilities)
    ))
}

/// R-SA-024 — append [`build_output_path_system_prompt_instruction`]'s instruction to an existing
/// system prompt body, if any output path is configured; otherwise returns `system_prompt`
/// unchanged. Mirrors pi-subagents' `injectOutputPathSystemPrompt`
/// (`pi-subagents/src/runs/shared/single-output.ts:104-108`).
///
/// # Where it is wired
///
/// `exec/mod.rs::build_attempt_spawn_plan` calls this on the persona body it is about to ship as
/// the child's `--system-prompt <spill path>` / `--append-system-prompt <spill path>` (SUBA-030),
/// immediately after
/// folding in `build_agent_memory_injection`'s block — which is exactly upstream's own composition
/// order, where `injectOutputPathSystemPrompt` is the statement following the memory fold
/// (`execution.ts:1433-1443`). Upstream's other call site, `api/preflight.ts:313`, applies the same
/// injector to the `effectiveSystemPrompt` of a `SubagentLaunchContract` PROJECTION rather than to
/// a run; cyrup has no port of that preflight/contract API surface at all (no `LaunchContract`
/// type exists in this crate), so there is nothing here for that second site to attach to. The two
/// upstream sites do not disagree — both put the instruction on the system prompt — so the one
/// cyrup surface that exists carries it.
///
/// This does NOT replace the task-side [`inject_single_output_instruction`]: upstream's foreground
/// single run gets BOTH, the task side from its caller (`subagent-executor.ts:3674`) and this one
/// from `execution.ts:1443`, and `exec/mod.rs` reproduces that pairing.
///
/// Composing the override here also retires the empty-body hazard rather than tripping over it:
/// the `--system-prompt` flag is emitted when the body is non-empty AFTER this injection, so an
/// output path makes an otherwise-empty `Replace`-mode persona ship the override (which is what
/// upstream ships too — `runs/shared/pi-args.ts:570-585` emits for any non-null string), while a run with no
/// output path and no persona still emits no flag and leaves the child's assembled prompt intact.
#[must_use]
pub fn inject_output_path_system_prompt(
    system_prompt: &str,
    output_path: Option<&Path>,
    capabilities: OutputInstructionCapabilities<'_>,
) -> String {
    let Some(instruction) = build_output_path_system_prompt_instruction(output_path, capabilities)
    else {
        return system_prompt.to_string();
    };
    if system_prompt.is_empty() {
        instruction
    } else {
        format!("{system_prompt}\n\n{instruction}")
    }
}

/// G82 — source: `injectSingleOutputInstruction(task, outputPath, capabilities)`
/// (`pi-subagents/src/runs/shared/single-output.ts:99-102`):
///
/// ```text
/// return `${task}\n\n---\n**Output:**\n${formatOutputPathInstruction(outputPath, capabilities)}`;
/// ```
///
/// The TASK-side sibling of [`inject_output_path_system_prompt`], and the LIVE one:
/// `exec/mod.rs::build_task_text` calls it for every run with a configured output path, mirroring
/// upstream's single-run site `subagent-executor.ts:3674` (`task = injectSingleOutputInstruction(
/// task, outputPath, agentConfig)`). Upstream keys it on the PATH alone at all five of its call
/// sites — `subagent-executor.ts:2979,3674` @v0.43.0, `chain-execution.ts:363,1320` @v0.43.0,
/// `async-execution.ts:711,1289` @v0.43.0 — never on `outputMode`, which is consulted only by
/// `validateFileOnlyOutputMode` and by delivery-side `finalizeSingleOutput`.
///
/// Its `**Output:**` header is one of the lines [`crate::exec::task_intent`]'s
/// `stripFrameworkInstructions` port removes before classification, so an injected output
/// instruction never contributes write-intent signal to the task it was appended to. That is the
/// concrete reason this function, and not the `Runtime output path override:`-prefixed
/// [`build_output_path_system_prompt_instruction`], is what belongs in the task text: the latter's
/// header is NOT one of `stripFrameworkInstructions`' alternatives.
#[must_use]
pub fn inject_single_output_instruction(
    task: &str,
    output_path: Option<&Path>,
    capabilities: OutputInstructionCapabilities<'_>,
) -> String {
    let Some(path) = output_path else {
        return task.to_string();
    };
    format!(
        "{task}\n\n---\n**Output:**\n{}",
        format_output_path_instruction(path, capabilities)
    )
}

// ============================================================================================
// G82: authorship from the CHILD'S OWN successful write
// (pi `extractChildWrittenOutput`, `single-output.ts:13-52`)
// ============================================================================================

/// G82 — source: `extractChildWrittenOutput(messages, outputPath, cwd)`
/// (`pi-subagents/src/runs/shared/single-output.ts:13-52`). Upstream's own doc comment states the
/// contract verbatim:
///
/// > Content the child itself sent to the configured output path, taken from its last `write` tool
/// > call whose tool result reports success. Unlike reading the path from disk, this cannot be
/// > polluted by a sibling run writing the same path (#420); requiring the successful tool result
/// > keeps failed, cancelled, or unanswered write calls from counting as authored output. Returns
/// > undefined when no such write exists (e.g. bash or edit-based construction), in which case
/// > callers must not assume file authorship.
///
/// This is the authorship signal [`resolve_output_handoff`] deliberately CANNOT provide: that
/// function is an mtime/size stat heuristic (its own doc says so), so any other process touching
/// `output_path` between the pre-spawn snapshot and the child's exit makes it report
/// `ChildWrote` for content the child never authored. The two are complementary and both are used:
/// the handoff decides what content is DELIVERED, this decides what content is ATTRIBUTED to the
/// child (and is therefore an admissible acceptance-report source, `acceptance.ts:755-771`).
///
/// # Wire-shape delta
///
/// Upstream walks a rich `Message[]`: assistant messages' `toolCall` content parts supply the
/// call id + `arguments`, and `role === "toolResult"` messages with `isError === false` supply the
/// success set. This crate's transcript is [`SubagentEvent`], where
/// [`SubagentEvent::ToolExecutionStart`] carries `tool_call_id`/`tool_name`/`args` and
/// [`SubagentEvent::ToolExecutionEnd`] carries `tool_call_id`/`is_error` — the same two facts,
/// keyed the same way. One difference is forced by the wire: `is_error` is a non-optional field
/// with `#[serde(default)]`, so cyrup has no "result present but success unknown" state to
/// distinguish from `is_error: false`; upstream's `isError === false` (rather than `!isError`)
/// exists to reject exactly that third state. The two behaviours upstream's strictness actually
/// buys — a FAILED result does not count, and a call with NO result at all does not count — are
/// both preserved here.
///
/// Path comparison resolves both sides against `cwd` (upstream `path.resolve(cwd ?? ".", ...)`),
/// and lowercases both on Windows (upstream's `process.platform === "win32"` branch).
#[must_use]
pub fn extract_child_written_output(
    events: &[SubagentEvent],
    output_path: Option<&Path>,
    cwd: &Path,
) -> Option<String> {
    let target = comparable_path(cwd, output_path?);

    // `for (const message of messages) if (message.role === "toolResult" && message.isError === false)`
    // — the success set is built in a FIRST full pass, so a result that arrives before its call in
    // the transcript still counts (upstream does not assume ordering either).
    let successful_call_ids: std::collections::HashSet<&str> = events
        .iter()
        .filter_map(|event| match event {
            SubagentEvent::ToolExecutionEnd {
                tool_call_id,
                is_error: false,
                ..
            } => Some(tool_call_id.as_str()),
            _ => None,
        })
        .collect();

    // `content = args.content` with NO early return: the LAST matching write wins, so a later
    // successful rewrite of the same path supersedes an earlier draft.
    let mut content: Option<String> = None;
    for event in events {
        let SubagentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } = event
        else {
            continue;
        };
        if tool_name != "write" || !successful_call_ids.contains(tool_call_id.as_str()) {
            continue;
        }
        // `typeof args.path !== "string" || typeof args.content !== "string"` → skip.
        let (Some(write_path), Some(write_content)) = (
            args.get("path").and_then(serde_json::Value::as_str),
            args.get("content").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        if comparable_path(cwd, Path::new(write_path)) != target {
            continue;
        }
        content = Some(write_content.to_string());
    }
    content
}

/// `path.resolve(cwd ?? ".", p)` plus the Windows case-folding
/// (`single-output.ts:28-29,45-46`). `Path::join` reproduces `path.resolve`'s "an absolute second
/// argument replaces the base" rule; the result is normalized only in the ways `path.resolve` is
/// (it does not touch the filesystem, so a symlink is not resolved on either side).
fn comparable_path(cwd: &Path, path: &Path) -> std::path::PathBuf {
    let resolved = normalize_dot_segments(&cwd.join(path));
    if cfg!(windows) {
        std::path::PathBuf::from(resolved.to_string_lossy().to_lowercase())
    } else {
        resolved
    }
}

/// The `.`/`..` collapsing half of `path.resolve` — purely lexical, exactly like Node's, so a
/// child that wrote `./reports/../reports/out.md` still matches a configured `reports/out.md`.
fn normalize_dot_segments(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

// ============================================================================================
// R-SA-042: UTF-8-safe output truncation
// ============================================================================================

/// The byte/line cap applied to a delivered final output (R-SA-042). Default parity with
/// pi-subagents (`DEFAULT_MAX_OUTPUT`, `pi-subagents/src/shared/types.ts:1791-1794` @v0.43.0): 200KB / 5000
/// lines. An agent's own `AgentConfig::max_output` (arch-SA §3.4) may override either field
/// independently; that layering/resolution is a later phase's concern (`exec/mod.rs`) — this type
/// is the narrow value this module's [`truncate_output`] actually consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputCap {
    /// Maximum delivered output size, in bytes.
    pub bytes: usize,
    /// Maximum delivered output size, in lines.
    pub lines: usize,
}

impl Default for OutputCap {
    fn default() -> Self {
        // R-SA-042: "default parity: 200KB / 5000 lines".
        Self {
            bytes: 200 * 1024,
            lines: 5000,
        }
    }
}

/// The result of applying [`truncate_output`] to some text (R-SA-042). `text` is always what
/// should actually be delivered — callers never need to branch on `truncated` to decide which
/// field to use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncationResult {
    /// The (possibly truncated) text to deliver. Equal to the original input iff `!truncated`.
    pub text: String,
    /// Whether truncation actually occurred — the "truncation fact recorded" this file's task
    /// brief requires.
    pub truncated: bool,
    /// The original, pre-truncation size in bytes. Only meaningful when `truncated`.
    pub original_bytes: usize,
    /// The original, pre-truncation line count. Only meaningful when `truncated`.
    pub original_lines: usize,
}

/// Format a byte count as a short human-readable size (`"12.3KB"`), matching pi-subagents'
/// `formatBytes` (`pi-subagents/src/shared/types.ts:1981-1985` @v0.43.0) closely enough for the truncation
/// marker text to read the same way; not a general-purpose formatter used elsewhere.
fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// The largest byte-length prefix of `s` that both (a) is at most `max_bytes` long and (b) falls
/// on a UTF-8 character boundary — never splitting a multi-byte codepoint (this file's explicit
/// test obligation). Walks `max_bytes` down to the nearest valid boundary via
/// `str::is_char_boundary` rather than a byte-slice-then-`from_utf8_lossy` approach, so the result
/// is guaranteed valid UTF-8 with no replacement-character insertion at the cut point.
fn utf8_safe_prefix(s: &str, max_bytes: usize) -> &str {
    if max_bytes >= s.len() {
        return s;
    }
    let mut cut = max_bytes;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.get(..cut).unwrap_or("")
}

/// R-SA-042 (MUST) — truncate `output` against `cap`'s byte/line budget, UTF-8-boundary-safe,
/// recording the truncation fact.
///
/// Line-cap is applied first (keep only the first `cap.lines` lines), then the byte-cap is applied
/// to whatever remains — mirroring pi-subagents' own `truncateOutput`
/// (`pi-subagents/src/shared/types.ts:1987-2029` @v0.43.0) ordering exactly: a huge single first line still
/// gets byte-truncated even though it is only "1 line", and a many-short-lines output that already
/// fits under the byte cap after line-truncation is not further cut. If neither cap is exceeded,
/// `output` is returned unchanged and `truncated` is `false`.
///
/// A leading `[TRUNCATED: showing first N of M lines, X of Y - full output at <path>]\n` marker
/// (the `artifact_path` clause included only when `artifact_path` is `Some`) is prepended when
/// truncation occurs, matching pi-subagents' own marker text closely enough to be recognizable in
/// context while staying a plain Rust `format!` rather than a port of its exact template engine.
#[must_use]
pub fn truncate_output(
    output: &str,
    cap: OutputCap,
    artifact_path: Option<&Path>,
) -> TruncationResult {
    let original_bytes = output.len();
    let original_lines = count_lines(output);

    if original_bytes <= cap.bytes && original_lines <= cap.lines {
        return TruncationResult {
            text: output.to_string(),
            truncated: false,
            original_bytes,
            original_lines,
        };
    }

    // Line cap first: keep only the first `cap.lines` lines (splitting on '\n', matching
    // pi-subagents' own `output.split("\n")`/`.slice(0, config.lines)`/`.join("\n")`).
    let mut kept: String = if original_lines > cap.lines {
        output
            .split('\n')
            .take(cap.lines)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        output.to_string()
    };

    // Byte cap second, applied to whatever the line-cap step left, UTF-8-boundary-safe.
    if kept.len() > cap.bytes {
        kept = utf8_safe_prefix(&kept, cap.bytes).to_string();
    }

    let kept_lines = count_lines(&kept);
    let kept_bytes = kept.len();
    let artifact_suffix = artifact_path
        .map(|p| format!(" - full output at {}", p.display()))
        .unwrap_or_default();
    let marker = format!(
        "[TRUNCATED: showing first {kept_lines} of {original_lines} lines, \
         {} of {}{artifact_suffix}]\n",
        format_bytes(kept_bytes),
        format_bytes(original_bytes),
    );

    TruncationResult {
        text: format!("{marker}{kept}"),
        truncated: true,
        original_bytes,
        original_lines,
    }
}

/// Count lines the same way pi-subagents' `countLines` does
/// (`pi-subagents/src/runs/shared/single-output.ts:110-114`): the number of newline separators, plus
/// one more unless the text ends in a newline — i.e. "how many lines would `output.split('\n')`
/// produce", which for an empty string is `0` (a genuine empty document has zero lines, not one).
fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.split('\n').count()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    // ---- looks_like_acceptance_report ----

    #[test]
    fn detects_fenced_acceptance_report_language_tag() {
        let text = "Here is my answer.\n```acceptance-report\n{\"ok\": true}\n```\n";
        assert!(looks_like_acceptance_report(text));
    }

    #[test]
    fn detects_json_block_with_criteria_satisfied_and_companion_key() {
        let text =
            "Done.\n```json\n{\"criteriaSatisfied\": true, \"changedFiles\": [\"a.rs\"]}\n```\n";
        assert!(looks_like_acceptance_report(text));
    }

    #[test]
    fn json_block_missing_companion_key_does_not_count() {
        let text = "Done.\n```json\n{\"criteriaSatisfied\": true}\n```\n";
        assert!(!looks_like_acceptance_report(text));
    }

    #[test]
    fn detects_acceptance_report_marker_outside_fence() {
        let text = "Summary text.\nACCEPTANCE_REPORT: all good.";
        assert!(looks_like_acceptance_report(text));
    }

    #[test]
    fn plain_text_is_not_acceptance_report_shaped() {
        assert!(!looks_like_acceptance_report("Just a normal answer."));
        assert!(!looks_like_acceptance_report("```rust\nfn main() {}\n```"));
    }

    /// G79 widened the acceptance-report PARSER to accept the `acceptance_report` fence tag and a
    /// snake_case alias for every field (`acceptance.ts:702`, `:486-508` @v0.43.0). This probe is
    /// what the LIVE lattice gate's self-report floor consults (`acceptance.rs::self_report_floor`),
    /// so if it keeps the pre-G79 spellings the two disagree: a child emits a report the parser
    /// accepts in full, the floor scores it `not-required`, and the run is REJECTED for missing an
    /// attestation it actually produced — the exact failure G79 exists to remove.
    #[test]
    fn the_probe_accepts_every_spelling_g79_taught_the_parser() {
        // The underscore fence tag.
        assert!(looks_like_acceptance_report(
            "Done.\n```acceptance_report\n{\"ok\": true}\n```\n"
        ));
        // snake_case on BOTH the marker key and the companion key.
        assert!(looks_like_acceptance_report(
            "Done.\n```json\n{\"criteria_satisfied\": true, \"changed_files\": [\"a.rs\"]}\n```\n"
        ));
        // Mixed spellings across the pair still count.
        assert!(looks_like_acceptance_report(
            "Done.\n```json\n{\"criteriaSatisfied\": true, \"tests_added_or_updated\": [\"t.rs\"]}\n```\n"
        ));

        // The snake_case aliases are derived mechanically; assert they reproduce upstream's
        // `ACCEPTANCE_REPORT_FIELDS` table entry-for-entry rather than by a second hardcoded list.
        assert_eq!(snake_case_alias("criteriaSatisfied"), "criteria_satisfied");
        assert_eq!(
            snake_case_alias("testsAddedOrUpdated"),
            "tests_added_or_updated"
        );
        assert_eq!(snake_case_alias("noStagedFiles"), "no_staged_files");
        assert_eq!(snake_case_alias("commandsRun"), "commands_run");
        assert_eq!(snake_case_alias("manualNotes"), "manual_notes");

        // Widening spellings must not widen the RULE: a marker key with no companion key still
        // does not count, in either spelling.
        assert!(!looks_like_acceptance_report(
            "Done.\n```json\n{\"criteria_satisfied\": true}\n```\n"
        ));
    }

    // ---- extract_final_output: reverse-scan priority ordering ----

    fn message_end(role: &str, texts: &[&str]) -> SubagentEvent {
        let content: Vec<serde_json::Value> = texts
            .iter()
            .map(|t| serde_json::json!({"type": "text", "text": t}))
            .collect();
        SubagentEvent::MessageEnd {
            message: serde_json::json!({"role": role, "content": content}),
        }
    }

    fn message_end_error(texts: &[&str]) -> SubagentEvent {
        let content: Vec<serde_json::Value> = texts
            .iter()
            .map(|t| serde_json::json!({"type": "text", "text": t}))
            .collect();
        SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": content,
                "stopReason": "error",
                "errorMessage": "boom",
            }),
        }
    }

    #[test]
    fn falls_back_to_last_non_empty_text_when_no_acceptance_report_present() {
        let events = vec![
            message_end("assistant", &["first message"]),
            message_end("assistant", &["second message", "  ", "third and last"]),
        ];
        let out = extract_final_output(&events);
        assert_eq!(out.as_deref(), Some("third and last"));
    }

    #[test]
    fn prefers_acceptance_report_shaped_part_over_later_plain_text_in_same_message() {
        // Within ONE message, an earlier acceptance-report part beats a later plain part.
        let events = vec![message_end(
            "assistant",
            &[
                "```acceptance-report\n{\"criteriaSatisfied\": true}\n```",
                "some trailing plain remark",
            ],
        )];
        let out = extract_final_output(&events).expect("must extract");
        assert!(out.contains("acceptance-report"));
    }

    #[test]
    fn prefers_earlier_message_acceptance_report_over_later_message_plain_text() {
        // R-SA-029's core claim: reverse-scan priority means a chronologically EARLIER message's
        // acceptance-report-shaped text wins over a chronologically LATER message's plain text,
        // as long as no later message ALSO has an acceptance-report shape.
        //
        // The scan is newest-first and returns on the first acceptance-report hit; since the last
        // message has no acceptance-report shape, the scan continues past it to the earlier
        // message that does have one.
        let events = vec![
            message_end(
                "assistant",
                &["```acceptance-report\n{\"criteriaSatisfied\": true}\n```"],
            ),
            message_end("assistant", &["a later, purely plain-text message"]),
        ];
        let out = extract_final_output(&events).expect("must extract");
        assert!(
            out.contains("acceptance-report"),
            "expected the earlier acceptance-report message to win, got: {out}"
        );
    }

    #[test]
    fn multiple_candidate_segments_most_recent_acceptance_report_wins() {
        // Two messages BOTH carry an acceptance-report shape; the reverse scan must return the
        // one from the MORE RECENT message (message recency is the outer priority level).
        let events = vec![
            message_end("assistant", &["```acceptance-report\nOLDER\n```"]),
            message_end("assistant", &["unrelated plain text"]),
            message_end("assistant", &["```acceptance-report\nNEWEST\n```"]),
        ];
        let out = extract_final_output(&events).expect("must extract");
        assert!(out.contains("NEWEST"), "got: {out}");
        assert!(!out.contains("OLDER"), "got: {out}");
    }

    #[test]
    fn skips_error_flagged_messages_entirely() {
        let events = vec![
            message_end("assistant", &["good earlier answer"]),
            message_end_error(&["this must never be returned"]),
        ];
        let out = extract_final_output(&events);
        assert_eq!(out.as_deref(), Some("good earlier answer"));
    }

    #[test]
    fn skips_empty_and_whitespace_only_text_parts() {
        let events = vec![message_end("assistant", &["real content", "   ", "\n\t"])];
        let out = extract_final_output(&events);
        assert_eq!(out.as_deref(), Some("real content"));
    }

    #[test]
    fn ignores_non_assistant_message_end_events() {
        let events = vec![
            message_end("assistant", &["the only valid answer"]),
            message_end("user", &["a user echo, must be ignored"]),
        ];
        let out = extract_final_output(&events);
        assert_eq!(out.as_deref(), Some("the only valid answer"));
    }

    #[test]
    fn ignores_non_message_end_events() {
        let events = vec![
            SubagentEvent::AgentStart,
            message_end("assistant", &["the answer"]),
            SubagentEvent::ToolExecutionStart {
                tool_call_id: "c1".into(),
                tool_name: "bash".to_string(),
                args: serde_json::Value::Null,
            },
        ];
        let out = extract_final_output(&events);
        assert_eq!(out.as_deref(), Some("the answer"));
    }

    #[test]
    fn no_messages_at_all_returns_none() {
        assert_eq!(extract_final_output(&[]), None);
    }

    #[test]
    fn only_error_messages_returns_none() {
        let events = vec![message_end_error(&["never returned"])];
        assert_eq!(extract_final_output(&events), None);
    }

    // ---- detect_subagent_error / trailing_assistant_error / terminal-stop helpers ----

    fn tool_result_end(tool_name: &str, text: &str, is_error: bool) -> SubagentEvent {
        // cyrup's real tool-result wire shape (`agent.rs:113-115`):
        // `{"content":[{"type":"text","text":…}],"details":null,"terminate":false}`.
        SubagentEvent::ToolExecutionEnd {
            tool_call_id: "tc".into(),
            tool_name: tool_name.to_string(),
            result: serde_json::json!({
                "content": [{"type": "text", "text": text}],
                "details": serde_json::Value::Null,
                "terminate": false
            }),
            is_error,
        }
    }

    fn message_end_with_error(text: &str, error_message: &str) -> SubagentEvent {
        SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "stopReason": "error",
                "errorMessage": error_message,
            }),
        }
    }

    /// A clean terminal assistant stop (`stopReason: "stop"`, no `errorMessage`) — the real wire
    /// shape (`message_end_line` in `tests/exec_run_sync_integration.rs`; pi `events.assistantMessage`).
    /// The bare `message_end` helper above deliberately omits `stopReason` (it exists for
    /// extract_final_output tests that do not care), so a terminal-stop assertion must build the
    /// stop explicitly.
    fn assistant_stop(text: &str) -> SubagentEvent {
        SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": [{"type": "text", "text": text}],
                "stopReason": "stop",
            }),
        }
    }

    #[test]
    fn parse_exit_code_matches_pi_regex_shapes() {
        assert_eq!(parse_exit_code("process exited with code 127"), Some(127));
        assert_eq!(parse_exit_code("exit 1"), Some(1));
        assert_eq!(parse_exit_code("exit code: 2"), Some(2));
        assert_eq!(parse_exit_code("exit status 3"), Some(3));
        assert_eq!(parse_exit_code("exited with code 0"), Some(0));
        assert_eq!(parse_exit_code("no numeric code here"), None);
        assert_eq!(parse_exit_code("exiting the loop cleanly"), None);
    }

    #[test]
    fn detect_subagent_error_flags_a_trailing_nonzero_bash_exit() {
        // pi "does not retry on ordinary task/tool failures": a bash result reporting exit 127,
        // with NO later assistant text, is a run failure diagnosed at exit code 127.
        let events = vec![tool_result_end(
            "bash",
            "process exited with code 127",
            false,
        )];
        let detected = detect_subagent_error(&events).expect("must diagnose a failure");
        assert_eq!(detected.exit_code, 127);
        assert_eq!(detected.error_type, "bash");
        assert_eq!(
            detected.message(),
            "bash failed (exit 127): process exited with code 127"
        );
    }

    /// The joint the `TOOL_FAILURE_PREFIX` guard exists for, asserted across the two modules that
    /// form it: this module MINTS the `<tool> failed …` message that `exec::fallback` then
    /// classifies. `FATAL_BASH_PATTERNS` above and `RETRYABLE_MODEL_FAILURE_PATTERNS` share
    /// `"connection refused"` and `"timeout"` verbatim, so without the guard a failed tool whose
    /// own output mentions either would re-run the child's WHOLE task on the next model.
    #[test]
    fn a_re_diagnosed_tool_failure_is_never_classified_as_a_retryable_model_failure() {
        for (tool, text) in [
            (
                "bash",
                "curl: (7) Failed to connect to api.test: Connection refused",
            ),
            ("bash", "timeout: sending signal TERM to command, exit 124"),
            ("mcp.server/write", "quota exceeded, exit code: 3"),
        ] {
            let events = vec![tool_result_end(tool, text, true)];
            let detected = detect_subagent_error(&events).expect("must diagnose a failure");
            let message = detected.message();

            // Half one: the DETAILS this module put in the message do match the retryable set —
            // that is what made the misclassification possible in the first place.
            assert!(
                crate::exec::fallback::is_retryable_model_failure(Some(text)),
                "sanity check: the tool's own output text matches a retryable pattern — {text}"
            );
            // Half two: the assembled `<tool> failed …` message does NOT.
            assert!(
                !crate::exec::fallback::is_retryable_model_failure(Some(&message)),
                "a failed TOOL must not spend another model attempt — {message}"
            );
        }
    }

    #[test]
    fn detect_subagent_error_flags_an_explicit_is_error_tool_result() {
        let events = vec![tool_result_end("read", "EISDIR: illegal operation", true)];
        let detected = detect_subagent_error(&events).expect("must diagnose a failure");
        assert_eq!(detected.exit_code, 1); // no numeric code in the text
        assert_eq!(detected.error_type, "read");
    }

    #[test]
    fn detect_subagent_error_flags_a_fatal_bash_pattern_without_a_code() {
        let events = vec![tool_result_end(
            "bash",
            "bash: frobnicate: command not found",
            false,
        )];
        let detected = detect_subagent_error(&events).expect("must diagnose a fatal pattern");
        assert_eq!(detected.exit_code, 1);
        assert_eq!(detected.error_type, "bash");
    }

    #[test]
    fn detect_subagent_error_ignores_a_tool_error_the_assistant_recovered_from() {
        // pi "treats recovered child tool errors as successful": the tool error precedes the last
        // assistant text, so it is BEFORE scan_start and must not be diagnosed.
        let events = vec![
            tool_result_end("read", "EISDIR: illegal operation", true),
            message_end("assistant", &["Done"]),
        ];
        assert_eq!(detect_subagent_error(&events), None);
    }

    #[test]
    fn detect_subagent_error_ignores_a_recovered_zero_exit_bash_result() {
        let events = vec![
            tool_result_end("bash", "ran fine, exit 0", false),
            message_end("assistant", &["all good"]),
        ];
        assert_eq!(detect_subagent_error(&events), None);
    }

    #[test]
    fn trailing_assistant_error_is_cleared_by_a_clean_recovering_stop() {
        // pi "treats recovered assistant provider errors as successful": errorMessage then a clean
        // terminal stop with real text clears it.
        let events = vec![
            message_end_with_error("temporary provider failure", "provider transport failed"),
            assistant_stop("Recovered"),
        ];
        assert_eq!(trailing_assistant_error(&events), None);
    }

    #[test]
    fn trailing_assistant_error_survives_an_empty_stop() {
        // pi "keeps provider errors failed when followed only by empty assistant output": a clean
        // terminal stop with EMPTY text does NOT clear the error (the clear requires real text).
        let events = vec![
            message_end_with_error("temporary provider failure", "provider transport failed"),
            assistant_stop(""),
        ];
        assert_eq!(
            trailing_assistant_error(&events).as_deref(),
            Some("provider transport failed")
        );
    }

    #[test]
    fn is_terminal_assistant_stop_requires_stop_reason_and_no_tool_call() {
        assert!(is_terminal_assistant_stop(&assistant_stop("done")));
        // A message with no explicit stopReason is NOT a terminal stop.
        assert!(!is_terminal_assistant_stop(&message_end(
            "assistant",
            &["done"]
        )));
        let with_tool_call = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant",
                "content": [{"type": "toolCall", "name": "edit"}],
                "stopReason": "stop",
            }),
        };
        assert!(!is_terminal_assistant_stop(&with_tool_call));
        assert!(!is_terminal_assistant_stop(&message_end_error(&["boom"])));
    }

    #[test]
    fn message_end_has_error_message_is_a_truthy_test() {
        assert!(message_end_has_error_message(&message_end_with_error(
            "x", "boom"
        )));
        assert!(!message_end_has_error_message(&message_end(
            "assistant",
            &["ok"]
        )));
        let empty_error = SubagentEvent::MessageEnd {
            message: serde_json::json!({
                "role": "assistant", "content": [], "errorMessage": ""
            }),
        };
        assert!(!message_end_has_error_message(&empty_error));
    }

    /// SUBA-089 — `message_error_messages` mirrors pi's `messageError` over `result.messages`:
    /// every `message_end` regardless of role, raw (untrimmed, empty kept), non-strings and
    /// non-message events skipped, order preserved.
    #[test]
    fn message_error_messages_collects_every_message_end_error_message_untrimmed() {
        let events = vec![
            message_end("assistant", &["thinking"]),
            message_end_with_error("x", "  overloaded \n"),
            SubagentEvent::MessageEnd {
                message: serde_json::json!({
                    "role": "toolResult", "content": [], "errorMessage": "tool boom"
                }),
            },
            SubagentEvent::MessageEnd {
                message: serde_json::json!({
                    "role": "assistant", "content": [], "errorMessage": 42
                }),
            },
            SubagentEvent::MessageEnd {
                message: serde_json::json!({
                    "role": "assistant", "content": [], "errorMessage": ""
                }),
            },
            SubagentEvent::ToolExecutionEnd {
                tool_call_id: "c1".into(),
                tool_name: "bash".to_string(),
                result: serde_json::json!({"errorMessage": "not a message"}),
                is_error: true,
            },
        ];
        assert_eq!(
            message_error_messages(&events),
            vec![
                "  overloaded \n".to_string(),
                "tool boom".to_string(),
                String::new()
            ]
        );
        assert!(message_error_messages(&[]).is_empty());
    }

    // ---- OutputCap / truncate_output: UTF-8 boundary safety ----

    #[test]
    fn no_truncation_when_within_both_caps() {
        let cap = OutputCap {
            bytes: 1000,
            lines: 100,
        };
        let result = truncate_output("short output\nsecond line", cap, None);
        assert!(!result.truncated);
        assert_eq!(result.text, "short output\nsecond line");
    }

    #[test]
    fn default_output_cap_matches_pi_parity_200kb_5000_lines() {
        let cap = OutputCap::default();
        assert_eq!(cap.bytes, 200 * 1024);
        assert_eq!(cap.lines, 5000);
    }

    #[test]
    fn truncates_on_line_count_and_records_the_fact() {
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let text = lines.join("\n");
        let cap = OutputCap {
            bytes: 10_000,
            lines: 5,
        };
        let result = truncate_output(&text, cap, None);
        assert!(result.truncated);
        assert_eq!(result.original_lines, 20);
        assert!(result.text.starts_with("[TRUNCATED:"));
        // Exactly the first 5 lines must be kept after the marker.
        let body = result.text.split_once('\n').map_or("", |(_, rest)| rest);
        let kept: Vec<&str> = body.split('\n').collect();
        assert_eq!(kept, vec!["line 0", "line 1", "line 2", "line 3", "line 4"]);
    }

    #[test]
    fn truncates_on_byte_count_never_splitting_a_multibyte_character() {
        // Each "é" is 2 UTF-8 bytes; construct a string whose natural byte-cap cut point would
        // land mid-character if done naively, and assert the result is still valid UTF-8 with no
        // truncated/garbled trailing character.
        let text: String = std::iter::repeat_n('é', 100).collect();
        assert_eq!(text.len(), 200); // 100 * 2 bytes

        let cap = OutputCap {
            bytes: 55, // deliberately odd relative to the 2-byte character width
            lines: 100,
        };
        let result = truncate_output(&text, cap, None);
        assert!(result.truncated);

        // The whole delivered text must be valid UTF-8 (guaranteed by the `String` type itself,
        // but the real assertion is that the KEPT body has no dangling half-character): strip the
        // marker line and confirm the body's char count times 2 equals its byte length (i.e. no
        // partial trailing codepoint was kept).
        let (_, body) = result.text.split_once('\n').expect("marker line present");
        assert_eq!(body.len(), body.chars().count() * 2);
        assert!(body.chars().all(|c| c == 'é'));
    }

    #[test]
    fn truncates_on_byte_count_with_mixed_ascii_and_multibyte_content() {
        // A realistic mixed-width string: multi-byte emoji interleaved with ASCII, and a byte
        // cap chosen to land exactly inside one of the multi-byte sequences if cut naively.
        let text = "abc 🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉 xyz".to_string(); // 🎉 is 4 bytes each
        let cap = OutputCap {
            bytes: 10,
            lines: 10,
        };
        let result = truncate_output(&text, cap, None);
        assert!(result.truncated);
        let (_, body) = result.text.split_once('\n').expect("marker line present");
        // Must be valid UTF-8 (implicit: this compiles and holds a `String`) and must not exceed
        // the byte cap.
        assert!(body.len() <= cap.bytes);
        // Every character kept must be a COMPLETE character, never a partial one — re-encoding
        // every char and summing byte lengths must equal the body's own byte length.
        let reencoded_len: usize = body.chars().map(char::len_utf8).sum();
        assert_eq!(reencoded_len, body.len());
    }

    #[test]
    fn truncation_marker_names_artifact_path_when_provided() {
        let text = "x".repeat(200);
        let cap = OutputCap {
            bytes: 10,
            lines: 100,
        };
        let path = Path::new("/tmp/full-output.txt");
        let result = truncate_output(&text, cap, Some(path));
        assert!(result.text.contains("full output at /tmp/full-output.txt"));
    }

    #[test]
    fn empty_output_is_never_truncated() {
        let cap = OutputCap { bytes: 0, lines: 0 };
        let result = truncate_output("", cap, None);
        assert!(!result.truncated);
        assert_eq!(result.text, "");
        assert_eq!(result.original_lines, 0);
    }

    // ---- File-only handoff: real filesystem stat-snapshot behavior ----

    #[test]
    fn validate_file_only_requires_path_fails_fast_when_missing() {
        let err =
            validate_file_only_requires_path(crate::discovery::types::OutputMode::FileOnly, None);
        assert!(matches!(
            err,
            Some(crate::error::SubagentError::OutputPathRequired)
        ));
    }

    #[test]
    fn validate_file_only_requires_path_passes_when_present() {
        let path = Path::new("/tmp/whatever.txt");
        let err = validate_file_only_requires_path(
            crate::discovery::types::OutputMode::FileOnly,
            Some(path),
        );
        assert!(err.is_none());
    }

    #[test]
    fn validate_file_only_requires_path_is_a_no_op_for_other_modes() {
        let err =
            validate_file_only_requires_path(crate::discovery::types::OutputMode::Inline, None);
        assert!(err.is_none());
    }

    #[test]
    fn snapshot_of_nonexistent_path_reports_not_existed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("does-not-exist.txt");
        let snap = snapshot_output_file(Some(&path)).expect("Some for a configured path");
        assert!(!snap.existed());
    }

    #[test]
    fn snapshot_of_none_path_returns_none() {
        assert!(snapshot_output_file(None).is_none());
    }

    #[test]
    fn handoff_detects_and_reads_back_a_childs_own_write() {
        // Real filesystem I/O, no mocks (crate testing convention): snapshot a not-yet-existing
        // path, then simulate "the child wrote its own output" by writing the file AFTER the
        // snapshot was taken, then resolve the handoff.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("output.txt");

        let before = snapshot_output_file(Some(&path)).expect("Some for a configured path");
        assert!(!before.existed());

        std::fs::write(&path, "the child's own real content").expect("child writes its file");

        let handoff = resolve_output_handoff(&path, "orchestrator's captured text", Some(before));
        assert!(
            matches!(&handoff, OutputHandoff::ChildWrote { .. }),
            "expected ChildWrote, got {handoff:?}"
        );
        if let OutputHandoff::ChildWrote { content } = handoff {
            assert_eq!(content, "the child's own real content");
        }
    }

    #[test]
    fn handoff_detects_change_via_mtime_or_size_when_file_pre_existed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("output.txt");
        std::fs::write(&path, "pre-existing stale content").expect("initial write");

        let before = snapshot_output_file(Some(&path)).expect("Some for a configured path");
        assert!(before.existed());

        // Ensure a real, observable mtime delta on filesystems with coarse mtime resolution.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, "the child overwrote it with new content")
            .expect("child overwrites file");

        let handoff = resolve_output_handoff(&path, "orchestrator's captured text", Some(before));
        assert!(
            matches!(&handoff, OutputHandoff::ChildWrote { .. }),
            "expected ChildWrote, got {handoff:?}"
        );
        if let OutputHandoff::ChildWrote { content } = handoff {
            assert_eq!(content, "the child overwrote it with new content");
        }
    }

    #[test]
    fn handoff_persists_orchestrators_output_when_file_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("output.txt");
        std::fs::write(&path, "unchanged content").expect("initial write");

        let before = snapshot_output_file(Some(&path)).expect("Some for a configured path");

        // The child does NOT touch the file at all.
        let handoff = resolve_output_handoff(&path, "orchestrator's captured text", Some(before));
        assert!(
            matches!(&handoff, OutputHandoff::OrchestratorWrote { .. }),
            "expected OrchestratorWrote, got {handoff:?}"
        );
        if let OutputHandoff::OrchestratorWrote { written, error } = handoff {
            assert!(written, "persist should succeed: {error:?}");
            assert!(error.is_none());
        }
        let on_disk = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(on_disk, "orchestrator's captured text");
    }

    #[test]
    fn handoff_persists_orchestrators_output_when_path_never_existed_and_child_never_created_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("output.txt");

        let before = snapshot_output_file(Some(&path)).expect("Some for a configured path");
        assert!(!before.existed());

        // Nothing writes the file — orchestrator must create parent dirs and persist its own
        // output.
        let handoff = resolve_output_handoff(&path, "orchestrator's captured text", Some(before));
        assert!(
            matches!(&handoff, OutputHandoff::OrchestratorWrote { .. }),
            "expected OrchestratorWrote, got {handoff:?}"
        );
        if let OutputHandoff::OrchestratorWrote { written, error } = handoff {
            assert!(written, "persist should succeed: {error:?}");
        }
        assert_eq!(
            std::fs::read_to_string(&path).expect("read back"),
            "orchestrator's captured text"
        );
    }

    #[test]
    fn handoff_with_no_snapshot_assumes_changed_and_reads_back_existing_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("output.txt");
        std::fs::write(&path, "already there before this attempt began").expect("initial write");

        // No `before` snapshot at all (`None`) — per doc comment, "assume changed" so existing
        // on-disk content is preferred over blindly overwriting it.
        let handoff = resolve_output_handoff(&path, "orchestrator's captured text", None);
        assert!(
            matches!(&handoff, OutputHandoff::ChildWrote { .. }),
            "expected ChildWrote, got {handoff:?}"
        );
        if let OutputHandoff::ChildWrote { content } = handoff {
            assert_eq!(content, "already there before this attempt began");
        }
    }

    #[test]
    fn handoff_uses_no_lock_concurrent_snapshot_and_resolve_do_not_block() {
        // R-SA-031: "The orchestrator MUST NOT use a lock." This test's only real assertion is
        // that snapshot + resolve against the SAME path from what simulates two independent
        // "attempts" never blocks or errors due to any lock contention — both complete
        // immediately. There is deliberately no file-locking primitive anywhere in this module to
        // even test against; this documents the absence as a behavioral property.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shared-output.txt");

        let snap_a = snapshot_output_file(Some(&path));
        let snap_b = snapshot_output_file(Some(&path));
        std::fs::write(&path, "concurrent-ish write").expect("write");
        let handoff_a = resolve_output_handoff(&path, "a", snap_a);
        let handoff_b = resolve_output_handoff(&path, "b", snap_b);
        // Both resolve without blocking/panicking; exact winner is intentionally unspecified
        // (accepted footgun per R-SA-031's own text), so only structural success is asserted.
        assert!(matches!(
            handoff_a,
            OutputHandoff::ChildWrote { .. } | OutputHandoff::OrchestratorWrote { .. }
        ));
        assert!(matches!(
            handoff_b,
            OutputHandoff::ChildWrote { .. } | OutputHandoff::OrchestratorWrote { .. }
        ));
    }

    // ---- R-SA-024: output-path system-prompt injection ----

    #[test]
    fn builds_output_path_instruction_when_path_present() {
        let path = Path::new("/work/out.md");
        let instruction = build_output_path_system_prompt_instruction(Some(path), None)
            .expect("Some for a configured path");
        assert!(instruction.contains("/work/out.md"));
        assert!(instruction.contains("authoritative"));
    }

    #[test]
    fn no_instruction_built_when_no_output_path() {
        assert!(build_output_path_system_prompt_instruction(None, None).is_none());
    }

    #[test]
    fn injects_instruction_after_existing_system_prompt() {
        let path = Path::new("/work/out.md");
        let merged = inject_output_path_system_prompt("You are a helpful agent.", Some(path), None);
        assert!(merged.starts_with("You are a helpful agent."));
        assert!(merged.contains("/work/out.md"));
    }

    #[test]
    fn injects_instruction_alone_when_system_prompt_empty() {
        let path = Path::new("/work/out.md");
        let merged = inject_output_path_system_prompt("", Some(path), None);
        assert!(merged.contains("/work/out.md"));
        assert!(!merged.starts_with('\n'));
    }

    #[test]
    fn system_prompt_unchanged_when_no_output_path() {
        let merged = inject_output_path_system_prompt("unchanged", None, None);
        assert_eq!(merged, "unchanged");
    }

    // ============================================================================================
    // G82: capability-aware output instruction + child-write authorship.
    // Cases transcribed from `pi-subagents:v0.43.0:test/unit/single-output.test.ts`.
    // ============================================================================================

    fn capability_agent(tools: Option<Vec<crate::discovery::types::ToolRef>>) -> AgentDefinition {
        AgentDefinition {
            default_turn_budget: None,
            default_acceptance: None,
            acceptance_role: None,
            permission_rules: None,
            runner: None,
            name: "cap".to_string(),
            local_name: "cap".to_string(),
            package_name: None,
            description: "capability probe agent".to_string(),
            aliases: Vec::new(),
            tools,
            extensions: None,
            extensions_from_default: false,
            subagent_only_extensions: Vec::new(),
            exclude_tools: None,
            allow_nested_subagents: None,
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: crate::discovery::types::SystemPromptMode::Replace,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            default_reads: None,
            default_progress: None,
            output: None,
            completion_guard: None,
            interactive: None,
            max_subagent_depth: None,
            default_context: None,
            default_async: None,
            default_timeout_ms: None,
            memory: None,
            tool_budget: None,
            disabled: None,
            system_prompt_body: String::new(),
            source: crate::discovery::types::AgentSource::User,
            file_path: std::path::PathBuf::from("/tmp/agent.md"),
            present_fields: std::collections::HashSet::new(),
            extra_fields: std::collections::BTreeMap::new(),
            override_info: None,
            model_source: None,
            model_provider: None,
        }
    }

    fn builtin(names: &[&str]) -> Option<Vec<crate::discovery::types::ToolRef>> {
        Some(
            names
                .iter()
                .map(|n| crate::discovery::types::ToolRef::Builtin((*n).to_string()))
                .collect(),
        )
    }

    /// `single-output.test.ts:85-90` — "appends direct-write instructions for mutation-capable
    /// agents".
    #[test]
    fn a_mutation_capable_agent_is_told_to_write_the_file_itself() {
        let agent = capability_agent(builtin(&["read", "write"]));
        let output = inject_single_output_instruction(
            "Analyze this",
            Some(Path::new("/tmp/report.md")),
            Some(&agent),
        );
        assert!(
            output.starts_with("Analyze this\n\n---\n**Output:**\n"),
            "{output:?}"
        );
        assert!(
            output.contains("Write your findings to exactly this path: /tmp/report.md"),
            "{output:?}"
        );
        assert!(
            output.contains("This path is authoritative for this run."),
            "{output:?}"
        );
        assert!(
            output.contains("Ignore any other output filename or output path mentioned elsewhere"),
            "{output:?}"
        );
    }

    /// `single-output.test.ts:92-98` — "tells read-only agents to return the artifact for runtime
    /// persistence". This is the branch cyrup did not have: before G82 a read-only agent was
    /// ordered to write a file it has no tool to write.
    #[test]
    fn a_read_only_agent_is_told_to_return_the_artifact_for_the_runtime_to_persist() {
        let agent = capability_agent(builtin(&["read", "grep", "find", "ls"]));
        let output = inject_single_output_instruction(
            "Analyze this",
            Some(Path::new("/tmp/report.md")),
            Some(&agent),
        );
        assert!(
            output.contains("Return the complete artifact in your final response."),
            "{output:?}"
        );
        assert!(
            output.contains("The runtime will persist it to exactly this path: /tmp/report.md"),
            "{output:?}"
        );
        assert!(
            output.contains(
                "Do not call contact_supervisor merely because no write-capable tool is available."
            ),
            "{output:?}"
        );
        assert!(
            !output.contains("Write your findings to exactly this path"),
            "the direct-write instruction must NOT appear: {output:?}"
        );
    }

    /// `single-output.test.ts:110-115` — "uses runtime-persistence instructions in read-only system
    /// prompts", and its mutation-capable sibling at `:106-112`.
    #[test]
    fn the_system_prompt_form_branches_on_capability_too() {
        let read_only = capability_agent(builtin(&["read"]));
        let prompt = inject_output_path_system_prompt(
            "Analyze only",
            Some(Path::new("/tmp/new.md")),
            Some(&read_only),
        );
        assert!(prompt.starts_with("Analyze only"), "{prompt:?}");
        assert!(
            prompt.contains("Runtime output path override:"),
            "{prompt:?}"
        );
        assert!(
            prompt.contains("The runtime will persist it to exactly this path: /tmp/new.md"),
            "{prompt:?}"
        );
        assert!(
            !prompt.contains("Write your findings to exactly this path"),
            "{prompt:?}"
        );

        // An agent with no declared allowlist at all is mutation-capable (`tools === undefined`).
        let unrestricted = capability_agent(None);
        let prompt = inject_output_path_system_prompt(
            "Analyze only",
            Some(Path::new("/tmp/new.md")),
            Some(&unrestricted),
        );
        assert!(
            prompt.contains("Write your findings to exactly this path: /tmp/new.md"),
            "{prompt:?}"
        );
    }

    /// `single-output.test.ts:117-119` — "leaves prompts unchanged when no output path is active",
    /// for the task-side form too.
    #[test]
    fn no_output_path_means_no_task_instruction() {
        assert_eq!(
            inject_single_output_instruction("Base task", None, None),
            "Base task"
        );
    }

    // ---- extractChildWrittenOutput ----

    fn write_call(id: &str, path: &str, content: &str) -> SubagentEvent {
        SubagentEvent::ToolExecutionStart {
            tool_call_id: id.into(),
            tool_name: "write".to_string(),
            args: serde_json::json!({"path": path, "content": content}),
        }
    }

    fn tool_result(id: &str, is_error: bool) -> SubagentEvent {
        SubagentEvent::ToolExecutionEnd {
            tool_call_id: id.into(),
            tool_name: "write".to_string(),
            result: serde_json::Value::Null,
            is_error,
        }
    }

    fn completed_write(id: &str, path: &str, content: &str) -> Vec<SubagentEvent> {
        vec![write_call(id, path, content), tool_result(id, false)]
    }

    /// `single-output.test.ts:191-198` — "returns the last successfully written content for the
    /// configured path".
    #[test]
    fn the_last_successful_write_to_the_configured_path_wins() {
        let mut events = completed_write("w1", "/tmp/out.md", "draft");
        events.extend(completed_write("w2", "/tmp/other.md", "unrelated"));
        events.extend(completed_write("w3", "/tmp/out.md", "final report"));
        assert_eq!(
            extract_child_written_output(
                &events,
                Some(Path::new("/tmp/out.md")),
                Path::new("/repo")
            ),
            Some("final report".to_string())
        );
    }

    /// `single-output.test.ts:200-210` — "ignores write calls whose tool result failed".
    #[test]
    fn a_failed_write_result_is_not_authorship() {
        let failed_only = vec![
            write_call("w1", "/tmp/out.md", "never landed"),
            tool_result("w1", true),
        ];
        assert_eq!(
            extract_child_written_output(
                &failed_only,
                Some(Path::new("/tmp/out.md")),
                Path::new("/repo")
            ),
            None
        );

        let mut failed_after_success = completed_write("w1", "/tmp/out.md", "landed");
        failed_after_success.push(write_call("w2", "/tmp/out.md", "never landed"));
        failed_after_success.push(tool_result("w2", true));
        assert_eq!(
            extract_child_written_output(
                &failed_after_success,
                Some(Path::new("/tmp/out.md")),
                Path::new("/repo")
            ),
            Some("landed".to_string()),
            "the later FAILED write must not overwrite the earlier successful one"
        );
    }

    /// `single-output.test.ts:212-221` — "ignores write calls with no confirmed successful tool
    /// result". On cyrup's wire the only representable "unconfirmed" state is the absence of any
    /// `ToolExecutionEnd` for the call (see `extract_child_written_output`'s wire-shape note).
    #[test]
    fn an_unanswered_write_call_is_not_authorship() {
        let missing_result = vec![write_call("w1", "/tmp/out.md", "unconfirmed")];
        assert_eq!(
            extract_child_written_output(
                &missing_result,
                Some(Path::new("/tmp/out.md")),
                Path::new("/repo")
            ),
            None
        );
    }

    /// `single-output.test.ts:223-227` — "resolves relative write paths against the child cwd".
    #[test]
    fn relative_write_paths_resolve_against_the_child_cwd() {
        let events = completed_write("w1", "reports/out.md", "relative content");
        assert_eq!(
            extract_child_written_output(
                &events,
                Some(Path::new("/repo/reports/out.md")),
                Path::new("/repo")
            ),
            Some("relative content".to_string())
        );
        assert_eq!(
            extract_child_written_output(
                &events,
                Some(Path::new("/elsewhere/reports/out.md")),
                Path::new("/repo")
            ),
            None,
            "a different absolute target must not match"
        );
        // `path.resolve` collapses `.`/`..` lexically on both sides.
        let dotted = completed_write("w1", "./reports/../reports/out.md", "dotted");
        assert_eq!(
            extract_child_written_output(
                &dotted,
                Some(Path::new("reports/out.md")),
                Path::new("/repo")
            ),
            Some("dotted".to_string())
        );
    }

    /// `single-output.test.ts:235-245` — "ignores non-write tools and missing arguments".
    #[test]
    fn non_write_tools_and_missing_arguments_are_ignored() {
        let events = vec![
            SubagentEvent::ToolExecutionStart {
                tool_call_id: "e1".into(),
                tool_name: "edit".to_string(),
                args: serde_json::json!({"path": "/tmp/out.md", "oldText": "a", "newText": "b"}),
            },
            tool_result("e1", false),
            SubagentEvent::ToolExecutionStart {
                tool_call_id: "w1".into(),
                tool_name: "write".to_string(),
                args: serde_json::json!({"path": "/tmp/out.md"}),
            },
            tool_result("w1", false),
        ];
        assert_eq!(
            extract_child_written_output(
                &events,
                Some(Path::new("/tmp/out.md")),
                Path::new("/repo")
            ),
            None,
            "an `edit` call and a `write` call with no `content` are both non-authorship"
        );
        assert_eq!(
            extract_child_written_output(&[], Some(Path::new("/tmp/out.md")), Path::new("/repo")),
            None
        );
        assert_eq!(
            extract_child_written_output(
                &completed_write("w1", "/tmp/out.md", "x"),
                None,
                Path::new("/repo")
            ),
            None,
            "no configured output path means no authorship question to answer"
        );
    }

    /// The `part.name !== "write"` half of `single-output.ts:40` on its own. The case above cannot
    /// prove it: its `edit` call carries no `content` argument, so the args type-check rejects it
    /// whether or not the tool name is inspected. A tool that IS shaped like `write` — both `path`
    /// and `content` strings, successful result, matching path — is the only input that isolates
    /// the name check, and only a `write` call is authorship of the configured output.
    #[test]
    fn a_write_shaped_call_from_a_different_tool_is_not_authorship() {
        for tool in ["edit", "multi_edit", "bash", "create_file", "notebook_edit"] {
            let events = vec![
                SubagentEvent::ToolExecutionStart {
                    tool_call_id: "t1".into(),
                    tool_name: tool.to_string(),
                    args: serde_json::json!({"path": "/tmp/out.md", "content": "impostor"}),
                },
                tool_result("t1", false),
            ];
            assert_eq!(
                extract_child_written_output(
                    &events,
                    Some(Path::new("/tmp/out.md")),
                    Path::new("/repo")
                ),
                None,
                "`{tool}` is not the `write` tool, so its content is not authored output"
            );
        }
    }

    /// The property `resolve_output_handoff` cannot supply: a SIBLING process writing the same
    /// path makes the stat heuristic report `ChildWrote`, while authorship — taken from the child's
    /// own transcript — correctly reports that this child wrote nothing there (#420).
    #[test]
    fn authorship_is_immune_to_a_sibling_writing_the_same_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shared.md");
        let before = snapshot_output_file(Some(&path));

        // Somebody else — not this child — writes the configured path mid-run.
        std::fs::write(&path, "written by a sibling run").expect("sibling write");

        // The stat heuristic attributes it to the child...
        assert_eq!(
            resolve_output_handoff(&path, "this child's receipt", before),
            OutputHandoff::ChildWrote {
                content: "written by a sibling run".to_string()
            }
        );
        // ...but the child's own transcript shows it never wrote that path.
        let events = completed_write("w1", "somewhere/else.md", "this child's real artifact");
        assert_eq!(
            extract_child_written_output(&events, Some(&path), dir.path()),
            None
        );
    }
}
