//! The framer: pi's `StdinBuffer` (`packages/tui/src/stdin-buffer.ts` @v0.87.1), over bytes.
//!
//! pi's header states the problem: *"stdin data events can arrive in partial chunks, especially
//! for escape sequences like mouse events. Without buffering, partial sequences can be
//! misinterpreted as regular keypresses"* (`stdin-buffer.ts:1-18`). [`StdinBuffer::push`] takes one
//! `read(2)` chunk and emits every [`Frame`] the bytes so far complete; whatever is left is held
//! until the next chunk or until [`StdinBuffer::deadline`] passes, when the reader calls
//! [`StdinBuffer::flush`] (pi's `setTimeout` callback, `:387-396`).
//!
//! What is ported, with the pi line each rule comes from:
//!
//! - completeness: `isCompleteSequence` and its CSI/OSC/DCS/APC helpers (`:31-181`), including the
//!   six-byte X10 mouse rule (`:45-48`) and the SGR mouse shape check (`:106-122`);
//! - `extractCompleteSequences` (`:194-257`), including WezTerm's `ESC ESC [` split (`:210-232`);
//! - bracketed paste, with no timeout (`:324-378`);
//! - the Kitty printable-codepoint dedup (`parseUnmodifiedKittyPrintableCodepoint` and
//!   `emitDataSequence`, `:186-192`, `:399-408`) — `TUI-046`;
//! - the two timeouts: a lone `ESC` waits the escape timeout, anything else the 50 ms sequence
//!   timeout (`:23-24`, `:387-388`).
//!
//! Two things work differently because pi's input is a UTF-8 **string** and this is bytes. Both are
//! `[CYRUP-DELTA]`s, argued where they are implemented:
//!
//! - an 8-bit meta byte becomes [`Frame::Meta`] only when UTF-8 decoding fails (`TUI-050`, see
//!   [`next_char`]);
//! - the Kitty dedup compares a whole `char`, not a UTF-16 code unit (see [`StdinBuffer::emit`]).
//!
//! And one bound pi does not have: a paste with no end marker is delivered in
//! [`MAX_PASTE_BYTES`] pieces instead of growing without limit (see [`StdinBuffer::cap_paste`]).

// The framer is only driven by the unix reader; on other targets only the completeness helpers
// `crate::escape_reassembly` borrows are reachable.
#![cfg_attr(not(unix), allow(dead_code))]

use std::time::{Duration, Instant};

/// `ESC`.
const ESC: u8 = 0x1b;

/// Pi `DEFAULT_SEQUENCE_TIMEOUT_MS` (`stdin-buffer.ts:23`): how long a partial sequence other than
/// a lone `ESC` is held before it is flushed as it stands.
pub(crate) const DEFAULT_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(50);

/// Pi `DEFAULT_ESCAPE_TIMEOUT_MS` (`stdin-buffer.ts:24`, and `terminal.ts:115`): how long a lone
/// `ESC` is held before it is the Escape key. `crate::app::resolve_escape_timeout` widens it under
/// SSH and honours `CYRUP_TUI_ESC_TIMEOUT` (`TUI-106`).
pub(crate) const DEFAULT_ESCAPE_TIMEOUT: Duration = Duration::from_millis(10);

/// `ESC [ 200 ~` (`stdin-buffer.ts:25`).
const PASTE_START: &[u8] = b"\x1b[200~";

/// `ESC [ 201 ~` (`stdin-buffer.ts:26`).
const PASTE_END: &[u8] = b"\x1b[201~";

/// The most paste content held before a piece of it is delivered.
///
/// `[CYRUP-DELTA]` — pi has no bound: a paste whose end marker never arrives grows `pasteBuffer`
/// forever (`stdin-buffer.ts:324-343`). Here, past this many bytes the content so far is delivered
/// as a [`Frame::Paste`] and paste mode continues, so a real paste loses nothing and a lost end
/// marker costs bounded memory. The pieces are paste events, never keystrokes: replaying pasted
/// text as keys would run every `Enter` in it as a submit.
pub(crate) const MAX_PASTE_BYTES: usize = 64 * 1024 * 1024;

/// One unit of terminal input, carrying the bytes it arrived as.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// An escape sequence, starting with `ESC`: complete, or flushed incomplete by its timeout.
    Seq(Vec<u8>),
    /// The raw content of a bracketed paste, without its markers.
    Paste(Vec<u8>),
    /// One character that is not part of an escape sequence. Its bytes are its UTF-8 encoding.
    Char(char),
    /// One byte above `0x7F` that is not UTF-8: an 8-bit meta key (`TUI-050`).
    Meta(u8),
}

/// What the bytes at a position amount to.
enum Scan {
    /// A complete unit of this many bytes.
    Complete(usize),
    /// More bytes are needed to decide.
    Incomplete,
}

/// The result of decoding one UTF-8 character.
enum Utf8 {
    /// A character, `len` bytes long.
    Char(char, usize),
    /// A valid prefix of a character that the buffer ends inside.
    Incomplete,
    /// The byte here can never start a character, or the ones after it do not continue it.
    Invalid,
}

/// Decode the character at the start of `bytes`.
///
/// This is where `TUI-050` is decided. `[CYRUP-DELTA]` — pi's rule (`stdin-buffer.ts:303-315`)
/// turns a one-byte `Buffer` chunk above `0x7F` into `ESC` + (byte − 128). That code is dead in pi:
/// `ProcessTerminal.start` calls `process.stdin.setEncoding("utf8")` (`terminal.ts:181`), so
/// `process` only ever receives strings and `Buffer.isBuffer(data)` is never true; Node's decoder
/// turns the byte into U+FFFD. Ported literally it would also be wrong: a two-byte character split
/// across two reads arrives as a one-byte chunk holding its lead, which the rule would read as Alt.
/// So a high byte is a meta key only when it cannot be UTF-8 — a byte that never starts a
/// character (`0x80`–`0xBF`, `0xC0`, `0xC1`, `0xF5`–`0xFF`), or a lead byte that the next byte
/// does not continue — and a lead byte at the end of the input is held, like a lone `ESC`, until
/// the next read decides it or the escape timeout expires.
fn next_char(bytes: &[u8]) -> Utf8 {
    let window = bytes.get(..bytes.len().min(4)).unwrap_or(bytes);
    let valid = match std::str::from_utf8(window) {
        Ok(s) => s,
        Err(e) if e.valid_up_to() > 0 => window
            .get(..e.valid_up_to())
            .and_then(|v| std::str::from_utf8(v).ok())
            .unwrap_or_default(),
        Err(e) if e.error_len().is_none() => return Utf8::Incomplete,
        Err(_) => return Utf8::Invalid,
    };
    match valid.chars().next() {
        Some(c) => Utf8::Char(c, c.len_utf8()),
        None => Utf8::Incomplete,
    }
}

/// The length of one character-sized unit at the start of `bytes`, for the positions where pi
/// counts one character after an introducer (`ESC x`, `ESC O x`). An invalid byte counts alone.
fn unit_len(bytes: &[u8]) -> Scan {
    match next_char(bytes) {
        Utf8::Char(_, n) => Scan::Complete(n),
        Utf8::Incomplete => Scan::Incomplete,
        Utf8::Invalid => Scan::Complete(1),
    }
}

/// Is this CSI buffer a complete sequence? Pi `isCompleteCsiSequence` (`stdin-buffer.ts:86-128`),
/// plus the X10 mouse rule from its caller (`:44-48`). `buf` starts `ESC [`.
///
/// The X10 rule counts bytes where pi counts UTF-16 code units. The three bytes after `ESC [ M` are
/// raw coordinates that go above `0x7F` on a wide terminal, where Node's decoder turns each into
/// one U+FFFD, so the two counts agree on everything a terminal sends.
pub(crate) fn is_complete_csi(buf: &[u8]) -> bool {
    if !buf.starts_with(b"\x1b[") {
        return true;
    }
    if buf.starts_with(b"\x1b[M") {
        return buf.len() >= 6;
    }
    let Some(payload) = buf.get(2..).filter(|p| !p.is_empty()) else {
        return false;
    };
    let Some(&last) = payload.last() else {
        return false;
    };
    if !(0x40..=0x7e).contains(&last) {
        return false;
    }
    // SGR mouse: `ESC [ < digits ; digits ; digits [Mm]` (`:106-122`). A final byte in range is not
    // enough; the shape must match.
    if payload.first() == Some(&b'<') {
        if last != b'M' && last != b'm' {
            return false;
        }
        let inner = payload.get(1..payload.len() - 1).unwrap_or_default();
        let mut parts = 0;
        for part in inner.split(|b| *b == b';') {
            if part.is_empty() || !part.iter().all(u8::is_ascii_digit) {
                return false;
            }
            parts += 1;
        }
        return parts == 3;
    }
    true
}

/// The length of the escape sequence at the start of `rem` (which starts with `ESC`), or
/// [`Scan::Incomplete`]. Pi `isCompleteSequence` (`stdin-buffer.ts:31-80`) applied to growing
/// prefixes, as `extractCompleteSequences` does (`:204-244`).
fn scan_sequence(rem: &[u8]) -> Scan {
    let Some(&intro) = rem.get(1) else {
        return Scan::Incomplete;
    };
    let first_prefix = |from: usize, done: &dyn Fn(&[u8]) -> bool| {
        (from..=rem.len())
            .find(|&end| rem.get(..end).is_some_and(done))
            .map_or(Scan::Incomplete, Scan::Complete)
    };
    match intro {
        // CSI (`:43-50`).
        b'[' => first_prefix(3, &is_complete_csi),
        // OSC ends with ST or BEL (`:134-145`).
        b']' => first_prefix(3, &|c: &[u8]| {
            c.ends_with(b"\x1b\\") || c.ends_with(b"\x07")
        }),
        // DCS (`:152-163`) and APC (`:170-181`) end with ST. XTVersion replies are DCS and Kitty
        // graphics replies are APC — `TUI-047`.
        b'P' | b'_' => first_prefix(3, &|c: &[u8]| c.ends_with(b"\x1b\\")),
        // SS3: `ESC O` and one character (`:67-71`).
        b'O' => match unit_len(rem.get(2..).unwrap_or_default()) {
            Scan::Complete(n) => Scan::Complete(2 + n),
            Scan::Incomplete => Scan::Incomplete,
        },
        // Meta key: `ESC` and one character; anything else is "unknown, treat as complete"
        // (`:73-79`), which is also two characters.
        _ => match unit_len(rem.get(1..).unwrap_or_default()) {
            Scan::Complete(n) => Scan::Complete(1 + n),
            Scan::Incomplete => Scan::Incomplete,
        },
    }
}

/// Split `buf` into frames. Returns the frames and where the unconsumed remainder starts.
///
/// Pi `extractCompleteSequences` (`stdin-buffer.ts:194-257`). `more` says whether later bytes can
/// still extend `buf`: when they cannot (the text in front of a paste marker), a trailing UTF-8
/// lead byte is decided now instead of being held.
fn extract(buf: &[u8], more: bool) -> (Vec<Frame>, usize) {
    let mut frames = Vec::new();
    let mut pos = 0;
    while let Some(rem) = buf.get(pos..).filter(|r| !r.is_empty()) {
        if rem.first() == Some(&ESC) {
            match scan_sequence(rem) {
                Scan::Complete(n) => {
                    // WezTerm sends Escape as a raw `ESC` and its Kitty release as a CSI-u, so the
                    // two arrive as `ESC ESC [ …`. `ESC ESC` would otherwise complete as a meta
                    // key and leave the CSI's tail to be typed (`:210-232`).
                    if n == 2
                        && rem.get(1) == Some(&ESC)
                        && matches!(rem.get(2), Some(b'[' | b']' | b'O' | b'P' | b'_'))
                    {
                        frames.push(Frame::Seq(vec![ESC]));
                        pos += 1;
                        continue;
                    }
                    frames.push(Frame::Seq(rem.get(..n).unwrap_or(rem).to_vec()));
                    pos += n;
                }
                Scan::Incomplete => return (frames, pos),
            }
        } else {
            match next_char(rem) {
                Utf8::Char(c, n) => {
                    frames.push(Frame::Char(c));
                    pos += n;
                }
                Utf8::Incomplete if more => return (frames, pos),
                Utf8::Incomplete | Utf8::Invalid => {
                    frames.push(Frame::Meta(rem.first().copied().unwrap_or_default()));
                    pos += 1;
                }
            }
        }
    }
    (frames, buf.len())
}

/// Pi `parseUnmodifiedKittyPrintableCodepoint` (`stdin-buffer.ts:186-192`): the codepoint of a
/// Kitty `CSI codepoint[:shifted[:base]] u` with no modifier field, if it is printable. The regex
/// is `^\x1b\[(\d+)(?::\d*)?(?::\d+)?u$`.
fn unmodified_kitty_printable(seq: &[u8]) -> Option<char> {
    let body = seq.strip_prefix(b"\x1b[")?.strip_suffix(b"u")?;
    let mut parts = body.split(|b| *b == b':');
    let first = parts.next().filter(|p| !p.is_empty())?;
    let second = parts.next();
    let third = parts.next();
    let all_digits = |p: &[u8]| p.iter().all(u8::is_ascii_digit);
    let ok = all_digits(first)
        && parts.next().is_none()
        && second.is_none_or(all_digits)
        && third.is_none_or(|p| !p.is_empty() && all_digits(p));
    if !ok {
        return None;
    }
    let codepoint: u32 = std::str::from_utf8(first).ok()?.parse().ok()?;
    (codepoint >= 32)
        .then(|| char::from_u32(codepoint))
        .flatten()
}

/// Pi's `StdinBuffer` over bytes. See the module docs.
#[derive(Debug)]
pub(crate) struct StdinBuffer {
    /// Held bytes: a partial escape sequence or a partial UTF-8 character (pi `buffer`).
    buffer: Vec<u8>,
    /// `Some` while inside a bracketed paste (pi `pasteMode` + `pasteBuffer`).
    paste: Option<Vec<u8>>,
    /// Pi `pendingKittyPrintableCodepoint`.
    pending_kitty: Option<char>,
    /// When the held bytes are flushed if nothing completes them (pi's pending `setTimeout`).
    deadline: Option<Instant>,
    /// Pi `timeoutMs`.
    sequence_timeout: Duration,
    /// Pi `escapeTimeoutMs`.
    escape_timeout: Duration,
    /// [`MAX_PASTE_BYTES`], lowered by tests.
    paste_cap: usize,
}

impl StdinBuffer {
    /// A buffer with pi's sequence timeout and the given escape timeout (pi's `new StdinBuffer({
    /// escapeTimeout: resolveEscapeTimeoutMs() })`, `terminal.ts:214`).
    pub(crate) fn new(escape_timeout: Duration) -> Self {
        Self::with_timeouts(DEFAULT_SEQUENCE_TIMEOUT, escape_timeout)
    }

    /// Both timeouts explicit (pi's `StdinBufferOptions`).
    pub(crate) fn with_timeouts(sequence_timeout: Duration, escape_timeout: Duration) -> Self {
        Self {
            buffer: Vec::new(),
            paste: None,
            pending_kitty: None,
            deadline: None,
            sequence_timeout,
            escape_timeout,
            paste_cap: MAX_PASTE_BYTES,
        }
    }

    /// Lower the paste cap, for tests.
    #[cfg(test)]
    pub(crate) fn with_paste_cap(mut self, cap: usize) -> Self {
        self.paste_cap = cap;
        self
    }

    /// The held bytes (pi `getBuffer`).
    #[cfg(test)]
    pub(crate) fn held(&self) -> &[u8] {
        &self.buffer
    }

    /// Whether a bracketed paste is open.
    #[cfg(test)]
    pub(crate) fn in_paste(&self) -> bool {
        self.paste.is_some()
    }

    /// When [`Self::flush`] is due, if anything is held.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Feed one read's bytes, received at `now`, and append every frame they complete (pi
    /// `process`, `stdin-buffer.ts:296-397`).
    pub(crate) fn push(&mut self, data: &[u8], now: Instant, out: &mut Vec<Frame>) {
        self.deadline = None;
        // Pi emits an empty `data` event for an empty chunk with nothing held (`:317-320`). A
        // zero-byte `read(2)` is end of file, which the reader handles before it gets here, so
        // there is no empty chunk to report.
        let mut chunk: Vec<u8> = data.to_vec();
        loop {
            if let Some(paste) = self.paste.as_mut() {
                let from = paste.len().saturating_sub(PASTE_END.len() - 1);
                paste.append(&mut chunk);
                match find(paste, PASTE_END, from) {
                    Some(end) => chunk = self.close_paste(end, out),
                    None => self.cap_paste(out),
                }
                if chunk.is_empty() {
                    return;
                }
                continue;
            }

            self.buffer.append(&mut chunk);
            if let Some(start) = find(&self.buffer, PASTE_START, 0) {
                // Text in front of the marker is extracted; a partial sequence there cannot be
                // completed any more and is dropped, as pi drops `extractCompleteSequences`'s
                // remainder (`:347-354`).
                let (frames, _) = extract(self.buffer.get(..start).unwrap_or_default(), false);
                for frame in frames {
                    self.emit(frame, out);
                }
                self.pending_kitty = None;
                let content = self
                    .buffer
                    .get(start + PASTE_START.len()..)
                    .unwrap_or_default()
                    .to_vec();
                self.buffer.clear();
                self.paste = Some(Vec::new());
                chunk = content;
                continue;
            }

            let (frames, rest) = extract(&self.buffer, true);
            self.buffer.drain(..rest);
            for frame in frames {
                self.emit(frame, out);
            }
            if !self.buffer.is_empty() {
                self.deadline = Some(now + self.hold_timeout());
            }
            return;
        }
    }

    /// Release whatever is held (pi `flush`, `stdin-buffer.ts:410-424`, and the timer callback
    /// that emits its result, `:389-395`). The reader calls this once [`Self::deadline`] passes.
    ///
    /// A held escape sequence is one [`Frame::Seq`], as in pi. A held UTF-8 lead byte whose
    /// continuation never came is an 8-bit meta key after all (`TUI-050`).
    pub(crate) fn flush(&mut self, out: &mut Vec<Frame>) {
        self.deadline = None;
        if self.buffer.is_empty() {
            return;
        }
        let held = std::mem::take(&mut self.buffer);
        self.pending_kitty = None;
        if held.first() == Some(&ESC) {
            self.emit(Frame::Seq(held), out);
        } else {
            for byte in held {
                self.emit(Frame::Meta(byte), out);
            }
        }
    }

    /// Forget everything held without emitting it (pi `clear`, `stdin-buffer.ts:426-435`).
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.deadline = None;
        self.buffer.clear();
        self.paste = None;
        self.pending_kitty = None;
    }

    /// Pi's timeout choice (`stdin-buffer.ts:388`): a lone `ESC` waits the escape timeout, any
    /// other partial sequence the sequence timeout. A held UTF-8 lead byte is the same question as
    /// a lone `ESC` — a key on its own, or the start of something longer — so it waits the escape
    /// timeout too, and inherits its SSH widening.
    fn hold_timeout(&self) -> Duration {
        match self.buffer.first() {
            Some(&ESC) if self.buffer.len() == 1 => self.escape_timeout,
            Some(&ESC) => self.sequence_timeout,
            _ => self.escape_timeout,
        }
    }

    /// End a paste whose end marker starts at `end`: emit the content and return the bytes after
    /// the marker, which pi feeds back through `process` (`:328-342`).
    fn close_paste(&mut self, end: usize, out: &mut Vec<Frame>) -> Vec<u8> {
        let mut paste = self.paste.take().unwrap_or_default();
        let rest = paste
            .get(end + PASTE_END.len()..)
            .unwrap_or_default()
            .to_vec();
        paste.truncate(end);
        self.pending_kitty = None;
        out.push(Frame::Paste(paste));
        rest
    }

    /// Deliver the front of an over-long paste (see [`MAX_PASTE_BYTES`]). The last few bytes stay
    /// behind in case they are the start of the end marker, and the cut never splits a UTF-8
    /// character.
    fn cap_paste(&mut self, out: &mut Vec<Frame>) {
        let Some(paste) = self.paste.as_mut() else {
            return;
        };
        if paste.len() <= self.paste_cap {
            return;
        }
        let mut cut = paste.len() - (PASTE_END.len() - 1);
        while cut > 0 && paste.get(cut).is_some_and(|b| b & 0xc0 == 0x80) {
            cut -= 1;
        }
        let rest = paste.split_off(cut);
        let piece = std::mem::replace(paste, rest);
        self.pending_kitty = None;
        out.push(Frame::Paste(piece));
    }

    /// Pi `emitDataSequence` (`stdin-buffer.ts:399-408`): drop a raw character that repeats the
    /// Kitty printable sequence just before it, and remember the next one.
    ///
    /// `[CYRUP-DELTA]` — pi only compares when `sequence.length === 1`, i.e. one UTF-16 code unit,
    /// so a Kitty sequence for a character outside the BMP is never deduplicated and that
    /// character is inserted twice. That is an artefact of JavaScript strings; a [`Frame::Char`]
    /// is always one whole character.
    fn emit(&mut self, frame: Frame, out: &mut Vec<Frame>) {
        if let Frame::Char(c) = frame
            && self.pending_kitty == Some(c)
        {
            self.pending_kitty = None;
            return;
        }
        self.pending_kitty = match &frame {
            Frame::Seq(seq) => unmodified_kitty_printable(seq),
            _ => None,
        };
        out.push(frame);
    }
}

/// The first index at or after `from` where `needle` starts in `hay`.
fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

#[cfg(test)]
#[path = "frame_tests.rs"]
mod tests;
