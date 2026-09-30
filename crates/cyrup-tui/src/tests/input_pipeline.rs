//! The production input pipeline end-to-end, from BYTES: what [`crate::crossterm_input_stream`]'s
//! reader thread does to a sequence of `read(2)` chunks — [`ByteDecoder`] (pi's `StdinBuffer` +
//! the crossterm-rules decoder), with its timeouts firing exactly when the thread's poll would
//! fire them — then [`map_event`], then the real editor.
//!
//! These are the byte-level successors of the event-level pipeline tests that drove
//! `EscapeReassembler` and `StrayReplyFilter` (both now Windows-only), and they keep those modules'
//! guarantees as regression guards: a late terminal reply never reaches the editor, a sequence
//! split at its `ESC` is one key, and ordinary typing — including `Escape` and `Alt+]` — is
//! delivered intact.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::InputEvent;
use crate::UiTheme;
use crate::app::{App, map_event};
use crate::input::frame::DEFAULT_ESCAPE_TIMEOUT;
use crate::input::reader::ByteDecoder;

/// Feed `(gap_ms, bytes)` chunks — each read arriving `gap_ms` after the previous one — with the
/// local 10 ms escape timeout, then let the input go idle.
fn pipeline(chunks: &[(u64, &[u8])]) -> Vec<InputEvent> {
    pipeline_with(DEFAULT_ESCAPE_TIMEOUT, chunks)
}

fn pipeline_with(escape_timeout: Duration, chunks: &[(u64, &[u8])]) -> Vec<InputEvent> {
    let mut decoder = ByteDecoder::new(escape_timeout);
    let mut now = Instant::now();
    let mut events = Vec::new();
    for (gap, bytes) in chunks {
        now += Duration::from_millis(*gap);
        decoder.flush_due(now, &mut events);
        decoder.feed(bytes, now, &mut events);
    }
    decoder.flush_due(now + Duration::from_secs(1), &mut events);
    events.into_iter().filter_map(map_event).collect()
}

/// What the prompt holds after the events are handled.
fn typed(events: &[InputEvent]) -> String {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    for ev in events {
        app.handle_input(ev);
    }
    app.state().editor.text()
}

fn keys(events: &[InputEvent]) -> Vec<KeyEvent> {
    events
        .iter()
        .map(|e| match e {
            InputEvent::Key(k) => *k,
            other => panic!("expected only keys, got {other:?}"),
        })
        .collect()
}

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

/// A user launched cyrup and got `11;rgb:0c0c/0b0b/1313` typed into their prompt: the terminal
/// answered the boot OSC 11 probe after `terminal_query`'s deadline. The reply is one frame now,
/// whole, split at its `ESC`, or terminated by `ST`.
#[test]
fn a_late_osc11_reply_never_reaches_the_editor() {
    for chunks in [
        vec![(0, &b"\x1b]11;rgb:0c0c/0b0b/1313\x07"[..])],
        vec![(0, &b"\x1b"[..]), (5, &b"]11;rgb:0c0c/0b0b/1313\x07"[..])],
        vec![(0, &b"\x1b]11;rgb:0c0c/0b0b/1313\x1b"[..]), (5, &b"\\"[..])],
    ] {
        let delivered = pipeline(&chunks);
        assert!(delivered.is_empty(), "{chunks:?} delivered {delivered:?}");
    }
}

/// `TUI-047`'s Verify, from bytes: an XTVersion reply (DCS) and a Kitty graphics reply (APC), whole
/// and split at the `ESC` of their `ST`, put nothing in the prompt — where crossterm typed ~20
/// characters plus two Alt chords for each.
#[test]
fn late_dcs_and_apc_replies_never_reach_the_editor() {
    for reply in [&b"\x1bP>|cyrup 1.0\x1b\\"[..], &b"\x1b_Gi=31;OK\x1b\\"[..]] {
        let st = reply.len() - 2;
        for chunks in [
            vec![(0, reply)],
            vec![(0, &reply[..st]), (5, &reply[st..])],
            vec![(0, &reply[..1]), (5, &reply[1..])],
        ] {
            let delivered = pipeline(&chunks);
            assert!(delivered.is_empty(), "{chunks:?} delivered {delivered:?}");
        }
    }
    // And typing on either side of one is untouched.
    let delivered = pipeline(&[(0, b"ab\x1bP>|cyrup 1.0\x1b\\cd")]);
    assert_eq!(typed(&delivered), "abcd");
}

/// Other late replies: a cursor-position report, the Kitty flags, DA1, a colour-scheme report.
#[test]
fn late_csi_replies_never_reach_the_editor() {
    let delivered = pipeline(&[
        (0, b"\x1b"),
        (5, b"[12;40R"),
        (0, b"\x1b[?1u\x1b[?62;22c\x1b[?997;1n"),
    ]);
    assert!(delivered.is_empty(), "{delivered:?}");
}

/// TUI-045's Verify at the pipeline level: the two-chunk form is one `Up`, not `Esc` + `[` + `A`.
#[test]
fn an_arrow_key_split_at_the_esc_byte_reaches_the_app_as_one_up() {
    let delivered = pipeline(&[(0, b"\x1b"), (5, b"[A")]);
    assert_eq!(keys(&delivered), [key(KeyCode::Up, KeyModifiers::NONE)]);
    assert_eq!(typed(&delivered), "");
}

/// The safety half: ordinary typing is delivered byte-for-byte, including the two keys the old
/// OSC 11 filter held (`Escape`, then `Alt+]` — which is an OSC introducer until its timeout says
/// it is a key).
#[test]
fn ordinary_typing_survives_the_pipeline_intact() {
    let delivered = pipeline(&[(0, b"hello 11; world"), (0, b"\x1b"), (30, b"\x1b]")]);
    let mut want: Vec<KeyEvent> = "hello 11; world"
        .chars()
        .map(|c| key(KeyCode::Char(c), KeyModifiers::NONE))
        .collect();
    want.push(key(KeyCode::Esc, KeyModifiers::NONE));
    want.push(key(KeyCode::Char(']'), KeyModifiers::ALT));
    assert_eq!(keys(&delivered), want);
    assert_eq!(typed(&delivered[..15]), "hello 11; world");
}

/// Every printable ASCII byte, in one burst: one key each, none held, none lost.
#[test]
fn every_printable_byte_of_a_burst_is_accounted_for() {
    let burst: Vec<u8> = (0x20..=0x7e).collect();
    let delivered = pipeline(&[(0, &burst)]);
    let want: Vec<KeyEvent> = burst
        .iter()
        .map(|b| {
            let c = char::from(*b);
            let mods = if c.is_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            key(KeyCode::Char(c), mods)
        })
        .collect();
    assert_eq!(keys(&delivered), want);
}

/// `Escape` then typing is `Escape` then the typing; two `Escape` presses are two.
#[test]
fn escape_presses_are_delivered_as_escape() {
    let delivered = pipeline(&[(0, b"\x1b"), (30, b"abc"), (0, b"\x1b"), (30, b"\x1b")]);
    assert_eq!(
        keys(&delivered),
        [
            key(KeyCode::Esc, KeyModifiers::NONE),
            key(KeyCode::Char('a'), KeyModifiers::NONE),
            key(KeyCode::Char('b'), KeyModifiers::NONE),
            key(KeyCode::Char('c'), KeyModifiers::NONE),
            key(KeyCode::Esc, KeyModifiers::NONE),
            key(KeyCode::Esc, KeyModifiers::NONE),
        ]
    );
}

/// A paste split across reads — with its opener split too — is one paste, newlines, tabs and an
/// embedded escape sequence included.
#[test]
fn a_split_bracketed_paste_is_one_paste_event() {
    let delivered = pipeline(&[
        (0, b"\x1b[20"),
        (5, b"0~line1\r\n\tx\x1b[A"),
        (2_000, b"rest\x1b[201~"),
    ]);
    match delivered.as_slice() {
        [InputEvent::Paste(p)] => assert_eq!(p, "line1\r\n\tx\x1b[Arest"),
        other => panic!("one paste, got {other:?}"),
    }
}

/// A sequence fragment that never completes is dropped after the sequence timeout — pi's key
/// matcher matches nothing on it — rather than typed.
#[test]
fn an_unterminated_csi_prefix_is_not_typed() {
    let delivered = pipeline(&[(0, b"\x1b[1;"), (200, b"x")]);
    assert_eq!(typed(&delivered), "x");
}

/// `TUI-046`: a Kitty printable sequence followed by the raw copy some terminals also send is ONE
/// character in the prompt — while a real doubled letter, which crossterm's events made
/// indistinguishable from that copy, is still two.
#[test]
fn a_kitty_printable_duplicate_is_typed_once_and_a_real_double_letter_twice() {
    assert_eq!(typed(&pipeline(&[(0, "\x1b[224uà".as_bytes())])), "à");
    assert_eq!(
        typed(&pipeline(&[(0, b"\x1b[64u"), (5, b"@")])),
        "@",
        "across reads"
    );
    assert_eq!(typed(&pipeline(&[(0, b"hello")])), "hello");
    assert_eq!(typed(&pipeline(&[(0, b"\x1b[108ul")])), "l");
    assert_eq!(typed(&pipeline(&[(0, b"\x1b[108ull")])), "ll");
}

/// `TUI-050`'s Verify, as far as it goes without an xterm: with `metaSendsEscape: false`, `Alt+B`
/// is the one byte `0xE2`, and it moves the caret back one word.
#[test]
fn an_eight_bit_alt_b_moves_back_a_word() {
    let delivered = pipeline(&[(0, b"foo bar"), (0, &[0xe2]), (30, b"X")]);
    assert_eq!(typed(&delivered), "foo Xbar");
    // A multi-byte character split across reads is still that character.
    let delivered = pipeline(&[(0, &[0xc3]), (5, &[0xa9])]);
    assert_eq!(typed(&delivered), "é");
}

/// `TUI-106` over bytes: under SSH a lone `ESC` waits 100 ms, so an arrow split 60 ms apart is one
/// `Up`; locally it waits 10 ms and the same split is `Escape` first.
#[test]
fn a_split_arrow_over_ssh_reassembles_across_a_60ms_gap() {
    let ssh = crate::app::resolve_escape_timeout(|k| {
        (k == "SSH_CONNECTION").then(|| "a 1 b 22".to_owned())
    });
    let chunks: &[(u64, &[u8])] = &[(0, b"\x1b"), (60, b"[A")];
    assert_eq!(
        keys(&pipeline_with(ssh, chunks)),
        [key(KeyCode::Up, KeyModifiers::NONE)]
    );
    let local = crate::app::resolve_escape_timeout(|_| None);
    assert_eq!(
        keys(&pipeline_with(local, chunks))[0],
        key(KeyCode::Esc, KeyModifiers::NONE)
    );
}
