//! The one set of key-grammar tests for overlay key specs (EXT-103): `KeySpec::parse` /
//! `KeySpec::matches`, `key_ids` and `parse_user_bindings`. `cyrup-llama` and `cyrup-mcp` both
//! resolve through these, so the grammar is exercised here once and not per overlay.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;

use crate::host::{KeySpec, OverlayKey, OverlayKeyCode, key_ids, parse_user_bindings};

fn press(code: OverlayKeyCode) -> OverlayKey {
    OverlayKey::plain(code)
}

fn chr(c: char) -> OverlayKey {
    press(OverlayKeyCode::Char(c))
}

fn ctrl(c: char) -> OverlayKey {
    OverlayKey::ctrl(OverlayKeyCode::Char(c))
}

fn with_shift(code: OverlayKeyCode) -> OverlayKey {
    OverlayKey {
        shift: true,
        ..OverlayKey::plain(code)
    }
}

fn spec(s: &str) -> KeySpec {
    KeySpec::parse(s).unwrap()
}

/// Modifiers, aliases, camelCase page keys, function keys, and specs that name no key.
#[test]
fn key_spec_matching() {
    assert!(spec("ctrl+c").matches(&ctrl('c')));
    assert!(!spec("ctrl+c").matches(&chr('c')));
    assert!(!spec("c").matches(&ctrl('c')));
    assert!(spec("enter").matches(&press(OverlayKeyCode::Enter)));
    assert!(spec("return").matches(&press(OverlayKeyCode::Enter)));
    assert!(spec("esc").matches(&press(OverlayKeyCode::Escape)));
    assert!(spec("escape").matches(&press(OverlayKeyCode::Escape)));
    assert!(spec("pageUp").matches(&press(OverlayKeyCode::PageUp)));
    assert!(spec("f5").matches(&press(OverlayKeyCode::F(5))));
    assert!(spec("ctrl+-").matches(&ctrl('-')));
    assert!(spec("alt+backspace").matches(&OverlayKey {
        alt: true,
        ..press(OverlayKeyCode::Backspace)
    }));
    assert!(KeySpec::parse("super+x").is_none());
    assert!(KeySpec::parse("f13").is_none());
    assert!(KeySpec::parse("").is_none());
    assert!(KeySpec::parse("nonsense").is_none());
}

/// Every alias pi's grammar and `cyrup-tui`'s `Key::parse` spell parses to one code, and token
/// case is irrelevant.
#[test]
fn key_spec_aliases_and_case() {
    for (text, code) in [
        ("enter", OverlayKeyCode::Enter),
        ("RETURN", OverlayKeyCode::Enter),
        ("esc", OverlayKeyCode::Escape),
        ("Escape", OverlayKeyCode::Escape),
        ("del", OverlayKeyCode::Delete),
        ("delete", OverlayKeyCode::Delete),
        ("pgup", OverlayKeyCode::PageUp),
        ("pageUp", OverlayKeyCode::PageUp),
        ("pgdn", OverlayKeyCode::PageDown),
        ("pageDown", OverlayKeyCode::PageDown),
        ("ins", OverlayKeyCode::Insert),
        ("space", OverlayKeyCode::Char(' ')),
        ("F12", OverlayKeyCode::F(12)),
        ("K", OverlayKeyCode::Char('k')),
    ] {
        assert_eq!(spec(text).code, code, "{text}");
    }
    let held = spec("Control+Option+Shift+x");
    assert!(held.ctrl && held.alt && held.shift);
    assert!(spec("meta+x").alt);
    assert_eq!(spec(" ctrl + c "), spec("ctrl+c"), "tokens are trimmed");
    // Function keys outside 1..=12 and a bare `f` that is not one.
    assert!(KeySpec::parse("f0").is_none());
    assert!(KeySpec::parse("f999").is_none());
    assert_eq!(spec("f").code, OverlayKeyCode::Char('f'));
    assert!(KeySpec::parse("f1x").is_none());
    // A spec with modifiers only, or a second multi-character token, names no key.
    assert!(KeySpec::parse("ctrl").is_none());
    assert!(KeySpec::parse("ctrl++").is_none());
    assert!(KeySpec::parse("up+down+nope").is_none());
    assert!(KeySpec::parse("cmd+k").is_none());
}

/// `shift` is compared only when the spec asks, and a printable character arrives already
/// shift-resolved by the host.
#[test]
fn key_spec_shift_handling_for_characters() {
    // Unshifted spec: case-insensitive on an unshifted keystroke, exact once shift is reported.
    assert!(spec("k").matches(&chr('k')));
    assert!(spec("k").matches(&chr('K')));
    assert!(!spec("k").matches(&with_shift(OverlayKeyCode::Char('K'))));
    // Shifted spec: needs the shift bit or an uppercase character.
    assert!(spec("shift+k").matches(&chr('K')));
    assert!(spec("shift+k").matches(&with_shift(OverlayKeyCode::Char('k'))));
    assert!(!spec("shift+k").matches(&chr('k')));
    // Non-character keys compare shift only when the spec asks.
    assert!(spec("up").matches(&press(OverlayKeyCode::Up)));
    assert!(spec("up").matches(&with_shift(OverlayKeyCode::Up)));
    assert!(spec("shift+up").matches(&with_shift(OverlayKeyCode::Up)));
    assert!(!spec("shift+up").matches(&press(OverlayKeyCode::Up)));
    // Alt must agree exactly.
    assert!(!spec("alt+b").matches(&chr('b')));
    assert!(!spec("b").matches(&OverlayKey {
        alt: true,
        ..chr('b')
    }));
}

/// A `BackTab` keystroke is `Tab` + shift (pi: `matchesKey(data, "shift+tab")` is `"\x1b[Z"`).
/// This is the one place the two former overlay copies disagreed: llama's folded it, mcp's did not.
#[test]
fn key_spec_backtab_is_tab_with_shift() {
    let backtab = press(OverlayKeyCode::BackTab);
    assert!(spec("shift+tab").matches(&backtab));
    assert!(spec("backtab").matches(&backtab));
    assert!(spec("shift+tab").matches(&with_shift(OverlayKeyCode::Tab)));
    assert!(spec("backtab").matches(&with_shift(OverlayKeyCode::Tab)));
    // Plain tab is not shift+tab, either way round (pi: `modifier === 0` accepts `"\t"` only).
    assert!(!spec("tab").matches(&backtab));
    assert!(!spec("tab").matches(&with_shift(OverlayKeyCode::Tab)));
    assert!(spec("tab").matches(&press(OverlayKeyCode::Tab)));
    assert!(!spec("shift+tab").matches(&press(OverlayKeyCode::Tab)));
    assert!(!spec("backtab").matches(&press(OverlayKeyCode::Tab)));
    // Ctrl still has to agree.
    assert!(!spec("ctrl+shift+tab").matches(&backtab));
}

/// `KeyId | KeyId[]`: a string is one id, an array keeps its strings, anything else is absent.
#[test]
fn key_ids_accepts_a_string_or_a_list_of_strings() {
    assert_eq!(key_ids(&json!("ctrl+p")), Some(vec!["ctrl+p".to_string()]));
    assert_eq!(
        key_ids(&json!(["up", 3, "ctrl+p", null])),
        Some(vec!["up".to_string(), "ctrl+p".to_string()])
    );
    assert_eq!(key_ids(&json!([])), Some(Vec::new()));
    assert_eq!(key_ids(&json!(3)), None);
    assert_eq!(key_ids(&json!(null)), None);
    assert_eq!(key_ids(&json!({"a": 1})), None);
}

/// The reader: legacy ids are migrated exactly as the TUI does; a malformed document or one that
/// is not an object is no document at all.
#[test]
fn user_bindings_migrate_legacy_ids_and_reject_non_objects() {
    let current = parse_user_bindings(r#"{"tui.select.up": "ctrl+p"}"#).unwrap();
    assert_eq!(
        current,
        vec![("tui.select.up".to_string(), json!("ctrl+p"))]
    );

    // `selectUp` is the legacy spelling of `tui.select.up`.
    let legacy = parse_user_bindings(r#"{"selectUp": "ctrl+p"}"#).unwrap();
    assert_eq!(legacy, current);

    assert_eq!(parse_user_bindings("{}"), Some(Vec::new()));
    assert_eq!(parse_user_bindings("{ not json"), None);
    assert_eq!(parse_user_bindings("[1, 2]"), None);
    assert_eq!(parse_user_bindings("\"up\""), None);
    assert_eq!(parse_user_bindings(""), None);
}
