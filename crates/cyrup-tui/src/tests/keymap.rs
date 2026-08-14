//! Key parsing + keymap resolution (R-10-018 / R-10-023 / R-10-024).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{Action, Key, Keymap};

#[test]
fn parse_string_key_specs() {
    assert_eq!(Key::parse("ctrl+c").unwrap(), Key::ctrl('c'));
    let shift_tab = Key::parse("shift+tab").unwrap();
    assert_eq!(shift_tab.code, KeyCode::Tab);
    assert_eq!(shift_tab.mods, KeyModifiers::SHIFT);
    assert_eq!(Key::parse("esc").unwrap(), Key::plain(KeyCode::Esc));
    let alt_enter = Key::parse("alt+enter").unwrap();
    assert_eq!(alt_enter.code, KeyCode::Enter);
    assert_eq!(alt_enter.mods, KeyModifiers::ALT);
}

#[test]
fn parse_rejects_empty_and_modifier_only() {
    assert!(Key::parse("").is_err());
    assert!(Key::parse("ctrl+").is_err());
}

#[test]
fn key_matches_event() {
    let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(Key::ctrl('c').matches(&ev));
    assert!(!Key::plain(KeyCode::Char('c')).matches(&ev));
}

#[test]
fn matches_ignores_lock_and_unsupported_modifier_masks() {
    // Pi strips the Caps/Num lock mask (and any unsupported modifier bit) before comparing
    // (keys.ts:361,656,779). crossterm surfaces `HYPER`/`META` as the closest analogues to the JS
    // `LOCK_MASK`; a Ctrl+D chord with a stray lock/hyper bit still resolves to the exit binding.
    let d_with_hyper =
        KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL | KeyModifiers::HYPER);
    assert_eq!(Keymap::default().action_for(&d_with_hyper), Some(Action::Quit));
    // A bare key carrying only a lock/hyper bit still matches a no-modifier binding.
    let a_with_meta = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::META);
    assert!(Key::plain(KeyCode::Char('a')).matches(&a_with_meta));
}

#[test]
fn matches_normalizes_shifted_letters() {
    // Pi normalizes a shifted ASCII letter to its lowercase codepoint (keys.ts:360-366): a `shift+a`
    // binding matches a terminal reporting `Char('A')` + SHIFT (the disambiguate/Kitty path).
    let shift_a_upper = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT);
    let binding = Key { code: KeyCode::Char('a'), mods: KeyModifiers::SHIFT };
    assert!(binding.matches(&shift_a_upper));
    // Symmetric: a spec written as `shift+A` matches a `Char('a')` + SHIFT event too.
    let shift_a_lower = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::SHIFT);
    let binding_upper = Key { code: KeyCode::Char('A'), mods: KeyModifiers::SHIFT };
    assert!(binding_upper.matches(&shift_a_lower));
    // Without shift, an uppercase letter is a distinct key (no spurious collapse).
    let plain_upper = KeyEvent::new(KeyCode::Char('A'), KeyModifiers::NONE);
    assert!(!Key::plain(KeyCode::Char('a')).matches(&plain_upper));
}

#[test]
fn default_keymap_binds_pi_app_actions() {
    // Pi defaults (core/keybindings.ts:63-202): Ctrl+D exit, Ctrl+C clear, Esc interrupt.
    let km = Keymap::default();
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    let plain_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(km.action_for(&ctrl_d), Some(Action::Quit));
    assert_eq!(km.action_for(&ctrl_c), Some(Action::Clear));
    assert_eq!(km.action_for(&esc), Some(Action::Interrupt));
    assert_eq!(km.action_for(&plain_a), None);
}

#[test]
fn keymap_rebind_overrides() {
    let mut km = Keymap::empty();
    km.bind(Key::ctrl('q'), Action::Quit);
    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.action_for(&ctrl_q), Some(Action::Quit));
    // Rebinding the same key replaces, not duplicates.
    km.bind(Key::ctrl('q'), Action::Interrupt);
    assert_eq!(km.action_for(&ctrl_q), Some(Action::Interrupt));
}
