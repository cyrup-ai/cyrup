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
fn left_right_step_over_whole_grapheme_cluster() {
    // A family-emoji ZWJ sequence is many `char`s but ONE grapheme cluster: one Left must skip it all
    // (spec/tui/03 §4 grapheme motion). "a👨‍👩‍👧b" — the middle cluster is 5 scalar values.
    let mut ed = InputEditor::new();
    ed.set_text("a👨‍👩‍👧b");
    // Cursor at end. One Left lands before the trailing 'b'.
    ed.handle_key(&key(KeyCode::Left));
    let (_, after_b) = ed.cursor();
    // One more Left skips the entire family cluster in a single step, landing just after 'a'.
    ed.handle_key(&key(KeyCode::Left));
    let (_, after_family) = ed.cursor();
    assert_eq!(after_family, 1, "Left did not step the whole grapheme cluster as one unit");
    assert!(after_b - after_family >= 5, "the cluster should span ≥5 scalar columns");
    // Right steps back over the whole cluster in one move.
    ed.handle_key(&key(KeyCode::Right));
    assert_eq!(ed.cursor().1, after_b);
}

#[test]
fn backspace_removes_whole_grapheme_cluster() {
    let mut ed = InputEditor::new();
    ed.set_text("a👨‍👩‍👧");
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(ed.text(), "a", "backspace must delete the entire emoji cluster, not one scalar");
    assert_eq!(ed.cursor(), (0, 1));
}

#[test]
fn forward_delete_removes_whole_grapheme_cluster() {
    let mut ed = InputEditor::new();
    ed.set_text("👨‍👩‍👧b");
    ed.handle_key(&key(KeyCode::Home));
    ed.handle_key(&key(KeyCode::Delete));
    assert_eq!(ed.text(), "b", "forward-delete must remove the entire emoji cluster");
    assert_eq!(ed.cursor(), (0, 0));
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
fn bash_mode_detected_after_leading_whitespace() {
    // Pi enters bash mode on `text.trimStart().startsWith("!")` (interactive-mode.ts:2525): a leading
    // indent before `!` still shows the bash-green border (item #5 "trim_start on bash input").
    let mut ed = InputEditor::new();
    type_str(&mut ed, "   !ls");
    assert!(ed.is_bash_mode(), "leading whitespace before ! must still be bash mode");
    // A non-`!` first non-space char is not bash mode.
    ed.set_text("  echo hi");
    assert!(!ed.is_bash_mode());
}

#[test]
fn ctrl_w_at_line_start_kills_across_the_line_join() {
    // Cross-line char/word kill (item #5): at column 0 the word-left target is the end of the previous
    // line, so Ctrl+W deletes the newline and joins the two rows — `take_range` now spans logical
    // lines (previously it returned empty, moving the cursor but deleting nothing).
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello");
    newline(&mut ed);
    type_str(&mut ed, "again");
    ed.handle_key(&KeyCode::Home.into()); // row 1, col 0
    assert_eq!(ed.cursor(), (1, 0));
    ed.handle_key(&ctrl('w')); // kill back across the join
    assert_eq!(ed.text(), "helloagain");
    assert_eq!(ed.line_count(), 1);
    assert_eq!(ed.cursor(), (0, 5));
    // The killed newline yanks back verbatim, restoring the two lines.
    ed.handle_key(&ctrl('y'));
    assert_eq!(ed.text(), "hello\nagain");
}

#[test]
fn undo_whitespace_boundary_removes_last_word_not_whole_line() {
    // Pi's fish-style rule (editor.ts:1085-1094): each whitespace captures the state before itself, so
    // a single undo removes the most-recent word (+ its leading space), not the entire typed line.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "foo bar baz");
    ed.handle_key(&ctrl('-')); // undo → drops " baz"
    assert_eq!(ed.text(), "foo bar");
    ed.handle_key(&ctrl('-')); // undo → drops " bar"
    assert_eq!(ed.text(), "foo");
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

// ---- wrap-aware vertical (visual-line) motion (spec/tui/03 §4) ------------------------------

#[test]
fn visual_line_map_wraps_long_line_at_word_boundary() {
    // A single logical line wider than the layout width expands into multiple visual lines,
    // breaking at the last whitespace that fits (`wordWrapLine`, editor.ts:114).
    let mut ed = InputEditor::new();
    ed.set_view_width(10);
    type_str(&mut ed, "alpha bravo charlie");
    let map = ed.visual_line_map();
    assert!(map.len() >= 2, "long line must wrap into >1 visual line: {map:?}");
    // Every visual line is a slice of logical line 0.
    assert!(map.iter().all(|vl| vl.logical == 0));
    // First visual line starts at column 0; the second resumes after the wrap.
    assert_eq!(map[0].start, 0);
    assert!(map[1].start > 0);
}

#[test]
fn cursor_down_moves_by_visual_line_not_logical_line() {
    // One *logical* line that wraps to several *visual* lines: Down advances a visual line, so the
    // cursor stays on logical row 0 but jumps forward across the wrap (it does NOT fall to history).
    let mut ed = InputEditor::new();
    ed.set_view_width(10);
    type_str(&mut ed, "alpha bravo charlie");
    // Cursor at end (col 19). Move Home, then Down should land on the second visual line, row 0.
    ed.handle_key(&KeyCode::Home.into());
    assert_eq!(ed.cursor(), (0, 0));
    ed.handle_key(&key(KeyCode::Down));
    let (row, col) = ed.cursor();
    assert_eq!(row, 0, "still the same logical line");
    assert!(col > 0, "cursor advanced to the next visual line, col={col}");
}

#[test]
fn vertical_motion_keeps_sticky_goal_column() {
    // Goal column survives a short intermediate line: Down from a long line through a short line and
    // onto another long line restores the original column (sticky preferred_visual_col).
    let mut ed = InputEditor::new();
    ed.set_view_width(40);
    type_str(&mut ed, "abcdefghij");
    newline(&mut ed); // logical row 1
    type_str(&mut ed, "xy");
    newline(&mut ed); // logical row 2
    type_str(&mut ed, "0123456789");
    // Navigate to row 0, col 7.
    ed.handle_key(&key(KeyCode::Up));
    ed.handle_key(&key(KeyCode::Up));
    ed.handle_key(&KeyCode::Home.into());
    for _ in 0..7 {
        ed.handle_key(&key(KeyCode::Right));
    }
    assert_eq!(ed.cursor(), (0, 7));
    // Down to the short row clamps to its end (col 2)...
    ed.handle_key(&key(KeyCode::Down));
    assert_eq!(ed.cursor(), (1, 2));
    // ...then Down to the long row RESTORES the sticky goal column 7.
    ed.handle_key(&key(KeyCode::Down));
    assert_eq!(ed.cursor(), (2, 7));
}

#[test]
fn horizontal_motion_reseeds_goal_column() {
    let mut ed = InputEditor::new();
    ed.set_view_width(40);
    type_str(&mut ed, "abcdefghij");
    newline(&mut ed);
    type_str(&mut ed, "xy");
    newline(&mut ed);
    type_str(&mut ed, "0123456789");
    ed.handle_key(&key(KeyCode::Up));
    ed.handle_key(&key(KeyCode::Up));
    ed.handle_key(&KeyCode::Home.into());
    for _ in 0..7 {
        ed.handle_key(&key(KeyCode::Right));
    }
    ed.handle_key(&key(KeyCode::Down)); // row1 col2 (clamped)
    // A horizontal move re-seeds the goal: Left to col1, then Down must land at col1, not col7.
    ed.handle_key(&key(KeyCode::Left));
    assert_eq!(ed.cursor(), (1, 1));
    ed.handle_key(&key(KeyCode::Down));
    assert_eq!(ed.cursor(), (2, 1));
}

// ---- large-paste markers (spec/tui/03 §5.5) ------------------------------------------------

#[test]
fn large_multiline_paste_collapses_to_marker_and_expands_on_submit() {
    let mut ed = InputEditor::new();
    let big: String = (0..15).map(|i| format!("line {i}\n")).collect();
    ed.handle_paste(&big);
    // The buffer shows a compact marker, not the 15 lines.
    assert!(ed.text().contains("[paste #1 +16 lines]"), "buffer={:?}", ed.text());
    assert_eq!(ed.line_count(), 1);
    // expanded_text restores the full content.
    assert_eq!(ed.expanded_text(), big);
}

#[test]
fn small_paste_inserts_verbatim() {
    let mut ed = InputEditor::new();
    ed.handle_paste("just a line");
    assert_eq!(ed.text(), "just a line");
    assert_eq!(ed.expanded_text(), "just a line");
}

#[test]
fn backspace_deletes_whole_paste_marker_atomically() {
    let mut ed = InputEditor::new();
    let big = "x".repeat(1200);
    ed.handle_paste(&big);
    assert!(ed.text().contains("[paste #1"));
    // Cursor sits just after the marker's closing ']'. One Backspace removes the entire marker.
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(ed.text(), "");
    assert_eq!(ed.expanded_text(), "");
}

#[test]
fn backslash_enter_inserts_soft_newline_instead_of_submitting() {
    // Pi editor.ts:796-802 / spec/tui/03 §5.7: a terminals-without-Shift+Enter workaround. If the char
    // immediately before the cursor is a literal backslash, Enter deletes it and inserts a newline.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "foo\\");
    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(out, EditorOutcome::Edited, "backslash-Enter must NOT submit");
    // The backslash is gone and the line is broken: "foo" then an empty line, cursor at line start.
    assert_eq!(ed.text(), "foo\n");
    assert_eq!(ed.cursor(), (1, 0));
}

#[test]
fn plain_enter_still_submits_without_a_trailing_backslash() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello");
    match ed.handle_key(&key(KeyCode::Enter)) {
        EditorOutcome::Submit(text) => assert_eq!(text, "hello"),
        other => panic!("expected Submit, got {other:?}"),
    }
}

#[test]
fn backslash_not_immediately_before_cursor_still_submits() {
    // The guard only fires on the char *immediately* before the cursor. A backslash elsewhere on the
    // line (cursor moved left past it) submits normally.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "a\\b");
    match ed.handle_key(&key(KeyCode::Enter)) {
        EditorOutcome::Submit(text) => assert_eq!(text, "a\\b"),
        other => panic!("expected Submit, got {other:?}"),
    }
}

#[test]
fn submit_expands_paste_marker() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "prefix ");
    let big: String = (0..20).map(|i| format!("L{i}\n")).collect();
    ed.handle_paste(&big);
    type_str(&mut ed, " suffix");
    let out = ed.handle_key(&key(KeyCode::Enter));
    match out {
        EditorOutcome::Submit(text) => {
            assert!(text.starts_with("prefix "));
            assert!(text.ends_with(" suffix"));
            assert!(text.contains("L0\n"));
            assert!(!text.contains("[paste #"));
        }
        other => panic!("expected Submit, got {other:?}"),
    }
}
