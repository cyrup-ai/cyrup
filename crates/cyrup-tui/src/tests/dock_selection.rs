//! Text selection over the dock in the alternate screen.
//!
//! Pi's renderer-level selection runs over every rendered screen line: a press that no component
//! acts on starts a selection, and the editor deliberately leaves press, drag and release unhandled
//! so the prompt can be selected and copied with the mouse (`components/editor.ts:656-658`,
//! `tui-alt-screen.ts:938-962` and `:1313-1391` @v1.0.0). A component that DOES act on a press (a
//! selector row, a completion row) keeps it and starts no selection.
//!
//! Each test drives a real fullscreen `App` over a `TestBackend` end to end with
//! `App::handle_input(InputEvent::Mouse(..))`, finds the cells it presses in the frame it drew, and
//! asserts on what the run loop would act on (`AppAction::CopySelection`) and on the cells the next
//! frame paints (the reverse-video highlight).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::style::Modifier;

use super::harness::key;
use crate::component::InputEvent;
use crate::{App, AppAction, ExtensionWidget, SelectorKind, UiTheme};
use cyrup_session_svc::AgentSessionEvent;

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

fn down(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Left), at.0, at.1))
}

fn drag(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), at.0, at.1))
}

fn up(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Left), at.0, at.1))
}

/// A whole gesture: press at `from`, drag to `to`, release there. Answers the release's action and
/// repaints, as the run loop does after every event.
fn drag_across(app: &mut App<TestBackend>, from: (u16, u16), to: (u16, u16)) -> AppAction {
    down(app, from);
    drag(app, to);
    let action = up(app, to);
    app.draw().unwrap();
    action
}

/// A press and release on one cell: a click.
fn click(app: &mut App<TestBackend>, at: (u16, u16)) -> AppAction {
    down(app, at);
    let action = up(app, at);
    app.draw().unwrap();
    action
}

fn type_text(app: &mut App<TestBackend>, text: &str) {
    for c in text.chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
}

/// The glyphs of screen row `y` of the frame just drawn, each with the column it starts in. A wide
/// glyph's second cell holds no glyph of its own.
fn glyphs(app: &mut App<TestBackend>, y: u16) -> Vec<(u16, String)> {
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    let mut out = Vec::new();
    let mut x = 0;
    while x < COLS {
        let symbol = buf[(x, y)].symbol().to_string();
        let step = crate::text_width::str_width(&symbol).max(1) as u16;
        out.push((x, symbol));
        x += step;
    }
    out
}

/// The text of screen row `y`, trailing blanks dropped.
fn row_text(app: &mut App<TestBackend>, y: u16) -> String {
    let text: String = glyphs(app, y)
        .into_iter()
        .map(|(_, symbol)| symbol)
        .collect();
    text.trim_end().to_string()
}

/// The screen cell of the first occurrence of `needle` in the frame just drawn.
fn find(app: &mut App<TestBackend>, needle: &str) -> (u16, u16) {
    for y in 0..ROWS {
        let row = glyphs(app, y);
        for i in 0..row.len() {
            let rest: String = row[i..].iter().map(|(_, symbol)| symbol.as_str()).collect();
            if rest.starts_with(needle) {
                return (row[i].0, y);
            }
        }
    }
    panic!("{needle:?} is not on screen");
}

fn reversed(app: &mut App<TestBackend>, at: (u16, u16)) -> bool {
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    alt.backend_for_test().buffer()[(at.0, at.1)]
        .modifier
        .contains(Modifier::REVERSED)
}

/// Every cell of screen row `y` that carries the selection highlight, as a column range. The
/// caret is drawn in reverse video too, so its cell is not counted.
fn highlighted(app: &mut App<TestBackend>, y: u16) -> Option<(u16, u16)> {
    let slot = app.state().regions.slot;
    let caret = app.editor_mut().cursor_in(slot);
    let cols: Vec<u16> = (0..COLS)
        .filter(|&x| reversed(app, (x, y)) && Some((x, y)) != caret)
        .collect();
    Some((*cols.first()?, *cols.last()?))
}

/// The first row of the footer that paints text.
fn footer_row(app: &mut App<TestBackend>) -> u16 {
    app.draw().unwrap();
    let footer = app.state().regions.footer;
    (footer.y..footer.bottom())
        .find(|&y| !row_text(app, y).is_empty())
        .expect("the footer paints something")
}

fn selection(app: &mut App<TestBackend>) -> Option<String> {
    app.altscreen_for_test()
        .expect("fullscreen is live")
        .selection_text()
}

fn copied(action: AppAction) -> String {
    match action {
        AppAction::CopySelection(text) => text,
        other => panic!("expected a copy, got {other:?}"),
    }
}

fn cursor(app: &App<TestBackend>) -> (usize, usize) {
    app.state().editor.cursor()
}

/// An app with `text` typed into the prompt and the frame drawn; answers the screen cell the first
/// character was painted in.
fn app_with_prompt(text: &str) -> (App<TestBackend>, (u16, u16)) {
    let mut app = fullscreen_app();
    type_text(&mut app, text);
    let origin = find(&mut app, text);
    (app, origin)
}

// ---- the editor -------------------------------------------------------------------------------

/// The row in `TUI-147`: text typed into the prompt can be selected with the mouse, and the release
/// copies it.
#[test]
fn a_drag_across_the_prompt_copies_the_typed_text() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    let action = drag_across(&mut app, (x, y), (x + 4, y));
    assert_eq!(copied(action), "hello");
}

/// The highlight is painted over the dock cells, covering exactly the selected ones, and survives
/// the release (copy-on-select does not clear it).
#[test]
fn the_selection_is_highlighted_over_the_prompt_cells() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    drag_across(&mut app, (x, y), (x + 4, y));
    assert_eq!(highlighted(&mut app, y), Some((x, x + 4)));
    assert!(!reversed(&mut app, (x + 5, y)), "the space after `hello`");
    assert_eq!(selection(&mut app).as_deref(), Some("hello"));
}

/// A drag that starts on the editor selects instead of moving the caret: pi synthesises a click
/// only from a press and release on one cell with nothing between them.
#[test]
fn a_drag_across_the_prompt_does_not_move_the_caret() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    assert_eq!(cursor(&app), (0, 11));
    drag_across(&mut app, (x + 1, y), (x + 6, y));
    assert_eq!(cursor(&app), (0, 11), "the caret stayed where it was typed");
}

/// …while a click on the same cell still places the caret, and copies nothing.
#[test]
fn a_click_on_the_prompt_places_the_caret_and_copies_nothing() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    let action = click(&mut app, (x + 2, y));
    assert_eq!(cursor(&app), (0, 2));
    assert!(!matches!(action, AppAction::CopySelection(_)), "{action:?}");
    assert_eq!(selection(&mut app), None);
    assert_eq!(highlighted(&mut app, y), None);
}

/// A selection spans the rows of a multi-line prompt: whole rows between the ends, trailing blanks
/// trimmed from each, joined by newlines.
#[test]
fn a_drag_down_a_multi_line_prompt_copies_its_lines() {
    let mut app = fullscreen_app();
    app.editor_mut().handle_paste("alpha\nbeta gamma\ndelta");
    app.draw().unwrap();
    let (x, y) = find(&mut app, "alpha");
    let action = drag_across(&mut app, (x + 2, y), (x + 3, y + 2));
    assert_eq!(copied(action), "pha\nbeta gamma\ndelt");
}

/// A wide glyph is one grapheme two cells wide: the copy takes it whole, and the column of
/// everything after it is its screen column.
#[test]
fn a_wide_glyph_is_copied_whole_and_keeps_later_columns_aligned() {
    let (mut app, (x, y)) = app_with_prompt("a世界b");
    // 'a' is column 0, each ideograph two columns, 'b' column 5; end in the middle of 界.
    let action = drag_across(&mut app, (x, y), (x + 4, y));
    assert_eq!(copied(action), "a世界");
    // 界 starts in column 3; the backend draws a wide glyph once, from its first cell.
    assert_eq!(highlighted(&mut app, y), Some((x, x + 3)));
    let action = drag_across(&mut app, (x + 1, y), (x + 5, y));
    assert_eq!(copied(action), "世界b");
}

/// A backwards drag selects the same text.
#[test]
fn a_drag_to_the_left_selects_the_same_range() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    let action = drag_across(&mut app, (x + 10, y), (x + 6, y));
    assert_eq!(copied(action), "world");
}

/// With copy-on-select off the release withholds the copy and keeps the selection, which `/copy`
/// then reads.
#[test]
fn copy_on_select_off_keeps_the_prompt_selection_without_copying() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    app.altscreen_for_test().unwrap().set_copy_on_select(false);
    let action = drag_across(&mut app, (x, y), (x + 4, y));
    assert!(!matches!(action, AppAction::CopySelection(_)), "{action:?}");
    assert_eq!(selection(&mut app).as_deref(), Some("hello"));
    assert_eq!(highlighted(&mut app, y), Some((x, x + 4)));
}

// ---- multi-click ------------------------------------------------------------------------------

/// A double click selects the word under the pointer and a triple click the whole screen row, on
/// dock cells as on document rows.
#[test]
fn double_click_selects_a_word_and_triple_click_a_line_in_the_prompt() {
    let (mut app, (x, y)) = app_with_prompt("alpha beta gamma");
    let beta = (x + 7, y);
    // The first click places the caret (the editor takes it), the second is a double click.
    click(&mut app, beta);
    assert_eq!(cursor(&app), (0, 7));
    let action = click(&mut app, beta);
    assert_eq!(copied(action), "beta");
    assert_eq!(highlighted(&mut app, y), Some((x + 6, x + 9)));
    let action = click(&mut app, beta);
    let line = row_text(&mut app, y);
    assert_eq!(copied(action), line);
    assert!(line.contains("alpha beta gamma"), "{line:?}");
}

/// A word keeps a path whole across `/` and `-`, as pi's joiners do.
#[test]
fn a_double_click_keeps_a_path_whole() {
    let (mut app, (x, y)) = app_with_prompt("open src/some-file.rs now");
    let inside = (x + 9, y);
    click(&mut app, inside);
    let action = click(&mut app, inside);
    assert_eq!(copied(action), "src/some-file.rs");
}

// ---- the rest of the dock ---------------------------------------------------------------------

/// The footer is selectable text like any other row.
#[test]
fn a_drag_across_the_footer_copies_it() {
    let mut app = fullscreen_app();
    let y = footer_row(&mut app);
    let row = row_text(&mut app, y);
    let width = row.chars().count() as u16;
    let action = drag_across(&mut app, (0, y), (width - 1, y));
    assert_eq!(copied(action), row);
    assert_eq!(highlighted(&mut app, y).map(|(first, _)| first), Some(0));
}

/// Extension widgets and queued messages are dock rows too.
#[test]
fn widgets_and_queued_messages_are_selectable() {
    let mut app = fullscreen_app();
    app.state_mut().extension_widgets.push(ExtensionWidget {
        key: "w".into(),
        lines: vec!["WIDGET-ROW text".into()],
        below: false,
    });
    app.ingest_event(&AgentSessionEvent::QueueUpdate {
        steering: vec!["QUEUEDMSG".into()],
        follow_up: vec![],
    });
    app.draw().unwrap();

    let (x, y) = find(&mut app, "WIDGET-ROW");
    let action = drag_across(&mut app, (x, y), (x + 9, y));
    assert_eq!(copied(action), "WIDGET-ROW");

    let (x, y) = find(&mut app, "Steering");
    let action = drag_across(&mut app, (x, y), (x + 18, y));
    assert_eq!(copied(action), "Steering: QUEUEDMSG");
}

/// The rule rows around the editor and blank rows start a selection as well: nothing acts on them.
#[test]
fn a_press_on_a_rule_row_starts_a_selection() {
    let mut app = fullscreen_app();
    app.draw().unwrap();
    let slot = app.state().regions.slot;
    let action = drag_across(&mut app, (slot.x, slot.y), (slot.x + 5, slot.y));
    assert!(
        !matches!(action, AppAction::CopySelection(ref text) if text.is_empty()),
        "{action:?}"
    );
    assert_eq!(highlighted(&mut app, slot.y), Some((slot.x, slot.x + 5)));
}

// ---- components keep what they act on ---------------------------------------------------------

/// A press on a selector row is the selector's: the row is highlighted, no selection starts, and one
/// already made is cleared. A row that is not an item (the title) is nobody's, so it is selectable.
#[test]
fn a_selector_row_keeps_its_press_and_a_title_is_selectable() {
    let mut app = fullscreen_app();
    app.state_mut().available_thinking_levels = ["off", "low", "high"].map(str::to_string).to_vec();
    app.state_mut().thinking_level = "low".into();
    app.open_selector(SelectorKind::Thinking);
    app.draw().unwrap();

    // A selection from the footer first.
    let footer = footer_row(&mut app);
    drag_across(&mut app, (0, footer), (5, footer));
    assert!(selection(&mut app).is_some(), "the footer is selected");

    // A press on a level row: the selector's, and it drops the selection.
    let high = find(&mut app, "high");
    down(&mut app, high);
    assert_eq!(
        selection(&mut app),
        None,
        "a handled press clears the selection"
    );
    assert!(
        app.state().pointer.press_in_flight(),
        "the selector holds the gesture"
    );
    drag(&mut app, (high.0 + 3, high.1));
    let action = up(&mut app, (high.0 + 3, high.1));
    app.draw().unwrap();
    assert!(!matches!(action, AppAction::CopySelection(_)), "{action:?}");
    assert_eq!(
        selection(&mut app),
        None,
        "a drag over a handled press selects nothing"
    );
    assert_eq!(highlighted(&mut app, high.1), None);

    // The title is not an item: dragging across it selects it.
    let title = find(&mut app, "Thinking Level");
    let action = drag_across(&mut app, title, (title.0 + 7, title.1));
    assert_eq!(copied(action), "Thinking");
}

/// A press on a completion row is the popup's.
#[test]
fn a_completion_row_keeps_its_press() {
    let mut app = fullscreen_app();
    type_text(&mut app, "/m");
    let footer = footer_row(&mut app);
    drag_across(&mut app, (0, footer), (5, footer));
    assert!(selection(&mut app).is_some());

    let popup = app.state().regions.popup;
    assert!(popup.height > 0);
    down(&mut app, (popup.x + 3, popup.y));
    assert_eq!(selection(&mut app), None);
    assert!(app.state().pointer.press_in_flight());
    up(&mut app, (popup.x + 4, popup.y));
}

// ---- what clears a selection ------------------------------------------------------------------

/// A new press replaces the selection.
#[test]
fn a_new_press_replaces_the_selection() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    drag_across(&mut app, (x, y), (x + 4, y));
    assert_eq!(highlighted(&mut app, y), Some((x, x + 4)));
    drag_across(&mut app, (x + 6, y), (x + 10, y));
    assert_eq!(highlighted(&mut app, y), Some((x + 6, x + 10)));
    assert_eq!(selection(&mut app).as_deref(), Some("world"));
}

/// Losing focus in the middle of a drag drops the selection; a finished one survives it.
#[test]
fn focus_loss_cancels_an_in_flight_drag_but_not_a_finished_selection() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    down(&mut app, (x, y));
    drag(&mut app, (x + 4, y));
    app.handle_input(&InputEvent::FocusLost);
    app.draw().unwrap();
    assert_eq!(selection(&mut app), None);
    assert!(!reversed(&mut app, (x + 2, y)), "the highlight is gone");

    drag_across(&mut app, (x, y), (x + 4, y));
    app.handle_input(&InputEvent::FocusLost);
    app.draw().unwrap();
    assert_eq!(selection(&mut app).as_deref(), Some("hello"));
}

/// The selection holds screen coordinates, not text: if the prompt is repainted with other text
/// under a live selection the highlight stays on the same cells and a copy reads what is painted
/// there now. Pi's selection is `{row, col}` over `previousScreen`, which the next frame replaces.
#[test]
fn a_repaint_under_a_live_selection_keeps_the_cells_and_reads_the_new_text() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    drag_across(&mut app, (x, y), (x + 4, y));
    assert_eq!(selection(&mut app).as_deref(), Some("hello"));

    app.editor_mut().set_text("HOWDY world");
    app.draw().unwrap();
    assert_eq!(highlighted(&mut app, y), Some((x, x + 4)));
    assert_eq!(selection(&mut app).as_deref(), Some("HOWDY"));
}

/// A secondary-button press over the dock is not a selection gesture and not a click.
#[test]
fn a_right_click_over_the_prompt_is_unchanged() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    let action = app.handle_input(&mouse(MouseEventKind::Down(MouseButton::Right), x + 2, y));
    assert!(matches!(action, AppAction::None), "{action:?}");
    app.handle_input(&mouse(MouseEventKind::Up(MouseButton::Right), x + 2, y));
    assert_eq!(cursor(&app), (0, 11));
    assert_eq!(selection(&mut app), None);
}

// ---- document and dock ------------------------------------------------------------------------

fn app_with_transcript_and_prompt() -> (App<TestBackend>, (u16, u16), (u16, u16)) {
    let mut app = fullscreen_app();
    app.transcript_mut().push_status("transcript line one");
    app.transcript_mut().push_status("transcript line two");
    type_text(&mut app, "typed prompt");
    let doc = find(&mut app, "transcript line one");
    let prompt = find(&mut app, "typed prompt");
    (app, doc, prompt)
}

/// A drag that starts in the document and ends over the dock keeps selecting document rows,
/// clamped to the viewport: nothing of the dock is copied.
#[test]
fn a_drag_from_the_document_into_the_dock_stays_in_the_document() {
    let (mut app, doc, prompt) = app_with_transcript_and_prompt();
    let action = drag_across(&mut app, doc, (prompt.0 + 40, prompt.1));
    let text = copied(action);
    assert!(text.starts_with("transcript line one"), "{text:?}");
    assert!(text.contains("transcript line two"), "{text:?}");
    assert!(
        !text.contains("typed"),
        "no dock text in a document selection: {text:?}"
    );
    assert_eq!(highlighted(&mut app, prompt.1), None);
}

/// A drag that starts in the dock selects what is painted, wherever the pointer goes: pi resolves
/// every point of the gesture against the rows its anchor was on (`getSelectionPoint`,
/// `tui-alt-screen.ts:1149-1158`), and the screen has no scroll view to keep it in.
#[test]
fn a_drag_from_the_dock_up_over_the_document_selects_screen_rows() {
    let (mut app, doc, prompt) = app_with_transcript_and_prompt();
    let action = drag_across(&mut app, (prompt.0 + 5, prompt.1), (doc.0 + 11, doc.1));
    let text = copied(action);
    assert!(text.starts_with("line one"), "{text:?}");
    assert!(text.contains("transcript line two"), "{text:?}");
    assert!(text.ends_with("typed"), "{text:?}");
    assert!(
        highlighted(&mut app, doc.1).is_some(),
        "the document rows are highlighted too"
    );
}

/// A row of the popup the list does not act on (the `(i/N)` readout) is nobody's, so it is
/// selectable; pi's `SelectList.handleMouse` answers `undefined` there.
#[test]
fn the_popup_readout_row_is_selectable() {
    let mut app = fullscreen_app();
    type_text(&mut app, "/");
    let readout = find(&mut app, "(1/");
    let popup = app.state().regions.popup;
    assert!(popup.contains(ratatui::layout::Position::new(readout.0, readout.1)));
    let action = drag_across(&mut app, readout, (readout.0 + 2, readout.1));
    assert_eq!(copied(action), "(1/");
    assert!(
        app.state().editor.autocomplete().is_some(),
        "the popup is still open: the press was not a click on a row"
    );
}

/// A selection that began in the dock may span several dock components: from the prompt down to the
/// footer, the rows between are taken whole. (A pointer clamped into the document's viewport, as a
/// document gesture's is, could never reach below it.)
#[test]
fn a_drag_from_the_prompt_down_to_the_footer_spans_the_dock() {
    let (mut app, (x, y)) = app_with_prompt("hello world");
    let footer = footer_row(&mut app);
    assert!(footer > y);
    let footer_text = row_text(&mut app, footer);
    let last = (footer_text.chars().count() - 1) as u16;
    down(&mut app, (x + 6, y));
    drag(&mut app, (last, footer));
    // Mid-gesture, before the release re-resolves the focus.
    let live = selection(&mut app).expect("a live drag has selected text");
    assert!(live.ends_with(footer_text.trim()), "{live:?}");
    let action = up(&mut app, (last, footer));
    let text = copied(action);
    assert!(text.starts_with("world"), "{text:?}");
    assert!(text.ends_with(footer_text.trim()), "{text:?}");
    assert!(text.lines().count() >= 2, "{text:?}");
}
