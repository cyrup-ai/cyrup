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

/// Pi newline key is Shift+Enter (or Ctrl+J); Alt+Enter is `app.message.followUp`, not newline
/// (spec/tui/03 §5.7 / keybindings.ts).
fn newline(ed: &mut InputEditor) {
    ed.handle_key(&KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
}

#[test]
fn shift_enter_inserts_newline_enter_submits() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "a");
    newline(&mut ed);
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
    newline(&mut ed);
    type_str(&mut ed, "cd");
    assert_eq!(ed.line_count(), 2);
    ed.handle_key(&key(KeyCode::Home));
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(ed.text(), "abcd");
    assert_eq!(ed.cursor(), (0, 2));
}

#[test]
fn empty_submit_is_a_noop() {
    // Pressing Enter on an empty/whitespace buffer does not submit (it is a no-op edit).
    let mut ed = InputEditor::new();
    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(out, EditorOutcome::Edited);
    assert!(ed.is_empty());
}

#[test]
fn ctrl_char_is_ignored_by_editor() {
    // Ctrl+C is not an editor binding (it is app.clear) → editor reports Ignored so the app keymap
    // can claim it.
    let mut ed = InputEditor::new();
    let out = ed.handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(out, EditorOutcome::Ignored);
    assert!(ed.is_empty());
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}
fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

#[test]
fn word_navigation_with_alt_left_right() {
    // spec/tui/03 §5.2 — word motion, punctuation sub-boundary.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "foo.bar baz");
    // cursor at end (col 11). Alt+Left → start of "baz".
    ed.handle_key(&alt(KeyCode::Left));
    assert_eq!(ed.cursor(), (0, 8));
    // Alt+Left again → stops at the '.' punctuation sub-boundary inside foo.bar → "bar".
    ed.handle_key(&alt(KeyCode::Left));
    assert_eq!(ed.cursor(), (0, 4));
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&alt(KeyCode::Right));
    assert_eq!(ed.cursor(), (0, 3)); // end of "foo"
}

#[test]
fn ctrl_w_kills_word_backward_into_ring() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello world");
    ed.handle_key(&ctrl('w')); // kill "world"
    assert_eq!(ed.text(), "hello ");
    assert_eq!(ed.kill_ring_top(), Some("world"));
    // Ctrl+Y yanks it back.
    ed.handle_key(&ctrl('y'));
    assert_eq!(ed.text(), "hello world");
}

#[test]
fn ctrl_k_kills_to_line_end() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "keep DROP");
    for _ in 0..4 {
        ed.handle_key(&key(KeyCode::Left));
    }
    ed.handle_key(&ctrl('k'));
    assert_eq!(ed.text(), "keep ");
    assert_eq!(ed.kill_ring_top(), Some("DROP"));
}

#[test]
fn ctrl_u_kills_to_line_start() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "drop KEEP");
    for _ in 0..4 {
        ed.handle_key(&key(KeyCode::Left));
    }
    ed.handle_key(&ctrl('u'));
    assert_eq!(ed.text(), "KEEP");
    assert_eq!(ed.kill_ring_top(), Some("drop "));
}

#[test]
fn undo_restores_previous_state() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello");
    ed.handle_key(&ctrl('w')); // kill "hello"
    assert_eq!(ed.text(), "");
    ed.handle_key(&ctrl('-')); // undo the kill
    assert_eq!(ed.text(), "hello");
}

#[test]
fn prompt_history_recall_with_up_down() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "first");
    ed.handle_key(&key(KeyCode::Enter));
    type_str(&mut ed, "second");
    ed.handle_key(&key(KeyCode::Enter));
    assert!(ed.is_empty());
    // Up recalls most-recent first.
    ed.handle_key(&key(KeyCode::Up));
    assert_eq!(ed.text(), "second");
    ed.handle_key(&key(KeyCode::Up));
    assert_eq!(ed.text(), "first");
    // Down walks newer, then restores the (empty) draft.
    ed.handle_key(&key(KeyCode::Down));
    assert_eq!(ed.text(), "second");
    ed.handle_key(&key(KeyCode::Down));
    assert_eq!(ed.text(), "");
    // History holds both, most-recent first, no blanks/dups.
    assert_eq!(ed.history().len(), 2);
}

#[test]
fn bash_mode_detected_on_leading_bang() {
    let mut ed = InputEditor::new();
    assert!(!ed.is_bash_mode());
    type_str(&mut ed, "!ls");
    assert!(ed.is_bash_mode());
    // Deleting back past the '!' exits bash mode.
    for _ in 0..3 {
        ed.handle_key(&key(KeyCode::Backspace));
    }
    assert!(!ed.is_bash_mode());
}

#[test]
fn char_jump_forward_moves_to_target() {
    // spec/tui/03 §5.10 — Ctrl+] then a target char jumps to its next occurrence.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "alpha.beta.gamma");
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&ctrl(']'));
    ed.handle_key(&key(KeyCode::Char('.')));
    assert_eq!(ed.cursor(), (0, 5)); // first '.'
}
