//! The native modifier probe and the Shift+Enter rescue (G63).
//!
//! Ports `pi/packages/tui/src/native-modifiers.ts` + `terminal.ts`. At **v0.83.0** — cyrup's
//! baseline — `terminal.ts:44` already exports `normalizeAppleTerminalInput` and `:305-312`
//! (`forwardInputSequence`) already runs the darwin probe, so Apple Terminal's ambiguous bare `\r`
//! was resolved into `shift+enter` upstream and simply submitted in cyrup. That half is a **port
//! bug**. The `win32` arm of the gate (`terminal.ts:319-320`) and the `win32` prebuild branch of
//! `loadNativeModifiersHelper` (`native-modifiers.ts:24-36`) are **v0.84.1** additions.
//!
//! # Coverage honesty
//!
//! This suite runs on Linux. **Neither the darwin nor the win32 platform path is exercised here**,
//! and no test below observes a real keyboard: `platform` and the modifier probe are both
//! parameters (upstream reads `process.platform` and `cjsRequire`s a native addon), so what is
//! proven is the DECISION FUNCTION over a synthesized environment plus its wiring into
//! `App::handle_input`. The macOS/Windows probe body itself — the thing that answers "is Shift
//! physically down right now" — is not implemented in this crate and is not covered by anything
//! here; with no probe installed the rescue is inert, exactly as upstream is when its `.node`
//! addon is missing (`native-modifiers.ts:54`, `terminal.ts:305`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use cyrup_tui::{
    clear_native_modifier_probe, host_platform, is_apple_terminal_session,
    is_native_modifier_pressed, normalize_native_shift_enter, rescue_native_shift_enter,
    set_native_modifier_probe, should_detect_native_shift_enter, App, AppAction, InputEvent,
    ModifierKey, UiTheme,
};
use ratatui::backend::TestBackend;

fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

fn shift_down(_k: ModifierKey) -> bool {
    true
}

fn shift_up(_k: ModifierKey) -> bool {
    false
}

// ---- isAppleTerminalSession (`terminal.ts:43-45`) -------------------------------------------

#[test]
fn apple_terminal_is_darwin_plus_an_exact_term_program() {
    assert!(is_apple_terminal_session("darwin", Some("Apple_Terminal")));
    // `process.platform === "darwin" && ...` — both conjuncts required.
    assert!(!is_apple_terminal_session("linux", Some("Apple_Terminal")));
    assert!(!is_apple_terminal_session("win32", Some("Apple_Terminal")));
    assert!(!is_apple_terminal_session("darwin", Some("iTerm.app")));
    assert!(!is_apple_terminal_session("darwin", None));
    // Upstream's `===` is case-sensitive and exact; no substring or lowercasing.
    assert!(!is_apple_terminal_session("darwin", Some("apple_terminal")));
    assert!(!is_apple_terminal_session("darwin", Some("Apple_Terminal_2")));
}

// ---- shouldDetectNativeShiftEnter (`terminal.ts:319-320`) -----------------------------------

#[test]
fn only_a_bare_enter_on_a_swallowing_platform_needs_the_probe() {
    let apple = Some("Apple_Terminal");
    assert!(should_detect_native_shift_enter(&enter(), "darwin", apple), "v0.83.0 arm");
    // v0.84.1 arm — `|| process.platform === "win32"`, with no TERM_PROGRAM condition.
    assert!(should_detect_native_shift_enter(&enter(), "win32", None));

    // A terminal that DID encode the modifier is left alone (`sequence === "\r"` is a bare CR).
    let shifted = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    assert!(!should_detect_native_shift_enter(&shifted, "darwin", apple));
    // Any other key, and any other platform.
    let a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
    assert!(!should_detect_native_shift_enter(&a, "darwin", apple));
    assert!(!should_detect_native_shift_enter(&enter(), "linux", None));
    assert!(!should_detect_native_shift_enter(&enter(), "darwin", Some("iTerm.app")));
    // Releases never reach upstream's handler.
    let release = KeyEvent { kind: KeyEventKind::Release, ..enter() };
    assert!(!should_detect_native_shift_enter(&release, "darwin", apple));
}

// ---- normalizeNativeShiftEnterInput (`terminal.ts:44-52`) -----------------------------------

#[test]
fn the_rewrite_needs_both_the_gate_and_a_held_shift() {
    // `if (shouldDetect && data === "\r" && isShiftPressed) return "\x1b[13;2u"` — the Kitty
    // encoding of shift+enter, i.e. `Enter` + `SHIFT` once decoded.
    assert_eq!(
        normalize_native_shift_enter(enter(), true, true).modifiers,
        KeyModifiers::SHIFT
    );
    assert_eq!(normalize_native_shift_enter(enter(), true, false), enter());
    assert_eq!(normalize_native_shift_enter(enter(), false, true), enter());
    assert_eq!(normalize_native_shift_enter(enter(), false, false), enter());
}

// ---- forwardInputSequence, end to end (`terminal.ts:316-327`) -------------------------------

#[test]
fn apple_terminal_with_shift_held_becomes_shift_enter() {
    let out = rescue_native_shift_enter(enter(), "darwin", Some("Apple_Terminal"), shift_down);
    assert_eq!(out, KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
}

#[test]
fn apple_terminal_without_shift_stays_a_plain_enter() {
    let out = rescue_native_shift_enter(enter(), "darwin", Some("Apple_Terminal"), shift_up);
    assert_eq!(out, enter(), "a plain Enter must still submit");
}

#[test]
fn a_terminal_that_encodes_modifiers_is_never_touched() {
    // Kitty/ghostty on macOS report the modifier themselves; the probe must not be consulted at all.
    let out = rescue_native_shift_enter(enter(), "darwin", Some("ghostty"), |_| {
        panic!("the probe must not run off the gated path")
    });
    assert_eq!(out, enter());
}

#[test]
fn the_windows_console_arm_needs_no_term_program() {
    assert_eq!(
        rescue_native_shift_enter(enter(), "win32", None, shift_down),
        KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
    );
}

// ---- the probe registry (`native-modifiers.ts:21-59`) ---------------------------------------

/// The registry is process-wide, so both halves live in ONE test — two `#[test]`s mutating it
/// would race under the default parallel runner.
#[test]
fn the_probe_registry_defaults_to_inert_and_is_consulted_once_installed() {
    // Upstream: `loadNativeModifiersHelper()` returns undefined → `isNativeModifierPressed` is
    // `false` (`native-modifiers.ts:53-54`). This is the state this build ships in.
    clear_native_modifier_probe();
    assert!(!is_native_modifier_pressed(ModifierKey::Shift));
    assert!(!is_native_modifier_pressed(ModifierKey::Command));

    assert!(set_native_modifier_probe(|k| k == ModifierKey::Shift).is_none());
    assert!(is_native_modifier_pressed(ModifierKey::Shift));
    assert!(!is_native_modifier_pressed(ModifierKey::Option));

    assert!(clear_native_modifier_probe().is_some());
    assert!(!is_native_modifier_pressed(ModifierKey::Shift), "clearing restores the inert state");
}

#[test]
fn host_platform_uses_upstreams_process_platform_spelling() {
    assert!(matches!(host_platform(), "darwin" | "win32" | "linux" | "unknown"));
    // This box: the platform paths above are therefore NOT exercised by the live composition.
    #[cfg(target_os = "linux")]
    assert_eq!(host_platform(), "linux");
}

// ---- the user action: Shift+Enter must insert a newline, not send the message ---------------

#[test]
fn a_rescued_shift_enter_inserts_a_newline_instead_of_submitting() {
    // What the rescue produces (`Enter` + `SHIFT`) must reach the app as `tui.input.newLine`.
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("hello");
    let rescued =
        rescue_native_shift_enter(enter(), "darwin", Some("Apple_Terminal"), shift_down);
    let action = app.handle_input(&InputEvent::Key(rescued));
    assert_eq!(action, AppAction::Redraw, "no submission");
    assert_eq!(app.state().editor.text(), "hello\n", "a newline was inserted");
    assert_eq!(app.state().editor.line_count(), 2);
}

#[test]
fn an_unrescued_enter_still_submits() {
    // The failure this fixes: on Apple Terminal the rescue never ran, so Shift+Enter arrived here.
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("hello");
    let action = app.handle_input(&InputEvent::Key(enter()));
    assert_eq!(action, AppAction::Submit("hello".to_string()));
}
