//! Reassemble an escape sequence that was split **at the `ESC` byte** across two `read(2)` calls.
//!
//! This is the cyrup half of Pi's `packages/tui/src/stdin-buffer.ts` (@v0.83.0, 434 lines) — the
//! module whose header states the whole problem: *"stdin data events can arrive in partial chunks,
//! especially for escape sequences… Without buffering, partial sequences can be misinterpreted as
//! regular keypresses"* (`stdin-buffer.ts:1-18`).
//!
//! # What crossterm already does, and the one thing it does not
//!
//! Most of `stdin-buffer.ts`'s duty is discharged for cyrup by crossterm 0.29's own parser.
//! `Parser::advance` (`crossterm-0.29.0/src/event/source/unix/tty.rs:247-268`) pushes the read
//! buffer one byte at a time and **keeps** `self.buffer` whenever `parse_event` answers `Ok(None)`,
//! so a CSI, SS3, OSC or bracketed-paste frame arriving over any number of reads is reassembled —
//! including a paste body spanning many reads, which is the second duty Pi's file carries.
//!
//! The exception is a buffer that is **exactly one `ESC` byte**:
//!
//! ```text
//! crossterm-0.29.0/src/event/sys/unix/parse.rs:36-42
//!     if buffer.len() == 1 {
//!         if input_available { Ok(None) } else { Ok(Some(Esc)) }
//!     }
//! ```
//!
//! and `input_available` is `read_count == TTY_BUFFER_SIZE` (`tty.rs:149-154`, `TTY_BUFFER_SIZE =
//! 1_024` at `:40`). So **any** read that does not fill 1,024 bytes and ends on `0x1B` emits a
//! `Key(Esc)` and clears the buffer; the sequence's tail arrives in the next read with no introducer
//! in front of it and is decoded as literal characters. `\x1b` then `[A` becomes `Esc`, `Char('[')`,
//! `Char('A')` instead of `Up`.
//!
//! Pi is immune because `isCompleteSequence` (`stdin-buffer.ts:29-78`) classifies a bare `ESC` as
//! `"incomplete"` (`:34-36`) and `process()` (`:371-386`) parks the remainder behind a
//! `setTimeout(this.timeoutMs)` — 10 ms by default (`:262`, `:284`) — that flushes it only if
//! nothing completes it.
//!
//! **Measured 2026-08-13 (`docs/gap-analysis/REPRO-LOG.md`, `TUI-045`).** Two `tmux send-keys -H`
//! writes 60 ms apart on a *local* pty: at idle the user gets a swallowed `Escape` plus the literal
//! characters `[A` in the prompt; mid-stream the bare `Escape` reaches the interrupt handler and
//! **aborts the running turn** (`Operation aborted` at token 267 of 300).
//!
//! # Why this operates on events rather than on bytes
//!
//! `[CYRUP-DELTA]` — Pi filters **raw bytes** ahead of its own `parseKeypress`. crossterm exposes no
//! equivalent seam: `parse_event` is `pub(crate)` (`parse.rs:26`), `EventSource` is private, and
//! `event::poll` itself drains the tty into crossterm's parser (see
//! [`crate::terminal_query`]'s module doc), so there is no point at which cyrup can hand bytes to
//! crossterm. The reassembly therefore runs **after** crossterm on the shredded events, reconstructs
//! the sequence's bytes from them, and decodes the result itself.
//!
//! The decoder below is consequently a mirror of crossterm's own `parse_csi` family
//! (`parse.rs:137-214`, `:348-393`, `:497-616`, `:619-660`) so that a split sequence produces
//! **byte-identical events** to the same sequence delivered in one read — that equality is what this
//! module's own `tests` pin. Completeness, by contrast, is a direct port of Pi's
//! `isCompleteSequence` / `isCompleteCsiSequence` (`stdin-buffer.ts:29-126`), including the
//! `ESC [ M` six-byte rule (`:43-46`) and the SGR-mouse shape check (`:102-120`).
//!
//! # Scope, and what is deliberately left to its neighbours
//!
//! Only the `ESC` `[` (CSI, including bracketed paste) and `ESC` `O` (SS3) introducers are claimed
//! here, which is exactly the split form `TUI-045` measures. The three remaining introducers Pi
//! lists — `ESC ]` (OSC), `ESC P` (DCS) and `ESC _` (APC) — are replayed untouched so that
//! [`crate::stray_reply`]'s OSC 11 machine keeps seeing them exactly as it does today; generalising
//! that machine to DCS/APC is `TUI-047`. A bare `ESC` followed by an ordinary character (Pi's
//! meta-key case, `stdin-buffer.ts:71-74`) is **not** claimed either: cyrup's hold window is the
//! reader thread's idle poll, and folding a real `Escape` press followed by fast typing into an
//! `Alt+`chord is a worse failure than the one being fixed.
//!
//! # The safety contract
//!
//! Identical in shape to [`crate::stray_reply`]'s, and for the same reason — this sits on the path
//! every keystroke takes. **The only paths that discard a held event are a successfully decoded
//! sequence and a completed bracketed paste.** Every other exit — an unexpected event, an
//! undecodable sequence, the [`MAX_HELD`] cap, or the input simply going idle ([`EscapeReassembler::flush`],
//! driven by the reader thread's shortened poll) — replays the held prefix in order, degrading to
//! today's behaviour rather than to lost input.

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode,
};

/// Hard cap on how many events may be held while reassembling a non-paste sequence.
///
/// The longest reachable frame is a Kitty `CSI unicode:shifted:base ; mods:kind ; text u`, well
/// under 40 characters; the cap is generous enough for that and still bounds the hold to well under
/// a screen line.
pub const MAX_HELD: usize = 64;

/// Hard cap on the held-event count while reassembling a **bracketed paste** whose `ESC [ 200 ~`
/// opener was split. A paste is arbitrarily long, so the bound is the memory bound rather than a
/// frame-shape bound; hitting it replays everything, which is exactly today's behaviour.
pub const MAX_PASTE_HELD: usize = 65_536;

/// `ESC [ 200 ~` — the bracketed-paste opener (`stdin-buffer.ts:23`).
const PASTE_START: &[u8] = b"\x1b[200~";

/// The tail of `ESC [ 201 ~` after its `ESC` (`stdin-buffer.ts:24`), matched one event at a time.
const PASTE_END_TAIL: [char; 5] = ['[', '2', '0', '1', '~'];

/// Where the machine is inside a split sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nothing held; every event passes straight through.
    Idle,
    /// A bare `Esc` is held — it may be the head of a sequence whose tail is in the next read.
    Esc,
    /// `ESC [` seen; accumulating CSI parameter bytes until a final byte in `0x40..=0x7E`.
    Csi,
    /// `ESC O` seen; the next character completes the SS3 sequence.
    Ss3,
    /// `ESC [ 200 ~` seen; accumulating the pasted text.
    PasteBody,
    /// Inside a paste body, an `Esc` arrived; `n` characters of `[201~` have matched.
    PasteEnd(u8),
}

/// Reassembles a sequence split at the `ESC` byte and passes everything else through untouched.
///
/// Feed it with [`push`](Self::push) and, when the input goes idle, [`flush`](Self::flush) — the
/// same contract as [`crate::stray_reply::StrayReplyFilter`], which it is chained in front of.
#[derive(Debug)]
pub struct EscapeReassembler {
    state: State,
    held: Vec<Event>,
    /// The sequence bytes reconstructed from `held`, always starting `\x1b`.
    bytes: Vec<u8>,
    /// The pasted text reconstructed from `held` while in [`State::PasteBody`].
    paste: String,
}

impl Default for EscapeReassembler {
    fn default() -> Self {
        Self::new()
    }
}

impl EscapeReassembler {
    /// A machine holding nothing.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            held: Vec::new(),
            bytes: Vec::new(),
            paste: String::new(),
        }
    }

    /// Whether events are currently being held. The reader thread uses this to shorten its poll
    /// timeout so a held `Esc` is released after one short tick rather than a full idle interval.
    #[must_use]
    pub fn is_holding(&self) -> bool {
        !self.held.is_empty()
    }

    /// Feed one crossterm event, appending whatever should be forwarded to `out`.
    pub fn push(&mut self, ev: Event, out: &mut Vec<Event>) {
        match self.step(ev) {
            Step::Forward(ev) => out.push(ev),
            Step::Hold => {}
            Step::Emit(ev) => {
                self.disarm();
                out.push(ev);
            }
            Step::Drop => self.disarm(),
            Step::ReplayHeld => self.replay(out),
            Step::Replay(ev) => {
                self.replay(out);
                out.push(ev);
            }
            Step::ReplayThenRetry(ev) => {
                self.replay(out);
                // Re-offer to a now-idle machine: `Esc` `Esc` must release the first and hold the
                // second, not drop either.
                match self.step(ev) {
                    Step::Forward(ev) | Step::Replay(ev) | Step::ReplayThenRetry(ev) => {
                        self.replay(out);
                        out.push(ev);
                    }
                    Step::Emit(ev) => {
                        self.disarm();
                        out.push(ev);
                    }
                    Step::Drop => self.disarm(),
                    Step::Hold => {}
                    Step::ReplayHeld => self.replay(out),
                }
            }
        }
    }

    /// Release everything held — the reader thread calls this when input goes idle. This is what
    /// makes a lone `Escape` press reach the app, and it is the only reason the hold is invisible.
    pub fn flush(&mut self, out: &mut Vec<Event>) {
        self.replay(out);
    }

    /// Drop the held prefix because it was consumed by a successful decode.
    fn disarm(&mut self) {
        self.held.clear();
        self.bytes.clear();
        self.paste.clear();
        self.state = State::Idle;
    }

    /// Give the held prefix back, in order, and disarm. Never drops anything.
    fn replay(&mut self, out: &mut Vec<Event>) {
        out.append(&mut self.held);
        self.bytes.clear();
        self.paste.clear();
        self.state = State::Idle;
    }

    fn step(&mut self, ev: Event) -> Step {
        let Event::Key(key) = ev else {
            // Resize / focus / an already-assembled paste can never be part of a split sequence.
            return if self.state == State::Idle {
                Step::Forward(ev)
            } else {
                Step::Replay(ev)
            };
        };
        // Release/Repeat reports cannot be part of a split sequence's tail either.
        if key.kind != KeyEventKind::Press {
            return if self.state == State::Idle {
                Step::Forward(Event::Key(key))
            } else {
                Step::Replay(Event::Key(key))
            };
        }
        let cap = if matches!(self.state, State::PasteBody | State::PasteEnd(_)) {
            MAX_PASTE_HELD
        } else {
            MAX_HELD
        };
        if self.held.len() >= cap {
            return Step::ReplayThenRetry(Event::Key(key));
        }
        match self.state {
            State::Idle => self.step_idle(key),
            State::Esc => self.step_esc(key),
            State::Csi => self.step_csi(key),
            State::Ss3 => self.step_ss3(key),
            State::PasteBody => self.step_paste_body(key),
            State::PasteEnd(n) => self.step_paste_end(n, key),
        }
    }

    fn step_idle(&mut self, key: KeyEvent) -> Step {
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.state = State::Esc;
            self.bytes.clear();
            self.bytes.push(0x1b);
            self.held.push(Event::Key(key));
            return Step::Hold;
        }
        Step::Forward(Event::Key(key))
    }

    fn step_esc(&mut self, key: KeyEvent) -> Step {
        match sequence_byte(key) {
            // CSI — `ESC [`, which covers every arrow/function/Kitty key and bracketed paste.
            Some(b'[') => {
                self.state = State::Csi;
                self.bytes.push(b'[');
                self.held.push(Event::Key(key));
                Step::Hold
            }
            // SS3 — `ESC O`, the application-cursor-key form.
            Some(b'O') => {
                self.state = State::Ss3;
                self.bytes.push(b'O');
                self.held.push(Event::Key(key));
                Step::Hold
            }
            // `]`, `P`, `_` belong to `stray_reply` / TUI-047; everything else is real typing.
            _ => Step::ReplayThenRetry(Event::Key(key)),
        }
    }

    fn step_csi(&mut self, key: KeyEvent) -> Step {
        let Some(b) = sequence_byte(key) else {
            return Step::ReplayThenRetry(Event::Key(key));
        };
        self.bytes.push(b);
        self.held.push(Event::Key(key));

        if self.bytes == PASTE_START {
            self.state = State::PasteBody;
            self.paste.clear();
            return Step::Hold;
        }
        if !is_complete_csi(&self.bytes) {
            return Step::Hold;
        }
        match decode_csi(&self.bytes) {
            // A key: emit exactly what an unsplit read would have produced.
            Ok(Some(ev)) => Step::Emit(ev),
            // A terminal reply or a mouse report: crossterm would have consumed it internally or
            // `map_event_on` would have dropped it, so dropping it here is the same observable.
            Ok(None) => Step::Drop,
            Err(()) => Step::ReplayHeld,
        }
    }

    fn step_ss3(&mut self, key: KeyEvent) -> Step {
        let Some(b) = sequence_byte(key) else {
            return Step::ReplayThenRetry(Event::Key(key));
        };
        self.bytes.push(b);
        self.held.push(Event::Key(key));
        match decode_ss3(b) {
            Some(ev) => Step::Emit(ev),
            None => Step::ReplayHeld,
        }
    }

    fn step_paste_body(&mut self, key: KeyEvent) -> Step {
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            self.state = State::PasteEnd(0);
            self.held.push(Event::Key(key));
            return Step::Hold;
        }
        let Some(c) = paste_char(key) else {
            return Step::ReplayThenRetry(Event::Key(key));
        };
        self.paste.push(c);
        self.held.push(Event::Key(key));
        Step::Hold
    }

    fn step_paste_end(&mut self, matched: u8, key: KeyEvent) -> Step {
        let expected = PASTE_END_TAIL.get(usize::from(matched)).copied();
        if key.modifiers == KeyModifiers::NONE && Some(key.code) == expected.map(KeyCode::Char) {
            self.held.push(Event::Key(key));
            let next = matched.saturating_add(1);
            if usize::from(next) == PASTE_END_TAIL.len() {
                return Step::Emit(Event::Paste(std::mem::take(&mut self.paste)));
            }
            self.state = State::PasteEnd(next);
            return Step::Hold;
        }
        // Not the closing marker after all — the `ESC` and whatever matched were pasted content.
        self.paste.push('\x1b');
        for c in PASTE_END_TAIL.iter().take(usize::from(matched)) {
            self.paste.push(*c);
        }
        self.state = State::PasteBody;
        self.step_paste_body(key)
    }
}

/// The outcome of one transition.
enum Step {
    /// Nothing is held; forward this event as-is.
    Forward(Event),
    /// The event joined the held prefix.
    Hold,
    /// The held prefix reassembled into this event — discard the prefix and emit it.
    Emit(Event),
    /// The held prefix reassembled into a complete sequence crossterm would not have surfaced as an
    /// event at all (a mouse report, a cursor-position/DSR/DA reply) — discard it and emit nothing.
    Drop,
    /// The held prefix (which already includes the current event) was not a sequence: replay it.
    ReplayHeld,
    /// The held prefix was not a sequence: replay it, then this event.
    Replay(Event),
    /// The held prefix was not a sequence, and this event might start a new one: replay the prefix,
    /// then re-offer this event to an idle machine.
    ReplayThenRetry(Event),
}

/// The raw byte a crossterm key event carries when it is part of an escape sequence's tail.
///
/// Every byte of a CSI/SS3 tail is printable ASCII, which crossterm decodes with
/// `char_code_to_event` (`parse.rs:129-135`) — `KeyCode::Char(c)`, with `SHIFT` set iff `c` is
/// uppercase. Anything else (a control byte, a non-ASCII character, a modifier chord) cannot be
/// sequence tail and aborts the reassembly.
fn sequence_byte(key: KeyEvent) -> Option<u8> {
    let KeyCode::Char(c) = key.code else {
        return None;
    };
    if !c.is_ascii() {
        return None;
    }
    let expected = if c.is_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    if key.modifiers != expected {
        return None;
    }
    u8::try_from(u32::from(c)).ok()
}

/// The character a crossterm key event carries when it is part of a **paste body**.
///
/// Unlike a sequence tail this can be any byte, so the inverse of crossterm's C0 arms
/// (`parse.rs:92-117`) is applied: `\r` arrives as `Enter`, `\t` as `Tab`, `0x7F` as `Backspace`,
/// `0x01..=0x1A` as `Ctrl+a..z` and `0x1C..=0x1F` as `Ctrl+4..7`. `ESC` is handled by the caller
/// because it may open the closing marker.
fn paste_char(key: KeyEvent) -> Option<char> {
    match (key.code, key.modifiers) {
        (KeyCode::Enter, KeyModifiers::NONE) => Some('\r'),
        (KeyCode::Tab, KeyModifiers::NONE) => Some('\t'),
        (KeyCode::Backspace, KeyModifiers::NONE) => Some('\u{7f}'),
        (KeyCode::Char(' '), KeyModifiers::CONTROL) => Some('\0'),
        (KeyCode::Char(c), KeyModifiers::CONTROL) if c.is_ascii_lowercase() => {
            char::from_u32(u32::from(c as u8 - b'a' + 1))
        }
        (KeyCode::Char(c), KeyModifiers::CONTROL) if ('4'..='7').contains(&c) => {
            char::from_u32(u32::from(c as u8 - b'4' + 0x1c))
        }
        (KeyCode::Char(c), KeyModifiers::NONE) if !c.is_control() => Some(c),
        (KeyCode::Char(c), KeyModifiers::SHIFT) if c.is_uppercase() => Some(c),
        _ => None,
    }
}

/// Is this CSI buffer a complete sequence? Port of Pi's `isCompleteCsiSequence`
/// (`stdin-buffer.ts:84-126`) plus the old-style-mouse rule from its caller (`:43-46`).
#[allow(clippy::indexing_slicing)] // every slice is guarded by the length checks above it
fn is_complete_csi(buf: &[u8]) -> bool {
    if !buf.starts_with(b"\x1b[") {
        return true;
    }
    // `ESC [ M` + 3 bytes = 6 total (`stdin-buffer.ts:43-46`).
    if buf.starts_with(b"\x1b[M") {
        return buf.len() >= 6;
    }
    if buf.len() < 3 {
        return false;
    }
    let payload = &buf[2..];
    let Some(&last) = payload.last() else {
        return false;
    };
    if !(0x40..=0x7e).contains(&last) {
        return false;
    }
    // SGR mouse: `ESC [ < digits ; digits ; digits [Mm]` (`stdin-buffer.ts:102-120`). A final byte
    // in range is not enough — the shape has to match, because `;` and digits are in no final-byte
    // range but `<`'s payload can contain characters that are.
    if payload.first() == Some(&b'<') {
        if last != b'M' && last != b'm' {
            return false;
        }
        let inner = &payload[1..payload.len() - 1];
        let Ok(inner) = std::str::from_utf8(inner) else {
            return false;
        };
        let parts: Vec<&str> = inner.split(';').collect();
        return parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    }
    true
}

/// Decode a complete CSI sequence into the event crossterm's `parse_csi` (`parse.rs:137-214`) would
/// have produced for the same bytes delivered in one read.
///
/// `Ok(None)` means "crossterm would not have surfaced a key here" — a mouse report (cyrup enables
/// no mouse reporting and `map_event_on` drops `Event::Mouse`), a cursor-position report, a
/// keyboard-enhancement-flags reply or a device-attributes reply, all of which crossterm consumes as
/// internal events. `Err(())` means the bytes are not a sequence at all and must be replayed.
#[allow(clippy::indexing_slicing)] // every index is guarded by an explicit length check
fn decode_csi(buf: &[u8]) -> Result<Option<Event>, ()> {
    if !buf.starts_with(b"\x1b[") || buf.len() < 3 {
        return Err(());
    }
    let key = |code: KeyCode| Ok(Some(Event::Key(KeyEvent::from(code))));
    match buf[2] {
        b'[' => {
            if buf.len() < 4 {
                return Err(());
            }
            match buf[3] {
                val @ b'A'..=b'E' => key(KeyCode::F(1 + val - b'A')),
                _ => Err(()),
            }
        }
        b'D' => key(KeyCode::Left),
        b'C' => key(KeyCode::Right),
        b'A' => key(KeyCode::Up),
        b'B' => key(KeyCode::Down),
        b'H' => key(KeyCode::Home),
        b'F' => key(KeyCode::End),
        b'Z' => Ok(Some(Event::Key(KeyEvent::new_with_kind(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
            KeyEventKind::Press,
        )))),
        // Mouse reports — normal, SGR and rxvt. Dropped, see the doc comment.
        b'M' | b'<' => Ok(None),
        b'I' => Ok(Some(Event::FocusGained)),
        b'O' => Ok(Some(Event::FocusLost)),
        b';' => decode_csi_modifier_key_code(buf),
        // Kitty legacy functional keys omit the `1` when no modifier is held.
        b'P' => key(KeyCode::F(1)),
        b'Q' => key(KeyCode::F(2)),
        b'S' => key(KeyCode::F(4)),
        // `CSI ? … u` (keyboard flags) and `CSI ? … c` (device attributes) are internal events.
        b'?' => Ok(None),
        b'0'..=b'9' => match buf[buf.len() - 1] {
            // rxvt mouse and a cursor-position report are both internal.
            b'M' | b'R' => Ok(None),
            b'~' => decode_csi_special_key_code(buf),
            b'u' => decode_csi_u_encoded_key_code(buf),
            _ => decode_csi_modifier_key_code(buf),
        },
        _ => Err(()),
    }
}

/// `ESC O <final>` — crossterm's SS3 arm (`parse.rs:45-72`).
fn decode_ss3(final_byte: u8) -> Option<Event> {
    let code = match final_byte {
        b'D' => KeyCode::Left,
        b'C' => KeyCode::Right,
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'H' => KeyCode::Home,
        b'F' => KeyCode::End,
        val @ b'P'..=b'S' => KeyCode::F(1 + val - b'P'),
        _ => return None,
    };
    Some(Event::Key(KeyEvent::from(code)))
}

/// `parse_modifiers` (`parse.rs:303-325`).
fn parse_modifiers(mask: u8) -> KeyModifiers {
    let m = mask.saturating_sub(1);
    let mut out = KeyModifiers::empty();
    if m & 1 != 0 {
        out |= KeyModifiers::SHIFT;
    }
    if m & 2 != 0 {
        out |= KeyModifiers::ALT;
    }
    if m & 4 != 0 {
        out |= KeyModifiers::CONTROL;
    }
    if m & 8 != 0 {
        out |= KeyModifiers::SUPER;
    }
    if m & 16 != 0 {
        out |= KeyModifiers::HYPER;
    }
    if m & 32 != 0 {
        out |= KeyModifiers::META;
    }
    out
}

/// `parse_modifiers_to_state` (`parse.rs:327-337`).
fn parse_modifiers_to_state(mask: u8) -> KeyEventState {
    let m = mask.saturating_sub(1);
    let mut state = KeyEventState::empty();
    if m & 64 != 0 {
        state |= KeyEventState::CAPS_LOCK;
    }
    if m & 128 != 0 {
        state |= KeyEventState::NUM_LOCK;
    }
    state
}

/// `parse_key_event_kind` (`parse.rs:339-346`).
fn parse_key_event_kind(kind: u8) -> KeyEventKind {
    match kind {
        2 => KeyEventKind::Repeat,
        3 => KeyEventKind::Release,
        _ => KeyEventKind::Press,
    }
}

/// `modifier_and_kind_parsed` (`parse.rs:226-239`).
fn modifier_and_kind_parsed<'a>(iter: &mut impl Iterator<Item = &'a str>) -> Option<(u8, u8)> {
    let mut sub = iter.next()?.split(':');
    let mask = sub.next()?.parse::<u8>().ok()?;
    let kind = sub.next().and_then(|k| k.parse::<u8>().ok()).unwrap_or(1);
    Some((mask, kind))
}

/// `parse_csi_modifier_key_code` (`parse.rs:348-393`).
#[allow(clippy::indexing_slicing)] // callers guarantee `buf.len() >= 3`
fn decode_csi_modifier_key_code(buf: &[u8]) -> Result<Option<Event>, ()> {
    let s = std::str::from_utf8(&buf[2..buf.len() - 1]).map_err(|_| ())?;
    let mut split = s.split(';');
    split.next();

    let (modifiers, kind) = if let Some((mask, kind_code)) = modifier_and_kind_parsed(&mut split) {
        (parse_modifiers(mask), parse_key_event_kind(kind_code))
    } else if buf.len() > 3 {
        let digit = char::from(buf[buf.len() - 2]).to_digit(10).ok_or(())?;
        (
            parse_modifiers(u8::try_from(digit).map_err(|_| ())?),
            KeyEventKind::Press,
        )
    } else {
        (KeyModifiers::NONE, KeyEventKind::Press)
    };

    let code = match buf[buf.len() - 1] {
        b'A' => KeyCode::Up,
        b'B' => KeyCode::Down,
        b'C' => KeyCode::Right,
        b'D' => KeyCode::Left,
        b'F' => KeyCode::End,
        b'H' => KeyCode::Home,
        b'P' => KeyCode::F(1),
        b'Q' => KeyCode::F(2),
        b'R' => KeyCode::F(3),
        b'S' => KeyCode::F(4),
        _ => return Err(()),
    };
    Ok(Some(Event::Key(KeyEvent::new_with_kind(
        code, modifiers, kind,
    ))))
}

/// `parse_csi_special_key_code` (`parse.rs:619-660`).
#[allow(clippy::indexing_slicing)] // callers guarantee `buf.len() >= 3`
fn decode_csi_special_key_code(buf: &[u8]) -> Result<Option<Event>, ()> {
    let s = std::str::from_utf8(&buf[2..buf.len() - 1]).map_err(|_| ())?;
    let mut split = s.split(';');
    let first = split.next().ok_or(())?.parse::<u8>().map_err(|_| ())?;

    let (modifiers, kind, state) =
        if let Some((mask, kind_code)) = modifier_and_kind_parsed(&mut split) {
            (
                parse_modifiers(mask),
                parse_key_event_kind(kind_code),
                parse_modifiers_to_state(mask),
            )
        } else {
            (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE)
        };

    let code = match first {
        1 | 7 => KeyCode::Home,
        2 => KeyCode::Insert,
        3 => KeyCode::Delete,
        4 | 8 => KeyCode::End,
        5 => KeyCode::PageUp,
        6 => KeyCode::PageDown,
        v @ 11..=15 => KeyCode::F(v - 10),
        v @ 17..=21 => KeyCode::F(v - 11),
        v @ 23..=26 => KeyCode::F(v - 12),
        v @ 28..=29 => KeyCode::F(v - 15),
        v @ 31..=34 => KeyCode::F(v - 17),
        _ => return Err(()),
    };
    Ok(Some(Event::Key(KeyEvent::new_with_kind_and_state(
        code, modifiers, kind, state,
    ))))
}

/// `translate_functional_key_code` (`parse.rs:396-495`).
fn translate_functional_key_code(codepoint: u32) -> Option<(KeyCode, KeyEventState)> {
    let keypad = match codepoint {
        57399..=57408 => Some(KeyCode::Char(
            char::from_u32(u32::from(b'0') + (codepoint - 57399)).unwrap_or('0'),
        )),
        57409 => Some(KeyCode::Char('.')),
        57410 => Some(KeyCode::Char('/')),
        57411 => Some(KeyCode::Char('*')),
        57412 => Some(KeyCode::Char('-')),
        57413 => Some(KeyCode::Char('+')),
        57414 => Some(KeyCode::Enter),
        57415 => Some(KeyCode::Char('=')),
        57416 => Some(KeyCode::Char(',')),
        57417 => Some(KeyCode::Left),
        57418 => Some(KeyCode::Right),
        57419 => Some(KeyCode::Up),
        57420 => Some(KeyCode::Down),
        57421 => Some(KeyCode::PageUp),
        57422 => Some(KeyCode::PageDown),
        57423 => Some(KeyCode::Home),
        57424 => Some(KeyCode::End),
        57425 => Some(KeyCode::Insert),
        57426 => Some(KeyCode::Delete),
        57427 => Some(KeyCode::KeypadBegin),
        _ => None,
    };
    if let Some(code) = keypad {
        return Some((code, KeyEventState::KEYPAD));
    }
    let other = match codepoint {
        57358 => Some(KeyCode::CapsLock),
        57359 => Some(KeyCode::ScrollLock),
        57360 => Some(KeyCode::NumLock),
        57361 => Some(KeyCode::PrintScreen),
        57362 => Some(KeyCode::Pause),
        57363 => Some(KeyCode::Menu),
        57376..=57398 => u8::try_from(codepoint - 57376 + 13).ok().map(KeyCode::F),
        57428 => Some(KeyCode::Media(MediaKeyCode::Play)),
        57429 => Some(KeyCode::Media(MediaKeyCode::Pause)),
        57430 => Some(KeyCode::Media(MediaKeyCode::PlayPause)),
        57431 => Some(KeyCode::Media(MediaKeyCode::Reverse)),
        57432 => Some(KeyCode::Media(MediaKeyCode::Stop)),
        57433 => Some(KeyCode::Media(MediaKeyCode::FastForward)),
        57434 => Some(KeyCode::Media(MediaKeyCode::Rewind)),
        57435 => Some(KeyCode::Media(MediaKeyCode::TrackNext)),
        57436 => Some(KeyCode::Media(MediaKeyCode::TrackPrevious)),
        57437 => Some(KeyCode::Media(MediaKeyCode::Record)),
        57438 => Some(KeyCode::Media(MediaKeyCode::LowerVolume)),
        57439 => Some(KeyCode::Media(MediaKeyCode::RaiseVolume)),
        57440 => Some(KeyCode::Media(MediaKeyCode::MuteVolume)),
        57441 => Some(KeyCode::Modifier(ModifierKeyCode::LeftShift)),
        57442 => Some(KeyCode::Modifier(ModifierKeyCode::LeftControl)),
        57443 => Some(KeyCode::Modifier(ModifierKeyCode::LeftAlt)),
        57444 => Some(KeyCode::Modifier(ModifierKeyCode::LeftSuper)),
        57445 => Some(KeyCode::Modifier(ModifierKeyCode::LeftHyper)),
        57446 => Some(KeyCode::Modifier(ModifierKeyCode::LeftMeta)),
        57447 => Some(KeyCode::Modifier(ModifierKeyCode::RightShift)),
        57448 => Some(KeyCode::Modifier(ModifierKeyCode::RightControl)),
        57449 => Some(KeyCode::Modifier(ModifierKeyCode::RightAlt)),
        57450 => Some(KeyCode::Modifier(ModifierKeyCode::RightSuper)),
        57451 => Some(KeyCode::Modifier(ModifierKeyCode::RightHyper)),
        57452 => Some(KeyCode::Modifier(ModifierKeyCode::RightMeta)),
        57453 => Some(KeyCode::Modifier(ModifierKeyCode::IsoLevel3Shift)),
        57454 => Some(KeyCode::Modifier(ModifierKeyCode::IsoLevel5Shift)),
        _ => None,
    };
    other.map(|code| (code, KeyEventState::empty()))
}

/// `parse_csi_u_encoded_key_code` (`parse.rs:497-616`).
///
/// The one arm deliberately not mirrored is crossterm's `'\n' if !is_raw_mode_enabled()`
/// (`parse.rs:552`): this path only ever runs from the reader thread, which exists only after
/// `App::into_stdout` has enabled raw mode, so the guard is always false here.
#[allow(clippy::indexing_slicing)] // callers guarantee `buf.len() >= 3`
fn decode_csi_u_encoded_key_code(buf: &[u8]) -> Result<Option<Event>, ()> {
    let s = std::str::from_utf8(&buf[2..buf.len() - 1]).map_err(|_| ())?;
    let mut split = s.split(';');
    let mut codepoints = split.next().ok_or(())?.split(':');
    let codepoint = codepoints
        .next()
        .ok_or(())?
        .parse::<u32>()
        .map_err(|_| ())?;

    let (mut modifiers, kind, state_from_modifiers) =
        if let Some((mask, kind_code)) = modifier_and_kind_parsed(&mut split) {
            (
                parse_modifiers(mask),
                parse_key_event_kind(kind_code),
                parse_modifiers_to_state(mask),
            )
        } else {
            (KeyModifiers::NONE, KeyEventKind::Press, KeyEventState::NONE)
        };

    let (mut code, state_from_keycode) =
        if let Some((special, state)) = translate_functional_key_code(codepoint) {
            (special, state)
        } else if let Some(c) = char::from_u32(codepoint) {
            let code = match c {
                '\x1b' => KeyCode::Esc,
                '\r' => KeyCode::Enter,
                '\t' => {
                    if modifiers.contains(KeyModifiers::SHIFT) {
                        KeyCode::BackTab
                    } else {
                        KeyCode::Tab
                    }
                }
                '\x7f' => KeyCode::Backspace,
                _ => KeyCode::Char(c),
            };
            (code, KeyEventState::empty())
        } else {
            return Err(());
        };

    if let KeyCode::Modifier(m) = code {
        match m {
            ModifierKeyCode::LeftAlt | ModifierKeyCode::RightAlt => {
                modifiers.set(KeyModifiers::ALT, true);
            }
            ModifierKeyCode::LeftControl | ModifierKeyCode::RightControl => {
                modifiers.set(KeyModifiers::CONTROL, true);
            }
            ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift => {
                modifiers.set(KeyModifiers::SHIFT, true);
            }
            ModifierKeyCode::LeftSuper | ModifierKeyCode::RightSuper => {
                modifiers.set(KeyModifiers::SUPER, true);
            }
            ModifierKeyCode::LeftHyper | ModifierKeyCode::RightHyper => {
                modifiers.set(KeyModifiers::HYPER, true);
            }
            ModifierKeyCode::LeftMeta | ModifierKeyCode::RightMeta => {
                modifiers.set(KeyModifiers::META, true);
            }
            _ => {}
        }
    }

    if modifiers.contains(KeyModifiers::SHIFT)
        && let Some(shifted) = codepoints
            .next()
            .and_then(|c| c.parse::<u32>().ok())
            .and_then(char::from_u32)
    {
        code = KeyCode::Char(shifted);
        modifiers.set(KeyModifiers::SHIFT, false);
    }

    Ok(Some(Event::Key(KeyEvent::new_with_kind_and_state(
        code,
        modifiers,
        kind,
        state_from_keycode | state_from_modifiers,
    ))))
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

    fn key(code: KeyCode, mods: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, mods))
    }
    fn esc() -> Event {
        key(KeyCode::Esc, KeyModifiers::NONE)
    }
    /// Exactly what crossterm produces for one raw printable byte: `char_code_to_event`
    /// (`parse.rs:129-135`) sets `SHIFT` iff the character is uppercase.
    fn byte(b: u8) -> Event {
        let c = char::from(b);
        let mods = if c.is_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        key(KeyCode::Char(c), mods)
    }
    /// The event stream crossterm produces for a sequence whose leading `ESC` arrived in the
    /// PREVIOUS read: a lone `Esc`, then the tail decoded byte-by-byte as ordinary characters.
    fn split_at_esc(tail: &str) -> Vec<Event> {
        let mut v = vec![esc()];
        v.extend(tail.bytes().map(byte));
        v
    }

    /// Feed a whole burst with no idle gap, then the reader thread's idle flush.
    fn run(events: Vec<Event>) -> Vec<Event> {
        let mut r = EscapeReassembler::new();
        let mut out = Vec::new();
        for ev in events {
            r.push(ev, &mut out);
        }
        r.flush(&mut out);
        out
    }

    /// The characters a user would see typed into the prompt.
    fn typed(out: &[Event]) -> String {
        out.iter()
            .filter_map(|e| match e {
                Event::Key(KeyEvent {
                    code: KeyCode::Char(c),
                    ..
                }) => Some(*c),
                _ => None,
            })
            .collect()
    }

    // ------------------------------------------------------------------ TUI-045, the repro ----

    /// The exact live reproduction from `REPRO-LOG.md`: `tmux send-keys -H 1b`, 60 ms, then
    /// `5b 41`. RED before this module existed — the assertion below produced
    /// `[Esc, Char('['), Char('A')]`, which at idle types `[A` into the prompt and mid-stream
    /// aborts the running turn.
    #[test]
    fn an_arrow_key_split_at_the_esc_byte_arrives_as_one_up_press() {
        let out = run(split_at_esc("[A"));
        assert_eq!(
            out,
            vec![key(KeyCode::Up, KeyModifiers::NONE)],
            "split Up must reassemble; user would otherwise see {:?} plus a bare Escape",
            typed(&out)
        );
    }

    #[test]
    fn every_split_cursor_key_reassembles_to_the_key_an_unsplit_read_would_give() {
        for (tail, code) in [
            ("[A", KeyCode::Up),
            ("[B", KeyCode::Down),
            ("[C", KeyCode::Right),
            ("[D", KeyCode::Left),
            ("[H", KeyCode::Home),
            ("[F", KeyCode::End),
            ("OA", KeyCode::Up),
            ("OB", KeyCode::Down),
            ("OC", KeyCode::Right),
            ("OD", KeyCode::Left),
            ("OH", KeyCode::Home),
            ("OF", KeyCode::End),
        ] {
            assert_eq!(
                run(split_at_esc(tail)),
                vec![key(code, KeyModifiers::NONE)],
                "ESC-split {tail:?} must decode to {code:?}"
            );
        }
    }

    #[test]
    fn split_special_and_modified_keys_reassemble() {
        assert_eq!(
            run(split_at_esc("[3~")),
            vec![key(KeyCode::Delete, KeyModifiers::NONE)]
        );
        assert_eq!(
            run(split_at_esc("[5~")),
            vec![key(KeyCode::PageUp, KeyModifiers::NONE)]
        );
        assert_eq!(
            run(split_at_esc("[6~")),
            vec![key(KeyCode::PageDown, KeyModifiers::NONE)]
        );
        assert_eq!(
            run(split_at_esc("[15~")),
            vec![key(KeyCode::F(5), KeyModifiers::NONE)]
        );
        // Ctrl+Left — the modifier form the editor's word-motion bindings use.
        assert_eq!(
            run(split_at_esc("[1;5D")),
            vec![key(KeyCode::Left, KeyModifiers::CONTROL)]
        );
        assert_eq!(
            run(split_at_esc("[1;3A")),
            vec![key(KeyCode::Up, KeyModifiers::ALT)]
        );
        // Shift+Tab.
        assert_eq!(
            run(split_at_esc("[Z")),
            vec![key(KeyCode::BackTab, KeyModifiers::SHIFT)]
        );
    }

    #[test]
    fn a_split_kitty_csi_u_sequence_reassembles() {
        // `CSI 27 u` — Escape under DISAMBIGUATE_ESCAPE_CODES, the flag cyrup pushes.
        assert_eq!(
            run(split_at_esc("[27u")),
            vec![key(KeyCode::Esc, KeyModifiers::NONE)]
        );
        // `CSI 97 ; 5 u` — Ctrl+a.
        assert_eq!(
            run(split_at_esc("[97;5u")),
            vec![key(KeyCode::Char('a'), KeyModifiers::CONTROL)]
        );
        // A functional keypad code plus its KEYPAD state bit.
        assert_eq!(
            run(split_at_esc("[57419u")),
            vec![Event::Key(KeyEvent::new_with_kind_and_state(
                KeyCode::Up,
                KeyModifiers::NONE,
                KeyEventKind::Press,
                KeyEventState::KEYPAD,
            ))]
        );
    }

    #[test]
    fn a_split_bracketed_paste_is_reassembled_into_one_paste_event() {
        let mut v = split_at_esc("[200~hello world");
        v.push(esc());
        v.extend("[201~".bytes().map(byte));
        assert_eq!(run(v), vec![Event::Paste("hello world".into())]);
    }

    #[test]
    fn a_split_paste_keeps_newlines_tabs_and_an_embedded_escape() {
        let mut v = split_at_esc("[200~a");
        // crossterm turns the body's `\r` into `Enter` and `\t` into `Tab`.
        v.push(key(KeyCode::Enter, KeyModifiers::NONE));
        v.push(key(KeyCode::Tab, KeyModifiers::NONE));
        // An `ESC` inside the body that is NOT the closing marker.
        v.push(esc());
        v.extend("[20x".bytes().map(byte));
        v.push(esc());
        v.extend("[201~".bytes().map(byte));
        assert_eq!(run(v), vec![Event::Paste("a\r\t\u{1b}[20x".into())]);
    }

    #[test]
    fn a_split_terminal_reply_is_dropped_rather_than_typed() {
        // A late DSR colour-scheme report split at the ESC: `CSI ? 997 ; 1 n`.
        assert_eq!(run(split_at_esc("[?997;1n")), vec![]);
        // And a cursor-position report.
        assert_eq!(run(split_at_esc("[12;34R")), vec![]);
    }

    // --------------------------------------------------------------- keystroke safety ----

    #[test]
    fn a_lone_escape_press_is_still_delivered() {
        assert_eq!(
            run(vec![esc()]),
            vec![esc()],
            "a lone Escape must reach the app"
        );
    }

    #[test]
    fn a_double_escape_releases_the_first_and_holds_the_second() {
        assert_eq!(run(vec![esc(), esc()]), vec![esc(), esc()]);
        assert_eq!(run(vec![esc(), esc(), esc()]), vec![esc(), esc(), esc()]);
    }

    #[test]
    fn escape_then_ordinary_typing_is_delivered_unchanged() {
        // The meta-key case this module deliberately does not claim: `Esc` then a character is
        // NOT folded into an Alt chord, because a real Escape press followed by fast typing is
        // far more common than a split meta sequence.
        assert_eq!(run(vec![esc(), byte(b'x')]), vec![esc(), byte(b'x')]);
        assert_eq!(
            run(vec![esc(), byte(b'a'), byte(b'b')]),
            vec![esc(), byte(b'a'), byte(b'b')]
        );
    }

    #[test]
    fn escape_then_an_unterminated_csi_prefix_is_replayed_in_order() {
        let burst = vec![esc(), byte(b'['), byte(b'1'), byte(b';')];
        assert_eq!(
            run(burst.clone()),
            burst,
            "an unterminated prefix must be replayed whole"
        );
        // And a CSI whose final byte is not decodable is replayed rather than eaten.
        let burst = vec![
            esc(),
            byte(b'['),
            byte(b'9'),
            byte(b'9'),
            byte(b'9'),
            byte(b'~'),
        ];
        assert_eq!(run(burst.clone()), burst);
        // SS3 with a final byte that is not a cursor key.
        let burst = vec![esc(), byte(b'O'), byte(b'x')];
        assert_eq!(run(burst.clone()), burst);
    }

    #[test]
    fn the_osc_dcs_and_apc_introducers_are_left_to_their_own_machines() {
        // `stray_reply` owns `ESC ]`; TUI-047 owns `ESC P` / `ESC _`. All three must pass through
        // this module byte-for-byte, or that machine would never see them.
        for intro in *b"]P_" {
            let burst = vec![esc(), byte(intro), byte(b'1')];
            assert_eq!(
                run(burst.clone()),
                burst,
                "introducer {} must pass through",
                intro as char
            );
        }
    }

    #[test]
    fn ordinary_typing_is_untouched() {
        let burst: Vec<Event> = "the quick brown fox; jumps over 11 lazy dogs -- 1;2;3 [x] {y} ]z]"
            .bytes()
            .map(byte)
            .collect();
        assert_eq!(run(burst.clone()), burst);
    }

    #[test]
    fn a_non_key_event_mid_hold_replays_the_prefix_and_disarms() {
        for interrupt in [
            Event::Resize(80, 24),
            Event::Paste("hi".into()),
            Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            )),
        ] {
            let mut r = EscapeReassembler::new();
            let mut out = Vec::new();
            for ev in [esc(), byte(b'['), byte(b'1')] {
                r.push(ev, &mut out);
            }
            assert!(out.is_empty(), "the prefix is held, not forwarded");
            r.push(interrupt.clone(), &mut out);
            assert_eq!(out, vec![esc(), byte(b'['), byte(b'1'), interrupt.clone()]);
            assert_eq!(
                (r.state, r.held.as_slice()),
                (State::Idle, [].as_slice()),
                "after replaying, the machine must be disarmed rather than armed-and-empty"
            );
            // The observable consequence: a bare final byte afterwards is real typing.
            out.clear();
            r.push(byte(b'A'), &mut out);
            r.flush(&mut out);
            assert_eq!(out, vec![byte(b'A')]);
        }
    }

    #[test]
    fn the_hold_is_bounded_and_gives_up_by_replaying_everything() {
        let mut burst = vec![esc(), byte(b'[')];
        // Every one of these is a legal CSI parameter byte, so only the cap can stop it.
        burst.extend(std::iter::repeat_n(byte(b'1'), MAX_HELD * 3));
        assert_eq!(
            run(burst.clone()),
            burst,
            "every held event must be replayed at the cap"
        );

        // And the machine recovers: a real split sequence right after is still reassembled.
        let mut r = EscapeReassembler::new();
        let mut out = Vec::new();
        for ev in burst {
            r.push(ev, &mut out);
        }
        out.clear();
        for ev in split_at_esc("[A") {
            r.push(ev, &mut out);
        }
        assert_eq!(out, vec![key(KeyCode::Up, KeyModifiers::NONE)]);
    }

    #[test]
    fn holding_never_exceeds_the_cap() {
        let mut r = EscapeReassembler::new();
        let mut out = Vec::new();
        r.push(esc(), &mut out);
        r.push(byte(b'['), &mut out);
        for _ in 0..1000 {
            r.push(byte(b'1'), &mut out);
            assert!(
                r.held.len() <= MAX_HELD,
                "hold must stay bounded: {}",
                r.held.len()
            );
        }
    }

    #[test]
    fn an_idle_gap_mid_sequence_releases_rather_than_eats() {
        // Degrading to the OLD behaviour is the safe direction; losing the keys is not.
        let mut r = EscapeReassembler::new();
        let mut out = Vec::new();
        r.push(esc(), &mut out);
        r.push(byte(b'['), &mut out);
        r.flush(&mut out);
        assert_eq!(out, vec![esc(), byte(b'[')]);
    }

    #[test]
    fn every_event_of_a_non_matching_burst_is_accounted_for() {
        // Property-ish sweep: no 3-event burst that cannot complete a sequence may be altered.
        let alphabet = [
            esc(),
            byte(b'x'),
            byte(b'1'),
            byte(b';'),
            byte(b']'),
            byte(b'_'),
            byte(b'\\'),
        ];
        for i in 0..alphabet.len() {
            for j in 0..alphabet.len() {
                for k in 0..alphabet.len() {
                    let burst = vec![
                        alphabet[i].clone(),
                        alphabet[j].clone(),
                        alphabet[k].clone(),
                    ];
                    assert_eq!(
                        run(burst.clone()),
                        burst,
                        "burst {burst:?} must be unaltered"
                    );
                }
            }
        }
    }
}
