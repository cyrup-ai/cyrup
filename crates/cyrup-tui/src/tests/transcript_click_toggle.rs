//! Click-to-toggle on the fullscreen transcript.
//!
//! A left click on a thinking run, a completed tool block, a branch or compaction summary or a skill
//! invocation flips that one entry between its collapsed and expanded form (pi
//! `assistant-message.ts:164`, `tool-execution.ts:175-181`, `branch-summary-message.ts`,
//! `compaction-summary-message.ts`, `skill-invocation-message.ts` @v1.0.0). Every test drives a real
//! fullscreen `App` over a `TestBackend`: entries are committed, a frame is drawn, the entry is found
//! in the drawn cells by its text, and the click goes in through `App::handle_input` as the press and
//! release crossterm would deliver. What is asserted is what a user would see in the next frame.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use serde_json::json;

use crate::theme::UiTheme;
use crate::{App, InputEvent};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

/// The text of every screen row of the frame just drawn.
fn screen(app: &mut App<TestBackend>) -> Vec<String> {
    app.draw().unwrap();
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    (0..ROWS)
        .map(|y| {
            (0..COLS)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn has(rows: &[String], needle: &str) -> bool {
    rows.iter().any(|r| r.contains(needle))
}

/// The cell (column, row) of the first occurrence of `needle`. The transcript is ASCII to the left
/// of every needle used here, so a byte offset is a column.
fn find(rows: &[String], needle: &str) -> (u16, u16) {
    for (y, row) in rows.iter().enumerate() {
        if let Some(x) = row.find(needle) {
            return (u16::try_from(x).unwrap(), u16::try_from(y).unwrap());
        }
    }
    panic!("{needle:?} is not on screen: {rows:#?}");
}

fn mouse(kind: MouseEventKind, (column, row): (u16, u16)) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// A left press and release on one cell — the click crossterm reports.
fn click(app: &mut App<TestBackend>, at: (u16, u16)) {
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), at));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), at));
}

fn ctrl_o(app: &mut App<TestBackend>) {
    app.handle_input(&InputEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
}

fn read_result(text: &str) -> serde_json::Value {
    json!({ "content": [{ "type": "text", "text": text }], "details": null })
}

// ---------------------------------------------------------------------------------------------
// each clickable kind
// ---------------------------------------------------------------------------------------------

/// `compaction-summary-message.ts`: collapsed it is one hint row, and a click opens the body; a
/// second click on the opened block closes it again.
#[test]
fn a_click_toggles_a_compaction_summary() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_compaction_summary(12_345, "condensed history");
    let rows = screen(&mut app);
    assert!(
        has(&rows, "Compacted from 12,345 tokens (ctrl+o to expand)"),
        "{rows:#?}"
    );
    assert!(!has(&rows, "condensed history"), "{rows:#?}");

    click(&mut app, find(&rows, "Compacted from"));
    let rows = screen(&mut app);
    assert!(has(&rows, "condensed history"), "{rows:#?}");
    assert!(!has(&rows, "to expand"), "{rows:#?}");

    click(&mut app, find(&rows, "condensed history"));
    let rows = screen(&mut app);
    assert!(!has(&rows, "condensed history"), "{rows:#?}");
    assert!(has(&rows, "ctrl+o to expand"), "{rows:#?}");
}

#[test]
fn a_click_toggles_a_branch_summary() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    assert!(has(&rows, "Branch summary (ctrl+o to expand)"), "{rows:#?}");

    click(&mut app, find(&rows, "Branch summary"));
    let rows = screen(&mut app);
    assert!(has(&rows, "we merged the spike"), "{rows:#?}");

    click(&mut app, find(&rows, "we merged the spike"));
    let rows = screen(&mut app);
    assert!(!has(&rows, "we merged the spike"), "{rows:#?}");
}

/// `skill-invocation-message.ts` starts collapsed to `[skill] name (key to expand)` and is opened by
/// a click on its block.
#[test]
fn a_skill_invocation_is_collapsed_until_clicked() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_skill_invocation("deploy", "Run the **deploy** steps");
    let rows = screen(&mut app);
    assert!(
        has(&rows, "[skill] deploy (ctrl+o to expand)"),
        "the collapsed form is one row: {rows:#?}"
    );
    assert!(!has(&rows, "Run the deploy steps"), "{rows:#?}");

    click(&mut app, find(&rows, "[skill] deploy"));
    let rows = screen(&mut app);
    assert!(has(&rows, "Run the deploy steps"), "{rows:#?}");
    assert!(!has(&rows, "to expand"), "{rows:#?}");

    click(&mut app, find(&rows, "Run the deploy steps"));
    let rows = screen(&mut app);
    assert!(has(&rows, "[skill] deploy (ctrl+o to expand)"), "{rows:#?}");
}

/// `Ctrl+O` opens a skill block, like the two summaries: pi seeds it from `toolOutputExpanded`.
#[test]
fn ctrl_o_expands_a_skill_invocation() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_skill_invocation("deploy", "Run the **deploy** steps");
    ctrl_o(&mut app);
    let rows = screen(&mut app);
    assert!(has(&rows, "Run the deploy steps"), "{rows:#?}");
}

/// One thinking run: a click swaps the body for the hidden label and back.
#[test]
fn a_click_toggles_a_thinking_run() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");

    click(&mut app, find(&rows, "pondering"));
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "{rows:#?}");
    assert!(!has(&rows, "pondering the design"), "{rows:#?}");

    click(&mut app, find(&rows, "Thinking..."));
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");
}

/// A run committed under `hideThinkingBlock` opens on a click — the override beats the frozen flag.
#[test]
fn a_click_opens_a_thinking_run_that_committed_hidden() {
    let mut app = fullscreen_app();
    app.state_mut().set_hide_thinking(true);
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "{rows:#?}");

    click(&mut app, find(&rows, "Thinking..."));
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");
}

/// A finished tool block: the collapsed `read` shows its header only, a click on the header opens the
/// body. The block carries a result, which is what makes pi's region answer.
#[test]
fn a_click_toggles_a_completed_tool_block() {
    let mut app = fullscreen_app();
    let t = app.transcript_mut();
    t.push_tool_start("read", json!({ "path": "a.rs" }));
    t.push_tool_end("read", false, Some(read_result("line one\nline two")));
    t.commit_tools();
    let rows = screen(&mut app);
    assert!(has(&rows, "read a.rs"), "{rows:#?}");
    assert!(!has(&rows, "line one"), "{rows:#?}");

    click(&mut app, find(&rows, "read a.rs"));
    let rows = screen(&mut app);
    assert!(
        has(&rows, "line one") && has(&rows, "line two"),
        "{rows:#?}"
    );

    click(&mut app, find(&rows, "line two"));
    let rows = screen(&mut app);
    assert!(!has(&rows, "line one"), "{rows:#?}");
}

// ---------------------------------------------------------------------------------------------
// what must not toggle
// ---------------------------------------------------------------------------------------------

/// A drag is a selection, not a click — even one that returns to the cell it started on.
#[test]
fn a_drag_does_not_toggle() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    let (x, y) = find(&rows, "Branch summary");

    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), (x, y)));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), (x + 4, y)));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), (x + 4, y)));
    assert!(!has(&screen(&mut app), "we merged the spike"));

    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), (x, y)));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), (x + 4, y)));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), (x, y)));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), (x, y)));
    let rows = screen(&mut app);
    assert!(!has(&rows, "we merged the spike"), "{rows:#?}");
}

/// The condition is `button === "left"`.
#[test]
fn a_right_click_does_not_toggle() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    let at = find(&rows, "Branch summary");

    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Right), at));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Right), at));
    let rows = screen(&mut app);
    assert!(!has(&rows, "we merged the spike"), "{rows:#?}");
}

/// Modifiers are not read: pi's handler tests the type and the button and nothing else.
#[test]
fn a_modified_click_toggles_too() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    let (column, row) = find(&rows, "Branch summary");

    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_input(&InputEvent::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::SHIFT,
        }));
    }
    let rows = screen(&mut app);
    assert!(has(&rows, "we merged the spike"), "{rows:#?}");
}

/// The region is the box's content: the tinted padding row above the label and the padding column
/// in front of it are outside every `MouseRegion` (`box.ts` hit-tests the content box).
#[test]
fn the_padding_of_a_boxed_block_does_not_toggle() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    let (x, y) = find(&rows, "[branch]");
    assert!(x >= 1, "the label sits inside a padding column: {rows:#?}");

    click(&mut app, (0, y));
    assert!(!has(&screen(&mut app), "we merged the spike"));
    click(&mut app, (x, y - 1));
    assert!(!has(&screen(&mut app), "we merged the spike"));

    click(&mut app, (x, y));
    assert!(has(&screen(&mut app), "we merged the spike"));
}

/// Plain assistant text is not clickable.
#[test]
fn assistant_text_is_not_clickable() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .commit_assistant(Some("plain answer".into()));
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let before = screen(&mut app);
    click(&mut app, find(&before, "plain answer"));
    assert_eq!(screen(&mut app), before);
}

// ---------------------------------------------------------------------------------------------
// the global broadcasts
// ---------------------------------------------------------------------------------------------

/// `setToolsExpanded` overwrites every component's local flag, so a per-entry click does not
/// survive a `Ctrl+O` round trip.
#[test]
fn ctrl_o_resets_block_overrides() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "Branch summary"));
    assert!(has(&screen(&mut app), "we merged the spike"));

    ctrl_o(&mut app); // expanded
    assert!(has(&screen(&mut app), "we merged the spike"));
    ctrl_o(&mut app); // collapsed again: the broadcast overwrote the click
    let rows = screen(&mut app);
    assert!(!has(&rows, "we merged the spike"), "{rows:#?}");
}

/// Only `setHideThinkingBlock` clears a thinking run's override; `Ctrl+O` does not reach an
/// assistant message, and the hide toggle does not reach a tool block.
#[test]
fn the_broadcasts_reset_only_their_own_overrides() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "pondering"));
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "Branch summary"));
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking...") && has(&rows, "we merged the spike"));

    // Ctrl+O twice: the summary falls back to the (collapsed) global, the thinking run is untouched.
    ctrl_o(&mut app);
    ctrl_o(&mut app);
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "{rows:#?}");
    assert!(!has(&rows, "we merged the spike"), "{rows:#?}");

    // Re-open the summary, then flip hideThinkingBlock: the run reopens, the summary stays open.
    click(&mut app, find(&rows, "Branch summary"));
    app.state_mut().set_hide_thinking(false);
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");
    assert!(has(&rows, "we merged the spike"), "{rows:#?}");
}

// ---------------------------------------------------------------------------------------------
// the viewport
// ---------------------------------------------------------------------------------------------

/// An entry that begins above the viewport top and collapses past the pointer must not leave the
/// viewport over whatever now sits at the old offset: the clicked row stays under the pointer, on
/// the entry that was toggled.
#[test]
fn collapsing_an_entry_that_starts_above_the_viewport_keeps_it_under_the_pointer() {
    let mut app = fullscreen_app();
    app.set_tools_expanded(true);
    let body: String = (1..=40)
        .map(|i| format!("body line {i:02}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    app.transcript_mut().push_compaction_summary(40, body);
    for i in 0..40 {
        app.transcript_mut().push_status(format!("filler {i:02}"));
    }
    let rows = screen(&mut app);
    assert!(has(&rows, "filler 39"), "following the tail: {rows:#?}");

    // The compaction block starts two rows in (behind the status line `Ctrl+O` pushed); park the
    // viewport thirty rows into it.
    app.altscreen_for_test().unwrap().scroll_to_row_for_test(32);
    let rows = screen(&mut app);
    assert!(
        !has(&rows, "Compacted from"),
        "the block's header is above the viewport: {rows:#?}"
    );
    let (x, y) = find(&rows, "body line 14");

    click(&mut app, (x, y));
    let rows = screen(&mut app);
    assert!(
        rows.get(usize::from(y))
            .is_some_and(|r| r.contains("Compacted from 40 tokens (ctrl+o to expand)")),
        "the collapsed block is still under the pointer on row {y}: {rows:#?}"
    );
}

// ---------------------------------------------------------------------------------------------
// the front trim
// ---------------------------------------------------------------------------------------------

/// Overrides follow their entry across the retained document's front trim: the index of an entry
/// moves, the entry does not.
#[test]
fn a_front_trim_keeps_an_override_on_its_own_entry() {
    let mut app = fullscreen_app();
    for i in 0..2_499 {
        app.transcript_mut().push_status(format!("early {i}"));
    }
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    for i in 0..2_500 {
        app.transcript_mut().push_status(format!("late {i}"));
    }
    let rows = screen(&mut app);
    // Park the viewport on the summary: it is the 2_500th entry, behind 2_499 two-row statuses.
    app.altscreen_for_test()
        .unwrap()
        .scroll_to_row_for_test(2_499 * 2);
    let rows_at_summary = screen(&mut app);
    assert!(has(&rows_at_summary, "Branch summary"), "{rows:#?}");
    click(&mut app, find(&rows_at_summary, "Branch summary"));
    assert!(has(&screen(&mut app), "we merged the spike"));

    // Ten more entries evict the ten oldest: every index moves down ten, the summary included.
    for i in 0..10 {
        app.transcript_mut().push_status(format!("newer {i}"));
    }
    let rows = screen(&mut app);
    assert_eq!(app.state().transcript.retained_dropped(), 10);
    assert!(
        has(&rows, "we merged the spike"),
        "the override stayed on the summary after the trim: {rows:#?}"
    );
}

/// The override of an evicted entry goes with it.
#[test]
fn an_evicted_entry_drops_its_override() {
    let mut t = crate::TranscriptView::new();
    t.set_retain_document(true);
    t.push_branch_summary("first");
    for i in 0..4_999 {
        t.push_status(format!("s{i}"));
    }
    t.drain_committed();
    assert_eq!(t.document().len(), 5_000);
    assert!(t.toggle_entry(0).is_some());
    assert_eq!(t.entry_expansion(0), Some(crate::Expansion::Open));
    assert_eq!(t.tracked_expansions(), 1);

    t.push_status("pushes the summary out");
    t.drain_committed();
    assert_eq!(t.retained_dropped(), 1);
    assert_eq!(
        t.tracked_expansions(),
        0,
        "the evicted entry's override is gone"
    );
    assert_eq!(t.entry_expansion(0), None);
}

// ---------------------------------------------------------------------------------------------
// the in-flight turn
// ---------------------------------------------------------------------------------------------

/// Pi wraps a thinking run in its `MouseRegion` from the first delta, so a run that is still
/// streaming toggles — and the committed entry renders as the live block did, with no flash back to
/// the default at the commit.
#[test]
fn a_streaming_thinking_run_toggles_and_keeps_its_state_through_the_commit() {
    let mut app = fullscreen_app();
    app.transcript_mut().push_thinking_delta("pondering live");
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering live"), "{rows:#?}");

    click(&mut app, find(&rows, "pondering"));
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "{rows:#?}");
    assert!(!has(&rows, "pondering live"), "{rows:#?}");

    click(&mut app, find(&rows, "Thinking..."));
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering live"), "{rows:#?}");
    click(&mut app, find(&rows, "pondering"));
    assert!(has(&screen(&mut app), "Thinking..."));

    app.transcript_mut().commit_thinking(None);
    let rows = screen(&mut app);
    assert!(
        has(&rows, "Thinking...") && !has(&rows, "pondering live"),
        "the committed run stays closed: {rows:#?}"
    );

    // …and it is the committed entry's override now: a click opens it.
    click(&mut app, find(&rows, "Thinking..."));
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering live"), "{rows:#?}");
}

/// A tool block answers a click as soon as it has a result — still live, before the turn commits —
/// and not before (`if (!this.result …) return undefined`).
#[test]
fn a_live_tool_toggles_once_it_has_a_result_and_commits_as_it_was() {
    use crate::transcript::{LiveBlock, ToggleTarget};

    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_tool_start("read", json!({ "path": "a.rs" }));
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "read a.rs"));
    assert!(
        app.transcript_mut()
            .toggle(ToggleTarget::Live(LiveBlock::Tool(0)))
            .is_none(),
        "a running tool has nothing to toggle"
    );

    app.transcript_mut()
        .push_tool_end("read", false, Some(read_result("line one\nline two")));
    assert!(
        app.state().transcript.active_tools().len() == 1,
        "the finished tool has not committed"
    );
    let rows = screen(&mut app);
    assert!(!has(&rows, "line one"), "{rows:#?}");
    click(&mut app, find(&rows, "read a.rs"));
    let rows = screen(&mut app);
    assert!(
        has(&rows, "line one") && has(&rows, "line two"),
        "{rows:#?}"
    );

    app.transcript_mut().commit_tools();
    let rows = screen(&mut app);
    assert!(
        has(&rows, "line one"),
        "the committed tool keeps the live click: {rows:#?}"
    );
    // A different cell than the clicks before it: a third click on one word within the double-click
    // interval is a line selection, which is not a click on a toggle region (pi's `isClick` too).
    click(&mut app, find(&rows, "a.rs"));
    assert!(!has(&screen(&mut app), "line one"));
}

/// thinking, text, tools, then the next turn's thinking, committed with no frame in between: each
/// override lands on the entry its block became.
#[test]
fn live_overrides_survive_interleaved_commits() {
    let mut app = fullscreen_app();
    app.transcript_mut().push_thinking_delta("first thought");
    app.transcript_mut().push_assistant_delta("the answer");
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "first thought"));

    app.transcript_mut()
        .push_tool_start("read", json!({ "path": "a.rs" }));
    app.transcript_mut()
        .push_tool_end("read", false, Some(read_result("line one")));
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "read a.rs"));
    assert!(has(&screen(&mut app), "line one"));

    // The turn ends: all three commit before any frame is drawn.
    app.transcript_mut().commit_thinking(None);
    app.transcript_mut().commit_assistant(None);
    app.transcript_mut().commit_tools();
    // The next turn starts thinking.
    app.transcript_mut().push_thinking_delta("second thought");

    let rows = screen(&mut app);
    assert!(
        has(&rows, "Thinking..."),
        "first run stays closed: {rows:#?}"
    );
    assert!(!has(&rows, "first thought"), "{rows:#?}");
    assert!(has(&rows, "the answer"), "{rows:#?}");
    assert!(has(&rows, "line one"), "the tool stays open: {rows:#?}");
    assert!(
        has(&rows, "second thought"),
        "the new run is default: {rows:#?}"
    );
}

/// The broadcasts reach the live blocks too: `Ctrl+O` overwrites a live tool's click, and
/// `hideThinkingBlock` a streaming run's.
#[test]
fn the_broadcasts_reset_live_overrides() {
    let mut app = fullscreen_app();
    app.transcript_mut().push_thinking_delta("pondering live");
    app.transcript_mut()
        .push_tool_start("read", json!({ "path": "a.rs" }));
    app.transcript_mut()
        .push_tool_end("read", false, Some(read_result("line one")));
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "pondering"));
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "read a.rs"));
    let rows = screen(&mut app);
    assert!(
        has(&rows, "Thinking...") && has(&rows, "line one"),
        "{rows:#?}"
    );

    ctrl_o(&mut app);
    ctrl_o(&mut app);
    let rows = screen(&mut app);
    assert!(
        !has(&rows, "line one"),
        "the tool is back to the global: {rows:#?}"
    );
    assert!(
        has(&rows, "Thinking..."),
        "Ctrl+O does not reach the run: {rows:#?}"
    );

    app.state_mut().set_hide_thinking(false);
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering live"), "{rows:#?}");
}

/// A tall live tool that begins above the viewport and collapses past the pointer keeps the
/// pointer over itself, the same anchor a committed entry gets. The tools after it are what make
/// the difference visible: a viewport left at the old offset lands among them.
#[test]
fn collapsing_a_live_tool_keeps_it_under_the_pointer() {
    let mut app = fullscreen_app();
    app.set_tools_expanded(true);
    let body = (1..=40)
        .map(|i| format!("ln{i:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    let t = app.transcript_mut();
    t.push_tool_start("bash", json!({ "command": "seq" }));
    t.push_tool_end("bash", false, Some(read_result(&body)));
    for i in 0..30 {
        let name = format!("f{i}.rs");
        t.push_tool_start("read", json!({ "path": name }));
        t.push_tool_end("read", false, Some(read_result("tail")));
    }
    let rows = screen(&mut app);
    assert!(has(&rows, "f29.rs"), "following the tail: {rows:#?}");

    // The status line `Ctrl+O` pushed is the only committed entry (two rows); the bash block
    // starts right behind it.
    app.altscreen_for_test()
        .unwrap()
        .scroll_to_row_for_test(2 + 25);
    let rows = screen(&mut app);
    assert!(
        !has(&rows, "$ seq"),
        "the block's header is above the viewport"
    );
    // Mid-screen, so the collapsed block has room above the pointer.
    let y = 8;
    assert!(rows[usize::from(y)].contains("ln"), "{rows:#?}");

    click(&mut app, (1, y));
    let rows = screen(&mut app);
    assert!(
        has(&rows, "earlier lines"),
        "the collapsed bash block is on screen: {rows:#?}"
    );
}

// ---------------------------------------------------------------------------------------------
// extension-rendered snapshots
// ---------------------------------------------------------------------------------------------

/// An extension renderer's text is a snapshot taken under `options.expanded`. A per-entry toggle
/// changes that input for one row, and the retained document — which the alternate screen repaints
/// from — is where such a row lives once committed.
#[test]
fn toggling_a_retained_tool_makes_its_extension_render_stale() {
    use crate::transcript::{RenderSource, RenderSurface, RenderedText};

    let live = cyrup_ext::RenderOptions::new(false, 1, None);
    let source = RenderSource {
        surface: RenderSurface::ToolResult,
        key: "ext".into(),
        payload: json!({}),
        under: live.clone().partial(false),
    };
    let mut t = crate::TranscriptView::new();
    t.set_retain_document(true);
    t.push_tool_start_rendered("ext", Some("c1".into()), json!({}), None);
    t.push_tool_end_rendered(
        "ext",
        Some("c1"),
        false,
        Some(read_result("out")),
        Some(RenderedText::new("snapshot", source.clone())),
    );
    t.commit_tools();
    t.drain_committed();
    assert!(t.stale_extension_renders(&live).is_empty());

    assert!(t.toggle_entry(0).is_some());
    let stale = t.stale_extension_renders(&live);
    assert_eq!(stale.len(), 1, "the toggled row asks for a fresh render");
    assert!(stale[0].next.under.expanded, "…under its own expansion");

    t.set_extension_render(
        stale[0].slot,
        RenderedText::new("fresh", stale[0].next.clone()),
    );
    assert!(t.stale_extension_renders(&live).is_empty());

    // The global flag reaches the retained row as well: this is `Ctrl+O` round the document, and
    // it overwrites the row's own override.
    assert!(t.set_tool_expanded(true));
    let live_open = cyrup_ext::RenderOptions::new(true, 1, None);
    assert!(
        t.stale_extension_renders(&live_open).is_empty(),
        "the row already shows the expanded render"
    );
    assert!(t.set_tool_expanded(false));
    assert_eq!(t.stale_extension_renders(&live).len(), 1);
}
