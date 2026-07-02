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
//! Argv/system-prompt steering for `file-only` mode (R-SA-024's other half — actually building
//! the child's argv and injecting the output-path override into the system prompt before spawn)
//! is [`build_output_path_system_prompt_instruction`]'s narrow, testable core; wiring it into the
//! full `ChildSpawnSpec`/system-prompt assembly is a later phase's concern (`exec/mod.rs`'s
//! `run_sync` entry point and `spawn/mod.rs`'s `ChildSpawnSpec` construction — neither implemented
//! yet), per this file's task brief.
//!
//! This module has ZERO dependency on `cyrup-agent` — every message/content shape it inspects is
//! the same opaque `serde_json::Value` [`crate::exec::ndjson::SubagentEvent`] already exposes,
//! never a typed `AgentMessage`/`Content` re-import (arch-SA §2.1/§1.1, restated at every module
//! boundary in this crate).

use std::path::Path;

use crate::exec::ndjson::SubagentEvent;

// ============================================================================================
// R-SA-029: Final-output extraction
// ============================================================================================

/// A fenced-code-block scan match: the language tag (lowercased) and the fenced body, used by
/// [`looks_like_acceptance_report`] to test each fenced block in a text part independently of the
/// others (pi-subagents' own `getFinalOutput`, `pi-subagents/src/shared/utils.ts:244-267`, is the
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
/// `getFinalOutput` regex pair verbatim (`pi-subagents/src/shared/utils.ts:257-261`), not a
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
        if lang == "acceptance-report" {
            return true;
        }
        if matches!(lang.as_str(), "json" | "jsonc" | "json5")
            && block.body.contains("\"criteriaSatisfied\"")
            && ACCEPTANCE_REPORT_COMPANION_KEYS
                .iter()
                .any(|key| block.body.contains(&format!("\"{key}\"")))
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
            let last = non_empty
                .last()
                .map(|s| (*s).clone())
                .unwrap_or_default();
            fallback = Some(last);
        }
    }

    fallback
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
/// (`pi-subagents/src/runs/shared/single-output.ts:92-100`): the snapshot's only job is to be
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
    let is_file_only = matches!(
        output_mode,
        crate::discovery::types::OutputMode::FileOnly
    );
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
        (None, None) => false,             // never existed, still doesn't: unchanged
        (None, Some(_)) => true,           // didn't exist before, exists now: the child wrote it
        (Some(_), None) => false,          // existed before, gone now: nothing to read back
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
// R-SA-024: argv/system-prompt steering for file-only output mode
// ============================================================================================

/// R-SA-024 (MUST, file-only half) — build the authoritative output-path override instruction to
/// inject into the child's **system prompt** (not merely conveyed via argv) when
/// `output_mode == "file-only"`, so the child's own write-tool behavior is steered at generation
/// time. Mirrors pi-subagents' own `formatOutputPathInstruction`/`injectOutputPathSystemPrompt`
/// (`pi-subagents/src/runs/shared/single-output.ts:36-51`) verbatim in wording and structure.
///
/// Returns `None` if no output path is configured (nothing to inject); wiring this into the full
/// system-prompt assembly pipeline and the rest of R-SA-024's argv contract (`--model`,
/// tools-allowlist flag) is a later phase's concern (`exec/mod.rs`'s `run_sync` and
/// `spawn/mod.rs`'s `ChildSpawnSpec` construction, neither implemented yet) — this function is
/// the narrow, independently testable piece that belongs to this file's output-handoff scope.
#[must_use]
pub fn build_output_path_system_prompt_instruction(output_path: Option<&Path>) -> Option<String> {
    let path = output_path?;
    Some(format!(
        "Runtime output path override:\nWrite your findings to exactly this path: {}\n\
         This path is authoritative for this run.\n\
         Ignore any other output filename or output path mentioned elsewhere, including output \
         destinations in the base agent prompt, system prompt, or task instructions.",
        path.display()
    ))
}

/// R-SA-024 — append [`build_output_path_system_prompt_instruction`]'s instruction to an existing
/// system prompt body, if any output path is configured; otherwise returns `system_prompt`
/// unchanged. Mirrors pi-subagents' `injectOutputPathSystemPrompt`
/// (`pi-subagents/src/runs/shared/single-output.ts:49-52`).
#[must_use]
pub fn inject_output_path_system_prompt(system_prompt: &str, output_path: Option<&Path>) -> String {
    let Some(instruction) = build_output_path_system_prompt_instruction(output_path) else {
        return system_prompt.to_string();
    };
    if system_prompt.is_empty() {
        instruction
    } else {
        format!("{system_prompt}\n\n{instruction}")
    }
}

// ============================================================================================
// R-SA-042: UTF-8-safe output truncation
// ============================================================================================

/// The byte/line cap applied to a delivered final output (R-SA-042). Default parity with
/// pi-subagents (`DEFAULT_MAX_OUTPUT`, `pi-subagents/src/shared/types.ts:888-891`): 200KB / 5000
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
/// `formatBytes` (`pi-subagents/src/shared/types.ts:1068-1072`) closely enough for the truncation
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
/// (`pi-subagents/src/shared/types.ts:1075-1108`) ordering exactly: a huge single first line still
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
/// (`pi-subagents/src/runs/shared/single-output.ts:56-60`): the number of newline separators, plus
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
        let text = "Done.\n```json\n{\"criteriaSatisfied\": true, \"changedFiles\": [\"a.rs\"]}\n```\n";
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
        assert!(!looks_like_acceptance_report(
            "```rust\nfn main() {}\n```"
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
            message_end(
                "assistant",
                &["```acceptance-report\nOLDER\n```"],
            ),
            message_end("assistant", &["unrelated plain text"]),
            message_end(
                "assistant",
                &["```acceptance-report\nNEWEST\n```"],
            ),
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
        let (_, body) = result
            .text
            .split_once('\n')
            .expect("marker line present");
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
        let (_, body) = result
            .text
            .split_once('\n')
            .expect("marker line present");
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
        let cap = OutputCap {
            bytes: 0,
            lines: 0,
        };
        let result = truncate_output("", cap, None);
        assert!(!result.truncated);
        assert_eq!(result.text, "");
        assert_eq!(result.original_lines, 0);
    }

    // ---- File-only handoff: real filesystem stat-snapshot behavior ----

    #[test]
    fn validate_file_only_requires_path_fails_fast_when_missing() {
        let err = validate_file_only_requires_path(
            crate::discovery::types::OutputMode::FileOnly,
            None,
        );
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
        let err = validate_file_only_requires_path(
            crate::discovery::types::OutputMode::Inline,
            None,
        );
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
        let instruction = build_output_path_system_prompt_instruction(Some(path))
            .expect("Some for a configured path");
        assert!(instruction.contains("/work/out.md"));
        assert!(instruction.contains("authoritative"));
    }

    #[test]
    fn no_instruction_built_when_no_output_path() {
        assert!(build_output_path_system_prompt_instruction(None).is_none());
    }

    #[test]
    fn injects_instruction_after_existing_system_prompt() {
        let path = Path::new("/work/out.md");
        let merged = inject_output_path_system_prompt("You are a helpful agent.", Some(path));
        assert!(merged.starts_with("You are a helpful agent."));
        assert!(merged.contains("/work/out.md"));
    }

    #[test]
    fn injects_instruction_alone_when_system_prompt_empty() {
        let path = Path::new("/work/out.md");
        let merged = inject_output_path_system_prompt("", Some(path));
        assert!(merged.contains("/work/out.md"));
        assert!(!merged.starts_with('\n'));
    }

    #[test]
    fn system_prompt_unchanged_when_no_output_path() {
        let merged = inject_output_path_system_prompt("unchanged", None);
        assert_eq!(merged, "unchanged");
    }
}
