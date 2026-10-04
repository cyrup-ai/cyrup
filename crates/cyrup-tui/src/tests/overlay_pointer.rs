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
    OverlayOutcome as ExtOverlayOutcome, OverlaySpan, ThemeRole,
};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

use crate::overlay::ExtensionOverlay;
use crate::{App, AppAction, InputEvent, UiTheme};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn fullscreen_app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
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
    outcome: ExtOverlayOutcome,
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
    fn handle_mouse(&mut self, event: OverlayMouse) -> ExtOverlayOutcome {
        self.events.lock().unwrap().push(event);
        self.outcome
    }
}

fn probe(rows: usize, outcome: ExtOverlayOutcome) -> (Box<Probe>, Arc<Mutex<Vec<OverlayMouse>>>) {
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

/// A report outside the box the overlay painted is a miss: the overlay does not see it and it
/// carries on to the document, which a wheel notch scrolls.
#[test]
fn a_report_outside_the_overlay_falls_through_to_the_document() {
    let mut app = fullscreen_app();
    fill_transcript(&mut app);
    let (overlay, _result) = chooser();
    let _released = open(&mut app, overlay);
    let before = screen(&mut app)[0].clone();
    let rect = painted(&mut app);
    assert!(rect.y > 0, "the overlay does not cover the top row");

    // The overlay answers "miss" for a wheel notch and for a press at the top-left corner.
    let ev = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(app.handle_overlay_pointer(&ev), None);
    let ev = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(app.handle_overlay_pointer(&ev), None);

    // …and the whole chain: the notch scrolled the document under the overlay, the list kept its
    // highlight.
    app.handle_input(&notch_up(0, 0));
    app.draw().unwrap();
    assert_ne!(
        screen(&mut app)[0],
        before,
        "the document scrolled: {:?}",
        screen(&mut app)[0]
    );
    assert_eq!(highlighted(&mut app), "option-alpha");
}

/// Offered to the dock, a press on the editor slot is a gesture in flight; under an overlay that
/// covers the slot the press goes to the overlay and the dock never sees it — nor does the wheel
/// reach the document, whatever the overlay answers.
#[test]
fn a_report_inside_the_overlay_never_reaches_the_dock_or_the_document() {
    let mut app = fullscreen_app();
    fill_transcript(&mut app);

    // Control: with no overlay a press on the editor slot is the dock's.
    app.draw().unwrap();
    let slot = app.state_mut().regions.slot;
    let at = Position::new(slot.x + 2, slot.y);
    app.handle_input(&down(at.x, at.y));
    assert!(
        app.state().pointer.press_in_flight(),
        "without an overlay the dock takes the press (the detector works)"
    );
    app.handle_input(&up(at.x, at.y));
    assert!(!app.state().pointer.press_in_flight());

    // An overlay tall enough to cover the editor slot, and one that ignores the pointer.
    let (overlay, events) = probe(60, ExtOverlayOutcome::Ignored);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    assert!(
        rect.contains(at) && rect.contains(Position::new(at.x, at.y + slot.height - 1)),
        "the box {rect:?} covers the editor slot {slot:?}"
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
    let (overlay, events) = probe(8, ExtOverlayOutcome::Redraw);
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
            app.handle_overlay_pointer(&notch(x, y)).is_some(),
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
            app.handle_overlay_pointer(&notch(x, y)).is_none(),
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
    let (below, below_events) = probe(8, ExtOverlayOutcome::Redraw);
    let (above, above_events) = probe(8, ExtOverlayOutcome::Redraw);
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
    let (overlay, events) = probe(8, ExtOverlayOutcome::Redraw);
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

/// Any other report over the modal (a right-click paste, hover) is still the modal's: it is not
/// offered to the editor or the document under it, and outside the box it is a miss.
#[test]
fn a_right_click_inside_the_overlay_is_swallowed() {
    let mut app = fullscreen_app();
    let (overlay, events) = probe(8, ExtOverlayOutcome::Redraw);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);

    let inside = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Right),
        column: rect.x + 1,
        row: rect.y + 1,
        modifiers: KeyModifiers::NONE,
    };
    assert_eq!(
        app.handle_overlay_pointer(&inside),
        Some(AppAction::None),
        "consumed"
    );
    let outside = MouseEvent {
        column: 0,
        row: 0,
        ..inside
    };
    assert_eq!(app.handle_overlay_pointer(&outside), None, "a miss");
    assert!(events.lock().unwrap().is_empty());
}

/// An overlay with nothing to point at (the trait default) still shields what is beneath it.
#[test]
fn an_overlay_that_ignores_the_pointer_still_takes_the_event() {
    let mut app = fullscreen_app();
    let (overlay, _events) = probe(8, ExtOverlayOutcome::Ignored);
    let _released = open(&mut app, overlay);
    let rect = painted(&mut app);
    let ev = down(rect.x + 1, rect.y + 1);
    assert_eq!(
        app.handle_input(&ev),
        AppAction::None,
        "taken, nothing to repaint"
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
    let (overlay, _events) = probe(6, ExtOverlayOutcome::Ignored);
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
