//! [`StdinBuffer`] against pi's own corpus, `packages/tui/test/stdin-buffer.test.ts` @v0.87.1
//! (MIT, from OpenTUI), case for case and in the same order, then the cases pi has no test for:
//! DCS/APC frames (`TUI-047`), UTF-8 across reads and the 8-bit meta byte (`TUI-050`), and the
//! paste cap.
//!
//! pi's `emittedSequences` are strings; here each frame is shown as the bytes it carries (a
//! [`Frame::Char`] as its UTF-8, a [`Frame::Meta`] as its one byte), and pastes are collected
//! apart, as pi's `paste` listener does. pi's `await wait(ms)` is [`Harness::wait`], which flushes
//! exactly when the reader thread would: once the buffer's deadline has passed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::*;

/// A buffer, a clock, and everything it has emitted.
struct Harness {
    buf: StdinBuffer,
    now: Instant,
    frames: Vec<Frame>,
}

impl Harness {
    /// Pi's `beforeEach`: `new StdinBuffer({ timeout: 10 })` — a 10 ms sequence timeout and the
    /// default 10 ms escape timeout.
    fn new() -> Self {
        Self::with(StdinBuffer::with_timeouts(
            Duration::from_millis(10),
            DEFAULT_ESCAPE_TIMEOUT,
        ))
    }

    fn with(buf: StdinBuffer) -> Self {
        Self {
            buf,
            now: Instant::now(),
            frames: Vec::new(),
        }
    }

    fn push(&mut self, bytes: impl AsRef<[u8]>) -> &mut Self {
        self.buf.push(bytes.as_ref(), self.now, &mut self.frames);
        self
    }

    /// Let `ms` pass, firing the flush if its deadline falls inside the wait.
    fn wait(&mut self, ms: u64) -> &mut Self {
        self.now += Duration::from_millis(ms);
        if self.buf.deadline().is_some_and(|d| d <= self.now) {
            self.buf.flush(&mut self.frames);
        }
        self
    }

    /// Pi's `emittedSequences`: every non-paste frame, as bytes.
    fn seqs(&self) -> Vec<Vec<u8>> {
        self.frames
            .iter()
            .filter_map(|f| match f {
                Frame::Seq(b) => Some(b.clone()),
                Frame::Char(c) => Some(c.to_string().into_bytes()),
                Frame::Meta(b) => Some(vec![*b]),
                Frame::Paste(_) => None,
            })
            .collect()
    }

    /// Pi's `emittedPaste`.
    fn pastes(&self) -> Vec<String> {
        self.frames
            .iter()
            .filter_map(|f| match f {
                Frame::Paste(b) => Some(String::from_utf8(b.clone()).unwrap()),
                _ => None,
            })
            .collect()
    }
}

fn bytes(list: &[&str]) -> Vec<Vec<u8>> {
    list.iter().map(|s| s.as_bytes().to_vec()).collect()
}

/// The pi cases that are "feed these chunks, expect these sequences" with no timing.
#[test]
fn pi_stdin_buffer_corpus_without_timing() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        // describe("Regular Characters")
        (
            "pass through regular characters immediately",
            &["a"],
            &["a"],
        ),
        (
            "pass through multiple regular characters",
            &["abc"],
            &["a", "b", "c"],
        ),
        (
            "handle unicode characters",
            &["hello 世界"],
            &["h", "e", "l", "l", "o", " ", "世", "界"],
        ),
        // describe("Complete Escape Sequences")
        (
            "complete mouse SGR",
            &["\x1b[<35;20;5m"],
            &["\x1b[<35;20;5m"],
        ),
        ("complete arrow key", &["\x1b[A"], &["\x1b[A"]),
        ("complete function key", &["\x1b[11~"], &["\x1b[11~"]),
        ("meta key", &["\x1ba"], &["\x1ba"]),
        ("SS3", &["\x1bOA"], &["\x1bOA"]),
        // describe("Partial Escape Sequences")
        (
            "buffer incomplete CSI sequence",
            &["\x1b[", "1;", "5H"],
            &["\x1b[1;5H"],
        ),
        (
            "buffer split across many chunks",
            &["\x1b", "[", "<", "3", "5", ";", "2", "0", ";", "5", "m"],
            &["\x1b[<35;20;5m"],
        ),
        // describe("Mixed Content")
        (
            "characters then sequence",
            &["abc\x1b[A"],
            &["a", "b", "c", "\x1b[A"],
        ),
        (
            "sequence then characters",
            &["\x1b[Aabc"],
            &["\x1b[A", "a", "b", "c"],
        ),
        (
            "multiple complete sequences",
            &["\x1b[A\x1b[B\x1b[C"],
            &["\x1b[A", "\x1b[B", "\x1b[C"],
        ),
        // describe("Kitty Keyboard Protocol")
        ("Kitty CSI u press", &["\x1b[97u"], &["\x1b[97u"]),
        ("Kitty CSI u release", &["\x1b[97;1:3u"], &["\x1b[97;1:3u"]),
        (
            "batched Kitty press and release",
            &["\x1b[97u\x1b[97;1:3u"],
            &["\x1b[97u", "\x1b[97;1:3u"],
        ),
        (
            "multiple batched Kitty events",
            &["\x1b[97u\x1b[97;1:3u\x1b[98u\x1b[98;1:3u"],
            &["\x1b[97u", "\x1b[97;1:3u", "\x1b[98u", "\x1b[98;1:3u"],
        ),
        (
            "Kitty arrow with event type",
            &["\x1b[1;1:1A"],
            &["\x1b[1;1:1A"],
        ),
        (
            "Kitty functional key with event type",
            &["\x1b[3;1:3~"],
            &["\x1b[3;1:3~"],
        ),
        (
            "ESC+ESC+CSI splits (WezTerm Escape key regression)",
            &["\x1b\x1b[27;129:3u"],
            &["\x1b", "\x1b[27;129:3u"],
        ),
        (
            "ESC+ESC+CSI splits with no modifier",
            &["\x1b\x1b[27;1:3u"],
            &["\x1b", "\x1b[27;1:3u"],
        ),
        (
            "ESC+ESC alone stays one sequence (ctrl+alt+[)",
            &["\x1b\x1b"],
            &["\x1b\x1b"],
        ),
        (
            "plain character then Kitty release",
            &["a\x1b[97;1:3u"],
            &["a", "\x1b[97;1:3u"],
        ),
        (
            "drop raw duplicate after matching Kitty printable sequence",
            &["\x1b[224uà"],
            &["\x1b[224u"],
        ),
        (
            "drop raw duplicate after matching Kitty printable sequence across chunks",
            &["\x1b[64u", "@"],
            &["\x1b[64u"],
        ),
        (
            "keep non-matching plain character after Kitty printable sequence",
            &["\x1b[97ub"],
            &["\x1b[97u", "b"],
        ),
        (
            "keep raw character after modified Kitty printable sequence",
            &["\x1b[64;3u@"],
            &["\x1b[64;3u", "@"],
        ),
        (
            "rapid typing with Kitty protocol",
            &["\x1b[104u\x1b[104;1:3u\x1b[105u\x1b[105;1:3u"],
            &["\x1b[104u", "\x1b[104;1:3u", "\x1b[105u", "\x1b[105;1:3u"],
        ),
        // describe("Mouse Events")
        ("mouse press", &["\x1b[<0;10;5M"], &["\x1b[<0;10;5M"]),
        ("mouse release", &["\x1b[<0;10;5m"], &["\x1b[<0;10;5m"]),
        ("mouse move", &["\x1b[<35;20;5m"], &["\x1b[<35;20;5m"]),
        (
            "split mouse events",
            &["\x1b[<3", "5;1", "5;", "10m"],
            &["\x1b[<35;15;10m"],
        ),
        (
            "multiple mouse events",
            &["\x1b[<35;1;1m\x1b[<35;2;2m\x1b[<35;3;3m"],
            &["\x1b[<35;1;1m", "\x1b[<35;2;2m", "\x1b[<35;3;3m"],
        ),
        (
            "old-style mouse (ESC[M + 3 bytes)",
            &["\x1b[M abc"],
            &["\x1b[M ab", "c"],
        ),
        // describe("Edge Cases")
        ("buffer input", &["\x1b[A"], &["\x1b[A"]),
    ];
    for (name, chunks, want) in cases {
        let mut h = Harness::new();
        for chunk in *chunks {
            h.push(chunk);
        }
        assert_eq!(h.seqs(), bytes(want), "{name}");
        assert!(h.pastes().is_empty(), "{name}");
    }
}

// describe("Partial Escape Sequences")

#[test]
fn buffers_incomplete_mouse_sgr_sequence() {
    let mut h = Harness::new();
    h.push("\x1b");
    assert!(h.seqs().is_empty());
    assert_eq!(h.buf.held(), b"\x1b");
    h.push("[<35");
    assert!(h.seqs().is_empty());
    assert_eq!(h.buf.held(), b"\x1b[<35");
    h.push(";20;5m");
    assert_eq!(h.seqs(), bytes(&["\x1b[<35;20;5m"]));
    assert_eq!(h.buf.held(), b"");
}

#[test]
fn flushes_incomplete_sequence_after_timeout() {
    let mut h = Harness::new();
    h.push("\x1b[<35");
    assert!(h.seqs().is_empty());
    h.wait(15);
    assert_eq!(h.seqs(), bytes(&["\x1b[<35"]));
}

/// Legacy `Alt+Enter` split further apart than the escape timeout is `Escape` then `Enter`.
#[test]
fn flushes_a_lone_esc_as_escape_when_cr_arrives_after_the_timeout() {
    let mut h = Harness::new();
    h.push("\x1b").wait(20).push("\r");
    assert_eq!(h.seqs(), bytes(&["\x1b", "\r"]));
}

#[test]
fn merges_esc_cr_split_within_a_larger_escape_timeout() {
    let mut h = Harness::with(StdinBuffer::new(Duration::from_millis(100)));
    h.push("\x1b").wait(20).push("\r");
    assert_eq!(h.seqs(), bytes(&["\x1b\r"]));
}

#[test]
fn does_not_apply_the_sequence_timeout_to_a_lone_esc() {
    let mut h = Harness::with(StdinBuffer::with_timeouts(
        Duration::from_millis(100),
        DEFAULT_ESCAPE_TIMEOUT,
    ));
    h.push("\x1b").wait(20).push("\r");
    assert_eq!(h.seqs(), bytes(&["\x1b", "\r"]));
}

#[test]
fn keeps_fragmented_mouse_sequences_buffered_across_delayed_chunks_by_default() {
    let mut h = Harness::with(StdinBuffer::new(DEFAULT_ESCAPE_TIMEOUT));
    h.push("\x1b[").wait(20);
    assert!(h.seqs().is_empty());
    h.push("<65;48;39M");
    assert_eq!(h.seqs(), bytes(&["\x1b[<65;48;39M"]));
}

// describe("Mixed Content")

#[test]
fn partial_sequence_with_preceding_characters() {
    let mut h = Harness::new();
    h.push("abc\x1b[<35");
    assert_eq!(h.seqs(), bytes(&["a", "b", "c"]));
    assert_eq!(h.buf.held(), b"\x1b[<35");
    h.push(";20;5m");
    assert_eq!(h.seqs(), bytes(&["a", "b", "c", "\x1b[<35;20;5m"]));
}

// describe("Mouse Events")

#[test]
fn buffers_incomplete_old_style_mouse_sequence() {
    let mut h = Harness::new();
    h.push("\x1b[M");
    assert_eq!(h.buf.held(), b"\x1b[M");
    h.push(" a");
    assert_eq!(h.buf.held(), b"\x1b[M a");
    h.push("b");
    assert_eq!(h.seqs(), bytes(&["\x1b[M ab"]));
}

// describe("Edge Cases")

/// Pi emits one empty `data` event for an empty chunk (`stdin-buffer.ts:317-320`). A zero-byte
/// `read(2)` is end of file, which the reader stops on, so here an empty push emits nothing.
#[test]
fn empty_input_emits_nothing() {
    let mut h = Harness::new();
    h.push("");
    assert!(h.frames.is_empty());
}

#[test]
fn lone_escape_character_with_timeout() {
    let mut h = Harness::new();
    h.push("\x1b");
    assert!(h.seqs().is_empty());
    h.wait(15);
    assert_eq!(h.seqs(), bytes(&["\x1b"]));
}

#[test]
fn flushes_a_lone_escape_promptly_with_the_longer_default_sequence_timeout() {
    let mut h = Harness::with(StdinBuffer::new(DEFAULT_ESCAPE_TIMEOUT));
    h.push("\x1b").wait(20);
    assert_eq!(h.seqs(), bytes(&["\x1b"]));
}

#[test]
fn lone_escape_character_with_explicit_flush() {
    let mut h = Harness::new();
    h.push("\x1b");
    assert!(h.seqs().is_empty());
    h.buf.flush(&mut h.frames);
    assert_eq!(h.seqs(), bytes(&["\x1b"]));
}

#[test]
fn very_long_sequences() {
    let long = format!("\x1b[{}H", "1;".repeat(50));
    let mut h = Harness::new();
    h.push(&long);
    assert_eq!(h.seqs(), vec![long.into_bytes()]);
}

// describe("Flush")

#[test]
fn flushes_incomplete_sequences() {
    let mut h = Harness::new();
    h.push("\x1b[<35");
    h.buf.flush(&mut h.frames);
    assert_eq!(h.seqs(), bytes(&["\x1b[<35"]));
    assert_eq!(h.buf.held(), b"");
}

#[test]
fn flush_with_nothing_held_emits_nothing() {
    let mut h = Harness::new();
    h.buf.flush(&mut h.frames);
    assert!(h.frames.is_empty());
}

#[test]
fn emits_flushed_data_via_timeout() {
    let mut h = Harness::new();
    h.push("\x1b[<35");
    assert!(h.seqs().is_empty());
    h.wait(15);
    assert_eq!(h.seqs(), bytes(&["\x1b[<35"]));
}

// describe("Clear") / describe("Destroy")

#[test]
fn clear_forgets_buffered_content_and_its_timeout() {
    let mut h = Harness::new();
    h.push("\x1b[<35");
    assert_eq!(h.buf.held(), b"\x1b[<35");
    h.buf.clear();
    assert_eq!(h.buf.held(), b"");
    assert!(h.buf.deadline().is_none());
    h.wait(15);
    assert!(h.frames.is_empty());
}

// describe("Bracketed Paste")

#[test]
fn complete_bracketed_paste_is_one_paste_event() {
    let mut h = Harness::new();
    h.push("\x1b[200~hello world\x1b[201~");
    assert_eq!(h.pastes(), ["hello world"]);
    assert!(h.seqs().is_empty());
}

#[test]
fn paste_arriving_in_chunks() {
    let mut h = Harness::new();
    h.push("\x1b[200~");
    assert!(h.pastes().is_empty());
    h.push("hello ");
    assert!(h.pastes().is_empty());
    h.push("world\x1b[201~");
    assert_eq!(h.pastes(), ["hello world"]);
    assert!(h.seqs().is_empty());
}

#[test]
fn paste_with_input_before_and_after() {
    let mut h = Harness::new();
    h.push("a").push("\x1b[200~pasted\x1b[201~").push("b");
    assert_eq!(h.seqs(), bytes(&["a", "b"]));
    assert_eq!(h.pastes(), ["pasted"]);
}

#[test]
fn paste_with_newlines() {
    let mut h = Harness::new();
    h.push("\x1b[200~line1\nline2\nline3\x1b[201~");
    assert_eq!(h.pastes(), ["line1\nline2\nline3"]);
    assert!(h.seqs().is_empty());
}

#[test]
fn paste_with_unicode() {
    let mut h = Harness::new();
    h.push("\x1b[200~Hello 世界 🎉\x1b[201~");
    assert_eq!(h.pastes(), ["Hello 世界 🎉"]);
    assert!(h.seqs().is_empty());
}

// ------------------------------------------------------------------ beyond pi's corpus -------

/// Paste has no timeout (pi `stdin-buffer.ts:324-343` never arms one): a paste split into reads
/// seconds apart is still one paste, and nothing inside it is a keystroke.
#[test]
fn a_paste_has_no_timeout_and_keeps_escapes_crlf_and_split_markers() {
    let mut h = Harness::new();
    h.push("\x1b[20").wait(5).push("0~a\r\n\x1b[A\tb");
    assert!(h.buf.deadline().is_none(), "paste mode arms no timer");
    h.wait(5_000).push("é\x1b[20").wait(5_000).push("1~z");
    assert_eq!(h.pastes(), ["a\r\n\x1b[A\tbé"]);
    assert_eq!(h.seqs(), bytes(&["z"]));
}

/// A paste whose end marker never arrives is delivered in capped pieces, split on a character
/// boundary and never through a marker, and nothing is lost when the marker does come.
#[test]
fn an_over_long_paste_is_delivered_in_capped_pieces() {
    let mut h = Harness::with(StdinBuffer::new(DEFAULT_ESCAPE_TIMEOUT).with_paste_cap(8));
    h.push("\x1b[200~").push("abcdefgh").push("ij€kl\x1b[2");
    let pieces = h.pastes();
    assert!(!pieces.is_empty(), "the cap delivers before the marker");
    assert!(h.buf.in_paste());
    h.push("01~x");
    let all: String = h.pastes().concat();
    assert_eq!(all, "abcdefghij€kl");
    assert_eq!(h.seqs(), bytes(&["x"]));
    assert!(!h.buf.in_paste());
}

/// `TUI-047`: pi's `isCompleteDcsSequence` / `isCompleteApcSequence` (`stdin-buffer.ts:152-181`).
/// An XTVersion reply (DCS) and a Kitty graphics reply (APC) are each ONE frame — whole, split at
/// every byte, and split at the `ESC` of their `ST`.
#[test]
fn dcs_and_apc_replies_are_one_frame_whole_and_split() {
    for frame in [
        "\x1bP>|xterm(392)\x1b\\",
        "\x1bP1+r544e=787465726d\x1b\\",
        "\x1b_Gi=31;OK\x1b\\",
        "\x1b_Gi=1;ENOENT:file not found\x1b\\",
        "\x1b]11;rgb:0c0c/0b0b/1313\x07",
        "\x1b]11;rgb:0c0c/0b0b/1313\x1b\\",
    ] {
        let raw = frame.as_bytes();
        for cut in 0..=raw.len() {
            let mut h = Harness::new();
            h.push(&raw[..cut]).push(&raw[cut..]).push("k");
            assert_eq!(
                h.seqs(),
                vec![raw.to_vec(), b"k".to_vec()],
                "{frame:?} split at {cut}"
            );
        }
        let mut h = Harness::new();
        for b in raw {
            h.push([*b]);
        }
        assert_eq!(h.seqs(), vec![raw.to_vec()], "{frame:?} byte by byte");
    }
}

/// A DCS/APC payload may hold anything but `ST` — C0 controls, DEL, non-ASCII — and a BEL does not
/// end a DCS (only OSC takes BEL).
#[test]
fn dcs_and_apc_payloads_are_unconstrained() {
    let raw = "\x1bPq#0;2;0;0;0\t\x07\x7f€\r\n\x1b\\";
    let mut h = Harness::new();
    h.push(raw);
    assert_eq!(h.seqs(), bytes(&[raw]));
}

/// A UTF-8 character split across reads is one character, however it is split, and a character
/// whose tail is late by more than the escape timeout still is not typed as something else until
/// the timeout says so.
#[test]
fn a_utf8_character_split_across_reads_is_one_char() {
    for text in ["é", "世", "🎉"] {
        let raw = text.as_bytes();
        for cut in 1..raw.len() {
            let mut h = Harness::new();
            h.push(&raw[..cut]);
            assert!(h.frames.is_empty(), "{text:?} held at {cut}");
            h.push(&raw[cut..]);
            assert_eq!(h.frames, vec![Frame::Char(text.chars().next().unwrap())]);
        }
    }
}

/// `TUI-050`: a byte that is not UTF-8 is an 8-bit meta key. pi (`stdin-buffer.ts:303-315`) would
/// convert `0xE1` to `ESC a`; see [`next_char`] for why the conversion waits for UTF-8 to fail.
#[test]
fn an_eight_bit_meta_byte_becomes_a_meta_frame() {
    // A lead byte on its own: held, then the escape timeout decides it.
    let mut h = Harness::new();
    h.push([0xe1]);
    assert!(h.frames.is_empty(), "a lead byte waits for a continuation");
    assert_eq!(
        h.buf.deadline(),
        Some(h.now + DEFAULT_ESCAPE_TIMEOUT),
        "and waits the escape timeout"
    );
    h.wait(15);
    assert_eq!(h.frames, vec![Frame::Meta(0xe1)]);

    // A lead byte followed by something that does not continue it: decided at once.
    let mut h = Harness::new();
    h.push([0xe2]).push(b"x");
    assert_eq!(h.frames, vec![Frame::Meta(0xe2), Frame::Char('x')]);

    // Bytes that can never start UTF-8: decided at once, whatever surrounds them.
    let mut h = Harness::new();
    h.push([b'a', 0xa1, 0xc1, 0xff, b'b']);
    assert_eq!(
        h.frames,
        vec![
            Frame::Char('a'),
            Frame::Meta(0xa1),
            Frame::Meta(0xc1),
            Frame::Meta(0xff),
            Frame::Char('b')
        ]
    );

    // A lead byte in front of an escape sequence.
    let mut h = Harness::new();
    h.push([0xe6]).push("\x1b[A");
    assert_eq!(
        h.frames,
        vec![Frame::Meta(0xe6), Frame::Seq(b"\x1b[A".to_vec())]
    );
}

/// The Kitty dedup is scoped exactly as pi scopes it: only the ONE raw character right after an
/// unmodified printable CSI-u, and it is disarmed by any other frame, a flush and a paste.
#[test]
fn the_kitty_dedup_is_scoped_to_the_next_frame() {
    let cases: &[(&[&str], &[&str])] = &[
        // A second raw copy is typing, not a duplicate.
        (&["\x1b[97ua", "a"], &["\x1b[97u", "a"]),
        // Anything in between disarms it.
        (&["\x1b[97u\x1b[A", "a"], &["\x1b[97u", "\x1b[A", "a"]),
        // Control codepoints are not printable.
        (&["\x1b[13u\r"], &["\x1b[13u", "\r"]),
        // The shifted/base alternates still count as unmodified (pi's regex).
        (&["\x1b[97:65u", "a"], &["\x1b[97:65u"]),
        (&["\x1b[97::97u", "a"], &["\x1b[97::97u"]),
        // A modifier field or a trailing text field does not.
        (&["\x1b[97;1u", "a"], &["\x1b[97;1u", "a"]),
        // Outside the BMP (pi's UTF-16 check misses this one).
        (&["\x1b[127881u🎉"], &["\x1b[127881u"]),
    ];
    for (chunks, want) in cases {
        let mut h = Harness::new();
        for c in *chunks {
            h.push(c);
        }
        assert_eq!(h.seqs(), bytes(want), "{chunks:?}");
    }
    let mut h = Harness::new();
    h.push("\x1b[97u\x1b[200~x\x1b[201~a");
    assert_eq!(h.seqs(), bytes(&["\x1b[97u", "a"]), "a paste disarms it");
}

/// An `ESC` held across reads still frames correctly when the rest arrives inside the timeout,
/// and each lone `ESC` of `ESC ESC` pairs separated by time is its own key.
#[test]
fn esc_split_from_its_sequence_reassembles_inside_the_timeout() {
    let mut h = Harness::new();
    h.push("\x1b").wait(5).push("[A");
    assert_eq!(h.seqs(), bytes(&["\x1b[A"]));

    let mut h = Harness::new();
    h.push("\x1b").wait(15).push("\x1b").wait(15);
    assert_eq!(h.seqs(), bytes(&["\x1b", "\x1b"]));
}

/// The deadline follows the last read, as pi's `clearTimeout`/`setTimeout` pair does: a sequence
/// that keeps arriving is not flushed half-way.
#[test]
fn the_timeout_restarts_on_every_read() {
    let mut h = Harness::new();
    h.push("\x1b[1").wait(8).push(";5").wait(8).push("H");
    assert_eq!(h.seqs(), bytes(&["\x1b[1;5H"]));
}
