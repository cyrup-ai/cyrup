//! The pointer over the editor and its completion popup in the alternate screen.
//!
//! Pi's `Editor.handleMouse` (`packages/tui/src/components/editor.ts:632-686` @v1.0.0): a left
//! click maps the pointer to a visual line of the wrapped buffer and a grapheme on it and puts the
//! caret there; a click on a completion row runs the list's `onSelect`, which applies the
//! completion. Each test drives a real fullscreen `App` over a `TestBackend` end to end —
//! `App::handle_input(InputEvent::Mouse(..))` — and clicks the cells the last frame painted, taken
//! from the recorded regions, then reads the caret back from the editor.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind as Kind,
};

use super::harness::{ctrl, key};
use crate::component::InputEvent;
use crate::{App, AppAction, UiTheme};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

fn mouse(kind: Kind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// A left click: press and release on one cell, each routed through `App::handle_input`. Answers
/// the action the release produced and repaints, as the run loop does after every event.
fn click(app: &mut App<TestBackend>, column: u16, row: u16) -> AppAction {
    app.handle_input(&mouse(Kind::Down(MouseButton::Left), column, row));
    let action = app.handle_input(&mouse(Kind::Up(MouseButton::Left), column, row));
    app.draw().unwrap();
    action
}

fn type_text(app: &mut App<TestBackend>, text: &str) {
    for c in text.chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
}

/// The screen cell of column `col` on visual line `vi` of the editor (top rule is row 0 of the
/// slot).
fn editor_cell(app: &App<TestBackend>, vi: u16, col: u16) -> (u16, u16) {
    let slot = app.state().regions.slot;
    (slot.x + col, slot.y + 1 + vi)
}

fn cursor(app: &App<TestBackend>) -> (usize, usize) {
    app.state().editor.cursor()
}

/// The text of screen row `y` of the fullscreen frame just drawn.
fn screen_row(app: &mut App<TestBackend>, y: u16) -> String {
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    (0..COLS)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

/// An app whose editor holds `text` (small paste: verbatim, newlines kept), drawn once.
fn app_with(text: &str) -> App<TestBackend> {
    let mut app = fullscreen_app();
    app.editor_mut().handle_paste(text);
    app.draw().unwrap();
    app
}

#[test]
fn a_click_puts_the_caret_on_the_clicked_line_and_column() {
    let mut app = app_with("alpha\nbeta gamma\ndelta");
    assert_eq!(
        cursor(&app),
        (2, 5),
        "the paste leaves the caret at the end"
    );

    let (x, y) = editor_cell(&app, 1, 3);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (1, 3));

    let (x, y) = editor_cell(&app, 0, 1);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, 1));
}

#[test]
fn a_click_past_the_end_of_a_line_lands_at_its_end() {
    let mut app = app_with("alpha\nbeta gamma\ndelta");
    let (x, y) = editor_cell(&app, 0, 60);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, 5));
}

#[test]
fn press_alone_and_a_drag_do_not_move_the_caret() {
    let mut app = app_with("alpha\nbeta gamma\ndelta");
    let (x, y) = editor_cell(&app, 0, 2);
    app.handle_input(&mouse(Kind::Down(MouseButton::Left), x, y));
    assert_eq!(cursor(&app), (2, 5), "a press is not a click");
    app.handle_input(&mouse(Kind::Drag(MouseButton::Left), x + 1, y));
    app.handle_input(&mouse(Kind::Up(MouseButton::Left), x + 1, y));
    assert_eq!(
        cursor(&app),
        (2, 5),
        "a drag that ends elsewhere is not a click"
    );
}

#[test]
fn the_rule_rows_claim_the_click_and_move_nothing() {
    let mut app = app_with("alpha\nbeta gamma\ndelta");
    let slot = app.state().regions.slot;
    for y in [slot.y, slot.y + slot.height - 1] {
        let action = click(&mut app, slot.x + 2, y);
        assert_eq!(cursor(&app), (2, 5), "row {y} is a rule");
        assert!(
            matches!(action, AppAction::None),
            "the click is the editor's, so the document sees nothing: {action:?}"
        );
    }
}

#[test]
fn a_click_below_the_last_visible_line_moves_nothing() {
    // The dock sizes the slot to the text, so a taller rectangle is a clipped or foreign layout;
    // the editor still answers for rows it did not paint a line on.
    let mut app = app_with("one\ntwo");
    let slot = app.state().regions.slot;
    let taller = ratatui::layout::Rect {
        height: slot.height + 3,
        ..slot
    };
    let at = ratatui::layout::Position::new(0, 4);
    let moved = app
        .editor_mut()
        .pointer(taller, crate::Pointer::Click { at, count: 1 });
    assert!(!moved, "nothing to repaint");
    assert_eq!(cursor(&app), (1, 3));
}

#[test]
fn the_right_half_of_a_wide_glyph_lands_on_its_start() {
    // 'a' is column 0, each ideograph two columns, 'b' column 5.
    let mut app = app_with("a世界b");
    for (column, expected) in [(0, 0), (1, 1), (2, 1), (3, 2), (4, 2), (5, 3), (30, 4)] {
        let (x, y) = editor_cell(&app, 0, column);
        click(&mut app, x, y);
        assert_eq!(cursor(&app), (0, expected), "click at column {column}");
    }
}

#[test]
fn a_click_past_a_wrapped_segment_stays_off_the_wrap_boundary() {
    let mut app = fullscreen_app();
    type_text(&mut app, "word ".repeat(30).trim_end());
    let map = app.state().editor.visual_line_map();
    assert!(map.len() >= 2, "the prompt must soft-wrap: {map:?}");
    let first = map[0];
    assert!(first.len < usize::from(COLS) - 4, "room right of segment 0");

    // Far right of the first wrap segment: past its text, but not at the boundary, which is the
    // first column of the next row.
    let (x, y) = editor_cell(&app, 0, COLS - 2);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, first.len - 1));

    // The last segment has no successor: past its end is the end of the line.
    let last = map[map.len() - 1];
    let (x, y) = editor_cell(&app, (map.len() - 1) as u16, COLS - 2);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, last.start + last.len));
}

#[test]
fn a_click_on_a_wrapped_row_counts_columns_from_that_rows_start() {
    let mut app = fullscreen_app();
    type_text(&mut app, "word ".repeat(30).trim_end());
    let second = app.state().editor.visual_line_map()[1];
    let (x, y) = editor_cell(&app, 1, 7);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, second.start + 7));
}

#[test]
fn a_click_on_a_scrolled_editor_resolves_against_the_visible_window() {
    let mut app = fullscreen_app();
    let lines: Vec<String> = (0..40).map(|i| format!("line{i:02}")).collect();
    app.editor_mut().set_text(&lines.join("\n"));
    app.draw().unwrap();
    // The caret is on the last line, so the window is scrolled to the bottom.
    let (x, y) = editor_cell(&app, 0, 2);
    click(&mut app, x, y);
    let (row, col) = cursor(&app);
    assert_eq!(col, 2);
    assert!(
        row > 0,
        "the first visible row is not logical line 0: {row}"
    );
    app.draw().unwrap();
    let slot = app.state().regions.slot;
    let rendered = screen_row(&mut app, slot.y + 1);
    assert!(
        rendered.contains(&format!("line{row:02}")),
        "row {row} is the line the click landed on, and it is the one painted there: {rendered}"
    );
}

#[test]
fn a_paste_marker_is_one_target() {
    let mut app = fullscreen_app();
    app.editor_mut().handle_paste("x ");
    app.editor_mut().handle_paste(&"y\n".repeat(30));
    app.editor_mut().handle_paste(" z");
    app.draw().unwrap();
    let text = app.state().editor.text();
    let start = text.chars().position(|c| c == '[').expect("a marker");
    let end = text.chars().position(|c| c == ']').expect("a marker") + 1;
    assert!(end - start > 8, "{text:?}");
    // Every column of the marker, left half and right half alike, is the marker's start.
    for column in [start + 1, (start + end) / 2, end - 2] {
        let (x, y) = editor_cell(&app, 0, column as u16);
        click(&mut app, x, y);
        assert_eq!(cursor(&app), (0, start), "click at column {column}");
    }
    let (x, y) = editor_cell(&app, 0, end as u16);
    click(&mut app, x, y);
    assert_eq!(
        cursor(&app),
        (0, end),
        "the cell after the marker is past it"
    );
}

#[test]
fn a_click_forgets_the_sticky_column_of_a_vertical_run() {
    let mut app = app_with("abcdefghij\nxyz\nabcdefghij");
    // Up from the end of line 2 parks at the end of the shorter line and remembers column 10.
    app.handle_input(&key(KeyCode::Up));
    assert_eq!(cursor(&app), (1, 3));
    app.draw().unwrap();
    // Click at the end of that same line. The caret does not move, but the run is over.
    let (x, y) = editor_cell(&app, 1, 9);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (1, 3));
    app.handle_input(&key(KeyCode::Down));
    assert_eq!(
        cursor(&app),
        (2, 3),
        "the run starts from the clicked column, not the one remembered before it"
    );
}

#[test]
fn the_editors_horizontal_padding_offsets_the_columns() {
    let mut app = fullscreen_app();
    app.editor_mut().set_padding_x(2);
    app.editor_mut().handle_paste("abcdef");
    app.draw().unwrap();
    // Columns 0 and 1 are padding, so text column 3 is screen column 5.
    let (x, y) = editor_cell(&app, 0, 5);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, 3));
    // A click in the padding itself clamps to the first column.
    let (x, y) = editor_cell(&app, 0, 1);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, 0));
}

#[test]
fn a_click_leaves_history_browsing() {
    let mut app = fullscreen_app();
    app.editor_mut().push_history("older");
    app.handle_input(&key(KeyCode::Up));
    app.draw().unwrap();
    assert_eq!(app.state().editor.text(), "older", "Up recalled history");
    let (x, y) = editor_cell(&app, 0, 2);
    click(&mut app, x, y);
    // Still browsing, Down would walk forward out of history and restore the draft.
    app.handle_input(&key(KeyCode::Down));
    assert_eq!(app.state().editor.text(), "older");
}

#[test]
fn a_click_is_the_end_of_a_typing_run_for_undo() {
    let mut app = fullscreen_app();
    type_text(&mut app, "hello");
    let (x, y) = editor_cell(&app, 0, 2);
    click(&mut app, x, y);
    type_text(&mut app, "X");
    assert_eq!(app.state().editor.text(), "heXllo");
    app.handle_input(&ctrl(KeyCode::Char('-')));
    assert_eq!(
        app.state().editor.text(),
        "hello",
        "typing after a click is a unit of its own, not part of the run before it"
    );
}

#[test]
fn double_and_triple_clicks_place_the_caret_like_a_single_one() {
    let mut app = app_with("alpha beta\ngamma");
    let (x, y) = editor_cell(&app, 0, 7);
    for _ in 0..4 {
        click(&mut app, x, y);
        assert_eq!(cursor(&app), (0, 7));
    }
    assert_eq!(app.state().editor.text(), "alpha beta\ngamma");
}

// ---- the completion popup ---------------------------------------------------------------------

fn popup_len(app: &App<TestBackend>) -> usize {
    app.state()
        .editor
        .autocomplete()
        .map_or(0, |ac| ac.list.items().len())
}

fn selected(app: &App<TestBackend>) -> usize {
    app.state()
        .editor
        .autocomplete()
        .map_or(usize::MAX, |ac| ac.list.selected())
}

fn popup_row(app: &App<TestBackend>, row: u16, dx: u16) -> (u16, u16) {
    let popup = app.state().regions.popup;
    assert!(popup.height > row, "popup is {popup:?}");
    (popup.x + dx, popup.y + row)
}

#[test]
fn a_click_on_a_completion_row_applies_that_completion_without_submitting() {
    let mut app = fullscreen_app();
    type_text(&mut app, "/m");
    assert!(popup_len(&app) >= 2, "`/m` offers several commands");

    // Press highlights the row, so the highlighted completion is the one the click must apply.
    let (x, y) = popup_row(&app, 1, 3);
    app.handle_input(&mouse(Kind::Down(MouseButton::Left), x, y));
    app.draw().unwrap();
    assert_eq!(selected(&app), 1);
    let value = app
        .state()
        .editor
        .autocomplete()
        .and_then(|ac| ac.selected().map(|c| c.value.clone()))
        .unwrap();
    let action = app.handle_input(&mouse(Kind::Up(MouseButton::Left), x, y));
    app.draw().unwrap();

    assert!(
        !matches!(action, AppAction::Submit(_)),
        "pi's onSelect applies the completion and nothing else: {action:?}"
    );
    assert_eq!(app.state().editor.text(), format!("/{value} "));
    assert!(
        app.state().editor.autocomplete().is_none(),
        "the popup closes"
    );
}

#[test]
fn clicking_a_completion_can_be_undone_as_one_step() {
    let mut app = fullscreen_app();
    type_text(&mut app, "/m");
    let (x, y) = popup_row(&app, 0, 3);
    click(&mut app, x, y);
    assert_ne!(app.state().editor.text(), "/m");
    app.handle_input(&ctrl(KeyCode::Char('-')));
    assert_eq!(app.state().editor.text(), "/m");
}

#[test]
fn the_wheel_over_the_popup_moves_the_highlight_and_clamps_at_the_ends() {
    let mut app = fullscreen_app();
    type_text(&mut app, "/m");
    let last = popup_len(&app) - 1;
    assert!(last >= 2);
    let (x, y) = popup_row(&app, 0, 3);

    assert!(matches!(
        app.handle_input(&mouse(Kind::ScrollDown, x, y)),
        AppAction::Redraw
    ));
    assert_eq!(selected(&app), 1);
    for _ in 0..last + 3 {
        app.handle_input(&mouse(Kind::ScrollDown, x, y));
    }
    assert_eq!(selected(&app), last, "no wrap past the end");

    // A notch against the end changes nothing, and is still the popup's: the document under it
    // must not scroll.
    let claimed = app.handle_dock_pointer(&MouseEvent {
        kind: Kind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(claimed.is_some(), "the popup keeps the wheel");

    for _ in 0..last + 3 {
        app.handle_input(&mouse(Kind::ScrollUp, x, y));
    }
    assert_eq!(selected(&app), 0, "no wrap past the start");
}

#[test]
fn a_click_in_the_padding_columns_still_belongs_to_the_row_it_is_level_with() {
    let mut app = fullscreen_app();
    app.editor_mut().set_padding_x(2);
    type_text(&mut app, "/m");
    let popup = app.state().regions.popup;
    assert!(popup.height >= 2);
    // Column 0 of the popup rectangle is padding: the list is painted two columns in. Pi shifts
    // `x` by the padding and the list never reads it, so the row is still hit.
    click(&mut app, popup.x, popup.y + 1);
    assert!(app.state().editor.autocomplete().is_none());
    assert_ne!(app.state().editor.text(), "/m", "a completion was applied");
}

#[test]
fn a_click_on_a_mention_row_applies_it() {
    let mut app = fullscreen_app();
    app.editor_mut()
        .set_mention_files(vec!["alpha.rs".into(), "beta.rs".into()]);
    type_text(&mut app, "see @");
    assert!(popup_len(&app) >= 2);
    let (x, y) = popup_row(&app, 1, 2);
    click(&mut app, x, y);
    assert!(app.state().editor.autocomplete().is_none());
    let text = app.state().editor.text();
    assert!(
        text.starts_with("see @") && text.len() > "see @".len() && text.ends_with(".rs "),
        "{text:?}"
    );
}

#[test]
fn a_click_in_the_text_refreshes_the_open_popup() {
    let mut app = fullscreen_app();
    type_text(&mut app, "/mo");
    let narrow = popup_len(&app);
    // Between `/` and `mo`: the token before the caret is now just `/`, which matches everything.
    let (x, y) = editor_cell(&app, 0, 1);
    click(&mut app, x, y);
    assert_eq!(cursor(&app), (0, 1));
    assert!(
        popup_len(&app) > narrow,
        "the popup was recomputed for the new caret: {narrow} -> {}",
        popup_len(&app)
    );

    // Before the `/` there is no completion context at all.
    let (x, y) = editor_cell(&app, 0, 0);
    click(&mut app, x, y);
    assert!(app.state().editor.autocomplete().is_none());
}
