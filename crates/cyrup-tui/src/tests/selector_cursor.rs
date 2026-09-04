//! The hardware cursor while a **selector** owns the input slot.
//!
//! # What was broken
//!
//! `Selector::cursor(&self) -> Option<(u16, u16)>` was a defaulted accessor returning `None` that
//! all 13 implementors took — including the ones with a live text input — and that no code anywhere
//! read: `crates/cyrup-tui/src/editor.rs:2535` was the crate's only `Frame::set_cursor_position`
//! caller, and it runs for the editor, never for a selector. So while an extension `ui.input`
//! dialog, `/model` or `/resume`'s search box was open, the terminal's real cursor sat wherever the
//! previous frame left it — which is what an IME composes against and what a screen reader follows.
//!
//! Pi does place it: a focused `Input` emits `CURSOR_MARKER` at its caret (`packages/tui/src/
//! components/input.ts:434`) and `TUI.extractCursorPosition` (`tui.ts:1189-1207`) finds it in the
//! rendered output and hands it to `positionHardwareCursor`. The port scans the rendered CELLS for
//! the reverse-video caret instead (`selector::caret_cell`), which every selector draws through the
//! one shared `search_input_spans`, and the accessor is gone.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiRequest};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

use crate::{App, SelectorKind, UiTheme};

/// The first reverse-video cell in the rendered buffer, scanning bottom-up exactly as
/// `caret_cell`/`extractCursorPosition` do — i.e. where the caret actually is on screen.
fn caret_in_buffer(app: &App<TestBackend>) -> Option<(u16, u16)> {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    for y in (0..area.height).rev() {
        for x in 0..area.width {
            if buf
                .cell((x, y))
                .is_some_and(|c| c.modifier.contains(Modifier::REVERSED))
            {
                return Some((x, y));
            }
        }
    }
    None
}

/// An open `ui.input` dialog (a `TextInputSelector`) in the input slot.
fn app_with_input_dialog() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(UiRequest {
        kind: UiKind::Input,
        prompt: "Name?".to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions {
            timeout_ms: None,
            signal_id: None,
        },
        reply: tx,
    });
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ExtensionInput)
    );
    app
}

/// The regression: with the hardware cursor enabled, the terminal cursor is shown AND parked on the
/// dialog's caret cell — not left wherever the last frame put it.
#[test]
fn an_input_dialogs_caret_gets_the_hardware_cursor() {
    let mut app = app_with_input_dialog();
    app.state_mut().editor.set_show_hardware_cursor(true);
    app.draw().unwrap();

    let caret = caret_in_buffer(&app).expect("the input dialog draws a reverse-video caret");
    let backend = app.terminal().backend();
    assert!(
        backend.cursor_visible(),
        "ratatui hides the cursor for a frame that sets no position — the selector slot used to \
         set none at all",
    );
    let pos = backend.cursor_position();
    assert_eq!(
        (pos.x, pos.y),
        caret,
        "the hardware cursor must sit on the caret cell (Pi `positionHardwareCursor`, \
         tui-alt-screen.ts:1300-1301)",
    );
}

/// Pi gates the whole mechanism on `showHardwareCursor` (`tui.ts:389-397`: off ⇒ `hideCursor()`),
/// and cyrup keeps that flag on the editor. A selector must respect the same switch rather than
/// forcing a cursor the user turned off.
#[test]
fn the_show_hardware_cursor_setting_still_gates_the_selector_slot() {
    let mut app = app_with_input_dialog();
    app.state_mut().editor.set_show_hardware_cursor(false);
    app.draw().unwrap();
    assert!(
        caret_in_buffer(&app).is_some(),
        "the software caret is drawn either way"
    );
    assert!(
        !app.terminal().backend().cursor_visible(),
        "showHardwareCursor=false must leave the terminal cursor hidden",
    );
}

/// A pure-list selector emits no caret, which is Pi's "no component emitted a marker" case: the
/// cursor stays hidden instead of being parked on some arbitrary cell of the list.
#[test]
fn a_list_selector_without_an_input_shows_no_hardware_cursor() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    app.state_mut().editor.set_show_hardware_cursor(true);
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(UiRequest {
        kind: UiKind::Confirm,
        prompt: "Proceed?".to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions {
            timeout_ms: None,
            signal_id: None,
        },
        reply: tx,
    });
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ExtensionConfirm)
    );
    app.draw().unwrap();
    assert_eq!(
        caret_in_buffer(&app),
        None,
        "a confirm dialog has no Input and no caret"
    );
    assert!(!app.terminal().backend().cursor_visible());
}
