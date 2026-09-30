//! One [`Frame`] to one crossterm [`Event`].
//!
//! The rules are crossterm 0.29's `parse_event` family (`src/event/sys/unix/parse.rs`), so every
//! consumer downstream sees exactly the events it saw when crossterm read the tty itself: the
//! ASCII/C0/Alt/UTF-8 arms (`:26-135`), CSI and SS3 keys (`:137-214`, `:348-660`), the Kitty CSI-u
//! form (`:497-616`), and the three mouse encodings (`:663-812`). The code is MIT-licensed
//! crossterm's, ported; its unit tests are ported with it in `decode_tests.rs`.
//!
//! Where the framer has already decided something crossterm had to guess, the framer wins:
//!
//! - a frame is complete by pi's rules, so crossterm's "wait for more bytes" answers mean the
//!   sequence was flushed incomplete, and it is [`Decoded::Ignored`] — pi hands the same bytes to
//!   its key matcher, which matches nothing;
//! - a terminal reply — a cursor-position report, a DA1 or Kitty-flags reply, a DSR, an OSC, DCS or
//!   APC string — is [`Decoded::Reply`] and reaches no listener. crossterm kept the first three
//!   as internal events; the rest it shredded into keystrokes (`TUI-047`) or, for a `CSI ?` reply
//!   with any final byte but `u`/`c`, held forever and took the next keys down with it;
//! - an introducer flushed alone (`ESC [`, `ESC O`, `ESC ]`, `ESC P`, `ESC _` with nothing after it
//!   inside the sequence timeout) is the `Alt` chord the user typed. crossterm waited for more
//!   bytes there and so dropped the chord together with the key after it.

// Driven by the unix reader; elsewhere only `crate::escape_reassembly` borrows the CSI/SS3 parsers.
#![cfg_attr(not(unix), allow(dead_code))]

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MediaKeyCode,
    ModifierKeyCode, MouseButton, MouseEvent, MouseEventKind,
};

use super::frame::Frame;

/// What a frame means to the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Decoded {
    /// Input: a key, a paste, a mouse report, a focus change.
    Event(Event),
    /// A terminal's answer to a query, delivered on the input stream. Swallowed — pi's
    /// `handleTerminalInput` never lets one reach an input listener (`tui.ts:788-794`), and a
    /// keystroke parser cannot mistake it for typing once it is one frame.
    Reply,
    /// Bytes that mean nothing: a sequence flushed incomplete, or one no parser knows.
    Ignored,
}

/// Decode one frame.
pub(crate) fn decode(frame: Frame) -> Decoded {
    match frame {
        Frame::Char(c) => Decoded::Event(Event::Key(char_key(c))),
        // An 8-bit meta key is `Alt` plus the 7-bit character (`TUI-050`). `ESC` + that character
        // would decode the same way except for the introducers (`0xDB` is `Alt+[`, not a CSI), so
        // it is built directly.
        Frame::Meta(byte) => {
            let c = char::from(byte & 0x7f);
            if c == '\x1b' {
                // crossterm decodes `ESC ESC` as a plain Escape (`parse.rs:77`).
                return Decoded::Event(Event::Key(KeyCode::Esc.into()));
            }
            let mut key = char_key(c);
            key.modifiers |= KeyModifiers::ALT;
            Decoded::Event(Event::Key(key))
        }
        // crossterm's `parse_csi_bracketed_paste` (`parse.rs:815-825`) decodes lossily too.
        Frame::Paste(bytes) => {
            Decoded::Event(Event::Paste(String::from_utf8_lossy(&bytes).into_owned()))
        }
        Frame::Seq(bytes) => decode_seq(&bytes),
    }
}

/// crossterm's non-`ESC` arms for one character (`parse.rs:92-124`) and `char_code_to_event`
/// (`:129-135`).
///
/// `\n` is `Ctrl+J`: crossterm maps it to `Enter` only outside raw mode (`:96-101`), and this
/// reader only runs in raw mode.
fn char_key(c: char) -> KeyEvent {
    match c {
        '\r' => KeyCode::Enter.into(),
        '\t' => KeyCode::Tab.into(),
        '\x7f' => KeyCode::Backspace.into(),
        '\0' => KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL),
        '\x01'..='\x1a' => KeyEvent::new(
            KeyCode::Char(char::from(c as u8 - 0x01 + b'a')),
            KeyModifiers::CONTROL,
        ),
        '\x1c'..='\x1f' => KeyEvent::new(
            KeyCode::Char(char::from(c as u8 - 0x1c + b'4')),
            KeyModifiers::CONTROL,
        ),
        '\x1b' => KeyCode::Esc.into(),
        c if c.is_uppercase() => KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT),
        c => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
    }
}

/// Decode an escape sequence (`bytes` starts with `ESC`).
pub(crate) fn decode_seq(bytes: &[u8]) -> Decoded {
    let key = |k: KeyEvent| Decoded::Event(Event::Key(k));
    let Some(&intro) = bytes.get(1) else {
        return key(KeyCode::Esc.into());
    };
    let alone = bytes.len() == 2;
    match intro {
        b'[' if !alone => match parse_csi(bytes) {
            Ok(Some(decoded)) => decoded,
            Ok(None) | Err(()) => Decoded::Ignored,
        },
        b'O' if !alone => match bytes.get(2..) {
            Some(&[b]) => decode_ss3(b).map_or(Decoded::Ignored, Decoded::Event),
            _ => Decoded::Ignored,
        },
        b']' if !alone => {
            if bytes.ends_with(b"\x07") || bytes.ends_with(b"\x1b\\") {
                Decoded::Reply
            } else {
                Decoded::Ignored
            }
        }
        b'P' | b'_' if !alone => {
            if bytes.ends_with(b"\x1b\\") {
                Decoded::Reply
            } else {
                Decoded::Ignored
            }
        }
        // `ESC ESC` is Escape (`parse.rs:77`).
        0x1b if alone => key(KeyCode::Esc.into()),
        // `ESC` + one character is that character with `Alt` (`parse.rs:78-88`).
        _ => match std::str::from_utf8(bytes.get(1..).unwrap_or_default()) {
            Ok(rest) => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) => {
                        let mut k = char_key(c);
                        k.modifiers |= KeyModifiers::ALT;
                        key(k)
                    }
                    _ => Decoded::Ignored,
                }
            }
            Err(_) => Decoded::Ignored,
        },
    }
}

/// `ESC O <final>` — crossterm's SS3 arm (`parse.rs:45-72`).
pub(crate) fn decode_ss3(final_byte: u8) -> Option<Event> {
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

/// crossterm's `parse_csi` (`parse.rs:137-214`) over a complete frame. `Ok(None)` is crossterm's
/// "wait for more bytes"; `Err(())` its "could not parse".
#[allow(clippy::indexing_slicing)] // every index is guarded by an explicit length check
pub(crate) fn parse_csi(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    if !buf.starts_with(b"\x1b[") || buf.len() < 3 {
        return Ok(None);
    }
    let key = |code: KeyCode| Ok(Some(Decoded::Event(Event::Key(KeyEvent::from(code)))));
    let last = buf[buf.len() - 1];
    match buf[2] {
        b'[' => match buf.get(3) {
            None => Ok(None),
            Some(val @ b'A'..=b'E') => key(KeyCode::F(1 + val - b'A')),
            Some(_) => Err(()),
        },
        b'D' => key(KeyCode::Left),
        b'C' => key(KeyCode::Right),
        b'A' => key(KeyCode::Up),
        b'B' => key(KeyCode::Down),
        b'H' => key(KeyCode::Home),
        b'F' => key(KeyCode::End),
        b'Z' => Ok(Some(Decoded::Event(Event::Key(KeyEvent::new_with_kind(
            KeyCode::BackTab,
            KeyModifiers::SHIFT,
            KeyEventKind::Press,
        ))))),
        b'M' => parse_csi_normal_mouse(buf),
        b'<' => parse_csi_sgr_mouse(buf),
        b'I' => Ok(Some(Decoded::Event(Event::FocusGained))),
        b'O' => Ok(Some(Decoded::Event(Event::FocusLost))),
        b';' => parse_csi_modifier_key_code(buf),
        // Kitty legacy functional keys omit the `1` when no modifier is held.
        b'P' => key(KeyCode::F(1)),
        b'Q' => key(KeyCode::F(2)),
        b'S' => key(KeyCode::F(4)),
        // `CSI ? … u` (Kitty flags), `CSI ? … c` (DA1) and every other private-parameter reply
        // (`CSI ? 997 ; 1 n`, DECRPM `CSI ? … $ y`, DA2 `CSI > … c`, DA3 `CSI = … c`).
        b'?' | b'>' | b'=' => Ok(Some(Decoded::Reply)),
        b'0'..=b'9' => {
            if buf.len() == 3 || !(64..=126).contains(&last) {
                return Ok(None);
            }
            match last {
                b'M' => parse_csi_rxvt_mouse(buf),
                b'~' => parse_csi_special_key_code(buf),
                b'u' => parse_csi_u_encoded_key_code(buf),
                // Cursor-position report (`parse.rs:241-257`), and the DSR, window-ops and
                // DECRPM replies crossterm failed to parse.
                b'R' | b'n' | b't' | b'y' => Ok(Some(Decoded::Reply)),
                _ => parse_csi_modifier_key_code(buf),
            }
        }
        _ => Err(()),
    }
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

/// The text between `ESC [` and the final byte.
#[allow(clippy::indexing_slicing)] // callers guarantee `buf.len() >= 3`
fn params(buf: &[u8]) -> Result<&str, ()> {
    std::str::from_utf8(&buf[2..buf.len() - 1]).map_err(|_| ())
}

/// `next_parsed` (`parse.rs:216-224`).
fn next_parsed<'a, T: std::str::FromStr>(
    iter: &mut impl Iterator<Item = &'a str>,
) -> Result<T, ()> {
    iter.next().ok_or(())?.parse::<T>().map_err(|_| ())
}

/// `parse_csi_modifier_key_code` (`parse.rs:348-393`).
#[allow(clippy::indexing_slicing)] // callers guarantee `buf.len() >= 3`
fn parse_csi_modifier_key_code(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    let mut split = params(buf)?.split(';');
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
    Ok(Some(Decoded::Event(Event::Key(KeyEvent::new_with_kind(
        code, modifiers, kind,
    )))))
}

/// `parse_csi_special_key_code` (`parse.rs:619-660`).
fn parse_csi_special_key_code(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    let mut split = params(buf)?.split(';');
    let first = next_parsed::<u8>(&mut split)?;

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
    Ok(Some(Decoded::Event(Event::Key(
        KeyEvent::new_with_kind_and_state(code, modifiers, kind, state),
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
/// The one arm not mirrored is crossterm's `'\n' if !is_raw_mode_enabled()` (`parse.rs:552`):
/// this decoder only runs under the raw-mode reader, so that guard is always false.
fn parse_csi_u_encoded_key_code(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    let mut split = params(buf)?.split(';');
    let mut codepoints = split.next().ok_or(())?.split(':');
    let codepoint = next_parsed::<u32>(&mut codepoints)?;

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

    // `REPORT_ALTERNATE_KEYS`: the shifted codepoint replaces the base and `SHIFT` is cleared
    // (`parse.rs:593-606`) — why that flag is withheld, see `crate::keyboard_protocol`.
    if modifiers.contains(KeyModifiers::SHIFT)
        && let Some(shifted) = codepoints
            .next()
            .and_then(|c| c.parse::<u32>().ok())
            .and_then(char::from_u32)
    {
        code = KeyCode::Char(shifted);
        modifiers.set(KeyModifiers::SHIFT, false);
    }

    Ok(Some(Decoded::Event(Event::Key(
        KeyEvent::new_with_kind_and_state(
            code,
            modifiers,
            kind,
            state_from_keycode | state_from_modifiers,
        ),
    ))))
}

/// `parse_csi_rxvt_mouse` (`parse.rs:663-688`): `ESC [ Cb ; Cx ; Cy M`.
fn parse_csi_rxvt_mouse(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    let mut split = params(buf)?.split(';');
    let cb = next_parsed::<u8>(&mut split)?.checked_sub(32).ok_or(())?;
    let (kind, modifiers) = parse_cb(cb)?;
    let column = one_based(next_parsed::<u16>(&mut split)?)?;
    let row = one_based(next_parsed::<u16>(&mut split)?)?;
    Ok(Some(mouse(kind, column, row, modifiers)))
}

/// `parse_csi_normal_mouse` (`parse.rs:690-714`): `ESC [ M Cb Cx Cy`, six bytes.
///
/// crossterm computes each coordinate as `u16::from(byte.saturating_sub(32)) - 1`, which
/// underflows (a debug-build panic) on a coordinate byte of 32 or less; here that is a parse
/// failure.
fn parse_csi_normal_mouse(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    let (Some(&cb), Some(&cx), Some(&cy)) = (buf.get(3), buf.get(4), buf.get(5)) else {
        return Ok(None);
    };
    let (kind, modifiers) = parse_cb(cb.checked_sub(32).ok_or(())?)?;
    let column = one_based(u16::from(cx.saturating_sub(32)))?;
    let row = one_based(u16::from(cy.saturating_sub(32)))?;
    Ok(Some(mouse(kind, column, row, modifiers)))
}

/// `parse_csi_sgr_mouse` (`parse.rs:716-760`): `ESC [ < Cb ; Cx ; Cy (;) (M | m)`.
fn parse_csi_sgr_mouse(buf: &[u8]) -> Result<Option<Decoded>, ()> {
    if !buf.ends_with(b"m") && !buf.ends_with(b"M") {
        return Ok(None);
    }
    let inner = buf.get(3..buf.len() - 1).ok_or(())?;
    let mut split = std::str::from_utf8(inner).map_err(|_| ())?.split(';');
    let (kind, modifiers) = parse_cb(next_parsed::<u8>(&mut split)?)?;
    let column = one_based(next_parsed::<u16>(&mut split)?)?;
    let row = one_based(next_parsed::<u16>(&mut split)?)?;
    // A lowercase `m` is a release; SGR is the one encoding that says which button.
    let kind = match (buf.last(), kind) {
        (Some(b'm'), MouseEventKind::Down(button)) => MouseEventKind::Up(button),
        (_, other) => other,
    };
    Ok(Some(mouse(kind, column, row, modifiers)))
}

/// A 1-based terminal coordinate to crossterm's 0-based one.
fn one_based(v: u16) -> Result<u16, ()> {
    v.checked_sub(1).ok_or(())
}

fn mouse(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Decoded {
    Decoded::Event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }))
}

/// `parse_cb` (`parse.rs:775-812`): the button, drag bit and modifiers of a mouse report.
fn parse_cb(cb: u8) -> Result<(MouseEventKind, KeyModifiers), ()> {
    let button_number = (cb & 0b0000_0011) | ((cb & 0b1100_0000) >> 4);
    let dragging = cb & 0b0010_0000 == 0b0010_0000;

    let kind = match (button_number, dragging) {
        (0, false) => MouseEventKind::Down(MouseButton::Left),
        (1, false) => MouseEventKind::Down(MouseButton::Middle),
        (2, false) => MouseEventKind::Down(MouseButton::Right),
        (0, true) => MouseEventKind::Drag(MouseButton::Left),
        (1, true) => MouseEventKind::Drag(MouseButton::Middle),
        (2, true) => MouseEventKind::Drag(MouseButton::Right),
        (3, false) => MouseEventKind::Up(MouseButton::Left),
        (3..=5, true) => MouseEventKind::Moved,
        (4, false) => MouseEventKind::ScrollUp,
        (5, false) => MouseEventKind::ScrollDown,
        (6, false) => MouseEventKind::ScrollLeft,
        (7, false) => MouseEventKind::ScrollRight,
        _ => return Err(()),
    };

    let mut modifiers = KeyModifiers::empty();
    if cb & 0b0000_0100 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if cb & 0b0000_1000 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if cb & 0b0001_0000 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    Ok((kind, modifiers))
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;
