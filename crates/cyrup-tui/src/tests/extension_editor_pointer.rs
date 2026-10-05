//! The pointer over an extension `ui.editor` dialog in the alternate screen.
//!
//! `ExtensionEditorComponent` adds its `Editor` to its container as a bare child
//! (`extension-editor.ts:78-79`), so a click on the editor's rows reaches `Editor.handleMouse`
//! (`packages/tui/src/components/editor.ts:632-686` @v1.0.0) and puts the caret there. Pi wires no
//! autocomplete provider into this editor, so there is no popup to click. The dialog is opened the
//! way the extension UI effect opens it; the caret is read back by typing a marker character and
//! reading the buffer, so the test sees what the user would.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use super::harness::key;
use crate::{App, AppAction, InputEvent, SelectorKind, UiTheme};
use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};

const COLS: u16 = 80;
const ROWS: u16 = 30;

fn open_dialog(initial: &str) -> (App<TestBackend>, tokio::sync::oneshot::Receiver<UiReply>) {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    let (reply, rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(UiRequest {
        kind: UiKind::Editor,
        prompt: "edit the notes".to_string(),
        options: serde_json::Value::Null,
        message: initial.to_string(),
        placeholder: None,
        opts: DialogOptions {
            timeout_ms: None,
            signal_id: None,
        },
        reply,
    });
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ExtensionEditor)
    );
    (app, rx)
}

/// The screen cell of the first occurrence of `needle` in the frame just drawn.
fn cell_of(app: &mut App<TestBackend>, needle: &str) -> (u16, u16) {
    app.draw().unwrap();
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    for y in 0..ROWS {
        let row: String = (0..COLS)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        if let Some(byte) = row.find(needle) {
            return (row[..byte].chars().count() as u16, y);
        }
    }
    panic!("`{needle}` is not on screen");
}

fn mouse(kind: MouseEventKind, at: (u16, u16)) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column: at.0,
        row: at.1,
        modifiers: KeyModifiers::NONE,
    })
}

fn click(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), at));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), at))
}

/// Type `X` and submit: the reply is the buffer with `X` at the caret.
fn reply_after_typing_x(
    app: &mut App<TestBackend>,
    rx: &mut tokio::sync::oneshot::Receiver<UiReply>,
) -> String {
    app.handle_input(&key(KeyCode::Char('X')));
    app.handle_input(&key(KeyCode::Enter));
    match rx.try_recv().expect("the dialog replied") {
        UiReply::Text(Some(text)) => text,
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn a_click_on_the_editor_rows_puts_the_caret_there() {
    let (mut app, mut rx) = open_dialog("alpha\nbeta gamma\ndelta");
    let (x, y) = cell_of(&mut app, "beta gamma");
    click(&mut app, (x + 2, y));
    assert_eq!(
        reply_after_typing_x(&mut app, &mut rx),
        "alpha\nbeXta gamma\ndelta"
    );
}

#[test]
fn a_click_past_the_end_of_a_line_lands_at_its_end() {
    let (mut app, mut rx) = open_dialog("alpha\nbeta gamma\ndelta");
    let (x, y) = cell_of(&mut app, "alpha");
    click(&mut app, (x + 40, y));
    assert_eq!(
        reply_after_typing_x(&mut app, &mut rx),
        "alphaX\nbeta gamma\ndelta"
    );
}

/// Only a click places the caret (`editor.ts:652`): the press that begins it and a lone press do
/// not.
#[test]
fn a_press_alone_does_not_move_the_caret() {
    let (mut app, mut rx) = open_dialog("alpha\nbeta gamma");
    let (x, y) = cell_of(&mut app, "alpha");
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), (x + 1, y)));
    assert_eq!(
        reply_after_typing_x(&mut app, &mut rx),
        "alpha\nbeta gammaX",
        "the caret stayed at the end of the seed"
    );
}

/// The title, the hint and the dialog's rules are not the editor's rows: a click there moves
/// nothing and is left to the renderer (`None`).
#[test]
fn a_click_on_the_title_or_the_hint_moves_nothing() {
    let (mut app, mut rx) = open_dialog("alpha\nbeta gamma");
    let title = cell_of(&mut app, "edit the notes");
    assert_eq!(click(&mut app, title), AppAction::None);
    let hint = cell_of(&mut app, "submit");
    assert_eq!(click(&mut app, hint), AppAction::None);
    assert_eq!(
        reply_after_typing_x(&mut app, &mut rx),
        "alpha\nbeta gammaX"
    );
}
