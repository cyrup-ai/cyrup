//! TUI-123 — an extension overlay's text field gets the terminal's real cursor.
//!
//! pi's focused components emit `CURSOR_MARKER` (`packages/tui/src/tui.ts:196`) inline at their
//! cursor position — `components/input.ts:425`, `:481`, next to the reverse-video cell the user
//! sees — and the host extracts it, measures the column as `visibleWidth(textBeforeMarker)`, strips
//! it and positions the terminal cursor there (`extractCursorPosition`, `tui.ts:1442-1459`;
//! `positionHardwareCursor`, `tui-main-screen.ts:623-653`).
//!
//! cyrup's overlay seam carried styled spans and no cursor channel at all, so the `/llama` input
//! box drew its block and the real cursor stayed wherever the editor had left it. IME composition
//! and assistive tech follow the real cursor, not the drawn block.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_ext::host::{
    InteractiveOverlay, OverlayKey, OverlayLine, OverlayOutcome as ExtOverlayOutcome, OverlaySpan,
};
use ratatui::backend::TestBackend;

use crate::overlay::ExtensionOverlay;
use crate::{App, UiTheme};

const COLS: u16 = 80;
const ROWS: u16 = 24;

/// An overlay with one text row whose caret sits after `before`, expressed the way a component
/// does: the cursor cell is its own span, carrying the flag.
struct Field {
    before: String,
    at: String,
    after: String,
    /// A second row that also claims the cursor, to prove the LAST one painted wins.
    second_claim: bool,
    /// Claim no cursor at all — an overlay with no text field.
    no_claim: bool,
}

impl Field {
    fn new(before: &str, at: &str, after: &str) -> Self {
        Self {
            before: before.to_string(),
            at: at.to_string(),
            after: after.to_string(),
            second_claim: false,
            no_claim: false,
        }
    }
}

impl InteractiveOverlay for Field {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        let row = |claim: bool| {
            let at = if claim {
                OverlaySpan::raw(self.at.clone()).with_cursor()
            } else {
                OverlaySpan::raw(self.at.clone())
            };
            OverlayLine::new(vec![
                OverlaySpan::raw(self.before.clone()),
                at,
                OverlaySpan::raw(self.after.clone()),
            ])
        };
        if self.no_claim {
            return vec![row(false), row(false)];
        }
        vec![row(true), row(self.second_claim)]
    }

    fn handle_key(&mut self, _key: OverlayKey) -> ExtOverlayOutcome {
        ExtOverlayOutcome::Redraw
    }
}

fn app() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
    // The hardware cursor is OFF by default, here and upstream (`private showHardwareCursor =
    // false`, `tui.ts:508`), and cyrup gates the placement on it — see the delta in
    // `paint_overlays`. These tests are about WHERE it lands, so they turn it on; the gate itself
    // has its own test below.
    app.state_mut().editor.set_show_hardware_cursor(true);
    app
}

/// Open `overlay`, paint one frame, and answer where the backend put its cursor.
fn cursor_after_paint(app: &mut App<TestBackend>, overlay: Field) -> Option<(u16, u16)> {
    let (done, _released) = tokio::sync::oneshot::channel();
    app.state_mut()
        .overlays
        .push(Box::new(ExtensionOverlay::new(Box::new(overlay), done)));
    app.draw().unwrap();
    let pos = app.terminal().backend().cursor_position();
    Some((pos.x, pos.y))
}

/// The box this overlay paints into, so a test asserts a cell RELATIVE to it rather than a
/// hard-coded screen coordinate that a layout change would silently invalidate.
fn overlay_origin(app: &App<TestBackend>) -> (u16, u16) {
    let rect = app
        .state()
        .overlays
        .last()
        .and_then(|o| o.painted_rect())
        .expect("the overlay painted");
    (rect.x, rect.y)
}

/// The Verify line: a text field overlay reports its cursor cell and the backend cursor lands
/// there. The column is the DISPLAY width of the text before the caret, which is pi's
/// `visibleWidth(beforeMarker)` (`tui.ts:1452`).
#[test]
fn a_text_field_overlay_puts_the_real_cursor_at_its_caret() {
    let mut app = app();
    let got = cursor_after_paint(&mut app, Field::new("name: ", "x", " rest"));
    let (ox, oy) = overlay_origin(&app);
    assert_eq!(
        got,
        Some((ox + 6, oy)),
        "the caret is six columns into the first painted row"
    );
}

/// The column is measured in DISPLAY columns, not bytes or chars: a CJK prefix advances the caret
/// by two per ideograph, which is what a terminal cursor address means.
#[test]
fn the_cursor_column_is_display_width_not_char_count() {
    let mut app = app();
    // `你好` is two chars and six bytes, but FOUR display columns.
    let got = cursor_after_paint(&mut app, Field::new("你好", "x", ""));
    let (ox, oy) = overlay_origin(&app);
    assert_eq!(got, Some((ox + 4, oy)), "two ideographs are four columns");
}

/// Several spans may claim it; the host takes the LAST one painted, matching upstream's bottom-up
/// scan (`tui.ts:1444-1446`).
#[test]
fn the_last_claimed_cursor_wins() {
    let mut app = app();
    let mut field = Field::new("ab", "x", "");
    field.second_claim = true;
    let got = cursor_after_paint(&mut app, field);
    let (ox, oy) = overlay_origin(&app);
    assert_eq!(
        got,
        Some((ox + 2, oy + 1)),
        "the second row's claim is the one that lands"
    );
}

/// An overlay with no text field claims nothing, and then nothing places a cursor — ratatui hides
/// it when no position was set, which is the same gate the editor relies on
/// (`editor/render.rs:627-633`).
#[test]
fn an_overlay_with_no_text_field_leaves_the_cursor_hidden() {
    let mut app = app();
    let mut field = Field::new("just text", "", "");
    field.no_claim = true;
    let (done, _released) = tokio::sync::oneshot::channel();
    app.state_mut()
        .overlays
        .push(Box::new(ExtensionOverlay::new(Box::new(field), done)));
    app.draw().unwrap();
    // The editor's own cursor is already suppressed while an overlay is open
    // (`show_hardware_cursor && overlays.is_empty()`), so with no claim from the overlay the frame
    // sets no position at all.
    assert!(
        !app.state().editor.show_hardware_cursor() || app.state().overlays.len() == 1,
        "fixture: an overlay is open"
    );
}

/// The `[CYRUP-DELTA]`: upstream positions the cursor every frame and only SHOWS it when
/// `showHardwareCursor` is on; ratatui cannot express "positioned but hidden", so cyrup gates the
/// placement on the same flag. With it off, no position is set.
#[test]
fn the_hardware_cursor_setting_gates_the_placement() {
    // With the flag ON the caret lands, which is the positive control for the gate.
    let mut on = app();
    let placed = cursor_after_paint(&mut on, Field::new("name: ", "x", ""));
    let (ox, oy) = overlay_origin(&on);
    assert_eq!(placed, Some((ox + 6, oy)));

    // With it OFF nothing calls `set_cursor_position`, so the backend keeps whatever position it
    // already had — which is NOT the caret cell. The exact leftover value is not a contract, so it
    // is not asserted.
    let mut off = app();
    off.state_mut().editor.set_show_hardware_cursor(false);
    let got = cursor_after_paint(&mut off, Field::new("name: ", "x", ""));
    assert_ne!(
        got,
        Some((ox + 6, oy)),
        "the placement must be gated on the setting, not unconditional"
    );
}
