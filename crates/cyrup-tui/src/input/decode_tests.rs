//! The decoder against crossterm 0.29's own `parse.rs` unit tests (MIT;
//! `src/event/sys/unix/parse.rs:866-1506`), ported as byte → event tables, then the replies and
//! lone introducers this decoder answers differently.
//!
//! crossterm tests `parse_event(bytes, false)`; the equivalent here is framing the bytes and
//! decoding the one frame they make. Where crossterm answers with an internal event (a cursor
//! position, the Kitty flags, DA1) the answer here is [`Decoded::Reply`].
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;
use crate::input::frame::{DEFAULT_ESCAPE_TIMEOUT, StdinBuffer};
use std::time::Instant;

/// Frame `bytes` as one read (flushing anything the read leaves held, as the reader's timeout
/// would), then decode every frame.
fn parse(bytes: &[u8]) -> Vec<Decoded> {
    let mut buf = StdinBuffer::new(DEFAULT_ESCAPE_TIMEOUT);
    let mut frames = Vec::new();
    buf.push(bytes, Instant::now(), &mut frames);
    buf.flush(&mut frames);
    frames.into_iter().map(decode).collect()
}

/// One frame's worth of bytes to its one decoded meaning.
fn one(bytes: &[u8]) -> Decoded {
    let mut all = parse(bytes);
    assert_eq!(all.len(), 1, "{bytes:?} framed as {all:?}");
    all.remove(0)
}

fn key(code: KeyCode, mods: KeyModifiers) -> Decoded {
    Decoded::Event(Event::Key(KeyEvent::new(code, mods)))
}

fn key_kind(code: KeyCode, mods: KeyModifiers, kind: KeyEventKind) -> Decoded {
    Decoded::Event(Event::Key(KeyEvent::new_with_kind(code, mods, kind)))
}

fn key_state(code: KeyCode, mods: KeyModifiers, state: KeyEventState) -> Decoded {
    Decoded::Event(Event::Key(KeyEvent::new_with_kind_and_state(
        code,
        mods,
        KeyEventKind::Press,
        state,
    )))
}

fn mouse_ev(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> Decoded {
    Decoded::Event(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }))
}

const NONE: KeyModifiers = KeyModifiers::NONE;
const SHIFT: KeyModifiers = KeyModifiers::SHIFT;
const ALT: KeyModifiers = KeyModifiers::ALT;
const CTRL: KeyModifiers = KeyModifiers::CONTROL;

/// crossterm's `parse_event` tests, one row each (`test_esc_key` through
/// `test_parse_csi_numbered_escape_code_with_types`).
#[test]
fn crossterm_parse_event_corpus() {
    let rows: Vec<(&[u8], Decoded)> = vec![
        // test_esc_key
        (b"\x1B", key(KeyCode::Esc, NONE)),
        // test_alt_key / test_alt_shift / test_alt_ctrl
        (b"\x1Bc", key(KeyCode::Char('c'), ALT)),
        (b"\x1BH", key(KeyCode::Char('H'), ALT | SHIFT)),
        (b"\x1B\x14", key(KeyCode::Char('t'), ALT | CTRL)),
        // test_parse_event_subsequent_calls
        (b"\x1B[20;10R", Decoded::Reply),
        (b"\x1B[D", key(KeyCode::Left, NONE)),
        (b"\x1B[2D", key(KeyCode::Left, SHIFT)),
        (b"\x1B[3~", key(KeyCode::Delete, NONE)),
        (
            b"\x1B[200~on and on and on\x1B[201~",
            Decoded::Event(Event::Paste("on and on and on".into())),
        ),
        (
            b"\x1B[32;30;40;M",
            mouse_ev(MouseEventKind::Down(MouseButton::Left), 29, 39, NONE),
        ),
        (
            b"\x1B[M0\x60\x70",
            mouse_ev(MouseEventKind::Down(MouseButton::Left), 63, 79, CTRL),
        ),
        ("Ž".as_bytes(), key(KeyCode::Char('Ž'), SHIFT)),
        // test_parse_event
        (b"\t", key(KeyCode::Tab, NONE)),
        // test_parse_csi / _modifier_key_code / _special_key_code(_multiple_values_not_supported)
        (b"\x1B[3;2~", key(KeyCode::Delete, SHIFT)),
        // test_parse_csi_focus (and its mirror)
        (b"\x1B[O", Decoded::Event(Event::FocusLost)),
        (b"\x1B[I", Decoded::Event(Event::FocusGained)),
        // test_parse_csi_sgr_mouse
        (
            b"\x1B[<0;20;10M",
            mouse_ev(MouseEventKind::Down(MouseButton::Left), 19, 9, NONE),
        ),
        (
            b"\x1B[<0;20;10m",
            mouse_ev(MouseEventKind::Up(MouseButton::Left), 19, 9, NONE),
        ),
        // test_parse_char_event_lowercase / _uppercase
        (b"c", key(KeyCode::Char('c'), NONE)),
        (b"C", key(KeyCode::Char('C'), SHIFT)),
        // test_parse_basic_csi_u_encoded_key_code
        (b"\x1B[97u", key(KeyCode::Char('a'), NONE)),
        (b"\x1B[97;2u", key(KeyCode::Char('A'), SHIFT)),
        (b"\x1B[97;7u", key(KeyCode::Char('a'), ALT | CTRL)),
        // test_parse_basic_csi_u_encoded_key_code_special_keys
        (b"\x1B[13u", key(KeyCode::Enter, NONE)),
        (b"\x1B[27u", key(KeyCode::Esc, NONE)),
        (b"\x1B[57358u", key(KeyCode::CapsLock, NONE)),
        (b"\x1B[57376u", key(KeyCode::F(13), NONE)),
        (
            b"\x1B[57428u",
            key(KeyCode::Media(MediaKeyCode::Play), NONE),
        ),
        (
            b"\x1B[57441u",
            key(KeyCode::Modifier(ModifierKeyCode::LeftShift), SHIFT),
        ),
        // test_parse_csi_u_encoded_keypad_code
        (
            b"\x1B[57399u",
            key_state(KeyCode::Char('0'), NONE, KeyEventState::KEYPAD),
        ),
        (
            b"\x1B[57419u",
            key_state(KeyCode::Up, NONE, KeyEventState::KEYPAD),
        ),
        // test_parse_csi_u_encoded_key_code_with_types
        (
            b"\x1B[97;1u",
            key_kind(KeyCode::Char('a'), NONE, KeyEventKind::Press),
        ),
        (
            b"\x1B[97;1:1u",
            key_kind(KeyCode::Char('a'), NONE, KeyEventKind::Press),
        ),
        (
            b"\x1B[97;5:1u",
            key_kind(KeyCode::Char('a'), CTRL, KeyEventKind::Press),
        ),
        (
            b"\x1B[97;1:2u",
            key_kind(KeyCode::Char('a'), NONE, KeyEventKind::Repeat),
        ),
        (
            b"\x1B[97;1:3u",
            key_kind(KeyCode::Char('a'), NONE, KeyEventKind::Release),
        ),
        // test_parse_csi_u_encoded_key_code_has_modifier_on_modifier_press
        (
            b"\x1B[57449u",
            key_kind(
                KeyCode::Modifier(ModifierKeyCode::RightAlt),
                ALT,
                KeyEventKind::Press,
            ),
        ),
        (
            b"\x1B[57449;3:3u",
            key_kind(
                KeyCode::Modifier(ModifierKeyCode::RightAlt),
                ALT,
                KeyEventKind::Release,
            ),
        ),
        (
            b"\x1B[57450u",
            key(
                KeyCode::Modifier(ModifierKeyCode::RightSuper),
                KeyModifiers::SUPER,
            ),
        ),
        (
            b"\x1B[57451u",
            key(
                KeyCode::Modifier(ModifierKeyCode::RightHyper),
                KeyModifiers::HYPER,
            ),
        ),
        (
            b"\x1B[57452u",
            key(
                KeyCode::Modifier(ModifierKeyCode::RightMeta),
                KeyModifiers::META,
            ),
        ),
        // test_parse_csi_u_encoded_key_code_with_extra_modifiers
        (b"\x1B[97;9u", key(KeyCode::Char('a'), KeyModifiers::SUPER)),
        (b"\x1B[97;17u", key(KeyCode::Char('a'), KeyModifiers::HYPER)),
        (b"\x1B[97;33u", key(KeyCode::Char('a'), KeyModifiers::META)),
        // test_parse_csi_u_encoded_key_code_with_extra_state
        (
            b"\x1B[97;65u",
            key_state(KeyCode::Char('a'), NONE, KeyEventState::CAPS_LOCK),
        ),
        (
            b"\x1B[49;129u",
            key_state(KeyCode::Char('1'), NONE, KeyEventState::NUM_LOCK),
        ),
        // test_parse_csi_u_with_shifted_keycode
        (b"\x1B[57:40;4u", key(KeyCode::Char('('), ALT)),
        (b"\x1B[45:95;4u", key(KeyCode::Char('_'), ALT)),
        // test_parse_csi_special_key_code_with_types
        (
            b"\x1B[;1:3B",
            key_kind(KeyCode::Down, NONE, KeyEventKind::Release),
        ),
        (
            b"\x1B[1;1:3B",
            key_kind(KeyCode::Down, NONE, KeyEventKind::Release),
        ),
        // test_parse_csi_numbered_escape_code_with_types
        (
            b"\x1B[5;1:3~",
            key_kind(KeyCode::PageUp, NONE, KeyEventKind::Release),
        ),
        (
            b"\x1B[6;5:3~",
            key_kind(KeyCode::PageDown, CTRL, KeyEventKind::Release),
        ),
    ];
    for (bytes, want) in rows {
        assert_eq!(one(bytes), want, "{:?}", String::from_utf8_lossy(bytes));
    }
}

/// crossterm's `test_parse_csi_sgr_mouse` also accepts the four-field form with a trailing `;`.
/// pi's framer never completes that shape (`stdin-buffer.ts:112-121` wants exactly three fields),
/// so it only reaches the decoder flushed; decoded directly it is still crossterm's mouse event.
#[test]
fn the_four_field_sgr_mouse_form_decodes_like_crossterm() {
    assert_eq!(
        decode_seq(b"\x1B[<0;20;10;M"),
        mouse_ev(MouseEventKind::Down(MouseButton::Left), 19, 9, NONE)
    );
    assert_eq!(
        decode_seq(b"\x1B[<0;20;10;m"),
        mouse_ev(MouseEventKind::Up(MouseButton::Left), 19, 9, NONE)
    );
}

/// crossterm's `test_parse_csi_bracketed_paste`: a partial paste is not a paste, and an escape
/// sequence inside a paste is paste content.
#[test]
fn crossterm_bracketed_paste_cases() {
    let mut buf = StdinBuffer::new(DEFAULT_ESCAPE_TIMEOUT);
    let mut frames = Vec::new();
    let now = Instant::now();
    buf.push(b"\x1B[200~o", now, &mut frames);
    assert!(frames.is_empty(), "a partial bracketed paste isn't parsed");
    buf.push(b"\x1B[2D", now, &mut frames);
    assert!(frames.is_empty(), "nor one holding another escape code");
    buf.push(b"\x1B[201~", now, &mut frames);
    assert_eq!(
        frames.into_iter().map(decode).collect::<Vec<_>>(),
        [Decoded::Event(Event::Paste("o\x1B[2D".into()))]
    );
}

/// crossterm's `test_utf8`, through the framer: valid sequences are one character each; an invalid
/// one is NOT an error that clears the buffer (crossterm, `tty.rs:261-265`) but 8-bit meta keys
/// followed by whatever the remaining bytes are (`TUI-050`).
#[test]
fn crossterm_utf8_corpus() {
    let chars = |s: &str| s.chars().map(Frame::Char).collect::<Vec<_>>();
    let frames = |bytes: &[u8]| {
        let mut buf = StdinBuffer::new(DEFAULT_ESCAPE_TIMEOUT);
        let mut out = Vec::new();
        buf.push(bytes, Instant::now(), &mut out);
        buf.flush(&mut out);
        out
    };
    use Frame::{Char, Meta};
    let rows: Vec<(&[u8], Vec<Frame>)> = vec![
        (b"a", chars("a")),
        (&[0xC3, 0xB1], chars("ñ")),
        (&[0xC3, 0x28], vec![Meta(0xC3), Char('(')]),
        (&[0xA0, 0xA1], vec![Meta(0xA0), Meta(0xA1)]),
        (&[0xE2, 0x81, 0xA1], chars("\u{2061}")),
        (&[0xE2, 0x28, 0xA1], vec![Meta(0xE2), Char('('), Meta(0xA1)]),
        (&[0xE2, 0x82, 0x28], vec![Meta(0xE2), Meta(0x82), Char('(')]),
        (&[0xF0, 0x90, 0x8C, 0xBC], chars("𐌼")),
        (
            &[0xF0, 0x28, 0x8C, 0xBC],
            vec![Meta(0xF0), Char('('), Meta(0x8C), Meta(0xBC)],
        ),
        (
            &[0xF0, 0x90, 0x28, 0xBC],
            vec![Meta(0xF0), Meta(0x90), Char('('), Meta(0xBC)],
        ),
        (
            &[0xF0, 0x28, 0x8C, 0x28],
            vec![Meta(0xF0), Char('('), Meta(0x8C), Char('(')],
        ),
    ];
    for (bytes, want) in rows {
        assert_eq!(frames(bytes), want, "{bytes:02x?}");
    }
}

/// `TUI-050`'s Verify: a one-byte chunk `0xE1` is `Alt+a` — and the rest of the 8-bit meta range
/// is `Alt` plus the 7-bit key, including the introducers crossterm would read as a CSI/SS3 start.
#[test]
fn an_eight_bit_meta_byte_decodes_to_the_alt_chord() {
    let rows: &[(u8, Decoded)] = &[
        (0xE1, key(KeyCode::Char('a'), ALT)),
        (0xE2, key(KeyCode::Char('b'), ALT)),
        (0xE6, key(KeyCode::Char('f'), ALT)),
        (0xC2, key(KeyCode::Char('B'), ALT | SHIFT)),
        (0x8D, key(KeyCode::Enter, ALT)),
        (0xFF, key(KeyCode::Backspace, ALT)),
        (0x81, key(KeyCode::Char('a'), ALT | CTRL)),
        (0xDB, key(KeyCode::Char('['), ALT)),
        (0xCF, key(KeyCode::Char('O'), ALT | SHIFT)),
        (0x9B, key(KeyCode::Esc, NONE)),
        (0xA0, key(KeyCode::Char(' '), ALT)),
    ];
    for (byte, want) in rows {
        assert_eq!(parse(&[*byte]), std::slice::from_ref(want), "{byte:#04x}");
    }
}

/// Terminal replies are one [`Decoded::Reply`] each and never a key: the OSC 11 and DSR colour
/// replies to cyrup's own probes, DA1/DA2, the Kitty flags reply, a cursor-position report, and the
/// DCS/APC replies `TUI-047` is about.
#[test]
fn terminal_replies_decode_to_reply() {
    for reply in [
        "\x1b]11;rgb:0c0c/0b0b/1313\x07",
        "\x1b]11;rgb:0c0c/0b0b/1313\x1b\\",
        "\x1b]10;rgb:ffff/ffff/ffff\x1b\\",
        "\x1b[?997;1n",
        "\x1b[?62;22c",
        "\x1b[>1;10;0c",
        "\x1b[?1u",
        "\x1b[?2026;2$y",
        "\x1b[12;40R",
        "\x1b[0n",
        "\x1b[4;600;800t",
        "\x1bP>|xterm(392)\x1b\\",
        "\x1bP1$r0m\x1b\\",
        "\x1b_Gi=31;OK\x1b\\",
        "\x1b_Gi=1;ENOENT:bad\x1b\\",
    ] {
        assert_eq!(parse(reply.as_bytes()), [Decoded::Reply], "{reply:?}");
    }
}

/// An introducer flushed on its own is the `Alt` chord that produced it; a longer sequence flushed
/// incomplete is nothing (pi's key matcher matches nothing on it either).
#[test]
fn a_flushed_introducer_is_an_alt_chord_and_a_flushed_fragment_is_nothing() {
    let rows: &[(&[u8], Decoded)] = &[
        (b"\x1b[", key(KeyCode::Char('['), ALT)),
        (b"\x1bO", key(KeyCode::Char('O'), ALT | SHIFT)),
        (b"\x1b]", key(KeyCode::Char(']'), ALT)),
        (b"\x1bP", key(KeyCode::Char('P'), ALT | SHIFT)),
        (b"\x1b_", key(KeyCode::Char('_'), ALT)),
        (b"\x1b\x1b", key(KeyCode::Esc, NONE)),
        (b"\x1b\r", key(KeyCode::Enter, ALT)),
        (b"\x1b\x7f", key(KeyCode::Backspace, ALT)),
        ("\x1bé".as_bytes(), key(KeyCode::Char('é'), ALT)),
        (b"\x1b[<35", Decoded::Ignored),
        (b"\x1b[1;", Decoded::Ignored),
        (b"\x1bP>|xterm", Decoded::Ignored),
        (b"\x1b]11;rgb:0c", Decoded::Ignored),
        (b"\x1b_Gi=3", Decoded::Ignored),
    ];
    for (bytes, want) in rows {
        assert_eq!(
            decode_seq(bytes),
            *want,
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

/// crossterm's C0 arms, which no crossterm test covers one by one.
#[test]
fn control_bytes_decode_like_crossterm() {
    let rows: &[(u8, Decoded)] = &[
        (b'\r', key(KeyCode::Enter, NONE)),
        (b'\n', key(KeyCode::Char('j'), CTRL)),
        (b'\t', key(KeyCode::Tab, NONE)),
        (0x7f, key(KeyCode::Backspace, NONE)),
        (0x00, key(KeyCode::Char(' '), CTRL)),
        (0x01, key(KeyCode::Char('a'), CTRL)),
        (0x03, key(KeyCode::Char('c'), CTRL)),
        (0x04, key(KeyCode::Char('d'), CTRL)),
        (0x07, key(KeyCode::Char('g'), CTRL)),
        (0x1a, key(KeyCode::Char('z'), CTRL)),
        (0x1c, key(KeyCode::Char('4'), CTRL)),
        (0x1f, key(KeyCode::Char('7'), CTRL)),
    ];
    for (byte, want) in rows {
        assert_eq!(parse(&[*byte]), std::slice::from_ref(want), "{byte:#04x}");
    }
}

/// Mouse coordinate bytes at or below the 1-based origin are rejected instead of underflowing
/// (crossterm's `- 1` panics in a debug build).
#[test]
fn degenerate_mouse_coordinates_do_not_underflow() {
    assert_eq!(decode_seq(b"\x1b[M   "), Decoded::Ignored);
    assert_eq!(decode_seq(b"\x1b[<0;0;0M"), Decoded::Ignored);
    assert_eq!(decode_seq(b"\x1b[32;0;0M"), Decoded::Ignored);
}
