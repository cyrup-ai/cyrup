//! The **Program Status Protocol** (OSC 7501) — a program telling the terminal whether it is idle,
//! working, blocked on the user, done or failed, so a terminal that supports the protocol can show
//! it in a tab title, a taskbar entry or a notification (TUI-171).
//!
//! Spec: <https://www.superlogical.com/rex/docs/build/program-status>
//!
//! This is the byte layer plus the process-global emitter, ported 1:1 from
//! `packages/tui/src/program-status.ts` (53 lines, new in pi `503c60552` / #10607) and the
//! `programStatus*` members of `packages/tui/src/terminal.ts`. The interactive-mode half — WHEN
//! each state is reported — is [`crate::program_status_reporter`].
//!
//! ## Why the support bit and the last status are process-global
//!
//! Same argument [`crate::terminal_progress`] makes for `PROGRESS_ARMED`, and for the same reason
//! it is the one that matters: the `state=clear` write on exit has to work from
//! [`crate::panic_hook::restore_terminal_best_effort`], which runs from a `std::panic` hook with no
//! `&App` to consult, and under the release profile's `panic = "abort"` is the only code that runs
//! at all. **A status left set after cyrup quits is worse than never setting one** — the user's
//! terminal keeps claiming a dead process is working.
//!
//! The remembered status is kept across a clear-on-stop deliberately: pi's `setProgramStatus`
//! stores it whether or not the terminal supports the protocol (`terminal.ts:576-579`), and
//! `writeProgramStatus` re-sends it once support is (re-)confirmed (`:581-585`) — which is what
//! makes the Ctrl+Z suspend / resume round trip restore the status instead of losing it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use base64::Engine as _;

/// Feature-detection query. A supporting terminal replies with the same body
/// (`program-status.ts:20`).
pub const PROGRAM_STATUS_QUERY: &str = "\x1b]7501;?\x1b\\";

/// The `state=clear` report, as a constant because the exit and crash paths write exactly this and
/// nothing else.
pub const PROGRAM_STATUS_CLEAR: &str = "\x1b]7501;state=clear\x1b\\";

/// The environment override, cyrup's spelling of pi's `PI_PROGRAM_STATUS` (`terminal.ts:289`).
/// `1` forces support on and SKIPS the query; `0` forces it off and skips the query; anything else
/// (including unset) queries. cyrup renames every `PI_*` terminal override one-for-one —
/// `CYRUP_HYPERLINKS`, `CYRUP_TRUE_COLOR`, `CYRUP_IMAGE_PROTOCOL`, `CYRUP_HARDWARE_CURSOR`,
/// `CYRUP_CLEAR_ON_SHRINK`, `CYRUP_TUI_ESC_TIMEOUT` — so this is the matching name.
pub const PROGRAM_STATUS_ENV: &str = "CYRUP_PROGRAM_STATUS";

/// Decoded `msg` limit (`program-status.ts:30`). Its base64 encoding stays under the protocol's
/// 2732-byte encoded limit.
const MAX_MESSAGE_BYTES: usize = 2048;

/// What a program is doing (`program-status.ts:10`). [`ProgramState::Clear`] removes the status
/// instead of reporting one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ProgramState {
    /// Nothing running.
    #[default]
    Idle,
    /// A run or a compaction is in progress.
    Working,
    /// Waiting on the user (see [`BlockedKind`]).
    Blocked,
    /// The last run finished cleanly.
    Done,
    /// The last run failed.
    Error,
    /// Remove the status.
    Clear,
}

impl ProgramState {
    /// The `state=` token (`program-status.ts:10` spells the wire values).
    pub fn as_str(self) -> &'static str {
        match self {
            ProgramState::Idle => "idle",
            ProgramState::Working => "working",
            ProgramState::Blocked => "blocked",
            ProgramState::Done => "done",
            ProgramState::Error => "error",
            ProgramState::Clear => "clear",
        }
    }
}

/// What a blocked program waits for (`program-status.ts:14`). Omitted for every other state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockedKind {
    /// A permission prompt.
    Permission,
    /// A question or a selection dialog.
    Question,
    /// A login / auth flow.
    Auth,
}

impl BlockedKind {
    /// The `kind=` token.
    pub fn as_str(self) -> &'static str {
        match self {
            BlockedKind::Permission => "permission",
            BlockedKind::Question => "question",
            BlockedKind::Auth => "auth",
        }
    }
}

/// One status report (`ProgramStatus`, `program-status.ts:8-17`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProgramStatus {
    /// The state. The only field that is always emitted.
    pub state: ProgramState,
    /// Stable program name, `[A-Za-z0-9_.+-]{1,32}`. Other values are **omitted**, not rejected.
    pub app: Option<String>,
    /// Only emitted when `state == Blocked` and this is set.
    pub kind: Option<BlockedKind>,
    /// One human-readable line. Control characters become spaces; longer text is cut to the spec
    /// limit.
    pub message: Option<String>,
}

impl ProgramStatus {
    /// A bare state with no app, kind or message.
    pub fn new(state: ProgramState) -> Self {
        Self {
            state,
            ..Self::default()
        }
    }

    /// A state carrying a message.
    pub fn with_message(state: ProgramState, message: impl Into<String>) -> Self {
        Self {
            state,
            app: None,
            kind: None,
            message: Some(message.into()),
        }
    }
}

/// `isProgramStatusReply(sequence)` — `program-status.ts:23-25`, whose regex is
/// `/^\x1b\]7501;\?[^\x07\x1b]*(?:\x07|\x1b\\)$/`.
///
/// It matches the **echo of the query**, with either string terminator, and tolerates `key=value`
/// pairs a later spec revision may add after the `?`. A `state=` report is NOT a reply, and neither
/// is any other OSC. Hand-written rather than pulled in as a regex: the three clauses are the
/// prefix, the terminator, and "no `BEL` or `ESC` in between" — which is what makes the character
/// class exclude the terminators it would otherwise swallow.
pub fn is_program_status_reply(sequence: &str) -> bool {
    const PREFIX: &str = "\x1b]7501;?";
    let Some(rest) = sequence.strip_prefix(PREFIX) else {
        return false;
    };
    let body = if let Some(body) = rest.strip_suffix("\x1b\\") {
        body
    } else if let Some(body) = rest.strip_suffix('\x07') {
        body
    } else {
        return false;
    };
    !body.contains('\x07') && !body.contains('\x1b')
}

/// Whether `forwarded` — the concatenation of every sequence the terminal sent before its DA1
/// sentinel ([`crate::terminal_query::exchange_with`]) — contains the query echo.
///
/// Pi tests each framed sequence individually (`terminal.ts:239`), against the same regex
/// [`is_program_status_reply`] ports. Scanning the concatenation is **equivalent, not approximate**,
/// and the reason is in the regex's own character class: the body between `?` and the terminator
/// admits no `BEL` and no `ESC` (`program-status.ts:24`), so the first of either after the prefix
/// IS the frame's terminator and the frame is delimited unambiguously wherever it sits.
///
/// Pi additionally requires the reply to arrive BEFORE the DA1 sentinel (`terminal.ts:305-313`: the
/// last owed DA1 closes the window, and a late reply sets nothing). cyrup needs no counter for
/// that: `exchange_with` breaks its read loop ON its own sentinel, so **everything in `forwarded`
/// arrived strictly before DA1** by construction, and a late reply never reaches here at all — once
/// the reader thread exists, `crate::input::decode` frames the OSC and swallows it as
/// `Decoded::Reply`.
pub fn contains_program_status_reply(forwarded: &str) -> bool {
    const PREFIX: &str = "\x1b]7501;?";
    let mut rest = forwarded;
    while let Some(at) = rest.find(PREFIX) {
        let frame = rest.get(at..).unwrap_or("");
        let body = frame.get(PREFIX.len()..).unwrap_or("");
        // The body admits neither terminator, so the first `BEL` or `ESC` after the prefix ends the
        // frame. Cut there and hand the frame to the single-frame test, so the two cannot disagree.
        let end = match body.find(['\x07', '\x1b']) {
            Some(i) if body.get(i..i + 1) == Some("\x07") => PREFIX.len() + i + 1,
            Some(i) => PREFIX.len() + i + 2,
            None => frame.len(),
        };
        if frame.get(..end).is_some_and(is_program_status_reply) {
            return true;
        }
        rest = body;
    }
    false
}

/// Pi's `PI_PROGRAM_STATUS` tri-state (`terminal.ts:289-291`), as cyrup's
/// [`PROGRAM_STATUS_ENV`]:
///
/// ```ts
/// const programStatusOverride = process.env.PI_PROGRAM_STATUS;
/// this.programStatusSupported = programStatusOverride === "1";
/// this.programStatusQueryPending = programStatusOverride !== "1" && programStatusOverride !== "0";
/// ```
///
/// So `1` and `0` BOTH skip the query; only `1` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgramStatusOverride {
    /// `CYRUP_PROGRAM_STATUS=1` — supported, and the query is not sent.
    ForceOn,
    /// `CYRUP_PROGRAM_STATUS=0` — unsupported, and the query is not sent.
    ForceOff,
    /// Anything else, including unset — send the query and believe the answer.
    Query,
}

/// [`ProgramStatusOverride`] from the raw variable value. Taken as a parameter rather than read
/// here so a test can drive all three without `std::env::set_var`, which this crate's
/// `#![forbid(unsafe_code)]` rules out in `src/` (the convention
/// `crate::app::resolve_escape_timeout` already follows for `CYRUP_TUI_ESC_TIMEOUT`).
pub fn resolve_program_status_override(value: Option<&str>) -> ProgramStatusOverride {
    match value {
        Some("1") => ProgramStatusOverride::ForceOn,
        Some("0") => ProgramStatusOverride::ForceOff,
        _ => ProgramStatusOverride::Query,
    }
}

/// `APP_PATTERN.test(app)` — `program-status.ts:27`, `/^[A-Za-z0-9_.+-]{1,32}$/`.
fn app_is_valid(app: &str) -> bool {
    let len = app.chars().count();
    (1..=32).contains(&len)
        && app
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '+' | '-'))
}

/// `text.replace(CONTROL_CHARACTERS, " ")` — `program-status.ts:28`,
/// `/[\u0000-\u001f\u007f-\u009f]+/g`.
///
/// **A RUN of control characters collapses to ONE space**, which is the `+` in the pattern and the
/// detail a per-character replacement gets wrong: `"a\x1b[31mb"` is `ESC` (a control) followed by
/// the literal `[31m`, so it becomes `"a [31mb"` — one space, not one per control byte.
fn collapse_control_runs(text: &str) -> String {
    let is_ctl = |c: char| matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}');
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if is_ctl(c) {
            if !in_run {
                out.push(' ');
                in_run = true;
            }
        } else {
            out.push(c);
            in_run = false;
        }
    }
    out
}

/// `truncateUtf8(text, maxBytes)` — `program-status.ts:32-43`.
///
/// Cuts on a whole **code point**, as upstream's `for (const char of text)` does (a `String`
/// iterator yields code points, not grapheme clusters), so the result is always valid UTF-8 but a
/// combining mark may be separated from its base. Kept faithful rather than improved: the limit is
/// a byte budget the terminal enforces, and widening the unit here would let a report exceed it.
fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = 0usize;
    for (i, c) in text.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    text.get(..end).unwrap_or("")
}

/// `formatProgramStatus(status)` — `program-status.ts:46-53`.
///
/// Emits `ESC ] 7501 ; <pairs joined by ':'> ESC \`, with the pairs built in this exact order:
/// `state=` always; `app=` only when set AND matching [`app_is_valid`]; `kind=` only when
/// `state == Blocked` and a kind is set; `msg=<base64>` only when the processed message is
/// non-empty. "Processed" is control-run collapse → trim → [`truncate_utf8`] → base64, in that
/// order, because terminals discard reports whose text contains control characters.
pub fn format_program_status(status: &ProgramStatus) -> String {
    let mut pairs = vec![format!("state={}", status.state.as_str())];
    if let Some(app) = status.app.as_deref().filter(|a| app_is_valid(a)) {
        pairs.push(format!("app={app}"));
    }
    if status.state == ProgramState::Blocked
        && let Some(kind) = status.kind
    {
        pairs.push(format!("kind={}", kind.as_str()));
    }
    let collapsed = collapse_control_runs(status.message.as_deref().unwrap_or(""));
    let message = truncate_utf8(collapsed.trim(), MAX_MESSAGE_BYTES);
    if !message.is_empty() {
        pairs.push(format!(
            "msg={}",
            base64::engine::general_purpose::STANDARD.encode(message)
        ));
    }
    format!("\x1b]7501;{}\x1b\\", pairs.join(":"))
}

// ------------------------------------------------------------------- the process-global half ----

/// Whether the terminal confirmed OSC 7501 support — pi's `programStatusSupported`
/// (`terminal.ts:166`). Set by the negotiation ([`crate::keyboard_protocol::negotiate`]) or forced
/// by [`PROGRAM_STATUS_ENV`]; cleared by [`stop_program_status`].
static SUPPORTED: AtomicBool = AtomicBool::new(false);

/// What the STARTUP negotiation decided, kept across a [`stop_program_status`] so a Ctrl+Z
/// suspend or an external-editor round trip can put it back — [`resume_program_status`].
///
/// **[CYRUP-DELTA], and it is forced.** Pi's resume re-runs `ui.start()` →
/// `queryAndEnableKittyProtocol`, which re-queries and re-confirms (`terminal.ts:288-300`) and then
/// `writeProgramStatus()` re-sends. cyrup CANNOT re-query on resume: by then the crossterm reader
/// thread is live and a reply would race it — the reason `App::suspend` re-pushes the keyboard
/// flags unconditionally rather than re-negotiating, and says so
/// (`app/crossterm.rs`, "The startup decision still stands"). The terminal's capability has not
/// changed across a `fg`, so this remembers the one answer and restores it, which reaches the same
/// state pi's re-query reaches.
static NEGOTIATED_SUPPORT: AtomicBool = AtomicBool::new(false);

/// The last non-`clear` status written, or attempted — pi's `programStatus` (`terminal.ts:165`),
/// which is remembered **whether or not** the terminal supports the protocol, so a later
/// confirmation can re-send it (`:576-585`).
static LAST: Mutex<Option<ProgramStatus>> = Mutex::new(None);

/// Poison-tolerant access to [`LAST`], for the reason [`crate::terminal_progress::lock_progress_armed`]
/// gives for its own: a sibling that panicked has already reported its failure, and refusing the
/// lock here would turn that into a second, misleading one.
fn last() -> std::sync::MutexGuard<'static, Option<ProgramStatus>> {
    LAST.lock().unwrap_or_else(|p| p.into_inner())
}

/// Whether reports are being written — pi's `if (this.programStatusSupported)` guard.
pub fn program_status_is_supported() -> bool {
    SUPPORTED.load(Ordering::Relaxed)
}

/// Whether a status is remembered, i.e. whether the exit path owes the terminal a `state=clear` —
/// pi's `if (this.programStatusSupported && this.programStatus)` in `stop()`
/// (`terminal.ts:464-476`).
pub fn program_status_is_armed() -> bool {
    program_status_is_supported() && last().is_some()
}

/// Record the negotiation's outcome and, on a confirmation, re-send whatever status is remembered —
/// pi's reply handler, which sets `programStatusSupported = true` and calls `writeProgramStatus()`
/// (`terminal.ts:239-245`), and the same pair after the query write (`:300`).
pub fn set_program_status_supported(supported: bool) {
    SUPPORTED.store(supported, Ordering::Relaxed);
    NEGOTIATED_SUPPORT.store(supported, Ordering::Relaxed);
    if supported {
        write_remembered_program_status();
    }
}

/// `ProcessTerminal.setProgramStatus(status)` — `terminal.ts:576-579`.
///
/// The status is REMEMBERED even when the terminal does not support the protocol, so a later
/// confirmation (a resume, or a reply that arrives after a restart) can re-send it. A `clear`
/// forgets it instead.
///
/// Flushed immediately, for the reason [`crate::terminal_progress::write_terminal_progress`] gives:
/// a buffered stdout would hold the report until the next frame, or discard it entirely on a write
/// from a crash path.
pub fn write_program_status(status: &ProgramStatus) {
    {
        let mut slot = last();
        *slot = if status.state == ProgramState::Clear {
            None
        } else {
            Some(status.clone())
        };
    }
    if program_status_is_supported() {
        emit(&format_program_status(status));
    }
}

/// `ProcessTerminal.writeProgramStatus()` — `terminal.ts:581-585`: re-send the remembered status,
/// if there is one and the terminal supports the protocol.
pub fn write_remembered_program_status() {
    if !program_status_is_supported() {
        return;
    }
    let remembered = last().clone();
    if let Some(status) = remembered {
        emit(&format_program_status(&status));
    }
}

/// `ProcessTerminal.stop()`'s program-status leg — `terminal.ts:464-476`: write `state=clear` when
/// a status is armed, then drop support so nothing is written until a new negotiation confirms it
/// again. The remembered status survives, which is what lets a resume re-send it.
pub fn stop_program_status() {
    if program_status_is_armed() {
        emit(PROGRAM_STATUS_CLEAR);
    }
    SUPPORTED.store(false, Ordering::Relaxed);
}

/// The resume half of [`stop_program_status`] — pi's `ui.start()` re-query followed by
/// `writeProgramStatus()` (`terminal.ts:288-300`, `:581-585`), reached here by restoring the
/// startup negotiation's answer instead of asking again; see [`NEGOTIATED_SUPPORT`].
///
/// A no-op for a terminal that never supported the protocol, and for one whose status was forgotten
/// by [`clear_program_status_on_exit`].
pub fn resume_program_status() {
    if !NEGOTIATED_SUPPORT.load(Ordering::Relaxed) {
        return;
    }
    SUPPORTED.store(true, Ordering::Relaxed);
    write_remembered_program_status();
}

/// Forget the remembered status as well — the true process exit, as opposed to a suspend.
pub fn clear_program_status_on_exit() {
    stop_program_status();
    NEGOTIATED_SUPPORT.store(false, Ordering::Relaxed);
    *last() = None;
}

/// The one write. Routed through [`crate::dead_terminal::terminal_stdout`] like every other
/// terminal write in this crate, so a hung-up terminal takes the dead-terminal exit path rather
/// than an ignored error.
fn emit(sequence: &str) {
    use std::io::Write;
    let mut out = crate::dead_terminal::terminal_stdout();
    let _ = out.write_all(sequence.as_bytes());
    let _ = out.flush();
}

/// Serializes every TEST that reads or writes [`SUPPORTED`] or [`LAST`], across modules — declared
/// outside `mod tests` and `pub(crate)` for exactly the reason
/// [`crate::terminal_progress::lock_progress_armed`] documents: a lock declared inside one module's
/// `tests` is a different instance from anything another module can reach, and `cargo test` runs
/// this crate's unit tests as threads in ONE process. The second toucher is
/// `panic_hook::restore_terminal_best_effort`, whose restoration tests clear this global from
/// another thread.
#[cfg(test)]
pub(crate) fn lock_program_status() -> std::sync::MutexGuard<'static, ()> {
    static GLOBAL_LOCK: Mutex<()> = Mutex::new(());
    GLOBAL_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

/// Reset both globals — tests only.
#[cfg(test)]
pub(crate) fn reset_program_status_for_test() {
    SUPPORTED.store(false, Ordering::Relaxed);
    NEGOTIATED_SUPPORT.store(false, Ordering::Relaxed);
    *last() = None;
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

    /// `decodeMessage(sequence)` from upstream's test file
    /// (`packages/tui/test/program-status.test.ts:5-8`): the `:msg=` payload, base64-decoded.
    fn decode_message(sequence: &str) -> Option<String> {
        let at = sequence.find(":msg=")? + ":msg=".len();
        let rest = sequence.get(at..)?;
        let end = rest.find('\x1b').unwrap_or(rest.len());
        let b64 = rest.get(..end)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        String::from_utf8(bytes).ok()
    }

    /// Upstream's first format case (`program-status.test.ts:12-18`), with cyrup's app name.
    #[test]
    fn format_encodes_state_app_kind_and_a_base64_message() {
        let status = ProgramStatus {
            state: ProgramState::Blocked,
            app: Some("cyrup".to_string()),
            kind: Some(BlockedKind::Permission),
            message: Some("Allow bash?".to_string()),
        };
        assert_eq!(
            format_program_status(&status),
            "\x1b]7501;state=blocked:app=cyrup:kind=permission:msg=QWxsb3cgYmFzaD8=\x1b\\",
            "byte for byte: pair order is state, app, kind, msg (program-status.ts:47-52)"
        );
        assert_eq!(
            format_program_status(&ProgramStatus::new(ProgramState::Clear)),
            PROGRAM_STATUS_CLEAR
        );
        assert_eq!(PROGRAM_STATUS_CLEAR, "\x1b]7501;state=clear\x1b\\");
    }

    /// Upstream's second case (`program-status.test.ts:20-27`): a space in the app name fails
    /// `APP_PATTERN`, a `kind` outside `blocked` is dropped, and a whitespace-only message trims to
    /// empty — so all three pairs vanish and only `state=` remains.
    #[test]
    fn kind_outside_blocked_invalid_app_names_and_empty_messages_are_omitted() {
        let status = ProgramStatus {
            state: ProgramState::Working,
            app: Some("my app".to_string()),
            kind: Some(BlockedKind::Auth),
            message: Some(" \n ".to_string()),
        };
        assert_eq!(
            format_program_status(&status),
            "\x1b]7501;state=working\x1b\\"
        );
        let at_limit = ProgramStatus {
            state: ProgramState::Idle,
            app: Some("a".repeat(32)),
            ..ProgramStatus::default()
        };
        assert!(
            format_program_status(&at_limit).contains(&format!(":app={}\x1b", "a".repeat(32))),
            "32 characters is the inclusive upper bound"
        );
        let over_limit = ProgramStatus {
            state: ProgramState::Idle,
            app: Some("a".repeat(33)),
            ..ProgramStatus::default()
        };
        assert!(
            !format_program_status(&over_limit).contains("app="),
            "33 characters is dropped, not truncated"
        );
    }

    /// Upstream's third case (`program-status.test.ts:29-32`), and the one that pins the `+` in
    /// `/[\u0000-\u001f\u007f-\u009f]+/g`: a RUN of control characters collapses to ONE space, so
    /// `ESC` followed by the literal `[31m` yields `" [31m"` and not a space per byte.
    #[test]
    fn control_characters_are_replaced_because_terminals_discard_the_report() {
        let status =
            ProgramStatus::with_message(ProgramState::Error, "first\nsecond\x1b[31m\u{9b}third\t");
        assert_eq!(
            decode_message(&format_program_status(&status)).as_deref(),
            Some("first second [31m third"),
            "the trailing tab's space is trimmed"
        );
        // Upstream's own string does NOT discriminate run-collapse from per-character replacement:
        // it has no two ADJACENT control characters (`\x1b` is followed by the literal `[31m`, and
        // `\u{9b}` by `third`), so both readings give the same answer. This case is cyrup's
        // addition and it is the one that pins the `+` in
        // `/[\u0000-\u001f\u007f-\u009f]+/g` (`program-status.ts:28`): three control bytes in a
        // row are ONE space, not three.
        let run = ProgramStatus::with_message(ProgramState::Error, "a\r\n\x07b");
        assert_eq!(
            decode_message(&format_program_status(&run)).as_deref(),
            Some("a b"),
            "a RUN of control characters collapses to exactly one space"
        );
    }

    /// Upstream's fourth case (`program-status.test.ts:34-40`).
    #[test]
    fn long_messages_are_cut_at_a_utf8_boundary_within_the_spec_limits() {
        let status = ProgramStatus {
            state: ProgramState::Working,
            app: Some("cyrup".to_string()),
            kind: None,
            message: Some("é".repeat(2000)),
        };
        let sequence = format_program_status(&status);
        let message = decode_message(&sequence).expect("a msg pair");
        assert_eq!(message, "é".repeat(1024), "2048 bytes / 2 bytes per `é`");
        assert!(message.len() <= MAX_MESSAGE_BYTES);
        assert!(sequence.len() <= 4096, "under the encoded limit");
    }

    /// Upstream's `isProgramStatusReply` case (`program-status.test.ts:43-50`).
    #[test]
    fn the_query_echo_is_recognised_with_either_terminator_and_future_pairs() {
        assert!(is_program_status_reply("\x1b]7501;?\x1b\\"));
        assert!(is_program_status_reply("\x1b]7501;?\x07"));
        assert!(is_program_status_reply("\x1b]7501;?version=2\x1b\\"));
        assert!(
            !is_program_status_reply("\x1b]7501;state=idle\x1b\\"),
            "a report is not a reply"
        );
        assert!(!is_program_status_reply("\x1b]11;rgb:0000/0000/0000\x07"));
        // The query cyrup writes must be the thing this recognises.
        assert!(is_program_status_reply(PROGRAM_STATUS_QUERY));
    }

    /// [`contains_program_status_reply`] is the frame-scanning form the negotiation uses, and it
    /// must agree with [`is_program_status_reply`] on every framed reply while still rejecting a
    /// report buried in the same stream.
    #[test]
    fn the_frame_scanner_agrees_with_the_single_frame_test() {
        assert!(contains_program_status_reply(
            "\x1b[?1u\x1b]7501;?\x1b\\\x1b[?62;1;2c"
        ));
        assert!(contains_program_status_reply(
            "\x1b]7501;?\x07\x1b[?62;1;2c"
        ));
        assert!(contains_program_status_reply(
            "\x1b]11;rgb:0/0/0\x07\x1b]7501;?version=2\x1b\\"
        ));
        assert!(
            !contains_program_status_reply("\x1b[?1u\x1b[?62;1;2;6;9;15;22c"),
            "DA1 alone is not support"
        );
        assert!(
            !contains_program_status_reply("\x1b]7501;state=working\x1b\\"),
            "our own report echoed back is not a reply"
        );
        assert!(
            !contains_program_status_reply("\x1b]7501;?"),
            "an unterminated prefix is not a reply"
        );
    }

    /// Pi's `PI_PROGRAM_STATUS` tri-state (`terminal.ts:289-291`): `1` and `0` BOTH skip the query;
    /// only `1` reports.
    #[test]
    fn the_env_override_is_pis_tri_state() {
        assert_eq!(
            resolve_program_status_override(Some("1")),
            ProgramStatusOverride::ForceOn
        );
        assert_eq!(
            resolve_program_status_override(Some("0")),
            ProgramStatusOverride::ForceOff
        );
        assert_eq!(
            resolve_program_status_override(None),
            ProgramStatusOverride::Query
        );
        assert_eq!(
            resolve_program_status_override(Some("yes")),
            ProgramStatusOverride::Query
        );
        assert_eq!(PROGRAM_STATUS_ENV, "CYRUP_PROGRAM_STATUS");
    }

    /// The row's own "a status left set after cyrup quits is worse than never setting it", as the
    /// armed bit the exit and crash paths answer from.
    ///
    /// Pi's `stop()` writes the clear only `if (this.programStatusSupported && this.programStatus)`
    /// (`terminal.ts:464-476`), then drops support; the remembered status SURVIVES, which is what
    /// [`resume_program_status`] re-sends after a Ctrl+Z. `clear_program_status_on_exit` forgets it
    /// as well, because that is a real exit.
    #[test]
    fn the_status_is_cleared_on_exit_and_re_sent_after_a_restart() {
        let _guard = lock_program_status();
        reset_program_status_for_test();

        // Unsupported: nothing is armed, so neither the exit path nor the panic hook owes a write.
        write_program_status(&ProgramStatus::with_message(ProgramState::Working, "s"));
        assert!(!program_status_is_armed(), "no support, nothing to clear");

        // Support confirmed: the remembered status is re-sent and the clear is now owed.
        set_program_status_supported(true);
        assert!(program_status_is_supported());
        assert!(
            program_status_is_armed(),
            "a status recorded while unsupported is remembered and re-sent on confirmation \
             (terminal.ts:576-585)"
        );

        // The suspend leg: clear + disarm, remembered status kept.
        stop_program_status();
        assert!(!program_status_is_supported(), "stop() drops support");
        assert!(!program_status_is_armed(), "and so owes no second clear");

        // The resume leg: support back, status re-sent.
        resume_program_status();
        assert!(program_status_is_supported());
        assert!(program_status_is_armed());

        // The exit leg forgets it, so a later spurious confirmation re-sends nothing.
        clear_program_status_on_exit();
        assert!(!program_status_is_armed());
        resume_program_status();
        assert!(
            !program_status_is_supported(),
            "a forgotten negotiation does not come back"
        );
        set_program_status_supported(true);
        assert!(
            !program_status_is_armed(),
            "support without a remembered status owes no clear"
        );

        // A `clear` report forgets the status too (terminal.ts:577).
        write_program_status(&ProgramStatus::with_message(ProgramState::Working, "s"));
        assert!(program_status_is_armed());
        write_program_status(&ProgramStatus::new(ProgramState::Clear));
        assert!(!program_status_is_armed());

        reset_program_status_for_test();
    }
}
