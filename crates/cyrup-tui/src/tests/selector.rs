//! Editor-swap selector engine tests (spec/tui/05; gap "ListView/InputSlot selector engine").
//!
//! Drive the three dependency-free selectors (thinking / show-images / theme) through the real
//! `App::handle_input` routing and assert the rendered frame (full-width `─` rules, row labels,
//! the `→` selection cursor, the `(current)` marker) plus the routing outcomes (nav wrap, confirm
//! applies + closes, cancel restores the editor, theme live-preview + cancel restores the theme).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::*;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, InputEvent, SelectorKind, UiTheme, search_input_spans};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

#[test]
fn selector_search_input_renders_a_block_cursor() {
    // Feature #9 "selector IME cursor" — the embedded search Input draws a reverse-video caret at the
    // cursor byte offset (Pi's `Input`), where cyrup previously drew the query text with no caret.
    let theme = UiTheme::dark();

    // Caret in the middle: the char under it is reversed; text on either side keeps the base style.
    let spans = search_input_spans("abc", 1, 40, &theme);
    let reversed: Vec<&str> = spans
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        reversed,
        vec!["b"],
        "the caret must reverse exactly the char under it"
    );
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "abc", "the query text is preserved around the caret");

    // Caret at end: drawn as a reversed trailing space so an empty/末-position caret is still visible.
    let end = search_input_spans("hi", 2, 40, &theme);
    let end_cursor: Vec<&str> = end
        .iter()
        .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(
        end_cursor,
        vec![" "],
        "end-of-query caret is a reversed space"
    );

    // Empty query: a single reversed space caret.
    let empty = search_input_spans("", 0, 40, &theme);
    assert_eq!(empty.len(), 1);
    assert!(empty[0].style.add_modifier.contains(Modifier::REVERSED));
}
fn ctrl(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

#[test]
fn thinking_selector_renders_borders_labels_and_cursor() {
    // 24 rows, not 16. At pi 0.84.3 the picker's own envelope is 18 rows tall —
    // rule/blank/title/blank/cycle-hint/blank/`Input`/blank/7 levels/blank/footer/rule
    // (`thinking-selector.ts:77-97`) — and the chrome clamps `desired_height` to what is left after
    // the transcript, so at 16 total rows the bottom rule (and `max`) are legitimately clipped.
    let mut app = App::new(TestBackend::new(60, 24), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::Thinking);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Thinking));
    app.draw().unwrap();
    let text = buf_text(&app);

    // Full-width DynamicBorder rules top & bottom (spec/tui/05 §11) — at least two ruled rows.
    let rule_rows = text.lines().filter(|l| l.contains("──────────")).count();
    assert!(
        rule_rows >= 2,
        "expected top+bottom `─` rules, got {rule_rows}:\n{text}"
    );
    // Every Pi thinking level + its description (thinking-selector.ts:24-32). `max` is the rung
    // Pi added in fbdd4638; without it the top of the ladder is unreachable from the TUI.
    for level in ["off", "minimal", "low", "medium", "high", "xhigh", "max"] {
        assert!(text.contains(level), "missing level {level}:\n{text}");
    }
    assert!(
        text.contains("No reasoning"),
        "missing description:\n{text}"
    );
    // The default level (medium) is preselected with the `→` cursor (select-list.ts:160).
    assert!(
        text.contains("→ medium"),
        "expected cursor on preselected `medium`:\n{text}"
    );
}

#[test]
fn confirm_applies_thinking_level_and_closes() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::Thinking);
    // medium (idx 3) → down → high (idx 4); confirm.
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        app.active_selector_kind(),
        None,
        "selector should close on confirm"
    );
    assert_eq!(app.state().thinking_level, "high");
}

#[test]
fn nav_wraps_top_to_bottom() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::ShowImages);
    // show-images preselects Yes (idx 0); Up wraps to No (idx 1) (select-list.ts:115-118).
    app.handle_input(&key(KeyCode::Up));
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None);
    assert!(!app.state().show_images, "Up-wrap should land on `No`");
}

#[test]
fn cancel_closes_and_restores_editor_text() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.editor_mut().set_text("draft prompt");
    app.open_selector(SelectorKind::Thinking);
    // Esc and Ctrl+C both cancel (tui.select.cancel) — Esc must NOT interrupt the agent here.
    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(
        app.active_selector_kind(),
        None,
        "Esc should dismiss the selector"
    );
    assert_eq!(
        app.state().editor.text(),
        "draft prompt",
        "editor text restored on close"
    );
    // The level is unchanged by a cancel.
    assert_eq!(app.state().thinking_level, "medium");
}

#[test]
fn ctrl_c_cancels_selector() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::Thinking);
    app.handle_input(&ctrl('c'));
    assert_eq!(
        app.active_selector_kind(),
        None,
        "Ctrl+C should dismiss the selector"
    );
}

#[test]
fn theme_selector_marks_current_and_live_previews() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    assert_eq!(app.state().theme.name, "dark");
    app.open_selector(SelectorKind::Theme);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("dark"), "theme list missing `dark`:\n{text}");
    assert!(
        text.contains("light"),
        "theme list missing `light`:\n{text}"
    );
    assert!(
        text.contains("(current)"),
        "current theme marker missing:\n{text}"
    );

    // Navigating re-themes the whole UI live (theme-selector.ts:54-56 onPreview).
    app.handle_input(&key(KeyCode::Down));
    assert_eq!(
        app.state().theme.name,
        "light",
        "nav should live-preview `light`"
    );

    // Cancel restores the prior theme (caller responsibility, spec/tui/05 §6.11).
    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(app.active_selector_kind(), None);
    assert_eq!(
        app.state().theme.name,
        "dark",
        "cancel restores the previewed-away theme"
    );
}

#[test]
fn theme_confirm_keeps_selection() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.open_selector(SelectorKind::Theme);
    app.handle_input(&key(KeyCode::Down)); // preview light
    app.handle_input(&key(KeyCode::Enter)); // confirm light
    assert_eq!(app.active_selector_kind(), None);
    assert_eq!(
        app.state().theme.name,
        "light",
        "confirm commits the highlighted theme"
    );
}

#[test]
fn selector_suppresses_autocomplete_popup_and_takes_input_slot() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    // A pure-list selector ignores text keys (no embedded search Input) — they do not reach the editor.
    app.open_selector(SelectorKind::Thinking);
    app.handle_input(&key(KeyCode::Char('x')));
    assert_eq!(
        app.state().editor.text(),
        "",
        "typing must not leak to the editor under a selector"
    );
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Thinking));
}
