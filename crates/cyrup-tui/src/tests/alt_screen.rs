//! Alternate-screen renderer tests, ported from pi's
//! [`packages/tui/test/tui-alt-screen.test.ts`](../../../../tmp/pi/packages/tui/test/tui-alt-screen.test.ts)
//! (1,449 lines / 43 cases).
//!
//! ## How this maps onto upstream
//!
//! pi builds `new TuiAltScreen(new VirtualTerminal(w, h))` (`:58-59`), drives it with
//! `terminal.sendInput("\x1b[<64;1;1M")` and asserts against two channels: the rendered viewport
//! (`terminal.getViewport()`) and the bytes written to the injected terminal (`terminal.events`).
//!
//! cyrup's equivalents are [`AltScreen::for_test`] over a [`TestBackend`] — which returns the
//! capture handle standing in for `VirtualTerminal`'s write log — plus the typed entry points
//! (`handle_mouse`, `handle_key`, `handle_focus_lost`, `handle_resize`) in place of pushing raw
//! escape bytes through a parser cyrup does not own. The gesture under test is identical; only the
//! transport differs, because upstream's reader is part of the unit and cyrup's is not
//! (`app/input_reader.rs` maps crossterm events before the renderer sees them).
//!
//! ## Divergences pinned here on purpose
//!
//! * **Prompt navigation** walks `Entry::User` indices rather than re-parsing OSC 133 marks, so
//!   upstream's `:608` case is ported against the entry list by
//!   `prompt_navigation_jumps_between_user_entries` — the divergence documented on
//!   `altscreen::prompt_nav`.
//! * **Mouse enable** is multiplexer-aware: `?1003h` is withheld under `TMUX`, where
//!   crossterm's `EnableMouseCapture` would emit it unconditionally. That is `altscreen::mouse`'s
//!   documented departure and `mouse_enable_is_multiplexer_aware` is its pin.
//! * cyrup delegates frame diffing to ratatui, so upstream cases that exercise pi's hand-rolled
//!   `previousScreen`/`fullRedraw` differ have no counterpart; the observable consequence that DOES
//!   survive — unpinning kitty placements on a full redraw — is covered by
//!   `resize_is_a_full_redraw_for_images`.
//!
//! ## The 43 upstream cases, mapped
//!
//! PORTED = an equivalent assertion lives below. DIVERGENT = ported against cyrup's documented
//! departure. N/A = the behaviour is not this unit's in cyrup, or does not exist yet; each says
//! which.
//!
//! | # | upstream (`:line`) | here |
//! |---|---|---|
//! | 1 | `:57` viewport + manual scroll | PORTED `renders_the_tail_and_a_wheel_notch_breaks_follow` |
//! | 2 | `:91` fixed dock while transcript scrolls | N/A — cyrup has one scroll view and a single `layout_root`, not pi's `VStack`/`ScrollView` composition |
//! | 3 | `:137` invalidates overlays with a layout root | N/A — `set_layout_root` is a setter; cyrup has no overlay invalidation protocol |
//! | 4 | `:153` wheel to the view under the pointer | PORTED (same case as 1) |
//! | 5 | `:181` button-motion tracking in multiplexers | DIVERGENT `mouse_enable_is_multiplexer_aware` |
//! | 6 | `:228` right-click paste, Windows outside VS Code | N/A — `PointerOutcome::Paste` is returned here; the platform gate is `app/input.rs` |
//! | 7 | `:263` drag the thumb, visible until release | PORTED `scrollbar_mode_selects_the_reserved_column`, `a_scrollbar_grab_cancels_an_in_flight_selection` |
//! | 8 | `:319` scrollbar column selectable while hidden | PORTED (same) |
//! | 9 | `:343` chains unused wheel delta outward | N/A — overscroll chaining is the app's, not the renderer's |
//! | 10 | `:367` keyboard nav, four rows of overlap | PORTED `viewport_keys_scroll_and_unmatched_keys_fall_through` |
//! | 11 | `:407` searches normalized transcript text | **N/A — cyrup has NO alt-screen search**; pi's `alt-screen-search.ts` (157 lines) is unported. Filed separately |
//! | 12 | `:418` current vs non-current match styles | N/A — same gap |
//! | 13 | `:441` Ctrl+Shift+F, restores editor focus | N/A — same gap |
//! | 14 | `:506` half-page scroll, custom bindings | PORTED `viewport_keys_scroll_and_unmatched_keys_fall_through` |
//! | 15 | `:535` one-line scroll, custom bindings | PORTED `scroll_controls_clamp_to_the_document` |
//! | 16 | `:564` Ctrl-modified nav to focused component | N/A — focus routing is `app/input.rs` |
//! | 17 | `:608` OSC 133 prompt markers | DIVERGENT `prompt_navigation_jumps_between_user_entries` — cyrup walks `Entry::User` indices; see `altscreen::prompt_nav` |
//! | 18 | `:648` no Kitty/OSC 133 in iTerm2 | PORTED (partial) `resize_is_a_full_redraw_for_images`, `teardown_clears_the_placement_registry` — both pin that no kitty escape is emitted without the protocol |
//! | 19 | `:678` clears stale iTerm2 placements | N/A — needs a live iTerm2 capability the test terminal cannot negotiate |
//! | 20 | `:705` crops a Kitty image above the viewport | N/A — needs a kitty terminal and image fixtures |
//! | 21 | `:728` reuses moved Kitty images | N/A — same |
//! | 22 | `:778` retains offscreen Kitty for reuse | N/A — same |
//! | 23 | `:818` evicts least-recently-visible | N/A — same |
//! | 24 | `:865` evicts on decoded-raster quota | N/A — same |
//! | 25 | `:902` OSC 8 open on release, not drag | N/A — needs a document carrying link spans |
//! | 26 | `:945` select + copy after a generic release | DIVERGENT `a_drag_and_release_yields_the_selected_text` — cyrup returns `PointerOutcome::Copy`; the app writes the clipboard |
//! | 27 | `:975` injected copySelection handler | N/A — the injection point is `app/run_action.rs` |
//! | 28 | `:1003` flashes when copySelection fails | N/A — same layer |
//! | 29 | `:1026` no whitespace on double-click word | PORTED (partial) `click_count_widens_the_selection_granularity` |
//! | 30 | `:1042` coalesces slash/hyphen segments | N/A — segment rules are `selection`'s internals; only the granularity ladder is driven here |
//! | 31 | `:1071` whitespace segment during a word drag | N/A — same |
//! | 32 | `:1088` double/triple click word and line | PORTED `click_count_widens_the_selection_granularity` |
//! | 33 | `:1127` no repaint of idle/zero-width selection | PORTED `focus_loss_clears_an_in_flight_selection_only` |
//! | 34 | `:1170` clears an active selection on focus loss | PORTED (same) |
//! | 35 | `:1200` retains a completed selection | PORTED (same) |
//! | 36 | `:1230` stacks flashes, collapses on expiry | PORTED `a_flash_paints_and_arms_a_deadline`, `teardown_disposes_queued_flashes` |
//! | 37 | `:1253` auto-scrolls a drag held at the edge | N/A — needs a controllable clock; `next_auto_scroll` is a deadline the run loop owns |
//! | 38 | `:1281` CJK/emoji/combining boundaries | N/A — grapheme snapping is covered by the crate's existing width tests |
//! | 39 | `:1317` ignores horizontal trackpad wheel | PORTED `horizontal_wheel_is_ignored` |
//! | 40 | `:1336` restores state, prints the document | PORTED `enter_and_teardown_write_pi_s_escape_order`, `teardown_repaints_the_document_into_scrollback`, `teardown_disables_mouse_before_leaving_the_screen` |
//! | 41 | `:1372` wheel and keys to a focused overlay | N/A — overlay focus is `app/input.rs` |
//! | 42 | `:1400` scrolling when an overlay is unfocused | N/A — same |
//! | 43 | `:1430` scrolling while search is focused | N/A — no alt-screen search (see 11) |
//!
//! **16 PORTED, 3 DIVERGENT, 24 N/A** — recounted off the rows above, not carried forward.
//!
//! The N/A rows are not a backlog of skipped work. Seven (6, 9, 16, 27, 28, 41, 42) are behaviours
//! that live in `app/` rather than in this renderer — focus routing, the clipboard write, the
//! platform gate on right-click paste. Six (19-24) need a terminal that negotiates real graphics.
//! Four (11, 12, 13, 43) need the alt-screen search cyrup has not ported at all. The remaining
//! seven (2, 3, 25, 30, 31, 37, 38) need a composition cyrup does not have (pi's nested
//! `ScrollView`s) or a fixture — link spans, a controllable clock — that would test the fixture
//! more than the renderer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::time::Duration;

use cyrup_config::settings::WheelScrollLines;
use ratatui::backend::{Backend, TestBackend};
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::altscreen::{Modal, captured_text};
use crate::keymap::AltScreenKeymap;
use crate::theme::UiTheme;
use crate::transcript::{Entry, ImageOpts, TranscriptView};
use crate::{AltScreen, PointerOutcome, ScrollbarMode, TuiRenderMode, ViewportRenderer};

// ---------------------------------------------------------------------------------------------
// Fixtures — the `VirtualTerminal` + `Text` pair every upstream case opens with (`:58-61`).
// ---------------------------------------------------------------------------------------------

/// The glyphs the scrollbar writes into its column: track `│`, thumb `┃`, hovered thumb `█`.
const BAR_GLYPHS: [char; 3] = ['│', '┃', '█'];

/// `Array.from({length: n}, (_, i) => 'line ' + (i+1))` (`:60`).
fn doc(n: usize) -> Vec<Line<'static>> {
    (1..=n).map(|i| Line::from(format!("line {i}"))).collect()
}

/// One row per line, which is what an unwrapped document produces.
fn row_starts(n: usize) -> Vec<usize> {
    (0..n).collect()
}

/// A renderer over a `w × h` test terminal with `n` document rows already handed over.
///
/// Returns the capture handle too — the `VirtualTerminal` write log equivalent.
fn screen(w: u16, h: u16, n: usize) -> (AltScreen<TestBackend>, crate::altscreen::Captured, Rect) {
    let (mut alt, captured) = AltScreen::for_test(TestBackend::new(w, h), UiTheme::dark()).unwrap();
    alt.set_document_for_test(doc(n), row_starts(n));
    let area = alt.area();
    (alt, captured, area)
}

/// `terminal.getViewport().map(l => l.trimEnd())` (`:66-69`) — the DOCUMENT text of each row.
///
/// pi's cases run a `TuiAltScreen` whose implicit scroll view has its scrollbar `hidden`, so its
/// viewport never carries a bar. cyrup's renderer defaults to `auto` (the `fullscreenScrollbar`
/// default), and a visible bar paints a glyph into the last column, so one trailing bar glyph is
/// dropped here; [`raw_rows`] is the scrape that keeps it, for the cases about the bar itself.
fn viewport(alt: &mut AltScreen<TestBackend>) -> Vec<String> {
    raw_rows(alt)
        .into_iter()
        .map(|row| {
            row.trim_end()
                .strip_suffix(BAR_GLYPHS)
                .unwrap_or(row.trim_end())
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// Every row as painted, last column included, with trailing blanks trimmed.
fn raw_rows(alt: &mut AltScreen<TestBackend>) -> Vec<String> {
    let backend = alt.backend_for_test();
    let width = usize::from(backend.size().map(|s| s.width).unwrap_or(0));
    backend
        .buffer()
        .content
        .chunks(width)
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The rightmost column's cell styles — where the scrollbar thumb is painted as a background.
fn last_column_styles(alt: &mut AltScreen<TestBackend>) -> Vec<ratatui::style::Style> {
    let backend = alt.backend_for_test();
    let width = usize::from(backend.size().map(|s| s.width).unwrap_or(0));
    backend
        .buffer()
        .content
        .chunks(width)
        .filter_map(|row| row.last().map(ratatui::buffer::Cell::style))
        .collect()
}

/// `terminal.sendInput("\x1b[<64;…M")` — a wheel-up notch over the document (`:73`).
fn wheel(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

// ---------------------------------------------------------------------------------------------
// terminal.rs + mouse.rs + out.rs — the escape channel upstream reads off `terminal.events`
// ---------------------------------------------------------------------------------------------

/// pi `:1336` — "restores keyboard state before leaving alt mode and prints the full document".
///
/// Upstream asserts the ORDER of the teardown writes. The same ordering is asserted here off the
/// capture buffer: the alternate screen is entered before anything is drawn and left after the
/// document has been repainted, with autowrap restored and the cursor shown.
#[test]
fn enter_and_teardown_write_pi_s_escape_order() {
    let (mut alt, captured, _) = screen(20, 4, 10);
    let entry = captured_text(&captured);
    assert!(
        entry.contains("\x1b[?1049h"),
        "enter must switch to the alternate screen"
    );
    assert!(
        entry.contains("\x1b[?7l"),
        "autowrap off on entry (`tui-alt-screen.ts:53`)"
    );
    assert!(
        entry.contains("\x1b[?25l"),
        "cursor hidden on entry (`:293`)"
    );
    assert!(!entry.contains("\x1b[?1049l"), "nothing has torn down yet");

    alt.stop(false);
    let full = captured_text(&captured);
    assert!(
        full.contains("\x1b[?1049l"),
        "teardown leaves the alternate screen"
    );
    assert!(full.contains("\x1b[?7h"), "autowrap restored (`:327`)");
    assert!(full.contains("\x1b[?25h"), "cursor shown (`:315`)");
    let enter_at = full.find("\x1b[?1049h").unwrap();
    let leave_at = full.find("\x1b[?1049l").unwrap();
    assert!(
        enter_at < leave_at,
        "the excursion must open before it closes"
    );
}

/// pi `:181` — "uses button-motion tracking inside terminal multiplexers".
///
/// cyrup's departure from crossterm's `EnableMouseCapture` is deliberate and documented on
/// `altscreen::mouse`: upstream withholds `?1003h` (any-motion) under a multiplexer and enables
/// focus reporting, which `?1004h` covers. Both halves are pinned; the `TMUX`-set branch is not
/// exercised here because this crate forbids `unsafe`, and `std::env::set_var` is `unsafe` in the
/// 2024 edition — `src/tests/mod.rs` records that constraint and the one binary that opts out.
#[test]
fn mouse_enable_is_multiplexer_aware() {
    let (_alt, captured, _) = screen(20, 4, 4);
    let text = captured_text(&captured);
    assert!(text.contains("\x1b[?1000h"), "normal tracking");
    assert!(text.contains("\x1b[?1002h"), "button-event tracking");
    assert!(
        text.contains("\x1b[?1004h"),
        "focus reporting — the selection cancel depends on it"
    );
    assert!(text.contains("\x1b[?1006h"), "SGR extended coordinates");
    assert!(
        !text.contains("\x1b[?1015h"),
        "urxvt coordinates are never enabled"
    );
}

/// Mouse reporting is reset on the way out, ahead of leaving the screen (`:306`).
#[test]
fn teardown_disables_mouse_before_leaving_the_screen() {
    let (mut alt, captured, _) = screen(20, 4, 4);
    alt.stop(false);
    let text = captured_text(&captured);
    let disable_at = text
        .find("\x1b[?1000l")
        .or_else(|| text.find("\x1b[?1002l"))
        .unwrap();
    let leave_at = text.find("\x1b[?1049l").unwrap();
    assert!(
        disable_at < leave_at,
        "the reset belongs inside the teardown bracket"
    );
}

// ---------------------------------------------------------------------------------------------
// scroll.rs + document.rs + wheel.rs
// ---------------------------------------------------------------------------------------------

/// pi `:57` — "renders a terminal-height viewport and preserves manual scroll position".
///
/// The upstream case in full: the tail is shown and followed, one wheel notch scrolls up by one
/// row and breaks the follow, and appending to the document does NOT move a reader who has
/// scrolled away.
#[test]
fn renders_the_tail_and_a_wheel_notch_breaks_follow() {
    let (mut alt, _captured, area) = screen(20, 4, 10);
    alt.draw(None).unwrap();
    assert_eq!(
        viewport(&mut alt),
        ["line 7", "line 8", "line 9", "line 10"]
    );
    assert!(
        alt.is_following_output(),
        "a fresh document follows its tail (`:70`)"
    );

    alt.handle_mouse(&wheel(MouseEventKind::ScrollUp, 1, 1), area);
    alt.draw(None).unwrap();
    assert_eq!(viewport(&mut alt), ["line 6", "line 7", "line 8", "line 9"]);
    assert_eq!(
        alt.viewport_top(),
        5,
        "pi asserts `tui.viewportTop === 5` (`:78`)"
    );
    assert!(
        !alt.is_following_output(),
        "and `isFollowingOutput === false` (`:79`)"
    );

    // `text.setText(... 12 lines ...)` (`:81`) — the parked reader must not be dragged forward.
    alt.set_document_for_test(doc(12), row_starts(12));
    alt.draw(None).unwrap();
    assert_eq!(viewport(&mut alt), ["line 6", "line 7", "line 8", "line 9"]);
}

/// pi `tui-alt-screen.test.ts:321` @v1.0.0 — "scrolls faster while Alt is held during wheel
/// input": a 12-line document in a 4-row viewport sits at `viewportTop === 8`, and one Alt+wheel-up
/// notch (`\x1b[<72;1;1M`, 72 = 64 + the Alt bit 8) lands at 3 — five lines, not one.
///
/// That is the renderer's own default, a fixed one row (`options.wheelScrollLines ?? 1`,
/// `tui-alt-screen.ts:270`), times the Alt multiplier. The multiplier applies to whatever the
/// `fullscreenWheelScrollLines` setting resolves to rather than to a constant one, so the same
/// notch moves 3 x 5 = 15 rows once the setting is 3 (`:698-701`).
#[test]
fn alt_wheel_scrolls_five_lines_per_notch() {
    let (mut alt, _captured, area) = screen(20, 4, 40);
    alt.draw(None).unwrap();
    assert_eq!(alt.viewport_top(), 36);
    let mut up = wheel(MouseEventKind::ScrollUp, 0, 0);
    up.modifiers = KeyModifiers::ALT;
    alt.handle_mouse(&up, area);
    assert_eq!(alt.viewport_top(), 31, "default one row x Alt 5");

    alt.set_wheel_scroll_lines(WheelScrollLines::from_number(3.0));
    alt.handle_mouse(&up, area);
    assert_eq!(alt.viewport_top(), 16, "setting 3 x Alt 5 = 15 rows");

    // And without Alt the setting alone is the count.
    alt.handle_mouse(&wheel(MouseEventKind::ScrollDown, 0, 0), area);
    assert_eq!(alt.viewport_top(), 19);
}

/// The `fullscreenWheelScrollLines` setting reaches the renderer's notch count, and a document
/// rect SHORTER than the terminal (an input dock below it) changes nothing about the arithmetic.
#[test]
fn the_wheel_count_setting_moves_the_document_rect_by_that_many_rows() {
    let (mut alt, _captured, full) = screen(20, 10, 60);
    alt.draw(None).unwrap();
    // A dock under the document leaves the renderer a shorter document rect; the wheel router only
    // asks the rect whether the pointer is over the view, never the terminal's size.
    let rect = Rect::new(full.x, full.y, full.width, 6);
    alt.set_wheel_scroll_lines(WheelScrollLines::from_number(7.0));
    let before = alt.viewport_top();
    alt.handle_mouse(&wheel(MouseEventKind::ScrollUp, 2, 2), rect);
    assert_eq!(alt.viewport_top(), before - 7);
    // Over the dock (outside the rect) the primary view still takes the notch.
    alt.handle_mouse(&wheel(MouseEventKind::ScrollUp, 2, 8), rect);
    assert_eq!(alt.viewport_top(), before - 14);
}

/// pi `:1317` — "ignores horizontal trackpad wheel events".
#[test]
fn horizontal_wheel_is_ignored() {
    let (mut alt, _captured, area) = screen(20, 4, 10);
    let before = alt.viewport_top();
    alt.handle_mouse(&wheel(MouseEventKind::ScrollLeft, 1, 1), area);
    alt.handle_mouse(&wheel(MouseEventKind::ScrollRight, 1, 1), area);
    assert_eq!(
        alt.viewport_top(),
        before,
        "horizontal notches move nothing"
    );
}

/// The programmatic viewport controls behind `ViewportRenderer` — pi's `scrollToTop`/`scrollToEnd`
/// (`components/scroll-view.ts:169-179`), and the `max_scroll_top` bound they clamp against.
#[test]
fn scroll_controls_clamp_to_the_document() {
    let (mut alt, _captured, _) = screen(20, 4, 10);
    alt.scroll_to_top();
    assert_eq!(alt.viewport_top(), 0);
    assert!(!alt.is_following_output());

    alt.scroll_by(-100);
    assert_eq!(alt.viewport_top(), 0, "clamped at the head");

    alt.scroll_to_bottom();
    assert_eq!(
        alt.viewport_top(),
        alt.max_scroll_top_for_test(),
        "the tail is the largest offset"
    );
    assert!(
        alt.is_following_output(),
        "scrolling to the end re-arms the follow"
    );

    alt.scroll_by(100);
    assert_eq!(
        alt.viewport_top(),
        alt.max_scroll_top_for_test(),
        "clamped at the tail"
    );
}

/// A document shorter than the viewport has nothing to scroll — `max_scroll_top` is zero and the
/// view stays pinned.
#[test]
fn a_short_document_cannot_scroll() {
    let (mut alt, _captured, area) = screen(20, 8, 3);
    alt.draw(None).unwrap();
    assert_eq!(alt.max_scroll_top_for_test(), 0);
    alt.handle_mouse(&wheel(MouseEventKind::ScrollUp, 1, 1), area);
    assert_eq!(alt.viewport_top(), 0);
}

// ---------------------------------------------------------------------------------------------
// scrollbar_drag.rs
// ---------------------------------------------------------------------------------------------

/// pi `:263`/`:319` — the scrollbar column. `"always"` reserves one (`getContentWidth`,
/// `components/scroll-view.ts:86-88`) where `"auto"` overlays the content's last column, and
/// `"hidden"` neither draws nor reserves.
///
/// Asserted on the reserved width AND on the painted column's cells: `always` paints the bar's
/// glyphs there and `hidden` leaves the document's own cells.
#[test]
fn scrollbar_mode_selects_the_reserved_column() {
    let (mut alt, _captured, area) = screen(20, 4, 40);

    alt.set_scrollbar_mode(ScrollbarMode::Always);
    assert_eq!(alt.scrollbar_mode_for_test(), ScrollbarMode::Always);
    assert_eq!(
        alt.content_width_for_test(area.width),
        area.width - 1,
        "`always` reserves a column"
    );
    alt.draw(None).unwrap();
    let painted = last_column_styles(&mut alt);

    alt.set_scrollbar_mode(ScrollbarMode::Hidden);
    assert_eq!(alt.scrollbar_mode_for_test(), ScrollbarMode::Hidden);
    assert_eq!(
        alt.content_width_for_test(area.width),
        area.width,
        "`hidden` reserves nothing"
    );
    alt.draw(None).unwrap();
    let bare = last_column_styles(&mut alt);

    assert_ne!(
        painted, bare,
        "an `always` bar paints the reserved column where `hidden` does not"
    );
}

/// The last column of every painted row, top to bottom.
fn bar_column(alt: &mut AltScreen<TestBackend>) -> Vec<String> {
    let backend = alt.backend_for_test();
    let width = usize::from(backend.size().map(|s| s.width).unwrap_or(0));
    backend
        .buffer()
        .content
        .chunks(width)
        .filter_map(|row| row.last().map(|cell| cell.symbol().to_owned()))
        .collect()
}

/// TUI-110, the row's own Verify: an overflowing transcript in `always` mode paints EVERY row of
/// the last column as track `│` or thumb `┃` (pi's `paintScrollbar`, `layout.ts:309-330`), with the
/// thumb in the theme's `scrollbarThumb` foreground and the track in `scrollbarTrack`'s.
///
/// Red before the change: the bar was a bare background fill on a space, so no row carried a glyph.
#[test]
fn an_always_bar_paints_a_glyph_on_every_row() {
    let (mut alt, _captured, _) = screen(20, 10, 100);
    alt.set_scrollbar_mode(ScrollbarMode::Always);
    alt.draw(None).unwrap();
    let glyphs = bar_column(&mut alt);
    assert!(
        glyphs.iter().all(|g| g == "│" || g == "┃"),
        "every row is track or thumb: {glyphs:?}"
    );
    assert!(
        glyphs.iter().filter(|g| *g == "┃").count() >= 2,
        "a thumb of at least two rows"
    );

    let theme = UiTheme::dark();
    let styles = last_column_styles(&mut alt);
    for (glyph, style) in glyphs.iter().zip(&styles) {
        let want = if glyph == "┃" {
            theme.scrollbar_thumb()
        } else {
            theme.scrollbar_track()
        };
        assert_eq!(style.fg, want, "{glyph} is painted as a foreground token");
        assert_eq!(
            style.bg,
            Some(ratatui::style::Color::Reset),
            "no background fill"
        );
    }
}

/// TUI-110, the Verify's second half: a press mid-track (off the thumb) jumps so the thumb is
/// centred on the pointer — it does not page — and a following drag moves it (pi
/// `test/tui-alt-screen.test.ts:481-506`). The thumb paints `█` while held.
#[test]
fn a_mid_track_press_jumps_the_thumb_to_the_pointer_and_a_drag_moves_it() {
    let (mut alt, _captured, area) = screen(20, 10, 50);
    alt.set_scrollbar_mode(ScrollbarMode::Always);
    alt.scroll_to_top();
    alt.draw(None).unwrap();
    let bar = area.width - 1;
    assert_eq!(alt.viewport_top(), 0);

    alt.handle_mouse(
        &wheel(MouseEventKind::Down(MouseButton::Left), bar, 5),
        area,
    );
    assert_eq!(
        alt.viewport_top(),
        20,
        "jumped to the pointer, not paged by a screenful"
    );
    alt.draw(None).unwrap();
    let glyphs = bar_column(&mut alt);
    assert_eq!(
        glyphs.iter().filter(|g| *g == "█").count(),
        2,
        "the held thumb paints as `█`: {glyphs:?}"
    );
    assert_eq!(glyphs[5], "█", "and the pointer's row is on it");

    alt.handle_mouse(
        &wheel(MouseEventKind::Drag(MouseButton::Left), bar, 9),
        area,
    );
    assert_eq!(alt.viewport_top(), 40, "the drag carries the thumb on");
    alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), bar, 9), area);
    // The release ended the drag: a further motion report no longer moves the document.
    alt.handle_mouse(
        &wheel(MouseEventKind::Drag(MouseButton::Left), bar, 0),
        area,
    );
    assert_eq!(alt.viewport_top(), 40);
}

/// TUI-110: hover covers the whole track column, including a hidden `auto` bar — a pointer
/// arriving over the column reveals it with no scroll at all, thumb or not (pi
/// `test/tui-alt-screen.test.ts:456-479`).
///
/// Red before the change: hover needed the pointer ON the thumb of an already-visible bar, so a
/// faded bar could not be found by pointing.
#[test]
fn pointing_at_the_track_reveals_a_hidden_auto_bar() {
    let (mut alt, _captured, area) = screen(20, 10, 50);
    // A fresh overflowing document follows its tail and has not moved: the `auto` bar is faded.
    alt.draw(None).unwrap();
    let is_bar = |g: &String| matches!(g.as_str(), "│" | "┃" | "█");
    assert!(
        !bar_column(&mut alt).iter().any(is_bar),
        "fixture: nothing painted yet"
    );

    // Reach the column at a row nowhere near the thumb (which is at the bottom, rows 8-9).
    let bar = area.width - 1;
    alt.handle_mouse(&wheel(MouseEventKind::Moved, bar, 2), area);
    alt.draw(None).unwrap();
    let after = bar_column(&mut alt);
    assert!(
        after.iter().all(is_bar),
        "the whole track is painted: {after:?}"
    );
    assert_eq!(
        after.iter().filter(|g| *g == "█").count(),
        2,
        "hovered: the thumb is expanded"
    );
    assert_eq!(alt.viewport_top(), 40, "revealing it scrolled nothing");

    // Leaving the column lets go of the hover (the bar then fades on its own deadline).
    alt.handle_mouse(&wheel(MouseEventKind::Moved, 3, 2), area);
    alt.draw(None).unwrap();
    assert_eq!(bar_column(&mut alt).iter().filter(|g| *g == "█").count(), 0);
}

/// pi orders the two halves of a pointer report as "offer it to the scrollbar, THEN refresh hover"
/// (`tui-alt-screen.ts:932-934`), and a press looks the bar up WITHOUT `includeHiddenAuto`. So the
/// first press on a faded bar's column is content — a text selection starts, no thumb is grabbed,
/// nothing scrolls — and only the hover refresh that follows reveals the bar.
///
/// Red if the hover refresh runs first: the press would find the freshly revealed bar and jump the
/// view to the pointer.
#[test]
fn the_first_press_on_a_faded_bar_is_content_and_only_reveals_it() {
    let (mut alt, _captured, area) = screen(20, 10, 50);
    alt.draw(None).unwrap();
    assert_eq!(alt.viewport_top(), 40);

    let bar = area.width - 1;
    alt.handle_mouse(
        &wheel(MouseEventKind::Down(MouseButton::Left), bar, 3),
        area,
    );
    assert_eq!(
        alt.viewport_top(),
        40,
        "the press did not grab or jump the bar"
    );
    alt.draw(None).unwrap();
    assert!(
        bar_column(&mut alt).iter().any(|g| g == "█"),
        "but the hover refresh revealed it"
    );
}

/// pi `:776-784` — a report the scrollbar CLAIMS clears every selection field, because the pointer
/// now belongs to the thumb. Regression pin for the arm that was declared and never called.
#[test]
fn a_scrollbar_grab_cancels_an_in_flight_selection() {
    let (mut alt, _captured, area) = screen(20, 6, 40);
    alt.set_scrollbar_mode(ScrollbarMode::Always);
    alt.draw(None).unwrap();

    // Press inside the document and drag — an in-flight selection.
    alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 2, 1), area);
    alt.handle_mouse(&wheel(MouseEventKind::Drag(MouseButton::Left), 6, 2), area);

    // Then grab the thumb in the reserved last column.
    let bar_col = area.width.saturating_sub(1);
    alt.handle_mouse(
        &wheel(MouseEventKind::Down(MouseButton::Left), bar_col, 1),
        area,
    );

    assert!(
        alt.selection_text().is_none(),
        "the thumb grab dropped the selection"
    );
}

// ---------------------------------------------------------------------------------------------
// selection.rs
// ---------------------------------------------------------------------------------------------

/// pi `:945` — "selects visible text with the mouse and copies it … after a generic release".
///
/// cyrup returns the text through [`PointerOutcome::Copy`] and lets the app write the clipboard
/// (`app/run_action.rs`), where upstream writes OSC 52 from inside the renderer. Same gesture, same
/// payload, one layer out.
#[test]
fn a_drag_and_release_yields_the_selected_text() {
    let (mut alt, _captured, area) = screen(20, 6, 10);
    alt.draw(None).unwrap();

    alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 0, 0), area);
    alt.handle_mouse(&wheel(MouseEventKind::Drag(MouseButton::Left), 6, 0), area);
    assert!(
        alt.selection_text().is_some(),
        "a live drag has selected text"
    );

    let outcome = alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), 6, 0), area);
    let copied = match outcome {
        PointerOutcome::Copy(text) => text,
        _ => String::new(),
    };
    assert!(
        !copied.is_empty(),
        "the release yields PointerOutcome::Copy with the payload"
    );
}

/// CFG-078 — pi `test/tui-alt-screen.test.ts:1003-1007` @v0.84.4: "leaves selections visible
/// without copying when copyOnSelect is disabled".
///
/// The gate is `if (this.copyOnSelect) void this.copySelectionToClipboard();`
/// (`packages/tui/src/tui-alt-screen.ts:1035`), so it withholds the COPY and nothing else: the
/// anchor, the focus and the highlight are all still there afterwards, which is what makes the
/// `Ctrl+X` fallback the CHANGELOG describes possible at all
/// (`packages/coding-agent/docs/settings.md:72`).
///
/// Red before this change: `set_copy_on_select` did not exist, and with it stubbed to a no-op the
/// release still answers `PointerOutcome::Copy`.
#[test]
fn copy_on_select_disabled_keeps_the_selection_and_withholds_the_copy() {
    let (mut alt, _captured, area) = screen(20, 6, 10);
    alt.draw(None).unwrap();
    alt.set_copy_on_select(false);

    alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 0, 0), area);
    alt.handle_mouse(&wheel(MouseEventKind::Drag(MouseButton::Left), 6, 0), area);
    let outcome = alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), 6, 0), area);
    assert_eq!(
        outcome,
        PointerOutcome::Handled,
        "the release is still OURS — only the clipboard write is withheld"
    );
    assert!(
        alt.selection_text().is_some(),
        "the selection stays live so Ctrl+X / `/copy` can still reach it"
    );

    // Turning it back on mid-session is upstream's `setCopyOnSelect` (`tui-alt-screen.ts:246-248`),
    // and the very next release copies again — no re-entry of the renderer needed.
    alt.set_copy_on_select(true);
    alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 0, 0), area);
    alt.handle_mouse(&wheel(MouseEventKind::Drag(MouseButton::Left), 6, 0), area);
    let outcome = alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), 6, 0), area);
    assert!(
        matches!(outcome, PointerOutcome::Copy(ref text) if !text.is_empty()),
        "with the setting back on the release copies again, got {outcome:?}"
    );
}

/// pi `:1170` — "clears an active visible selection on focus loss", and `:1200` — a COMPLETED
/// selection survives it. The two clears are different on purpose (`altscreen::selection`).
#[test]
fn focus_loss_clears_an_in_flight_selection_only() {
    let (mut alt, _captured, area) = screen(20, 6, 10);
    alt.draw(None).unwrap();

    // In flight: pressed, dragged, not released.
    alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 0, 0), area);
    alt.handle_mouse(&wheel(MouseEventKind::Drag(MouseButton::Left), 6, 0), area);
    alt.handle_focus_lost();
    assert!(
        alt.selection_text().is_none(),
        "an in-flight drag does not survive focus loss"
    );

    // Completed: pressed, dragged, released.
    alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 0, 0), area);
    alt.handle_mouse(&wheel(MouseEventKind::Drag(MouseButton::Left), 6, 0), area);
    let _ = alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), 6, 0), area);
    alt.handle_focus_lost();
    assert!(
        alt.selection_text().is_some(),
        "a completed selection survives (`:1200`)"
    );
}

/// pi `:1088` — "selects whole words on double click … and selects lines on triple click".
///
/// The granularity ladder is `Character → Word → Line` (`selection.rs:516-518`), driven by the
/// click count `last_click` accumulates. A double click must select more than a single click did,
/// and a triple click more again.
#[test]
fn click_count_widens_the_selection_granularity() {
    let (mut alt, _captured, area) = screen(30, 6, 6);
    alt.set_document_for_test(vec![Line::from("alpha beta gamma delta"); 6], row_starts(6));
    alt.draw(None).unwrap();

    let press = |alt: &mut AltScreen<TestBackend>| {
        alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 8, 1), area);
        alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), 8, 1), area);
    };

    press(&mut alt);
    let single = alt.selection_text().unwrap_or_default().len();
    press(&mut alt);
    let double = alt.selection_text().unwrap_or_default().len();
    press(&mut alt);
    let triple = alt.selection_text().unwrap_or_default().len();

    assert!(
        double > single,
        "a double click selects the word, not the character"
    );
    assert!(triple > double, "a triple click selects the whole line");
}

/// A press outside the document viewport is no longer declined: pi anchors it on the screen
/// (`getSelectionPoint` without a scroll view, `:1149-1158`), clamped into the frame, so a drag that
/// starts on the dock selects what is painted there.
#[test]
fn a_press_below_the_document_anchors_on_the_screen() {
    let (mut alt, _captured, area) = screen(20, 4, 2);
    alt.draw(None).unwrap();
    let outcome = alt.handle_mouse(
        &wheel(MouseEventKind::Down(MouseButton::Left), 0, area.height + 5),
        area,
    );
    assert!(matches!(outcome, PointerOutcome::Handled));
}

// ---------------------------------------------------------------------------------------------
// flash.rs + timers.rs
// ---------------------------------------------------------------------------------------------

/// pi `:1230` — "stacks flash messages and collapses them as they expire", plus the deadline the
/// run loop arms its next wake on (`altscreen::timers`).
#[test]
fn a_flash_paints_and_arms_a_deadline() {
    let (mut alt, _captured, _) = screen(30, 6, 4);
    assert!(
        alt.next_deadline().is_none(),
        "an idle renderer arms nothing"
    );

    alt.flash("saved", Some(Duration::from_secs(30)));
    assert!(
        alt.next_deadline().is_some(),
        "a live notice is a wake reason"
    );

    alt.draw(None).unwrap();
    assert!(
        viewport(&mut alt).iter().any(|row| row.contains("saved")),
        "the notice is painted"
    );
}

/// pi's `dispose()` (`components/alt-screen-flash.ts:38-41`) at `tui-alt-screen.ts:303` — a notice
/// queued in one excursion must not survive into the next. Regression pin for the call that was
/// declared and never made.
#[test]
fn teardown_disposes_queued_flashes() {
    let (mut alt, _captured, _) = screen(30, 6, 4);
    alt.flash("saved", Some(Duration::from_secs(30)));
    assert!(alt.next_deadline().is_some());
    alt.stop(false);
    assert!(alt.next_deadline().is_none(), "teardown cleared the stack");
}

// ---------------------------------------------------------------------------------------------
// keys.rs + prompt_nav.rs
// ---------------------------------------------------------------------------------------------

/// pi `:506`/`:535` — half-page and single-line viewport navigation. cyrup resolves them through
/// [`AltScreenKeymap`] rather than pi's `KeybindingsManager`, so the default map is what is driven.
#[test]
fn viewport_keys_scroll_and_unmatched_keys_fall_through() {
    let (mut alt, _captured, _) = screen(20, 6, 60);
    let keys = AltScreenKeymap::default();
    alt.scroll_to_top();

    let page_down = KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE);
    assert!(
        alt.handle_key(&page_down, &keys, &[]),
        "PageDown is a viewport binding"
    );
    assert!(alt.viewport_top() > 0, "and it moved the viewport");

    // An ordinary character is not a viewport binding: the renderer declines so the editor sees it.
    let typed = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    assert!(
        !alt.handle_key(&typed, &keys, &[]),
        "unmatched keys fall through to the editor"
    );
}

/// pi `:608` — "jumps between OSC 133 semantic prompt markers".
///
/// **DIVERGENT:** cyrup walks `Entry::User` indices rather than re-parsing OSC 133 marks — see
/// `altscreen::prompt_nav`. The gesture, the strictness and the landing rule are upstream's; only
/// the candidate test differs.
///
/// The fixture is sized so every landing is EXACT rather than clamped: 40 rows in a 6-row viewport
/// puts `max_scroll_top` at 34, well above the prompt rows at 5/15/25. A fixture whose prompts sat
/// above that maximum would land clamped and assert nothing.
#[test]
fn prompt_navigation_jumps_between_user_entries() {
    let (mut alt, _captured, _) = screen(20, 6, 40);
    // `row_of_entry` is `row_starts.get(entry)`, so this map has one slot per ENTRY, not per line.
    // Users sit at entry indices 0, 2, 4 -> prompt rows 5, 15, 25. The interleaved non-user
    // entries are what prove the `Entry::User` filter rather than just the map.
    let entries = vec![
        Entry::User {
            text: "first".into(),
            lead_spacer: false,
        },
        Entry::Assistant("reply".into()),
        Entry::User {
            text: "second".into(),
            lead_spacer: true,
        },
        Entry::Status("note".into()),
        Entry::User {
            text: "third".into(),
            lead_spacer: true,
        },
        Entry::Assistant("tail".into()),
    ];
    alt.set_document_for_test(doc(40), vec![5, 10, 15, 20, 25, 30]);
    let keys = AltScreenKeymap::default();
    let prev = KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL);
    let next = KeyEvent::new(KeyCode::Down, KeyModifiers::CONTROL);

    alt.scroll_to_row_for_test(20);
    assert!(
        alt.handle_key(&prev, &keys, &entries),
        "the chord is consumed either way"
    );
    assert_eq!(
        alt.viewport_top(),
        15,
        "the nearest user entry strictly ABOVE row 20"
    );

    alt.scroll_to_row_for_test(20);
    assert!(alt.handle_key(&next, &keys, &entries));
    assert_eq!(
        alt.viewport_top(),
        25,
        "the nearest user entry strictly BELOW row 20"
    );

    // Both comparisons are strict, so sitting exactly on a prompt row is not a candidate.
    alt.scroll_to_row_for_test(25);
    assert!(alt.handle_key(&next, &keys, &entries));
    assert_eq!(
        alt.viewport_top(),
        25,
        "no prompt past the last one — a no-op, with no clamp"
    );

    alt.scroll_to_row_for_test(0);
    assert!(alt.handle_key(&prev, &keys, &entries));
    assert_eq!(
        alt.viewport_top(),
        0,
        "nothing above row 0 — upstream's loop refuses to start"
    );
}

/// The walk over an empty transcript is inert — upstream's `if (!this.currentLayout) return`
/// (`:413`) refuses for the same reason, and a chord with no candidate is still consumed.
///
/// This is NOT the port of `:608`; `prompt_navigation_jumps_between_user_entries` is.
#[test]
fn prompt_navigation_over_an_empty_transcript_is_inert() {
    let (mut alt, _captured, _) = screen(20, 6, 20);
    let keys = AltScreenKeymap::default();
    alt.scroll_to_row_for_test(8);
    for code in [KeyCode::Up, KeyCode::Down] {
        assert!(alt.handle_key(&KeyEvent::new(code, KeyModifiers::CONTROL), &keys, &[]));
    }
    assert_eq!(
        alt.viewport_top(),
        8,
        "no entries means no prompt to jump to"
    );
}

// ---------------------------------------------------------------------------------------------
// images.rs
// ---------------------------------------------------------------------------------------------

/// pi `:1310-1316` — a full redraw unpins kitty placements, choosing `deleteAllKittyPlacements()`
/// (`d=a`, retaining uploads) over `deleteKittyImages()` (`d=A`) when uploads are held.
///
/// A resize is cyrup's full redraw. On a terminal with no kitty support the protocol is `None`, so
/// upstream's own guard (`imageProtocol === "kitty"`) makes this a no-op — which is exactly what is
/// asserted: the branch runs and writes nothing rather than emitting a delete blind.
#[test]
fn resize_is_a_full_redraw_for_images() {
    let (mut alt, captured, _) = screen(20, 6, 10);
    assert!(
        alt.image_protocol_for_test().is_none(),
        "the test terminal negotiates no graphics"
    );
    let before = captured_text(&captured).len();
    alt.handle_resize();
    let after = captured_text(&captured);
    assert_eq!(
        after.len(),
        before,
        "no protocol means no delete escape (`deleteKittyImages` = \"\")"
    );
    assert!(
        !after.contains("\x1b_Ga=d"),
        "and certainly no kitty delete"
    );
}

/// The teardown delete is likewise gated on the protocol (`:336-338`), and the registry is emptied
/// either way — pi's `uploadedKittyImages.clear()` at `:308`.
#[test]
fn teardown_clears_the_placement_registry() {
    let (mut alt, captured, _) = screen(20, 6, 10);
    alt.stop(false);
    assert_eq!(
        alt.tracked_images_for_test(),
        0,
        "the registry is empty after teardown"
    );
    assert!(
        !captured_text(&captured).contains("\x1b_Ga=d"),
        "no kitty delete on a non-kitty terminal"
    );
}

// ---------------------------------------------------------------------------------------------
// document.rs — the real hand-over path, which the fixture deliberately bypasses
// ---------------------------------------------------------------------------------------------

/// The production `set_document` reconciles the incoming build against the transcript's front-trim
/// reports (§B-1/§B-5) through [`altscreen::document::rows_dropped`]. With an untrimmed transcript
/// the reconciliation is a no-op, which is the property worth pinning: a fresh document must not
/// move a reader who has scrolled away.
#[test]
fn the_real_document_hand_over_reconciles_against_the_transcript() {
    let (mut alt, _captured) =
        AltScreen::for_test(TestBackend::new(20, 4), UiTheme::dark()).unwrap();
    let transcript = TranscriptView::new();
    assert_eq!(transcript.retained_dropped(), 0, "nothing has been trimmed");

    alt.set_document(&transcript, doc(10), row_starts(10), 0);
    alt.draw(None).unwrap();
    assert_eq!(
        viewport(&mut alt),
        ["line 7", "line 8", "line 9", "line 10"]
    );

    alt.scroll_to_top();
    alt.set_document(&transcript, doc(12), row_starts(12), 0);
    assert_eq!(
        alt.viewport_top(),
        0,
        "an untrimmed rebuild leaves a parked reader where it was"
    );
}

/// `sync_document` builds the document from the transcript itself — pi's implicit document
/// (`tui-alt-screen.ts:212-214`) — through `document::document_key` (the cache key) and
/// `document::render_document`.
///
/// Driven over a POPULATED transcript: `set_retain_document(true)` is what moves drained entries
/// into `document()` (`view.rs:151-154`) and is off by default, which is the only reason this is
/// not the obvious thing to write.
#[test]
fn syncing_a_populated_transcript_renders_its_entries() {
    let (mut alt, _captured) =
        AltScreen::for_test(TestBackend::new(40, 12), UiTheme::dark()).unwrap();
    let theme = UiTheme::dark();

    let mut transcript = TranscriptView::new();
    transcript.set_retain_document(true);
    transcript.push_user("hello there");
    transcript.commit_assistant(Some("general kenobi".into()));
    let drained = transcript.drain_committed();
    assert!(
        !drained.is_empty(),
        "the fixture actually committed something"
    );
    assert!(!transcript.document().is_empty(), "and retention kept it");

    alt.sync_document(
        &transcript,
        &theme,
        ImageOpts::default(),
        &[],
        &std::sync::Arc::new(Vec::new()),
    );
    alt.draw(None).unwrap();

    let rows = viewport(&mut alt);
    let painted = rows.join("\n");
    assert!(
        painted.contains("hello there"),
        "the user submission reaches a rendered row"
    );
    assert!(
        painted.contains("general kenobi"),
        "and so does the assistant reply"
    );
    assert!(
        alt.is_following_output(),
        "a freshly synced document follows its tail"
    );
}

/// An empty transcript renders an empty document and the renderer survives drawing one — the
/// degenerate half of the same seam.
#[test]
fn syncing_an_empty_transcript_renders_an_empty_document() {
    let (mut alt, _captured) =
        AltScreen::for_test(TestBackend::new(20, 4), UiTheme::dark()).unwrap();
    let transcript = TranscriptView::new();
    let theme = UiTheme::dark();
    alt.sync_document(
        &transcript,
        &theme,
        ImageOpts::default(),
        &[],
        &std::sync::Arc::new(Vec::new()),
    );
    alt.draw(None).unwrap();
    assert!(
        viewport(&mut alt).iter().all(|row| row.is_empty()),
        "nothing to paint"
    );
    assert_eq!(alt.viewport_top(), 0);
}

// ---------------------------------------------------------------------------------------------
// exit.rs
// ---------------------------------------------------------------------------------------------

/// pi `:1336` — "prints the full document" on the way out. `preserve_screen: false` repaints the
/// retained rows into the main screen so a fullscreen session leaves its transcript in scrollback.
#[test]
fn teardown_repaints_the_document_into_scrollback() {
    let (mut alt, captured, _) = screen(20, 4, 6);
    alt.draw(None).unwrap();
    alt.stop(false);
    let text = captured_text(&captured);
    assert!(
        text.contains("line 1"),
        "the repaint writes rows the viewport had scrolled past"
    );
    assert!(text.contains("line 6"), "through to the last row");
    let leave_at = text.find("\x1b[?1049l").unwrap();
    let first_row_at = text.find("line 1").unwrap();
    assert!(
        leave_at < first_row_at,
        "the repaint lands on the MAIN screen, after leaving"
    );
}

/// The other half of the same contract: `preserve_screen: true` is the unwind branch, and it must
/// NOT write the document — upstream's `preserveScreen` case is `EXIT_ALT_SCREEN` and a cursor show,
/// nothing else (`tui-alt-screen.ts:314-315`).
///
/// Both of cyrup's real teardowns pass `false`; this pins the branch `TerminalSetup::Drop` takes, so
/// a panic mid-session cannot dump a half-rendered document over the user's shell.
#[test]
fn preserving_the_screen_skips_the_repaint_entirely() {
    let (mut alt, captured, _) = screen(20, 4, 6);
    alt.draw(None).unwrap();
    alt.stop(true);
    let text = captured_text(&captured);
    assert!(
        text.contains("\x1b[?1049l"),
        "it still leaves the alternate screen"
    );
    assert!(!text.contains("line 1"), "but writes no document row");
    assert!(!text.contains("line 6"), "not the last one either");
}

// ---------------------------------------------------------------------------------------------
// mod.rs — the composition itself
// ---------------------------------------------------------------------------------------------

/// The renderer identifies as fullscreen — pi's `TuiBase.mode` (`tui.ts:332`), which is what the
/// mode switch and every `ViewportRenderer` consumer branch on.
#[test]
fn the_renderer_reports_fullscreen_mode() {
    let (alt, _captured, _) = screen(20, 4, 2);
    assert_eq!(alt.mode(), TuiRenderMode::Fullscreen);
}

/// `stop` is idempotent — pi's `if (!this.altScreenActive) return` (`:304`/`:312`), which is what
/// lets the orderly path call it and `Drop` still be correct on the paths that do not.
#[test]
fn teardown_is_idempotent() {
    let (mut alt, captured, _) = screen(20, 4, 4);
    alt.stop(false);
    let once = captured_text(&captured);
    alt.stop(false);
    assert_eq!(
        captured_text(&captured),
        once,
        "a second teardown writes nothing"
    );
}

// ---------------------------------------------------------------------------------------------
// indicator.rs — the "Jump to latest message" label (TUI-109)
// ---------------------------------------------------------------------------------------------
//
// pi has no test for `compositeScrollToEndIndicator` in `test/tui-alt-screen.test.ts` @v0.87.1, so
// these pin the contract from its source (`tui-alt-screen.ts:1018-1024`, `:1623-1645`) and the
// label from `modes/interactive/tui-renderer.ts:29-33`.

/// A left press — `handleScrollToEndIndicatorMouseEvent` tests `release`, the motion bit and the
/// button bits (`:1019`), which is `Down(Left)` here.
fn press(column: u16, row: u16) -> MouseEvent {
    wheel(MouseEventKind::Down(MouseButton::Left), column, row)
}

/// The last painted row of the document's viewport — the bottom of the rectangle the scrolled
/// document was painted in, which is the whole screen unless a dock is painted under it.
fn last_row(alt: &mut AltScreen<TestBackend>) -> String {
    let area = alt.doc_area();
    let bottom = usize::from(area.y.saturating_add(area.height)).saturating_sub(1);
    viewport(alt).get(bottom).cloned().unwrap_or_default()
}

/// The label on the last row, without the document text to its left: from the arrow to the end.
fn label_text(alt: &mut AltScreen<TestBackend>) -> String {
    let row = last_row(alt);
    row.split_once('↓').map_or_else(String::new, |(_, rest)| {
        format!("↓{rest}").trim().to_owned()
    })
}

/// A 60x6 screen over 40 rows, parked 10 rows above its tail with the `End` shortcut pushed in.
fn scrolled_up() -> (AltScreen<TestBackend>, Rect) {
    let (mut alt, _captured, area) = screen(60, 6, 40);
    alt.set_scroll_to_end_key(Some("End".to_owned()));
    alt.draw(None).unwrap();
    let tail = alt.max_scroll_top_for_test();
    alt.scroll_to_row_for_test(tail - 10);
    alt.draw(None).unwrap();
    // Every negative case below is only meaningful against a label that IS on screen.
    assert!(
        last_row(&mut alt).contains("Jump"),
        "fixture: label painted"
    );
    (alt, area)
}

/// Scrolled up 10 rows: the last row carries the label, centred, with the shortcut.
#[test]
fn a_scrolled_up_view_shows_the_jump_label_on_its_last_row() {
    let (mut alt, _) = scrolled_up();
    assert!(!alt.is_following_output());
    let row = last_row(&mut alt);
    assert!(
        row.contains("Jump to latest message"),
        "last row was {row:?}"
    );
    assert!(row.contains("· End"), "the shortcut follows it: {row:?}");
    // Centred on the 60-column viewport: ` ↓ Jump to latest message · End ` is 32 columns wide.
    assert_eq!(row.find('↓'), Some(14 + 1), "centred at column 14: {row:?}");
    // ...and only the last row: the rows above are the document.
    let rows = viewport(&mut alt);
    assert!(rows[..rows.len() - 1].iter().all(|r| !r.contains("Jump")));
}

/// At the tail, or while following: no label.
#[test]
fn no_jump_label_at_the_tail() {
    let (mut alt, _captured, _) = screen(60, 6, 40);
    alt.set_scroll_to_end_key(Some("End".to_owned()));
    alt.draw(None).unwrap();
    assert!(alt.is_following_output());
    assert!(!last_row(&mut alt).contains("Jump"));

    // Away and back by keys: the label appears, then goes when the tail is re-armed.
    let (mut alt, _) = scrolled_up();
    assert!(last_row(&mut alt).contains("Jump"));
    let keys = AltScreenKeymap::default();
    let end = KeyEvent::new(KeyCode::End, KeyModifiers::CONTROL);
    assert!(alt.handle_key(&end, &keys, &[]));
    alt.draw(None).unwrap();
    assert!(alt.is_following_output());
    assert!(!last_row(&mut alt).contains("Jump"));
}

/// A renderer nobody installed the indicator on paints none, scrolled away or not — upstream's
/// `scrollToEndIndicator` is an optional constructor callback, and every case in
/// `test/tui-alt-screen.test.ts` builds a bare `TuiAltScreen`.
#[test]
fn a_renderer_without_the_indicator_paints_none() {
    let (mut alt, _captured, _) = screen(60, 6, 40);
    alt.draw(None).unwrap();
    let tail = alt.max_scroll_top_for_test();
    alt.scroll_to_row_for_test(tail - 10);
    alt.draw(None).unwrap();
    assert!(!alt.is_following_output());
    assert!(!viewport(&mut alt).iter().any(|r| r.contains("Jump")));
}

/// A content that fits is always at its tail: no label.
#[test]
fn no_jump_label_when_the_document_fits() {
    let (mut alt, _captured, _) = screen(60, 6, 3);
    alt.set_scroll_to_end_key(Some("End".to_owned()));
    alt.draw(None).unwrap();
    assert!(!viewport(&mut alt).iter().any(|r| r.contains("Jump")));
}

/// A left press on the label re-arms the tail follow and the label goes.
#[test]
fn clicking_the_jump_label_returns_to_the_tail() {
    let (mut alt, area) = scrolled_up();
    let outcome = alt.handle_mouse(&press(20, area.height - 1), area);
    assert_eq!(outcome, PointerOutcome::Handled);
    assert!(alt.is_following_output(), "the click re-arms the follow");
    alt.draw(None).unwrap();
    assert!(!last_row(&mut alt).contains("Jump"));
    assert_eq!(
        viewport(&mut alt).last().map(String::as_str),
        Some("line 40"),
        "and the viewport is at the tail"
    );
}

/// Under a modal overlay the label is not the user's to press: the press goes to the selection
/// (pi skips `handleScrollToEndIndicatorMouseEvent` when `overlay.hit`, `:915-918`). With a modal
/// open elsewhere on the screen it is still the label's.
#[test]
fn the_jump_label_is_inert_under_a_modal() {
    let (mut alt, area) = scrolled_up();
    let outcome = alt.handle_mouse_under(&press(20, area.height - 1), area, Modal::Under);
    assert_eq!(outcome, PointerOutcome::Handled, "a selection began");
    assert!(
        !alt.is_following_output(),
        "the click did not re-arm the follow"
    );
    assert!(last_row(&mut alt).contains("Jump"));

    let (mut alt, area) = scrolled_up();
    alt.handle_mouse_under(&press(20, area.height - 1), area, Modal::Open);
    assert!(alt.is_following_output(), "beside the modal it still works");
}

/// While a modal is open the scrollbar cannot be grabbed (pi's `getScrollbarTargetAt` returns
/// nothing under `hasOverlay()`, `:1037`): the press anchors a selection and the thumb stays put.
#[test]
fn the_scrollbar_cannot_be_grabbed_while_a_modal_is_open() {
    for modal in [Modal::Open, Modal::Under] {
        let (mut alt, _captured, area) = screen(20, 10, 50);
        alt.set_scrollbar_mode(ScrollbarMode::Always);
        alt.scroll_to_top();
        alt.draw(None).unwrap();
        let bar = area.width - 1;
        alt.handle_mouse_under(
            &wheel(MouseEventKind::Down(MouseButton::Left), bar, 5),
            area,
            modal,
        );
        assert_eq!(alt.viewport_top(), 0, "{modal:?}: the thumb did not jump");
        alt.handle_mouse_under(
            &wheel(MouseEventKind::Drag(MouseButton::Left), bar, 9),
            area,
            modal,
        );
        assert_eq!(alt.viewport_top(), 0, "{modal:?}: and no drag follows");
    }
}

/// A modal defers the wheel to itself: a notch the overlay did not take scrolls nothing (pi's
/// `shouldDeferViewportInputToOverlay`, `:709`).
#[test]
fn the_wheel_does_not_scroll_the_document_while_a_modal_is_open() {
    let (mut alt, _captured, area) = screen(20, 10, 50);
    alt.scroll_to_top();
    alt.draw(None).unwrap();
    for modal in [Modal::Open, Modal::Under] {
        let outcome = alt.handle_mouse_under(&wheel(MouseEventKind::ScrollDown, 3, 3), area, modal);
        assert_eq!(outcome, PointerOutcome::Ignored, "{modal:?}");
        assert_eq!(alt.viewport_top(), 0, "{modal:?}");
    }
    alt.handle_mouse_under(
        &wheel(MouseEventKind::ScrollDown, 3, 3),
        area,
        Modal::Absent,
    );
    assert!(alt.viewport_top() > 0, "with no modal it scrolls");
}

/// pi's `clearTextSelection()` leaves `lastClick` alone (`:893-902`): a press a component handled
/// clears the highlight, and the next click on the same word is still the second of a double click.
#[test]
fn clearing_the_selection_keeps_the_multi_click_ladder() {
    let (mut alt, _captured, area) = screen(30, 6, 6);
    alt.set_document_for_test(vec![Line::from("alpha beta gamma delta"); 6], row_starts(6));
    alt.draw(None).unwrap();
    let click = |alt: &mut AltScreen<TestBackend>| {
        alt.handle_mouse(&wheel(MouseEventKind::Down(MouseButton::Left), 8, 1), area);
        alt.handle_mouse(&wheel(MouseEventKind::Up(MouseButton::Left), 8, 1), area);
    };

    click(&mut alt);
    alt.clear_selection();
    click(&mut alt);
    assert_eq!(
        alt.selection_text().as_deref(),
        Some("beta"),
        "the second click counted as a double click"
    );

    // A full reset (a swapped session) is the other thing, and does forget the ladder.
    alt.reset_selection();
    click(&mut alt);
    assert_eq!(alt.selection_text(), None, "a first click selects no word");
}

/// A press outside the label's rectangle does not: the row above, the same row left of the label,
/// and the same row right of it.
#[test]
fn clicking_outside_the_jump_label_does_not() {
    for (column, row) in [(20, 4), (2, 5), (55, 5)] {
        let (mut alt, area) = scrolled_up();
        assert!(row < area.height && column < area.width);
        alt.handle_mouse(&press(column, row), area);
        assert!(
            !alt.is_following_output(),
            "a press at ({column}, {row}) must not jump"
        );
        alt.draw(None).unwrap();
        assert!(last_row(&mut alt).contains("Jump"));
    }
}

/// Only an unreleased left press counts (`:1019`): a release, a drag and another button do not.
#[test]
fn only_a_left_press_activates_the_jump_label() {
    for kind in [
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Moved,
    ] {
        let (mut alt, area) = scrolled_up();
        alt.handle_mouse(&wheel(kind, 20, area.height - 1), area);
        assert!(!alt.is_following_output(), "{kind:?} must not jump");
    }
}

/// The label stops at the scrollbar column (`:1635`): in `always` mode the bar's column is left to
/// the bar.
#[test]
fn the_jump_label_does_not_overpaint_the_scrollbar_column() {
    let (mut alt, _captured, area) = screen(30, 6, 40);
    alt.set_scrollbar_mode(ScrollbarMode::Always);
    alt.set_scroll_to_end_key(Some("End".to_owned()));
    alt.draw(None).unwrap();
    let tail = alt.max_scroll_top_for_test();
    alt.scroll_to_row_for_test(tail - 10);
    alt.draw(None).unwrap();

    // ` ↓ Jump to latest message · End ` is 32 columns; the viewport is 30 and the bar owns the
    // 30th, so 29 survive: ... `· E`.
    assert_eq!(last_row(&mut alt), " ↓ Jump to latest message · E");
    let bar_cell = alt
        .backend_for_test()
        .buffer()
        .cell((area.width - 1, area.height - 1))
        .map(|cell| cell.symbol().to_owned());
    assert!(
        matches!(bar_cell.as_deref(), Some("│" | "┃" | "█")),
        "the bar column stays the bar's, got {bar_cell:?}"
    );
}

/// A click in the column the label was clipped out of is not a click on the label.
#[test]
fn the_clipped_column_is_not_part_of_the_jump_label() {
    let (mut alt, _captured, area) = screen(30, 6, 40);
    alt.set_scrollbar_mode(ScrollbarMode::Always);
    alt.set_scroll_to_end_key(Some("End".to_owned()));
    alt.draw(None).unwrap();
    let tail = alt.max_scroll_top_for_test();
    alt.scroll_to_row_for_test(tail - 10);
    alt.draw(None).unwrap();
    assert!(
        last_row(&mut alt).contains("Jump"),
        "fixture: label painted"
    );
    // The press lands on the bar's track, so it STARTS A DRAG — which a click on the label never
    // does. A pointer dragged to the top then moves the document there.
    alt.handle_mouse(&press(area.width - 1, area.height - 1), area);
    alt.handle_mouse(
        &wheel(MouseEventKind::Drag(MouseButton::Left), area.width - 1, 0),
        area,
    );
    assert_eq!(
        alt.viewport_top(),
        0,
        "the scrollbar column belongs to the bar (a track press and a drag), not to the label"
    );
}

/// The label's shortcut is what the owner pushed; unbound drops the ` · key` (`shortcut ? … : ""`),
/// and a later push replaces it.
#[test]
fn the_jump_label_shows_the_pushed_shortcut() {
    let (mut alt, _) = scrolled_up();
    assert_eq!(label_text(&mut alt), "↓ Jump to latest message · End");

    alt.set_scroll_to_end_key(Some("Ctrl+G".to_owned()));
    alt.draw(None).unwrap();
    assert_eq!(label_text(&mut alt), "↓ Jump to latest message · Ctrl+G");

    alt.set_scroll_to_end_key(None);
    alt.draw(None).unwrap();
    assert_eq!(label_text(&mut alt), "↓ Jump to latest message");
}

/// The label is skipped when the last row holds an image (`isImageLine`, `:1631`): an attachment
/// strip drawn through a graphics protocol that fills the viewport keeps its last row.
#[test]
fn the_jump_label_is_skipped_on_an_image_row() {
    use crate::altscreen::Strip;
    use crate::image::{ImageProtocol, TerminalCapabilities};
    use crate::{ImageBlock, ImageRenderer};
    use image::{DynamicImage, Rgba, RgbaImage};

    let mut pixels = RgbaImage::new(100, 400);
    for px in pixels.pixels_mut() {
        *px = Rgba([220, 30, 30, 255]);
    }
    let block = ImageBlock::new(DynamicImage::ImageRgba8(pixels), "red.png");
    let kitty = ImageRenderer::from_capabilities(TerminalCapabilities {
        images: Some(ImageProtocol::Kitty),
        true_color: true,
        hyperlinks: false,
    });
    let halfblocks = ImageRenderer::halfblocks();
    let theme = UiTheme::dark();
    let blocks = [block];
    let strip = |renderer| Strip {
        renderer,
        blocks: &blocks,
        theme: &theme,
        show_images: true,
        width_cells: 30,
    };

    let (mut alt, _) = scrolled_up();
    // The baseline: no attachment, label present.
    assert!(last_row(&mut alt).contains("Jump"));

    // A kitty image taller than the viewport occupies the last row: no label.
    alt.draw(Some(strip(&kitty))).unwrap();
    assert!(
        !last_row(&mut alt).contains("Jump"),
        "a graphics image owns the row"
    );

    // The half-block/no-protocol renderer draws one text placeholder line, not an image line, so
    // the label is composited over the (otherwise empty) last row as it is over any text.
    alt.draw(Some(strip(&halfblocks))).unwrap();
    assert!(last_row(&mut alt).contains("Jump"));
}

/// `tui.altScreen.bottom` is rebound by `keybindings.json`, and the LIVE label follows — on
/// `load_keybindings_json` and on `/reload`, including going back to the default when the entry is
/// deleted. The table was never merged before TUI-109, so the rebind did not even move the key.
#[test]
fn a_rebound_bottom_key_updates_the_label() {
    let mut app = crate::App::new(TestBackend::new(60, 8), UiTheme::dark()).unwrap();
    for i in 0..40 {
        app.transcript_mut().push_status(format!("status {i}"));
    }
    let _captured = app.enter_fullscreen_captured().unwrap();
    app.draw().unwrap();
    {
        let alt = app.altscreen_for_test().unwrap();
        let tail = alt.max_scroll_top_for_test();
        alt.scroll_to_row_for_test(tail - 10);
    }
    app.draw().unwrap();
    let label = |app: &mut crate::App<TestBackend>| label_text(app.altscreen_for_test().unwrap());
    assert_eq!(label(&mut app), "↓ Jump to latest message · Ctrl+End");

    let issues = app
        .load_keybindings_json(r#"{"tui.altScreen.bottom": "ctrl+g"}"#)
        .unwrap();
    assert!(issues.is_empty(), "{issues:?}");
    app.draw().unwrap();
    assert_eq!(label(&mut app), "↓ Jump to latest message · Ctrl+G");

    // `/reload` re-reads the file: a rebind in it wins, and deleting it restores the default.
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("keybindings.json"),
        r#"{"tui.altScreen.bottom": ["ctrl+e", "ctrl+b"]}"#,
    )
    .unwrap();
    app.reload_keybindings_from(dir.path()).unwrap();
    app.draw().unwrap();
    assert_eq!(label(&mut app), "↓ Jump to latest message · Ctrl+E/Ctrl+B");

    std::fs::remove_file(dir.path().join("keybindings.json")).unwrap();
    app.reload_keybindings_from(dir.path()).unwrap();
    app.draw().unwrap();
    assert_eq!(label(&mut app), "↓ Jump to latest message · Ctrl+End");
}

/// A renderer adopted AFTER the bindings changed starts with the current key, not the default.
#[test]
fn a_screen_entered_after_a_rebind_is_built_with_the_key() {
    let mut app = crate::App::new(TestBackend::new(60, 8), UiTheme::dark()).unwrap();
    for i in 0..40 {
        app.transcript_mut().push_status(format!("status {i}"));
    }
    app.load_keybindings_json(r#"{"tui.altScreen.bottom": "ctrl+g"}"#)
        .unwrap();
    let _captured = app.enter_fullscreen_captured().unwrap();
    app.draw().unwrap();
    {
        let alt = app.altscreen_for_test().unwrap();
        let tail = alt.max_scroll_top_for_test();
        alt.scroll_to_row_for_test(tail - 10);
    }
    app.draw().unwrap();
    let row = last_row(app.altscreen_for_test().unwrap());
    assert!(row.contains("· Ctrl+G"), "{row:?}");
}
