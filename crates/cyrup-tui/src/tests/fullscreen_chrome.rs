//! The alternate screen paints the whole interface, not only the transcript.
//!
//! With `fullscreen` the default renderer, the frame has to carry everything inline mode draws: the
//! editor with what was typed into it, the footer, the selector that replaces the editor, the
//! overlays that float over everything, and the turn that is still streaming. Each test reads the
//! cells a real fullscreen frame was painted into (`App::draw` over a `TestBackend`), so what is
//! asserted is what a user would see.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::Frame;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;

use crate::overlay::{Overlay, OverlayOutcome};
use crate::theme::UiTheme;
use crate::{App, SelectorKind};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    // The startup hint bar would be a second thing on screen to tell the tests apart from.
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

/// The text of every screen row of the fullscreen frame just drawn.
fn screen(app: &mut App<TestBackend>) -> Vec<String> {
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

fn row_of(rows: &[String], needle: &str) -> Option<usize> {
    rows.iter().position(|r| r.contains(needle))
}

/// What was typed into the editor is on screen, below the transcript and above the footer — the
/// frame used to be the transcript and nothing else, so the prompt was invisible.
#[test]
fn the_editor_is_painted_under_the_transcript() {
    let mut app = fullscreen_app();
    app.transcript_mut().push_status("a line of transcript");
    app.editor_mut().handle_paste("typed into the prompt");
    app.draw().unwrap();
    let rows = screen(&mut app);

    let transcript = row_of(&rows, "a line of transcript").expect("the transcript is painted");
    let prompt = row_of(&rows, "typed into the prompt").expect("the editor text is painted");
    assert!(
        transcript < prompt,
        "the prompt belongs under the transcript: {rows:#?}"
    );
    assert!(
        prompt < usize::from(ROWS) - 1,
        "the footer sits below the prompt: {rows:#?}"
    );
}

/// The scrolled document gets the rows the dock leaves, and no more: the dock is a region of its
/// own, recorded for the pointer to resolve against.
#[test]
fn the_document_rectangle_is_what_the_dock_leaves() {
    let mut app = fullscreen_app();
    app.draw().unwrap();
    let regions = app.state_mut().regions;
    let doc = app.altscreen_for_test().unwrap().doc_area();

    assert_eq!(doc, regions.msg, "the renderer scrolls the message region");
    assert!(
        doc.height < ROWS,
        "a dock is painted under the document: {doc:?}"
    );
    assert!(
        regions.slot.y >= doc.y + doc.height,
        "the editor slot is below the document: {regions:?}"
    );
    assert!(regions.slot.height >= 3, "the editor keeps its three rows");
    assert_eq!(
        regions.footer.y + regions.footer.height,
        ROWS,
        "the footer is the last thing on screen: {regions:?}"
    );
}

/// The reply that is still streaming is part of the document; before this it appeared only once the
/// turn committed.
#[test]
fn a_streaming_reply_is_visible_before_it_commits() {
    let mut app = fullscreen_app();
    app.transcript_mut()
        .push_assistant_delta("a partial answer");
    app.draw().unwrap();
    let rows = screen(&mut app);
    assert!(
        row_of(&rows, "a partial answer").is_some(),
        "the in-flight reply is painted: {rows:#?}"
    );

    // …and it grows in place as more arrives.
    app.transcript_mut()
        .push_assistant_delta(" and the rest of it");
    app.draw().unwrap();
    let rows = screen(&mut app);
    assert!(
        row_of(&rows, "a partial answer and the rest of it").is_some(),
        "the reply follows the stream: {rows:#?}"
    );
}

/// A selector replaces the editor in the slot, exactly as inline.
#[test]
fn a_selector_owns_the_slot_while_it_is_open() {
    let mut app = fullscreen_app();
    app.editor_mut().handle_paste("draft that must not show");
    app.open_selector(SelectorKind::Theme);
    app.draw().unwrap();
    let rows = screen(&mut app);
    assert!(
        row_of(&rows, "draft that must not show").is_none(),
        "the editor is not painted under a selector: {rows:#?}"
    );
    assert!(
        row_of(&rows, "dark").is_some() && row_of(&rows, "light").is_some(),
        "the theme choices are painted: {rows:#?}"
    );
}

struct Marker;

impl Overlay for Marker {
    fn render(&mut self, frame: &mut Frame, area: Rect, _theme: &UiTheme) {
        frame.render_widget(
            ratatui::widgets::Paragraph::new("OVERLAY-MARKER"),
            Rect::new(area.x + 2, area.y + 2, 14, 1),
        );
    }
    fn handle(&mut self, _key: &KeyEvent) -> OverlayOutcome {
        OverlayOutcome::Ignored
    }
}

/// An overlay floats over the whole screen. It used to take every key while painting nothing, so a
/// modal extension UI froze an apparently idle screen.
#[test]
fn an_overlay_is_painted_over_the_frame() {
    let mut app = fullscreen_app();
    app.state_mut().overlays.push(Box::new(Marker));
    app.draw().unwrap();
    let rows = screen(&mut app);
    assert_eq!(
        row_of(&rows, "OVERLAY-MARKER"),
        Some(2),
        "the overlay is painted at the top of the screen: {rows:#?}"
    );
}

/// The terminal's own cursor sits on the editor's caret, in screen coordinates — the thing an IME
/// composes against and a screen reader follows. The editor is no longer at the top of the frame, so
/// a caret placed in editor-local coordinates would be a screen cell somewhere in the transcript.
#[test]
fn the_hardware_cursor_follows_the_editor_caret() {
    let mut app = fullscreen_app();
    app.editor_mut().set_show_hardware_cursor(true);
    app.editor_mut().handle_paste("abc");
    app.draw().unwrap();
    let slot = app.state_mut().regions.slot;
    let want = app
        .editor_mut()
        .cursor_in(slot)
        .expect("a caret is placed in the slot");
    let got = app.altscreen_for_test().unwrap().cursor_for_test();
    assert_eq!((got.x, got.y), want, "slot {slot:?}");
    assert!(
        got.y >= slot.y,
        "the caret is inside the editor, not the transcript"
    );
}

/// A terminal too short for the dock still draws: the editor keeps its three rows, the document is
/// squeezed to whatever is left (possibly nothing), and nothing panics.
#[test]
fn a_terminal_shorter_than_the_dock_still_draws() {
    for rows in [1_u16, 2, 3, 4, 5, 6] {
        let mut app = App::new(TestBackend::new(COLS, rows), UiTheme::dark()).unwrap();
        app.state_mut().show_startup_hints = false;
        app.state_mut().startup_header = crate::StartupHeader::Hidden;
        let _captured = app.enter_fullscreen_captured().expect("renderer builds");
        app.editor_mut().handle_paste("x");
        app.draw().unwrap();
        let regions = app.state_mut().regions;
        assert!(
            regions.slot.y + regions.slot.height <= rows,
            "{rows} rows: the slot stays on screen: {regions:?}"
        );
        assert!(
            regions.slot.height >= rows.min(3),
            "{rows} rows: the editor keeps its rows before anything else: {regions:?}"
        );
    }
}

/// At startup the hint block is the first rows of the document (pi's `headerContainer`), inside the
/// document region, and it stays there as the conversation starts: the first line lands under it
/// instead of replacing it.
#[test]
fn the_startup_hints_are_the_first_rows_of_the_document() {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    assert!(
        app.state_mut().show_startup_hints,
        "hints are on by default"
    );
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.draw().unwrap();
    let rows = screen(&mut app);
    let doc = app.state_mut().regions.msg;
    let hint_row = rows
        .iter()
        .enumerate()
        .find(|(_, r)| r.contains("interrupt") || r.contains("commands"))
        .map(|(i, _)| i)
        .unwrap_or_else(|| panic!("the hint bar is painted: {rows:#?}"));
    assert!(
        hint_row < usize::from(doc.y + doc.height),
        "the hints are inside the document region {doc:?}, row {hint_row}"
    );
    assert!(hint_row <= 2, "and at the top of it: {rows:#?}");

    app.transcript_mut().push_status("first line");
    app.transcript_mut().drain_committed();
    app.draw().unwrap();
    let rows = screen(&mut app);
    assert_eq!(
        rows.iter().position(|r| r.contains("commands")),
        Some(hint_row),
        "the block stays put: {rows:#?}"
    );
    assert!(
        rows.iter()
            .position(|r| r.contains("first line"))
            .is_some_and(|y| y > hint_row),
        "the first line lands under it: {rows:#?}"
    );
}
