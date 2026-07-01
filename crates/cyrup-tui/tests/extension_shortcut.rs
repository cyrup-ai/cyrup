//! Extension keyboard-shortcut dispatch routing (feature #10; R-08-017; Pi `registerShortcut`).
//!
//! Proves the *routed keypress* half the audit called for: a key press that matches an
//! extension-registered shortcut resolves — through the real `App::handle_input` routing chain — to
//! an [`AppAction::ExtensionShortcut`] carrying the key-id, which the run loop hands to
//! `ExtensionHost::run_shortcut` → `LiveExtension::execute_shortcut`. The guest-execution tail is
//! wasm-toolchain-gated (ledger 09 #13); the dispatch decision is closed and asserted here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, AppAction, InputEvent, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode, mods: KeyModifiers) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, mods))
}

#[test]
fn registered_shortcut_keypress_dispatches_to_ext_host() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    // The host's `shortcut_keys()` returns raw key-id strings; install them the way the binary does.
    app.set_extension_shortcuts(["ctrl+j".to_string(), "alt+k".to_string()]);

    // A matching press routes to the owning extension via its key-id (the run loop then calls
    // `ExtensionHost::run_shortcut(id)`), NOT into the editor as text.
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
        AppAction::ExtensionShortcut("ctrl+j".to_string())
    );
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('k'), KeyModifiers::ALT)),
        AppAction::ExtensionShortcut("alt+k".to_string())
    );
    // The editor never saw the shortcut keys (buffer stays empty).
    assert!(app.state().editor.is_empty());
}

#[test]
fn builtin_bindings_win_over_extension_shortcuts() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    // An extension cannot shadow a built-in global binding: register Ctrl+D (the exit key).
    app.set_extension_shortcuts(["ctrl+d".to_string()]);
    // Ctrl+D on an empty buffer still resolves to the built-in Quit action, not the shortcut.
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('d'), KeyModifiers::CONTROL)),
        AppAction::Quit
    );
}

#[test]
fn unregistered_key_falls_through_to_the_editor() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_extension_shortcuts(["ctrl+j".to_string()]);
    // A plain letter with no registered shortcut is typed into the editor, never captured.
    let out = app.handle_input(&key(KeyCode::Char('h'), KeyModifiers::NONE));
    assert_eq!(out, AppAction::Redraw);
    assert!(!app.state().editor.is_empty());
}

#[test]
fn unparseable_shortcut_ids_are_dropped_not_panicked() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    // A malformed key-id is silently dropped; a valid sibling still routes.
    app.set_extension_shortcuts(["not a key+++".to_string(), "ctrl+j".to_string()]);
    assert_eq!(
        app.handle_input(&key(KeyCode::Char('j'), KeyModifiers::CONTROL)),
        AppAction::ExtensionShortcut("ctrl+j".to_string())
    );
}
