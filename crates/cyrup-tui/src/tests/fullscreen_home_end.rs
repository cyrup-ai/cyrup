//! TUI-173 — bare Home/End belong to the editor in fullscreen; the transcript's top/bottom are
//! `ctrl+home`/`ctrl+end`.
//!
//! pi 1.0.3 (`6100fe5a8`, #10314) moved `tui.altScreen.top`/`bottom` from `home`/`end` to
//! `ctrl+home`/`ctrl+end` (`packages/tui/src/keybindings.ts:208-209` @f1b2e77f5) and dropped those
//! two chords from `tui.editor.cursorLineStart`/`cursorLineEnd` (`:98-105`, now
//! `["home","ctrl+a"]` / `["end","ctrl+e"]`). Upstream's test is "routes Home and End to the
//! focused component and Ctrl+Home/End to the transcript" (`test/tui-alt-screen.test.ts`).
//! Fullscreen is cyrup's default render mode, so before this every Home/End in the prompt editor
//! jumped the transcript instead.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::*;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{AltScreenAction, AltScreenKeymap, App, TuiRenderMode, UiTheme};
use ratatui::backend::TestBackend;

fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

/// The keymap half: under `Fullscreen`, bare Home/End resolve to nothing (so they fall through to
/// the editor) and `ctrl+home`/`ctrl+end` are top/bottom. pi's `keybindings.test.ts` asserts the
/// same pair: `getKeys("tui.altScreen.top") == ["ctrl+home"]`, `bottom == ["ctrl+end"]`.
#[test]
fn fullscreen_home_end_are_unbound_and_ctrl_home_end_are_top_bottom() {
    let keys = AltScreenKeymap::default();
    let fs = TuiRenderMode::Fullscreen;
    assert_eq!(
        keys.action_in_mode(&ev(KeyCode::Home, KeyModifiers::NONE), fs),
        None
    );
    assert_eq!(
        keys.action_in_mode(&ev(KeyCode::End, KeyModifiers::NONE), fs),
        None
    );
    assert_eq!(
        keys.action_in_mode(&ev(KeyCode::Home, KeyModifiers::CONTROL), fs),
        Some(AltScreenAction::Top)
    );
    assert_eq!(
        keys.action_in_mode(&ev(KeyCode::End, KeyModifiers::CONTROL), fs),
        Some(AltScreenAction::Bottom)
    );
    assert_eq!(
        keys.keys_label(AltScreenAction::Top).as_deref(),
        Some("ctrl+home")
    );
    assert_eq!(
        keys.keys_label(AltScreenAction::Bottom).as_deref(),
        Some("ctrl+end")
    );
}

/// An app with a transcript tall enough to scroll and a one-line editor buffer.
fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    for i in 0..80 {
        app.transcript_mut().push_status(format!("row {i}"));
    }
    app
}

fn top(app: &mut App<TestBackend>) -> usize {
    app.altscreen_for_test().unwrap().viewport_top()
}

/// The app half, with the alternate screen live: Home/End move the editor caret and leave the
/// viewport alone; Ctrl+Home/Ctrl+End scroll the transcript and leave the caret alone.
#[test]
fn with_the_alternate_screen_live_home_end_move_the_caret_and_ctrl_home_end_scroll() {
    let mut app = fullscreen_app();
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.editor_mut().set_view_width(40);
    app.editor_mut().set_text("hello world");
    app.draw().unwrap();

    // Park the viewport mid-document so both "top" and "bottom" are real moves.
    let tail = app.altscreen_for_test().unwrap().max_scroll_top_for_test();
    assert!(tail > 10, "fixture: the document overflows the viewport");
    app.altscreen_for_test()
        .unwrap()
        .scroll_to_row_for_test(tail / 2);
    let parked = top(&mut app);
    assert_eq!(app.state().editor.cursor(), (0, 11), "precondition");

    app.handle_input(&key(KeyCode::Home));
    assert_eq!(app.state().editor.cursor(), (0, 0), "Home moves the caret");
    assert_eq!(
        top(&mut app),
        parked,
        "and leaves the viewport where it was"
    );

    app.handle_input(&key(KeyCode::End));
    assert_eq!(app.state().editor.cursor(), (0, 11), "End moves the caret");
    assert_eq!(
        top(&mut app),
        parked,
        "and leaves the viewport where it was"
    );

    app.handle_input(&ctrl(KeyCode::Home));
    assert_eq!(top(&mut app), 0, "Ctrl+Home scrolls to the top");
    assert_eq!(
        app.state().editor.cursor(),
        (0, 11),
        "without moving the caret"
    );

    app.handle_input(&ctrl(KeyCode::End));
    assert_eq!(top(&mut app), tail, "Ctrl+End scrolls to the bottom");
    assert!(
        app.altscreen_for_test().unwrap().is_following_output(),
        "and re-arms the tail follow"
    );
    assert_eq!(
        app.state().editor.cursor(),
        (0, 11),
        "without moving the caret"
    );
}

/// `/hotkeys` reads the editor rows off the editor map, so they lose the `Ctrl+Home`/`Ctrl+End`
/// aliases: pi's `docs/keybindings.md:60-61` @f1b2e77f5 lists `home`, `ctrl+a` / `end`, `ctrl+e`.
#[test]
fn hotkeys_editor_rows_read_home_ctrl_a_and_end_ctrl_e() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("/hotkeys");
    app.handle_input(&key(KeyCode::Enter));
    let text = app
        .state()
        .transcript
        .pending()
        .iter()
        .map(|e| format!("{e:?}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("| `Home/Ctrl+A` | Start of line |"),
        "line-start row:\n{text}"
    );
    assert!(
        text.contains("| `End/Ctrl+E` | End of line |"),
        "line-end row:\n{text}"
    );
}
