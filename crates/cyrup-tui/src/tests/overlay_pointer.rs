//! The pointer over an extension overlay on the fullscreen screen.
//!
//! pi hit-tests a mouse report against the overlays first (`dispatchMouseToOverlay`, `tui.ts`): the
//! overlay under the pointer takes the event and nothing beneath it sees it, while a report that
//! lands outside every overlay carries on to the dock and the scrolled document. These tests drive
//! a fullscreen `App<TestBackend>` with real `ExtensionOverlay`s and read the cells the frame was
//! painted into, so they see what a user would: the highlighted row after a wheel notch, the
//! document scrolled by a notch that missed the overlay, the colours an overlay is painted in after
//! the theme changed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::{Arc, Mutex};

use cyrup_ext::host::{
    CustomSpec, InteractiveOverlay, OverlayColor, OverlayKey, OverlayLine, OverlayMouse,
    OverlayMouseOutcome, OverlayOutcome as ExtOverlayOutcome, OverlaySpan, ThemeRole,
};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

use crate::app::OverlayRoute;
use crate::overlay::ExtensionOverlay;
use crate::{App, AppAction, InputEvent, UiTheme};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    app
}

/// Open `overlay` the way the host does for an extension and paint one frame.
fn open(app: &mut App<TestBackend>, overlay: Box<dyn InteractiveOverlay>) -> OverlayDone {
    let (done, released) = tokio::sync::oneshot::channel();
    app.state_mut()
        .overlays
        .push(Box::new(ExtensionOverlay::new(overlay, done)));
    app.draw().unwrap();
    released
}

type OverlayDone = tokio::sync::oneshot::Receiver<()>;

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

fn cell_fg(app: &mut App<TestBackend>, x: u16, y: u16) -> Color {
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    alt.backend_for_test().buffer()[(x, y)].fg
}

/// The screen row a text is on.
fn row_of(app: &mut App<TestBackend>, needle: &str) -> u16 {
    let rows = screen(app);
    let row = rows
        .iter()
        .position(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} is not on screen: {rows:#?}"));
    u16::try_from(row).unwrap()
}

/// The text of the highlighted row of a `SpecOverlay` chooser (its `> ` gutter).
fn highlighted(app: &mut App<TestBackend>) -> String {
    screen(app)
        .into_iter()
        .find_map(|r| {
            r.contains("option-")
                .then(|| r.split_once("> "))
                .flatten()
                // The scrollbar's glyph in the last column is outside the overlay's box.
                .map(|(_, label)| label.trim_end_matches(['│', '┃', '█']).trim().to_string())
        })
        .unwrap_or_default()
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}
fn down(column: u16, row: u16) -> InputEvent {
    mouse(MouseEventKind::Down(MouseButton::Left), column, row)
}
fn up(column: u16, row: u16) -> InputEvent {
    mouse(MouseEventKind::Up(MouseButton::Left), column, row)
}
fn notch_down(column: u16, row: u16) -> InputEvent {
    mouse(MouseEventKind::ScrollDown, column, row)
}
fn notch_up(column: u16, row: u16) -> InputEvent {
    mouse(MouseEventKind::ScrollUp, column, row)
}

/// A press and its release on one cell.
fn click(app: &mut App<TestBackend>, column: u16, row: u16) {
    app.handle_input(&down(column, row));
    app.handle_input(&up(column, row));
}

/// The rectangle the topmost overlay was last painted into.
fn painted(app: &mut App<TestBackend>) -> Rect {
    use crate::overlay::Overlay;
    app.state_mut()
        .overlays
        .last()
        .and_then(|o| Overlay::painted_rect(o.as_ref()))
        .expect("the overlay has been painted")
}

/// A real chooser, the way a WASM guest's `ui.custom` spec becomes one.
fn chooser() -> (Box<dyn InteractiveOverlay>, Arc<Mutex<Option<String>>>) {
    let spec = CustomSpec::from_json(&serde_json::json!({
        "title": "Pick one",
        "options": ["option-alpha", "option-bravo", "option-charlie", "option-delta"],
    }));
    let (overlay, result) = spec.into_overlay();
    (Box::new(overlay), result)
}

/// An overlay that records the pointer events it is given and answers a fixed outcome.
struct Probe {
    events: Arc<Mutex<Vec<OverlayMouse>>>,
    outcome: OverlayMouseOutcome,
    rows: usize,
}

impl InteractiveOverlay for Probe {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        let span = |text: &str, fg: Option<OverlayColor>| {
            OverlayLine::new(vec![OverlaySpan {
                text: text.to_string(),
                fg,
                ..OverlaySpan::default()
            }])
        };
        let mut lines = vec![
            span("ACCENT-ROW", Some(OverlayColor::Theme(ThemeRole::Accent))),
            span("DIM-ROW", Some(OverlayColor::Theme(ThemeRole::Dim))),
            span("MUTED-ROW", Some(OverlayColor::Theme(ThemeRole::Muted))),
            span("LITERAL-ROW", Some(OverlayColor::Red)),
        ];
        while lines.len() < self.rows {
            lines.push(span("filler", None));
        }
        lines
    }
    fn handle_key(&mut self, _key: OverlayKey) -> ExtOverlayOutcome {
        ExtOverlayOutcome::Ignored
    }
    fn handle_mouse(&mut self, event: OverlayMouse) -> OverlayMouseOutcome {
        self.events.lock().unwrap().push(event);
        self.outcome
    }
}

fn probe(rows: usize, outcome: OverlayMouseOutcome) -> (Box<Probe>, Arc<Mutex<Vec<OverlayMouse>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    (
        Box::new(Probe {
            events: Arc::clone(&events),
            outcome,
            rows,
        }),
        events,
    )
}

/// Enough transcript that the document can scroll, so a wheel notch that reaches it is visible in
/// the top row of the screen.
fn fill_transcript(app: &mut App<TestBackend>) {
    for i in 0..60 {
        app.transcript_mut().push_status(format!("history-{i:02}"));
    }
    app.draw().unwrap();
}

// =============================================================================================
// TUI-124: the pointer
// =============================================================================================

/// A press on a row of the overlay's list selects it; the click that completes the gesture
/// activates it, and the overlay closes and releases the task blocked on it.
#[test]
fn a_press_selects_a_row_and_a_click_activates_it() {
    let mut app = fullscreen_app();
    let (overlay, result) = chooser();
    let mut released = open(&mut app, overlay);
    assert_eq!(highlighted(&mut app), "option-alpha");

    let row = row_of(&mut app, "option-delta");
    let column = painted(&mut app).x + 4;

    let action = app.handle_input(&down(column, row));
    assert_eq!(action, AppAction::Redraw);
    app.draw().unwrap();
    assert_eq!(
        highlighted(&mut app),
        "option-delta",
        "the press selected the row under the pointer"
    );
    assert!(app.overlay_open(), "a press alone does not activate");
    assert_eq!(result.lock().unwrap().as_deref(), None);

    let action = app.handle_input(&up(column, row));
    assert_eq!(action, AppAction::Redraw);
    assert!(
        !app.overlay_open(),
        "the click activated the row and closed"
    );
    assert_eq!(result.lock().unwrap().as_deref(), Some("option-delta"));
    assert!(
        released.try_recv().is_ok(),
        "closing releases the extension task blocked on the overlay"
    );
}

/// The wheel moves the list one row per notch and stops at the ends.
#[test]
fn the_wheel_scrolls_the_list_and_clamps() {
    let mut app = fullscreen_app();
    let (overlay, _result) = chooser();
    let _released = open(&mut app, overlay);
    let row = row_of(&mut app, "option-bravo");
    let column = painted(&mut app).x + 4;

    app.handle_input(&notch_down(column, row));
    app.draw().unwrap();
    assert_eq!(highlighted(&mut app), "option-bravo");
    for _ in 0..8 {
        app.handle_input(&notch_down(column, row));
    }
    app.draw().unwrap();
    assert_eq!(
        highlighted(&mut app),
        "option-delta",
        "clamped at the last row, not wrapped"
    );
    for _ in 0..8 {
        app.handle_input(&notch_up(column, row));
    }
    app.draw().unwrap();
    assert_eq!(
        highlighted(&mut app),
        "option-alpha",
        "clamped at the first"
    );
    assert!(app.overlay_open());
}

/// A report outside the box the overlay painted is a miss for the overlay: it does not see it and
/// the dock does. The document does not take the wheel from under a modal (pi's
/// `shouldDeferViewportInputToOverlay`, `tui-alt-screen.ts:709`).
#[test]
fn a_report_outside_the_overlay_is_a_miss_and_the_wheel_waits_for_the_modal() {
    let mut app = fullscreen_app();
    fill_transcript(&mut app);
    let before = screen(&mut app)[0].clone();

    // Control: with no overlay the notch scrolls the document.
    app.handle_input(&notch_up(0, 0));
    app.draw().unwrap();
    assert_ne!(
        screen(&mut app)[0],
        before,
        "without a modal the document scrolls"
    );

    let (overlay, _result) = chooser();
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    assert!(rect.y > 0, "the overlay does not cover the top row");

    let ev = |kind| MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        app.handle_overlay_pointer(&ev(MouseEventKind::ScrollUp)),
        OverlayRoute::Miss
    );
    assert_eq!(
        app.handle_overlay_pointer(&ev(MouseEventKind::Down(MouseButton::Left))),
        OverlayRoute::Miss
    );

    // The whole chain: a notch beside the modal scrolls nothing, the list keeps its highlight.
    let held = screen(&mut app)[0].clone();
    app.handle_input(&notch_up(0, 0));
    app.handle_input(&notch_down(0, 0));
    app.draw().unwrap();
    assert_eq!(
        screen(&mut app)[0],
        held,
        "the document did not scroll under the modal"
    );
    assert_eq!(highlighted(&mut app), "option-alpha");
}

/// The dock is still reachable beside a modal: a press on a completion row outside the overlay's
/// box is the dock's.
#[test]
fn a_press_beside_the_overlay_still_reaches_the_dock() {
    let mut app = fullscreen_app();
    for c in "/m".chars() {
        app.handle_input(&crate::tests::harness::key(
            ratatui::crossterm::event::KeyCode::Char(c),
        ));
    }
    app.draw().unwrap();
    let popup = app.state_mut().regions.popup;
    assert!(popup.height > 0, "`/m` opens a popup");
    let (overlay, _events) = probe(4, OverlayMouseOutcome::Unhandled);
    let _released = open(&mut app, overlay);
    let at = Position::new(popup.x + 3, popup.y);
    assert!(
        !painted(&mut app).contains(at),
        "the box does not cover the popup"
    );
    app.handle_input(&down(at.x, at.y));
    assert!(
        app.state().pointer.press_in_flight(),
        "the dock took the press"
    );
}

/// Offered to the dock, a press on a completion row is a gesture in flight; under an overlay that
/// covers the popup the press goes to the overlay and the dock never sees it — nor does the wheel
/// reach the document, whatever the overlay answers.
#[test]
fn a_report_inside_the_overlay_never_reaches_the_dock_or_the_document() {
    let mut app = fullscreen_app();
    fill_transcript(&mut app);
    // A completion popup under the editor: a press on its rows is a component's, so it leaves a
    // gesture behind (a press on the editor body would start a text selection instead).
    for c in "/m".chars() {
        app.handle_input(&crate::tests::harness::key(
            ratatui::crossterm::event::KeyCode::Char(c),
        ));
    }

    // Control: with no overlay a press on a completion row is the dock's.
    app.draw().unwrap();
    let slot = app.state_mut().regions.slot;
    let popup = app.state_mut().regions.popup;
    assert!(popup.height > 0, "`/m` opens a popup");
    let at = Position::new(popup.x + 3, popup.y);
    app.handle_input(&down(at.x, at.y));
    assert!(
        app.state().pointer.press_in_flight(),
        "without an overlay the dock takes the press (the detector works)"
    );
    // Released on another cell, so it is not a click and activates nothing.
    app.handle_input(&up(at.x + 1, at.y));
    assert!(!app.state().pointer.press_in_flight());

    // An overlay tall enough to cover the editor slot, and one that ignores the pointer.
    let (overlay, events) = probe(60, OverlayMouseOutcome::Unhandled);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    assert!(
        rect.contains(at) && rect.contains(Position::new(at.x, slot.y)),
        "the box {rect:?} covers the editor slot {slot:?} and the popup {popup:?}"
    );
    let before = screen(&mut app)[0].clone();

    app.handle_input(&down(at.x, at.y));
    assert!(
        !app.state().pointer.press_in_flight(),
        "the dock was never offered the press"
    );
    app.handle_input(&up(at.x, at.y));
    app.handle_input(&notch_up(at.x, at.y));
    app.handle_input(&notch_down(at.x, at.y));
    app.draw().unwrap();
    assert_eq!(
        screen(&mut app)[0],
        before,
        "an ignored wheel notch inside the overlay does not scroll the document beneath"
    );

    let seen = events.lock().unwrap().clone();
    let local_x = at.x - rect.x;
    let local_y = at.y - rect.y;
    assert_eq!(
        seen,
        vec![
            OverlayMouse::Press {
                column: local_x,
                row: local_y
            },
            OverlayMouse::Click {
                column: local_x,
                row: local_y,
                count: 1
            },
            OverlayMouse::Wheel {
                column: local_x,
                row: local_y,
                lines: -1
            },
            OverlayMouse::Wheel {
                column: local_x,
                row: local_y,
                lines: 1
            },
        ],
        "the overlay got every event, positions local to the box it painted"
    );
}

/// The hit test is the rectangle the overlay painted: every cell of it is the overlay's, the cells
/// just outside are a miss.
#[test]
fn the_hit_test_is_exactly_the_rectangle_the_overlay_painted() {
    let mut app = fullscreen_app();
    let (overlay, events) = probe(8, OverlayMouseOutcome::Redraw);
    let _released = open(&mut app, overlay);
    // The box the painter lays out for 8 rows on this screen, computed independently of what the
    // overlay reports, and checked against the cells actually painted.
    let rect = ExtensionOverlay::box_rect(
        Rect::new(0, 0, COLS, ROWS),
        8,
        cyrup_ext::host::OverlayOptions::default(),
    );
    assert_eq!(
        painted(&mut app),
        rect,
        "the recorded box is the painted box"
    );
    assert_eq!(
        screen(&mut app)[usize::from(rect.y)]
            .chars()
            .nth(usize::from(rect.x)),
        Some('A'),
        "the first cell of the box holds the first painted glyph"
    );
    let last_x = rect.x + rect.width - 1;
    let last_y = rect.y + rect.height - 1;
    let notch = |x: u16, y: u16| MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };

    for (x, y) in [
        (rect.x, rect.y),
        (last_x, last_y),
        (rect.x, last_y),
        (last_x, rect.y),
    ] {
        assert!(
            app.handle_overlay_pointer(&notch(x, y)) != OverlayRoute::Miss,
            "({x}, {y}) is inside {rect:?}"
        );
    }
    for (x, y) in [
        (rect.x - 1, rect.y),
        (last_x + 1, rect.y),
        (rect.x, rect.y - 1),
        (rect.x, last_y + 1),
    ] {
        assert!(
            app.handle_overlay_pointer(&notch(x, y)) == OverlayRoute::Miss,
            "({x}, {y}) is outside {rect:?}"
        );
    }
    let seen = events.lock().unwrap().clone();
    assert!(
        seen.contains(&OverlayMouse::Wheel {
            column: rect.width - 1,
            row: rect.height - 1,
            lines: 1
        }),
        "the far corner is local (width - 1, height - 1): {seen:?}"
    );
}

/// Overlays are hit-tested topmost first: where two overlap, the one on top takes the event.
#[test]
fn the_topmost_overlay_under_the_pointer_takes_the_event() {
    let mut app = fullscreen_app();
    let (below, below_events) = probe(8, OverlayMouseOutcome::Redraw);
    let (above, above_events) = probe(8, OverlayMouseOutcome::Redraw);
    let _a = open(&mut app, below);
    let _b = open(&mut app, above);
    let rect = painted(&mut app);

    app.handle_input(&notch_down(rect.x + 2, rect.y + 2));

    assert_eq!(above_events.lock().unwrap().len(), 1);
    assert!(
        below_events.lock().unwrap().is_empty(),
        "the overlay beneath was not offered the event"
    );
}

/// Two clicks on one cell are a double click; a press released elsewhere is not a click at all.
#[test]
fn consecutive_clicks_are_counted_and_a_drag_is_not_a_click() {
    let mut app = fullscreen_app();
    let (overlay, events) = probe(8, OverlayMouseOutcome::Redraw);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    let (x, y) = (rect.x + 3, rect.y + 2);

    click(&mut app, x, y);
    click(&mut app, x, y);
    // A press dragged off its cell and released elsewhere.
    app.handle_input(&down(x, y));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), x + 2, y));
    app.handle_input(&up(x + 2, y));

    let clicks: Vec<u8> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            OverlayMouse::Click { count, .. } => Some(*count),
            _ => None,
        })
        .collect();
    assert_eq!(clicks, vec![1, 2], "the drag produced no click");
}

/// Any other report over the modal (a right press, hover) is not the overlay's to handle: it goes
/// no further than the text selection, which ignores it, and outside the box it is a miss. The
/// overlay is never offered it.
#[test]
fn a_right_click_inside_the_overlay_goes_no_further_than_the_selection() {
    let mut app = fullscreen_app();
    let (overlay, events) = probe(8, OverlayMouseOutcome::Redraw);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);

    let inside = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 1,
        row: rect.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(app.handle_overlay_pointer(&inside), OverlayRoute::Over);
    assert_eq!(
        app.handle_input(&InputEvent::Mouse(inside)),
        AppAction::None,
        "nothing beneath the modal answers it"
    );
    let outside = MouseEvent {
        column: 0,
        row: 0,
        ..inside
    };
    assert_eq!(app.handle_overlay_pointer(&outside), OverlayRoute::Miss);
    assert!(events.lock().unwrap().is_empty());
}

/// An overlay that leaves the pointer alone (the trait default) still shields what is beneath it:
/// its press is not consumed, but it starts a text selection on the painted rows rather than
/// reaching the dock or the document.
#[test]
fn an_overlay_that_leaves_the_pointer_alone_hands_the_press_to_the_selection() {
    let mut app = fullscreen_app();
    let (overlay, _events) = probe(8, OverlayMouseOutcome::Unhandled);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    let ev = down(rect.x + 1, rect.y + 1);
    assert_eq!(
        app.handle_overlay_pointer(&MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + 1,
            row: rect.y + 1,
            modifiers: KeyModifiers::NONE,
        }),
        OverlayRoute::Over
    );
    assert_eq!(
        app.handle_input(&ev),
        AppAction::Redraw,
        "the selection began"
    );
    assert!(!app.state().pointer.press_in_flight(), "not the dock's");
}

// =============================================================================================
// A press the overlay does not handle selects the text under it
// =============================================================================================

/// The text of the highlighted selection, or `None`.
fn selected(app: &mut App<TestBackend>) -> Option<String> {
    app.altscreen_for_test()
        .expect("fullscreen is live")
        .selection_text()
}

fn reversed_cells(app: &mut App<TestBackend>, y: u16) -> Vec<u16> {
    use ratatui::style::Modifier;
    let alt = app.altscreen_for_test().expect("fullscreen is live");
    let buf = alt.backend_for_test().buffer().clone();
    (0..COLS)
        .filter(|&x| buf[(x, y)].modifier.contains(Modifier::REVERSED))
        .collect()
}

fn column_of(app: &mut App<TestBackend>, needle: &str) -> u16 {
    let row = usize::from(row_of(app, needle));
    let text = screen(app)[row].clone();
    let (before, _) = text.split_once(needle).unwrap();
    u16::try_from(before.chars().count()).unwrap()
}

fn copied(action: AppAction) -> String {
    match action {
        AppAction::CopySelection(text) => text,
        other => panic!("expected a copy, got {other:?}"),
    }
}

/// pi's `overlay.result ?? (overlay.hit ? undefined : layout)` leaves a hit the component did not
/// handle to `handleSelectionMouseEvent`: a drag across the text of a modal selects it and the
/// release copies it. The rows are the overlay's, painted over the transcript, so the copy is not
/// the document's text beneath.
#[test]
fn a_drag_across_unhandled_overlay_text_copies_it() {
    let mut app = fullscreen_app();
    fill_transcript(&mut app);
    let (overlay, events) = probe(8, OverlayMouseOutcome::Unhandled);
    let _released = open(&mut app, overlay);
    let y = row_of(&mut app, "ACCENT-ROW");
    let x = column_of(&mut app, "ACCENT-ROW");

    app.handle_input(&down(x, y));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), x + 9, y));
    let action = app.handle_input(&up(x + 9, y));
    app.draw().unwrap();

    assert_eq!(copied(action), "ACCENT-ROW");
    assert_eq!(selected(&mut app).as_deref(), Some("ACCENT-ROW"));
    assert_eq!(
        reversed_cells(&mut app, y),
        (x..=x + 9).collect::<Vec<_>>(),
        "the highlight covers exactly the selected cells of the overlay"
    );
    assert!(app.overlay_open(), "selecting does not dismiss the modal");
    let seen = events.lock().unwrap().clone();
    assert!(
        matches!(seen.as_slice(), [OverlayMouse::Press { .. }]),
        "the overlay was offered the press, and a drag is no click: {seen:?}"
    );
}

/// The same on a real chooser: its title is selectable, its option rows are not.
#[test]
fn a_chooser_title_is_selectable_text() {
    let mut app = fullscreen_app();
    let (overlay, result) = chooser();
    let _released = open(&mut app, overlay);
    let y = row_of(&mut app, "Pick one");
    let x = column_of(&mut app, "Pick one");

    app.handle_input(&down(x, y));
    app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), x + 7, y));
    let action = app.handle_input(&up(x + 7, y));
    assert_eq!(copied(action), "Pick one");
    assert!(app.overlay_open());
    assert_eq!(result.lock().unwrap().as_deref(), None);
}

/// A press the overlay handles stays the overlay's: the row is selected, no text selection starts,
/// one already made is dropped, and the drag and release do not copy anything.
#[test]
fn a_press_the_overlay_handles_does_not_start_a_selection() {
    let mut app = fullscreen_app();
    let (overlay, result) = chooser();
    let _released = open(&mut app, overlay);

    // A selection on the title, made first.
    let title_y = row_of(&mut app, "Pick one");
    let title_x = column_of(&mut app, "Pick one");
    app.handle_input(&down(title_x, title_y));
    app.handle_input(&mouse(
        MouseEventKind::Drag(MouseButton::Left),
        title_x + 3,
        title_y,
    ));
    app.handle_input(&up(title_x + 3, title_y));
    assert_eq!(selected(&mut app).as_deref(), Some("Pick"));

    let y = row_of(&mut app, "option-charlie");
    let x = painted(&mut app).x + 4;
    let action = app.handle_input(&down(x, y));
    assert_eq!(action, AppAction::Redraw, "the row took the press");
    assert_eq!(selected(&mut app), None, "the handled press dropped it");
    app.draw().unwrap();
    assert_eq!(highlighted(&mut app), "option-charlie");

    // Dragged off the row and released: no copy, no click, no selection.
    let action = app.handle_input(&mouse(MouseEventKind::Drag(MouseButton::Left), x + 5, y));
    assert_eq!(action, AppAction::None);
    let action = app.handle_input(&up(x + 5, y));
    assert_eq!(action, AppAction::None);
    assert_eq!(selected(&mut app), None);
    assert!(app.overlay_open());
    assert_eq!(result.lock().unwrap().as_deref(), None);
}

/// A press the overlay left alone whose release is a clean click is offered to the overlay as a
/// click (pi `dispatchMouseToOverlay(clickEvent)` on release), and when the overlay takes it the
/// selection is cleared and nothing is copied.
#[test]
fn an_unhandled_press_released_in_place_is_offered_to_the_overlay_as_a_click() {
    /// Leaves the press to the selection, takes the click.
    struct ClickOnly {
        clicks: Arc<Mutex<Vec<u8>>>,
    }
    impl InteractiveOverlay for ClickOnly {
        fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
            vec![OverlayLine::new(vec![OverlaySpan {
                text: "CLICK-ME".to_string(),
                ..OverlaySpan::default()
            }])]
        }
        fn handle_key(&mut self, _key: OverlayKey) -> ExtOverlayOutcome {
            ExtOverlayOutcome::Ignored
        }
        fn handle_mouse(&mut self, event: OverlayMouse) -> OverlayMouseOutcome {
            match event {
                OverlayMouse::Click { count, .. } => {
                    self.clicks.lock().unwrap().push(count);
                    OverlayMouseOutcome::Close
                }
                _ => OverlayMouseOutcome::Unhandled,
            }
        }
    }
    let mut app = fullscreen_app();
    let clicks = Arc::new(Mutex::new(Vec::new()));
    let mut released = open(
        &mut app,
        Box::new(ClickOnly {
            clicks: Arc::clone(&clicks),
        }),
    );
    let y = row_of(&mut app, "CLICK-ME");
    let x = column_of(&mut app, "CLICK-ME");

    let action = app.handle_input(&down(x + 2, y));
    assert_eq!(action, AppAction::Redraw, "the press began a selection");
    let action = app.handle_input(&up(x + 2, y));
    assert_eq!(action, AppAction::Redraw, "the click closed the overlay");
    assert_eq!(*clicks.lock().unwrap(), vec![1]);
    assert!(!app.overlay_open());
    assert!(released.try_recv().is_ok());
    assert_eq!(selected(&mut app), None, "the click cleared the selection");
}

/// A click the overlay does not take copies nothing, and does not fall through to what is under
/// the modal either (pi: `overlay.hit` suppresses `dispatchMouseToLayout`).
#[test]
fn a_click_the_overlay_leaves_alone_does_not_reach_the_dock() {
    let mut app = fullscreen_app();
    for c in "/m".chars() {
        app.handle_input(&crate::tests::harness::key(
            ratatui::crossterm::event::KeyCode::Char(c),
        ));
    }
    app.draw().unwrap();
    let popup = app.state_mut().regions.popup;
    assert!(popup.height > 0);
    let (overlay, events) = probe(60, OverlayMouseOutcome::Unhandled);
    let _released = open(&mut app, overlay);
    let at = Position::new(popup.x + 3, popup.y);
    assert!(painted(&mut app).contains(at), "the box covers the popup");
    let before = app.state().editor.text();

    let action = app.handle_input(&down(at.x, at.y));
    assert_eq!(action, AppAction::Redraw);
    let action = app.handle_input(&up(at.x, at.y));
    assert!(!matches!(action, AppAction::CopySelection(_)), "{action:?}");
    assert_eq!(
        app.state().editor.text(),
        before,
        "the completion row under the modal was not activated"
    );
    let seen = events.lock().unwrap().clone();
    assert!(
        matches!(
            seen.as_slice(),
            [
                OverlayMouse::Press { .. },
                OverlayMouse::Click { count: 1, .. }
            ]
        ),
        "{seen:?}"
    );
}

/// pi's `clearTextSelection()` on a press a component handled leaves the multi-click ladder
/// running (`tui-alt-screen.ts:893-902`): a click on a word, a press the overlay handles, and a
/// second click on the word inside the window are still a double click, which selects the word.
#[test]
fn a_press_the_overlay_handled_does_not_reset_the_double_click_ladder() {
    let mut app = fullscreen_app();
    for c in "alpha beta gamma".chars() {
        app.handle_input(&crate::tests::harness::key(
            ratatui::crossterm::event::KeyCode::Char(c),
        ));
    }
    app.draw().unwrap();
    let beta = (
        column_of(&mut app, "alpha beta gamma") + 7,
        row_of(&mut app, "alpha beta gamma"),
    );
    let (overlay, _events) = probe(4, OverlayMouseOutcome::Handled);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    assert!(!rect.contains(Position::new(beta.0, beta.1)));

    // The first click of the ladder, then a handled press elsewhere, then the second click.
    click(&mut app, beta.0, beta.1);
    app.handle_input(&down(rect.x + 2, rect.y + 1));
    assert_eq!(
        selected(&mut app),
        None,
        "the handled press cleared the selection"
    );
    app.handle_input(&up(rect.x + 2, rect.y + 1));
    app.handle_input(&down(beta.0, beta.1));
    let action = app.handle_input(&up(beta.0, beta.1));
    assert_eq!(
        copied(action),
        "beta",
        "the second click was a double click"
    );
}

// =============================================================================================
// TUI-125: the theme
// =============================================================================================

/// Theme-role colours are resolved against the ACTIVE theme at paint time: the same overlay,
/// repainted after the theme changed, is painted in the new theme's colours, and a literal colour
/// is left alone.
#[test]
fn the_overlay_is_painted_in_the_active_theme_and_follows_a_switch() {
    let mut app = fullscreen_app();
    let (overlay, _events) = probe(6, OverlayMouseOutcome::Unhandled);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    let accent_row = rect.y + u16::try_from(row_in_box(&mut app, "ACCENT-ROW")).unwrap();
    let dim_row = rect.y + u16::try_from(row_in_box(&mut app, "DIM-ROW")).unwrap();
    let literal_row = rect.y + u16::try_from(row_in_box(&mut app, "LITERAL-ROW")).unwrap();

    let dark = app.state().theme.clone();
    assert_eq!(
        cell_fg(&mut app, rect.x, accent_row),
        dark.accent_style().fg.unwrap(),
        "`accent` is the dark theme's accent"
    );
    assert_eq!(
        cell_fg(&mut app, rect.x, dim_row),
        dark.dim_style().fg.unwrap()
    );
    assert_eq!(cell_fg(&mut app, rect.x, literal_row), Color::Red);
    let dark_accent = cell_fg(&mut app, rect.x, accent_row);
    let dark_dim = cell_fg(&mut app, rect.x, dim_row);

    // Switch to the light theme: the same overlay changes colour on the next frame.
    app.set_theme(UiTheme::light());
    app.draw().unwrap();
    let light = app.state().theme.clone();
    let light_accent = cell_fg(&mut app, rect.x, accent_row);
    let light_dim = cell_fg(&mut app, rect.x, dim_row);
    assert_eq!(light_accent, light.accent_style().fg.unwrap());
    assert_eq!(light_dim, light.dim_style().fg.unwrap());
    assert_ne!(
        light_accent, dark_accent,
        "the accent changed with the theme"
    );
    assert_ne!(light_dim, dark_dim, "the dim role changed with the theme");
    assert_eq!(
        cell_fg(&mut app, rect.x, literal_row),
        Color::Red,
        "a literal colour is not a theme role and is left alone"
    );

    // …and a user's own theme is honoured, not just the two built-ins.
    let mut custom = UiTheme::dark();
    custom.accent = Some(Color::Rgb(1, 2, 3));
    app.set_theme(custom);
    app.draw().unwrap();
    let custom = app.state().theme.clone();
    assert_eq!(
        cell_fg(&mut app, rect.x, accent_row),
        custom.accent_style().fg.unwrap()
    );
    assert_ne!(cell_fg(&mut app, rect.x, accent_row), dark_accent);
}

/// The row of `needle` inside the overlay's own box.
fn row_in_box(app: &mut App<TestBackend>, needle: &str) -> usize {
    let rect = painted(app);
    usize::from(row_of(app, needle) - rect.y)
}
