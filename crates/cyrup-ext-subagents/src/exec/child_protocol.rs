//! Bounded child-protocol I/O: the 16 MiB stdout line cap, its `protocol_output_limit`
//! diagnostic, the oversized-aggregate-line projection, the bounded stderr byte tail, and the
//! child lifecycle (drain start/cancel) projection.
//!
//! A faithful port of pi-subagents' `src/runs/shared/child-protocol.ts` (v0.43.0, 401 lines),
//! which both foreground (`runs/foreground/execution.ts:1042-1052`) and background
//! (`runs/background/subagent-runner.ts:745-755`) child readers are built on upstream.
//!
//! # Why a cap at all
//!
//! The parent reads a child subprocess's stdout. Before this module, that read was
//! `tokio::io::Lines`, which grows one `String` until it sees a `\n` — a child that emits a single
//! enormous line (or never emits a newline at all) grows the PARENT's heap without bound. The cap
//! turns that from an OOM into a diagnosed, terminating failure carrying a
//! [`ProtocolOutputLimit`] the run reports as its error
//! (`child-protocol.ts:6,244-293`, `execution.ts:1026-1041`).
//!
//! # Why the cap alone would be a regression, and what the projector is for
//!
//! cyrup's json mode emits granular `message_*`/`tool_execution_*` events AND the aggregate
//! `turn_end` (the whole assistant message + every tool result) and `agent_end` (every message of
//! the run). Those aggregates duplicate payload that was already streamed granularly, so a run
//! that reads a few large images can legitimately push ONE aggregate record past the cap even
//! though every granular event was small and valid. Capping without a recovery path would
//! therefore fail runs that upstream completes. [`PI_AGGREGATE_EVENT_PROJECTOR`] is upstream's
//! recovery path (`child-protocol.ts:226-238`): while an oversized line is still arriving, it is
//! fed to a streaming JSON *validator* that keeps only the top-level `type` and `willRetry`
//! fields; if the whole line turns out to be syntactically valid JSON, the reader emits the
//! reduced record (`{"type":"turn_end"}` / `{"type":"agent_end","willRetry":…}`) — the only fields
//! the run loop consumes from those two events — instead of failing. Anything else oversized, or
//! an oversized aggregate that is NOT valid JSON, still fails the run.
//!
//! # [CYRUP-DELTA]s, all documented rather than silent
//!
//! * Upstream's reader takes `onLine`/`onLimit` CALLBACKS; this one queues lines and the limit
//!   internally ([`BoundedLineReader::take_line`]/[`BoundedLineReader::take_limit`]). Two `FnMut`
//!   closures cannot both borrow the caller's state mutably in Rust, and — more importantly — the
//!   production reader is polled from inside a `tokio::select!` arm that must stay
//!   cancellation-safe, which a callback-driven push API cannot be.
//! * Upstream's `Buffer.toString("utf8")` replaces malformed UTF-8 with U+FFFD; this port uses
//!   `String::from_utf8_lossy`, which is the same substitution. The projector's own decode is
//!   strict (upstream uses `new TextDecoder("utf-8", { fatal: true })`), so an oversized aggregate
//!   carrying malformed UTF-8 fails the projection in both.
//! * A single trailing `\r` is stripped from each emitted line. Upstream splits on `\n` only and
//!   keeps the `\r`; cyrup's previous reader (`tokio::io::Lines`) stripped it, and the raw line is
//!   teed verbatim to the `.jsonl` artifact (R-SA-058), so keeping the strip preserves the
//!   artifact bytes this crate already produces.

use std::collections::VecDeque;

use tokio::io::{AsyncRead, AsyncReadExt};

/// The per-line stdout cap (`child-protocol.ts:6`): 16 MiB of a single line with no newline.
pub const MAX_CHILD_PENDING_LINE_BYTES: usize = 16 * 1024 * 1024;

/// The stderr cap (`child-protocol.ts:7`), used both as the per-line cap for the stderr reader and
/// as the size of the retained stderr tail surfaced into a failed run's error.
pub const MAX_CHILD_STDERR_BYTES: usize = 128 * 1024;

/// How much of an over-limit line's head/tail is retained for the diagnostic
/// (`child-protocol.ts:8`).
const MAX_PROTOCOL_DIAGNOSTIC_BYTES: usize = 4096;

/// Container-nesting ceiling for the oversized-line projection (`child-protocol.ts:9`).
const MAX_PROJECTED_JSON_DEPTH: usize = 256;

/// The stable machine-readable code of a [`ProtocolOutputLimit`] (`child-protocol.ts:285`).
pub const PROTOCOL_OUTPUT_LIMIT_CODE: &str = "protocol_output_limit";

/// Which of a child's two streams tripped the cap (`child-protocol.ts:287`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProtocolStream {
    /// The child's NDJSON protocol stream.
    Stdout,
    /// The child's diagnostic stream (R-SA-046: never protocol data).
    Stderr,
}

impl ProtocolStream {
    /// The wire spelling (`"stdout"`/`"stderr"`), as it appears in the formatted diagnostic.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ProtocolStream::Stdout => "stdout",
            ProtocolStream::Stderr => "stderr",
        }
    }
}

/// A child exceeded its per-line output cap (`shared/types.ts:804-811`).
///
/// Carries enough to diagnose WHICH stream, how large the line got before the reader gave up, and
/// bounded head/tail excerpts of it — never the line itself, which is by construction too large to
/// keep.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolOutputLimit {
    /// Always [`PROTOCOL_OUTPUT_LIMIT_CODE`].
    pub code: String,
    /// The stream that tripped the cap.
    pub stream: ProtocolStream,
    /// The cap that was exceeded, in bytes.
    pub limit_bytes: usize,
    /// How many bytes of the single line had been observed when the reader gave up. "At least",
    /// because the reader stops accumulating at that point.
    pub observed_bytes: usize,
    /// Up to [`MAX_PROTOCOL_DIAGNOSTIC_BYTES`] of the line's head.
    pub diagnostic_prefix: String,
    /// Up to [`MAX_PROTOCOL_DIAGNOSTIC_BYTES`] of the line's tail as observed so far.
    pub diagnostic_tail: String,
}

/// The human-facing error text a limited run reports (`child-protocol.ts:240-242`, verbatim).
#[must_use]
pub fn format_protocol_output_limit(limit: &ProtocolOutputLimit) -> String {
    format!(
        "{}: child {} line exceeded {} bytes (observed at least {} bytes without a newline).",
        limit.code,
        limit.stream.as_str(),
        limit.limit_bytes,
        limit.observed_bytes
    )
}

// ------------------------------------------------------------------------------------------------
// createBoundedByteTail (child-protocol.ts:370-392)
// ------------------------------------------------------------------------------------------------

/// A fixed-size ring over the LAST `max_bytes` of a byte stream, trimmed to a UTF-8 boundary
/// (`child-protocol.ts:370-392`) — how upstream keeps a child's trailing stderr for the run's
/// error without letting a chatty child grow the parent's heap.
#[derive(Debug)]
pub struct BoundedByteTail {
    tail: Vec<u8>,
    max_bytes: usize,
}

impl BoundedByteTail {
    /// A tail retaining at most `max_bytes` (clamped to at least 1, mirroring upstream's
    /// positive-integer precondition without making construction fallible).
    #[must_use]
    pub fn new(max_bytes: usize) -> Self {
        Self {
            tail: Vec::new(),
            max_bytes: max_bytes.max(1),
        }
    }

    /// Append `chunk`, discarding whatever no longer fits at the front.
    pub fn push(&mut self, chunk: &[u8]) {
        self.tail.extend_from_slice(chunk);
        if self.tail.len() > self.max_bytes {
            let mut start = self.tail.len().saturating_sub(self.max_bytes);
            // Advance off any UTF-8 continuation bytes so the retained tail starts on a character
            // boundary (`trimToUtf8Boundary`, `child-protocol.ts:370-375`).
            while start < self.tail.len()
                && self
                    .tail
                    .get(start)
                    .is_some_and(|byte| (byte & 0xc0) == 0x80)
            {
                start += 1;
            }
            self.tail.drain(..start);
        }
    }

    /// The retained tail as text (malformed sequences replaced, as `Buffer.toString("utf8")` does).
    #[must_use]
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.tail).into_owned()
    }

    /// The retained tail's length in bytes.
    #[must_use]
    pub fn byte_length(&self) -> usize {
        self.tail.len()
    }
}

impl Default for BoundedByteTail {
    fn default() -> Self {
        Self::new(MAX_CHILD_STDERR_BYTES)
    }
}

// ------------------------------------------------------------------------------------------------
// projectChildLifecycle (child-protocol.ts:394-401)
// ------------------------------------------------------------------------------------------------

/// What one child event says about the parent's final-drain window
/// (`child-protocol.ts:394-401`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildLifecycleAction {
    /// Arm the final-stop grace window: the child says it is done, so a child still holding stdout
    /// after the window is force-drained through the signal ladder.
    StartDrain,
    /// Disarm it: the child says it is about to retry, so the window must not fire against a run
    /// that is legitimately still working.
    CancelDrain,
    /// Neither.
    None,
}

/// Project one child event onto a drain action (`projectChildLifecycle`,
/// `child-protocol.ts:396-401`, verbatim including the ordering).
///
/// The `will_retry` arm is checked FIRST and it is what makes the whole projection load-bearing:
/// `agent_end` with `willRetry: true` is a child announcing an auto-retry, i.e. an `agent_end` that
/// is NOT the end of the run. Treating it as terminal (or letting a previously armed window survive
/// it) force-kills a child mid-retry.
#[must_use]
pub fn project_child_lifecycle(
    event_type: &str,
    will_retry: bool,
    terminal_assistant_stop: bool,
) -> ChildLifecycleAction {
    if event_type == "agent_end" && will_retry {
        return ChildLifecycleAction::CancelDrain;
    }
    if event_type == "agent_settled" {
        return ChildLifecycleAction::StartDrain;
    }
    if terminal_assistant_stop {
        return ChildLifecycleAction::StartDrain;
    }
    ChildLifecycleAction::None
}

// ------------------------------------------------------------------------------------------------
// createPiAggregateProjection (child-protocol.ts:30-238)
// ------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayState {
    ValueOrEnd,
    Value,
    CommaOrEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectState {
    KeyOrEnd,
    Key,
    Colon,
    Value,
    CommaOrEnd,
}

#[derive(Debug)]
enum Container {
    Array(ArrayState),
    Object {
        state: ObjectState,
        key: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringRole {
    Key,
    Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumberPhase {
    Minus,
    Zero,
    Int,
    Dot,
    Frac,
    Exp,
    ExpSign,
    ExpDigits,
}

impl NumberPhase {
    /// The phases a number may legally END in (`child-protocol.ts:165,214`).
    fn is_terminal(self) -> bool {
        matches!(
            self,
            NumberPhase::Zero | NumberPhase::Int | NumberPhase::Frac | NumberPhase::ExpDigits
        )
    }
}

#[derive(Debug)]
enum Token {
    Str {
        role: StringRole,
        value: String,
        capture: bool,
        escape: bool,
        unicode_digits: u8,
        unicode_value: String,
    },
    Literal {
        expected: &'static str,
        index: usize,
        value: Option<bool>,
    },
    Number {
        phase: NumberPhase,
    },
}

/// A completed JSON value, only as far as the projection cares: a captured `type` string, a
/// `willRetry` boolean, or "something else" (`completeValue`, `child-protocol.ts:42-54`).
#[derive(Debug)]
enum CompletedValue {
    Str(String),
    Bool(bool),
    Other,
}

/// The streaming JSON validator behind [`PI_AGGREGATE_EVENT_PROJECTOR`]
/// (`createPiAggregateProjection`, `child-protocol.ts:30-232`).
///
/// It never materializes the oversized line: it walks it character by character, tracking only the
/// container stack, the current token, and the two top-level fields the run loop consumes. If the
/// whole line parses, [`Self::finish`] returns the reduced record; anything malformed, too deeply
/// nested, or with a `type` longer than 64 characters invalidates the projection and the reader
/// falls back to failing the run.
#[derive(Debug)]
pub struct PiAggregateProjection {
    stack: Vec<Container>,
    root_ended: bool,
    token: Option<Token>,
    valid: bool,
    event_type: Option<String>,
    will_retry: Option<bool>,
    /// Bytes of an incomplete trailing UTF-8 sequence carried between chunks — upstream's
    /// `TextDecoder(..., { stream: true })`.
    carry: Vec<u8>,
}

impl Default for PiAggregateProjection {
    fn default() -> Self {
        Self::new()
    }
}

impl PiAggregateProjection {
    /// A fresh projection over one (oversized) line.
    #[must_use]
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            root_ended: false,
            token: None,
            valid: true,
            event_type: None,
            will_retry: None,
            carry: Vec::new(),
        }
    }

    /// Feed the next raw chunk of the line. Returns `false` once the line can no longer be a valid
    /// JSON document (`push`, `child-protocol.ts:205-210`).
    pub fn push(&mut self, chunk: &[u8]) -> bool {
        if !self.valid {
            return false;
        }
        self.carry.extend_from_slice(chunk);
        let carry = std::mem::take(&mut self.carry);
        let (text, rest) = match std::str::from_utf8(&carry) {
            Ok(text) => (text, &[][..]),
            Err(err) => {
                if err.error_len().is_some() {
                    // A genuinely invalid sequence, not a split character — upstream's fatal
                    // decoder throws here and the projection is abandoned.
                    self.valid = false;
                    return false;
                }
                let head = carry.get(..err.valid_up_to()).unwrap_or_default();
                let tail = carry.get(err.valid_up_to()..).unwrap_or_default();
                (std::str::from_utf8(head).unwrap_or_default(), tail)
            }
        };
        self.valid = self.process_text(text);
        self.carry.extend_from_slice(rest);
        self.valid
    }

    /// Close the line and return the reduced record, or `None` when the line was not a complete,
    /// valid, projectable aggregate (`finish`, `child-protocol.ts:211-222`).
    #[must_use]
    pub fn finish(mut self) -> Option<String> {
        if !self.carry.is_empty() {
            // A truncated trailing UTF-8 sequence: upstream's non-streaming final `decode()`
            // throws on it.
            self.valid = false;
        }
        if let Some(Token::Number { phase }) = &self.token
            && phase.is_terminal()
        {
            self.token = None;
            self.complete_value(CompletedValue::Other);
        }
        if !self.valid || self.token.is_some() || !self.stack.is_empty() || !self.root_ended {
            return None;
        }
        match (self.event_type.as_deref(), self.will_retry) {
            (Some("turn_end"), _) => Some("{\"type\":\"turn_end\"}".to_string()),
            (Some("agent_end"), Some(will_retry)) => {
                Some(format!("{{\"type\":\"agent_end\",\"willRetry\":{will_retry}}}"))
            }
            _ => None,
        }
    }

    fn process_text(&mut self, text: &str) -> bool {
        for ch in text.chars() {
            if !self.process_char(ch) {
                return false;
            }
        }
        true
    }

    /// The key of the innermost container when it is an object (`parent()?.key`).
    fn parent_key(&self) -> Option<String> {
        match self.stack.last() {
            Some(Container::Object { key, .. }) => key.clone(),
            _ => None,
        }
    }

    /// `isTopLevelField` (`child-protocol.ts:40`).
    fn is_top_level_field(&self, key: Option<&str>) -> bool {
        self.stack.len() == 1
            && matches!(self.stack.last(), Some(Container::Object { .. }))
            && matches!(key, Some("type") | Some("willRetry"))
    }

    /// `completeValue` (`child-protocol.ts:42-54`).
    fn complete_value(&mut self, value: CompletedValue) {
        let depth = self.stack.len();
        let Some(container) = self.stack.last_mut() else {
            self.root_ended = true;
            return;
        };
        match container {
            Container::Object { state, key } => {
                if depth == 1 {
                    match (key.as_deref(), &value) {
                        (Some("type"), CompletedValue::Str(text)) => {
                            self.event_type = Some(text.clone());
                        }
                        (Some("willRetry"), CompletedValue::Bool(flag)) => {
                            self.will_retry = Some(*flag);
                        }
                        _ => {}
                    }
                }
                *key = None;
                *state = ObjectState::CommaOrEnd;
            }
            Container::Array(state) => *state = ArrayState::CommaOrEnd,
        }
    }

    /// `startValue` (`child-protocol.ts:56-80`).
    fn start_value(&mut self, ch: char) -> bool {
        let key = self.parent_key();
        if self.is_top_level_field(key.as_deref()) {
            if key.as_deref() == Some("type") {
                self.event_type = None;
            } else {
                self.will_retry = None;
            }
        }
        if ch == '{' || ch == '[' {
            if self.stack.len() >= MAX_PROJECTED_JSON_DEPTH {
                return false;
            }
            self.stack.push(if ch == '{' {
                Container::Object {
                    state: ObjectState::KeyOrEnd,
                    key: None,
                }
            } else {
                Container::Array(ArrayState::ValueOrEnd)
            });
            return true;
        }
        if ch == '"' {
            self.token = Some(Token::Str {
                role: StringRole::Value,
                value: String::new(),
                capture: key.as_deref() == Some("type") && self.stack.len() == 1,
                escape: false,
                unicode_digits: 0,
                unicode_value: String::new(),
            });
            return true;
        }
        self.token = Some(match ch {
            't' => Token::Literal {
                expected: "true",
                index: 1,
                value: Some(true),
            },
            'f' => Token::Literal {
                expected: "false",
                index: 1,
                value: Some(false),
            },
            'n' => Token::Literal {
                expected: "null",
                index: 1,
                value: None,
            },
            '-' => Token::Number {
                phase: NumberPhase::Minus,
            },
            '0' => Token::Number {
                phase: NumberPhase::Zero,
            },
            '1'..='9' => Token::Number {
                phase: NumberPhase::Int,
            },
            _ => return false,
        });
        true
    }

    /// `closeContainer` (`child-protocol.ts:82-86`).
    fn close_container(&mut self) -> bool {
        self.stack.pop();
        self.complete_value(CompletedValue::Other);
        true
    }

    /// `processChar` (`child-protocol.ts:88-197`).
    fn process_char(&mut self, ch: char) -> bool {
        match self.token.take() {
            Some(Token::Str {
                role,
                mut value,
                capture,
                mut escape,
                mut unicode_digits,
                mut unicode_value,
            }) => {
                if unicode_digits > 0 {
                    if !ch.is_ascii_hexdigit() {
                        return false;
                    }
                    unicode_value.push(ch);
                    unicode_digits -= 1;
                    if unicode_digits == 0 && capture {
                        if value.chars().count() >= 64 {
                            return false;
                        }
                        let code = u32::from_str_radix(&unicode_value, 16).unwrap_or(0);
                        value.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    self.token = Some(Token::Str {
                        role,
                        value,
                        capture,
                        escape,
                        unicode_digits,
                        unicode_value,
                    });
                    return true;
                }
                if escape {
                    escape = false;
                    if ch == 'u' {
                        self.token = Some(Token::Str {
                            role,
                            value,
                            capture,
                            escape,
                            unicode_digits: 4,
                            unicode_value: String::new(),
                        });
                        return true;
                    }
                    if !"\"\\/bfnrt".contains(ch) {
                        return false;
                    }
                    if capture {
                        if value.chars().count() >= 64 {
                            return false;
                        }
                        value.push(match ch {
                            'b' => '\u{8}',
                            'f' => '\u{c}',
                            'n' => '\n',
                            'r' => '\r',
                            't' => '\t',
                            other => other,
                        });
                    }
                    self.token = Some(Token::Str {
                        role,
                        value,
                        capture,
                        escape,
                        unicode_digits,
                        unicode_value,
                    });
                    return true;
                }
                if ch == '\\' {
                    self.token = Some(Token::Str {
                        role,
                        value,
                        capture,
                        escape: true,
                        unicode_digits,
                        unicode_value,
                    });
                    return true;
                }
                if ch == '"' {
                    if role == StringRole::Key {
                        let Some(Container::Object { state, key }) = self.stack.last_mut() else {
                            return false;
                        };
                        *key = Some(value);
                        *state = ObjectState::Colon;
                    } else if capture {
                        self.complete_value(CompletedValue::Str(value));
                    } else {
                        self.complete_value(CompletedValue::Other);
                    }
                    return true;
                }
                if (ch as u32) < 0x20 {
                    return false;
                }
                if capture {
                    if value.chars().count() >= 64 {
                        return false;
                    }
                    value.push(ch);
                }
                self.token = Some(Token::Str {
                    role,
                    value,
                    capture,
                    escape,
                    unicode_digits,
                    unicode_value,
                });
                true
            }
            Some(Token::Literal {
                expected,
                index,
                value,
            }) => {
                if expected.as_bytes().get(index).copied() != Some(ch as u8) || !ch.is_ascii() {
                    return false;
                }
                let index = index + 1;
                if index == expected.len() {
                    match value {
                        Some(flag) => self.complete_value(CompletedValue::Bool(flag)),
                        None => self.complete_value(CompletedValue::Other),
                    }
                } else {
                    self.token = Some(Token::Literal {
                        expected,
                        index,
                        value,
                    });
                }
                true
            }
            Some(Token::Number { phase }) => {
                let next = match phase {
                    NumberPhase::Minus => match ch {
                        '0' => Some(NumberPhase::Zero),
                        '1'..='9' => Some(NumberPhase::Int),
                        _ => return false,
                    },
                    NumberPhase::Zero | NumberPhase::Int => match ch {
                        '0'..='9' => {
                            if phase == NumberPhase::Zero {
                                return false;
                            }
                            Some(NumberPhase::Int)
                        }
                        '.' => Some(NumberPhase::Dot),
                        'e' | 'E' => Some(NumberPhase::Exp),
                        _ => None,
                    },
                    NumberPhase::Dot => match ch {
                        '0'..='9' => Some(NumberPhase::Frac),
                        _ => return false,
                    },
                    NumberPhase::Frac => match ch {
                        '0'..='9' => Some(NumberPhase::Frac),
                        'e' | 'E' => Some(NumberPhase::Exp),
                        _ => None,
                    },
                    NumberPhase::Exp => match ch {
                        '+' | '-' => Some(NumberPhase::ExpSign),
                        '0'..='9' => Some(NumberPhase::ExpDigits),
                        _ => return false,
                    },
                    NumberPhase::ExpSign => match ch {
                        '0'..='9' => Some(NumberPhase::ExpDigits),
                        _ => return false,
                    },
                    NumberPhase::ExpDigits => match ch {
                        '0'..='9' => Some(NumberPhase::ExpDigits),
                        _ => None,
                    },
                };
                match next {
                    Some(phase) => {
                        self.token = Some(Token::Number { phase });
                        true
                    }
                    None => {
                        // The number ended at this character; it must have ended in a legal phase,
                        // and the character itself is re-processed as structure
                        // (`child-protocol.ts:165-168`).
                        if !phase.is_terminal() {
                            return false;
                        }
                        self.complete_value(CompletedValue::Other);
                        self.process_char(ch)
                    }
                }
            }
            None => {
                if ch == ' ' || ch == '\t' || ch == '\r' || ch == '\n' {
                    return true;
                }
                let Some(container) = self.stack.last() else {
                    return if self.root_ended {
                        false
                    } else {
                        self.start_value(ch)
                    };
                };
                match container {
                    Container::Object { state, .. } => {
                        let state = *state;
                        if state == ObjectState::KeyOrEnd || state == ObjectState::Key {
                            if ch == '}' && state == ObjectState::KeyOrEnd {
                                return self.close_container();
                            }
                            if ch != '"' {
                                return false;
                            }
                            self.token = Some(Token::Str {
                                role: StringRole::Key,
                                value: String::new(),
                                capture: self.stack.len() == 1,
                                escape: false,
                                unicode_digits: 0,
                                unicode_value: String::new(),
                            });
                            return true;
                        }
                        if state == ObjectState::Colon {
                            if ch != ':' {
                                return false;
                            }
                            if let Some(Container::Object { state, .. }) = self.stack.last_mut() {
                                *state = ObjectState::Value;
                            }
                            return true;
                        }
                        if state == ObjectState::Value {
                            return self.start_value(ch);
                        }
                        if ch == ',' {
                            if let Some(Container::Object { state, .. }) = self.stack.last_mut() {
                                *state = ObjectState::Key;
                            }
                            return true;
                        }
                        if ch == '}' {
                            return self.close_container();
                        }
                        false
                    }
                    Container::Array(state) => {
                        let state = *state;
                        if state == ArrayState::ValueOrEnd || state == ArrayState::Value {
                            if ch == ']' && state == ArrayState::ValueOrEnd {
                                return self.close_container();
                            }
                            return self.start_value(ch);
                        }
                        if ch == ',' {
                            if let Some(Container::Array(state)) = self.stack.last_mut() {
                                *state = ArrayState::Value;
                            }
                            return true;
                        }
                        if ch == ']' {
                            return self.close_container();
                        }
                        false
                    }
                }
            }
        }
    }
}

/// Does an oversized line's head look like one of the two redundant aggregate records the
/// projection can replace (`PI_AGGREGATE_EVENT_PROJECTOR.accepts`, `child-protocol.ts:234-236`)?
#[must_use]
pub fn pi_aggregate_projector_accepts(prefix: &str) -> bool {
    prefix.starts_with("{\"type\":\"turn_end\"") || prefix.starts_with("{\"type\":\"agent_end\"")
}

/// The projector upstream installs on the STDOUT reader only (`execution.ts:1043`), named for
/// symmetry with the upstream constant it ports.
pub const PI_AGGREGATE_EVENT_PROJECTOR: bool = true;

// ------------------------------------------------------------------------------------------------
// createBoundedLineReader (child-protocol.ts:244-368)
// ------------------------------------------------------------------------------------------------

/// A line splitter with a hard cap on how many bytes of a SINGLE line it will accumulate
/// (`createBoundedLineReader`, `child-protocol.ts:244-368`).
///
/// Lines and the (at most one) limit are queued rather than delivered by callback — see this
/// module's `[CYRUP-DELTA]` note. Once the limit trips, the reader is permanently closed: every
/// later `push` is a no-op, exactly like upstream's `if (limitExceeded) return`.
#[derive(Debug)]
pub struct BoundedLineReader {
    stream: ProtocolStream,
    max_pending_line_bytes: usize,
    projector_enabled: bool,
    pending: Vec<u8>,
    projected_prefix: Vec<u8>,
    projected_tail: Vec<u8>,
    projected_bytes: usize,
    projection: Option<PiAggregateProjection>,
    projecting: bool,
    limit_exceeded: bool,
    lines: VecDeque<String>,
    limit: Option<ProtocolOutputLimit>,
}

impl BoundedLineReader {
    /// A stdout reader with the 16 MiB cap and the aggregate projector installed — exactly
    /// upstream's stdout wiring (`execution.ts:1042-1046`).
    #[must_use]
    pub fn stdout() -> Self {
        Self::new(
            ProtocolStream::Stdout,
            MAX_CHILD_PENDING_LINE_BYTES,
            PI_AGGREGATE_EVENT_PROJECTOR,
        )
    }

    /// A stderr reader with the 128 KiB cap and NO projector — upstream's stderr wiring
    /// (`execution.ts:1047-1052`); an oversized stderr line is a diagnostic problem, never a
    /// protocol record worth reconstructing.
    #[must_use]
    pub fn stderr() -> Self {
        Self::new(ProtocolStream::Stderr, MAX_CHILD_STDERR_BYTES, false)
    }

    /// An explicitly configured reader (the cap is clamped to at least 1 byte, mirroring
    /// upstream's positive-integer precondition without making construction fallible).
    #[must_use]
    pub fn new(
        stream: ProtocolStream,
        max_pending_line_bytes: usize,
        projector_enabled: bool,
    ) -> Self {
        Self {
            stream,
            max_pending_line_bytes: max_pending_line_bytes.max(1),
            projector_enabled,
            pending: Vec::new(),
            projected_prefix: Vec::new(),
            projected_tail: Vec::new(),
            projected_bytes: 0,
            projection: None,
            projecting: false,
            limit_exceeded: false,
            lines: VecDeque::new(),
            limit: None,
        }
    }

    /// Whether the cap has been tripped (`exceeded()`, `child-protocol.ts:366`).
    #[must_use]
    pub fn exceeded(&self) -> bool {
        self.limit_exceeded
    }

    /// Pop the next completed line, if any.
    pub fn take_line(&mut self) -> Option<String> {
        self.lines.pop_front()
    }

    /// Pop the limit diagnostic, if the cap tripped and it has not been reported yet.
    pub fn take_limit(&mut self) -> Option<ProtocolOutputLimit> {
        self.limit.take()
    }

    /// Feed the next raw chunk (`push`, `child-protocol.ts:350-362`).
    pub fn push(&mut self, chunk: &[u8]) {
        if self.limit_exceeded {
            return;
        }
        let mut start = 0usize;
        for (index, byte) in chunk.iter().enumerate() {
            if *byte != b'\n' {
                continue;
            }
            if !self.append(chunk.get(start..index).unwrap_or_default()) {
                return;
            }
            self.finish_line();
            if self.limit_exceeded {
                return;
            }
            start = index + 1;
        }
        self.append(chunk.get(start..).unwrap_or_default());
    }

    /// Stream end: flush a final line with no trailing newline (`end`,
    /// `child-protocol.ts:363-365`).
    pub fn end(&mut self) {
        if !self.limit_exceeded {
            self.finish_line();
        }
    }

    /// `diagnosticTail` (`child-protocol.ts:268-273`).
    fn diagnostic_tail(prior: &[u8], segment: &[u8]) -> Vec<u8> {
        let from_segment = segment
            .get(segment.len().saturating_sub(MAX_PROTOCOL_DIAGNOSTIC_BYTES)..)
            .unwrap_or_default();
        if from_segment.len() == MAX_PROTOCOL_DIAGNOSTIC_BYTES {
            return from_segment.to_vec();
        }
        let want = MAX_PROTOCOL_DIAGNOSTIC_BYTES - from_segment.len();
        let mut tail = prior
            .get(prior.len().saturating_sub(want)..)
            .unwrap_or_default()
            .to_vec();
        tail.extend_from_slice(from_segment);
        tail
    }

    /// `failLimit` (`child-protocol.ts:275-293`): drop everything buffered, publish the
    /// diagnostic, and close the reader for good.
    fn fail_limit(&mut self, observed_bytes: usize, prefix: &[u8], tail: &[u8]) -> bool {
        self.limit_exceeded = true;
        self.pending.clear();
        self.projecting = false;
        self.projection = None;
        self.projected_prefix.clear();
        self.projected_tail.clear();
        self.projected_bytes = 0;
        self.limit = Some(ProtocolOutputLimit {
            code: PROTOCOL_OUTPUT_LIMIT_CODE.to_string(),
            stream: self.stream,
            limit_bytes: self.max_pending_line_bytes,
            observed_bytes,
            diagnostic_prefix: String::from_utf8_lossy(prefix).into_owned(),
            diagnostic_tail: String::from_utf8_lossy(tail).into_owned(),
        });
        false
    }

    /// `finishLine` (`child-protocol.ts:295-313`).
    fn finish_line(&mut self) {
        if self.projecting {
            match self.projection.take().and_then(PiAggregateProjection::finish) {
                Some(projected) => self.lines.push_back(projected),
                None => {
                    let observed = self.projected_bytes;
                    let prefix = std::mem::take(&mut self.projected_prefix);
                    let tail = std::mem::take(&mut self.projected_tail);
                    self.fail_limit(observed, &prefix, &tail);
                }
            }
        } else if !self.pending.is_empty() {
            self.lines.push_back(decode_line(&self.pending));
        }
        self.pending.clear();
        self.projecting = false;
        self.projection = None;
        self.projected_prefix.clear();
        self.projected_tail.clear();
        self.projected_bytes = 0;
    }

    /// `append` (`child-protocol.ts:315-347`).
    fn append(&mut self, segment: &[u8]) -> bool {
        if segment.is_empty() {
            return true;
        }
        if self.projecting {
            self.projected_bytes += segment.len();
            let tail = Self::diagnostic_tail(&self.projected_tail, segment);
            self.projected_tail = tail;
            let accepted = self
                .projection
                .as_mut()
                .is_some_and(|projection| projection.push(segment));
            if accepted {
                return true;
            }
            let observed = self.projected_bytes;
            let prefix = std::mem::take(&mut self.projected_prefix);
            let tail = std::mem::take(&mut self.projected_tail);
            return self.fail_limit(observed, &prefix, &tail);
        }
        let observed_bytes = self.pending.len() + segment.len();
        if observed_bytes > self.max_pending_line_bytes {
            let prior = std::mem::take(&mut self.pending);
            let mut prefix = prior
                .get(..MAX_PROTOCOL_DIAGNOSTIC_BYTES.min(prior.len()))
                .unwrap_or_default()
                .to_vec();
            if prefix.len() < MAX_PROTOCOL_DIAGNOSTIC_BYTES {
                let want = MAX_PROTOCOL_DIAGNOSTIC_BYTES - prefix.len();
                prefix.extend_from_slice(segment.get(..want.min(segment.len())).unwrap_or_default());
            }
            let tail = Self::diagnostic_tail(&prior, segment);
            if self.projector_enabled
                && pi_aggregate_projector_accepts(&String::from_utf8_lossy(&prefix))
            {
                let mut candidate = PiAggregateProjection::new();
                if !candidate.push(&prior) || !candidate.push(segment) {
                    return self.fail_limit(observed_bytes, &prefix, &tail);
                }
                self.projecting = true;
                self.projection = Some(candidate);
                self.projected_prefix = prefix;
                self.projected_tail = tail;
                self.projected_bytes = observed_bytes;
                return true;
            }
            return self.fail_limit(observed_bytes, &prefix, &tail);
        }
        self.pending.extend_from_slice(segment);
        true
    }
}

/// One line's bytes as text: malformed sequences replaced (Node's `Buffer.toString("utf8")`), and
/// a single trailing `\r` stripped so a CRLF child's `.jsonl` artifact keeps the bytes cyrup's
/// previous `tokio::io::Lines`-based reader produced.
fn decode_line(bytes: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(bytes).into_owned();
    if text.ends_with('\r') {
        text.pop();
    }
    text
}

// ------------------------------------------------------------------------------------------------
// The async source the spawn boundary actually reads through
// ------------------------------------------------------------------------------------------------

/// One step of [`BoundedLineStream::next`].
#[derive(Debug)]
pub enum BoundedRead {
    /// A complete line (or a projected replacement for an oversized aggregate record).
    Line(String),
    /// The per-line cap tripped. The stream is finished — no further lines will ever be produced.
    Limit(ProtocolOutputLimit),
    /// The underlying reader reached EOF and every buffered line has been delivered.
    Eof,
    /// A genuine I/O fault reading the byte stream itself.
    Err(std::io::Error),
}

/// A [`BoundedLineReader`] driven over an async byte source — what [`crate::spawn::SpawnedChild`]
/// reads a child's stdout through, replacing the unbounded `tokio::io::Lines`.
///
/// # Cancellation safety
///
/// [`BoundedLineStream::next`] is cancellation-safe and MUST stay so: the production read loop
/// races it inside a `tokio::select!` against cancel/interrupt/deadline/child-exit arms, so a
/// cancelled poll must not lose bytes. It cannot: completed lines live in the reader's own queue,
/// partial bytes live in the reader's `pending` buffer, and the only await point is
/// `AsyncReadExt::read`, which is itself cancellation-safe (a cancelled read has read nothing).
#[derive(Debug)]
pub struct BoundedLineStream<R> {
    reader: R,
    buf: Vec<u8>,
    lines: BoundedLineReader,
    eof: bool,
}

/// Chunk size for one `read` — the same order of magnitude as a pipe buffer, so an ordinary
/// NDJSON line arrives in one or two reads.
const READ_CHUNK_BYTES: usize = 64 * 1024;

impl<R: AsyncRead + Unpin> BoundedLineStream<R> {
    /// Wrap `reader` with the STDOUT bounding policy (16 MiB cap + aggregate projector).
    #[must_use]
    pub fn stdout(reader: R) -> Self {
        Self::with_reader(reader, BoundedLineReader::stdout())
    }

    /// Wrap `reader` with the STDERR bounding policy (128 KiB cap, no projector).
    #[must_use]
    pub fn stderr(reader: R) -> Self {
        Self::with_reader(reader, BoundedLineReader::stderr())
    }

    /// Wrap `reader` with an explicitly configured [`BoundedLineReader`].
    #[must_use]
    pub fn with_reader(reader: R, lines: BoundedLineReader) -> Self {
        Self {
            reader,
            buf: vec![0u8; READ_CHUNK_BYTES],
            lines,
            eof: false,
        }
    }

    /// Whether the cap has tripped on this stream.
    #[must_use]
    pub fn exceeded(&self) -> bool {
        self.lines.exceeded()
    }

    /// The next line, limit, EOF, or I/O error. See the type's cancellation-safety note.
    pub async fn next(&mut self) -> BoundedRead {
        loop {
            if let Some(line) = self.lines.take_line() {
                return BoundedRead::Line(line);
            }
            if let Some(limit) = self.lines.take_limit() {
                return BoundedRead::Limit(limit);
            }
            if self.lines.exceeded() || self.eof {
                return BoundedRead::Eof;
            }
            let Self {
                reader,
                buf,
                lines,
                eof,
            } = self;
            let read = match reader.read(buf).await {
                Ok(read) => read,
                Err(err) => return BoundedRead::Err(err),
            };
            if read == 0 {
                *eof = true;
                lines.end();
                continue;
            }
            lines.push(buf.get(..read).unwrap_or_default());
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use super::*;

    fn drain(reader: &mut BoundedLineReader) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(line) = reader.take_line() {
            out.push(line);
        }
        out
    }

    #[test]
    fn splits_lines_and_flushes_a_final_unterminated_line() {
        let mut reader = BoundedLineReader::stdout();
        reader.push(b"{\"type\":\"agent_start\"}\nnot json\n{\"type\":\"agent_end\"}");
        assert_eq!(
            drain(&mut reader),
            vec![
                "{\"type\":\"agent_start\"}".to_string(),
                "not json".to_string()
            ]
        );
        reader.end();
        assert_eq!(drain(&mut reader), vec!["{\"type\":\"agent_end\"}".to_string()]);
        assert!(reader.take_limit().is_none());
    }

    #[test]
    fn reassembles_a_line_split_across_chunks() {
        let mut reader = BoundedLineReader::stdout();
        for chunk in [&b"{\"type\":\"ag"[..], b"ent_start\"}", b"\n"] {
            reader.push(chunk);
        }
        assert_eq!(drain(&mut reader), vec!["{\"type\":\"agent_start\"}".to_string()]);
    }

    /// THE cap. A single line one byte over the limit must produce a `protocol_output_limit`
    /// rather than an ever-growing buffer.
    #[test]
    fn a_line_over_the_cap_trips_the_limit_and_closes_the_reader() {
        let mut reader = BoundedLineReader::new(ProtocolStream::Stdout, 64, false);
        reader.push(&[b'x'; 65]);
        assert!(reader.exceeded(), "the cap must trip at 65 > 64 bytes");
        let limit = reader.take_limit().expect("a diagnostic must be published");
        assert_eq!(limit.code, PROTOCOL_OUTPUT_LIMIT_CODE);
        assert_eq!(limit.stream, ProtocolStream::Stdout);
        assert_eq!(limit.limit_bytes, 64);
        assert_eq!(limit.observed_bytes, 65);
        assert_eq!(limit.diagnostic_prefix, "x".repeat(65));
        assert_eq!(
            format_protocol_output_limit(&limit),
            "protocol_output_limit: child stdout line exceeded 64 bytes (observed at least 65 \
             bytes without a newline)."
        );
        // Permanently closed: a later, perfectly ordinary line is NOT delivered.
        reader.push(b"{\"type\":\"agent_end\"}\n");
        assert!(drain(&mut reader).is_empty());
    }

    #[test]
    fn a_line_exactly_at_the_cap_is_delivered() {
        let mut reader = BoundedLineReader::new(ProtocolStream::Stdout, 64, false);
        reader.push(&[b'x'; 64]);
        reader.push(b"\n");
        assert!(!reader.exceeded());
        assert_eq!(drain(&mut reader), vec!["x".repeat(64)]);
    }

    #[test]
    fn stderr_reader_uses_the_stderr_cap_and_labels_the_stream() {
        let mut reader = BoundedLineReader::stderr();
        reader.push(&vec![b'e'; MAX_CHILD_STDERR_BYTES + 1]);
        let limit = reader.take_limit().expect("stderr trips its own cap");
        assert_eq!(limit.stream, ProtocolStream::Stderr);
        assert_eq!(limit.limit_bytes, MAX_CHILD_STDERR_BYTES);
    }

    // ---- the oversized-aggregate projection ----

    #[test]
    fn an_oversized_but_valid_turn_end_is_projected_instead_of_failing() {
        let mut reader = BoundedLineReader::new(ProtocolStream::Stdout, 64, true);
        let filler = "y".repeat(500);
        let line = format!("{{\"type\":\"turn_end\",\"message\":{{\"text\":\"{filler}\"}}}}");
        assert!(line.len() > 64);
        reader.push(line.as_bytes());
        reader.push(b"\n");
        assert!(!reader.exceeded(), "a projectable aggregate must not fail the run");
        assert_eq!(drain(&mut reader), vec!["{\"type\":\"turn_end\"}".to_string()]);
    }

    #[test]
    fn an_oversized_agent_end_keeps_will_retry() {
        for will_retry in [true, false] {
            let mut reader = BoundedLineReader::new(ProtocolStream::Stdout, 64, true);
            let filler = "z".repeat(500);
            let line = format!(
                "{{\"type\":\"agent_end\",\"willRetry\":{will_retry},\"messages\":[\"{filler}\"]}}"
            );
            reader.push(line.as_bytes());
            reader.push(b"\n");
            assert_eq!(
                drain(&mut reader),
                vec![format!("{{\"type\":\"agent_end\",\"willRetry\":{will_retry}}}")]
            );
        }
    }

    #[test]
    fn an_oversized_aggregate_that_is_malformed_still_fails() {
        let mut reader = BoundedLineReader::new(ProtocolStream::Stdout, 64, true);
        let filler = "y".repeat(500);
        // Missing the closing brace: syntactically invalid, so the projection must not rescue it.
        let line = format!("{{\"type\":\"turn_end\",\"message\":{{\"text\":\"{filler}\"}}");
        reader.push(line.as_bytes());
        reader.push(b"\n");
        assert!(reader.exceeded());
        assert!(reader.take_limit().is_some());
    }

    #[test]
    fn an_oversized_non_aggregate_line_is_not_projected() {
        let mut reader = BoundedLineReader::new(ProtocolStream::Stdout, 64, true);
        let filler = "y".repeat(500);
        let line = format!("{{\"type\":\"message_end\",\"message\":{{\"text\":\"{filler}\"}}}}");
        reader.push(line.as_bytes());
        reader.push(b"\n");
        assert!(
            reader.exceeded(),
            "only turn_end/agent_end are redundant enough to reduce"
        );
    }

    #[test]
    fn the_projector_is_off_for_stderr() {
        let mut reader = BoundedLineReader::new(ProtocolStream::Stderr, 64, false);
        let filler = "y".repeat(500);
        reader.push(format!("{{\"type\":\"turn_end\",\"m\":\"{filler}\"}}").as_bytes());
        reader.push(b"\n");
        assert!(reader.exceeded());
    }

    #[test]
    fn projection_validates_the_whole_json_grammar() {
        let ok = [
            r#"{"type":"turn_end","a":[1,2.5,-3e-2,true,false,null],"b":{"c":{}},"d":[]}"#,
            r#"{"type":"turn_end","s":"esc \" \\ \/ \b \f \n \r \t A"}"#,
            r#"  {"type":"turn_end"}  "#,
        ];
        for line in ok {
            let mut projection = PiAggregateProjection::new();
            assert!(projection.push(line.as_bytes()), "{line} must stay valid");
            assert_eq!(
                projection.finish(),
                Some("{\"type\":\"turn_end\"}".to_string()),
                "{line} must project"
            );
        }
        let bad = [
            r#"{"type":"turn_end",}"#,
            r#"{"type":"turn_end""#,
            r#"{"type":"turn_end"} trailing"#,
            r#"{"type":"turn_end","n":01}"#,
            r#"{"type":"turn_end","n":1.}"#,
            r#"{"type":"turn_end","s":"\x"}"#,
            r#"{"type":"turn_end" "x":1}"#,
            r#"[{"type":"turn_end"}]"#,
        ];
        for line in bad {
            let mut projection = PiAggregateProjection::new();
            let pushed = projection.push(line.as_bytes());
            let finished = if pushed { projection.finish() } else { None };
            assert_eq!(finished, None, "{line} must NOT project");
        }
    }

    #[test]
    fn projection_survives_chunk_boundaries_inside_a_multibyte_character() {
        let line = "{\"type\":\"turn_end\",\"s\":\"é\"}";
        let bytes = line.as_bytes();
        for split in 1..bytes.len() {
            let mut projection = PiAggregateProjection::new();
            assert!(projection.push(&bytes[..split]));
            assert!(projection.push(&bytes[split..]));
            assert_eq!(
                projection.finish(),
                Some("{\"type\":\"turn_end\"}".to_string()),
                "split at {split} must still project"
            );
        }
    }

    #[test]
    fn projection_rejects_over_deep_nesting() {
        let mut projection = PiAggregateProjection::new();
        let mut line = String::from("{\"type\":\"turn_end\",\"d\":");
        line.push_str(&"[".repeat(MAX_PROJECTED_JSON_DEPTH + 4));
        assert!(!projection.push(line.as_bytes()));
    }

    // ---- bounded byte tail ----

    #[test]
    fn byte_tail_keeps_only_the_last_bytes() {
        let mut tail = BoundedByteTail::new(8);
        tail.push(b"abcdefghijkl");
        assert_eq!(tail.text(), "efghijkl");
        assert_eq!(tail.byte_length(), 8);
    }

    #[test]
    fn byte_tail_trims_to_a_character_boundary() {
        let mut tail = BoundedByteTail::new(4);
        tail.push("aé€".as_bytes()); // 1 + 2 + 3 = 6 bytes
        assert_eq!(tail.text(), "€", "a split multi-byte head is dropped whole");
    }

    // ---- lifecycle projection ----

    #[test]
    fn lifecycle_projection_matches_upstream_table() {
        assert_eq!(
            project_child_lifecycle("agent_end", true, false),
            ChildLifecycleAction::CancelDrain
        );
        assert_eq!(
            project_child_lifecycle("agent_end", false, false),
            ChildLifecycleAction::None
        );
        assert_eq!(
            project_child_lifecycle("agent_settled", false, false),
            ChildLifecycleAction::StartDrain
        );
        assert_eq!(
            project_child_lifecycle("message_end", false, true),
            ChildLifecycleAction::StartDrain
        );
        assert_eq!(
            project_child_lifecycle("message_end", false, false),
            ChildLifecycleAction::None
        );
        // will_retry wins even when the same event would otherwise be a terminal stop.
        assert_eq!(
            project_child_lifecycle("agent_end", true, true),
            ChildLifecycleAction::CancelDrain
        );
    }

    // ---- the async stream ----

    #[tokio::test]
    async fn stream_yields_lines_then_eof() {
        let input = b"{\"type\":\"agent_start\"}\n{\"type\":\"agent_end\"}".to_vec();
        let mut stream = BoundedLineStream::stdout(std::io::Cursor::new(input));
        let mut lines = Vec::new();
        loop {
            match stream.next().await {
                BoundedRead::Line(line) => lines.push(line),
                BoundedRead::Eof => break,
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(
            lines,
            vec![
                "{\"type\":\"agent_start\"}".to_string(),
                "{\"type\":\"agent_end\"}".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn stream_surfaces_the_limit_then_stops() {
        let mut input = vec![b'x'; MAX_CHILD_STDERR_BYTES + 10];
        input.extend_from_slice(b"\n{\"type\":\"agent_end\"}\n");
        let mut stream = BoundedLineStream::with_reader(
            std::io::Cursor::new(input),
            BoundedLineReader::stderr(),
        );
        assert!(matches!(stream.next().await, BoundedRead::Limit(_)));
        assert!(matches!(stream.next().await, BoundedRead::Eof));
    }
}
