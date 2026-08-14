//! `/tree`'s label-timestamp column — Pi `tree-selector.ts:116` (`showLabelTimestamps = false`),
//! `:741-743` (the render condition) and `:658-660` (the `[+label time]` status marker).
//!
//! Pi's column shows `labelTimestamp`: the time an entry's **label** was set. It is off until the
//! `t` toggle (`app.tree.toggleLabelTimestamp`, `:1090`) turns it on, and even then it decorates
//! only rows that actually carry a label — `showLabelTimestamps && node.label && node.labelTimestamp`.
//!
//! cyrup had the toggle but not the value: the DAG→row projection fabricated the literal string
//! `"current"` for the branch tip, the column defaulted to ON, and the label gate was missing — so
//! `/tree` opened with the word "current" sitting in Pi's clock column, on an unlabeled row.
//!
//! What is asserted here is the render contract; the producer that will eventually fill the column
//! is the remaining half of the fix and lives in `cyrup-session`/`cyrup-session-svc` (see the
//! comment on `time_label` in `app.rs`'s `tree_node_from_dag`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_session_svc::{SessionDagKind, SessionDagNode};
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{
    tree_node_from_dag, App, InputEvent, SelectorKind, TreeNode, TreeSelector, UiTheme,
};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

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

/// Open `/tree` over `nodes` in an assembled app, exactly as the run loop's `/tree` arm does.
fn tree_app(nodes: Vec<TreeNode>) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.open_boxed_selector(SelectorKind::Tree, Box::new(TreeSelector::new(nodes)));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    app
}

/// Three rows. Row 0 is the highlighted anchor and is never moved by these tests; row 1 is labeled
/// and carries a label timestamp; row 2 carries the same kind of timestamp but no label. Pi
/// decorates row 1 only, and only once the toggle is on. (The highlighted row used to claim the
/// right-hand column for a cyrup-only `◀ selected` marker; that marker has no upstream analog and
/// was removed — see S37.)
fn labeled_and_unlabeled() -> Vec<TreeNode> {
    let anchor = TreeNode::message("e0", 0, "user: port the editor");

    let mut labeled = TreeNode::message("e1", 0, "user: wire up streaming");
    labeled.has_label = true;
    labeled.time_label = Some("12:04".to_string());

    let mut unlabeled = TreeNode::message("e2", 0, "user: fix the footer");
    unlabeled.has_label = false;
    unlabeled.time_label = Some("09:41".to_string());

    vec![anchor, labeled, unlabeled]
}

/// The projection must not invent a timestamp. Pi's `labelTimestamp` is a clock time on a labeled
/// row; the literal word `"current"` on the branch tip was neither, and it was the *only* producer
/// of the rendered column.
#[test]
fn the_dag_projection_no_longer_fabricates_a_current_timestamp() {
    let leaf = SessionDagNode {
        entry_id: "e9".into(),
        parent_id: None,
        depth: 0,
        label: "user: fix the footer".to_string(),
        kind: SessionDagKind::Message,
        foldable: false,
        // The exact shape that produced the fabrication: the active branch tip.
        is_leaf: true,
        has_label: true,
        timestamp: "2026-08-07T12:04:00.000Z".to_string(),
    };
    let row = tree_node_from_dag(&leaf);
    assert_ne!(
        row.time_label.as_deref(),
        Some("current"),
        "the branch tip must not be given the literal string \"current\" in Pi's label-timestamp \
         column"
    );
    assert_eq!(
        row.time_label, None,
        "nothing in this crate can source Pi's `labelTimestamp` yet, so the column must stay empty \
         rather than carry a stand-in"
    );
}

/// Pi's default is OFF (`private showLabelTimestamps = false`). `t` turns it on — and announces
/// itself in the header the way `getStatusLabels` does.
#[test]
fn the_label_timestamp_column_is_off_until_the_t_toggle() {
    let mut app = tree_app(labeled_and_unlabeled());
    app.draw().unwrap();
    let closed = buf_text(&app);
    assert!(
        closed.contains("wire up streaming"),
        "precondition: the labeled row is on screen:\n{closed}"
    );
    assert!(
        !closed.contains("12:04"),
        "the label-timestamp column must be OFF by default (Pi tree-selector.ts:116):\n{closed}"
    );
    assert!(
        !closed.contains("[+label time]"),
        "the header marker belongs to the ON state only:\n{closed}"
    );

    // `t` — `app.tree.toggleLabelTimestamp` (Pi `:1090`).
    app.handle_input(&key(KeyCode::Char('t')));
    app.draw().unwrap();
    let open = buf_text(&app);
    assert!(
        open.contains("12:04"),
        "`t` must reveal the labeled row's timestamp:\n{open}"
    );
    assert!(
        open.contains("[+label time]"),
        "Pi's `[+label time]` status marker is missing from the header (`getStatusLabels`, \
         tree-selector.ts:658-660):\n{open}"
    );
}

/// The column is a *label* timestamp: an entry with no label never shows one, even with the toggle
/// on and a timestamp attached (Pi's `flatNode.node.label &&` conjunct, `:742`).
#[test]
fn an_unlabeled_row_never_shows_a_label_timestamp() {
    let mut app = tree_app(labeled_and_unlabeled());
    app.handle_input(&key(KeyCode::Char('t')));
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(
        screen.contains("fix the footer"),
        "precondition: the unlabeled row is on screen:\n{screen}"
    );
    assert!(
        screen.contains("[+label time]"),
        "precondition: the toggle is ON:\n{screen}"
    );
    // The labeled sibling proves the toggle really is painting the column — so the absence below is
    // the label gate, not a column that failed to render at all.
    assert!(
        screen.contains("12:04"),
        "precondition: the column IS being painted for a labeled row:\n{screen}"
    );
    assert!(
        !screen.contains("09:41"),
        "an unlabeled entry must not render a label timestamp:\n{screen}"
    );
}
