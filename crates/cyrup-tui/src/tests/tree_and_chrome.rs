//! `/tree` selector-slot wiring + the compact startup-help bar, headless against `TestBackend`.
//!
//! Closes the two "built-but-unwired" gaps from `spec/gap-analysis/12-cyrup-tui.md`:
//! - the `TreeSelector` engine reaching the live input slot (`open_boxed_selector`) and routing its
//!   `Confirm` through to an [`AppCommand::ConfirmSelection`] for `navigate_tree` (Pi
//!   `tree-selector.ts` + `navigateTree`, agent-session.ts:2704);
//! - the `compactInstructions` startup bar (`chrome::render_compact_hints`, interactive-mode.ts:697)
//!   showing at startup and dismissing on the first submission.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::crossterm::event::KeyCode;
use crate::{App, AppAction, AppCommand, SelectorKind, TreeNode, TreeSelector, UiTheme};
use ratatui::backend::TestBackend;
use super::harness::*;

fn submit(app: &mut App<TestBackend>, line: &str) -> AppAction {
    app.editor_mut().set_text(line);
    app.handle_input(&key(KeyCode::Enter))
}

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

#[test]
fn tree_selector_occupies_the_slot_and_renders() {
    let mut app = new_app();
    let nodes = vec![
        TreeNode::message("e1", 0, "first prompt"),
        TreeNode::message("e2", 0, "second prompt"),
    ];
    app.open_boxed_selector(SelectorKind::Tree, Box::new(TreeSelector::new(nodes)));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    app.draw().unwrap();
    let text = buf_text(&app);
    // Header + the bespoke glyph + the node labels reach the buffer (the editor is swapped out).
    assert!(text.contains("Session Tree"), "tree header rendered:\n{text}");
    assert!(text.contains("first prompt"), "first node label rendered:\n{text}");
    assert!(text.contains('●'), "message glyph rendered:\n{text}");
}

#[test]
fn tree_selector_confirm_routes_navigate_to_the_run_loop() {
    let mut app = new_app();
    let nodes = vec![
        TreeNode::message("e1", 0, "first"),
        TreeNode::message("e2", 0, "second"),
    ];
    app.open_boxed_selector(SelectorKind::Tree, Box::new(TreeSelector::new(nodes)));
    // Move to the second row, then confirm → the run loop gets a ConfirmSelection{Tree, "e2"} it maps
    // to `navigate_tree("e2", …)`. The slot closes and the editor is restored.
    app.handle_input(&key(KeyCode::Down));
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        action,
        AppAction::Command(AppCommand::ConfirmSelection {
            kind: SelectorKind::Tree,
            value: "e2".to_string(),
        })
    );
    assert_eq!(app.active_selector_kind(), None, "slot closed on confirm");
}

#[test]
fn tree_selector_esc_cancels_and_restores_the_editor() {
    let mut app = new_app();
    app.editor_mut().set_text("draft");
    app.open_boxed_selector(
        SelectorKind::Tree,
        Box::new(TreeSelector::new(vec![TreeNode::message("e1", 0, "only")])),
    );
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None, "slot closed on cancel");
    assert_eq!(app.editor_mut().text(), "draft", "editor restored on cancel");
}

#[test]
fn startup_hint_bar_shows_then_dismisses_on_first_submission() {
    let mut app = new_app();
    app.draw().unwrap();
    let before = buf_text(&app);
    // The compact bar resolves its keys from the live keymap; the literal affordances are stable.
    assert!(before.contains("interrupt"), "startup interrupt hint:\n{before}");
    assert!(before.contains("commands"), "startup /commands hint:\n{before}");
    assert!(before.contains("bash"), "startup ! bash hint:\n{before}");

    // A real prompt submission dismisses the bar (Pi drops compactInstructions once chatting starts).
    assert!(matches!(submit(&mut app, "hello"), AppAction::Submit(_)));
    assert!(!app.state().show_startup_hints, "flag cleared on submit");
    app.draw().unwrap();
    let after = buf_text(&app);
    assert!(!after.contains("commands"), "hint bar gone after submission:\n{after}");
}

#[test]
fn startup_hint_bar_suppressed_while_a_selector_owns_the_slot() {
    let mut app = new_app();
    app.open_boxed_selector(
        SelectorKind::Tree,
        Box::new(TreeSelector::new(vec![TreeNode::message("e1", 0, "x")])),
    );
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(!text.contains("· commands"), "no startup bar under a selector:\n{text}");
}
