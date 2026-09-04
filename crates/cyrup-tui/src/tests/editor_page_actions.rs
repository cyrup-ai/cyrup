//! `tui.editor.pageUp` / `tui.editor.pageDown` — the EDITOR page actions (G62).
//!
//! Upstream binds `pageUp`/`pageDown` to the **editor**, and only to the editor:
//! `tui/src/keybindings.ts:89-90` at v0.83.0 defines `tui.editor.pageUp`/`pageDown`, the editor
//! handles them at `tui/src/components/editor.ts:855-862` by calling `pageScroll(±1)` (`:1857`),
//! and `packages/coding-agent/src/core/keybindings.ts` defines no `app.pageUp` at either v0.83.0 or
//! v0.84.1. cyrup had no `EditorAction` page variant at all, so PgUp/PgDn resolved globally and
//! always scrolled the transcript — even with a multi-page buffer under the caret.
//!
//! The `ctrl+pageUp`/`ctrl+pageDown` and `ctrl+home`/`ctrl+end` aliases asserted at the bottom are
//! **v0.84.1** additions (`keybindings.ts:92-99,108-109`), i.e. version lag rather than a port bug;
//! they are covered here because they land in the same key table.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::*;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, InputEditor, UiTheme};
use ratatui::backend::TestBackend;

fn press(ed: &mut InputEditor, code: KeyCode) {
    ed.handle_key(&KeyEvent::new(code, KeyModifiers::NONE));
}

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

fn numbered(n: usize) -> String {
    (0..n)
        .map(|i| format!("line{i:02}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A buffer of `n` short logical lines, none of which wrap at 40 columns, caret parked at `(0, 0)`.
/// 24 terminal rows → page = `max(5, floor(24 * 0.3))` = 7 (`editor.ts:1859`).
fn tall_editor(n: usize) -> InputEditor {
    let mut ed = InputEditor::new();
    ed.set_view_width(40);
    ed.set_terminal_height(24);
    ed.set_text(&numbered(n));
    // `set_text` leaves the caret at the end of the buffer; walk it back to the very top.
    for _ in 0..=n {
        press(&mut ed, KeyCode::Up);
    }
    press(&mut ed, KeyCode::Home);
    ed
}

// ---- the page primitive itself (`editor.ts:1857` pageScroll) -------------------------------

#[test]
fn page_up_moves_the_caret_a_full_page_of_visual_lines() {
    let mut ed = tall_editor(30);
    // Park the caret on logical line 20, column 4.
    for _ in 0..20 {
        press(&mut ed, KeyCode::Down);
    }
    press(&mut ed, KeyCode::Home);
    for _ in 0..4 {
        press(&mut ed, KeyCode::Right);
    }
    assert_eq!(ed.cursor(), (20, 4), "precondition");

    press(&mut ed, KeyCode::PageUp);
    // page = max(5, floor(24 * 0.3)) = 7; every line is one visual line at width 40.
    assert_eq!(ed.cursor(), (13, 4), "one page up, sticky column preserved");
}

#[test]
fn page_down_moves_the_caret_a_full_page_and_clamps_at_the_last_visual_line() {
    let mut ed = tall_editor(30);
    assert_eq!(ed.cursor(), (0, 0), "precondition");
    press(&mut ed, KeyCode::PageDown);
    assert_eq!(ed.cursor().0, 7, "one page down from the top");
    for _ in 0..10 {
        press(&mut ed, KeyCode::PageDown);
    }
    assert_eq!(
        ed.cursor().0,
        29,
        "clamped at the last visual line, never past it"
    );
}

#[test]
fn page_size_follows_the_terminal_height_like_upstream() {
    // `pageSize = Math.max(5, Math.floor(terminalRows * 0.3))` (`editor.ts:1859`).
    let mut ed = tall_editor(60);
    ed.set_terminal_height(50); // floor(50 * 0.3) = 15
    press(&mut ed, KeyCode::PageDown);
    assert_eq!(ed.cursor().0, 15);

    let mut small = tall_editor(60);
    small.set_terminal_height(4); // floor(4 * 0.3) = 1 → clamped up to the floor of 5
    press(&mut small, KeyCode::PageDown);
    assert_eq!(small.cursor().0, 5, "the max(5, …) floor applies");
}

#[test]
fn a_page_hop_keeps_the_sticky_goal_column_like_up_and_down() {
    // Upstream `pageScroll` routes through the SAME `moveToVisualLine` as `moveCursor`
    // (`editor.ts:1373,1863`), so `preferredVisualCol` survives a hop over a short line.
    let mut ed = InputEditor::new();
    ed.set_view_width(40);
    ed.set_terminal_height(24); // page = 7
    let mut lines: Vec<String> = (0..20).map(|_| "0123456789".to_string()).collect();
    lines[7] = String::new(); // the line a single page-down lands on is empty
    ed.set_text(&lines.join("\n"));
    for _ in 0..=20 {
        press(&mut ed, KeyCode::Up);
    }
    press(&mut ed, KeyCode::Home);
    for _ in 0..8 {
        press(&mut ed, KeyCode::Right);
    }
    assert_eq!(ed.cursor(), (0, 8), "precondition");

    press(&mut ed, KeyCode::PageDown);
    assert_eq!(ed.cursor(), (7, 0), "clamped to the short line");
    press(&mut ed, KeyCode::PageDown);
    assert_eq!(
        ed.cursor(),
        (14, 8),
        "the goal column is restored, not lost to the short line"
    );
}

#[test]
fn a_page_hop_does_not_recall_history() {
    // `pageScroll` never touches `historyIndex` (`editor.ts:1857-1866`), unlike cursorUp/cursorDown.
    let mut ed = InputEditor::new();
    ed.set_view_width(40);
    ed.set_terminal_height(24);
    ed.push_history("an older prompt");
    press(&mut ed, KeyCode::PageUp);
    assert_eq!(
        ed.text(),
        "",
        "PageUp on an empty editor must not pull history in"
    );
    press(&mut ed, KeyCode::Up);
    assert_eq!(ed.text(), "an older prompt", "cursorUp still does");
}

// ---- the user action: pressing PgUp with a multi-line buffer under the caret ----------------

#[test]
fn pgup_with_a_multiline_buffer_pages_the_editor_and_leaves_the_transcript_alone() {
    let mut app = new_app();
    app.editor_mut().set_view_width(40);
    app.editor_mut().set_terminal_height(24);
    app.editor_mut().set_text(&numbered(30));
    // `set_text` parks the caret on the last line (29); walk up to line 20.
    for _ in 0..9 {
        app.handle_input(&key(KeyCode::Up));
    }
    assert_eq!(app.state().editor.cursor().0, 20, "precondition");
    let scroll_before = app.state().transcript.scroll_offset();

    app.handle_input(&key(KeyCode::PageUp));

    assert_eq!(app.state().editor.cursor().0, 13, "PgUp paged the EDITOR");
    assert_eq!(
        app.state().transcript.scroll_offset(),
        scroll_before,
        "and did NOT scroll the transcript out from under it"
    );
}

#[test]
fn pgup_with_an_empty_editor_still_pages_the_transcript() {
    // cyrup's active-region transcript scroll has no pi analogue (pi pages committed history with
    // the terminal's own scrollback), so it is kept for the case upstream's editor binding is a
    // no-op in: a buffer with nothing to page through.
    let mut app = new_app();
    assert!(app.state().editor.is_empty());
    app.handle_input(&key(KeyCode::PageUp));
    assert!(
        app.state().transcript.scroll_offset() > 0,
        "the transcript still pages"
    );
}

#[test]
fn ctrl_pgup_and_ctrl_pgdn_are_page_aliases() {
    // v0.84.1 `keybindings.ts:108-109`: `["pageUp", "ctrl+pageUp"]` / `["pageDown", "ctrl+pageDown"]`.
    let mut app = new_app();
    app.editor_mut().set_view_width(40);
    app.editor_mut().set_terminal_height(24);
    app.editor_mut().set_text(&numbered(30));
    for _ in 0..=30 {
        app.handle_input(&key(KeyCode::Up));
    }
    app.handle_input(&key(KeyCode::Home));
    assert_eq!(app.state().editor.cursor(), (0, 0), "precondition");
    app.handle_input(&ctrl(KeyCode::PageDown));
    assert_eq!(app.state().editor.cursor().0, 7);
    app.handle_input(&ctrl(KeyCode::PageUp));
    assert_eq!(app.state().editor.cursor().0, 0);
}

#[test]
fn ctrl_home_and_ctrl_end_are_line_start_and_line_end_aliases() {
    // v0.84.1 `keybindings.ts:92-99`: `["home", "ctrl+home", "ctrl+a"]` / `["end", "ctrl+end", "ctrl+e"]`.
    let mut app = new_app();
    app.editor_mut().set_view_width(40);
    app.editor_mut().set_text("hello world");
    app.handle_input(&ctrl(KeyCode::Home));
    assert_eq!(app.state().editor.cursor(), (0, 0));
    app.handle_input(&ctrl(KeyCode::End));
    assert_eq!(app.state().editor.cursor(), (0, 11));
}

#[test]
fn the_hotkeys_table_names_the_editor_page_binding() {
    // Upstream reads the row off the EDITOR map — `getEditorKeyDisplay("tui.editor.pageUp")`
    // (`interactive-mode.ts:5766-5767`, rendered `:5808`).
    let mut app = new_app();
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
        text.contains("Scroll by page"),
        "the row is present:\n{text}"
    );
    // The `KeyId` is camelCase upstream — `"tui.editor.pageUp": { defaultKeys: ["pageUp",
    // "ctrl+pageUp"] }` (`tui/src/keybindings.ts:108-109` @v0.84.1; `:89-90` @v0.83.0 has the bare
    // `"pageUp"`), `keys.ts:122-123` spells the id itself `pageUp`/`pageDown`, `getKeys` returns
    // those strings verbatim (`keybindings.ts:202-204`), and `formatKeyPart` upper-cases only the
    // FIRST character (`keybinding-hints.ts:12-15`). So pi's cell is `PageUp/Ctrl+PageUp`, never the
    // fully-lowercased `Pageup` this once asserted.
    assert!(
        text.contains("`PageUp/Ctrl+PageUp` / `PageDown/Ctrl+PageDown`"),
        "and names every key bound to the editor page action:\n{text}"
    );
    // Pin the whole row, so the keys and the action can never drift apart or swap columns.
    assert!(
        text.contains("| `PageUp/Ctrl+PageUp` / `PageDown/Ctrl+PageDown` | Scroll by page |"),
        "the row is upstream's, verbatim (`interactive-mode.ts:5808` @v0.83.0):\n{text}"
    );
}
