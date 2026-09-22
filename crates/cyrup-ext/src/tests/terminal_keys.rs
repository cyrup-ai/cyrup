//! The one raw-terminal-key table (UW-7): [`encode_terminal_key`] / [`decode_terminal_key`].
//!
//! Why ONE table with a round-trip test rather than two independently written halves: the encoder
//! lives on cyrup's keystroke path (`cyrup-tui`) and the decoder on the extension's
//! (`cyrup-ext-subagents`), and they must agree BYTE FOR BYTE or the fleet roster silently never
//! activates. Two halves that must agree and cannot be compared is a defect waiting to happen;
//! one table plus [`round_trip_is_the_identity_for_every_variant`] cannot drift.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::{
    TerminalKey, TerminalKeyEvent, TerminalKeyModifiers, decode_terminal_key, encode_terminal_key,
};

/// Every member of the closed vocabulary, with a representative `Char` spread: ASCII lower, the
/// two roster bindings pi names (`j`/`k`), a digit, a symbol, a space, and a multi-byte char that
/// exercises `char::from_u32` on the way back.
fn every_key() -> Vec<TerminalKey> {
    let mut keys = vec![
        TerminalKey::Up,
        TerminalKey::Down,
        TerminalKey::Left,
        TerminalKey::Right,
        TerminalKey::Escape,
        TerminalKey::Enter,
    ];
    for c in ['j', 'k', 'a', 'Z', '7', '/', ' ', 'é', '→'] {
        keys.push(TerminalKey::Char(c));
    }
    keys
}

fn every_modifier_set() -> Vec<TerminalKeyModifiers> {
    vec![
        TerminalKeyModifiers::NONE,
        TerminalKeyModifiers::SHIFT,
        TerminalKeyModifiers::ALT,
        TerminalKeyModifiers::CTRL,
        TerminalKeyModifiers::SUPER,
        TerminalKeyModifiers::CTRL.union(TerminalKeyModifiers::SHIFT),
        TerminalKeyModifiers::CTRL
            .union(TerminalKeyModifiers::ALT)
            .union(TerminalKeyModifiers::SUPER),
    ]
}

/// **T5** — `encode` then `decode` is the identity over EVERY variant × EVERY modifier set ×
/// press/release. This is the test the one-table decision exists for.
///
/// MUTATION: swap `\x1b[B` for `\x1b[A` in [`encode_terminal_key`]'s arrow arm (or drop the `:3`
/// from its release arm) — `Down` then round-trips to `Up` (resp. a release round-trips as a
/// press) and this fails. Observed RED.
#[test]
fn round_trip_is_the_identity_for_every_variant() {
    for key in every_key() {
        for modifiers in every_modifier_set() {
            for release in [false, true] {
                let ev = TerminalKeyEvent {
                    key,
                    modifiers,
                    release,
                };
                let encoded =
                    encode_terminal_key(ev).unwrap_or_else(|| panic!("{ev:?} must be encodable"));
                assert_eq!(
                    decode_terminal_key(&encoded),
                    Some(ev),
                    "{ev:?} encoded as {encoded:?} did not round-trip"
                );
            }
        }
    }
}

/// The canonical LEGACY forms — the ones every existing cyrup test, doc comment and terminal
/// without the kitty protocol writes. Pinned literally rather than only via the round trip,
/// because the round trip alone would be satisfied by any self-consistent private encoding, and
/// `native_dispatch.rs`'s `host.terminal_input("\x1b[A")` is an EXISTING caller that would then
/// stop matching.
///
/// MUTATION: make the unmodified-press arm emit the kitty form instead of the legacy one — every
/// assertion below fails, and so does any extension keyed off `"\x1b[A"`. Observed RED.
#[test]
fn the_unmodified_press_forms_are_the_legacy_sequences() {
    let cases = [
        (TerminalKey::Up, "\x1b[A"),
        (TerminalKey::Down, "\x1b[B"),
        (TerminalKey::Right, "\x1b[C"),
        (TerminalKey::Left, "\x1b[D"),
        (TerminalKey::Escape, "\x1b"),
        (TerminalKey::Enter, "\r"),
        (TerminalKey::Char('j'), "j"),
    ];
    for (key, expected) in cases {
        assert_eq!(
            encode_terminal_key(TerminalKeyEvent::press(key)).as_deref(),
            Some(expected),
            "{key:?}"
        );
    }
}

/// A modified or released keystroke falls back to the kitty forms, which are the only ones that
/// can carry either field (pi `arrowMatch` `keys.ts:612`, `csiUMatch` `:598`).
///
/// MUTATION: drop the `release` term from `encode_terminal_key`'s `legacy` predicate — a released
/// `Down` then encodes as the plain `\x1b[B` a PRESS encodes as, the widget sees a press, and
/// `↓`-held auto-repeat scrolls the roster twice per key. Observed RED.
#[test]
fn modifiers_and_releases_use_the_kitty_forms() {
    assert_eq!(
        encode_terminal_key(TerminalKeyEvent {
            key: TerminalKey::Down,
            modifiers: TerminalKeyModifiers::NONE,
            release: true,
        })
        .as_deref(),
        Some("\x1b[1;1:3B")
    );
    assert_eq!(
        encode_terminal_key(TerminalKeyEvent {
            key: TerminalKey::Left,
            modifiers: TerminalKeyModifiers::CTRL,
            release: false,
        })
        .as_deref(),
        Some("\x1b[1;5D")
    );
    assert_eq!(
        encode_terminal_key(TerminalKeyEvent {
            key: TerminalKey::Char('j'),
            modifiers: TerminalKeyModifiers::CTRL,
            release: false,
        })
        .as_deref(),
        Some("\x1b[106;5u")
    );
    assert_eq!(
        encode_terminal_key(TerminalKeyEvent {
            key: TerminalKey::Escape,
            modifiers: TerminalKeyModifiers::NONE,
            release: true,
        })
        .as_deref(),
        Some("\x1b[27;1:3u")
    );
}

/// The alternates the DECODER accepts although the encoder never emits them, each with the pi arm
/// that accepts it. A terminal in application-cursor mode (`\x1bOA`), one with the kitty keyboard
/// protocol negotiated (`\x1b[…u`), and one with xterm `modifyOtherKeys=2` (`\x1b[27;…~`) all put
/// these on the wire, and the chunk may not have come from `encode_terminal_key` at all.
///
/// MUTATION: delete the `"\x1bOA"` arm of `decode_legacy` — `↑` stops working under tmux, `vim`'s
/// terminal and anything else that sets DECCKM. Observed RED.
#[test]
fn the_decoder_accepts_the_alternate_encodings() {
    let cases: &[(&str, TerminalKeyEvent)] = &[
        // SS3 / application cursor keys (DECCKM) — pi `LEGACY_KEY_SEQUENCES` second entries.
        ("\x1bOA", TerminalKeyEvent::press(TerminalKey::Up)),
        ("\x1bOB", TerminalKeyEvent::press(TerminalKey::Down)),
        ("\x1bOC", TerminalKeyEvent::press(TerminalKey::Right)),
        ("\x1bOD", TerminalKeyEvent::press(TerminalKey::Left)),
        // Enter's two legacy alternates — pi `keys.ts:921-922`.
        ("\n", TerminalKeyEvent::press(TerminalKey::Enter)),
        ("\x1bOM", TerminalKeyEvent::press(TerminalKey::Enter)),
        // kitty CSI u without the modifier field at all — pi `modValue = 1` default (`:603`).
        ("\x1b[27u", TerminalKeyEvent::press(TerminalKey::Escape)),
        ("\x1b[13u", TerminalKeyEvent::press(TerminalKey::Enter)),
        // kitty numpad Enter — pi `CODEPOINTS.kpEnter` (`keys.ts:306`), matched beside `enter`.
        ("\x1b[57414;1u", TerminalKeyEvent::press(TerminalKey::Enter)),
        // kitty flag-4 alternate keys: `<cp>:<shifted>:<base>` and `<cp>::<base>`. Parsed off and
        // discarded, as `matchesKittySequence`'s primary comparison does.
        (
            "\x1b[106:74;1u",
            TerminalKeyEvent::press(TerminalKey::Char('j')),
        ),
        (
            "\x1b[106::106;1u",
            TerminalKeyEvent::press(TerminalKey::Char('j')),
        ),
        // kitty explicit press / repeat event types — pi `1=press, 2=repeat` (`keys.ts:501-504`);
        // a repeat is a press, which is why `isKeyRelease` tests only `:3`.
        ("\x1b[1;1:1B", TerminalKeyEvent::press(TerminalKey::Down)),
        ("\x1b[1;1:2B", TerminalKeyEvent::press(TerminalKey::Down)),
        // xterm modifyOtherKeys — pi `parseModifyOtherKeysSequence` (`keys.ts:696-702`).
        (
            "\x1b[27;1;27~",
            TerminalKeyEvent::press(TerminalKey::Escape),
        ),
        ("\x1b[27;1;13~", TerminalKeyEvent::press(TerminalKey::Enter)),
        (
            "\x1b[27;1;106~",
            TerminalKeyEvent::press(TerminalKey::Char('j')),
        ),
    ];
    for (data, expected) in cases {
        assert_eq!(decode_terminal_key(data), Some(*expected), "{data:?}");
    }
}

/// Caps Lock / Num Lock are masked off before comparison — pi `LOCK_MASK = 64 + 128`
/// (`keys.ts:299`), applied at `matchesKittySequence` (`:656-657`). Without it a terminal with
/// Caps Lock on reports modifier 65 and every roster key reads as "some other key".
///
/// MUTATION: drop the `& !(64 | 128)` from `TerminalKeyModifiers::from_bits` — the modifier set
/// comes back non-empty and this fails. Observed RED.
#[test]
fn the_lock_modifiers_are_masked_off() {
    // modifier field 65 = raw bits 64 (Caps Lock), 1-indexed.
    assert_eq!(
        decode_terminal_key("\x1b[1;65B"),
        Some(TerminalKeyEvent::press(TerminalKey::Down))
    );
    // 129 = raw bits 128 (Num Lock).
    assert_eq!(
        decode_terminal_key("\x1b[106;129u"),
        Some(TerminalKeyEvent::press(TerminalKey::Char('j')))
    );
}

/// `None` is "not a key in THIS vocabulary", and the boundary is drawn where the doc says it is.
///
/// MUTATION: make `key_from_csi_u_codepoint` accept control codepoints — `\x1b[9;1u` (Tab) then
/// decodes as `Char('\t')`, the roster treats a Tab as a printable, and the round trip breaks
/// because `encode_terminal_key` refuses `Char('\t')`. Observed RED.
#[test]
fn out_of_vocabulary_chunks_decode_to_none() {
    for data in [
        "",                     // nothing at all
        "\x1b[Z",               // shift+tab
        "\t",                   // tab
        "\x7f",                 // backspace
        "\x1b[3~",              // delete
        "\x1b[5~",              // page up
        "\x1b[9;1u",            // kitty tab — a CONTROL codepoint, refused
        "\x1b[a",               // the legacy SHIFT arrow table, deliberately not ported
        "\x1bOa",               // the legacy CTRL arrow table, deliberately not ported
        "\x1b[<0;10;5M",        // an SGR mouse report
        "hello",                // a multi-char chunk that is not a sequence
        "\x1b[200~ab\x1b[201~", // bracketed paste content
    ] {
        assert_eq!(decode_terminal_key(data), None, "{data:?}");
    }
}

/// The encoder refuses what it cannot express in one canonical form, rather than emitting
/// something the decoder would read back as a different key.
///
/// MUTATION: let the `Char(c)` legacy arm through for control characters — `Char('\r')` then
/// encodes as `"\r"`, which decodes as `Enter`, and the round-trip test above fails. Observed RED.
#[test]
fn the_encoder_refuses_control_characters() {
    for c in ['\r', '\n', '\t', '\x1b', '\0'] {
        assert_eq!(
            encode_terminal_key(TerminalKeyEvent::press(TerminalKey::Char(c))),
            None,
            "Char({c:?})"
        );
        assert_eq!(
            encode_terminal_key(TerminalKeyEvent {
                key: TerminalKey::Char(c),
                modifiers: TerminalKeyModifiers::CTRL,
                release: false,
            }),
            None,
            "ctrl+Char({c:?})"
        );
    }
}
