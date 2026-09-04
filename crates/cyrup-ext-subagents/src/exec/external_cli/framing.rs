//! SUBA-074 stage 2 — the bounded framing an external CLI's streams are read through:
//! `parseExternalCliJsonlEvent`, `createByteTail`, `writeBoundedLog`, the eight code-owned byte
//! ceilings and the line splitter with its oversized-line rule
//! (`pi-subagents/src/runs/shared/external-cli-runner.ts:16-23`, `:49-60`, `:103-136`, `:273-329`
//! @v0.64.0).
//!
//! Every number here is a CEILING this crate owns. A caller may only narrow one
//! ([`StreamLimits::narrowed`]), never widen it — the same "user config cannot widen code-owned
//! limits" stance the capability contract takes.

use std::collections::VecDeque;
use std::io::Write;

use serde_json::{Map, Value};

/// `MAX_OUTPUT_TAIL_BYTES` (`:16`) — how much stdout is kept for the delivered output when no
/// parser produced a terminal value.
pub const MAX_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
/// `MAX_ERROR_TAIL_BYTES` (`:17`) — how much stderr is kept for the failure message.
pub const MAX_ERROR_TAIL_BYTES: usize = 64 * 1024;
/// `MAX_RAW_LOG_BYTES` (`:18`) — the cap on each of the two on-disk stream logs.
pub const MAX_RAW_LOG_BYTES: usize = 8 * 1024 * 1024;
/// `MAX_PARSER_LINE_BYTES` (`:19`) — the longest single JSONL line a parser is handed.
pub const MAX_PARSER_LINE_BYTES: usize = 256 * 1024;
/// `MAX_PARSER_STREAM_BYTES` (`:20`) — the total bytes a parser may be fed across the whole run.
pub const MAX_PARSER_STREAM_BYTES: usize = 32 * 1024 * 1024;
/// `MAX_PARSER_OUTPUT_BYTES` (`:21`) — the cap on a parser's terminal OUTPUT.
pub const MAX_PARSER_OUTPUT_BYTES: usize = 1024 * 1024;
/// `MAX_OVERSIZED_LINE_PREFIX_BYTES` (`:22`) — how much of an over-long line a parser may inspect.
pub const MAX_OVERSIZED_LINE_PREFIX_BYTES: usize = 512;
/// `MAX_SKIPPABLE_LINE_BYTES` (`:23`) — beyond this even a skippable line fails the parse.
pub const MAX_SKIPPABLE_LINE_BYTES: usize = 1024 * 1024;
/// The cap on a parser's terminal ERROR string (`:374`).
pub const MAX_PARSER_ERROR_BYTES: usize = 4 * 1024;

/// The five narrowable stream limits (`:74-80`).
///
/// [CYRUP-DELTA] upstream's `narrowLimit` (`:82-86`) uses `assert`, which throws into a caught
/// promise. This crate is linked in-process by the TUI, so a panic on a narrowing violation would
/// take the host down; [`Self::narrowed`] returns `Err` instead. No user-reachable input sets these
/// today — `parseAgentRunnerFrontmatter` accepts no `limits` key (`agents.ts:1890`) and none of the
/// adapters narrows one — so this exists for a future in-repo caller, exactly as upstream's does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamLimits {
    /// Bytes written to the stdout log.
    pub stdout_log_bytes: usize,
    /// Bytes written to the stderr log.
    pub stderr_log_bytes: usize,
    /// Longest line handed to a parser.
    pub parser_line_bytes: usize,
    /// Total bytes fed to a parser.
    pub parser_stream_bytes: usize,
    /// Longest terminal output a parser may return.
    pub parser_output_bytes: usize,
    /// Whether `parser_line_bytes` was narrowed by a caller. Upstream gates the oversized-line SKIP
    /// on `input.limits?.parserLineBytes === undefined` (`:276-278`): a caller who narrowed the line
    /// cap meant it, so an over-long line fails instead of being skipped.
    pub parser_line_bytes_narrowed: bool,
}

impl Default for StreamLimits {
    /// The code-owned ceilings.
    fn default() -> Self {
        Self {
            stdout_log_bytes: MAX_RAW_LOG_BYTES,
            stderr_log_bytes: MAX_RAW_LOG_BYTES,
            parser_line_bytes: MAX_PARSER_LINE_BYTES,
            parser_stream_bytes: MAX_PARSER_STREAM_BYTES,
            parser_output_bytes: MAX_PARSER_OUTPUT_BYTES,
            parser_line_bytes_narrowed: false,
        }
    }
}

impl StreamLimits {
    /// `narrowLimit` over the five limits (`:176-182`): `None` keeps the ceiling, `Some(n)` must be
    /// a positive value no greater than it.
    ///
    /// # Errors
    ///
    /// Upstream's message for the first limit that tries to widen.
    pub fn narrowed(
        stdout_log_bytes: Option<usize>,
        stderr_log_bytes: Option<usize>,
        parser_line_bytes: Option<usize>,
        parser_stream_bytes: Option<usize>,
        parser_output_bytes: Option<usize>,
    ) -> Result<Self, String> {
        fn narrow(value: Option<usize>, ceiling: usize, label: &str) -> Result<usize, String> {
            match value {
                None => Ok(ceiling),
                Some(value) if value > 0 && value <= ceiling => Ok(value),
                Some(_) => Err(format!(
                    "{label} may only narrow the code-owned {ceiling}-byte limit."
                )),
            }
        }
        Ok(Self {
            stdout_log_bytes: narrow(stdout_log_bytes, MAX_RAW_LOG_BYTES, "stdoutLogBytes")?,
            stderr_log_bytes: narrow(stderr_log_bytes, MAX_RAW_LOG_BYTES, "stderrLogBytes")?,
            parser_line_bytes: narrow(parser_line_bytes, MAX_PARSER_LINE_BYTES, "parserLineBytes")?,
            parser_stream_bytes: narrow(
                parser_stream_bytes,
                MAX_PARSER_STREAM_BYTES,
                "parserStreamBytes",
            )?,
            parser_output_bytes: narrow(
                parser_output_bytes,
                MAX_PARSER_OUTPUT_BYTES,
                "parserOutputBytes",
            )?,
            parser_line_bytes_narrowed: parser_line_bytes.is_some(),
        })
    }
}

/// `createByteTail(maxBytes)` (`:103-124`) — the LAST `max_bytes` of a stream, kept whole-chunk
/// where possible and sliced at the front when the oldest chunk only partly overflows.
#[derive(Debug)]
pub struct ByteTail {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    max_bytes: usize,
}

impl ByteTail {
    /// A tail bounded at `max_bytes`.
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            bytes: 0,
            max_bytes,
        }
    }

    /// Append a chunk, evicting from the front until the tail fits.
    pub fn push(&mut self, chunk: &[u8]) {
        self.chunks.push_back(chunk.to_vec());
        self.bytes += chunk.len();
        while self.bytes > self.max_bytes && !self.chunks.is_empty() {
            let excess = self.bytes - self.max_bytes;
            let Some(first) = self.chunks.front_mut() else {
                break;
            };
            if first.len() <= excess {
                self.bytes -= first.len();
                self.chunks.pop_front();
            } else {
                first.drain(..excess);
                self.bytes -= excess;
            }
        }
    }

    /// The tail as UTF-8, lossily — a tail can begin mid-codepoint by construction.
    #[must_use]
    pub fn text(&self) -> String {
        let mut buffer = Vec::with_capacity(self.bytes);
        for chunk in &self.chunks {
            buffer.extend_from_slice(chunk);
        }
        String::from_utf8_lossy(&buffer).into_owned()
    }
}

/// `writeBoundedLog` (`:126-136`) — a stream log that stops writing at its cap while still counting
/// the TOTAL bytes the child produced, so a truncated log is reported as truncated rather than
/// silently short.
#[derive(Debug)]
pub struct BoundedLog {
    file: Option<std::fs::File>,
    written: u64,
    total: u64,
    limit: u64,
}

impl BoundedLog {
    /// Open (truncating) the log at `path`. A log that cannot be opened degrades to counting only —
    /// observability must never fail the run, which is upstream's stance for the same artifact
    /// (`subagent-runner.ts:1523`).
    #[must_use]
    pub fn create(path: &std::path::Path, limit: usize) -> Self {
        Self {
            file: std::fs::File::create(path).ok(),
            written: 0,
            total: 0,
            limit: limit as u64,
        }
    }

    /// Record `chunk`, writing at most the remaining allowance.
    pub fn push(&mut self, chunk: &[u8]) {
        self.total += chunk.len() as u64;
        let remaining = self.limit.saturating_sub(self.written);
        if remaining == 0 {
            return;
        }
        let take = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(chunk.len());
        self.written += take as u64;
        if let Some(file) = self.file.as_mut()
            && let Some(bytes) = chunk.get(..take)
        {
            let _ = file.write_all(bytes);
        }
    }

    /// Total bytes the child produced on this stream.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Whether the log stopped short of the total (`:395-396`).
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.total > self.written
    }
}

/// `parseExternalCliJsonlEvent(line, label, maxTypeLength)` (`:49-60`).
///
/// # Errors
///
/// Upstream's three refusals, verbatim: malformed JSON, a non-object event, and an event whose
/// `type` is missing, empty, or longer than `max_type_length`.
pub fn parse_external_cli_jsonl_event(
    line: &str,
    label: &str,
    max_type_length: usize,
) -> Result<Map<String, Value>, String> {
    let value: Value = serde_json::from_str(line)
        .map_err(|error| format!("{label} emitted malformed JSONL: {error}"))?;
    let Value::Object(event) = value else {
        return Err(format!(
            "{label} emitted a JSONL event that is not an object."
        ));
    };
    let valid_type = event
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| !kind.is_empty() && kind.len() <= max_type_length);
    if !valid_type {
        return Err(format!(
            "{label} emitted a JSONL event with an invalid type."
        ));
    }
    Ok(event)
}

/// `ExternalCliParserProgress` (`:30-34`) — one parser observation, throttled by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParserProgress {
    /// `streaming`, or the terminal state's name.
    pub phase: String,
    /// How many events the parser has accepted.
    pub event_count: u64,
}

/// `ExternalCliParserTerminal` (`:36-40`) — the single terminal state a parser may reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParserTerminal {
    /// Whether the foreign run finished successfully.
    pub completed: bool,
    /// The delivered output, on success.
    pub output: Option<String>,
    /// The failure text, on failure.
    pub error: Option<String>,
}

impl ParserTerminal {
    /// `{ state: "completed" | "failed" }` as a word, for [`ParserProgress::phase`].
    #[must_use]
    pub const fn state(&self) -> &'static str {
        if self.completed {
            "completed"
        } else {
            "failed"
        }
    }
}

/// What a parser does with an event that arrives AFTER it already reached a terminal state.
///
/// The three upstream adapters disagree here in a way that is easy to lose in a port: claude-code
/// rejects only a DUPLICATE `result` and keeps counting everything else
/// (`claude-code-adapter.ts:63`), while codex-exec and cursor-agent reject ANY post-terminal event
/// (`codex-exec-adapter.ts:44`, `cursor-agent-adapter.ts:38`). Naming the policy makes the
/// divergence a constant a reviewer can diff, rather than an `if` buried in three parsers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterTerminal {
    /// Any event after the terminal one is a protocol error.
    RejectAny,
    /// Only a second terminal event of the adapter's own terminal type is an error.
    RejectDuplicateTerminal,
}

/// Split a byte stream into JSONL lines with upstream's oversized-line rule (`:289-329`).
///
/// The rule, in upstream's order: a line whose accumulated length passes `parser_line_bytes` is no
/// longer buffered whole; if it passes [`MAX_SKIPPABLE_LINE_BYTES`] the parse FAILS; otherwise a
/// bounded [`MAX_OVERSIZED_LINE_PREFIX_BYTES`] prefix is offered to the parser's skip hook, and the
/// rest of the line is discarded.
#[derive(Debug)]
pub struct LineSplitter {
    pending: Vec<u8>,
    pending_bytes: usize,
    oversized_accepted: bool,
    stream_bytes: usize,
    limits: StreamLimits,
}

/// One decision the splitter hands back for a completed (or over-long) line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineEvent {
    /// A normal line, to be parsed.
    Line(String),
    /// A line too long to buffer, offered as a bounded prefix plus its true byte length.
    Oversized {
        /// At most [`MAX_OVERSIZED_LINE_PREFIX_BYTES`] of the line.
        prefix: String,
        /// The line's real length in bytes.
        byte_length: usize,
    },
    /// The stream or a line blew a hard ceiling; the parse is over.
    Failed(String),
}

impl LineSplitter {
    /// A splitter bounded by `limits`.
    #[must_use]
    pub const fn new(limits: StreamLimits) -> Self {
        Self {
            pending: Vec::new(),
            pending_bytes: 0,
            oversized_accepted: false,
            stream_bytes: 0,
            limits,
        }
    }

    /// Feed a chunk, returning the line events it completed.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<LineEvent> {
        let mut events = Vec::new();
        self.stream_bytes += chunk.len();
        if self.stream_bytes > self.limits.parser_stream_bytes {
            events.push(LineEvent::Failed(
                "External CLI parser stream exceeded its byte limit.".to_string(),
            ));
            return events;
        }
        let mut start = 0;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            self.append(chunk.get(start..index).unwrap_or_default(), &mut events);
            self.finish_line(&mut events);
            start = index + 1;
        }
        self.append(chunk.get(start..).unwrap_or_default(), &mut events);
        events
    }

    /// Flush a trailing partial line at end of stream (`:369`).
    pub fn finish(&mut self) -> Vec<LineEvent> {
        let mut events = Vec::new();
        if self.pending_bytes > 0 {
            self.finish_line(&mut events);
        }
        events
    }

    /// `appendPendingLine` (`:289-307`).
    fn append(&mut self, chunk: &[u8], events: &mut Vec<LineEvent>) {
        self.pending_bytes += chunk.len();
        if self.oversized_accepted {
            if self.pending_bytes > MAX_SKIPPABLE_LINE_BYTES {
                events.push(LineEvent::Failed(
                    "External CLI parser line exceeded its byte limit.".to_string(),
                ));
            }
            return;
        }
        if self.pending_bytes <= self.limits.parser_line_bytes {
            self.pending.extend_from_slice(chunk);
            return;
        }
        if self.pending_bytes > MAX_SKIPPABLE_LINE_BYTES {
            events.push(LineEvent::Failed(
                "External CLI parser line exceeded its byte limit.".to_string(),
            ));
            return;
        }
        self.pending.truncate(MAX_OVERSIZED_LINE_PREFIX_BYTES);
        let remaining = MAX_OVERSIZED_LINE_PREFIX_BYTES.saturating_sub(self.pending.len());
        if remaining > 0 {
            self.pending
                .extend_from_slice(chunk.get(..remaining.min(chunk.len())).unwrap_or_default());
        }
        // A caller who NARROWED the line cap meant it, so the skip is not offered (`:276-278`).
        if self.limits.parser_line_bytes_narrowed {
            events.push(LineEvent::Failed(
                "External CLI parser line exceeded its byte limit.".to_string(),
            ));
            return;
        }
        events.push(LineEvent::Oversized {
            prefix: String::from_utf8_lossy(&self.pending).into_owned(),
            byte_length: self.pending_bytes,
        });
        self.oversized_accepted = true;
    }

    /// `finishPendingLine` (`:308-313`).
    fn finish_line(&mut self, events: &mut Vec<LineEvent>) {
        if !self.oversized_accepted {
            events.push(LineEvent::Line(
                String::from_utf8_lossy(&self.pending).into_owned(),
            ));
        }
        self.pending.clear();
        self.pending_bytes = 0;
        self.oversized_accepted = false;
    }
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

    /// The three JSONL refusals, verbatim (`:49-60`).
    #[test]
    fn a_jsonl_event_must_be_an_object_with_a_short_non_empty_type() {
        let event =
            parse_external_cli_jsonl_event(r#"{"type":"result","ok":true}"#, "Claude Code", 128)
                .unwrap();
        assert_eq!(event.get("type").and_then(Value::as_str), Some("result"));
        assert!(
            parse_external_cli_jsonl_event("{oops", "Claude Code", 128)
                .unwrap_err()
                .starts_with("Claude Code emitted malformed JSONL: ")
        );
        assert_eq!(
            parse_external_cli_jsonl_event("[]", "Claude Code", 128).unwrap_err(),
            "Claude Code emitted a JSONL event that is not an object."
        );
        assert_eq!(
            parse_external_cli_jsonl_event(r#"{"type":""}"#, "Claude Code", 128).unwrap_err(),
            "Claude Code emitted a JSONL event with an invalid type."
        );
        assert_eq!(
            parse_external_cli_jsonl_event(r#"{"nope":1}"#, "Claude Code", 128).unwrap_err(),
            "Claude Code emitted a JSONL event with an invalid type."
        );
        let long = format!(r#"{{"type":"{}"}}"#, "t".repeat(129));
        assert_eq!(
            parse_external_cli_jsonl_event(&long, "Claude Code", 128).unwrap_err(),
            "Claude Code emitted a JSONL event with an invalid type."
        );
    }

    /// The tail keeps the LAST bytes and slices the oldest chunk when it only partly overflows
    /// (`:110-120`).
    #[test]
    fn the_byte_tail_keeps_the_end_of_the_stream() {
        let mut tail = ByteTail::new(5);
        tail.push(b"abc");
        tail.push(b"defgh");
        assert_eq!(tail.text(), "defgh");
        let mut tail = ByteTail::new(4);
        tail.push(b"abcdef");
        assert_eq!(tail.text(), "cdef");
    }

    /// A log stops at its cap but still counts everything the child wrote (`:126-136`, `:393-396`).
    #[test]
    fn a_bounded_log_reports_the_true_total_and_flags_truncation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stdout.log");
        let mut log = BoundedLog::create(&path, 4);
        log.push(b"abc");
        assert!(!log.truncated());
        log.push(b"defgh");
        assert_eq!(log.total(), 8);
        assert!(log.truncated());
        assert_eq!(std::fs::read(&path).unwrap(), b"abcd");
    }

    /// A line split across chunk boundaries is reassembled; a trailing partial line is flushed at
    /// end of stream (`:314-329`, `:369`).
    #[test]
    fn lines_are_reassembled_across_chunk_boundaries() {
        let mut splitter = LineSplitter::new(StreamLimits::default());
        assert!(splitter.push(b"{\"a\":").is_empty());
        assert_eq!(
            splitter.push(b"1}\n{\"b\":2}\n"),
            vec![
                LineEvent::Line("{\"a\":1}".to_string()),
                LineEvent::Line("{\"b\":2}".to_string())
            ]
        );
        assert!(splitter.push(b"tail").is_empty());
        assert_eq!(splitter.finish(), vec![LineEvent::Line("tail".to_string())]);
    }

    /// An over-long but skippable line is offered as a bounded PREFIX and the rest is discarded;
    /// past `MAX_SKIPPABLE_LINE_BYTES` it fails the parse outright (`:289-307`).
    #[test]
    fn an_oversized_line_is_offered_as_a_bounded_prefix_then_fails_past_the_hard_cap() {
        let limits = StreamLimits {
            parser_line_bytes: 16,
            ..StreamLimits::default()
        };
        let mut splitter = LineSplitter::new(limits);
        let events = splitter.push(&[b'x'; 64]);
        match events.as_slice() {
            [
                LineEvent::Oversized {
                    prefix,
                    byte_length,
                },
            ] => {
                assert_eq!(prefix.len(), 64.min(MAX_OVERSIZED_LINE_PREFIX_BYTES));
                assert_eq!(*byte_length, 64);
            }
            other => panic!("expected one oversized event, got {other:?}"),
        }
        // The remainder of the same line produces nothing more until the newline.
        assert!(splitter.push(&[b'x'; 8]).is_empty());
        assert!(splitter.push(b"\n").is_empty());

        let mut splitter = LineSplitter::new(limits);
        let events = splitter.push(&vec![b'x'; MAX_SKIPPABLE_LINE_BYTES + 1]);
        assert_eq!(
            events,
            vec![LineEvent::Failed(
                "External CLI parser line exceeded its byte limit.".to_string()
            )]
        );
    }

    /// A caller who narrowed the line cap gets a FAILURE rather than a skip (`:276-278`).
    #[test]
    fn a_narrowed_line_cap_disables_the_oversized_skip() {
        let limits = StreamLimits::narrowed(None, None, Some(16), None, None).unwrap();
        assert!(limits.parser_line_bytes_narrowed);
        let mut splitter = LineSplitter::new(limits);
        assert_eq!(
            splitter.push(&[b'x'; 64]),
            vec![LineEvent::Failed(
                "External CLI parser line exceeded its byte limit.".to_string()
            )]
        );
    }

    /// The whole stream is bounded too (`:317-320`).
    #[test]
    fn the_parser_stream_is_bounded_as_a_whole() {
        let limits = StreamLimits::narrowed(None, None, None, Some(8), None).unwrap();
        let mut splitter = LineSplitter::new(limits);
        assert_eq!(
            splitter.push(&[b'x'; 9]),
            vec![LineEvent::Failed(
                "External CLI parser stream exceeded its byte limit.".to_string()
            )]
        );
    }

    /// A limit may only be NARROWED, and the refusal is a `Result` rather than a panic — this crate
    /// is linked in-process by the TUI, where upstream's `assert` would take the host down.
    #[test]
    fn a_limit_may_only_narrow_the_code_owned_ceiling() {
        assert_eq!(
            StreamLimits::narrowed(Some(MAX_RAW_LOG_BYTES + 1), None, None, None, None)
                .unwrap_err(),
            format!(
                "stdoutLogBytes may only narrow the code-owned {MAX_RAW_LOG_BYTES}-byte limit."
            )
        );
        assert!(StreamLimits::narrowed(Some(0), None, None, None, None).is_err());
        assert_eq!(
            StreamLimits::narrowed(None, None, None, None, None).unwrap(),
            StreamLimits::default()
        );
    }
}
