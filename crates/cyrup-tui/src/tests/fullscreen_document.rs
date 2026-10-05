//! What the fullscreen document is made of, and what happens to it between frames.
//!
//! Four things the retained document owes pi's `documentContainer` (`interactive-mode.ts:622-628`
//! @v1.0.0):
//!
//! * **The header scrolls.** `documentContainer = [headerContainer, loadedResourcesContainer,
//!   chatContainer]`, so the extension header — or the built-in startup block — is the first rows
//!   of the scrolled document and leaves the screen with the conversation instead of being pinned
//!   above it.
//! * **Links are links.** The OSC-8 escapes the inline renderer injects into its cells reach the
//!   alternate screen's cells too, for committed rows and for the in-flight turn alike.
//! * **A hidden-thinking toggle reaches committed runs**, which the repainted document can honour
//!   and native scrollback cannot.
//! * **A toggle is visible to the next click**, not only to the next frame.
//!
//! Every test drives a real fullscreen `App` over a `TestBackend` and reads the cells of the frame
//! it painted.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use serde_json::json;

use crate::altscreen::ViewportRenderer;
use crate::theme::UiTheme;
use crate::transcript::{ImageOpts, ThinkingHiding, entry_lines};
use crate::{App, Entry, InputEvent};

const COLS: u16 = 80;
const ROWS: u16 = 24;

/// The session directory every link test resolves its paths against. Absolute and outside `$HOME`,
/// so `shorten_path` is the identity.
const CWD: &str = "/tmp/aug-osc";

fn app_with(hints: bool, rows: u16) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, rows), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = hints;
    app.state_mut().startup_header = if hints {
        crate::StartupHeader::Shown
    } else {
        crate::StartupHeader::Hidden
    };
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

/// A fullscreen app with no startup block, so the extension header is the only header.
fn fullscreen_app() -> App<TestBackend> {
    app_with(false, ROWS)
}

fn set_header(app: &mut App<TestBackend>, header: Option<&str>) {
    app.state_mut().extension_header = header.map(str::to_string);
}

/// The text of every screen row of the frame just drawn, less the rightmost column — the
/// scrollbar's, whose thumb depends on the document's height and not on what is in it.
fn screen(app: &mut App<TestBackend>) -> Vec<String> {
    app.draw().unwrap();
    let rows = app.altscreen_for_test().expect("fullscreen is live");
    let buf = rows.backend_for_test().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..COLS - 1)
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

fn row_of(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|r| r.contains(needle))
}

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

/// A left press and release on one cell — the click crossterm reports. No frame is drawn.
fn click(app: &mut App<TestBackend>, at: (u16, u16)) {
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), at));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), at));
}

fn ctrl_up(app: &mut App<TestBackend>) {
    app.handle_input(&InputEvent::Key(KeyEvent::new(
        KeyCode::Up,
        KeyModifiers::CONTROL,
    )));
}

fn read_result(text: &str) -> serde_json::Value {
    json!({ "content": [{ "type": "text", "text": text }], "details": null })
}

fn fillers(app: &mut App<TestBackend>, n: usize) {
    for i in 0..n {
        app.transcript_mut().push_status(format!("filler {i:02}"));
    }
}

fn scroll_to_top(app: &mut App<TestBackend>) {
    app.altscreen_for_test().unwrap().scroll_to_top();
}

fn tool(app: &mut App<TestBackend>, path: &str, body: &str) {
    let t = app.transcript_mut();
    t.push_tool_start("read", json!({ "path": path }));
    t.push_tool_end("read", false, Some(read_result(body)));
    t.commit_tools();
}

// ---------------------------------------------------------------------------------------------
// A — the header is the first rows of the document
// ---------------------------------------------------------------------------------------------

/// The extension header opens the document: it is the first thing on screen, directly above the
/// conversation, and the message region it used to be docked above starts at the top of the screen.
#[test]
fn the_extension_header_is_the_first_rows_of_the_document() {
    let mut app = fullscreen_app();
    set_header(&mut app, Some("HEADER ONE\nHEADER TWO"));
    fillers(&mut app, 3);
    let rows = screen(&mut app);
    assert_eq!(rows[0], "HEADER ONE", "{rows:#?}");
    assert_eq!(rows[1], "HEADER TWO", "{rows:#?}");
    assert!(
        row_of(&rows, "filler 00").unwrap() > 1,
        "the conversation follows the header: {rows:#?}"
    );
}

/// In fullscreen the header takes no band of its own: the message region starts at the top of the
/// screen and has the same height with a header as without one. Inline keeps its band.
#[test]
fn the_header_region_has_no_rows_in_fullscreen_and_keeps_them_inline() {
    let mut app = fullscreen_app();
    app.draw().unwrap();
    let bare = app.state_mut().regions;

    set_header(&mut app, Some("HEADER ONE\nHEADER TWO\nHEADER THREE"));
    app.draw().unwrap();
    let with = app.state_mut().regions;
    assert_eq!(with.header.height, 0, "no band: {with:?}");
    assert_eq!(with.msg.y, 0, "the document starts at the top: {with:?}");
    assert_eq!(
        with.msg.height, bare.msg.height,
        "the document keeps its height"
    );
    assert_eq!(with.slot, bare.slot, "and the dock does not move");

    let mut inline = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    inline.state_mut().show_startup_hints = false;
    set_header(&mut inline, Some("HEADER ONE\nHEADER TWO\nHEADER THREE"));
    inline.draw().unwrap();
    assert_eq!(
        inline.state_mut().regions.header.height,
        3,
        "inline still docks the header above the message region"
    );
}

/// The header leaves the screen with the conversation, and comes back with it.
#[test]
fn the_header_scrolls_away_with_the_conversation() {
    let mut app = fullscreen_app();
    set_header(&mut app, Some("HEADER ONE\nHEADER TWO"));
    fillers(&mut app, 40);
    let rows = screen(&mut app);
    assert!(has(&rows, "filler 39"), "following the tail: {rows:#?}");
    assert!(
        !has(&rows, "HEADER"),
        "the header is part of the document, so it scrolled off the top: {rows:#?}"
    );

    scroll_to_top(&mut app);
    let rows = screen(&mut app);
    assert_eq!(rows[0], "HEADER ONE", "{rows:#?}");
}

/// A header that is removed, or changed, rebuilds the document — it is part of the cache key.
#[test]
fn changing_the_header_rebuilds_the_document() {
    let mut app = fullscreen_app();
    fillers(&mut app, 2);
    set_header(&mut app, Some("HEADER ONE"));
    assert_eq!(screen(&mut app)[0], "HEADER ONE");

    set_header(&mut app, Some("HEADER ONE\nHEADER TWO"));
    let rows = screen(&mut app);
    assert_eq!(rows[1], "HEADER TWO", "{rows:#?}");

    set_header(&mut app, None);
    let rows = screen(&mut app);
    assert!(!has(&rows, "HEADER"), "{rows:#?}");
}

/// A header row wider than the viewport is reflowed into more rows, not truncated.
#[test]
fn a_wide_header_row_wraps_instead_of_truncating() {
    let mut app = fullscreen_app();
    let wide = format!("{} {}", "alpha ".repeat(14), "OMEGA");
    set_header(&mut app, Some(&wide));
    let rows = screen(&mut app);
    assert!(
        has(&rows, "OMEGA"),
        "the tail of the line survived: {rows:#?}"
    );
}

/// The front trim removes entries, never the header: a reader parked in history stays on the same
/// content when ten entries are evicted from the top of a document that has a header.
#[test]
fn a_front_trim_does_not_count_the_header_as_trimmed_rows() {
    let mut app = fullscreen_app();
    set_header(&mut app, Some("HEADER ONE\nHEADER TWO\nHEADER THREE"));
    fillers(&mut app, 5_000);
    let _ = screen(&mut app);
    // Park well inside the history, nowhere near either end.
    app.altscreen_for_test()
        .unwrap()
        .scroll_to_row_for_test(3 + 2 * 2_000);
    let before = screen(&mut app);
    assert_eq!(app.state().transcript.retained_dropped(), 0);

    for i in 0..10 {
        app.transcript_mut().push_status(format!("newer {i}"));
    }
    let after = screen(&mut app);
    assert_eq!(app.state().transcript.retained_dropped(), 10);
    assert_eq!(
        before, after,
        "ten evicted entries (twenty rows) slid the document up by exactly the rows the reader \
         was shifted by — the header's three rows are not among them"
    );
}

/// The prompt walk lands on the right rows when the header sits in front of entry 0: the same
/// presses give the same screen as in a document with no header.
#[test]
fn prompt_navigation_accounts_for_the_header() {
    let build = |header: Option<&str>| {
        let mut app = fullscreen_app();
        set_header(&mut app, header);
        app.transcript_mut().push_user("first prompt");
        fillers(&mut app, 30);
        app.transcript_mut().push_user("second prompt");
        fillers(&mut app, 30);
        let _ = screen(&mut app);
        app
    };
    let mut plain = build(None);
    let mut headed = build(Some("HEADER ONE\nHEADER TWO\nHEADER THREE"));

    for step in 0..2 {
        ctrl_up(&mut plain);
        ctrl_up(&mut headed);
        let want = screen(&mut plain);
        let got = screen(&mut headed);
        assert_eq!(
            got, want,
            "press {step}: the jump lands on the prompt, not three rows beside it"
        );
    }
    let rows = screen(&mut headed);
    assert!(has(&rows, "first prompt"), "{rows:#?}");
    assert_eq!(
        row_of(&rows, "first prompt"),
        row_of(&screen(&mut plain), "first prompt")
    );
}

/// A click toggles the entry under it, at the row the entry is drawn on, whatever sits above entry
/// 0; a click on the header itself toggles nothing.
#[test]
fn toggle_hit_testing_accounts_for_the_header() {
    let mut app = fullscreen_app();
    set_header(&mut app, Some("HEADER ONE\nHEADER TWO\nHEADER THREE"));
    tool(&mut app, "a.rs", "line one\nline two");
    let rows = screen(&mut app);
    assert!(
        has(&rows, "read a.rs") && !has(&rows, "line one"),
        "{rows:#?}"
    );

    for y in 0..3 {
        click(&mut app, (2, y));
    }
    let rows = screen(&mut app);
    assert!(
        !has(&rows, "line one"),
        "the header answers no click: {rows:#?}"
    );

    click(&mut app, find(&rows, "read a.rs"));
    let rows = screen(&mut app);
    assert!(
        has(&rows, "line one"),
        "the block under the pointer opened: {rows:#?}"
    );
}

/// A selection addresses document rows, so the header's rows count: dragging across a word in an
/// entry copies that word, not the text three rows away.
#[test]
fn selection_accounts_for_the_header() {
    let mut app = fullscreen_app();
    set_header(&mut app, Some("HEADER ONE\nHEADER TWO\nHEADER THREE"));
    app.transcript_mut().push_status("alpha bravo charlie");
    app.transcript_mut().push_status("delta echo foxtrot");
    let rows = screen(&mut app);
    let (x, y) = find(&rows, "bravo");

    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), (x, y)));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), (x + 4, y)));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), (x + 4, y)));
    let selected = app.altscreen_for_test().unwrap().selection_text();
    assert_eq!(selected.as_deref(), Some("bravo"));

    // …and the header itself is selectable text, like any other row of the document.
    let (hx, hy) = find(&rows, "HEADER TWO");
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), (hx, hy)));
    app.handle_input(&mouse(
        MouseEventKind::Drag(MouseButton::Left),
        (hx + 5, hy),
    ));
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), (hx + 5, hy)));
    assert_eq!(
        app.altscreen_for_test()
            .unwrap()
            .selection_text()
            .as_deref(),
        Some("HEADER")
    );
}

/// The built-in startup block is the same kind of header: top-aligned in the document, still there
/// when the conversation starts, and scrolled away with it — not painted into the bottom of the
/// message region and then dropped.
#[test]
fn the_startup_block_is_the_first_rows_of_the_document() {
    let mut app = app_with(true, ROWS);
    let rows = screen(&mut app);
    let bar = row_of(&rows, "interrupt").unwrap_or_else(|| panic!("the hint bar: {rows:#?}"));
    assert!(bar <= 2, "top-aligned, under its framing blank: {rows:#?}");

    fillers(&mut app, 3);
    let rows = screen(&mut app);
    assert_eq!(row_of(&rows, "interrupt"), Some(bar), "{rows:#?}");
    assert!(
        row_of(&rows, "filler 00").unwrap() > bar,
        "the conversation sits under the block: {rows:#?}"
    );

    fillers(&mut app, 40);
    let rows = screen(&mut app);
    assert!(!has(&rows, "interrupt"), "scrolled away: {rows:#?}");
    scroll_to_top(&mut app);
    assert!(has(&screen(&mut app), "interrupt"));

    // A first submission dismisses the inline band, not this: pi's header is the document's
    // first child for the whole session, and only scrolls away.
    app.state_mut().show_startup_hints = false;
    app.altscreen_for_test().unwrap().scroll_to_bottom();
    assert!(
        !has(&screen(&mut app), "interrupt"),
        "still scrolled away, not re-pinned"
    );
    scroll_to_top(&mut app);
    assert!(has(&screen(&mut app), "interrupt"), "and still there");
}

/// An extension header replaces the built-in block (`setExtensionHeader` swaps `builtInHeader`
/// out), it does not stack on it.
#[test]
fn an_extension_header_replaces_the_startup_block() {
    let mut app = app_with(true, ROWS);
    set_header(&mut app, Some("CUSTOM HEADER"));
    let rows = screen(&mut app);
    assert!(!has(&rows, "interrupt"), "{rows:#?}");
    // `Spacer(1)`, the custom header in the `builtInHeader` child's place, `Spacer(1)`
    // (`interactive-mode.ts:1061-1065`, `:2515-2531`): the spacers are the container's, not the
    // built-in header's, so they stay.
    assert_eq!(rows[0], "", "{rows:#?}");
    assert_eq!(rows[1], "CUSTOM HEADER", "{rows:#?}");
    assert_eq!(rows[2], "", "{rows:#?}");
}

/// With the built-in header off (`quietStartup`), `headerContainer` holds an empty `Text` and
/// nothing else (`:1067-1068`): a custom header replaces that, and there are no spacers.
#[test]
fn a_custom_header_has_no_spacers_when_the_startup_header_is_off() {
    let mut app = app_with(false, ROWS);
    set_header(&mut app, Some("CUSTOM HEADER"));
    let rows = screen(&mut app);
    assert_eq!(rows[0], "CUSTOM HEADER", "{rows:#?}");
}

/// `shouldShowStartupHeader` is `verbose || quietStartup !== true`; `shouldShowStartupDetails` is
/// `verbose || quietStartup === false`. `"header"` keeps the first and drops the second.
#[test]
fn the_startup_header_decision_is_pis() {
    use crate::StartupHeader;
    use cyrup_config::settings::QuietStartup;
    let decide = StartupHeader::decide;
    assert_eq!(decide(false, QuietStartup::Off), StartupHeader::Shown);
    assert_eq!(decide(false, QuietStartup::On), StartupHeader::Hidden);
    assert_eq!(
        decide(false, QuietStartup::Header),
        StartupHeader::HeaderOnly
    );
    for quiet in [QuietStartup::Off, QuietStartup::On, QuietStartup::Header] {
        assert_eq!(
            decide(true, quiet),
            StartupHeader::Shown,
            "--verbose overrides {quiet}"
        );
    }
    assert!(StartupHeader::Pending.is_shown() && !StartupHeader::Pending.is_decided());
    assert!(StartupHeader::HeaderOnly.is_shown() && StartupHeader::HeaderOnly.is_decided());
    assert!(!StartupHeader::Hidden.is_shown());
}

// ---------------------------------------------------------------------------------------------
// B — OSC-8 hyperlinks in the fullscreen cells
// ---------------------------------------------------------------------------------------------

fn link_app() -> App<TestBackend> {
    let mut app = fullscreen_app();
    app.transcript_mut().set_cwd(Some(PathBuf::from(CWD)));
    app.transcript_mut().set_hyperlinks(true);
    app
}

fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}

const CLOSE: &str = "\u{1b}]8;;\u{7}";

/// Every cell of the last frame as `(x, y, symbol)`.
fn cells(app: &mut App<TestBackend>) -> Vec<(u16, u16, String)> {
    app.draw().unwrap();
    let alt = app.altscreen_for_test().unwrap();
    let buf = alt.backend_for_test().buffer().clone();
    let mut out = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push((x, y, buf[(x, y)].symbol().to_string()));
        }
    }
    out
}

/// The row of the cell whose symbol opens the link to `url`, if any.
fn link_row(cells: &[(u16, u16, String)], url: &str) -> Option<u16> {
    let open = open(url);
    cells
        .iter()
        .find(|(_, _, s)| s.starts_with(&open))
        .map(|(_, y, _)| *y)
}

/// The inline renderer injects an OSC-8 pair around a tool header's path (`osc::inject`, run on the
/// cells `insert_before` paints); the committed document the alternate screen paints must carry the
/// same pair, around the same text.
#[test]
fn a_committed_tool_path_is_a_hyperlink_in_the_fullscreen_cells() {
    let mut app = link_app();
    tool(&mut app, "a.rs", "body");
    let cells = cells(&mut app);
    let url = "file:///tmp/aug-osc/a.rs";

    let head = cells
        .iter()
        .find(|(_, _, s)| s.starts_with(&open(url)))
        .unwrap_or_else(|| panic!("no cell opens {url}"));
    assert!(
        head.2.ends_with('a'),
        "the link opens on the path's first cell: {head:?}"
    );
    let (_, y, _) = head;
    let tail = cells
        .iter()
        .find(|(_, row, s)| row == y && s.ends_with(CLOSE))
        .unwrap_or_else(|| panic!("no cell closes the link on row {y}"));
    assert!(tail.2.contains('s'), "and closes on its last: {tail:?}");
}

/// All four tool headers that link their path (`linkPath`: `read`, `write`, `edit`, `ls`) are
/// links in the fullscreen cells — the full set the inline renderer injects. (Markdown links and
/// bare URLs are not among them: neither renderer marks those, see `markdown::walk`.)
#[test]
fn every_linked_tool_header_is_a_hyperlink_in_the_fullscreen_cells() {
    let mut app = link_app();
    let calls = [
        ("read", json!({ "path": "r.rs" })),
        ("write", json!({ "path": "w.rs", "content": "x" })),
        (
            "edit",
            json!({ "path": "e.rs", "edits": [{ "oldText": "a", "newText": "b" }] }),
        ),
        ("ls", json!({ "path": "dir" })),
    ];
    for (name, args) in calls {
        let t = app.transcript_mut();
        t.push_tool_start(name, args);
        t.push_tool_end(name, false, Some(read_result("ok")));
        t.commit_tools();
    }
    let cells = cells(&mut app);
    for file in ["r.rs", "w.rs", "e.rs", "dir"] {
        let url = format!("file:///tmp/aug-osc/{file}");
        let y = link_row(&cells, &url).unwrap_or_else(|| panic!("{url} is not a link"));
        let line: String = cells
            .iter()
            .filter(|(_, row, _)| *row == y)
            .map(|(_, _, s)| crate::ansi::strip_ansi(s))
            .collect();
        assert!(
            line.contains(file),
            "the link is on its own header: {line:?}"
        );
    }
}

/// Hyperlinks are opt-in on the terminal's capability; without it no escape reaches a cell.
#[test]
fn no_hyperlink_is_written_without_the_capability() {
    let mut app = link_app();
    app.transcript_mut().set_hyperlinks(false);
    tool(&mut app, "a.rs", "body");
    assert!(
        cells(&mut app)
            .iter()
            .all(|(_, _, s)| !s.contains("\u{1b}"))
    );
}

/// The capability is part of what the cached document was built from: it can change after rows
/// were built (the terminal's answer arrives late), and the rows must follow.
#[test]
fn a_hyperlink_capability_that_changes_rebuilds_the_document() {
    let mut app = link_app();
    app.transcript_mut().set_hyperlinks(false);
    tool(&mut app, "a.rs", "body");
    let url = "file:///tmp/aug-osc/a.rs";
    assert!(link_row(&cells(&mut app), url).is_none());

    app.transcript_mut().set_hyperlinks(true);
    assert!(link_row(&cells(&mut app), url).is_some());
}

/// The in-flight turn's tool header is a link too, from the transcript's live cache.
#[test]
fn a_live_tool_path_is_a_hyperlink_in_the_fullscreen_cells() {
    let mut app = link_app();
    let t = app.transcript_mut();
    t.push_tool_start("read", json!({ "path": "live.rs" }));
    t.push_tool_end("read", false, Some(read_result("body")));
    let cells = cells(&mut app);
    assert!(
        link_row(&cells, "file:///tmp/aug-osc/live.rs").is_some(),
        "the live header carries its link"
    );
}

/// The link table is cached with the rows: a link keeps its row as the document scrolls, and the
/// 128th and later linked headers of a long session are links too — the table is per entry, not
/// one for the whole document, whose seven-bit id space would have run out.
#[test]
fn links_follow_their_rows_through_scrolling_and_a_long_session() {
    let mut app = link_app();
    for i in 0..140 {
        tool(&mut app, &format!("f{i}.rs"), "body");
    }
    let last = "file:///tmp/aug-osc/f139.rs";
    let first = "file:///tmp/aug-osc/f0.rs";
    let at_tail = cells(&mut app);
    let y = link_row(&at_tail, last).unwrap_or_else(|| panic!("the newest header is a link"));
    let text = screen(&mut app);
    assert!(
        text[usize::from(y)].contains("f139.rs"),
        "on its own row: {text:#?}"
    );

    scroll_to_top(&mut app);
    let at_top = cells(&mut app);
    let y = link_row(&at_top, first).unwrap_or_else(|| panic!("the oldest header is a link"));
    let text = screen(&mut app);
    assert!(
        text[usize::from(y)].contains("f0.rs"),
        "on its own row: {text:#?}"
    );

    app.altscreen_for_test().unwrap().scroll_by(3);
    let moved = cells(&mut app);
    let y = link_row(&moved, "file:///tmp/aug-osc/f2.rs")
        .unwrap_or_else(|| panic!("a header scrolled into view is a link"));
    let text = screen(&mut app);
    assert!(
        text[usize::from(y)].contains("f2.rs"),
        "on its own row: {text:#?}"
    );
}

/// The marker bits that carry a link id from a span to its cell are gone by the time the frame is
/// painted, whichever table they belong to.
#[test]
fn no_link_marker_survives_in_the_painted_cells() {
    let mut app = link_app();
    tool(&mut app, "a.rs", "body");
    let t = app.transcript_mut();
    t.push_tool_start("read", json!({ "path": "live.rs" }));
    t.push_tool_end("read", false, Some(read_result("body")));
    app.draw().unwrap();
    let alt = app.altscreen_for_test().unwrap();
    let buf = alt.backend_for_test().buffer().clone();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let bits = buf[(x, y)].modifier.bits();
            assert_eq!(bits & 0xFE00, 0, "cell ({x}, {y}) still carries a marker");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// C — hiding thinking reaches committed runs
// ---------------------------------------------------------------------------------------------

/// `setHideThinkingBlock` re-renders the assistant messages already in the chat container
/// (`assistant-message.ts:57-62`): in the repainted document a run committed visible turns into the
/// label when the setting is switched on, and back.
#[test]
fn hiding_thinking_reaches_runs_that_committed_earlier() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    assert!(has(&screen(&mut app), "pondering the design"));

    app.state_mut().set_hide_thinking(true);
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "{rows:#?}");
    assert!(!has(&rows, "pondering the design"), "{rows:#?}");

    app.state_mut().set_hide_thinking(false);
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");
}

/// …and a run committed hidden is shown when the setting is switched off.
#[test]
fn showing_thinking_reaches_runs_that_committed_hidden() {
    let mut app = fullscreen_app();
    app.state_mut().set_hide_thinking(true);
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    assert!(has(&screen(&mut app), "Thinking..."));

    app.state_mut().set_hide_thinking(false);
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");
}

/// A click flips a run relative to what it shows now: after the setting hid a run that committed
/// visible, the first click opens it.
#[test]
fn a_click_flips_a_run_from_the_setting_in_force() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    let _ = screen(&mut app);
    app.state_mut().set_hide_thinking(true);
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "{rows:#?}");

    click(&mut app, find(&rows, "Thinking..."));
    let rows = screen(&mut app);
    assert!(has(&rows, "pondering the design"), "{rows:#?}");
}

/// The click override keeps its documented life: it beats the setting until the next flip, which
/// clears it.
#[test]
fn a_click_override_lasts_until_the_next_flip_of_the_setting() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .commit_thinking(Some("pondering the design".into()));
    let rows = screen(&mut app);
    click(&mut app, find(&rows, "pondering"));
    let rows = screen(&mut app);
    assert!(has(&rows, "Thinking..."), "closed by the click: {rows:#?}");

    app.state_mut().set_hide_thinking(false);
    let rows = screen(&mut app);
    assert!(
        has(&rows, "pondering the design"),
        "the flip cleared the override and the setting (visible) rules: {rows:#?}"
    );
}

/// The renderer split: the inline flush draws a committed run as it committed, the fullscreen
/// document draws it under the live flag. Native scrollback cannot be repainted (ADR-0001).
#[test]
fn an_inline_flush_keeps_the_choice_a_run_committed_with() {
    let theme = UiTheme::dark();
    let entry = Entry::Thinking {
        text: "pondering the design".into(),
        hidden: false,
    };
    let text = |opts: ImageOpts<'_>| {
        entry_lines(&entry, &theme, 60, 0, opts)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let inline = text(ImageOpts::default());
    assert!(inline.contains("pondering the design"), "{inline}");

    let live = text(ImageOpts {
        thinking: ThinkingHiding::Live { hide: true },
        ..ImageOpts::default()
    });
    assert!(live.contains("Thinking..."), "{live}");
    assert!(!live.contains("pondering"), "{live}");
}

// ---------------------------------------------------------------------------------------------
// D — a second click before the frame
// ---------------------------------------------------------------------------------------------

/// Two clicks with no frame between them: the second is aimed where the first one's toggle has put
/// things. Block `a.rs` opens and pushes `b.rs` down; the click on `b.rs`'s new row must open it.
#[test]
fn a_second_click_before_the_frame_is_tested_against_the_toggled_document() {
    let scenario = || {
        let mut app = fullscreen_app();
        tool(&mut app, "a.rs", "a one\na two\na three");
        tool(&mut app, "b.rs", "b one");
        app
    };

    // Where `b.rs` ends up once `a.rs` is open: ask a twin that is allowed to draw.
    let mut twin = scenario();
    let rows = screen(&mut twin);
    let a_header = find(&rows, "read a.rs");
    let b_before = find(&rows, "read b.rs");
    click(&mut twin, a_header);
    let rows = screen(&mut twin);
    let b_after = find(&rows, "read b.rs");
    assert!(
        b_after.1 > b_before.1 + 1,
        "opening a.rs pushed b.rs down by more than its own header: {b_before:?} -> {b_after:?}"
    );

    let mut app = scenario();
    let _ = screen(&mut app);
    click(&mut app, a_header);
    click(&mut app, b_after);
    let rows = screen(&mut app);
    assert!(
        has(&rows, "a one"),
        "the first click opened a.rs: {rows:#?}"
    );
    assert!(
        has(&rows, "b one"),
        "the second click opened b.rs, at the row the toggle gave it: {rows:#?}"
    );
}
