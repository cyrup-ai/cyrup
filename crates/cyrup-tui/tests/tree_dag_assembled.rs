//! Feature #2 — `/tree` over the real session DAG, proven by an **assembled** `App` render. The
//! connector/fold/filter engine already existed but was data-starved (a flat user-message list); the
//! new `AgentSession::session_dag` getter (cyrup-session-svc) supplies a real multi-branch DAG. Here we
//! open the `/tree` selector in the assembled app over a **multi-branch** node set shaped exactly like
//! `session_dag` → `tree_node_from_dag` produces (a foldable root with two child branches, one nested)
//! and assert the rendered buffer carries branch **connectors** (`├─`/`└─`), **more than one node**,
//! and **fold markers** (`⊟`/`⊞`) — the whole tree, not a flat list (Pi `tree-selector.ts:691-727`).
//!
//! The getter itself is proven over a REAL multi-branch `AgentSession` in
//! `cyrup-session-svc/tests/session_dag.rs`; this test proves the assembled TUI render of that data.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, InputEvent, SelectorKind, TreeNode, TreeSelector, UiTheme};
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

/// A multi-branch flattened DAG in pre-order, matching `session_dag` output: a foldable depth-0 root
/// (`user: port the editor`) with two children — an assistant reply and a second user message that
/// itself branches (foldable, depth 1) into a nested assistant reply (depth 2) — plus a sibling final
/// user message. This is exactly the `FlatNode[]` shape `tree_node_from_dag` yields.
fn multi_branch_nodes() -> Vec<TreeNode> {
    let mut root = TreeNode::message("e0", 0, "user: port the editor");
    root.foldable = true;

    let assistant = TreeNode::message("e1", 1, "assistant: on it");

    let mut branch = TreeNode::message("e2", 1, "user: wire up streaming");
    branch.foldable = true;
    branch.has_label = true;

    let nested = TreeNode::message("e3", 2, "assistant: streaming wired");

    let mut fix = TreeNode::message("e4", 1, "user: fix the footer");
    fix.time_label = Some("current".to_string());

    vec![root, assistant, branch, nested, fix]
}

#[test]
fn assembled_tree_open_shows_connectors_multiple_nodes_and_fold_markers() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude-opus-4-6");
    // Open `/tree` the way the run loop does for the bespoke selector (`open_boxed_selector`).
    let tree = TreeSelector::new(multi_branch_nodes());
    app.open_boxed_selector(SelectorKind::Tree, Box::new(tree));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Tree));
    app.draw().unwrap();

    let screen = buf_text(&app);
    // (>1 node) The real branch tree renders multiple entries, not a flat single-column list.
    assert!(screen.contains("port the editor"), "root node missing:\n{screen}");
    assert!(screen.contains("wire up streaming"), "second branch node missing:\n{screen}");
    assert!(screen.contains("streaming wired"), "nested (depth-2) node missing:\n{screen}");
    // (connectors) child rows draw `├─`/`└─` connectors — the DAG structure, not a flat list.
    assert!(
        screen.contains("├─") || screen.contains("└─"),
        "branch connectors (├─/└─) missing — tree rendered flat:\n{screen}"
    );
    // (fold markers) the foldable nodes draw an open-fold `⊟` marker.
    assert!(screen.contains('⊟'), "fold marker `⊟` missing from foldable nodes:\n{screen}");
    // The header proves the tree selector (not a plain list) owns the slot.
    assert!(screen.contains("Session Tree"), "tree header missing:\n{screen}");
}

#[test]
fn assembled_tree_fold_toggles_marker_in_the_render() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    let tree = TreeSelector::new(multi_branch_nodes());
    app.open_boxed_selector(SelectorKind::Tree, Box::new(tree));
    app.draw().unwrap();
    assert!(buf_text(&app).contains('⊟'), "expected an open-fold marker before folding");

    // `z` folds the highlighted (root) node → its subtree collapses and the marker flips to `⊞`.
    app.handle_input(&key(KeyCode::Char('z')));
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(screen.contains('⊞'), "folded marker `⊞` missing after folding the root:\n{screen}");
    // The nested descendant is hidden once the root is folded (fold engine over the DAG).
    assert!(
        !screen.contains("streaming wired"),
        "folded subtree still visible after collapse:\n{screen}"
    );
}
