//! Input-editor unit tests (R-10-015): typing, backspace, cursor movement, newline, submit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{EditorOutcome, InputEditor};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn type_str(ed: &mut InputEditor, s: &str) {
    for c in s.chars() {
        ed.handle_key(&key(KeyCode::Char(c)));
    }
}

#[test]
fn typing_accumulates_text() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello");
    assert_eq!(ed.text(), "hello");
    assert_eq!(ed.cursor(), (0, 5));
    assert!(!ed.is_empty());
}

#[test]
fn backspace_deletes_char_before_cursor() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello");
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(ed.text(), "hell");
    assert_eq!(ed.cursor(), (0, 4));
}

#[test]
fn cursor_move_then_insert_lands_mid_string() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "abcd");
    ed.handle_key(&key(KeyCode::Left));
    ed.handle_key(&key(KeyCode::Left));
    assert_eq!(ed.cursor(), (0, 2));
    type_str(&mut ed, "X");
    assert_eq!(ed.text(), "abXcd");
    ed.handle_key(&key(KeyCode::Home));
    assert_eq!(ed.cursor(), (0, 0));
    ed.handle_key(&key(KeyCode::End));
    assert_eq!(ed.cursor(), (0, 5));
}

#[test]
fn alt_enter_inserts_newline_enter_submits() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "a");
    ed.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_str(&mut ed, "b");
    assert_eq!(ed.text(), "a\nb");
    assert_eq!(ed.cursor(), (1, 1));

    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(out, EditorOutcome::Submit("a\nb".to_string()));
    // Submit clears the buffer.
    assert!(ed.is_empty());
    assert_eq!(ed.text(), "");
}

#[test]
fn backspace_at_line_start_joins_previous_line() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "ab");
    ed.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_str(&mut ed, "cd");
    assert_eq!(ed.line_count(), 2);
    ed.handle_key(&key(KeyCode::Home));
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(ed.text(), "abcd");
    assert_eq!(ed.cursor(), (0, 2));
}

#[test]
fn empty_submit_yields_empty_string() {
    let mut ed = InputEditor::new();
    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(out, EditorOutcome::Submit(String::new()));
}

#[test]
fn ctrl_char_is_ignored_by_editor() {
    let mut ed = InputEditor::new();
    let out = ed.handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(out, EditorOutcome::Ignored);
    assert!(ed.is_empty());
}
