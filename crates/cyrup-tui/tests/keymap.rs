//! Key parsing + keymap resolution (R-10-018 / R-10-023 / R-10-024).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{Action, Key, Keymap};

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
