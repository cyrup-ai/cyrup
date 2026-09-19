//! The live child transcript: `<base>_transcript.jsonl`, written WHILE a child runs, one record
//! per parsed child event (pi `src/shared/child-transcript.ts` @v0.68.0).
//!
//! # Why
//!
//! The FleetView transcript pane ([`crate::tui::fleet::transcript_target`]) and `/subagents
//! status` ([`crate::extension::executor::foreground_transcript`]) both open
//! [`crate::artifacts::ArtifactPaths::transcript_path`] and fold it through
//! [`crate::tui::fleet_transcript::read_fleet_transcript`]. The reader landed before any writer,
//! so a running child rendered as nothing until it finished. [`ChildTranscriptWriter`] is the
//! writer half: it appends pi's record vocabulary to that file as each event is parsed.
//!
//! # Where it attaches
//!
//! Upstream feeds the writer from the PARSED child-event stream (`execution.ts:980`
//! `shared.transcriptWriter?.writeChildEvent(evt)`; `run-child-session.ts:410`). cyrup parses a
//! child's stdout in exactly one place, `exec::ndjson::parse_line` inside
//! `exec::drive_attempt::handle_child_line`, and both the foreground executor and the detached
//! background runner reach it through [`crate::exec::run_sync`], so a single feed there covers
//! both paths. Stderr is a separate diagnostic stream ([`crate::spawn::CapturedStderr`]) and is
//! not recorded here (see "Scope").
//!
//! # The record (pi `child-transcript.ts:108-121`, `:174-247`)
//!
//! One flat camelCase JSON object per line: a base of `version` (`1`), `recordType`, `source`,
//! `runId`, `agent`, `childIndex` (omitted when the run has none), `cwd`, `ts` (epoch ms) and
//! `timestamp` (ISO-8601), plus the kind-specific keys the reader dispatches on.
//! [`ChildTranscriptRecord`] is the serde mirror of upstream's per-kind object spreads: every
//! kind-specific field is an `Option` that skips serialization when absent, and the fields are
//! private so a record is only ever built through the per-kind constructors.
//!
//! The first record of every transcript is [`INITIAL_PROMPT_SENTINEL`]; the raw prompt is never
//! written (both upstream creators pass the constant, `subagent-runner.ts:889` /
//! `execution.ts:1849`), and [`ChildTranscriptWriter::write_initial_prompt_sentinel`] takes no
//! prompt parameter so this type cannot leak one.
//!
//! # Bounds
//!
//! * Tool payloads (a `toolResult` message's text, a `tool_start`'s pretty-printed `argsPayload`)
//!   are cut at [`MAX_TOOL_PAYLOAD_BYTES`] on a UTF-8 character boundary and marked with
//!   [`TOOL_PAYLOAD_TRUNCATION_MARKER`] ([`bounded_payload`] / [`bounded_text`], pi
//!   `boundedPayload` `:9-28`). Assistant/user text is not bounded here (`:197-207`).
//! * The whole file is capped at [`DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES`]: before each record the
//!   writer checks whether the record AND a trailing `truncated` marker still fit; on the first
//!   that would not, it writes the marker and drops everything after (`:142-158`).
//!
//! **[CYRUP-DELTA]** the file goes through [`crate::jsonl::BoundedJsonlWriter`], the crate's one
//! capped appender (`run_paths.rs`'s rule for every capped `.jsonl`), constructed with the same
//! cap. The substrate drops silently with no marker (`jsonl.rs` `write_line`), so the marker
//! pre-check is done here against its `bytes_written()`/`cap_bytes()`; the substrate's own cap
//! stays the backstop, so there is still exactly one byte-budget enforcer.
//!
//! **[CYRUP-DELTA]** [`ChildTranscriptWriter::create`] truncates the file
//! (`tokio::fs::write(path, "")`, upstream `:169`) before wrapping the substrate, because
//! `BoundedJsonlWriter::create` opens with `append(true)` and never truncates (`jsonl.rs`).
//!
//! # Failure surface
//!
//! A writer never fails the run. An initialisation or write failure is latched as
//! [`ChildTranscriptError`], every later write is a no-op, the path stays published, and
//! [`ChildTranscriptWriter::last_error`] (pi `getError`) hands the typed error to the result
//! boundary, where [`crate::exec::SingleResult::transcript_error`] carries its `Display` form
//! (upstream `transcriptError: transcriptWriter?.getError()`, `subagent-runner.ts:1559`).
//!
//! # Scope (disclosed, not a delta)
//!
//! Upstream's `writeStdoutLine`/`writeStderrLine`/`writeStderrText` (`:249-259`) have exactly ONE
//! feeder at v0.68.0 (`git grep` over `src/`): `src/runs/shared/child-hooks.ts:30-34`
//! `withChildSessionErrorReporting`, which installs an `onExtensionError` hook on the in-process
//! child session and writes one `stderr` record per contained extension fault —
//! `Extension error (<extensionPath>, <event>): <message>` — on BOTH paths
//! (`execution.ts:1381` and `run-child-session.ts:641`, through
//! `child-launch.ts:150` `createReportedChildSessionInput`). `writeStdoutLine` and
//! `writeStderrText` have no callers; no upstream path records the child's stderr stream.
//!
//! That record is not written here, because cyrup has no such EVENT on the wire this writer
//! records. cyrup's child is an OS subprocess in `--mode json` (`exec/spawn_plan.rs`), whose
//! extension faults go to its own stderr through `cyrup_modes::print::extension_error_sink`
//! (`eprintln!("Extension error ({id}): {error}")`, bound in `cyrup_modes::json`), never onto
//! its stdout event stream — [`SubagentEvent`] (`exec/ndjson.rs`) has no extension-error
//! variant, so `parse_line` cannot hand one to [`ChildTranscriptWriter::write_child_event`].
//! Those stderr lines reach the parent as bytes on [`crate::spawn::CapturedStderr`]'s pump task
//! (per line to `tracing`, the tail into the failure path), and turning that stream back into an
//! event by matching on the notice's text would record a heuristic, not the hook. The
//! [`TranscriptRecordType::Stdout`]/[`TranscriptRecordType::Stderr`] variants remain because the
//! reader renders them and the on-disk vocabulary is upstream's. A transcript written here never
//! carries a `stderr` record; a child's extension fault is visible in its stderr tail instead.
//!
//! One cosmetic non-delta: `argsPayload` is pretty-printed through `serde_json`, which orders
//! object keys alphabetically where JS keeps insertion order.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::exec::ndjson::{SubagentEvent, content_text};
use crate::exec::tool_call_summary::extract_tool_args_preview;
use crate::jsonl::BoundedJsonlWriter;

/// pi `CHILD_TRANSCRIPT_ARTIFACT_VERSION` (`child-transcript.ts:30`).
pub const CHILD_TRANSCRIPT_ARTIFACT_VERSION: u32 = 1;

/// pi `MAX_TOOL_PAYLOAD_BYTES` (`:6`): the cut applied to tool payloads only.
pub const MAX_TOOL_PAYLOAD_BYTES: usize = 32 * 1024;

/// pi `TOOL_PAYLOAD_TRUNCATION_MARKER` (`:7`). 23 bytes: U+2026 is three.
pub const TOOL_PAYLOAD_TRUNCATION_MARKER: &str = "\n\n… payload truncated";

/// pi `DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES` (`:31`, 50 MiB), the same figure as the crate-wide
/// [`crate::jsonl::DEFAULT_JSONL_CAP_BYTES`], read from there so the two cannot drift.
pub const DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES: u64 = crate::jsonl::DEFAULT_JSONL_CAP_BYTES;

/// The text of every transcript's first record: pi's `` `${PROMPT_REDACTED}; live Prompt Audit
/// only.` `` with `PROMPT_REDACTED = "[prompt redacted]"` (`src/shared/utils.ts:13`).
pub const INITIAL_PROMPT_SENTINEL: &str = "[prompt redacted]; live Prompt Audit only.";

/// pi `ChildTranscriptSource` (`:33`): which executor wrote the transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptSource {
    /// The foreground single-run executor (`execution.ts:1830-1850`).
    Foreground,
    /// The detached background runner (`subagent-runner.ts:870-889`).
    Async,
}

/// pi `ChildTranscriptRecordType` (`:34`): the `recordType` tag the reader dispatches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRecordType {
    /// A conversation turn: the sentinel, an assistant/user message, or a tool's result.
    Message,
    /// A tool call began.
    ToolStart,
    /// A tool call ended.
    ToolEnd,
    /// One raw stdout line (upstream `writeStdoutLine`; not produced here, see the module doc).
    Stdout,
    /// One raw stderr line (upstream `writeStderrLine`; not produced here, see the module doc).
    Stderr,
    /// The file-cap marker: nothing after it was recorded.
    Truncated,
}

/// The `sourceEventType` a record was projected from (upstream writes the raw `event.type`
/// string; `initial_prompt` is its own constant, `:215`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceEventType {
    /// The sentinel record.
    InitialPrompt,
    /// A `message_end` child event.
    MessageEnd,
    /// A `tool_execution_start` child event.
    ToolExecutionStart,
    /// A `tool_execution_end` child event.
    ToolExecutionEnd,
}

/// The `role` of a message record. The three named roles are the ones the reader dispatches on
/// (`"toolResult" | "tool_result"`, `"assistant"`, `"user"`); [`MessageRole::Other`] carries
/// cyrup's app/extension roles (`custom`, `bashExecution`, `branchSummary`, `compactionSummary`,
/// `cyrup_agent::AgentMessage`) verbatim, as upstream writes `role: message.role` for any role
/// rather than dropping the record.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessageRole {
    /// The prompt side of the conversation.
    #[serde(rename = "user")]
    User,
    /// A model turn.
    #[serde(rename = "assistant")]
    Assistant,
    /// A tool's output, on the wire as `toolResult`.
    #[serde(rename = "toolResult")]
    ToolResult,
    /// Any other role, carried as the wire string.
    #[serde(untagged)]
    Other(String),
}

impl MessageRole {
    fn from_wire(role: &str) -> Self {
        match role {
            "user" => Self::User,
            "assistant" => Self::Assistant,
            "toolResult" | "tool_result" => Self::ToolResult,
            other => Self::Other(other.to_string()),
        }
    }
}

/// Who and where: the base fields every record repeats (pi `ChildTranscriptWriterInput`,
/// `:52-60`, spread by `baseRecord` `:108-121`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptIdentity {
    /// Which executor is writing.
    pub source: TranscriptSource,
    /// The run this child belongs to (the same token the artifact bundle is named by).
    pub run_id: String,
    /// The child's agent name.
    pub agent: String,
    /// The child's fan-out index; upstream omits the key when undefined.
    pub child_index: Option<usize>,
    /// The child's working directory.
    pub cwd: PathBuf,
}

/// pi `normalizeUsage` (`:80-94`): the assistant turn's usage, reduced to five finite numbers.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptUsage {
    /// `input ?? inputTokens ?? 0`.
    pub input: u64,
    /// `output ?? outputTokens ?? 0`.
    pub output: u64,
    /// `cacheRead ?? 0`.
    pub cache_read: u64,
    /// `cacheWrite ?? 0`.
    pub cache_write: u64,
    /// `cost.total ?? cost ?? 0`.
    pub cost: f64,
}

impl TranscriptUsage {
    /// `None` unless `value` is an object (`:81`); numbers that are absent or non-finite read as
    /// `0`.
    fn normalize(value: &Value) -> Option<Self> {
        let raw = value.as_object()?;
        let count = |keys: &[&str]| {
            keys.iter()
                .find_map(|key| raw.get(*key).and_then(Value::as_u64))
                .unwrap_or(0)
        };
        let cost = match raw.get("cost") {
            Some(Value::Object(cost)) => cost.get("total").and_then(Value::as_f64),
            Some(cost) => cost.as_f64(),
            None => None,
        }
        .filter(|cost| cost.is_finite())
        .unwrap_or(0.0);
        Some(Self {
            input: count(&["input", "inputTokens"]),
            output: count(&["output", "outputTokens"]),
            cache_read: count(&["cacheRead"]),
            cache_write: count(&["cacheWrite"]),
            cost,
        })
    }
}

/// One JSONL line of the transcript. Flat, camelCase; every kind-specific key is omitted when
/// absent, mirroring upstream's per-kind object spreads (`:178-247`). Fields are private: the
/// constructors below are the only way to build one.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildTranscriptRecord {
    version: u32,
    record_type: TranscriptRecordType,
    source: TranscriptSource,
    run_id: String,
    agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child_index: Option<usize>,
    cwd: PathBuf,
    ts: i64,
    timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_event_type: Option<SourceEventType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<MessageRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stop_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<TranscriptUsage>,
    /// The message object for a `message` record; the marker sentence for a `truncated` one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    args_preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    args_payload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u64>,
}

impl ChildTranscriptRecord {
    /// pi `baseRecord(recordType)` (`:108-121`).
    fn base(identity: &TranscriptIdentity, record_type: TranscriptRecordType) -> Self {
        let ts = crate::time::now_epoch_millis();
        Self {
            version: CHILD_TRANSCRIPT_ARTIFACT_VERSION,
            record_type,
            source: identity.source,
            run_id: identity.run_id.clone(),
            agent: identity.agent.clone(),
            child_index: identity.child_index,
            cwd: identity.cwd.clone(),
            ts,
            timestamp: crate::time::format_iso8601_millis(ts),
            source_event_type: None,
            role: None,
            text: None,
            model: None,
            stop_reason: None,
            error_message: None,
            usage: None,
            message: None,
            output_truncated: None,
            tool_call_id: None,
            tool_name: None,
            is_error: None,
            args_preview: None,
            args_payload: None,
            max_bytes: None,
        }
    }

    /// pi `writeInitialUserMessage` (`:212-220`) with the sentinel as its text.
    #[must_use]
    pub fn initial_prompt(identity: &TranscriptIdentity) -> Self {
        let mut record = Self::base(identity, TranscriptRecordType::Message);
        record.source_event_type = Some(SourceEventType::InitialPrompt);
        record.role = Some(MessageRole::User);
        record.text = Some(INITIAL_PROMPT_SENTINEL.to_string());
        record.message = Some(serde_json::json!({
            "role": "user",
            "content": [{"type": "text", "text": INITIAL_PROMPT_SENTINEL}],
        }));
        record
    }

    /// pi `writeMessage` (`:174-208`) over a wire `message` object: the `toolResult` arm when the
    /// role says so, the unbounded assistant/user arm otherwise.
    #[must_use]
    pub fn message(
        identity: &TranscriptIdentity,
        source_event_type: SourceEventType,
        message: &Value,
    ) -> Self {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .map(MessageRole::from_wire);
        let text = content_text(message.get("content"));
        if role == Some(MessageRole::ToolResult) {
            return Self::tool_result(
                identity,
                source_event_type,
                string_field(message, "toolCallId"),
                string_field(message, "toolName"),
                message.get("isError").and_then(Value::as_bool),
                text.as_deref(),
                message.get("timestamp"),
            );
        }
        let mut record = Self::base(identity, TranscriptRecordType::Message);
        record.source_event_type = Some(source_event_type);
        record.role = role;
        record.text = text;
        record.model = string_field(message, "model");
        record.stop_reason = string_field(message, "stopReason");
        record.error_message = string_field(message, "errorMessage");
        record.usage = message.get("usage").and_then(TranscriptUsage::normalize);
        record.message = Some(message.clone());
        record
    }

    /// pi's `role === "toolResult"` arm (`:176-195`): bounded output text, the truncation flag,
    /// and a projected `message` whose `content` is the bounded text or empty.
    #[must_use]
    pub fn tool_result(
        identity: &TranscriptIdentity,
        source_event_type: SourceEventType,
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        is_error: Option<bool>,
        text: Option<&str>,
        timestamp: Option<&Value>,
    ) -> Self {
        let output = text.and_then(bounded_text);
        let mut projected = serde_json::Map::new();
        projected.insert("role".into(), Value::String("toolResult".into()));
        if let Some(id) = &tool_call_id {
            projected.insert("toolCallId".into(), Value::String(id.clone()));
        }
        if let Some(name) = &tool_name {
            projected.insert("toolName".into(), Value::String(name.clone()));
        }
        if let Some(is_error) = is_error {
            projected.insert("isError".into(), Value::Bool(is_error));
        }
        projected.insert(
            "content".into(),
            match &output {
                Some(text) => serde_json::json!([{"type": "text", "text": text}]),
                None => Value::Array(Vec::new()),
            },
        );
        if let Some(timestamp) = timestamp.filter(|value| !value.is_null()) {
            projected.insert("timestamp".into(), timestamp.clone());
        }
        let mut record = Self::base(identity, TranscriptRecordType::Message);
        record.source_event_type = Some(source_event_type);
        record.role = Some(MessageRole::ToolResult);
        record.tool_call_id = tool_call_id;
        record.tool_name = tool_name;
        record.is_error = is_error;
        record.output_truncated = output
            .as_deref()
            .map(|text| text.contains(TOOL_PAYLOAD_TRUNCATION_MARKER));
        record.text = output;
        record.message = Some(Value::Object(projected));
        record
    }

    /// pi's `tool_execution_start` arm (`:226-238`): `argsPreview` only for a non-empty args
    /// object, `argsPayload` as the bounded pretty JSON of the args (an empty object for
    /// non-object args, `eventArgs` `:96-100`).
    #[must_use]
    pub fn tool_start(
        identity: &TranscriptIdentity,
        tool_call_id: Option<&str>,
        tool_name: &str,
        args: &Value,
    ) -> Self {
        let args = match args {
            Value::Object(map) => Value::Object(map.clone()),
            _ => Value::Object(serde_json::Map::new()),
        };
        let mut record = Self::base(identity, TranscriptRecordType::ToolStart);
        record.source_event_type = Some(SourceEventType::ToolExecutionStart);
        record.tool_call_id = non_empty(tool_call_id);
        record.tool_name = Some(tool_name.to_string());
        record.args_preview = args
            .as_object()
            .filter(|map| !map.is_empty())
            .map(|_| extract_tool_args_preview(&args));
        record.args_payload = bounded_payload(&args);
        record
    }

    /// pi's `tool_execution_end` arm (`:239-247`). No output field: the output rides the
    /// `toolResult` message record.
    #[must_use]
    pub fn tool_end(
        identity: &TranscriptIdentity,
        tool_call_id: Option<&str>,
        tool_name: Option<&str>,
        is_error: Option<bool>,
    ) -> Self {
        let mut record = Self::base(identity, TranscriptRecordType::ToolEnd);
        record.source_event_type = Some(SourceEventType::ToolExecutionEnd);
        record.tool_call_id = non_empty(tool_call_id);
        record.tool_name = non_empty(tool_name);
        record.is_error = is_error;
        record
    }

    /// pi `writeTruncatedMarker`'s record (`:125-129`).
    #[must_use]
    pub fn truncated(identity: &TranscriptIdentity, max_bytes: u64) -> Self {
        let mut record = Self::base(identity, TranscriptRecordType::Truncated);
        record.max_bytes = Some(max_bytes);
        record.message = Some(Value::String(format!(
            "Child transcript exceeded {max_bytes} bytes; further records were omitted."
        )));
        record
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value.filter(|s| !s.is_empty()).map(str::to_string)
}

/// pi `boundedPayload` (`:9-28`) for a JSON value: a string as-is, anything else pretty-printed
/// (`JSON.stringify(v, null, 2)`); `None` for blank text or a value that will not serialize.
#[must_use]
pub fn bounded_payload(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => bounded_text(text),
        other => bounded_text(&serde_json::to_string_pretty(other).ok()?),
    }
}

/// pi `boundedPayload`'s string arm: blank -> `None`; at most [`MAX_TOOL_PAYLOAD_BYTES`] ->
/// verbatim; else the first `MAX - marker` bytes, backed off to a UTF-8 character boundary, plus
/// [`TOOL_PAYLOAD_TRUNCATION_MARKER`].
#[must_use]
pub fn bounded_text(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return None;
    }
    if text.len() <= MAX_TOOL_PAYLOAD_BYTES {
        return Some(text.to_string());
    }
    let mut end = MAX_TOOL_PAYLOAD_BYTES.saturating_sub(TOOL_PAYLOAD_TRUNCATION_MARKER.len());
    // Upstream's `(payload[end] & 0xc0) === 0x80` loop: step back off a continuation byte.
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = text.get(..end).unwrap_or_default();
    Some(format!("{head}{TOOL_PAYLOAD_TRUNCATION_MARKER}"))
}

/// pi's `writeError` (`:104`), typed. `Display` reproduces upstream's three sentences verbatim
/// (`:137`, `:163`, `:171`) so `transcriptError` reads identically.
#[derive(Debug, thiserror::Error)]
pub enum ChildTranscriptError {
    /// The parent directory or the file itself could not be created/truncated (`:167-172`).
    #[error("Failed to initialize child transcript '{}': {source}", path.display())]
    Initialize {
        /// The transcript path.
        path: PathBuf,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// A record or the truncation marker could not be appended (`:137`, `:163`).
    #[error("Failed to write child transcript '{}': {source}", path.display())]
    Write {
        /// The transcript path.
        path: PathBuf,
        /// The underlying I/O failure.
        #[source]
        source: std::io::Error,
    },
    /// A record could not be serialized (upstream's `JSON.stringify` cannot fail on these shapes;
    /// serde's can in principle, and the failure belongs on the same surface).
    #[error("Failed to serialize child transcript record for '{}': {source}", path.display())]
    Serialize {
        /// The transcript path.
        path: PathBuf,
        /// The serializer's error.
        #[source]
        source: serde_json::Error,
    },
}

/// pi `createChildTranscriptWriter` (`:102-264`). One per [`crate::exec::run_sync`], surviving
/// every model-fallback attempt (upstream's `shared.transcriptWriter`); single-owner on the drive
/// loop, never shared across tasks.
pub struct ChildTranscriptWriter {
    path: PathBuf,
    identity: TranscriptIdentity,
    /// `None` after a failed initialisation: upstream still returns a writer whose every write
    /// is a no-op while the path stays published and `getError()` names the cause (`:167-172`).
    sink: Option<BoundedJsonlWriter>,
    max_bytes: u64,
    truncated: bool,
    last_error: Option<ChildTranscriptError>,
}

impl ChildTranscriptWriter {
    /// Create the transcript at `path` with the default cap. Never fails: see
    /// [`Self::create_with_cap`].
    pub async fn create(path: &Path, identity: TranscriptIdentity) -> Self {
        Self::create_with_cap(path, identity, DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES).await
    }

    /// Create the transcript at `path` with an explicit byte cap: `mkdir -p` the parent, truncate
    /// the file (upstream `:168-169`), then open the capped appender over it. Any failure is
    /// latched as [`ChildTranscriptError::Initialize`] and the writer is returned anyway, with
    /// every later write a no-op.
    pub async fn create_with_cap(
        path: &Path,
        identity: TranscriptIdentity,
        max_bytes: u64,
    ) -> Self {
        let mut writer = Self {
            path: path.to_path_buf(),
            identity,
            sink: None,
            max_bytes,
            truncated: false,
            last_error: None,
        };
        match Self::open_sink(path, max_bytes).await {
            Ok(sink) => writer.sink = Some(sink),
            Err(source) => {
                writer.last_error = Some(ChildTranscriptError::Initialize {
                    path: path.to_path_buf(),
                    source,
                });
            }
        }
        writer
    }

    async fn open_sink(path: &Path, max_bytes: u64) -> std::io::Result<BoundedJsonlWriter> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, b"").await?;
        BoundedJsonlWriter::create_with_cap(path, max_bytes).await
    }

    /// The transcript's path (pi `writer.path`).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// pi `getError`: the latched initialisation/write failure, if any.
    #[must_use]
    pub fn last_error(&self) -> Option<&ChildTranscriptError> {
        self.last_error.as_ref()
    }

    /// Whether the file cap was reached and the `truncated` marker written (or attempted).
    #[must_use]
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// pi `writeInitialUserMessage(prompt)` (`:212-220`), always called with the sentinel.
    ///
    /// **[CYRUP-DELTA]** no prompt parameter. Both upstream creators pass the same constant
    /// (`subagent-runner.ts:889`, `execution.ts:1849`) and nothing else ever calls it, so the
    /// parameter only ever existed to be ignored; removing it makes "the transcript never carries
    /// the prompt" a property of the type rather than of every caller.
    pub async fn write_initial_prompt_sentinel(&mut self) {
        let record = ChildTranscriptRecord::initial_prompt(&self.identity);
        self.write_record(&record).await;
    }

    /// pi `writeChildEvent` (`:221-248`) over cyrup's one wire schema.
    ///
    /// **[CYRUP-DELTA]** a `tool_execution_end` yields TWO records, upstream's `tool_end` and a
    /// `toolResult` message carrying the bounded output. cyrup's wire carries the tool output
    /// inline in `tool_execution_end.result` (`exec/ndjson.rs`; `exec/output.rs` "that variant
    /// plays pi's `role === "toolResult"` role") and never emits pi's separate `toolResult`
    /// message, so without this record every tool row renders with a status glyph and no output,
    /// the regression `tui/fleet_transcript.rs`'s `rewrite_cyrup_record` already corrects for
    /// the raw event stream.
    pub async fn write_child_event(&mut self, event: &SubagentEvent) {
        match event {
            SubagentEvent::MessageEnd { message } => {
                let record = ChildTranscriptRecord::message(
                    &self.identity,
                    SourceEventType::MessageEnd,
                    message,
                );
                self.write_record(&record).await;
            }
            SubagentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
            } => {
                if tool_name.is_empty() {
                    return;
                }
                let record = ChildTranscriptRecord::tool_start(
                    &self.identity,
                    Some(tool_call_id.as_str()),
                    tool_name,
                    args,
                );
                self.write_record(&record).await;
            }
            SubagentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                let end = ChildTranscriptRecord::tool_end(
                    &self.identity,
                    Some(tool_call_id.as_str()),
                    Some(tool_name.as_str()),
                    Some(*is_error),
                );
                self.write_record(&end).await;
                let is_error = *is_error || result.get("isError") == Some(&Value::Bool(true));
                let text =
                    content_text(result.get("content")).or_else(|| content_text(Some(result)));
                let output = ChildTranscriptRecord::tool_result(
                    &self.identity,
                    SourceEventType::ToolExecutionEnd,
                    non_empty(Some(tool_call_id.as_str())),
                    non_empty(Some(tool_name.as_str())),
                    Some(is_error),
                    text.as_deref(),
                    None,
                );
                self.write_record(&output).await;
            }
            _ => {}
        }
    }

    /// pi `writeRecord` (`:142-165`): no-op once errored or truncated; otherwise the marker
    /// pre-check, then one appended line.
    async fn write_record(&mut self, record: &ChildTranscriptRecord) {
        if self.last_error.is_some() || self.truncated {
            return;
        }
        let line = match serde_json::to_string(record) {
            Ok(line) => line,
            Err(source) => {
                self.last_error = Some(ChildTranscriptError::Serialize {
                    path: self.path.clone(),
                    source,
                });
                return;
            }
        };
        // The marker probe is serialized per call (`:150-154`): its `ts` width can change.
        let marker = match serde_json::to_string(&ChildTranscriptRecord::truncated(
            &self.identity,
            self.max_bytes,
        )) {
            Ok(marker) => marker,
            Err(source) => {
                self.last_error = Some(ChildTranscriptError::Serialize {
                    path: self.path.clone(),
                    source,
                });
                return;
            }
        };
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        let line_bytes = line.len() as u64 + 1;
        let marker_bytes = marker.len() as u64 + 1;
        // Both upstream checks (`:146` and `:155`) in one: the record must fit AND leave room for
        // the marker after it.
        if sink
            .bytes_written()
            .saturating_add(line_bytes)
            .saturating_add(marker_bytes)
            > self.max_bytes
        {
            self.write_truncated_marker(&marker).await;
            return;
        }
        if let Err(source) = sink.write_line(&line).await {
            self.last_error = Some(ChildTranscriptError::Write {
                path: self.path.clone(),
                source,
            });
        }
    }

    /// pi `writeTruncatedMarker` (`:123-140`): latch `truncated`, then append the marker iff it
    /// fits under the cap.
    async fn write_truncated_marker(&mut self, marker: &str) {
        self.truncated = true;
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        let marker_bytes = marker.len() as u64 + 1;
        if sink.bytes_written().saturating_add(marker_bytes) > self.max_bytes {
            return;
        }
        if let Err(source) = sink.write_line(marker).await {
            self.last_error = Some(ChildTranscriptError::Write {
                path: self.path.clone(),
                source,
            });
        }
    }
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
    use crate::exec::ndjson::parse_line;
    use crate::tui::fleet_transcript::{
        FleetTranscriptEvent, FleetTranscriptReadOptions, ToolStatus, parse_transcript_lines,
        read_fleet_transcript,
    };

    fn identity(child_index: Option<usize>) -> TranscriptIdentity {
        TranscriptIdentity {
            source: TranscriptSource::Foreground,
            run_id: "run-1".to_string(),
            agent: "worker".to_string(),
            child_index,
            cwd: PathBuf::from("/w"),
        }
    }

    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("transcript readable")
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn records(path: &Path) -> Vec<Value> {
        lines(path)
            .iter()
            .map(|line| serde_json::from_str(line).expect("every line is JSON"))
            .collect()
    }

    fn keys(record: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = record
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    fn event(json: Value) -> SubagentEvent {
        parse_line(&json.to_string()).expect("the fixture line parses")
    }

    const BASE_KEYS: [&str; 8] = [
        "agent",
        "cwd",
        "recordType",
        "runId",
        "source",
        "timestamp",
        "ts",
        "version",
    ];

    fn expected_keys(extra: &[&'static str]) -> Vec<&'static str> {
        let mut all: Vec<&'static str> = BASE_KEYS.to_vec();
        all.extend_from_slice(extra);
        all.sort_unstable();
        all
    }

    // ---- bounded payloads (pi `boundedPayload`, `:9-28`) ----

    #[test]
    fn bounded_payload_cuts_at_32_kib_on_a_char_boundary_and_appends_the_marker() {
        // 40 KiB of a two-byte character: the cut lands mid-character unless backed off.
        let text: String = std::iter::repeat_n('é', 20 * 1024).collect();
        assert_eq!(text.len(), 40 * 1024);
        let bounded = bounded_text(&text).expect("non-blank");
        assert!(
            bounded.len() <= MAX_TOOL_PAYLOAD_BYTES,
            "{} > {MAX_TOOL_PAYLOAD_BYTES}",
            bounded.len()
        );
        assert!(bounded.ends_with(TOOL_PAYLOAD_TRUNCATION_MARKER));
        let head = &bounded[..bounded.len() - TOOL_PAYLOAD_TRUNCATION_MARKER.len()];
        assert!(head.chars().all(|c| c == 'é'), "no split code point");
        // `MAX - 23` is odd, so the backoff must have stepped one byte back.
        assert_eq!(head.len(), MAX_TOOL_PAYLOAD_BYTES - 23 - 1);
        assert_eq!(TOOL_PAYLOAD_TRUNCATION_MARKER.len(), 23);
        // The JSON arm goes through the same cut.
        let payload = bounded_payload(&Value::String(text)).expect("non-blank");
        assert_eq!(payload, bounded);
    }

    #[test]
    fn bounded_payload_keeps_small_inputs_and_omits_blank_ones() {
        let exact = "x".repeat(MAX_TOOL_PAYLOAD_BYTES);
        assert_eq!(bounded_text(&exact).as_deref(), Some(exact.as_str()));
        assert_eq!(bounded_text("  \n"), None);
        assert_eq!(bounded_payload(&Value::String("   ".into())), None);
        // A non-string is `JSON.stringify(v, null, 2)`: two-space pretty JSON.
        assert_eq!(
            bounded_payload(&serde_json::json!({"command": "ls"})).as_deref(),
            Some("{\n  \"command\": \"ls\"\n}")
        );
    }

    // ---- the cut at its two APPLICATION sites, not only in the helper (`:177`, `:228`) ----

    /// A 40 KiB tool result and 40 KiB tool args reach the file cut to [`MAX_TOOL_PAYLOAD_BYTES`]
    /// and marked, while a 40 KiB assistant message stays whole (`:197-207`). The helper being
    /// correct proves nothing if `tool_result`/`tool_start` stop calling it, and every other
    /// projection test here uses payloads far under the cut.
    #[tokio::test]
    async fn tool_payloads_are_cut_where_the_writer_applies_them_not_only_in_the_helper() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("t.jsonl");
        let mut writer = ChildTranscriptWriter::create(&path, identity(Some(0))).await;
        let big = "x".repeat(40 * 1024);
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "message_end",
                "message": {"role": "assistant", "content": [{"type": "text", "text": big}]}
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "tool_execution_start", "toolCallId": "call-1", "toolName": "bash",
                "args": {"command": big}
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "tool_execution_end", "toolCallId": "call-1", "toolName": "bash",
                "isError": false, "result": {"content": [{"type": "text", "text": big}]}
            })))
            .await;
        assert!(writer.last_error().is_none(), "{:?}", writer.last_error());

        let records = records(&path);
        assert_eq!(records.len(), 4, "{records:#?}");

        let assistant = &records[0];
        assert_eq!(
            assistant["text"].as_str().unwrap().len(),
            40 * 1024,
            "assistant text is NOT a tool payload and stays unbounded"
        );
        assert_eq!(assistant["outputTruncated"], Value::Null);

        let start = &records[1];
        let payload = start["argsPayload"].as_str().unwrap();
        assert!(
            payload.len() <= MAX_TOOL_PAYLOAD_BYTES,
            "argsPayload {} > {MAX_TOOL_PAYLOAD_BYTES}: the cut is not applied at `tool_start`",
            payload.len()
        );
        assert!(payload.ends_with(TOOL_PAYLOAD_TRUNCATION_MARKER));
        assert!(
            payload.starts_with("{\n  \"command\": \"xxx"),
            "{payload:.40}"
        );
        assert!(
            start["argsPreview"].as_str().unwrap().len() < 40 * 1024,
            "the preview is the reader's short form, never the raw args"
        );

        let result = &records[3];
        let text = result["text"].as_str().unwrap();
        assert_eq!(
            text.len(),
            MAX_TOOL_PAYLOAD_BYTES,
            "toolResult text must be cut at the application site (`tool_result`)"
        );
        assert!(text.ends_with(TOOL_PAYLOAD_TRUNCATION_MARKER));
        assert_eq!(result["outputTruncated"], true);
        assert_eq!(
            result["message"]["content"][0]["text"].as_str().unwrap(),
            text,
            "the projected message carries the SAME bounded text"
        );
    }

    // ---- the default cap reaches production through `create` (`:31`, `:104`) ----

    /// `create` is the constructor `run_sync` calls; the cap test below drives `create_with_cap`
    /// directly, so nothing else pins the delegation or the figure. pi's
    /// `DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES` is 50 MiB (`:31`), and both budgets a default writer
    /// enforces against — its own marker pre-check and the substrate's backstop — must be it.
    #[tokio::test]
    async fn create_caps_the_sink_at_the_50_mib_default() {
        assert_eq!(DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES, 50 * 1024 * 1024);
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("t.jsonl");
        let writer = ChildTranscriptWriter::create(&path, identity(None)).await;
        assert!(writer.last_error().is_none(), "{:?}", writer.last_error());
        assert_eq!(
            writer.max_bytes, DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES,
            "the marker pre-check budget"
        );
        assert_eq!(
            writer.sink.as_ref().expect("sink opened").cap_bytes(),
            DEFAULT_MAX_CHILD_TRANSCRIPT_BYTES,
            "the substrate's backstop"
        );
    }

    // ---- the sentinel (`:212-220`, `:889`, `:1849`) ----

    #[tokio::test]
    async fn the_first_record_is_the_redacted_sentinel_and_never_a_prompt() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("run-1_worker_0_transcript.jsonl");
        let mut writer = ChildTranscriptWriter::create(&path, identity(Some(0))).await;
        writer.write_initial_prompt_sentinel().await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "message_end",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "hello"}]}
            })))
            .await;

        let records = records(&path);
        assert_eq!(records.len(), 2);
        let first = &records[0];
        assert_eq!(first["version"], 1);
        assert_eq!(first["recordType"], "message");
        assert_eq!(first["role"], "user");
        assert_eq!(first["sourceEventType"], "initial_prompt");
        assert_eq!(first["text"], INITIAL_PROMPT_SENTINEL);
        assert_eq!(first["message"]["role"], "user");
        assert_eq!(
            first["message"]["content"][0]["text"],
            INITIAL_PROMPT_SENTINEL
        );
        assert_eq!(first["childIndex"], 0);
        assert!(INITIAL_PROMPT_SENTINEL.starts_with("[prompt redacted]"));
        assert!(writer.last_error().is_none());
        assert_eq!(writer.path(), path.as_path());
    }

    // ---- the event -> record projection (`:221-248`, `:174-208`) ----

    #[tokio::test]
    async fn child_events_project_to_pis_record_vocabulary() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("t.jsonl");
        let mut writer = ChildTranscriptWriter::create(&path, identity(None)).await;

        writer
            .write_child_event(&event(serde_json::json!({
                "type": "message_end",
                "message": {
                    "role": "assistant",
                    "content": [{"type": "text", "text": "Working."}, {"type": "thinking", "thinking": "hidden"}],
                    "model": "opus",
                    "stopReason": "toolUse",
                    "usage": {"inputTokens": 12, "output": 3, "cacheRead": 1, "cacheWrite": 0,
                              "totalTokens": 16, "cost": {"total": 0.25}}
                }
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "tool_execution_start", "toolCallId": "call-1", "toolName": "bash",
                "args": {"command": "cargo test"}
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "tool_execution_end", "toolCallId": "call-1", "toolName": "bash",
                "isError": true, "result": {"content": [{"type": "text", "text": "boom"}]}
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({"type": "agent_start"})))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({"type": "session_start"})))
            .await;

        let records = records(&path);
        assert_eq!(records.len(), 4, "{records:#?}");

        let assistant = &records[0];
        assert_eq!(
            keys(assistant),
            expected_keys(&[
                "sourceEventType",
                "role",
                "text",
                "model",
                "stopReason",
                "usage",
                "message"
            ])
        );
        assert_eq!(assistant["recordType"], "message");
        assert_eq!(assistant["sourceEventType"], "message_end");
        assert_eq!(assistant["role"], "assistant");
        assert_eq!(assistant["text"], "Working.");
        assert_eq!(assistant["model"], "opus");
        assert_eq!(assistant["stopReason"], "toolUse");
        assert_eq!(
            assistant["usage"],
            serde_json::json!({"input": 12, "output": 3, "cacheRead": 1, "cacheWrite": 0, "cost": 0.25})
        );
        assert_eq!(
            assistant["message"]["model"], "opus",
            "the full message rides along"
        );
        assert!(assistant["ts"].is_i64());
        assert!(assistant["timestamp"].as_str().unwrap().ends_with('Z'));
        assert_eq!(assistant["source"], "foreground");
        assert_eq!(assistant["runId"], "run-1");
        assert_eq!(assistant["agent"], "worker");
        assert_eq!(assistant["cwd"], "/w");

        let start = &records[1];
        assert_eq!(
            keys(start),
            expected_keys(&[
                "sourceEventType",
                "toolCallId",
                "toolName",
                "argsPreview",
                "argsPayload"
            ])
        );
        assert_eq!(start["recordType"], "tool_start");
        assert_eq!(start["toolCallId"], "call-1");
        assert_eq!(start["toolName"], "bash");
        assert_eq!(start["argsPreview"], "cargo test");
        assert_eq!(start["argsPayload"], "{\n  \"command\": \"cargo test\"\n}");

        let end = &records[2];
        assert_eq!(
            keys(end),
            expected_keys(&["sourceEventType", "toolCallId", "toolName", "isError"])
        );
        assert_eq!(end["recordType"], "tool_end");
        assert_eq!(end["isError"], true);

        let result = &records[3];
        assert_eq!(
            keys(result),
            expected_keys(&[
                "sourceEventType",
                "role",
                "text",
                "outputTruncated",
                "toolCallId",
                "toolName",
                "isError",
                "message"
            ])
        );
        assert_eq!(result["recordType"], "message");
        assert_eq!(result["role"], "toolResult");
        assert_eq!(result["sourceEventType"], "tool_execution_end");
        assert_eq!(result["text"], "boom");
        assert_eq!(result["outputTruncated"], false);
        assert_eq!(result["message"]["content"][0]["text"], "boom");
        assert_eq!(result["message"]["isError"], true);
    }

    // ---- the writer/reader contract in one process ----

    #[tokio::test]
    async fn the_production_reader_folds_what_the_writer_wrote() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("run-1_worker_0_transcript.jsonl");
        let mut writer = ChildTranscriptWriter::create(&path, identity(Some(0))).await;
        writer.write_initial_prompt_sentinel().await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "message_end",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "Running the tests."}], "model": "opus"}
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "tool_execution_start", "toolCallId": "call-1", "toolName": "bash",
                "args": {"command": "cargo test"}
            })))
            .await;
        writer
            .write_child_event(&event(serde_json::json!({
                "type": "tool_execution_end", "toolCallId": "call-1", "toolName": "bash",
                "isError": false, "result": {"content": [{"type": "text", "text": "done"}]}
            })))
            .await;

        let transcript = read_fleet_transcript(
            &path,
            &FleetTranscriptReadOptions {
                trusted_roots: vec![dir.path().to_path_buf()],
                ..Default::default()
            },
        );
        assert!(transcript.warning.is_none(), "{:?}", transcript.warning);
        let assistant = transcript.events.iter().find_map(|e| match e {
            FleetTranscriptEvent::Assistant { text, model, .. } => {
                Some((text.clone(), model.clone()))
            }
            _ => None,
        });
        assert_eq!(
            assistant,
            Some(("Running the tests.".to_string(), Some("opus".to_string())))
        );
        let tools: Vec<_> = transcript
            .events
            .iter()
            .filter_map(|e| match e {
                FleetTranscriptEvent::Tool(tool) => Some(tool),
                _ => None,
            })
            .collect();
        assert_eq!(
            tools.len(),
            1,
            "one tool row, not one per record: {tools:#?}"
        );
        assert_eq!(tools[0].name, "bash");
        assert_eq!(tools[0].args.as_deref(), Some("cargo test"));
        assert_eq!(tools[0].status, ToolStatus::Complete);
        assert_eq!(tools[0].output.as_deref(), Some("done"));
    }

    // ---- the file cap (`:142-158`) ----

    #[tokio::test]
    async fn records_past_the_cap_end_in_one_truncated_marker() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("t.jsonl");
        let cap = 1024u64;
        let mut writer = ChildTranscriptWriter::create_with_cap(&path, identity(None), cap).await;
        writer.write_initial_prompt_sentinel().await;
        assert!(!writer.is_truncated(), "the sentinel fits under 1 KiB");
        for i in 0..50 {
            writer
                .write_child_event(&event(serde_json::json!({
                    "type": "tool_execution_end", "toolCallId": format!("call-{i}"),
                    "toolName": "bash", "isError": false, "result": {}
                })))
                .await;
        }
        assert!(writer.is_truncated());
        assert!(writer.last_error().is_none());

        let lines = lines(&path);
        assert!(
            lines.len() >= 2,
            "sentinel + at least the marker: {lines:#?}"
        );
        let last: Value = serde_json::from_str(lines.last().unwrap()).unwrap();
        assert_eq!(last["recordType"], "truncated");
        assert_eq!(last["maxBytes"], cap);
        assert_eq!(
            last["message"],
            "Child transcript exceeded 1024 bytes; further records were omitted."
        );
        assert_eq!(keys(&last), expected_keys(&["maxBytes", "message"]));
        let on_disk = std::fs::metadata(&path).unwrap().len();
        assert!(on_disk <= cap, "{on_disk} > {cap}");
        assert!(
            parse_transcript_lines(&lines, false).explicit_truncation,
            "the reader must see the marker"
        );
    }

    // ---- failure surface (`:167-172`) ----

    #[tokio::test]
    async fn an_unwritable_path_surfaces_a_typed_initialize_error() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let blocker = dir.path().join("not-a-dir");
        std::fs::write(&blocker, b"x").unwrap();
        let path = blocker.join("t.jsonl");
        let mut writer = ChildTranscriptWriter::create(&path, identity(None)).await;
        assert!(
            matches!(
                writer.last_error(),
                Some(ChildTranscriptError::Initialize { .. })
            ),
            "{:?}",
            writer.last_error()
        );
        let rendered = writer.last_error().unwrap().to_string();
        assert!(
            rendered.starts_with("Failed to initialize child transcript '"),
            "{rendered}"
        );
        assert!(rendered.contains("t.jsonl"), "{rendered}");
        // Later writes are silent no-ops that keep the ORIGINAL error.
        writer.write_initial_prompt_sentinel().await;
        writer
            .write_child_event(&event(serde_json::json!({"type": "agent_end"})))
            .await;
        assert!(matches!(
            writer.last_error(),
            Some(ChildTranscriptError::Initialize { .. })
        ));
        assert!(!path.exists());
        assert_eq!(writer.path(), path.as_path(), "the path stays published");
    }

    #[tokio::test]
    async fn create_truncates_a_stale_transcript() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let path = dir.path().join("t.jsonl");
        std::fs::write(&path, b"{\"stale\":true}\nnot json\n").unwrap();
        let mut writer = ChildTranscriptWriter::create(&path, identity(None)).await;
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            0,
            "truncated on create"
        );
        writer.write_initial_prompt_sentinel().await;
        let records = records(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["sourceEventType"], "initial_prompt");
    }

    // ---- the role carrier ----

    #[test]
    fn message_roles_serialize_to_the_wire_strings_and_carry_unknown_roles_verbatim() {
        for (role, wire) in [
            (MessageRole::User, "\"user\""),
            (MessageRole::Assistant, "\"assistant\""),
            (MessageRole::ToolResult, "\"toolResult\""),
            (
                MessageRole::Other("bashExecution".into()),
                "\"bashExecution\"",
            ),
        ] {
            assert_eq!(serde_json::to_string(&role).unwrap(), wire);
            assert_eq!(serde_json::from_str::<MessageRole>(wire).unwrap(), role);
        }
        assert_eq!(
            MessageRole::from_wire("tool_result"),
            MessageRole::ToolResult
        );
    }
}
