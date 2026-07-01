//! Rich tool-execution rendering + Ctrl+O expand + diff result (tool-execution.ts / diff.ts; gap 3).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::{Component, TranscriptView, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

fn render(view: &mut TranscriptView, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let theme = UiTheme::dark();
    term.draw(|f| view.render(f, Rect::new(0, 0, w, h), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn running_tool_shows_gear_marker_with_args() {
    let mut view = TranscriptView::new();
    view.push_tool_start("read", Some("src/main.rs".to_string()));
    assert!(view.has_active(), "a live tool keeps the viewport active");
    let text = render(&mut view, 60, 8);
    assert!(text.contains("⚙ read(src/main.rs)"), "running marker + args: {text:?}");
}

#[test]
fn finished_tool_collapses_result_until_expanded() {
    let mut view = TranscriptView::new();
    view.push_tool_start("bash", Some("ls".to_string()));
    // 25 lines so the collapsed tail-20 block (spec/tui/06 §5.4) hides the first 5 (audit #7: the
    // dominant agent surface is the spec block — a tail-20 preview, NOT a head-1 one-liner).
    let body: String = (1..=25).map(|i| format!("ln{i:02}")).collect::<Vec<_>>().join("\n");
    view.push_tool_end("bash", false, Some(body));
    let collapsed = render(&mut view, 60, 30);
    assert!(collapsed.contains("✓ bash(ls)"), "done marker: {collapsed:?}");
    // The tail is shown (ln25), the head is hidden (ln01..ln05), with a hidden-count hint.
    assert!(collapsed.contains("ln25"), "tail line previewed: {collapsed:?}");
    assert!(collapsed.contains("ln06"), "20-line tail starts at ln06: {collapsed:?}");
    assert!(collapsed.contains("5 more lines"), "collapsed hidden-head hint present: {collapsed:?}");
    assert!(!collapsed.contains("ln01"), "head hidden when collapsed: {collapsed:?}");

    // Ctrl+O expand → every line visible, including the head.
    assert!(view.toggle_tool_expanded());
    let expanded = render(&mut view, 60, 30);
    assert!(expanded.contains("ln01"), "all lines visible when expanded: {expanded:?}");
}

#[test]
fn error_tool_uses_cross_marker() {
    let mut view = TranscriptView::new();
    view.push_tool_start("edit", Some("a.txt".to_string()));
    view.push_tool_end("edit", true, Some("permission denied".to_string()));
    let text = render(&mut view, 60, 8);
    assert!(text.contains("✗ edit(a.txt)"), "error marker: {text:?}");
}

#[test]
fn diff_result_renders_as_a_diff() {
    let mut view = TranscriptView::new();
    view.push_tool_start("edit", Some("a.txt".to_string()));
    // A unified-diff-shaped result renders via the diff renderer regardless of expand state.
    view.push_tool_end("edit", false, Some("-1 old text\n+1 new text".to_string()));
    let text = render(&mut view, 70, 10);
    assert!(text.contains("-1 old text"), "removed diff line: {text:?}");
    assert!(text.contains("+1 new text"), "added diff line: {text:?}");
}

#[test]
fn commit_moves_live_tools_to_scrollback() {
    let mut view = TranscriptView::new();
    view.push_tool_start("read", Some("x".to_string()));
    view.push_tool_end("read", false, None);
    assert_eq!(view.active_tools().len(), 1);
    view.commit_tools();
    assert_eq!(view.active_tools().len(), 0, "committed tools leave the live set");
    assert_eq!(view.pending().len(), 1, "committed as a scrollback entry");
}
