//! Accepting a completion from the keyboard is one undo step, exactly as accepting it with a click
//! is.
//!
//! Every place pi's `Editor` applies a completion opens with `this.pushUndoSnapshot();
//! this.lastAction = null;` — the Tab branch and the Enter branch of the open popup
//! (`components/editor.ts:770-772`, `:791-793` @v1.0.0), the list's `onSelect` (`:2243-2244`) and
//! the forced single-match auto-apply (`:2394-2395`). So `undo` after any of them restores the text
//! from before the completion, and what is typed next starts a new undo unit instead of folding
//! into the run the completion interrupted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::key_event as key;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{EditorOutcome, InputEditor};

fn type_str(ed: &mut InputEditor, s: &str) {
    for c in s.chars() {
        ed.handle_key(&key(KeyCode::Char(c)));
    }
}

/// `tui.editor.undo`.
fn undo(ed: &mut InputEditor) {
    ed.handle_key(&KeyEvent::new(KeyCode::Char('-'), KeyModifiers::CONTROL));
}

/// A scratch cwd holding `src/` and `srcmap.md`, so a path query for `sr` has two candidates and a
/// forced Tab opens a list instead of auto-applying a lone match.
fn path_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("srcmap.md"), "").unwrap();
    dir
}

#[test]
fn undo_after_tab_accepting_a_slash_command_restores_the_typed_prefix() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/sett");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "/settings ");
    undo(&mut ed);
    assert_eq!(ed.text(), "/sett");
}

#[test]
fn undo_after_tab_accepting_a_file_from_a_popup_restores_the_typed_prefix() {
    let dir = path_fixture();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text("open sr");
    ed.handle_key(&key(KeyCode::Tab));
    assert!(ed.autocomplete_open(), "Tab opens the forced file list");
    ed.handle_key(&key(KeyCode::Down));
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "open srcmap.md ");
    undo(&mut ed);
    assert_eq!(ed.text(), "open sr");
}

#[test]
fn undo_after_enter_accepting_an_argument_restores_the_typed_prefix() {
    let mut ed = InputEditor::new();
    ed.set_argument_sources(crate::ArgumentSources {
        thinking_levels: vec!["low".to_string(), "high".to_string()].into(),
        ..Default::default()
    });
    type_str(&mut ed, "/thinking hi");
    assert!(ed.autocomplete_open());
    let out = ed.handle_key(&key(KeyCode::Enter));
    assert_eq!(
        out,
        EditorOutcome::Edited,
        "an argument accept does not submit"
    );
    assert_eq!(ed.text(), "/thinking high");
    undo(&mut ed);
    assert_eq!(ed.text(), "/thinking hi");
}

#[test]
fn undo_after_the_single_match_auto_apply_restores_the_typed_prefix() {
    let dir = path_fixture();
    let mut ed = InputEditor::new();
    ed.set_cwd(dir.path().to_path_buf());
    ed.set_text("open srcm");
    ed.handle_key(&key(KeyCode::Tab));
    assert_eq!(ed.text(), "open srcmap.md ");
    undo(&mut ed);
    assert_eq!(ed.text(), "open srcm");
}

/// Enter on a slash item applies the completion and falls through to submit; the submit clears the
/// undo stack (`this.undoStack.clear()`, `editor.ts:1268`), so the snapshot the accept took must
/// not survive to resurrect the line.
#[test]
fn enter_accepting_a_slash_command_submits_and_leaves_nothing_to_undo() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/tre");
    assert_eq!(
        ed.handle_key(&key(KeyCode::Enter)),
        EditorOutcome::Submit("/tree".to_string())
    );
    assert!(ed.is_empty());
    undo(&mut ed);
    assert!(ed.is_empty(), "the submit cleared the undo stack");
}

/// `this.lastAction = null` after the snapshot: typing after an accepted completion is a new undo
/// unit, so one undo takes back only what was typed, not the completion with it.
#[test]
fn typing_after_an_accepted_completion_is_its_own_undo_unit() {
    let mut ed = InputEditor::new();
    type_str(&mut ed, "/sett");
    ed.handle_key(&key(KeyCode::Tab));
    type_str(&mut ed, "a");
    assert_eq!(ed.text(), "/settings a");
    undo(&mut ed);
    assert_eq!(ed.text(), "/settings ");
    undo(&mut ed);
    assert_eq!(ed.text(), "/sett");
}
