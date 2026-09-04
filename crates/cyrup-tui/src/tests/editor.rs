//! Input-editor unit tests (R-10-015): typing, backspace, cursor movement, newline, submit.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use super::harness::key_event as key;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{Component, EditorOutcome, InputEditor, UiTheme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::style::Modifier;

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
    assert_eq!(
        after_family, 1,
        "Left did not step the whole grapheme cluster as one unit"
    );
    assert!(
        after_b - after_family >= 5,
        "the cluster should span ≥5 scalar columns"
    );
    // Right steps back over the whole cluster in one move.
    ed.handle_key(&key(KeyCode::Right));
    assert_eq!(ed.cursor().1, after_b);
}

#[test]
fn backspace_removes_whole_grapheme_cluster() {
    let mut ed = InputEditor::new();
    ed.set_text("a👨‍👩‍👧");
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(
        ed.text(),
        "a",
        "backspace must delete the entire emoji cluster, not one scalar"
    );
    assert_eq!(ed.cursor(), (0, 1));
}

#[test]
fn forward_delete_removes_whole_grapheme_cluster() {
    let mut ed = InputEditor::new();
    ed.set_text("👨‍👩‍👧b");
    ed.handle_key(&key(KeyCode::Home));
    ed.handle_key(&key(KeyCode::Delete));
    assert_eq!(
        ed.text(),
        "b",
        "forward-delete must remove the entire emoji cluster"
    );
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

/// TUI-067 — `tui.input.copy` is the one upstream id whose job is to make the editor DECLINE a key
/// (`editor.ts:653-655`'s bare `return`). It had no `EditorAction` destination, so `merge_entries`
/// dropped the entry silently and the rebind did nothing.
#[test]
fn tui_input_copy_rebind_makes_the_editor_decline_the_key() {
    // Stock: the id is bound to nothing, so 'q' is still ordinary typed text.
    let mut ed = InputEditor::new();
    assert_eq!(
        ed.handle_key(&key(KeyCode::Char('q'))),
        EditorOutcome::Edited
    );
    assert_eq!(ed.text(), "q");

    // The discriminating case. Rebound onto a PRINTABLE chord the editor would otherwise insert,
    // the declination has to win — this is the assertion that fails without the `from_id` arm,
    // because a dropped entry leaves 'q' typing itself.
    let mut ed = InputEditor::new();
    let issues = ed
        .merge_keybindings_json(r#"{ "tui.input.copy": "q" }"#)
        .unwrap();
    assert!(
        issues.is_empty(),
        "a known id with a valid chord reports no issue: {issues:?}"
    );
    assert_eq!(
        ed.handle_key(&key(KeyCode::Char('q'))),
        EditorOutcome::Ignored
    );
    assert!(ed.is_empty(), "a declined key must not be inserted");

    // The Ctrl+Q form from the row's own Verify clause: resolved, declined, buffer untouched.
    let mut ed = InputEditor::new();
    ed.merge_keybindings_json(r#"{ "tui.input.copy": "ctrl+q" }"#)
        .unwrap();
    assert_eq!(ed.handle_key(&ctrl('q')), EditorOutcome::Ignored);
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
    assert!(
        ed.is_bash_mode(),
        "leading whitespace before ! must still be bash mode"
    );
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
    assert!(
        map.len() >= 2,
        "long line must wrap into >1 visual line: {map:?}"
    );
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
    assert!(
        col > 0,
        "cursor advanced to the next visual line, col={col}"
    );
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
    assert!(
        ed.text().contains("[paste #1 +16 lines]"),
        "buffer={:?}",
        ed.text()
    );
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
    assert_eq!(
        out,
        EditorOutcome::Edited,
        "backslash-Enter must NOT submit"
    );
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

// ---- wrap-aware RENDER + height: long/pasted lines wrap AND grow the box (usability bug) ----

/// Render an editor into a `w x h` headless `TestBackend` and return per-row `(symbols, reversed)`
/// so tests can assert a long line flows across visual rows (nothing clipped) and where the
/// reverse-video soft cursor lands. Row `y = 0` is the top rule; content starts at `y = 1`.
fn render_rows(ed: &mut InputEditor, w: u16, h: u16) -> Vec<(String, bool)> {
    let theme = UiTheme::dark();
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| ed.render(f, Rect::new(0, 0, w, h), &theme))
        .unwrap();
    let buf = term.backend().buffer();
    let area = buf.area;
    let mut rows = Vec::with_capacity(area.height as usize);
    for y in 0..area.height {
        let mut sym = String::new();
        let mut reversed = false;
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                sym.push_str(cell.symbol());
                reversed |= cell.modifier.contains(Modifier::REVERSED);
            }
        }
        rows.push((sym, reversed));
    }
    rows
}

#[test]
fn long_line_wraps_across_visual_rows_nothing_clipped() {
    // (a) A single LOGICAL line wider than the box wraps within the width and flows onto the next
    // row — the old renderer emitted one Line per logical line with no `.wrap`, clipping the tail.
    // width 14 → view_width 13 → "alpha bravo charlie" wraps to "alpha bravo " + "charlie".
    let mut ed = InputEditor::new();
    type_str(&mut ed, "alpha bravo charlie");
    let rows = render_rows(&mut ed, 14, 8);
    // The head sits on the first content row (y=1), the wrapped tail on the second (y=2).
    assert!(
        rows[1].0.contains("alpha") && rows[1].0.contains("bravo"),
        "first visual row missing the head:\n{rows:#?}"
    );
    assert!(
        rows[2].0.contains("charlie"),
        "second visual row missing the wrapped tail (clipped, not wrapped):\n{rows:#?}"
    );
    // The full text is present across the visual rows — nothing was dropped by clipping.
    let visible: String = rows.iter().map(|(s, _)| s.as_str()).collect();
    for word in ["alpha", "bravo", "charlie"] {
        assert!(
            visible.contains(word),
            "word {word:?} clipped from render:\n{rows:#?}"
        );
    }
}

#[test]
fn visual_line_count_grows_for_a_long_line() {
    // (b) The app sizes the editor slot from the VISUAL line count (app.rs region_constraints), so a
    // long line must report >=2 visual lines at the render content width (area.width - 1) to grow the
    // box; an empty buffer stays a single visual line (slot floor).
    let empty = InputEditor::new();
    assert_eq!(
        empty.visual_line_count(13),
        1,
        "empty buffer is a single visual line"
    );
    let mut ed = InputEditor::new();
    type_str(&mut ed, "alpha bravo charlie");
    assert!(
        ed.visual_line_count(13) >= 2,
        "long line did not grow the slot: count={}",
        ed.visual_line_count(13)
    );
    // A wide width fits the whole line on one visual row again (no spurious growth).
    assert_eq!(
        ed.visual_line_count(80),
        1,
        "short-enough width should not wrap"
    );
}

#[test]
fn cursor_at_end_of_wrapped_line_renders_on_second_row() {
    // (c) The caret at the end of a wrapped long line rides the SECOND visual row (Pi
    // editor.ts:545-551): the reverse-video soft cursor must paint on that row, and the hardware
    // cursor position must map there too.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "alpha bravo charlie"); // cursor at end (col 19) → last visual line "charlie"
    let rows = render_rows(&mut ed, 14, 8);
    assert!(
        !rows[1].1,
        "soft cursor must NOT be on the first visual row:\n{rows:#?}"
    );
    assert!(
        rows[2].1,
        "soft cursor (reverse cell) missing from the second visual row:\n{rows:#?}"
    );
    // The hardware-cursor y maps to the second content row (area.y + 1 + vrow == 2).
    let (_, cy) = ed.cursor_in(Rect::new(0, 0, 14, 8)).unwrap();
    assert_eq!(
        cy, 2,
        "hardware cursor not on the second visual row:\n{rows:#?}"
    );
}

// ---- paste-registry invariants: undo, word atomicity, marker grammar -----------------------
//
// TUI-042 / TUI-043 / TUI-044 / TUI-048 / TUI-049 / TUI-053, all reproduced live on 2026-08-13
// (docs/gap-analysis/REPRO-LOG.md) with the model's actual input read out of the session JSONL.

/// A single-line paste over 1000 chars — `[paste #N 1500 chars]` (`editor.ts:1206-1210`).
fn big_paste(n: usize, fill: char) -> String {
    std::iter::repeat_n(fill, n).collect()
}

#[test]
fn undo_restores_the_paste_registry_not_just_the_marker_text() {
    // TUI-042 (critical). RED before the fix: the marker text came back and `expanded_text()` — the
    // string `E::Submit` hands the agent — was the literal 21 characters `[paste #1 1500 chars]`,
    // because `Snapshot` carried no registry. pi snapshots `{ state, pastes, pasteCounter }`
    // (`editor.ts:216-220`, `:2012-2014`) and restores all three (`:2016-2030`).
    let mut ed = InputEditor::new();
    let big = big_paste(1500, 'z');
    ed.handle_paste(&big);
    assert_eq!(ed.text(), "[paste #1 1500 chars]");
    // One Backspace: the marker is atomic, so it vanishes whole and takes its registry entry.
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(ed.text(), "");
    // Undo. The marker is back on screen AND resolvable again.
    ed.handle_key(&ctrl('-'));
    assert_eq!(ed.text(), "[paste #1 1500 chars]");
    assert_eq!(
        ed.expanded_text(),
        big,
        "undo restored the marker text but not its content"
    );
    assert_eq!(ed.expanded_text().chars().count(), 1500);
}

#[test]
fn undo_rolls_back_the_paste_counter_so_the_next_paste_is_still_marker_one() {
    // TUI-042's quieter variant, also reproduced live: cyrup re-issued `#2` where pi re-issues `#1`,
    // because `paste_counter` was bumped *before* the undo snapshot was pushed and never rolled back.
    let mut ed = InputEditor::new();
    ed.handle_paste(&big_paste(1200, 'a'));
    assert_eq!(ed.text(), "[paste #1 1200 chars]");
    ed.handle_key(&ctrl('-'));
    assert_eq!(ed.text(), "", "undo must remove the pasted marker entirely");
    let second = big_paste(1300, 'b');
    ed.handle_paste(&second);
    assert_eq!(
        ed.text(),
        "[paste #1 1300 chars]",
        "the undone id must be re-issued, not skipped"
    );
    assert_eq!(ed.expanded_text(), second);
}

#[test]
fn undo_restores_the_snapshot_cursor_column() {
    // TUI-044, the item's own scenario, confirmed live by two readouts. `undo()` used to keep the
    // LIVE column and merely clamp it, so the next keystroke edited a position the user never chose.
    // pi's `Object.assign(this.state, snapshot.state)` restores both coordinates (`editor.ts:2019`,
    // `EditorState` at `:209-213`).
    let mut ed = InputEditor::new();
    type_str(&mut ed, "world");
    ed.handle_key(&ctrl('u')); // kill ring ← "world", buffer empty
    type_str(&mut ed, "hello");
    assert_eq!(ed.cursor(), (0, 5));
    ed.handle_key(&ctrl('y')); // yank → "helloworld" (snapshot taken at col 5)
    assert_eq!(ed.text(), "helloworld");
    for _ in 0..8 {
        ed.handle_key(&key(KeyCode::Left));
    }
    assert_eq!(ed.cursor(), (0, 2));
    ed.handle_key(&ctrl('-'));
    assert_eq!(ed.text(), "hello");
    assert_eq!(
        ed.cursor(),
        (0, 5),
        "undo must restore the snapshot's column, not clamp the live one"
    );
    // …and the very next keystroke therefore lands where pi puts it.
    type_str(&mut ed, "Z");
    assert_eq!(ed.text(), "helloZ");
}

#[test]
fn ctrl_w_at_a_marker_end_deletes_the_whole_marker() {
    // TUI-043 (critical). RED before the fix: exactly one character — the closing `]` — was deleted,
    // the marker stopped matching, and Enter sent the 20/21-char fragment to the model. pi's
    // `findWordBackward` skips one atomic segment whole (`word-navigation.ts:44-46`), and
    // `deleteWordBackwards` inherits that by computing its range from `moveWordBackwards`
    // (`editor.ts:1613-1616`).
    let mut ed = InputEditor::new();
    let big = big_paste(1500, 'q');
    ed.handle_paste(&big);
    ed.handle_key(&ctrl('w'));
    assert_eq!(
        ed.text(),
        "",
        "Ctrl+W must take the whole marker, not just its ']'"
    );
    assert_eq!(ed.kill_ring_top(), Some("[paste #1 1500 chars]"));
    assert_eq!(ed.expanded_text(), "");
    // Upstream's `deleteWordBackwards` has NO paste branch — it slices text and leaves the registry
    // alone (`editor.ts:1607-1630`), so the undo snapshot still resolves the marker.
    ed.handle_key(&ctrl('-'));
    assert_eq!(ed.expanded_text(), big);
}

#[test]
fn alt_d_at_a_marker_start_deletes_the_whole_marker() {
    // The mirror of the above: `findWordForward`'s atomic branch (`word-navigation.ts:97-99`).
    let mut ed = InputEditor::new();
    ed.handle_paste(&big_paste(1500, 'w'));
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&alt(KeyCode::Char('d')));
    assert_eq!(
        ed.text(),
        "",
        "Alt+D must take the whole marker, not just its '['"
    );
}

#[test]
fn word_motion_treats_a_paste_marker_as_one_unit() {
    // TUI-043's cursor half: measured live, Alt+Left from the marker's end (col 20) landed at col 19
    // — inside the marker, where the next keystroke corrupts it. pi lands at col 0.
    let mut ed = InputEditor::new();
    ed.handle_paste(&big_paste(1500, 'e'));
    assert_eq!(ed.cursor(), (0, 21));
    ed.handle_key(&alt(KeyCode::Left));
    assert_eq!(ed.cursor(), (0, 0), "word-left must clear the whole marker");
    ed.handle_key(&alt(KeyCode::Right));
    assert_eq!(
        ed.cursor(),
        (0, 21),
        "word-right must clear the whole marker"
    );
}

#[test]
fn arrow_keys_step_over_a_paste_marker_as_one_grapheme() {
    // pi's `moveCursor` steps by `this.segment(text, "grapheme")`, which merges markers
    // (`editor.ts:1808-1830`), so the caret can never be parked inside one. cyrup used plain
    // grapheme boundaries, so Left from the end landed on the `]`.
    let mut ed = InputEditor::new();
    ed.handle_paste(&big_paste(1500, 'r'));
    ed.handle_key(&key(KeyCode::Left));
    assert_eq!(ed.cursor(), (0, 0));
    ed.handle_key(&key(KeyCode::Right));
    assert_eq!(ed.cursor(), (0, 21));
}

#[test]
fn a_hand_typed_marker_shaped_string_is_not_expanded() {
    // TUI-049: `marker_at` accepted any body between `[paste #N ` and `]`, so text the user typed was
    // silently replaced by unrelated content in the message the model received. pi's grammar is
    // `/^\[paste #(\d+)( (\+\d+ lines|\d+ chars))?\]$/` (`editor.ts:24`).
    let mut ed = InputEditor::new();
    let big = big_paste(1500, 'm');
    ed.handle_paste(&big);
    newline(&mut ed);
    type_str(&mut ed, "[paste #1 see above]");
    let expanded = ed.expanded_text();
    assert!(
        expanded.starts_with(&big),
        "the REAL marker must still expand"
    );
    assert!(
        expanded.ends_with("\n[paste #1 see above]"),
        "a hand-typed marker must survive verbatim: {:?}",
        &expanded[expanded.len().saturating_sub(40)..]
    );
    // The bare form pi's regex does allow is still a marker.
    ed.clear();
    ed.handle_paste(&big);
    ed.handle_key(&ctrl('u'));
    type_str(&mut ed, "[paste #1]");
    assert_eq!(
        ed.expanded_text(),
        big,
        "`[paste #N]` is a legal marker upstream"
    );
}

#[test]
fn deleting_a_marker_renumbers_the_pastes_that_follow_it() {
    // `handleBackspace`'s paste branch (`editor.ts:1293-1315`): drop the entry, decrement the
    // counter, shift the higher ids down and renumber the markers in the buffer. cyrup did only
    // `pastes.remove(&id)`, so ids drifted from pi's for the life of the session.
    let mut ed = InputEditor::new();
    let first = big_paste(1200, 'x');
    let second = big_paste(1300, 'y');
    ed.handle_paste(&first);
    ed.handle_paste(&second);
    assert_eq!(ed.text(), "[paste #1 1200 chars][paste #2 1300 chars]");
    // Caret just past the FIRST marker, then Backspace.
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&key(KeyCode::Right));
    assert_eq!(ed.cursor(), (0, 21));
    ed.handle_key(&key(KeyCode::Backspace));
    assert_eq!(
        ed.text(),
        "[paste #1 1300 chars]",
        "the survivor must be renumbered to #1"
    );
    assert_eq!(
        ed.expanded_text(),
        second,
        "…and its content must follow the renumbering"
    );
}

#[test]
fn browsing_history_away_from_a_draft_keeps_its_paste_registry() {
    // The `history_draft` path reuses `Snapshot`, so it carries the registry now too. Entering
    // history browsing also pushes an undo snapshot upstream (`editor.ts:435-438`), which cyrup
    // never did — so Ctrl+- could not undo "I browsed away from what I was typing".
    let mut ed = InputEditor::new();
    ed.push_history("an earlier prompt");
    let big = big_paste(1500, 'h');
    ed.handle_paste(&big);
    ed.handle_key(&KeyCode::Home.into()); // col 0 → Up recalls history
    ed.handle_key(&key(KeyCode::Up));
    assert_eq!(ed.text(), "an earlier prompt");
    ed.handle_key(&key(KeyCode::Down));
    assert_eq!(
        ed.text(),
        "[paste #1 1500 chars]",
        "the draft must come back"
    );
    assert_eq!(ed.expanded_text(), big, "…with its paste still resolvable");

    // The Down path above was already correct at HEAD (nothing clears the registry while browsing).
    // The path that was NOT is Ctrl+-: upstream pushes an undo snapshot on entering history browsing
    // (`editor.ts:436`), so undo returns to the draft. cyrup pushed none, so the undo fell through to
    // the snapshot from before the paste and emptied the buffer instead.
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&key(KeyCode::Up));
    assert_eq!(ed.text(), "an earlier prompt");
    ed.handle_key(&ctrl('-'));
    assert_eq!(
        ed.text(),
        "[paste #1 1500 chars]",
        "undo must return to the draft, not past it"
    );
    assert_eq!(ed.expanded_text(), big);
}

#[test]
fn ctrl_undo_is_reachable_from_a_terminal_without_the_kitty_protocol() {
    // TUI-053: a legacy terminal sends Ctrl+- as the single byte 0x1F, which crossterm 0.29.0
    // decodes arithmetically to `Char('7') + CONTROL` (`event/sys/unix/parse.rs:110-113`). pi decodes
    // the byte explicitly — `if (data === "\x1f") return "ctrl+-"` (`keys.ts:1277`) — so undo works
    // everywhere upstream. RED before the fix: nothing happened on Terminal.app/xterm/gnome-terminal.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "foo bar baz");
    ed.handle_key(&ctrl('7'));
    assert_eq!(
        ed.text(),
        "foo bar",
        "0x1F (ctrl+7 as crossterm renders it) must reach editor.undo"
    );
    // `\x1d` → `ctrl+]` (`keys.ts:1276`) is the same class: char-jump forward.
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&ctrl('5'));
    ed.handle_key(&key(KeyCode::Char('b')));
    assert_eq!(ed.cursor(), (0, 4), "0x1D must reach editor.jumpForward");
}

#[test]
fn word_motion_keeps_pis_ascii_boundaries_after_the_segmenter_swap() {
    // TUI-048 replaced the character-class run with UAX#29 word segmentation plus pi's
    // `PUNCTUATION_REGEX` sub-boundaries (`word-navigation.ts:47-57`, `utils.ts:821`). These are the
    // cases where the two must agree, pinned so the swap cannot silently move them.
    for (text, from, expect) in [
        ("foo.bar", 7usize, 4usize),
        ("don't", 5, 4),
        ("3.14", 4, 2),
        ("foo bar", 7, 4),
        ("a  b", 4, 3),
    ] {
        let mut ed = InputEditor::new();
        type_str(&mut ed, text);
        assert_eq!(ed.cursor(), (0, from));
        ed.handle_key(&alt(KeyCode::Left));
        assert_eq!(ed.cursor(), (0, expect), "word-left in {text:?}");
    }
    // Forward takes the FIRST punctuation match inside a word-like segment (`word-navigation.ts:102`).
    let mut ed = InputEditor::new();
    type_str(&mut ed, "foo.bar");
    ed.handle_key(&KeyCode::Home.into());
    ed.handle_key(&alt(KeyCode::Right));
    assert_eq!(ed.cursor(), (0, 3));
}

#[test]
fn cjk_word_motion_no_longer_swallows_the_whole_run() {
    // TUI-048's headline case. The old class-run motion treated `你好世界` as ONE alphanumeric run
    // and jumped to column 0. UAX#29 segments it per ideograph, so the caret now stops inside the
    // run. **This is not yet parity**: ICU's `Intl.Segmenter` adds a dictionary pass that lands pi at
    // column 2 (`你好` / `世界`), and `unicode-segmentation` carries no such data — hence the range
    // assertion rather than a fixed column, and hence TUI-048 stays open. See the CYRUP-DELTA on
    // `InputEditor::word_segments`.
    let mut ed = InputEditor::new();
    type_str(&mut ed, "你好世界");
    assert_eq!(ed.cursor(), (0, 4));
    ed.handle_key(&alt(KeyCode::Left));
    let (_, col) = ed.cursor();
    assert!(
        col > 0 && col < 4,
        "word-left jumped the whole ideograph run: col {col}"
    );
}

#[test]
fn a_motion_between_two_kills_starts_a_new_kill_ring_entry() {
    // Every motion clears `lastAction` upstream (`editor.ts:1791`, `:1783`, `:1787`, `:1870`,
    // `:2065`, `:430`); cyrup cleared it on Left/Right only, so a Home between two kills let the
    // second accumulate into the first entry. RED at HEAD: the ring top was "worldhello ".
    let mut ed = InputEditor::new();
    type_str(&mut ed, "hello world");
    ed.handle_key(&ctrl('w')); // kill "world"
    assert_eq!(ed.kill_ring_top(), Some("world"));
    ed.handle_key(&KeyCode::Home.into()); // a motion → the kill run ends
    ed.handle_key(&ctrl('k')); // kill "hello " into a NEW entry
    assert_eq!(
        ed.kill_ring_top(),
        Some("hello "),
        "a motion must break the kill run"
    );
}

/// TUI-061 — `set_text` is Pi's PROGRAMMATIC `setText`, not the internal one.
///
/// RED at HEAD: one `set_text` served both the programmatic replacement and history browsing, so a
/// programmatic buffer replacement left the paste registry live (a subsequently hand-typed
/// `[paste #1 …]` still expanded — TUI-049's surface, narrowed but not closed by that fix) and was
/// not undoable. Pi splits them: `setText` (`editor.ts:1010-1021` @v0.83.0) cancels the
/// autocomplete, clears `lastAction`, exits history browsing, pushes an undo snapshot **when the
/// content actually differs**, and does `this.pastes.clear(); this.pasteCounter = 0;` before
/// delegating; `setTextInternal` (`:1043-1056`) — "Internal setText that doesn't reset history
/// state - used by navigateHistory" — does none of it.
#[test]
fn set_text_clears_the_paste_registry_and_is_undoable() {
    let mut ed = InputEditor::new();
    ed.handle_paste(&"x".repeat(1500));
    assert_eq!(ed.text(), "[paste #1 1500 chars]");
    assert!(ed.expanded_text().contains(&"x".repeat(1500)));

    ed.set_text("replaced");
    // The registry is gone, so a hand-retyped marker is literal text.
    ed.set_text("[paste #1 1500 chars]");
    assert_eq!(
        ed.expanded_text(),
        "[paste #1 1500 chars]",
        "a programmatic replacement must clear the registry"
    );
}

/// The undo snapshot half: a `set_text` that CHANGES the content is undoable (`editor.ts:1015-1017`,
/// "makes programmatic changes undoable").
#[test]
fn a_programmatic_set_text_is_undoable() {
    let mut ed = InputEditor::new();
    ed.set_text("before");
    ed.set_text("after");
    ed.handle_key(&KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL));
    assert_eq!(
        ed.text(),
        "before",
        "Ctrl+- must undo a programmatic replacement"
    );
}

/// Browsing history must NOT go through the external form — it uses `setTextInternal`, so the draft
/// it is about to restore keeps its registry (`navigateHistory`, `editor.ts:435-452`).
#[test]
fn browsing_history_does_not_clear_the_draft_registry() {
    let mut ed = InputEditor::new();
    ed.push_history("older");
    ed.handle_paste(&"y".repeat(1500));
    assert_eq!(ed.text(), "[paste #1 1500 chars]");
    // Up only recalls history when the caret is already at the start of a non-empty buffer:
    // `editor.ts:821-831` gates on `isOnFirstVisualLine() && (isEditorEmpty() || historyIndex > -1
    // || cursorCol === 0)` and otherwise does `moveToLineStart()`. The paste leaves the caret at
    // the end of the marker, so the first Up is that line-start move and the buffer is untouched.
    ed.handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        ed.text(),
        "[paste #1 1500 chars]",
        "the first Up only parks the caret at col 0"
    );
    assert_eq!(
        ed.cursor(),
        (0, 0),
        "…which is what makes the next Up eligible"
    );
    // Now Up into history, then Down back to the draft.
    ed.handle_key(&KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(ed.text(), "older");
    ed.handle_key(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(ed.text(), "[paste #1 1500 chars]");
    assert!(
        ed.expanded_text().contains(&"y".repeat(1500)),
        "the draft's paste registry must survive a history round trip"
    );
}
