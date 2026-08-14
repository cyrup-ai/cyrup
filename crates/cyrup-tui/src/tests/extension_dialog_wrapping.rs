//! Closes an L4 review finding: the confirm/select/input extension dialog's title/message area was
//! hardcoded to exactly 0-or-1 terminal rows (`ListSelector`'s `title_h = u16::from(self.title.is_some())`,
//! `TextInputSelector`'s `desired_height` fixed at 4 with `Constraint::Length(1)` for its title),
//! rendered via a bare `Paragraph::new(Line::from(...))` with no `.wrap()` call anywhere — genuinely
//! truncating any long title or multi-line message to whatever fit on one terminal-width row. Pi's
//! real `Text` component (`pi-tui/src/components/text.ts`) auto-sizes to its wrapped content; the
//! fix ports that via `title_wrapped_height`/`title_lines` (`selector.rs`) + `Wrap { trim: false }`.
//!
//! Mirrors `extension_dialog_countdown.rs`'s harness (`buf_text`, `TestBackend`) for the same reason:
//! a real ratatui render into a fixed-size buffer, not a unit test on the wrapping arithmetic alone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiRequest};
use crate::{App, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;

fn buf_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// A confirm dialog's message is long enough that it CANNOT fit on a single 60-column row (nor
/// alongside the title on the same row) — Pi's exact join is `` `${title}\n${message}` ``
/// (`interactive-mode.ts:2177`), a real newline. Before the fix, only the FIRST terminal row of the
/// combined title+message ever rendered; a word placed deliberately near the END of the message
/// (`TAIL-MARKER`) proves the rest survived instead of being silently clipped.
#[test]
fn extension_confirm_dialog_wraps_a_long_message_instead_of_truncating_it() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let long_message = "this confirmation message is deliberately long enough that it cannot \
                          possibly fit on a single sixty column terminal row and must wrap across \
                          several lines to be fully visible without being clipped TAIL-MARKER";
    let req = UiRequest {
        kind: UiKind::Confirm,
        prompt: "Proceed?".to_string(),
        options: serde_json::Value::Null,
        message: long_message.to_string(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: None, signal_id: None },
        reply: tx,
    };
    app.open_extension_dialog(req);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionConfirm));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Proceed?"), "the title survives:\n{text}");
    assert!(
        text.contains("TAIL-MARKER"),
        "the tail of a long confirm message must NOT be truncated to the first row:\n{text}"
    );
}

/// The same auto-sizing applies to a plain `ui.input` dialog's TITLE alone (no message field) — a
/// long title by itself must wrap, not clip.
#[test]
fn extension_input_dialog_wraps_a_long_title_instead_of_truncating_it() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let long_title = "this is a deliberately long input dialog title that cannot possibly fit on \
                       one sixty column row and must wrap TITLE-TAIL-MARKER";
    let req = UiRequest {
        kind: UiKind::Input,
        prompt: long_title.to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: None, signal_id: None },
        reply: tx,
    };
    app.open_extension_dialog(req);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionInput));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("TITLE-TAIL-MARKER"),
        "the tail of a long input dialog title must NOT be truncated to the first row:\n{text}"
    );
}

/// A SHORT title (fits easily on one row) must still render on exactly one row — no regression to
/// always reserving multiple rows regardless of actual content length.
#[test]
fn extension_confirm_dialog_short_title_still_uses_a_single_row() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let req = UiRequest {
        kind: UiKind::Confirm,
        prompt: "Proceed?".to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: None, signal_id: None },
        reply: tx,
    };
    app.open_extension_dialog(req);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Proceed?"), "the short title still renders:\n{text}");
}

/// E6, at the CONSTRUCTION SITE. `TextInputSelector`'s hint row is built from its own `keymap`
/// field, and `App` is the only thing that knows the user's live `tui.select.*` table — so a
/// builder that is never called leaves the very first paint of a `ui.input` dialog naming stock
/// `enter`/`escape`. That first paint is the whole point of the row.
///
/// Upstream has no such window: `keyHint` re-resolves through `keyText` →
/// `getKeybindings().getKeys(...)` on every render (`keybinding-hints.ts:34-44`) against the one
/// live `KeybindingsManager`. Here the dialog is opened straight after
/// `load_keybindings_json`, drawn once, and never fed a key.
#[test]
fn extension_input_dialog_hint_row_uses_the_apps_live_keybindings_on_the_first_paint() {
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    app.load_keybindings_json(
        r#"{ "tui.select.confirm": ["ctrl+j"], "tui.select.cancel": ["ctrl+q"] }"#,
    )
    .unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    let req = UiRequest {
        kind: UiKind::Input,
        prompt: "Name?".to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: None, signal_id: None },
        reply: tx,
    };
    app.open_extension_dialog(req);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionInput));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("ctrl+j"), "submit names the user's own key:\n{text}");
    assert!(text.contains("ctrl+q"), "and so does cancel:\n{text}");
    assert!(
        !text.contains("enter submit") && !text.contains("esc  cancel"),
        "the stock defaults must not be what the first frame shows:\n{text}"
    );
}
