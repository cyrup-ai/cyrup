//! The NDJSON event-stream parser for a spawned child's stdout (func-SA R-SA-026/057/058;
//! arch-SA §6.3.1).
//!
//! # Scope and relationship to `crate::spawn`
//!
//! [`crate::spawn`] owns the literal spawn boundary, but it defines NO event schema: it reads,
//! tees each raw line to the `.jsonl` artifact, and hands the line's text back unparsed. This
//! module is the crate's ONE NDJSON parser — [`SubagentEvent`] is the only child-event shape, and
//! [`parse_line`] the only place a child stdout line is deserialized. The foreground executor
//! (`exec/mod.rs`'s read loop, `exec/output.rs`, `exec/acceptance.rs`) folds progress state, usage
//! accounting, and final-output extraction from it. A newly added child event is therefore taught
//! here, once, with no second schema to guess at or drift from.
//!
//! # Wire schema
//!
//! The spawned child is `cyrup` itself, re-exec'd and run in one-shot JSON streaming mode
//! (`cyrup-modes::json::run_json`, `crates/cyrup-modes/src/json.rs`), which serializes
//! `cyrup_session_svc::event::AgentSessionEvent` — itself a superset-forward of
//! `cyrup_agent::event::AgentEvent` — as one `serde_json` object per line to stdout. Both enums
//! share one wire shape: `#[serde(tag = "type", rename_all = "snake_case", rename_all_fields =
//! "camelCase")]`, i.e. the discriminant lives in a `type` key with snake_case values
//! (`"tool_execution_start"`, `"message_end"`, …) and every OTHER field in a variant's payload is
//! camelCase (`toolCallId`, `toolName`, `isError`, …). [`SubagentEvent`] mirrors that shape
//! exactly so it can deserialize a real child's stdout byte-for-byte — there is no separate "pi
//! event schema" to reimplement (func-SA §4.4's `NdjsonEvent` data-model entry, restated
//! verbatim). Getting `rename_all_fields = "camelCase"` wrong here is silent, not loud: a known
//! `type` tag whose required payload field is missing fails the WHOLE line's deserialize, and
//! [`SubagentEvent::Unknown`]'s `#[serde(other)]` rescues only an unknown *tag*, so the event
//! simply never arrives.
//!
//! This crate has ZERO dependency on `cyrup-agent` or `cyrup-session-svc` (arch-SA §2.1/§1.1) —
//! the rich `AgentMessage`/`AssistantMessage`/`Content` payload types those crates own are
//! therefore never imported here. Payload fields that would otherwise be one of those rich types
//! (`message`, `args`, `result`, `partial_result`, `tool_results`, `messages`) are captured as
//! opaque `serde_json::Value`.
//! `usage`, when it appears, is read out of the embedded `message` value by
//! [`SubagentEvent::assistant_usage`] using only `serde_json::Value` field lookups (never a typed
//! `AssistantMessage` deserialize) — the assistant turn's `usage` field is nested inside
//! `message.usage` on the wire (`cyrup_core::AssistantMessage.usage: cyrup_core::Usage`,
//! `camelCase` struct fields), not a top-level field of the `MessageEnd` event itself.
//!
//! # Tolerance contract (R-SA-026)
//!
//! Every single line read from the child's stdout MUST be attempted as one JSON parse; a parse
//! failure on any one line MUST be tolerated (the line is skipped, never propagated as an error
//! and never aborts the run) and an unrecognized `type` tag MUST degrade to
//! [`SubagentEvent::Unknown`] rather than a parse error at all — these are two distinct tolerance
//! mechanisms (`serde(other)` handles the second; [`parse_line`] returning `None` handles the
//! first) and both are required. On stream end (the child closes stdout, normally because it
//! exited), any unterminated trailing buffered content — a final line with no trailing `\n` —
//! MUST be flushed through this exact same per-line parse path exactly once, never dropped and
//! never double-parsed. [`consume_stdout`] achieves this by building on `tokio::io::Lines`, whose
//! own `poll_next_line` (`tokio-io-util`) already surfaces a final unterminated line as one more
//! `Some(String)` at EOF before yielding `None` — see this module's own tests for a from-scratch
//! proof of that behavior against a scripted, partial-read-boundary reader, since R-SA-026's text
//! is explicit that this must actually be verified, not merely assumed from the dependency's
//! documented behavior.
//!
//! # Raw-stdout live tee (R-SA-058)
//!
//! [`consume_stdout`] tees every raw line — parseable or not — to the caller-supplied `tee`
//! sink BEFORE attempting to parse it, and does so as each line is read rather than buffering
//! output for a single flush at the end (R-SA-058's "as they are read, not buffered and written
//! at exit"). The actual `.jsonl` artifact file this feeds in production is owned by
//! `spawn::SpawnedChild` (see that module's `jsonl_writer` field) — this module stays
//! storage-agnostic and accepts any `FnMut(&str)` sink so it is trivially testable without real
//! file I/O.

use std::collections::HashMap;

use cyrup_core::{ToolCallId, Usage};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

/// One parsed line of a spawned child's NDJSON stdout stream (R-SA-026/057), matching cyrup's own
/// native JSON-streaming-mode wire-event shape verbatim (module-level docs above).
///
/// Every variant name and its wire `type` tag match `AgentSessionEvent`/`AgentEvent`'s `kind()`
/// discriminants exactly (`crates/cyrup-session-svc/src/event.rs`,
/// `crates/cyrup-agent/src/event.rs`) — this is not a redesigned or narrowed event vocabulary,
/// only a dependency-free re-typing of the identical wire bytes. [`SubagentEvent::Unknown`] is
/// the func-SA §4.3 "catch-all `Unknown` variant for forward compatibility" data-model entry:
/// any `type` tag this enum does not (yet) know about — including genuinely new event kinds added
/// to `cyrup-session-svc` after this crate was last updated — degrades here rather than failing
/// the whole line's parse.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SubagentEvent {
    /// A fresh agent run began (`AgentEvent::AgentStart`).
    AgentStart,
    /// A new turn (one assistant-message-then-tool-calls cycle) began.
    TurnStart,
    /// An assistant message began streaming. `message` is the in-progress
    /// `cyrup_agent::AgentMessage`, left opaque (module docs above).
    MessageStart { message: serde_json::Value },
    /// An assistant message received an incremental streaming update.
    ///
    /// Carries ONLY the delta. The producer is `cyrup-modes`' json mode, which since the v0.84.1
    /// wire projection (Pi `toJsonEvent`, `coding-agent/src/modes/json-event.ts:28-40`, applied at
    /// `print-mode.ts:110`) emits `message_update` as a two-key object — the cumulative outer
    /// `message` snapshot and the inner `assistantMessageEvent.partial` are both gone, per
    /// `coding-agent/docs/rpc.md:952-956`. This mirrors Pi retyping its own RPC client to the
    /// projected event in the same change (`rpc-client.ts:50`).
    ///
    /// Declaring `message` here would be an outright protocol break, not a tolerated extra: it has
    /// no `#[serde(default)]`, so a missing key fails the whole line's deserialization and
    /// [`parse_line`]'s `.ok()` (`:316`) drops the event silently — `#[serde(other)]` catches an
    /// unknown `type` TAG, never a known tag with a missing field. Omitting it instead is tolerant
    /// in both directions: serde ignores unknown fields, so an OLDER cyrup child still emitting
    /// `message` parses fine here.
    MessageUpdate {
        assistant_message_event: serde_json::Value,
    },
    /// An assistant message completed (R-SA-027: usage accumulation and R-SA-029: final-output
    /// extraction both key off this variant — extraction itself is `exec/output.rs`'s concern, a
    /// later phase; this module only exposes [`SubagentEvent::assistant_usage`] as the shared
    /// accessor so that later module does not need to re-derive the same `message.usage` JSON
    /// path). `usage` is intentionally NOT a field of this variant on the wire — see
    /// [`SubagentEvent::assistant_usage`]'s doc comment for exactly where it actually lives.
    MessageEnd { message: serde_json::Value },
    /// A tool call started executing (R-SA-027: increments `tool_count`, sets `current_tool` in
    /// the caller's `AgentProgress` — that fold itself lives in a later phase's `exec/mod.rs`
    /// `run_sync`, not here).
    ToolExecutionStart {
        tool_call_id: ToolCallId,
        tool_name: String,
        #[serde(default)]
        args: serde_json::Value,
    },
    /// A tool call reported incremental progress before finishing.
    ToolExecutionUpdate {
        tool_call_id: ToolCallId,
        tool_name: String,
        #[serde(default)]
        args: serde_json::Value,
        #[serde(default)]
        partial_result: serde_json::Value,
    },
    /// A tool call finished executing. Note this is the wire's actual terminal tool-call event —
    /// func-SA §4.3's illustrative `ToolResultEnd{call_id,is_error}` data-model entry names a
    /// shape that does not exist verbatim on cyrup's real wire (confirmed against
    /// `cyrup-session-svc::event::AgentSessionEvent`/`cyrup-agent::event::AgentEvent`, the actual
    /// source of truth per this file's task brief); `ToolExecutionEnd`'s own `is_error` field
    /// carries the identical information this crate needs and is what a real child process
    /// actually emits, so it is what this enum parses.
    ToolExecutionEnd {
        tool_call_id: ToolCallId,
        tool_name: String,
        #[serde(default)]
        result: serde_json::Value,
        #[serde(default)]
        is_error: bool,
    },
    /// A turn (assistant message + its tool calls/results) completed.
    ///
    /// `message` carries `#[serde(default)]` because `turn_end` is one of the two AGGREGATE records
    /// the bounded reader may deliver in reduced form: when a child's `turn_end` line exceeds
    /// [`crate::exec::child_protocol::MAX_CHILD_PENDING_LINE_BYTES`], the reader replaces it with
    /// the projected `{"type":"turn_end"}` (`child-protocol.ts:219`). Without the default that
    /// projected record fails to deserialize and [`parse_line`]'s `.ok()` drops it silently, so the
    /// recovery path upstream added would deliver nothing.
    TurnEnd {
        #[serde(default)]
        message: serde_json::Value,
        #[serde(default)]
        tool_results: Vec<serde_json::Value>,
    },
    /// The whole run completed — the last event a well-behaved child emits before closing stdout
    /// and exiting (though [`consume_stdout`] itself does not assume this; it simply reads until
    /// EOF regardless of which event, if any, arrives last).
    AgentEnd {
        #[serde(default)]
        messages: Vec<serde_json::Value>,
        #[serde(default)]
        will_retry: bool,
    },
    /// The WHOLE run settled — the last event a cyrup child emits before its run-scoped stream
    /// closes (`cyrup-session-svc/src/subscriber.rs:214-228`; Pi `_emitAgentSettled`,
    /// `agent-session.ts:599-600`). Distinct from [`SubagentEvent::AgentEnd`], which fires once per
    /// agent loop and may be followed by an auto-retry.
    ///
    /// This variant is not decorative: pi's `projectChildLifecycle`
    /// (`runs/shared/child-protocol.ts:398`) treats `agent_settled` as a drain START, so without it
    /// the event degraded to [`SubagentEvent::Unknown`] here and a child that settled without a
    /// terminal assistant stop (an error/aborted final message, a tool-call-terminated turn) never
    /// armed the parent's final-stop grace window at all.
    AgentSettled,
    /// The session-level steering/follow-up queue changed.
    QueueUpdate {
        #[serde(default)]
        steering: Vec<String>,
        #[serde(default)]
        follow_up: Vec<String>,
    },
    /// A context-window compaction pass began.
    CompactionStart {
        #[serde(default)]
        reason: serde_json::Value,
    },
    /// A context-window compaction pass settled.
    CompactionEnd {
        #[serde(default)]
        reason: serde_json::Value,
        #[serde(default)]
        aborted: bool,
        #[serde(default)]
        will_retry: bool,
    },
    /// A post-`agent_end` auto-retry backoff began.
    AutoRetryStart {
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        max_attempts: u32,
        #[serde(default)]
        delay_ms: u64,
        #[serde(default)]
        error_message: String,
    },
    /// A post-`agent_end` auto-retry sequence ended.
    AutoRetryEnd {
        #[serde(default)]
        success: bool,
        #[serde(default)]
        attempt: u32,
    },
    /// The active model changed mid-session (rare inside one subagent run, but a legitimate wire
    /// event this crate must not choke on).
    ModelChanged {
        #[serde(default)]
        provider: String,
        #[serde(default)]
        model: String,
    },
    /// Any event shape this enum does not specifically recognize — an unrecognized `type` tag
    /// (including a session-lifecycle event a child should never legitimately emit mid-run, such
    /// as `session_start`/`session_shutdown`/`session_replaced`/`session_info_changed`/
    /// `entry_appended`/`thinking_level_changed`, none of which the foreground executor has any
    /// use for) degrades here rather than failing the parse (R-SA-026's tolerance, restated at the
    /// tag level; [`parse_line`]'s `Ok(None)` return covers the OTHER tolerance case — a line that
    /// is not valid JSON at all).
    #[serde(other)]
    Unknown,
}

impl SubagentEvent {
    /// A short discriminant string, matching `AgentSessionEvent::kind()`'s naming exactly —
    /// useful for logging/tests without re-deriving a `match`.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            SubagentEvent::AgentStart => "agent_start",
            SubagentEvent::TurnStart => "turn_start",
            SubagentEvent::MessageStart { .. } => "message_start",
            SubagentEvent::MessageUpdate { .. } => "message_update",
            SubagentEvent::MessageEnd { .. } => "message_end",
            SubagentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            SubagentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            SubagentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
            SubagentEvent::TurnEnd { .. } => "turn_end",
            SubagentEvent::AgentEnd { .. } => "agent_end",
            SubagentEvent::AgentSettled => "agent_settled",
            SubagentEvent::QueueUpdate { .. } => "queue_update",
            SubagentEvent::CompactionStart { .. } => "compaction_start",
            SubagentEvent::CompactionEnd { .. } => "compaction_end",
            SubagentEvent::AutoRetryStart { .. } => "auto_retry_start",
            SubagentEvent::AutoRetryEnd { .. } => "auto_retry_end",
            SubagentEvent::ModelChanged { .. } => "model_changed",
            SubagentEvent::Unknown => "unknown",
        }
    }

    /// Extract the per-turn [`Usage`] out of a [`SubagentEvent::MessageEnd`]'s embedded assistant
    /// message, for R-SA-027's "every `MessageEnd` event's `usage` MUST be accumulated into the
    /// attempt's running `Usage` total" — the actual accumulation loop (`total.add(&usage)`,
    /// mirroring `exec::Usage::add`'s additive-never-replace contract) lives in a later phase's
    /// `exec/fallback.rs`, not here; this is only the shared, correctly-scoped accessor so that
    /// module does not need to re-derive the same JSON path.
    ///
    /// Returns `None` for any variant other than `MessageEnd`, for a `MessageEnd` whose `message`
    /// is not an assistant turn (a `MessageEnd` can legitimately wrap a `user`/`toolResult`
    /// message shape too, per `AgentMessage`'s `role` tag — those never carry `usage`), or if the
    /// embedded `usage` object fails to deserialize as [`Usage`] (tolerated per this whole
    /// module's tolerance contract — a malformed nested `usage` payload must not panic or bubble
    /// as an error, it simply contributes nothing to the running total).
    #[must_use]
    pub fn assistant_usage(&self) -> Option<Usage> {
        let SubagentEvent::MessageEnd { message } = self else {
            return None;
        };
        if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            return None;
        }
        let usage = message.get("usage")?;
        serde_json::from_value::<Usage>(usage.clone()).ok()
    }

    /// Whether this `MessageEnd`'s embedded assistant message is flagged error/aborted
    /// (`stopReason` of `"error"`/`"aborted"`, or a present `errorMessage`) — the shared
    /// "skip if flagged error/aborted" predicate R-SA-029's final-output extraction (a later
    /// phase's `exec/output.rs`) and R-SA-027's usage accounting both need. Kept here rather than
    /// duplicated in that later module for the same reason as [`SubagentEvent::assistant_usage`]:
    /// one JSON-path accessor, reused everywhere the flag is needed. Returns `false` (never
    /// "flagged") for any non-`MessageEnd` variant or a `MessageEnd` whose message is not an
    /// assistant turn at all, since only an assistant turn can be flagged in the first place.
    #[must_use]
    pub fn is_error_or_aborted_message(&self) -> bool {
        let SubagentEvent::MessageEnd { message } = self else {
            return false;
        };
        if message.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
            return false;
        }
        let stop_reason = message
            .get("stopReason")
            .and_then(serde_json::Value::as_str);
        matches!(stop_reason, Some("error") | Some("aborted"))
            || message.get("errorMessage").is_some_and(|v| !v.is_null())
    }
}

/// One line of a spawned child's raw NDJSON stdout, alongside whatever [`SubagentEvent`] it
/// parsed to (or did not, R-SA-026).
#[derive(Debug, Clone)]
pub struct NdjsonLine {
    /// The raw line text, exactly as read from the child's stdout, before any parse attempt —
    /// already teed to the caller's sink by the time [`consume_stdout`] hands this out.
    pub raw: String,
    /// The parsed event, or `None` if `raw` failed to parse as JSON at all (R-SA-026's per-line
    /// tolerance). A `raw` that DOES parse as JSON but carries an unrecognized `type` tag is
    /// still `Some(SubagentEvent::Unknown)`, not `None` — see [`parse_line`]'s doc comment for the
    /// distinction between these two independent tolerance mechanisms.
    pub parsed: Option<SubagentEvent>,
}

/// Attempt to parse one already-read line as one [`SubagentEvent`] (R-SA-026).
///
/// Returns `Some(event)` for both a fully recognized event AND a syntactically valid JSON object
/// whose `type` tag is unrecognized (which deserializes to [`SubagentEvent::Unknown`] via
/// `#[serde(other)]`) — `#[serde(other)]` only degrades an unrecognized *tag value*; it does not
/// make outright invalid JSON, or JSON that is valid but not a `type`-tagged object at all (e.g. a
/// bare JSON array or number), parse successfully. Both of THOSE failure shapes fall through to
/// this function's `None` return, which is R-SA-026's other, coarser tolerance mechanism: "a
/// parse failure on any single line MUST be tolerated (line skipped) and MUST NOT abort the run."
#[must_use]
pub fn parse_line(line: &str) -> Option<SubagentEvent> {
    serde_json::from_str::<SubagentEvent>(line).ok()
}

/// Read `reader`'s contents as NDJSON, line by line, driving `on_event` for every successfully
/// parsed line and `tee` for every raw line (parseable or not) — the streaming consumer at the
/// heart of R-SA-026/057/058.
///
/// Generic over `R: AsyncBufRead` rather than hardcoded to `tokio::process::ChildStdout` so this
/// exact function is exercised, unmodified, against both a real spawned child's piped stdout in
/// production and an in-memory scripted reader (partial reads, no trailing newline, interleaved
/// malformed lines) in this module's own tests — the production call site (a later phase's
/// `exec/fallback.rs`'s per-attempt driver, spawning through `crate::spawn::SpawnedChild`) wraps a
/// real `tokio::process::ChildStdout` in a `tokio::io::BufReader` before calling this.
///
/// # Ordering and tolerance contract
///
/// For every line, in order:
/// 1. The raw line text is passed to `tee` FIRST, unconditionally — including a line that will go
///    on to fail parsing (R-SA-058 makes no exception for unparseable lines: the artifact must
///    reflect exactly what the child wrote, byte for byte, whether or not this crate could later
///    make sense of it).
/// 2. [`parse_line`] is attempted; on success `on_event` is invoked with the parsed
///    [`SubagentEvent`]; on failure the line is silently skipped — `on_event` is simply not called
///    for that line, and reading continues with the next line (R-SA-026).
///
/// This function returns once the underlying reader reaches EOF (the child closed stdout,
/// normally because it exited) or a genuine I/O error occurs reading the next line — in the error
/// case, whatever complete lines were already read and processed remain processed; the error
/// itself is returned to the caller to decide how to classify the run (a later phase's concern,
/// not this function's). A final, unterminated trailing line (no trailing `\n` before EOF) is
/// flushed through this exact same per-line path exactly once, never dropped and never
/// double-parsed — `tokio::io::Lines::next_line` (which this function is built on) already
/// surfaces such a line as one final `Some(String)` before yielding `None`; see this module's
/// `flushes_a_final_unterminated_line_exactly_once_at_eof` test for a from-scratch proof of that
/// exact behavior rather than relying on it unverified.
///
/// # Errors
///
/// Returns the underlying `std::io::Error` from the first failed read, if any. No parse failure
/// of any individual line is ever surfaced as an `Err` here (R-SA-026) — only a genuine I/O fault
/// reading the byte stream itself is.
pub async fn consume_stdout<R>(
    reader: R,
    mut on_event: impl FnMut(SubagentEvent),
    mut tee: impl FnMut(&str),
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
{
    let mut lines = reader.lines();
    while let Some(line) = lines.next_line().await? {
        tee(&line);
        if let Some(event) = parse_line(&line) {
            on_event(event);
        }
        // R-SA-026: a line that failed to parse at all is silently skipped here — no error, no
        // abort, `on_event` simply is not invoked for it.
    }
    Ok(())
}

/// Like [`consume_stdout`], but collects every read line into an [`NdjsonLine`] and returns them
/// all once the stream ends, instead of driving live callbacks — a convenience used by this
/// module's own tests (and available to any later-phase caller that wants to buffer the whole
/// exchange rather than fold it incrementally, e.g. a short-lived `verify[]` command execution
/// per R-SA-032 that only needs the final event list, not a live progress fold).
///
/// # Errors
///
/// Returns the underlying `std::io::Error` from the first failed read, if any — identical error
/// contract to [`consume_stdout`].
pub async fn collect_ndjson<R>(reader: R) -> std::io::Result<Vec<NdjsonLine>>
where
    R: AsyncBufRead + Unpin,
{
    // `consume_stdout` invokes `tee` (raw line) strictly before `on_event` (parsed event) for the
    // same line, but takes two independent `FnMut` closures — a single `Vec` cannot be captured
    // mutably by both at once. A `RefCell` sidesteps that without weakening
    // `consume_stdout`'s own two-closure contract (which stays as-is since production callers
    // fold raw/parsed into genuinely separate destinations: a live `.jsonl` file write vs. an
    // in-memory progress fold).
    let collected = std::cell::RefCell::new(Vec::new());
    consume_stdout(
        reader,
        |event| {
            if let Some(last) = collected.borrow_mut().last_mut() {
                let last: &mut NdjsonLine = last;
                last.parsed = Some(event);
            }
        },
        |raw| {
            collected.borrow_mut().push(NdjsonLine {
                raw: raw.to_string(),
                parsed: None,
            });
        },
    )
    .await?;
    Ok(collected.into_inner())
}

/// Fold a batch of already-parsed [`SubagentEvent`]s into running per-tool `is_error` outcomes
/// keyed by `tool_call_id`, matching [`SubagentEvent::ToolExecutionStart`] against a later
/// [`SubagentEvent::ToolExecutionEnd`] — a small, self-contained convenience some later-phase
/// caller (progress rendering, completion-mutation guard's tool-call scan, R-SA-034) can reuse
/// instead of re-deriving the same `call_id` correlation. Not itself required by
/// R-SA-026/057/058; included here because [`SubagentEvent`] is this module's sole owner and this
/// is a pure function over already-parsed events, not a new I/O or parsing concern.
#[must_use]
pub fn correlate_tool_outcomes(events: &[SubagentEvent]) -> HashMap<String, bool> {
    let mut outcomes = HashMap::new();
    for event in events {
        if let SubagentEvent::ToolExecutionEnd {
            tool_call_id,
            is_error,
            ..
        } = event
        {
            outcomes.insert(tool_call_id.as_str().to_string(), *is_error);
        }
    }
    outcomes
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::ReadBuf;

    // ---- SubagentEvent: wire-shape parsing (R-SA-026/057) ----

    #[test]
    fn parses_tool_execution_start_with_camel_case_payload_fields() {
        let line = r#"{"type":"tool_execution_start","toolCallId":"c1","toolName":"bash","args":{"cmd":"ls"}}"#;
        let ev = parse_line(line).expect("valid, recognized shape must parse");
        assert_eq!(
            ev,
            SubagentEvent::ToolExecutionStart {
                tool_call_id: "c1".into(),
                tool_name: "bash".to_string(),
                args: serde_json::json!({"cmd": "ls"}),
            }
        );
    }

    /// A subagent child is a real `cyrup --print --mode json` re-exec, so its `message_update`
    /// lines are whatever `cyrup-modes`' json mode writes. Since the v0.84.1 wire projection (Pi
    /// `toJsonEvent`, `coding-agent/src/modes/json-event.ts:28-40`, applied at `print-mode.ts:110`)
    /// that is a TWO-key record with no cumulative `message`. This is the exact line a child emits,
    /// copied from the json-mode wire.
    ///
    /// Before the retype, `MessageUpdate` declared a required `message` field, so this line failed
    /// to deserialize and [`parse_line`]'s `.ok()` (`:316`) discarded the event entirely — not a
    /// tolerated degradation to [`SubagentEvent::Unknown`], which `#[serde(other)]` provides only
    /// for an unrecognized `type` TAG, never for a known tag with a missing field.
    #[test]
    fn parses_the_delta_only_message_update_the_json_wire_now_emits() {
        let line = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello"}}"#;
        // `expect` IS the assertion: with a required `message` field this returns `None` and the
        // child's streaming update vanishes from the parsed stream entirely.
        let ev = parse_line(line).expect(
            "the delta-only wire shape must parse — a required `message` field silently drops it",
        );
        assert_eq!(ev.kind(), "message_update", "must not degrade to Unknown");
        // Matched with `..` so this test compiles against either enum shape and therefore fails at
        // RUNTIME (with the message above) rather than at compile time.
        let delta = match ev {
            SubagentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => Some(assistant_message_event),
            _ => None,
        };
        assert_eq!(
            delta,
            Some(serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "Hello"})),
            "the delta payload survives verbatim"
        );
    }

    /// MIRROR: an OLDER cyrup child still emitting the pre-projection record (with the cumulative
    /// `message`) keeps parsing — serde ignores unknown fields, so dropping the field from the enum
    /// is tolerant in both directions rather than trading one break for another.
    #[test]
    fn still_parses_a_legacy_message_update_that_carries_the_cumulative_message() {
        let line = r#"{"type":"message_update","message":{"role":"assistant","content":[]},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello","partial":{"role":"assistant","content":[]}}}"#;
        let ev = parse_line(line).expect("a legacy child's line must still parse");
        assert_eq!(ev.kind(), "message_update");
        let delta = match ev {
            SubagentEvent::MessageUpdate {
                assistant_message_event,
                ..
            } => assistant_message_event,
            _ => serde_json::Value::Null,
        };
        assert_eq!(delta["delta"], "Hello", "the delta payload is still read");
    }

    #[test]
    fn parses_tool_execution_end_with_is_error_flag() {
        let line = r#"{"type":"tool_execution_end","toolCallId":"c1","toolName":"bash","result":"ok","isError":false}"#;
        let ev = parse_line(line).expect("valid shape must parse");
        assert_eq!(
            ev,
            SubagentEvent::ToolExecutionEnd {
                tool_call_id: "c1".into(),
                tool_name: "bash".to_string(),
                result: serde_json::json!("ok"),
                is_error: false,
            }
        );
        assert_eq!(ev.kind(), "tool_execution_end");
    }

    #[test]
    fn parses_message_end_and_extracts_nested_assistant_usage() {
        let line = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [{"type": "text", "text": "done"}],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude",
                "usage": {
                    "input": 10, "output": 20, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 30, "cost": {"input": 0.1, "output": 0.2, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.3}
                },
                "stopReason": "stop",
                "timestamp": 0
            }
        })
        .to_string();

        let ev = parse_line(&line).expect("message_end must parse");
        assert_eq!(ev.kind(), "message_end");
        let usage = ev.assistant_usage().expect("assistant usage must extract");
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.total_tokens, 30);
        assert!(!ev.is_error_or_aborted_message());
    }

    #[test]
    fn message_end_flags_error_stop_reason() {
        let line = serde_json::json!({
            "type": "message_end",
            "message": {
                "role": "assistant",
                "content": [],
                "api": "anthropic-messages",
                "provider": "anthropic",
                "model": "claude",
                "usage": {
                    "input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0,
                    "totalTokens": 0, "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
                },
                "stopReason": "error",
                "errorMessage": "boom",
                "timestamp": 0
            }
        })
        .to_string();
        let ev = parse_line(&line).expect("parses");
        assert!(ev.is_error_or_aborted_message());
    }

    #[test]
    fn message_end_wrapping_a_non_assistant_message_has_no_usage() {
        let line = serde_json::json!({
            "type": "message_end",
            "message": {"role": "user", "content": []}
        })
        .to_string();
        let ev = parse_line(&line).expect("parses");
        assert!(ev.assistant_usage().is_none());
        assert!(!ev.is_error_or_aborted_message());
    }

    #[test]
    fn unrecognized_type_tag_degrades_to_unknown_not_a_parse_error() {
        let ev = parse_line(r#"{"type":"session_replaced","generation":3}"#)
            .expect("a syntactically valid but unrecognized shape must still parse to Unknown");
        assert_eq!(ev, SubagentEvent::Unknown);
        assert_eq!(ev.kind(), "unknown");
    }

    #[test]
    fn totally_invalid_json_fails_to_parse_at_all() {
        assert!(parse_line("not json").is_none());
        assert!(parse_line("").is_none());
        assert!(parse_line("{\"type\": \"tool_execution_start\"").is_none()); // truncated
    }

    #[test]
    fn a_json_value_with_no_type_tag_fails_to_parse() {
        // A bare JSON array/number/object-without-type is not a `type`-tagged event at all —
        // this must fall into parse_line's None branch (R-SA-026's coarser tolerance), not
        // panic and not spuriously succeed.
        assert!(parse_line("[1,2,3]").is_none());
        assert!(parse_line("42").is_none());
        assert!(parse_line("{}").is_none());
    }

    #[test]
    fn agent_start_and_turn_start_are_unit_variants() {
        assert_eq!(
            parse_line(r#"{"type":"agent_start"}"#),
            Some(SubagentEvent::AgentStart)
        );
        assert_eq!(
            parse_line(r#"{"type":"turn_start"}"#),
            Some(SubagentEvent::TurnStart)
        );
    }

    #[test]
    fn agent_end_defaults_missing_optional_fields() {
        let ev = parse_line(r#"{"type":"agent_end"}"#).expect("agent_end with no payload parses");
        assert_eq!(
            ev,
            SubagentEvent::AgentEnd {
                messages: Vec::new(),
                will_retry: false
            }
        );
    }

    // ---- correlate_tool_outcomes ----

    #[test]
    fn correlate_tool_outcomes_keys_by_call_id() {
        let events = vec![
            SubagentEvent::ToolExecutionStart {
                tool_call_id: "c1".into(),
                tool_name: "bash".to_string(),
                args: serde_json::Value::Null,
            },
            SubagentEvent::ToolExecutionEnd {
                tool_call_id: "c1".into(),
                tool_name: "bash".to_string(),
                result: serde_json::Value::Null,
                is_error: true,
            },
        ];
        let outcomes = correlate_tool_outcomes(&events);
        assert_eq!(outcomes.get("c1"), Some(&true));
    }

    // ---- consume_stdout: streaming, tolerance, and live-tee behavior over an in-memory reader ----

    #[tokio::test]
    async fn consume_stdout_tees_every_raw_line_and_emits_only_parseable_events() {
        let input = concat!(
            "{\"type\":\"agent_start\"}\n",
            "this is not json\n",
            "{\"type\":\"tool_execution_start\",\"toolCallId\":\"c1\",\"toolName\":\"bash\"}\n",
            "{\"type\":\"agent_end\"}\n",
        );
        let reader = std::io::Cursor::new(input.as_bytes().to_vec());

        let mut teed = Vec::new();
        let mut events = Vec::new();
        consume_stdout(
            reader,
            |ev| events.push(ev),
            |line| teed.push(line.to_string()),
        )
        .await
        .expect("no I/O error over an in-memory reader");

        assert_eq!(
            teed,
            vec![
                "{\"type\":\"agent_start\"}".to_string(),
                "this is not json".to_string(),
                "{\"type\":\"tool_execution_start\",\"toolCallId\":\"c1\",\"toolName\":\"bash\"}"
                    .to_string(),
                "{\"type\":\"agent_end\"}".to_string(),
            ],
            "EVERY raw line must be teed, including the malformed one (R-SA-058 has no carve-out)"
        );
        assert_eq!(
            events.iter().map(SubagentEvent::kind).collect::<Vec<_>>(),
            vec!["agent_start", "tool_execution_start", "agent_end"],
            "the malformed line must be skipped from on_event without aborting subsequent lines \
             (R-SA-026)"
        );
    }

    #[tokio::test]
    async fn consume_stdout_flushes_a_final_unterminated_line_exactly_once_at_eof() {
        // No trailing '\n' after the last event — this is the exact scenario R-SA-026's last
        // sentence calls out: "On stream end ... any unterminated trailing buffered content MUST
        // be flushed through the same per-line parse path exactly once."
        let input = "{\"type\":\"agent_start\"}\n{\"type\":\"agent_end\"}";
        let reader = std::io::Cursor::new(input.as_bytes().to_vec());

        let mut events = Vec::new();
        let mut teed = Vec::new();
        consume_stdout(
            reader,
            |ev| events.push(ev),
            |line| teed.push(line.to_string()),
        )
        .await
        .expect("no I/O error");

        assert_eq!(
            teed,
            vec![
                "{\"type\":\"agent_start\"}".to_string(),
                "{\"type\":\"agent_end\"}".to_string()
            ],
            "the final line (no trailing newline) must still be teed exactly once"
        );
        assert_eq!(
            events.iter().map(SubagentEvent::kind).collect::<Vec<_>>(),
            vec!["agent_start", "agent_end"],
            "the final unterminated line must still be parsed exactly once, not dropped"
        );
    }

    /// A reader that deliberately splits the underlying byte stream into small, arbitrary chunks
    /// across multiple `poll_read` calls — including chunk boundaries that fall in the MIDDLE of
    /// a single NDJSON line — so this test exercises `consume_stdout` against genuinely partial
    /// reads, not merely a single already-complete in-memory buffer (`std::io::Cursor` handed to
    /// `tokio::io::BufReader` would satisfy every `poll_read` in one shot; this type deliberately
    /// does not).
    struct ChunkedReader {
        remaining: Vec<u8>,
        chunk_size: usize,
    }

    impl ChunkedReader {
        fn new(data: &[u8], chunk_size: usize) -> Self {
            Self {
                remaining: data.to_vec(),
                chunk_size,
            }
        }
    }

    impl tokio::io::AsyncRead for ChunkedReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let me = self.get_mut();
            if me.remaining.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let take = me.chunk_size.min(me.remaining.len()).min(buf.remaining());
            let take = take.max(1).min(me.remaining.len());
            let chunk: Vec<u8> = me.remaining.drain(..take).collect();
            buf.put_slice(&chunk);
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn consume_stdout_reassembles_lines_split_across_arbitrarily_small_partial_reads() {
        let input = concat!(
            "{\"type\":\"agent_start\"}\n",
            "{\"type\":\"tool_execution_start\",\"toolCallId\":\"c1\",\"toolName\":\"bash\"}\n",
            "garbage-not-json\n",
            "{\"type\":\"message_end\",\"message\":{\"role\":\"user\",\"content\":[]}}\n",
            "{\"type\":\"agent_end\"}\n",
        );

        // 3-byte chunks guarantee multiple line boundaries fall mid-chunk and multiple chunks are
        // needed to complete a single line — a real faithful stress of the buffering path
        // `tokio::io::Lines` (which `consume_stdout` is built on) must get right.
        let reader = tokio::io::BufReader::new(ChunkedReader::new(input.as_bytes(), 3));

        let mut events = Vec::new();
        let mut teed = Vec::new();
        consume_stdout(
            reader,
            |ev| events.push(ev),
            |line| teed.push(line.to_string()),
        )
        .await
        .expect("no I/O error across chunked partial reads");

        assert_eq!(
            teed.len(),
            5,
            "all five raw lines must be reassembled and teed, got {teed:?}"
        );
        assert_eq!(
            events.iter().map(SubagentEvent::kind).collect::<Vec<_>>(),
            vec![
                "agent_start",
                "tool_execution_start",
                "message_end",
                "agent_end"
            ],
            "four of the five lines parse; the malformed one is silently skipped, and reading \
             continues correctly past a chunk-split malformed line"
        );
    }

    #[tokio::test]
    async fn consume_stdout_reassembles_a_final_unterminated_line_across_partial_reads() {
        // Combines both stress cases: chunked partial reads AND no trailing newline on the very
        // last line.
        let input = "{\"type\":\"agent_start\"}\n{\"type\":\"agent_end\"}";
        let reader = tokio::io::BufReader::new(ChunkedReader::new(input.as_bytes(), 4));

        let mut events = Vec::new();
        consume_stdout(reader, |ev| events.push(ev), |_| {})
            .await
            .expect("no I/O error");

        assert_eq!(
            events.iter().map(SubagentEvent::kind).collect::<Vec<_>>(),
            vec!["agent_start", "agent_end"],
            "the final unterminated line must survive BOTH chunked reassembly and EOF-flush"
        );
    }

    #[tokio::test]
    async fn consume_stdout_on_an_entirely_empty_stream_emits_nothing() {
        let reader = std::io::Cursor::new(Vec::<u8>::new());
        let mut events = Vec::new();
        let mut teed = Vec::new();
        consume_stdout(
            reader,
            |ev| events.push(ev),
            |line| teed.push(line.to_string()),
        )
        .await
        .expect("no I/O error on an empty stream");
        assert!(events.is_empty());
        assert!(teed.is_empty());
    }

    #[tokio::test]
    async fn consume_stdout_tolerates_a_stream_of_entirely_malformed_lines() {
        let input = "not json\nalso not json\n{{{\n";
        let reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let mut events = Vec::new();
        let mut teed = Vec::new();
        consume_stdout(
            reader,
            |ev| events.push(ev),
            |line| teed.push(line.to_string()),
        )
        .await
        .expect("malformed lines are tolerated, never surfaced as an I/O error");
        assert_eq!(teed.len(), 3, "every malformed line must still be teed");
        assert!(
            events.is_empty(),
            "no event should be emitted for any of them"
        );
    }

    // ---- collect_ndjson ----

    #[tokio::test]
    async fn collect_ndjson_pairs_raw_text_with_its_parse_outcome() {
        let input = "{\"type\":\"agent_start\"}\nnope\n";
        let reader = std::io::Cursor::new(input.as_bytes().to_vec());
        let lines = collect_ndjson(reader).await.expect("no I/O error");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].raw, "{\"type\":\"agent_start\"}");
        assert!(matches!(lines[0].parsed, Some(SubagentEvent::AgentStart)));
        assert_eq!(lines[1].raw, "nope");
        assert!(lines[1].parsed.is_none());
    }

    // ---- Real-subprocess proof: consume_stdout against an actual child process's piped stdout ----
    //
    // Per this crate's testing convention (no mocked subprocess behavior — mirrors
    // `crate::spawn`'s own real-`sh`-child tests), this spawns a REAL `sh` process that emits
    // scripted NDJSON — including a deliberately malformed line and a deliberately delayed final
    // line with no trailing newline — and drains it through the exact same `consume_stdout`
    // this module exposes for production use, not just the in-memory Cursor/ChunkedReader tests
    // above.

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn consume_stdout_drains_a_real_child_processs_piped_stdout() {
        let sh_path = std::env::var_os("PATH")
            .and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("sh"))
                    .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));

        let mut child = tokio::process::Command::new(sh_path)
            .arg("-c")
            .arg(concat!(
                "printf '{\"type\":\"agent_start\"}\\n'; ",
                "printf 'not valid json\\n'; ",
                "printf '{\"type\":\"tool_execution_start\",\"toolCallId\":\"c1\",\"toolName\":\"bash\"}\\n'; ",
                "printf '{\"type\":\"agent_end\"}'" // deliberately no trailing newline
            ))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("real sh child spawns");

        let stdout = child.stdout.take().expect("stdout is piped");
        let reader = tokio::io::BufReader::new(stdout);

        let mut events = Vec::new();
        let mut teed = Vec::new();
        consume_stdout(
            reader,
            |ev| events.push(ev),
            |line| teed.push(line.to_string()),
        )
        .await
        .expect("no I/O error draining a real child's stdout");

        let status = child.wait().await.expect("child exits cleanly");
        assert!(status.success());

        assert_eq!(
            teed.len(),
            4,
            "all four raw lines, including the malformed one and the final \
                                    unterminated one, must be teed: {teed:?}"
        );
        assert_eq!(
            events.iter().map(SubagentEvent::kind).collect::<Vec<_>>(),
            vec!["agent_start", "tool_execution_start", "agent_end"],
            "the malformed line is skipped; the final unterminated line still parses"
        );
    }
}
