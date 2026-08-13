//! The FleetView transcript pane — Rust port of pi-subagents `src/tui/fleet-transcript.ts`
//! (`@v0.43.0`, 577 lines), the module `src/tui/fleet.ts:17` imports `readFleetTranscript`,
//! `renderFleetTranscript` and the `FleetTranscript` type from.
//!
//! # What it does
//!
//! Two halves, in strict order:
//!
//! 1. **Read** ([`read_fleet_transcript`], pi `:384-404`). A *containment-checked, bounded* tail
//!    read of one child's transcript JSONL, folded into a flat [`FleetTranscriptEvent`] list.
//!    Every hostile-input defence pi has is ported, because the file being read is written by a
//!    subagent child and a run's `status.json` can name an arbitrary path:
//!    * [`validate_transcript_path`] (pi `:159-186`) — refuse a path with no trusted root, outside
//!      the trusted roots, that is a symlink, that is not a regular file, or whose *real* path
//!      escapes the *real* roots.
//!    * [`read_tail_lines`] (pi `:197-221`) — `O_NOFOLLOW` open, seek to the last
//!      [`DEFAULT_MAX_BYTES`], drop the partial first line, and keep at most
//!      [`DEFAULT_MAX_RECORDS`] records.
//!    * [`safe_display_text`] (pi `:46-57`) — replace every terminal-control, bidi-override,
//!      private-use, non-character and surrogate code point with a printable `[U+XXXX]` token, and
//!      swap a payload that looks binary for [`BINARY_CONTENT_PLACEHOLDER`] wholesale. A subagent
//!      that prints `\x1b[2J` or a right-to-left override MUST NOT be able to repaint or spoof the
//!      supervisor's terminal.
//! 2. **Render** ([`render_fleet_transcript`], pi `:500-577`). Turn that event list into the
//!    detail pane's lines: assistant/supervisor turns behind a `◆`/`◇` marker on a `│` rail, and
//!    tool invocations either collapsed to one `├─ ✓ name args` row with a bounded output preview
//!    or (with `expanded_tools`) fully expanded with per-tool special-casing for `bash` and
//!    `read`.
//!
//! # Transport difference (stated per this port's convention)
//!
//! pi returns `string[]` with embedded ANSI escapes and computes widths by parsing those escapes
//! back out; cyrup returns [`Line<'static>`] whose spans carry [`ratatui::style::Style`]
//! structurally. The width/wrap/truncate/right-align behaviour pi's `pi-tui` helpers define is
//! ported verbatim in [`super::fleet_theme`]; see that module's doc for why the ANSI half of those
//! helpers has no counterpart here. Nothing about *what* is rendered changes.
//!
//! # Honest deltas vs. pi
//!
//! 1. **No markdown renderer for assistant turns.** pi renders an assistant message through
//!    `new Markdown(text, 0, 0, markdownTheme).render(width - 2)` (`:565`). cyrup has no markdown
//!    renderer reachable from this crate (`cyrup-tui` owns one and this crate must not depend on
//!    it — arch-SA §1.1/§6.1), so an assistant turn is word-wrapped as plain text, exactly as the
//!    supervisor branch immediately below it (`:568-570`) already is. Every other property of the
//!    branch — the `◆`, the bold `Assistant` label, the ` · model` suffix, the rail, the trailing
//!    rail-only spacer — is verbatim.
//! 2. **No syntax highlighting for `read` output.** pi calls `getLanguageFromPath` +
//!    `highlightCode` (`:457-465`) from `@earendil-works/pi-coding-agent`. cyrup has no such
//!    surface in this crate's dependency closure, so `read` output renders unhighlighted. pi's own
//!    `language === undefined` branch (`:465`, `output.split("\n")`) is exactly that, so this is a
//!    branch pi itself takes for an unknown extension rather than a shape it never produces.
//! 3. **Record schema: pi's `recordType` PLUS a rewrite of cyrup's own records.** pi's transcript
//!    file is written by its own transcript writer as `{"recordType":"message"|"tool_start"|
//!    "tool_end"|"stderr"|"truncated", …}`, and pi's fifth `ArtifactPaths` field `transcriptPath`
//!    has no cyrup analogue (`artifacts.rs:58`). [`parse_transcript_lines`] therefore recognises
//!    pi's vocabulary verbatim and first, and otherwise runs [`rewrite_cyrup_record`], which maps
//!    the two record shapes cyrup really writes onto pi's — TAG AND FIELDS. That second clause is
//!    the whole point: a tag rename alone left this module correctly ported and permanently EMPTY,
//!    because cyrup's message body is a `content` array where pi's is a flat string, its tool
//!    output rides inside `tool_execution_end.result` where pi's is a separate `toolResult`
//!    message, its tool arguments are an object where pi's are preview strings — and the file
//!    [`super::fleet::transcript_target`] actually points at is written by
//!    [`crate::artifacts::run_artifact_jsonl_lines`], whose `tool_call`/`result` tags the rename
//!    did not mention at all. See [`rewrite_cyrup_record`] for the per-shape mapping.

use std::path::{Path, PathBuf};

use ratatui::text::{Line, Span};
use serde_json::Value;

use super::fleet_theme::{self as th, Role};

// =================================================================================================
// Tunables (pi `fleet-transcript.ts:6-10`)
// =================================================================================================

/// pi `DEFAULT_MAX_RECORDS` (`fleet-transcript.ts:6`) — at most this many JSONL records are parsed
/// from the tail, however many the file holds.
pub const DEFAULT_MAX_RECORDS: usize = 240;
/// pi `DEFAULT_MAX_BYTES` (`:7`) — the tail window is the last 2 MiB of the file.
pub const DEFAULT_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// pi `MAX_MESSAGE_CHARS` (`:8`) — one message body is clipped to 64 KiB.
pub const MAX_MESSAGE_CHARS: usize = 64 * 1024;
/// pi `TOOL_PREVIEW_LINES` (`:9`) — a collapsed `bash` row previews at most this many trailing
/// output lines.
pub const TOOL_PREVIEW_LINES: usize = 7;
/// pi `BINARY_CONTENT_PLACEHOLDER` (`:10`).
pub const BINARY_CONTENT_PLACEHOLDER: &str = "[binary content omitted for safe display]";

// =================================================================================================
// Display sanitization (pi `fleet-transcript.ts:12-92`)
// =================================================================================================

/// pi `isUnsafeDisplayCodePoint` (`fleet-transcript.ts:12-28`) — the five classes of code point
/// that must never reach a terminal verbatim.
///
/// The `invalidScalar` class (`U+D800..=U+DFFF`, pi `:23`) is unreachable when the input is a Rust
/// `char` (surrogates are not valid scalar values), but is retained because this function takes a
/// raw `u32`: a caller decoding from a byte stream can still present one, and dropping the check
/// would be a silent narrowing of pi's contract.
#[must_use]
pub fn is_unsafe_display_code_point(code_point: u32) -> bool {
    let terminal_control = (code_point <= 0x1f && code_point != 0x09 && code_point != 0x0a)
        || (0x7f..=0x9f).contains(&code_point);
    let bidi_control = code_point == 0x061c
        || code_point == 0x200e
        || code_point == 0x200f
        || (0x202a..=0x202e).contains(&code_point)
        || (0x2066..=0x2069).contains(&code_point);
    let private_use = (0xe000..=0xf8ff).contains(&code_point)
        || (0xf_0000..=0xf_fffd).contains(&code_point)
        || (0x10_0000..=0x10_fffd).contains(&code_point);
    let invalid_scalar = (0xd800..=0xdfff).contains(&code_point);
    let non_character = (0xfdd0..=0xfdef).contains(&code_point)
        || (code_point & 0xffff) == 0xfffe
        || (code_point & 0xffff) == 0xffff;
    terminal_control || bidi_control || private_use || invalid_scalar || non_character
}

/// pi `looksLikeBinaryContent` (`fleet-transcript.ts:30-44`): a NUL anywhere, or ≥4 suspicious
/// control characters at ≥10% density, or ≥3 U+FFFD replacement characters at ≥10% density.
#[must_use]
pub fn looks_like_binary_content(text: &str) -> bool {
    if text.contains('\0') {
        return true;
    }
    let mut suspicious_controls = 0usize;
    let mut replacement_characters = 0usize;
    let mut code_points = 0usize;
    for character in text.chars() {
        code_points = code_points.saturating_add(1);
        let cp = character as u32;
        if cp <= 0x08 || (0x0e..=0x1f).contains(&cp) {
            suspicious_controls = suspicious_controls.saturating_add(1);
        }
        if cp == 0xfffd {
            replacement_characters = replacement_characters.saturating_add(1);
        }
    }
    if code_points == 0 {
        return false;
    }
    let total = code_points as f64;
    (suspicious_controls >= 4 && (suspicious_controls as f64) / total >= 0.1)
        || (replacement_characters >= 3 && (replacement_characters as f64) / total >= 0.1)
}

/// pi `safeDisplayText` (`fleet-transcript.ts:46-57`): normalise CRLF, swap a binary-looking body
/// for [`BINARY_CONTENT_PLACEHOLDER`] wholesale, and escape every unsafe code point as
/// `[U+XXXX]`.
#[must_use]
pub fn safe_display_text(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    if looks_like_binary_content(&normalized) {
        return BINARY_CONTENT_PLACEHOLDER.to_string();
    }
    let mut safe = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        let cp = character as u32;
        if is_unsafe_display_code_point(cp) {
            safe.push_str(&format!("[U+{cp:04X}]"));
        } else {
            safe.push(character);
        }
    }
    safe
}

/// pi `sanitizeJsonDisplayValue` (`fleet-transcript.ts:59-83`): recursively [`safe_display_text`]
/// every string — object KEYS included — reporting whether anything changed so the caller can
/// re-serialize only when it must.
#[must_use]
pub fn sanitize_json_display_value(value: &Value) -> (Value, bool) {
    match value {
        Value::String(s) => {
            let safe = safe_display_text(s);
            let changed = safe != *s;
            (Value::String(safe), changed)
        }
        Value::Array(items) => {
            let sanitized: Vec<(Value, bool)> = items.iter().map(sanitize_json_display_value).collect();
            let changed = sanitized.iter().any(|(_, c)| *c);
            (Value::Array(sanitized.into_iter().map(|(v, _)| v).collect()), changed)
        }
        Value::Object(map) => {
            let mut changed = false;
            let mut out = serde_json::Map::new();
            for (key, nested) in map {
                let safe_key = safe_display_text(key);
                let (safe_value, value_changed) = sanitize_json_display_value(nested);
                changed = changed || safe_key != *key || value_changed;
                out.insert(safe_key, safe_value);
            }
            if changed {
                (Value::Object(out), true)
            } else {
                (value.clone(), false)
            }
        }
        other => (other.clone(), false),
    }
}

/// pi `safeToolArgsPayload` (`fleet-transcript.ts:85-92`): sanitize a JSON args payload
/// structurally when it parses, else fall back to sanitizing it as opaque text.
#[must_use]
pub fn safe_tool_args_payload(payload: &str) -> String {
    match serde_json::from_str::<Value>(payload) {
        Ok(parsed) => {
            let (sanitized, changed) = sanitize_json_display_value(&parsed);
            if changed {
                serde_json::to_string(&sanitized).unwrap_or_else(|_| safe_display_text(payload))
            } else {
                safe_display_text(payload)
            }
        }
        Err(_) => safe_display_text(payload),
    }
}

// =================================================================================================
// Event model (pi `fleet-transcript.ts:96-129`)
// =================================================================================================

/// A tool invocation's terminal state (pi `"running" | "complete" | "error"`, `:99`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolStatus {
    /// pi `"running"` — a `tool_start` with no matching end/result yet. The default, matching
    /// pi's own `status: "running"` on a freshly-pushed `tool_start` event (`:317`).
    #[default]
    Running,
    /// pi `"complete"`.
    Complete,
    /// pi `"error"`.
    Error,
}

/// A notice's severity (pi `"muted" | "warning" | "error"`, `:100`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeTone {
    /// pi `"muted"`.
    Muted,
    /// pi `"warning"`.
    Warning,
    /// pi `"error"` — the tone every `stderr` record gets (`:330`).
    Error,
}

/// pi's `Extract<FleetTranscriptEvent, { kind: "tool" }>` / `MutableToolEvent` (`:99`, `:115-129`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetToolEvent {
    /// pi `toolCallId` — the correlation key `tool_end` / `toolResult` records match on.
    pub tool_call_id: Option<String>,
    /// pi `name` — the tool's name, defaulting to `"tool"` when a record omits it.
    pub name: String,
    /// pi `args` — the short, already-rendered args preview (`argsPreview`).
    pub args: Option<String>,
    /// pi `argsPayload` — the full JSON args blob, sanitized by [`safe_tool_args_payload`].
    pub args_payload: Option<String>,
    /// pi `output` — the tool's captured result text.
    pub output: Option<String>,
    /// pi `outputTruncated`.
    pub output_truncated: bool,
    /// pi `status`.
    pub status: ToolStatus,
    /// pi `error` — the first non-blank line of a failed tool's output.
    pub error: Option<String>,
    /// pi `startedAt` / `endedAt` (epoch millis) — the pair [`tool_duration`] needs.
    pub started_at: Option<i64>,
    /// pi `endedAt`.
    pub ended_at: Option<i64>,
    /// pi `timestamp`.
    pub timestamp: Option<i64>,
    /// pi `MutableToolEvent.resultSeen` (`:128`) — a PARSE-TIME latch preventing a second
    /// `toolResult` record from overwriting the first. pi `delete`s it before returning
    /// (`:378-380`); this port resets it to `false` at the same point, so a returned event never
    /// carries a live latch.
    pub result_seen: bool,
}

/// One rendered transcript event (pi `FleetTranscriptEvent`, `fleet-transcript.ts:96-100`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetTranscriptEvent {
    /// pi `{ kind: "assistant", text, model?, timestamp? }`.
    Assistant {
        /// The message body, already clipped to [`MAX_MESSAGE_CHARS`] and sanitized.
        text: String,
        /// The model that produced it, when the record named one.
        model: Option<String>,
        /// Epoch-millis timestamp from the record's `ts`.
        timestamp: Option<i64>,
    },
    /// pi `{ kind: "user", text, timestamp? }` — a SUPERVISOR message to the child.
    User {
        /// The message body.
        text: String,
        /// Epoch-millis timestamp.
        timestamp: Option<i64>,
    },
    /// pi `{ kind: "tool", … }`.
    Tool(FleetToolEvent),
    /// pi `{ kind: "notice", text, tone, timestamp? }` — currently only `stderr` records.
    Notice {
        /// The notice body.
        text: String,
        /// Its severity.
        tone: NoticeTone,
        /// Epoch-millis timestamp.
        timestamp: Option<i64>,
    },
}

impl FleetTranscriptEvent {
    /// The discriminant `fleet.ts:756-763` switches on to compute the header's "conversation
    /// state" caption.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Assistant { .. } => "assistant",
            Self::User { .. } => "user",
            Self::Tool(_) => "tool",
            Self::Notice { .. } => "notice",
        }
    }
}

/// pi `FleetTranscript` (`fleet-transcript.ts:102-107`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetTranscript {
    /// The path as the CALLER named it (pi keeps the un-resolved `filePath`, `:399`).
    pub path: PathBuf,
    /// The parsed events, oldest first.
    pub events: Vec<FleetTranscriptEvent>,
    /// Whether earlier activity was omitted (byte-window truncation, record-cap truncation, or an
    /// explicit `recordType: "truncated"` marker).
    pub truncated: bool,
    /// A single space-joined warning sentence, already [`safe_display_text`]-sanitized.
    pub warning: Option<String>,
}

/// pi `FleetTranscriptReadOptions` (`fleet-transcript.ts:109-113`).
#[derive(Clone, Debug, Default)]
pub struct FleetTranscriptReadOptions {
    /// The roots the transcript MUST resolve inside. An empty list refuses the read outright
    /// (pi `:160`) — there is no "no roots means anything goes" mode.
    pub trusted_roots: Vec<PathBuf>,
    /// Override for [`DEFAULT_MAX_RECORDS`].
    pub max_records: Option<usize>,
    /// Override for [`DEFAULT_MAX_BYTES`].
    pub max_bytes: Option<u64>,
}

// =================================================================================================
// Path validation + bounded tail read (pi `fleet-transcript.ts:145-221`)
// =================================================================================================

/// pi `pathWithin` (`fleet-transcript.ts:145-149`), on already-absolute inputs.
fn path_within(base: &Path, candidate: &Path) -> bool {
    let base = std::path::absolute(base).unwrap_or_else(|_| base.to_path_buf());
    let candidate = std::path::absolute(candidate).unwrap_or_else(|_| candidate.to_path_buf());
    candidate == base || candidate.starts_with(&base)
}

/// pi `validateTranscriptPath` (`fleet-transcript.ts:159-186`). Returns `(resolved_path, warning)`;
/// pi's "file does not exist yet" case is `(None, None)` — no path, and deliberately no warning,
/// so a child that has not written its transcript yet renders as an empty pane rather than an
/// error.
#[must_use]
pub fn validate_transcript_path(
    file_path: &Path,
    trusted_roots: &[PathBuf],
) -> (Option<PathBuf>, Option<String>) {
    if trusted_roots.is_empty() {
        return (
            None,
            Some(format!("Transcript preview has no trusted root: {}", file_path.display())),
        );
    }
    let resolved = std::path::absolute(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    if !trusted_roots.iter().any(|root| path_within(root, &resolved)) {
        return (
            None,
            Some(format!("Transcript is outside trusted roots: {}", file_path.display())),
        );
    }
    let meta = match std::fs::symlink_metadata(&resolved) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, None),
        Err(e) => {
            return (None, Some(format!("Transcript could not be inspected: {e}")));
        }
    };
    if meta.file_type().is_symlink() {
        return (
            None,
            Some(format!("Transcript preview refused a symlink: {}", file_path.display())),
        );
    }
    if !meta.is_file() {
        return (
            None,
            Some(format!("Transcript path is not a file: {}", file_path.display())),
        );
    }
    let real_path = match std::fs::canonicalize(&resolved) {
        Ok(p) => p,
        Err(e) => {
            return (None, Some(format!("Transcript path could not be resolved: {e}")));
        }
    };
    let real_roots: Vec<PathBuf> = trusted_roots
        .iter()
        .filter(|root| root.exists())
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect();
    if !real_roots.iter().any(|root| path_within(root, &real_path)) {
        return (
            None,
            Some(format!("Transcript resolves outside trusted roots: {}", file_path.display())),
        );
    }
    (Some(real_path), None)
}

/// pi `isCompleteRecord` (`fleet-transcript.ts:188-195`) — a trailing line only counts as a whole
/// record if it parses to a JSON OBJECT.
fn is_complete_record(line: Option<&String>) -> bool {
    let Some(line) = line else { return false };
    if line.trim().is_empty() {
        return false;
    }
    serde_json::from_str::<Value>(line).is_ok_and(|v| v.is_object())
}

/// The result of a bounded tail read (pi's inline `{ lines, truncated, warning }`, `:197`).
#[derive(Debug, Default)]
struct TailRead {
    lines: Vec<String>,
    truncated: bool,
    warning: Option<String>,
}

/// pi `readTailLines` (`fleet-transcript.ts:197-221`): open with `O_NOFOLLOW`, re-check via
/// `fstat` that the open descriptor is a regular file (closing the last TOCTOU window the
/// `lstat` in [`validate_transcript_path`] leaves open), read the last `max_bytes`, discard the
/// partial first line when the window did not start at byte 0, and discard a trailing partial
/// record.
fn read_tail_lines(path: &Path, max_bytes: u64) -> TailRead {
    use std::io::{Read, Seek, SeekFrom};
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = match options.open(path) {
        Ok(f) => f,
        Err(e) => {
            return TailRead {
                warning: Some(format!("Transcript could not be read: {e}")),
                ..TailRead::default()
            };
        }
    };
    let meta = match file.metadata() {
        Ok(m) => m,
        Err(e) => {
            return TailRead {
                warning: Some(format!("Transcript could not be read: {e}")),
                ..TailRead::default()
            };
        }
    };
    if !meta.is_file() {
        return TailRead {
            warning: Some(format!("Transcript path is not a file: {}", path.display())),
            ..TailRead::default()
        };
    }
    let size = meta.len();
    if size == 0 {
        return TailRead::default();
    }
    let bytes_to_read = size.min(max_bytes);
    let start = size.saturating_sub(bytes_to_read);
    if let Err(e) = file.seek(SeekFrom::Start(start)) {
        return TailRead {
            warning: Some(format!("Transcript could not be read: {e}")),
            ..TailRead::default()
        };
    }
    let mut buffer: Vec<u8> = Vec::new();
    if let Err(e) = file.take(bytes_to_read).read_to_end(&mut buffer) {
        return TailRead {
            warning: Some(format!("Transcript could not be read: {e}")),
            ..TailRead::default()
        };
    }
    let content = String::from_utf8_lossy(&buffer).into_owned();
    let ends_with_newline = content.ends_with('\n');
    let mut lines: Vec<String> = content
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    // pi's two distinct reasons to drop the last element (`:213-214`), both of which pop exactly
    // one line: it is the empty string left by a trailing newline, OR the window ended mid-record
    // and that partial record cannot be parsed.
    if lines.last().is_some_and(String::is_empty)
        || (!ends_with_newline && !is_complete_record(lines.last()))
    {
        lines.pop();
    }
    TailRead { lines, truncated: start > 0, warning: None }
}

// =================================================================================================
// Record parsing (pi `fleet-transcript.ts:223-404`)
// =================================================================================================

/// pi `clipMessage` (`fleet-transcript.ts:223-226`).
fn clip_message(text: &str) -> String {
    if text.chars().count() <= MAX_MESSAGE_CHARS {
        return text.to_string();
    }
    let head: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
    format!("{head}\n\n… message truncated")
}

/// pi `stringValue` (`fleet-transcript.ts:137-139`) — a non-blank string, else `None`.
fn string_value(value: Option<&Value>) -> Option<String> {
    let s = value?.as_str()?;
    if s.trim().is_empty() { None } else { Some(s.to_string()) }
}

/// pi `numberValue` (`fleet-transcript.ts:141-143`) — a finite number, else `None`.
fn number_value(value: Option<&Value>) -> Option<i64> {
    let n = value?.as_f64()?;
    if n.is_finite() { Some(n as i64) } else { None }
}

/// pi `objectValue` (`fleet-transcript.ts:131-135`).
fn object_value(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    value?.as_object()
}

/// pi `safeTranscriptEvent` (`fleet-transcript.ts:228-247`) — sanitize every user-visible string
/// on one event.
#[must_use]
pub fn safe_transcript_event(event: &FleetTranscriptEvent) -> FleetTranscriptEvent {
    match event {
        FleetTranscriptEvent::Assistant { text, model, timestamp } => {
            FleetTranscriptEvent::Assistant {
                text: safe_display_text(text),
                model: model.as_deref().map(safe_display_text),
                timestamp: *timestamp,
            }
        }
        FleetTranscriptEvent::User { text, timestamp } => FleetTranscriptEvent::User {
            text: safe_display_text(text),
            timestamp: *timestamp,
        },
        FleetTranscriptEvent::Notice { text, tone, timestamp } => FleetTranscriptEvent::Notice {
            text: safe_display_text(text),
            tone: *tone,
            timestamp: *timestamp,
        },
        FleetTranscriptEvent::Tool(tool) => FleetTranscriptEvent::Tool(FleetToolEvent {
            name: safe_display_text(&tool.name),
            args: tool.args.as_deref().map(safe_display_text),
            args_payload: tool.args_payload.as_deref().map(safe_tool_args_payload),
            output: tool.output.as_deref().map(safe_display_text),
            error: tool.error.as_deref().map(safe_display_text),
            tool_call_id: tool.tool_call_id.clone(),
            output_truncated: tool.output_truncated,
            status: tool.status,
            started_at: tool.started_at,
            ended_at: tool.ended_at,
            timestamp: tool.timestamp,
            result_seen: tool.result_seen,
        }),
    }
}

/// pi `findTool` (`fleet-transcript.ts:249-268`): with a call id, the newest tool event carrying
/// it; without one, the newest tool event of matching name whose result has not been seen yet.
fn find_tool<'a>(
    events: &'a mut [FleetTranscriptEvent],
    tool_call_id: Option<&str>,
    name: Option<&str>,
) -> Option<&'a mut FleetToolEvent> {
    if let Some(id) = tool_call_id {
        return events.iter_mut().rev().find_map(|event| match event {
            FleetTranscriptEvent::Tool(tool) if tool.tool_call_id.as_deref() == Some(id) => {
                Some(tool)
            }
            _ => None,
        });
    }
    events.iter_mut().rev().find_map(|event| match event {
        FleetTranscriptEvent::Tool(tool)
            if (name.is_none() || Some(tool.name.as_str()) == name) && !tool.result_seen =>
        {
            Some(tool)
        }
        _ => None,
    })
}

/// pi `appendTextEvent` (`fleet-transcript.ts:270-281`) — trim, clip, and drop an exact repeat of
/// the immediately preceding same-kind message.
fn append_text_event(
    events: &mut Vec<FleetTranscriptEvent>,
    assistant: bool,
    text: &str,
    model: Option<String>,
    timestamp: Option<i64>,
) {
    let clipped = clip_message(text.trim());
    if clipped.is_empty() {
        return;
    }
    let duplicate = match events.last() {
        Some(FleetTranscriptEvent::Assistant { text: previous, .. }) if assistant => {
            *previous == clipped
        }
        Some(FleetTranscriptEvent::User { text: previous, .. }) if !assistant => {
            *previous == clipped
        }
        _ => false,
    };
    if duplicate {
        return;
    }
    if assistant {
        events.push(FleetTranscriptEvent::Assistant { text: clipped, model, timestamp });
    } else {
        events.push(FleetTranscriptEvent::User { text: clipped, timestamp });
    }
}

/// The outcome of [`parse_transcript_lines`] (pi's inline `{ events, malformed,
/// explicitTruncation }`, `:283`).
#[derive(Debug, Default)]
pub struct ParsedTranscript {
    /// The folded events, oldest first, each already [`safe_transcript_event`]-sanitized.
    pub events: Vec<FleetTranscriptEvent>,
    /// How many lines failed to parse as a JSON object — counted, never silently swallowed.
    pub malformed: usize,
    /// Whether a `recordType: "truncated"` marker was seen.
    pub explicit_truncation: bool,
}

/// pi `parseTranscriptLines` (`fleet-transcript.ts:283-382`).
///
/// `conversation_started` is pi's `conversationStarted` parameter (`:283`), passed `true` when the
/// tail dropped earlier records: without it, a `user` record arriving before the window's first
/// `assistant` record would be dropped by the `assistantSeen` gate (`:373`) even though the
/// conversation demonstrably had already started.
///
/// See delta 3 in the module doc for the `recordType` (pi) / `type` (cyrup NDJSON) dual
/// vocabulary.
#[must_use]
pub fn parse_transcript_lines(lines: &[String], conversation_started: bool) -> ParsedTranscript {
    let mut events: Vec<FleetTranscriptEvent> = Vec::new();
    let mut malformed = 0usize;
    let mut explicit_truncation = false;
    let mut assistant_seen = conversation_started;

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            malformed = malformed.saturating_add(1);
            continue;
        };
        let Some(record) = parsed.as_object() else {
            malformed = malformed.saturating_add(1);
            continue;
        };

        // [CYRUP-DELTA, module doc item 3] Rewrite a cyrup-shaped record into pi's own record
        // vocabulary — FIELDS as well as tag — before dispatch. One cyrup record can carry what pi
        // spreads over several (a settled `result` holds both the assistant output and the run's
        // error; a `tool_execution_end` holds both the completion and the tool's output), so the
        // rewrite yields a LIST. A record carrying pi's own `recordType` passes through untouched.
        let rewritten: Vec<serde_json::Map<String, Value>> =
            if record.contains_key("recordType") { Vec::new() } else { rewrite_cyrup_record(record) };
        let dispatch: Vec<&serde_json::Map<String, Value>> =
            if rewritten.is_empty() { vec![record] } else { rewritten.iter().collect() };

        for record in dispatch {
        let record_type = string_value(record.get("recordType"));
        let timestamp = number_value(record.get("ts"));

        match record_type.as_deref() {
            Some("truncated") => {
                explicit_truncation = true;
            }
            Some("tool_start") => {
                let name = string_value(record.get("toolName")).unwrap_or_else(|| "tool".to_string());
                events.push(FleetTranscriptEvent::Tool(FleetToolEvent {
                    tool_call_id: string_value(record.get("toolCallId")),
                    name,
                    args: string_value(record.get("argsPreview")),
                    args_payload: string_value(record.get("argsPayload")),
                    status: ToolStatus::Running,
                    timestamp,
                    started_at: timestamp,
                    ..FleetToolEvent::default()
                }));
            }
            Some("tool_end") => {
                let is_error = record.get("isError") == Some(&Value::Bool(true));
                let call_id = string_value(record.get("toolCallId"));
                let tool_name = string_value(record.get("toolName"));
                if let Some(tool) =
                    find_tool(&mut events, call_id.as_deref(), tool_name.as_deref())
                {
                    if !tool.result_seen {
                        tool.status = if is_error { ToolStatus::Error } else { ToolStatus::Complete };
                    }
                    if timestamp.is_some() && tool.ended_at.is_none() {
                        tool.ended_at = timestamp;
                    }
                }
            }
            Some("stderr") => {
                if let Some(text) = string_value(record.get("text")) {
                    events.push(FleetTranscriptEvent::Notice {
                        text: clip_message(&text),
                        tone: NoticeTone::Error,
                        timestamp,
                    });
                }
            }
            Some("message") => {
                let message = object_value(record.get("message"));
                let role = string_value(record.get("role"))
                    .or_else(|| string_value(message.and_then(|m| m.get("role"))));
                let text = string_value(record.get("text"))
                    .or_else(|| string_value(message.and_then(|m| m.get("text"))))
                    .or_else(|| string_value(message.and_then(|m| m.get("content"))));
                match role.as_deref() {
                    Some("toolResult" | "tool_result") => {
                        apply_tool_result(&mut events, record, message, text.as_deref(), timestamp);
                    }
                    Some("assistant") => {
                        assistant_seen = true;
                        if let Some(text) = text {
                            append_text_event(
                                &mut events,
                                true,
                                &text,
                                string_value(record.get("model")),
                                timestamp,
                            );
                        }
                    }
                    Some("user") => {
                        if assistant_seen && let Some(text) = text {
                            append_text_event(&mut events, false, &text, None, timestamp);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        }
    }

    // pi `:378-380` — `delete event.resultSeen` before returning.
    for event in &mut events {
        if let FleetTranscriptEvent::Tool(tool) = event {
            tool.result_seen = false;
        }
    }
    let events = events.iter().map(safe_transcript_event).collect();
    ParsedTranscript { events, malformed, explicit_truncation }
}

/// pi's `role === "toolResult"` branch (`fleet-transcript.ts:338-363`), lifted so the `message`
/// arm above stays readable.
fn apply_tool_result(
    events: &mut Vec<FleetTranscriptEvent>,
    record: &serde_json::Map<String, Value>,
    message: Option<&serde_json::Map<String, Value>>,
    text: Option<&str>,
    timestamp: Option<i64>,
) {
    let tool_call_id = string_value(record.get("toolCallId"))
        .or_else(|| string_value(message.and_then(|m| m.get("toolCallId"))));
    let name = string_value(record.get("toolName"))
        .or_else(|| string_value(message.and_then(|m| m.get("toolName"))))
        .unwrap_or_else(|| "tool".to_string());
    let failed = record.get("isError") == Some(&Value::Bool(true))
        || message.and_then(|m| m.get("isError")) == Some(&Value::Bool(true));
    let output_truncated_flag = record.get("outputTruncated") == Some(&Value::Bool(true));

    if find_tool(events, tool_call_id.as_deref(), Some(name.as_str())).is_none() {
        events.push(FleetTranscriptEvent::Tool(FleetToolEvent {
            tool_call_id: tool_call_id.clone(),
            name: name.clone(),
            status: if failed { ToolStatus::Error } else { ToolStatus::Complete },
            timestamp,
            ..FleetToolEvent::default()
        }));
    }
    let Some(tool) = find_tool(events, tool_call_id.as_deref(), Some(name.as_str())) else {
        return;
    };
    if tool.result_seen {
        return;
    }
    tool.result_seen = true;
    tool.status = if failed { ToolStatus::Error } else { ToolStatus::Complete };
    if timestamp.is_some() && tool.ended_at.is_none() {
        tool.ended_at = timestamp;
    }
    if let Some(text) = text
        && (!failed || tool.output.is_none())
    {
        tool.output = Some(clip_message(text));
        tool.output_truncated = output_truncated_flag
            || text.contains("… payload truncated")
            || text.contains("[Showing lines");
    }
    if failed && let Some(text) = text {
        let first = text
            .split('\n')
            .map(|l| l.strip_suffix('\r').unwrap_or(l))
            .find(|candidate| !candidate.trim().is_empty())
            .unwrap_or(text);
        tool.error = Some(clip_message(first));
    }
}

/// \[CYRUP-DELTA, module doc item 3] Rewrite one cyrup-shaped transcript record into pi's own
/// record vocabulary — **field names included** — so [`parse_transcript_lines`] folds the files
/// cyrup ACTUALLY writes with pi's own semantics.
///
/// # Why this is a rewrite and not a tag rename
///
/// An earlier version of this mapped three `type` tags onto pi's `recordType` names and stopped
/// there. That produced an empty pane against every real file, because the FIELDS underneath the
/// tags do not line up:
///
/// * pi's `message` record carries a flat `text` string; cyrup's `message_end` carries a whole
///   `AgentMessage` whose body is a `content` ARRAY of typed blocks
///   (`cyrup_core::Content::{Text,Thinking,ToolCall,Image}`). `string_value(message.content)` on an
///   array is `None`, so no assistant or supervisor turn ever rendered.
/// * pi's tool output arrives as a separate `role: "toolResult"` message; cyrup's
///   `tool_execution_end` carries the result INLINE in its `result` field, which the old map never
///   read — so every tool row rendered with a status glyph and no output.
/// * pi's `tool_start` reads `argsPreview`/`argsPayload` strings; cyrup emits an `args` OBJECT, so
///   every collapsed tool row rendered bare.
///
/// # The two vocabularies
///
/// Both are handled, because two different files reach [`read_fleet_transcript`]:
///
/// 1. **The settled-run artifact** ([`crate::artifacts::run_artifact_jsonl_lines`]) — the file
///    [`super::fleet::transcript_target`] points at for a foreground child. Its records are
///    `{"type":"tool_call","text":…,"expandedText":…}` per call plus one
///    `{"type":"result","agent":…,"exitCode":…,"model":…,"output":…,"error":…}`. Neither tag was
///    recognised at all before, which is why a settled child's pane was blank.
/// 2. **The raw child NDJSON stream** ([`crate::exec::ndjson::SubagentEvent`]) — what a live
///    attempt's `.jsonl` holds.
///
/// Records that carry no transcript-visible content (`agent_start`, `turn_start`, `turn_end`,
/// `message_start`, `message_update`, and every lifecycle tag) rewrite to nothing, mirroring pi's
/// own fall-through for records it does not display.
fn rewrite_cyrup_record(
    record: &serde_json::Map<String, Value>,
) -> Vec<serde_json::Map<String, Value>> {
    let Some(tag) = record.get("type").and_then(Value::as_str) else { return Vec::new() };
    match tag {
        // --- vocabulary 1: the settled-run artifact ------------------------------------------
        "tool_call" => {
            let text = string_value(record.get("text")).unwrap_or_default();
            let expanded = string_value(record.get("expandedText"));
            let (name, args) = split_formatted_tool_call(&text);
            // A settled artifact records the call, never a failure — `SingleResult::tool_calls`
            // has no error channel — so the pair is `tool_start` + a clean `tool_end`, which is
            // pi's own COMPLETE status (`fleet-transcript.ts:307`). Rendering it as `running`
            // instead would mark every settled child's tools as still in flight.
            let mut start = serde_json::Map::new();
            start.insert("recordType".into(), Value::String("tool_start".into()));
            start.insert("toolName".into(), Value::String(name.clone()));
            if !args.is_empty() {
                start.insert("argsPreview".into(), Value::String(args));
            }
            if let Some(expanded) = expanded {
                start.insert("argsPayload".into(), Value::String(expanded));
            }
            let mut end = serde_json::Map::new();
            end.insert("recordType".into(), Value::String("tool_end".into()));
            end.insert("toolName".into(), Value::String(name));
            vec![start, end]
        }
        "result" => {
            let mut out = Vec::new();
            // The run's final assistant output IS the conversation this artifact preserves.
            if let Some(output) = string_value(record.get("output")) {
                let mut message = serde_json::Map::new();
                message.insert("recordType".into(), Value::String("message".into()));
                message.insert("role".into(), Value::String("assistant".into()));
                message.insert("text".into(), Value::String(output));
                if let Some(model) = string_value(record.get("model")) {
                    message.insert("model".into(), Value::String(model));
                }
                out.push(message);
            }
            // pi renders a run failure through its `stderr` record, the same red notice
            // (`fleet-transcript.ts:322-329`).
            if let Some(error) = string_value(record.get("error")) {
                let mut notice = serde_json::Map::new();
                notice.insert("recordType".into(), Value::String("stderr".into()));
                notice.insert("text".into(), Value::String(error));
                out.push(notice);
            }
            out
        }
        // --- vocabulary 2: the raw child NDJSON stream ---------------------------------------
        "message_end" => {
            let Some(message) = object_value(record.get("message")) else { return Vec::new() };
            let Some(role) = message.get("role").and_then(Value::as_str) else { return Vec::new() };
            let timestamp = number_value(message.get("timestamp"));
            let mut out = serde_json::Map::new();
            out.insert("recordType".into(), Value::String("message".into()));
            out.insert("role".into(), Value::String(role.to_string()));
            if let Some(ts) = timestamp {
                out.insert("ts".into(), Value::from(ts));
            }
            // `content` is an array of typed blocks on EVERY role; flatten it to the flat `text`
            // pi's own record carries.
            if let Some(text) = content_text(message.get("content")) {
                out.insert("text".into(), Value::String(text));
            }
            if role == "toolResult" {
                for key in ["toolCallId", "toolName"] {
                    if let Some(v) = string_value(message.get(key)) {
                        out.insert(key.into(), Value::String(v));
                    }
                }
                if message.get("isError") == Some(&Value::Bool(true)) {
                    out.insert("isError".into(), Value::Bool(true));
                }
            }
            if role == "assistant" && let Some(model) = string_value(message.get("model")) {
                out.insert("model".into(), Value::String(model));
            }
            vec![out]
        }
        "tool_execution_start" => {
            let mut out = serde_json::Map::new();
            out.insert("recordType".into(), Value::String("tool_start".into()));
            for key in ["toolCallId", "toolName"] {
                if let Some(v) = string_value(record.get(key)) {
                    out.insert(key.into(), Value::String(v));
                }
            }
            if let Some(args) = record.get("args").filter(|v| v.is_object()) {
                let preview = crate::exec::tool_call_summary::extract_tool_args_preview(args);
                if !preview.trim().is_empty() {
                    out.insert("argsPreview".into(), Value::String(preview));
                }
                if let Ok(payload) = serde_json::to_string(args) {
                    out.insert("argsPayload".into(), Value::String(payload));
                }
            }
            vec![out]
        }
        "tool_execution_end" => {
            // A `toolResult` message rather than a bare `tool_end`: it sets the SAME status
            // (`fleet-transcript.ts:349`) and additionally attaches the output, which a `tool_end`
            // record has no field for. The tool's own `result.content` is where cyrup puts what pi
            // puts in the separate message.
            let mut out = serde_json::Map::new();
            out.insert("recordType".into(), Value::String("message".into()));
            out.insert("role".into(), Value::String("toolResult".into()));
            for key in ["toolCallId", "toolName"] {
                if let Some(v) = string_value(record.get(key)) {
                    out.insert(key.into(), Value::String(v));
                }
            }
            let result = record.get("result");
            let is_error = record.get("isError") == Some(&Value::Bool(true))
                || result.and_then(|r| r.get("isError")) == Some(&Value::Bool(true));
            if is_error {
                out.insert("isError".into(), Value::Bool(true));
            }
            let text = content_text(result.and_then(|r| r.get("content")))
                .or_else(|| content_text(result));
            if let Some(text) = text {
                out.insert("text".into(), Value::String(text));
            }
            vec![out]
        }
        _ => Vec::new(),
    }
}

/// The displayable text of a `content` value: a bare string, or the concatenation of the `text`
/// members of an array of typed blocks (`cyrup_core::Content`, `{"type":"text","text":…}`).
///
/// Non-text blocks contribute nothing — a `thinking` block is not part of the visible transcript
/// (pi's transcript writer records assistant TEXT), a `toolCall` block is already rendered as its
/// own tool row, and an `image` block has no textual form.
fn content_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) => (!s.trim().is_empty()).then(|| s.clone()),
        Value::Array(blocks) => {
            let joined = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            (!joined.trim().is_empty()).then_some(joined)
        }
        _ => None,
    }
}

/// Split one [`crate::exec::tool_call_summary::format_tool_call`] preview back into its tool name
/// and its argument text.
///
/// That formatter emits exactly three shapes: `"$ <command>"` for `bash`, `"<name> <path>"` for
/// `read`/`write`/`edit`, and `"<name> <json>"` for everything else. The `$` prefix is `bash`'s own
/// marker, so it identifies the tool without the record having to carry a name — which the settled
/// artifact does not ([`crate::exec::tool_call_summary::ToolCallSummary`] is two preview strings and
/// nothing else).
fn split_formatted_tool_call(text: &str) -> (String, String) {
    let trimmed = text.trim();
    if let Some(command) = trimmed.strip_prefix("$ ") {
        return ("bash".to_string(), command.trim().to_string());
    }
    match trimmed.split_once(char::is_whitespace) {
        Some((name, args)) if !name.is_empty() => (name.to_string(), args.trim().to_string()),
        _ if trimmed.is_empty() => ("tool".to_string(), String::new()),
        _ => (trimmed.to_string(), String::new()),
    }
}

/// pi `readFleetTranscript` (`fleet-transcript.ts:384-404`) — validate, tail-read, parse, and
/// join every warning into one sanitized sentence.
#[must_use]
pub fn read_fleet_transcript(
    file_path: &Path,
    options: &FleetTranscriptReadOptions,
) -> FleetTranscript {
    let (resolved, warning) = validate_transcript_path(file_path, &options.trusted_roots);
    let Some(resolved) = resolved else {
        return FleetTranscript {
            path: file_path.to_path_buf(),
            events: Vec::new(),
            truncated: false,
            warning: warning.as_deref().map(safe_display_text),
        };
    };
    let max_records = options.max_records.unwrap_or(DEFAULT_MAX_RECORDS).max(1);
    let tail = read_tail_lines(&resolved, options.max_bytes.unwrap_or(DEFAULT_MAX_BYTES).max(1024));
    let records_omitted = tail.truncated || tail.lines.len() > max_records;
    let selected = tail
        .lines
        .get(tail.lines.len().saturating_sub(max_records)..)
        .unwrap_or(&tail.lines);
    let parsed = parse_transcript_lines(selected, records_omitted);
    let mut warnings: Vec<String> = Vec::new();
    if let Some(w) = tail.warning {
        warnings.push(w);
    }
    if parsed.malformed > 0 {
        warnings.push(format!(
            "Skipped {} malformed transcript record{}.",
            parsed.malformed,
            if parsed.malformed == 1 { "" } else { "s" }
        ));
    }
    FleetTranscript {
        path: file_path.to_path_buf(),
        truncated: tail.truncated || tail.lines.len() > max_records || parsed.explicit_truncation,
        events: parsed.events,
        warning: if warnings.is_empty() {
            None
        } else {
            Some(safe_display_text(&warnings.join(" ")))
        },
    }
}

// =================================================================================================
// Rendering (pi `fleet-transcript.ts:406-577`)
// =================================================================================================

/// pi `statusGlyph` (`fleet-transcript.ts:406-410`) — note this is the TOOL glyph set, distinct
/// from the item glyph in `fleet.ts:232-238`.
fn tool_status_glyph(status: ToolStatus) -> Span<'static> {
    match status {
        ToolStatus::Running => th::fg(Role::Warning, "●"),
        ToolStatus::Error => th::fg(Role::Error, "✗"),
        ToolStatus::Complete => th::fg(Role::Success, "✓"),
    }
}

/// pi `jsonScalar` (`fleet-transcript.ts:412-416`).
fn json_scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// pi `parseToolArgs` (`fleet-transcript.ts:418-425`).
fn parse_tool_args(event: &FleetToolEvent) -> Option<serde_json::Map<String, Value>> {
    let payload = event.args_payload.as_ref()?;
    serde_json::from_str::<Value>(payload).ok()?.as_object().cloned()
}

/// JS `Number.prototype.toFixed(1)` — rounds half AWAY FROM ZERO, where Rust's `{:.1}` rounds half
/// to even. `(1.25).toFixed(1)` is `"1.3"` in JS and `format!("{:.1}", 1.25)` is `"1.2"` in Rust,
/// so a token count landing exactly on a half-tenth would render one digit apart from upstream.
/// Duplicated (rather than cross-imported) per this crate's established convention for tiny
/// formatters — see `background/fleet_view.rs`'s own note on its private `format_tokens` copy.
fn to_fixed_1(value: f64) -> String {
    format!("{:.1}", (value * 10.0).round() / 10.0)
}

/// pi `toolDuration` (`fleet-transcript.ts:427-430`) — `"1.2s"`, only when both endpoints are
/// known.
#[must_use]
pub fn tool_duration(event: &FleetToolEvent) -> Option<String> {
    let started = event.started_at?;
    let ended = event.ended_at?;
    Some(format!("{}s", to_fixed_1((ended - started) as f64 / 1000.0)))
}

/// pi `bounded` (`fleet-transcript.ts:488-490`).
fn bounded(line: Line<'static>, width: usize) -> Line<'static> {
    th::clip(&line, width)
}

/// pi `railLine` (`fleet-transcript.ts:492-494`) — `│ ` then the content, clipped to `width`.
fn rail_line(content: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let mut spans = vec![th::fg(Role::BorderMuted, "│"), th::raw(" ")];
    spans.extend(content);
    bounded(Line::from(spans), width)
}

/// pi `renderWrapped` (`fleet-transcript.ts:496-498`).
fn render_wrapped(spans: Vec<Span<'static>>, width: usize) -> Vec<Line<'static>> {
    th::wrap_line(&Line::from(spans), width.max(1))
}

/// pi `renderExpandedTool` (`fleet-transcript.ts:432-486`) — the `x`/`Ctrl+O` expanded form, with
/// its two special cases (`bash` shows `$ command` + raw output + `Took Ns`; `read` shows
/// `read <path>` + the file body) and the generic `name` / `args` / `output` fallback.
fn render_expanded_tool(event: &FleetToolEvent, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let args = parse_tool_args(event);
    let glyph = tool_status_glyph(event.status);
    let output = event.output.as_ref().or(event.error.as_ref());
    let output_role = if event.status == ToolStatus::Error { Role::Error } else { Role::ToolOutput };
    let body_width = width.saturating_sub(4).max(1);

    if event.name == "bash" {
        let command = json_scalar(args.as_ref().and_then(|a| a.get("command")))
            .or_else(|| event.args.clone())
            .unwrap_or_else(|| "(unknown command)".to_string());
        lines.push(rail_line(
            vec![glyph, th::raw(" "), th::fg_bold(Role::ToolTitle, format!("$ {command}"))],
            width,
        ));
        if let Some(output) = output {
            for output_line in output.trim_end().split('\n') {
                for wrapped in render_wrapped(vec![th::fg(output_role, output_line)], body_width) {
                    let mut spans = vec![th::raw("  ")];
                    spans.extend(wrapped.spans);
                    lines.push(rail_line(spans, width));
                }
            }
        }
        if let Some(duration) = tool_duration(event) {
            lines.push(rail_line(vec![th::fg(Role::Dim, format!("  Took {duration}"))], width));
        }
        return lines;
    }

    if event.name == "read" {
        let file_path = json_scalar(
            args.as_ref()
                .and_then(|a| a.get("path").or_else(|| a.get("file_path"))),
        );
        // Delta 2: pi highlights by language here; cyrup takes pi's own `language === undefined`
        // branch (`:465`) — plain lines, error-coloured when the read failed.
        let rendered: Vec<Line<'static>> = match output {
            None => Vec::new(),
            Some(output) if event.status == ToolStatus::Error => output
                .split('\n')
                .map(|l| Line::from(vec![th::fg(Role::Error, l)]))
                .collect(),
            Some(output) => output.split('\n').map(|l| Line::from(vec![th::raw(l)])).collect(),
        };
        let title = format!(
            "read {}",
            file_path
                .or_else(|| event.args.clone())
                .unwrap_or_default()
        );
        lines.push(rail_line(
            vec![glyph, th::raw(" "), th::fg_bold(Role::ToolTitle, title)],
            width,
        ));
        for line in rendered {
            for wrapped in th::wrap_line(&line, body_width) {
                let mut spans = vec![th::raw("  ")];
                spans.extend(wrapped.spans);
                lines.push(rail_line(spans, width));
            }
        }
        return lines;
    }

    lines.push(rail_line(
        vec![glyph, th::raw(" "), th::fg_bold(Role::ToolTitle, event.name.clone())],
        width,
    ));
    if let Some(payload) = event.args_payload.as_ref() {
        lines.push(rail_line(vec![th::fg(Role::Dim, "  args")], width));
        for arg_line in payload.split('\n') {
            for wrapped in render_wrapped(vec![th::fg(Role::Muted, arg_line)], body_width) {
                let mut spans = vec![th::raw("  ")];
                spans.extend(wrapped.spans);
                lines.push(rail_line(spans, width));
            }
        }
    }
    if let Some(output) = output {
        let (role, label) = if event.status == ToolStatus::Error {
            (Role::Error, "  error")
        } else {
            (Role::Dim, "  output")
        };
        lines.push(rail_line(vec![th::fg(role, label)], width));
        for output_line in output.split('\n') {
            for wrapped in render_wrapped(vec![th::fg(output_role, output_line)], body_width) {
                let mut spans = vec![th::raw("  ")];
                spans.extend(wrapped.spans);
                lines.push(rail_line(spans, width));
            }
        }
    }
    lines
}

/// pi `renderFleetTranscript` (`fleet-transcript.ts:500-577`) — the whole detail-pane body.
///
/// `expanded_tools` is pi's `options.expandedTools`, toggled by the inspector's `x` / `Ctrl+O`
/// binding (`fleet.ts:708-712`).
#[must_use]
pub fn render_fleet_transcript(
    transcript: &FleetTranscript,
    width: usize,
    expanded_tools: bool,
) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    if transcript.truncated {
        lines.push(bounded(
            Line::from(vec![th::fg(Role::Dim, "↑ Earlier activity omitted")]),
            width,
        ));
    }
    if let Some(warning) = transcript.warning.as_ref() {
        for line in render_wrapped(
            vec![th::fg(Role::Warning, safe_display_text(warning))],
            width.saturating_sub(2).max(1),
        ) {
            let mut spans = vec![th::fg(Role::Warning, "!"), th::raw(" ")];
            spans.extend(line.spans);
            lines.push(bounded(Line::from(spans), width));
        }
    }

    for raw_event in &transcript.events {
        let event = safe_transcript_event(raw_event);
        match &event {
            FleetTranscriptEvent::Tool(tool) => {
                if expanded_tools
                    && (tool.output.is_some()
                        || tool.args_payload.is_some()
                        || tool.error.is_some())
                {
                    lines.extend(render_expanded_tool(tool, width));
                    lines.push(rail_line(vec![th::fg(Role::Dim, "  x to collapse")], width));
                    continue;
                }
                let mut head = vec![
                    th::fg(Role::BorderMuted, "├─"),
                    th::raw(" "),
                    tool_status_glyph(tool.status),
                    th::raw(" "),
                    th::fg_bold(Role::ToolTitle, tool.name.clone()),
                ];
                if let Some(args) = tool.args.as_ref() {
                    head.push(th::raw(" "));
                    head.push(th::fg(Role::Dim, args.clone()));
                }
                if tool.status == ToolStatus::Running {
                    head.push(th::fg(Role::Warning, " running"));
                }
                lines.push(bounded(Line::from(head), width));

                let body_width = width.saturating_sub(4).max(1);
                if let Some(output) = tool.output.as_ref()
                    && tool.status != ToolStatus::Error
                    && tool.name == "bash"
                {
                    let output_lines: Vec<&str> = output.trim_end().split('\n').collect();
                    let hidden = output_lines.len().saturating_sub(TOOL_PREVIEW_LINES);
                    let visible = output_lines
                        .get(hidden..)
                        .unwrap_or(&output_lines)
                        .to_vec();
                    for output_line in visible {
                        for wrapped in
                            render_wrapped(vec![th::fg(Role::ToolOutput, output_line)], body_width)
                        {
                            let mut spans = vec![th::raw("  ")];
                            spans.extend(wrapped.spans);
                            lines.push(rail_line(spans, width));
                        }
                    }
                    if hidden > 0 {
                        lines.push(rail_line(
                            vec![th::fg(
                                Role::Dim,
                                format!("  … {hidden} earlier lines · x to expand"),
                            )],
                            width,
                        ));
                    }
                    let duration = tool_duration(tool)
                        .map(|d| format!(" {d}"))
                        .unwrap_or_default();
                    lines.push(rail_line(
                        vec![th::fg(Role::Dim, format!("  Took{duration}"))],
                        width,
                    ));
                } else if let Some(output) = tool.output.as_ref()
                    && tool.status != ToolStatus::Error
                {
                    let collapsed = collapse_whitespace(output);
                    let summary = th::clip_str(&collapsed, width.saturating_sub(18).max(1));
                    let summary = if th::str_width(&collapsed) > width.saturating_sub(18).max(1) {
                        format!("{summary}…")
                    } else {
                        summary
                    };
                    if !summary.is_empty() {
                        lines.push(rail_line(
                            vec![th::fg(Role::Dim, format!("  {summary} · x to expand"))],
                            width,
                        ));
                    }
                }
                if let Some(error) = tool.error.as_ref() {
                    for error_line in render_wrapped(vec![th::raw(error.clone())], body_width) {
                        let text: String = error_line
                            .spans
                            .iter()
                            .map(|s| s.content.as_ref())
                            .collect();
                        lines.push(rail_line(vec![th::fg(Role::Error, format!("  {text}"))], width));
                    }
                }
            }
            FleetTranscriptEvent::Notice { text, tone, .. } => {
                let role = match tone {
                    NoticeTone::Error => Role::Error,
                    NoticeTone::Warning => Role::Warning,
                    NoticeTone::Muted => Role::Dim,
                };
                for notice_line in
                    render_wrapped(vec![th::raw(text.clone())], width.saturating_sub(2).max(1))
                {
                    let content: String =
                        notice_line.spans.iter().map(|s| s.content.as_ref()).collect();
                    lines.push(rail_line(vec![th::fg(role, content)], width));
                }
            }
            FleetTranscriptEvent::Assistant { text, model, .. } => {
                lines.extend(render_message(true, text, model.as_deref(), width));
            }
            FleetTranscriptEvent::User { text, .. } => {
                lines.extend(render_message(false, text, None, width));
            }
        }
    }

    // pi `:575` — drop trailing rail-only spacer lines.
    while lines
        .last()
        .is_some_and(|line| th::line_width(line) == 1)
    {
        lines.pop();
    }
    lines
}

/// pi's assistant/supervisor branch (`fleet-transcript.ts:559-572`). See delta 1 for why the
/// assistant body is word-wrapped rather than markdown-rendered.
fn render_message(
    assistant: bool,
    text: &str,
    model: Option<&str>,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let label = if assistant { "Assistant" } else { "Supervisor" };
    let marker = if assistant {
        th::fg(Role::Accent, "◆")
    } else {
        th::fg(Role::Warning, "◇")
    };
    let mut head = vec![marker, th::raw(" "), th::bold(label)];
    if assistant && let Some(model) = model {
        head.push(th::fg(Role::Dim, format!(" · {model}")));
    }
    lines.push(bounded(Line::from(head), width));
    for body in render_wrapped(vec![th::raw(text.to_string())], width.saturating_sub(2).max(1)) {
        lines.push(rail_line(body.spans, width));
    }
    lines.push(Line::from(vec![th::fg(Role::BorderMuted, "│")]));
    lines
}

/// JS `String.replace(/\s+/g, " ").trim()` — the collapse `fleet-transcript.ts:541` applies before
/// summarizing a tool's output on one row.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_transcript(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
        path
    }

    // -----------------------------------------------------------------------------------------
    // The cyrup record rewrite (module doc item 3) — against the files cyrup ACTUALLY writes
    // -----------------------------------------------------------------------------------------

    /// The artifact [`super::super::fleet::transcript_target`] points at for a foreground child is
    /// written by `artifacts::run_artifact_jsonl_lines`, whose vocabulary is `tool_call`/`result`.
    /// Before the rewrite, neither tag was recognised at all and this pane was EMPTY.
    #[test]
    fn the_settled_run_artifact_yields_conversation_and_tool_rows() {
        let lines: Vec<String> = vec![
            r#"{"type":"tool_call","text":"$ ls -la","expandedText":"$ ls -la /very/long/path"}"#.into(),
            r#"{"type":"tool_call","text":"read src/main.rs","expandedText":"read src/main.rs"}"#.into(),
            r#"{"type":"result","agent":"reviewer","exitCode":0,"model":"opus","output":"All good.","error":null}"#.into(),
        ];
        let parsed = parse_transcript_lines(&lines, false);
        assert!(!parsed.events.is_empty(), "the real artifact must not parse to nothing");

        let tools: Vec<&FleetToolEvent> = parsed
            .events
            .iter()
            .filter_map(|e| match e {
                FleetTranscriptEvent::Tool(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 2, "one row per recorded call");
        assert_eq!(tools[0].name, "bash", "the `$ ` prefix identifies bash");
        assert_eq!(tools[0].args.as_deref(), Some("ls -la"));
        assert_eq!(tools[0].status, ToolStatus::Complete, "a settled run's tools are not running");
        assert_eq!(tools[1].name, "read");
        assert_eq!(tools[1].args.as_deref(), Some("src/main.rs"));

        let assistant = parsed.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::Assistant { text, model, .. } => Some((text, model)),
            _ => None,
        });
        let (text, model) = assistant.expect("the run's final output IS the conversation");
        assert_eq!(text, "All good.");
        assert_eq!(model.as_deref(), Some("opus"));
    }

    /// The test above hand-writes the artifact's JSON, which can silently drift from the writer.
    /// This one drives the REAL producer chain end to end —
    /// [`crate::exec::ToolCallSummary::from_call`] -> [`crate::exec::SingleResult`] ->
    /// [`crate::artifacts::run_artifact_jsonl_lines`] -> the bytes this pane reads -> rendered
    /// [`Line`]s — so a change to either side that breaks the pane fails HERE rather than shipping
    /// an empty inspector.
    #[test]
    fn the_writers_own_output_renders_as_conversation_and_tool_rows() {
        let result = crate::exec::SingleResult {
            agent: "reviewer".to_string(),
            task: "task".to_string(),
            exit_code: 0,
            usage: Default::default(),
            model: Some(cyrup_core::ModelId::from("claude-opus-4-8")),
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: Some("The change is safe.".to_string()),
            structured_output: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            stopped: false,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: vec![
                crate::exec::ToolCallSummary::from_call(
                    "bash",
                    &serde_json::json!({ "command": "cargo test -p cyrup-ext-subagents" }),
                ),
                crate::exec::ToolCallSummary::from_call(
                    "read",
                    &serde_json::json!({ "file_path": "src/lib.rs" }),
                ),
            ],
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
        };

        // Exactly the bytes `extension.rs:4926` / `background/runner_main.rs:2612` write.
        let lines = crate::artifacts::run_artifact_jsonl_lines(&result);
        let parsed = parse_transcript_lines(&lines, false);
        assert!(
            !parsed.events.is_empty(),
            "the writer's own output must not parse to nothing — that is the empty-pane bug"
        );

        let tools: Vec<&FleetToolEvent> = parsed
            .events
            .iter()
            .filter_map(|e| match e {
                FleetTranscriptEvent::Tool(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tools.len(), 2, "one row per call the writer recorded");
        assert_eq!(tools[0].name, "bash");
        assert_eq!(tools[0].args.as_deref(), Some("cargo test -p cyrup-ext-subagents"));
        assert_eq!(tools[0].status, ToolStatus::Complete);
        assert_eq!(tools[1].name, "read");
        assert_eq!(tools[1].args.as_deref(), Some("src/lib.rs"));

        let (text, model) = parsed
            .events
            .iter()
            .find_map(|e| match e {
                FleetTranscriptEvent::Assistant { text, model, .. } => Some((text, model)),
                _ => None,
            })
            .expect("the run's final output IS the conversation this artifact preserves");
        assert_eq!(text, "The change is safe.");
        assert_eq!(model.as_deref(), Some("claude-opus-4-8"));

        // …and it reaches the painted pane, with the real text in real spans.
        let transcript = transcript_with(parsed.events.clone());
        let rendered = render_fleet_transcript(&transcript, 60, false);
        let flat = th::lines_text(&rendered);
        assert!(flat.contains("The change is safe."), "the assistant turn is painted");
        assert!(flat.contains("cargo test -p cyrup-ext-subagents"), "the bash args are painted");
        assert!(flat.contains("src/lib.rs"), "the read args are painted");
    }

    #[test]
    fn a_failed_run_records_its_error_as_a_notice() {
        let lines = vec![
            r#"{"type":"result","agent":"a","exitCode":1,"output":null,"error":"boom"}"#.to_string(),
        ];
        let parsed = parse_transcript_lines(&lines, false);
        let notice = parsed.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::Notice { text, tone, .. } => Some((text.clone(), *tone)),
            _ => None,
        });
        assert_eq!(notice, Some(("boom".to_string(), NoticeTone::Error)));
    }

    /// The raw child NDJSON stream. Its message body is a `content` ARRAY, which is exactly what a
    /// tag-only rename could not read.
    #[test]
    fn a_content_array_message_becomes_a_rendered_turn() {
        let lines = vec![
            r#"{"type":"message_end","message":{"role":"assistant","model":"sonnet","timestamp":42,"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"hello there"}]}}"#.to_string(),
            r#"{"type":"message_end","message":{"role":"user","timestamp":43,"content":[{"type":"text","text":"do it"}]}}"#.to_string(),
        ];
        let parsed = parse_transcript_lines(&lines, false);
        let assistant = parsed.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::Assistant { text, model, timestamp } => {
                Some((text.clone(), model.clone(), *timestamp))
            }
            _ => None,
        });
        assert_eq!(
            assistant,
            Some(("hello there".to_string(), Some("sonnet".to_string()), Some(42))),
            "the text block must be lifted out of the content array; thinking must not"
        );
        let user = parsed.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::User { text, .. } => Some(text.clone()),
            _ => None,
        });
        assert_eq!(user, Some("do it".to_string()), "a supervisor turn after an assistant one");
    }

    #[test]
    fn a_tool_execution_pair_carries_its_arguments_and_its_output() {
        let lines = vec![
            r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"command":"echo hi"}}"#.to_string(),
            r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"bash","isError":false,"result":{"content":[{"type":"text","text":"hi"}]}}"#.to_string(),
        ];
        let parsed = parse_transcript_lines(&lines, false);
        let tool = parsed.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::Tool(t) => Some(t),
            _ => None,
        });
        let tool = tool.expect("one tool row");
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.status, ToolStatus::Complete);
        assert_eq!(tool.args.as_deref(), Some("echo hi"), "args object must become a preview");
        assert!(tool.args_payload.is_some(), "the full args object must survive for `x` expansion");
        assert_eq!(
            tool.output.as_deref(),
            Some("hi"),
            "the tool's output rides inside `result.content` and must reach the row"
        );
    }

    #[test]
    fn a_failed_tool_execution_end_paints_as_an_error() {
        let lines = vec![
            r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"read","args":{"path":"/nope"}}"#.to_string(),
            r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"read","isError":true,"result":{"content":[{"type":"text","text":"ENOENT"}]}}"#.to_string(),
        ];
        let parsed = parse_transcript_lines(&lines, false);
        let tool = parsed.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::Tool(t) => Some(t),
            _ => None,
        });
        let tool = tool.expect("one tool row");
        assert_eq!(tool.status, ToolStatus::Error);
        assert_eq!(tool.error.as_deref(), Some("ENOENT"));
    }

    #[test]
    fn pis_own_record_type_vocabulary_still_wins_untouched() {
        // A record carrying BOTH must be read as pi's — the rewrite is the fallback, not an override.
        let lines = vec![
            r#"{"recordType":"message","type":"tool_call","role":"assistant","text":"pi wins"}"#.to_string(),
        ];
        let parsed = parse_transcript_lines(&lines, false);
        assert!(matches!(
            parsed.events.first(),
            Some(FleetTranscriptEvent::Assistant { text, .. }) if text == "pi wins"
        ));
    }

    #[test]
    fn lifecycle_only_records_rewrite_to_nothing() {
        for tag in ["agent_start", "turn_start", "turn_end", "message_start", "message_update", "agent_settled"] {
            let lines = vec![format!(r#"{{"type":"{tag}"}}"#)];
            let parsed = parse_transcript_lines(&lines, false);
            assert!(parsed.events.is_empty(), "{tag} must render nothing");
            assert_eq!(parsed.malformed, 0, "{tag} is well-formed, just not displayable");
        }
    }

    #[test]
    fn a_formatted_tool_preview_splits_back_into_name_and_args() {
        assert_eq!(split_formatted_tool_call("$ git status"), ("bash".into(), "git status".into()));
        assert_eq!(split_formatted_tool_call("edit a/b.rs"), ("edit".into(), "a/b.rs".into()));
        assert_eq!(
            split_formatted_tool_call("grep {\"pattern\":\"x\"}"),
            ("grep".into(), "{\"pattern\":\"x\"}".into())
        );
        assert_eq!(split_formatted_tool_call("bare"), ("bare".into(), String::new()));
        assert_eq!(split_formatted_tool_call("   "), ("tool".into(), String::new()));
    }

    // -----------------------------------------------------------------------------------------
    // Painted-cell style assertions — the half `lines_text` cannot see
    // -----------------------------------------------------------------------------------------

    fn transcript_of(events: Vec<FleetTranscriptEvent>) -> FleetTranscript {
        FleetTranscript {
            path: PathBuf::from("/tmp/t.jsonl"),
            events,
            truncated: false,
            warning: None,
        }
    }

    #[test]
    fn the_assistant_and_supervisor_markers_paint_different_colours() {
        let transcript = transcript_of(vec![
            FleetTranscriptEvent::Assistant {
                text: "answer".into(),
                model: Some("opus".into()),
                timestamp: None,
            },
            FleetTranscriptEvent::User { text: "ask".into(), timestamp: None },
        ]);
        let lines = render_fleet_transcript(&transcript, 60, false);
        assert!(
            th::paints_as(th::painted_style(&lines, 60, "◆"), Role::Accent),
            "the assistant marker is accent"
        );
        assert!(
            th::paints_as(th::painted_style(&lines, 60, "◇"), Role::Warning),
            "the supervisor marker is warning — same shape family, different colour"
        );
        assert!(
            th::paints_as(th::painted_style(&lines, 60, "│"), Role::BorderMuted),
            "the rail is border-muted"
        );
        assert!(
            th::paints_as(th::painted_style(&lines, 60, "· opus"), Role::Dim),
            "the model suffix is dim"
        );
    }

    #[test]
    fn the_three_tool_status_glyphs_paint_three_distinct_colours() {
        for (status, glyph, role) in [
            (ToolStatus::Running, "●", Role::Warning),
            (ToolStatus::Complete, "✓", Role::Success),
            (ToolStatus::Error, "✗", Role::Error),
        ] {
            let transcript = transcript_of(vec![FleetTranscriptEvent::Tool(FleetToolEvent {
                name: "bash".into(),
                status,
                args: Some("echo".into()),
                ..FleetToolEvent::default()
            })]);
            let lines = render_fleet_transcript(&transcript, 60, false);
            let painted = th::painted_style(&lines, 60, glyph);
            assert!(th::paints_as(painted, role), "{glyph} must paint as {role:?}");
        }
    }

    #[test]
    fn a_tool_row_paints_its_name_title_coloured_and_its_args_dim() {
        let transcript = transcript_of(vec![FleetTranscriptEvent::Tool(FleetToolEvent {
            name: "grep".into(),
            status: ToolStatus::Complete,
            args: Some("needle".into()),
            ..FleetToolEvent::default()
        })]);
        let lines = render_fleet_transcript(&transcript, 60, false);
        assert!(th::paints_as(th::painted_style(&lines, 60, "grep"), Role::ToolTitle));
        assert!(th::paints_as(th::painted_style(&lines, 60, "needle"), Role::Dim));
        assert!(th::paints_as(th::painted_style(&lines, 60, "├─"), Role::BorderMuted));
    }

    #[test]
    fn each_notice_tone_paints_its_own_colour() {
        for (tone, role, text) in [
            (NoticeTone::Error, Role::Error, "bad"),
            (NoticeTone::Warning, Role::Warning, "careful"),
            (NoticeTone::Muted, Role::Dim, "quiet"),
        ] {
            let transcript = transcript_of(vec![FleetTranscriptEvent::Notice {
                text: text.into(),
                tone,
                timestamp: None,
            }]);
            let lines = render_fleet_transcript(&transcript, 40, false);
            assert!(
                th::paints_as(th::painted_style(&lines, 40, text), role),
                "{tone:?} must paint as {role:?}"
            );
        }
    }

    #[test]
    fn a_transcript_warning_paints_warning_and_the_truncation_hint_paints_dim() {
        let transcript = FleetTranscript {
            path: PathBuf::from("/tmp/t.jsonl"),
            events: vec![FleetTranscriptEvent::User { text: "hi".into(), timestamp: None }],
            truncated: true,
            warning: Some("clipped".into()),
        };
        let lines = render_fleet_transcript(&transcript, 60, false);
        assert!(th::paints_as(th::painted_style(&lines, 60, "clipped"), Role::Warning));
        assert!(th::paints_as(
            th::painted_style(&lines, 60, "↑ Earlier activity omitted"),
            Role::Dim
        ));
    }

    // -----------------------------------------------------------------------------------------
    // Sanitization (pi :12-92)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn escapes_terminal_control_and_bidi_code_points() {
        assert_eq!(safe_display_text("a\x1b[2Jb"), "a[U+001B][2Jb");
        assert_eq!(safe_display_text("a\u{202e}b"), "a[U+202E]b");
        // tab and newline are explicitly allowed through (pi :13)
        assert_eq!(safe_display_text("a\tb\nc"), "a\tb\nc");
    }

    #[test]
    fn crlf_is_normalized_before_escaping() {
        assert_eq!(safe_display_text("a\r\nb"), "a\nb");
    }

    #[test]
    fn binary_looking_payloads_are_replaced_wholesale() {
        let binary = "\u{1}\u{2}\u{3}\u{4}abcd";
        assert_eq!(safe_display_text(binary), BINARY_CONTENT_PLACEHOLDER);
        assert!(looks_like_binary_content("a\0b"));
        assert!(!looks_like_binary_content("plain text"));
    }

    #[test]
    fn sanitize_json_rewrites_keys_as_well_as_values() {
        let value: Value = serde_json::json!({ "a\u{202e}": "v\u{202e}" });
        let (sanitized, changed) = sanitize_json_display_value(&value);
        assert!(changed);
        assert_eq!(sanitized["a[U+202E]"], Value::String("v[U+202E]".into()));
    }

    #[test]
    fn tool_args_payload_falls_back_to_text_sanitization_when_unparseable() {
        assert_eq!(safe_tool_args_payload("not json\u{202e}"), "not json[U+202E]");
    }

    // -----------------------------------------------------------------------------------------
    // Path validation (pi :159-186)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn empty_trusted_roots_refuses_the_read() {
        let (resolved, warning) = validate_transcript_path(Path::new("/tmp/x.jsonl"), &[]);
        assert!(resolved.is_none());
        assert!(warning.unwrap().contains("no trusted root"));
    }

    #[test]
    fn path_outside_trusted_roots_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let other = tempfile::TempDir::new().unwrap();
        let path = write_transcript(other.path(), "t.jsonl", &["{}"]);
        let (resolved, warning) =
            validate_transcript_path(&path, &[dir.path().to_path_buf()]);
        assert!(resolved.is_none());
        assert!(warning.unwrap().contains("outside trusted roots"));
    }

    #[test]
    fn missing_file_is_neither_resolved_nor_warned() {
        let dir = tempfile::TempDir::new().unwrap();
        let (resolved, warning) =
            validate_transcript_path(&dir.path().join("absent.jsonl"), &[dir.path().to_path_buf()]);
        assert!(resolved.is_none());
        assert!(warning.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_transcript_is_refused() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = write_transcript(dir.path(), "real.jsonl", &["{}"]);
        let link = dir.path().join("link.jsonl");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let (resolved, warning) = validate_transcript_path(&link, &[dir.path().to_path_buf()]);
        assert!(resolved.is_none());
        assert!(warning.unwrap().contains("refused a symlink"));
    }

    #[test]
    fn directory_is_not_a_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let (resolved, warning) = validate_transcript_path(&sub, &[dir.path().to_path_buf()]);
        assert!(resolved.is_none());
        assert!(warning.unwrap().contains("not a file"));
    }

    // -----------------------------------------------------------------------------------------
    // Parsing (pi :283-404)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn parses_pi_record_type_vocabulary() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_transcript(
            dir.path(),
            "t.jsonl",
            &[
                r#"{"recordType":"message","role":"assistant","text":"hello","model":"m1","ts":1000}"#,
                r#"{"recordType":"tool_start","toolName":"bash","toolCallId":"c1","argsPreview":"ls","ts":1100}"#,
                r#"{"recordType":"message","role":"toolResult","toolCallId":"c1","text":"a\nb","ts":1600}"#,
                r#"{"recordType":"message","role":"user","text":"go on","ts":1700}"#,
                r#"{"recordType":"stderr","text":"boom","ts":1800}"#,
            ],
        );
        let transcript = read_fleet_transcript(
            &path,
            &FleetTranscriptReadOptions {
                trusted_roots: vec![dir.path().to_path_buf()],
                ..FleetTranscriptReadOptions::default()
            },
        );
        assert_eq!(transcript.warning, None);
        assert_eq!(
            transcript.events.iter().map(FleetTranscriptEvent::kind).collect::<Vec<_>>(),
            vec!["assistant", "tool", "user", "notice"]
        );
        let FleetTranscriptEvent::Tool(tool) = &transcript.events[1] else {
            panic!("expected tool event");
        };
        assert_eq!(tool.status, ToolStatus::Complete);
        assert_eq!(tool.output.as_deref(), Some("a\nb"));
        assert_eq!(tool.started_at, Some(1100));
        assert_eq!(tool.ended_at, Some(1600));
        assert_eq!(tool_duration(tool).as_deref(), Some("0.5s"));
        // JS `toFixed(1)` rounds half away from zero (Rust's `{:.1}` would print "1.2s").
        assert_eq!(
            tool_duration(&FleetToolEvent {
                started_at: Some(0),
                ended_at: Some(1250),
                ..FleetToolEvent::default()
            })
            .as_deref(),
            Some("1.3s")
        );
        // pi deletes `resultSeen` before returning.
        assert!(!tool.result_seen);
    }

    #[test]
    fn a_user_record_before_any_assistant_record_is_dropped() {
        let parsed = parse_transcript_lines(
            &[r#"{"recordType":"message","role":"user","text":"early"}"#.to_string()],
            false,
        );
        assert!(parsed.events.is_empty());
        // …unless the tail told us the conversation had already started (pi's
        // `conversationStarted` parameter).
        let parsed = parse_transcript_lines(
            &[r#"{"recordType":"message","role":"user","text":"early"}"#.to_string()],
            true,
        );
        assert_eq!(parsed.events.len(), 1);
    }

    #[test]
    fn repeated_identical_assistant_messages_collapse() {
        let parsed = parse_transcript_lines(
            &[
                r#"{"recordType":"message","role":"assistant","text":"same"}"#.to_string(),
                r#"{"recordType":"message","role":"assistant","text":"same"}"#.to_string(),
            ],
            false,
        );
        assert_eq!(parsed.events.len(), 1);
    }

    #[test]
    fn malformed_records_are_counted_not_swallowed() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_transcript(
            dir.path(),
            "t.jsonl",
            &["not json", "[1,2]", r#"{"recordType":"message","role":"assistant","text":"ok"}"#],
        );
        let transcript = read_fleet_transcript(
            &path,
            &FleetTranscriptReadOptions {
                trusted_roots: vec![dir.path().to_path_buf()],
                ..FleetTranscriptReadOptions::default()
            },
        );
        assert_eq!(transcript.events.len(), 1);
        assert_eq!(
            transcript.warning.as_deref(),
            Some("Skipped 2 malformed transcript records.")
        );
    }

    #[test]
    fn explicit_truncation_marker_sets_the_flag() {
        let parsed = parse_transcript_lines(&[r#"{"recordType":"truncated"}"#.to_string()], false);
        assert!(parsed.explicit_truncation);
    }

    #[test]
    fn record_cap_truncates_and_flags() {
        let dir = tempfile::TempDir::new().unwrap();
        let lines: Vec<String> = (0..10)
            .map(|i| format!(r#"{{"recordType":"message","role":"assistant","text":"m{i}"}}"#))
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let path = write_transcript(dir.path(), "t.jsonl", &refs);
        let transcript = read_fleet_transcript(
            &path,
            &FleetTranscriptReadOptions {
                trusted_roots: vec![dir.path().to_path_buf()],
                max_records: Some(3),
                ..FleetTranscriptReadOptions::default()
            },
        );
        assert!(transcript.truncated);
        assert_eq!(transcript.events.len(), 3);
    }

    #[test]
    fn cyrup_ndjson_tags_map_onto_the_same_event_semantics() {
        // [CYRUP-DELTA] module doc item 3.
        //
        // FIXTURE CORRECTED: this test used to feed records with `role`/`text`/`ts` at the TOP
        // level of a `message_end` and an `argsPreview` STRING on a `tool_execution_start`. Cyrup
        // emits neither — `SubagentEvent::MessageEnd` wraps a whole `AgentMessage` (whose body is a
        // `content` array) and `ToolExecutionStart` carries an `args` OBJECT
        // (`exec/ndjson.rs:113-155`). The old fixture therefore proved the mapping against a file
        // that does not exist, which is how the pane could pass this test and still render nothing.
        // The assertions below are unchanged and extended; only the input is now real.
        let dir = tempfile::TempDir::new().unwrap();
        let path = write_transcript(
            dir.path(),
            "t.jsonl",
            &[
                r#"{"type":"message_end","message":{"role":"assistant","model":"opus","timestamp":10,"content":[{"type":"text","text":"from cyrup"}]}}"#,
                r#"{"type":"tool_execution_start","toolName":"bash","toolCallId":"c9","args":{"command":"pwd"}}"#,
                r#"{"type":"tool_execution_end","toolCallId":"c9","toolName":"bash","isError":true,"result":{"content":[{"type":"text","text":"permission denied"}]}}"#,
            ],
        );
        let transcript = read_fleet_transcript(
            &path,
            &FleetTranscriptReadOptions {
                trusted_roots: vec![dir.path().to_path_buf()],
                ..FleetTranscriptReadOptions::default()
            },
        );
        assert_eq!(
            transcript.events.iter().map(FleetTranscriptEvent::kind).collect::<Vec<_>>(),
            vec!["assistant", "tool"]
        );
        let FleetTranscriptEvent::Assistant { text, model, timestamp } = &transcript.events[0]
        else {
            panic!("expected assistant event");
        };
        assert_eq!(text, "from cyrup");
        assert_eq!(model.as_deref(), Some("opus"));
        assert_eq!(*timestamp, Some(10));
        let FleetTranscriptEvent::Tool(tool) = &transcript.events[1] else {
            panic!("expected tool event");
        };
        assert_eq!(tool.status, ToolStatus::Error);
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.args.as_deref(), Some("pwd"));
        assert_eq!(tool.error.as_deref(), Some("permission denied"));
    }

    #[test]
    fn control_sequences_in_a_record_never_reach_the_rendered_output() {
        let parsed = parse_transcript_lines(
            &[r#"{"recordType":"message","role":"assistant","text":"a\u001b[2Jb"}"#.to_string()],
            false,
        );
        let FleetTranscriptEvent::Assistant { text, .. } = &parsed.events[0] else {
            panic!("expected assistant");
        };
        assert!(!text.contains('\u{1b}'));
        assert!(text.contains("[U+001B]"));
    }

    // -----------------------------------------------------------------------------------------
    // Rendering (pi :500-577)
    // -----------------------------------------------------------------------------------------

    fn transcript_with(events: Vec<FleetTranscriptEvent>) -> FleetTranscript {
        FleetTranscript { path: PathBuf::from("t.jsonl"), events, truncated: false, warning: None }
    }

    #[test]
    fn renders_assistant_and_supervisor_markers() {
        let transcript = transcript_with(vec![
            FleetTranscriptEvent::Assistant {
                text: "hi".into(),
                model: Some("m1".into()),
                timestamp: None,
            },
            FleetTranscriptEvent::User { text: "go".into(), timestamp: None },
        ]);
        let text = th::lines_text(&render_fleet_transcript(&transcript, 40, false));
        assert!(text.contains("◆ Assistant · m1"), "{text}");
        assert!(text.contains("◇ Supervisor"), "{text}");
    }

    #[test]
    fn collapsed_bash_tool_previews_the_output_tail_and_reports_hidden_lines() {
        let output = (0..12).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        let transcript = transcript_with(vec![FleetTranscriptEvent::Tool(FleetToolEvent {
            name: "bash".into(),
            args: Some("ls".into()),
            output: Some(output),
            status: ToolStatus::Complete,
            started_at: Some(0),
            ended_at: Some(2500),
            ..FleetToolEvent::default()
        })]);
        let text = th::lines_text(&render_fleet_transcript(&transcript, 60, false));
        assert!(text.contains("├─ ✓ bash ls"), "{text}");
        assert!(text.contains("line11"), "{text}");
        assert!(!text.contains("line4"), "{text}");
        assert!(text.contains("… 5 earlier lines · x to expand"), "{text}");
        assert!(text.contains("Took 2.5s"), "{text}");
    }

    #[test]
    fn expanded_bash_tool_shows_the_command_and_every_line() {
        let transcript = transcript_with(vec![FleetTranscriptEvent::Tool(FleetToolEvent {
            name: "bash".into(),
            args_payload: Some(r#"{"command":"echo hi"}"#.into()),
            output: Some("hi".into()),
            status: ToolStatus::Complete,
            ..FleetToolEvent::default()
        })]);
        let text = th::lines_text(&render_fleet_transcript(&transcript, 60, true));
        assert!(text.contains("$ echo hi"), "{text}");
        assert!(text.contains("x to collapse"), "{text}");
    }

    #[test]
    fn a_running_tool_renders_its_running_suffix() {
        let transcript = transcript_with(vec![FleetTranscriptEvent::Tool(FleetToolEvent {
            name: "grep".into(),
            status: ToolStatus::Running,
            ..FleetToolEvent::default()
        })]);
        let text = th::lines_text(&render_fleet_transcript(&transcript, 40, false));
        assert!(text.contains("● grep running"), "{text}");
    }

    #[test]
    fn a_warning_renders_behind_a_bang() {
        let mut transcript = transcript_with(Vec::new());
        transcript.warning = Some("something odd".into());
        transcript.truncated = true;
        let text = th::lines_text(&render_fleet_transcript(&transcript, 40, false));
        assert!(text.contains("↑ Earlier activity omitted"), "{text}");
        assert!(text.contains("! something odd"), "{text}");
    }

    #[test]
    fn zero_width_renders_nothing() {
        let transcript = transcript_with(vec![FleetTranscriptEvent::User {
            text: "x".into(),
            timestamp: None,
        }]);
        assert!(render_fleet_transcript(&transcript, 0, false).is_empty());
    }
}
