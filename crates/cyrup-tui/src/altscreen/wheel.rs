//! **Wheel routing** — what one mouse-wheel notch does to the alternate screen, and the overscroll
//! *chaining* rule that decides which scroll view moves. cyrup's port of pi's
//! `TuiAltScreen.routeWheel` (`packages/tui/src/tui-alt-screen.ts:675-686` @v0.84.3) together with
//! the direction half of `parseWheelEvent` (`:648-673`). ADR-0005 §Decision B-6, whose `:462-501`
//! citation is the @v0.84.1 line numbering for the same two methods.
//!
//! # What crossterm already did
//! Upstream parses the report itself, from both the SGR (`\x1b[<b;x;yM`) and the legacy X10
//! (`\x1b[M`) encodings, recognising a wheel by `button & 64` and its direction by `button & 3`
//! (`:649-671`); it also converts the 1-based coordinates a terminal sends into the 0-based ones
//! the layout is measured in (`:657-658`, `:668-669`). cyrup receives a
//! [`MouseEvent`] that crossterm has already decoded from
//! either encoding, with `column`/`row` already 0-based, so all that survives the port is the
//! direction mapping in [`notch`] — and pi's refusal to treat a *horizontal* notch as a scroll
//! (`direction !== 0 && direction !== 1` at `:654`, `:665`, which drops the report through to the
//! ordinary mouse path).
//!
//! # Chaining, and why cyrup's single view still needs it
//! Upstream walks the scroll views under the pointer innermost-first (`layout.ts:400-410` sorts by
//! descending depth), hands each the part of the notch the previous one refused, and stops at the
//! first view that either consumed it all or is `overscroll: "contain"` (`:678-682`). Whatever is
//! still unconsumed then goes to the *primary* view, unless the walk already offered it there
//! (`:684`). The refusal itself is a return value, not an exception: `ScrollView.scrollBy` answers
//! with `requested - moved` (`components/scroll-view.ts:153`), which is
//! [`scroll::scroll_by_remaining`] here.
//!
//! cyrup has exactly one scroll view — the implicit, `primary`, `follow: "end"` view over the
//! retained document (`:218`), whose `overscroll` is the constructor default `"chain"`
//! (`components/scroll-view.ts:49`). Both of upstream's branches therefore lead to the same view,
//! which is why a notch outside it still scrolls the transcript: that is the `:684` fallback, not a
//! shortcut. The two-branch shape is kept because it is the *rule* B-6 owns, and because a second,
//! nested view added later (a scrollable overlay, a diff pane) becomes a hit-test entry rather than
//! a rewrite of this file.
//!
//! # This module owns no state
//! It reads a [`ScrollState`] and the [`Rect`] the caller laid the view out at, and mutates only
//! through [`scroll`]'s mutators. Two neighbouring concerns are deliberately *not* here: the
//! overlay deferral that lets a popup keep the wheel (`shouldDeferViewportInputToOverlay` at
//! `:566`) and the scrollbar-hover refresh upstream performs on the way out (`:685`) belong to
//! ADR-0005 §B-3's dispatcher and §B-7's hit-testing respectively, and both sit *around* this call
//! rather than inside it.
//!
//! # The inline renderer is untouched
//! Nothing here is reachable from regular mode. Mouse reporting is enabled only by
//! [`super::mouse::MouseSetup`], which only the alternate screen arms, and the reader arm drops
//! every report while it is disarmed (`super::mouse::map_reader_event`) — so an inline session
//! routes no wheel event, exactly as before ADR-0005 (R-ARCH-TUI-003: native scrollback is what
//! scrolls there).

use std::time::Instant;

use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::scroll::{self, ScrollState};
use super::wheel_scroll::{Direction, WheelAccelerator};

/// How many times faster a notch scrolls while Alt is held — pi's `ALT_WHEEL_SCROLL_MULTIPLIER`
/// (`tui-alt-screen.ts:75` @v0.87.1, #9166).
const ALT_WHEEL_SCROLL_MULTIPLIER: i32 = 5;

/// The signed line count a wheel event requests, or `None` when the event is not a vertical
/// notch — pi's `direction` (`tui-alt-screen.ts:653-656`) times the row count of `handleInput`
/// (`:698-701` @v1.0.0):
///
/// ```text
/// lines = this.wheelScroll.next(direction, performance.now());
/// delta = direction * ((button & 8) !== 0 ? lines * ALT_WHEEL_SCROLL_MULTIPLIER : lines);
/// ```
///
/// The accelerator decides `lines` from the `fullscreenWheelScrollLines` setting and the event's
/// timing ([`WheelAccelerator::next`]); the Alt multiplier is applied to ITS answer, never folded
/// into the setting, so Alt is always five times whatever the accelerator said. crossterm decodes
/// the SGR/X10 button code's bit 3 (value 8) into [`KeyModifiers::ALT`] on the event it hands
/// cyrup (`crossterm-0.29.0/src/event/sys/unix/parse.rs:802-803`).
///
/// Negative is **up**, matching [`crate::ViewportRenderer::scroll_by`] and pi's
/// `direction === 0 ? -1 : 1` for the wheel-up button (`:656`, `:667`).
///
/// [`MouseEventKind::ScrollLeft`] and [`MouseEventKind::ScrollRight`] are `None`, which is
/// upstream's `if (direction !== 0 && direction !== 1) return undefined` (`:654`, `:665`): a
/// horizontal notch is not a scroll here — and never reaches the accelerator, so it cannot start
/// or extend a gesture — and returning `None` is what lets [`route`] report the event as
/// unconsumed so the caller can offer it to the handlers that follow.
fn notch(
    kind: MouseEventKind,
    modifiers: KeyModifiers,
    accelerator: &mut WheelAccelerator,
    now: Instant,
) -> Option<i32> {
    let direction = match kind {
        MouseEventKind::ScrollUp => Direction::Up,
        MouseEventKind::ScrollDown => Direction::Down,
        MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight
        | MouseEventKind::Down(_)
        | MouseEventKind::Up(_)
        | MouseEventKind::Drag(_)
        | MouseEventKind::Moved => return None,
    };
    let lines = accelerator.next(direction, now);
    let lines = if modifiers.contains(KeyModifiers::ALT) {
        lines.saturating_mul(ALT_WHEEL_SCROLL_MULTIPLIER)
    } else {
        lines
    };
    Some(match direction {
        Direction::Up => lines.saturating_neg(),
        Direction::Down => lines,
    })
}

/// Offer `ev` to the scroll view laid out at `viewport`, returning whether it was a wheel event —
/// pi's `routeWheel(event)` (`tui-alt-screen.ts:675-686`) over the parse at `:648-673`.
///
/// `viewport` is the *whole* box the view occupies, including any column an
/// [`always`](scroll::ScrollbarMode::Always) scrollbar reserves inside it: upstream hit-tests
/// `box.rect` (`layout.ts:404`), which is the box the bar is drawn in the last column of
/// (`:280`).
///
/// The return value is "consumed", ADR-0005 §B-3's dispatcher precedence. `true` for a vertical
/// notch **even when nothing moved** — upstream answers `{ consume: true }` for every parsed wheel
/// event, whatever the views did with it (`:565-568`) — and `false` for every other kind, including
/// a horizontal notch, which upstream lets fall through to its ordinary mouse handling
/// (`:570-576`).
///
/// # Scrollbar activity
/// No mark is taken here. [`scroll::scroll_by_remaining`] marks one whenever the offset actually
/// moved, which is upstream's `if (moved !== 0) this.markScrollbarActivity()`
/// (`components/scroll-view.ts:151`) — so a notch that scrolls raises the `auto` thumb and restarts
/// its fade, and a notch refused at an edge raises nothing, exactly as upstream. Marking
/// unconditionally here would flash a bar up for a wheel the document cannot honour, which pi does
/// not do.
pub(super) fn route(
    scroll: &mut ScrollState,
    accelerator: &mut WheelAccelerator,
    viewport: Rect,
    ev: &MouseEvent,
    now: Instant,
) -> bool {
    let Some(lines) = notch(ev.kind, ev.modifiers, accelerator, now) else {
        return false;
    };
    // `getScrollViewsAt(this.currentLayout, event.x, event.y)` (`:678`) reduced to cyrup's one
    // view: `containsPoint(box.rect, x, y)` (`layout.ts:384-386`) is `Rect::contains`, to the
    // half-open bound.
    let over_view = viewport.contains(Position::new(ev.column, ev.row));
    // `remaining = scrollView.scrollBy(remaining)` (`:680`). The loop's `break` conditions
    // (`:681`) are both already met after this single view: it consumed what it could, and it is
    // the last one under the pointer.
    let remaining = if over_view {
        scroll::scroll_by_remaining(scroll, lines)
    } else {
        lines
    };
    // `if (remaining !== 0 && !seen.has(primary)) primary.scrollBy(remaining);` (`:684`) — the
    // chained overflow lands on the primary view. Here that is the same view the walk would have
    // used, so the guard reads as "the pointer was not over it", and the remainder it refuses in
    // turn is discarded because there is nothing further out to chain into.
    if remaining != 0 && !over_view {
        let _ = scroll::scroll_by_remaining(scroll, remaining);
    }
    true
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use std::time::Duration;

    use cyrup_config::settings::WheelScrollLines;
    use ratatui::crossterm::event::MouseButton;

    use super::*;

    /// A 100-row document in a 10-row viewport, parked at row 50 so a notch can move either way.
    fn parked() -> ScrollState {
        let mut state = ScrollState::default();
        scroll::update_layout(&mut state, 100, 10);
        scroll::scroll_to_row(&mut state, 50);
        state
    }

    /// The document `Rect` — deliberately SHORTER than any terminal it would sit in, with an input
    /// dock below it: the wheel code takes the area as a parameter and never reads a terminal size.
    const VIEWPORT: Rect = Rect::new(0, 0, 30, 10);

    fn event(kind: MouseEventKind, modifiers: KeyModifiers, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers,
        }
    }

    fn at(origin: Instant, ms: u64) -> Instant {
        origin + Duration::from_millis(ms)
    }

    fn fixed(n: f64) -> WheelScrollLines {
        WheelScrollLines::from_number(n)
    }

    /// Route one event through a fresh-enough state and return the offset it leaves.
    fn notch_at(
        state: &mut ScrollState,
        acc: &mut WheelAccelerator,
        kind: MouseEventKind,
        modifiers: KeyModifiers,
        now: Instant,
    ) -> usize {
        route(state, acc, VIEWPORT, &event(kind, modifiers, 1, 1), now);
        scroll::scroll_top(state)
    }

    /// `applies runtime wheel line count updates` (`test/tui-alt-screen.test.ts:508-529` @v1.0.0):
    /// a fixed 3 moves 3, `setLines(2)` takes effect on the next event, and Alt keeps its
    /// multiplier on top of whichever count is in force — `[-3, 2, -10]`.
    #[test]
    fn a_fixed_count_is_the_row_count_and_alt_multiplies_it() {
        let t0 = Instant::now();
        let mut state = parked();
        let mut acc = WheelAccelerator::new(fixed(3.0), true);
        let none = KeyModifiers::NONE;

        assert_eq!(
            notch_at(&mut state, &mut acc, MouseEventKind::ScrollUp, none, t0),
            47
        );
        acc.set_lines(fixed(2.0));
        assert_eq!(
            notch_at(&mut state, &mut acc, MouseEventKind::ScrollDown, none, t0),
            49,
            "the new count applies at once"
        );
        assert_eq!(
            notch_at(
                &mut state,
                &mut acc,
                MouseEventKind::ScrollUp,
                KeyModifiers::ALT,
                t0
            ),
            39,
            "Alt is 5 x the count in force: 2 x 5"
        );
    }

    /// The Alt multiplier is applied AFTER the accelerator (`tui-alt-screen.ts:698-701`), not folded
    /// into the setting: in `auto` mode on a terminal that does not pre-accelerate, notches 20 ms
    /// apart are 1 then 5 rows, and with Alt held 5 then 25.
    #[test]
    fn alt_multiplies_the_accelerated_count() {
        let t0 = Instant::now();
        let mut state = parked();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        let alt = KeyModifiers::ALT;

        assert_eq!(
            notch_at(
                &mut state,
                &mut acc,
                MouseEventKind::ScrollUp,
                alt,
                at(t0, 0)
            ),
            45,
            "first event of a gesture: 1 x 5"
        );
        assert_eq!(
            notch_at(
                &mut state,
                &mut acc,
                MouseEventKind::ScrollUp,
                alt,
                at(t0, 20)
            ),
            20,
            "20 ms gap: 5 rows x Alt 5 = 25"
        );
    }

    /// Without Alt the same gesture is 1 then 5 — the accelerator reaches the scroll offset
    /// through `route`, not only through its own unit tests.
    #[test]
    fn auto_mode_accelerates_a_fast_spin_through_the_router() {
        let t0 = Instant::now();
        let mut state = parked();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        for (ms, expected_top) in [(0, 49), (20, 44), (40, 39)] {
            assert_eq!(
                notch_at(
                    &mut state,
                    &mut acc,
                    MouseEventKind::ScrollUp,
                    KeyModifiers::NONE,
                    at(t0, ms)
                ),
                expected_top,
                "after the event at {ms} ms"
            );
        }
    }

    /// A terminal that already accelerates the wheel keeps `auto` at one row per event however
    /// quickly they arrive (`accelerate = false`).
    #[test]
    fn auto_mode_is_one_row_when_the_terminal_pre_accelerates() {
        let t0 = Instant::now();
        let mut state = parked();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, false);
        for ms in [0, 20, 40] {
            notch_at(
                &mut state,
                &mut acc,
                MouseEventKind::ScrollUp,
                KeyModifiers::NONE,
                at(t0, ms),
            );
        }
        assert_eq!(scroll::scroll_top(&state), 47);
    }

    /// A horizontal notch and a button press are declined and never reach the accelerator, so
    /// neither can start or extend a gesture: the vertical event after them still sees a 20 ms gap
    /// to the one before.
    #[test]
    fn declined_events_do_not_feed_the_accelerator() {
        let t0 = Instant::now();
        let mut state = parked();
        let mut acc = WheelAccelerator::new(WheelScrollLines::Auto, true);
        let none = KeyModifiers::NONE;
        notch_at(
            &mut state,
            &mut acc,
            MouseEventKind::ScrollDown,
            none,
            at(t0, 0),
        );
        for (kind, ms) in [
            (MouseEventKind::ScrollLeft, 10),
            (MouseEventKind::Down(MouseButton::Left), 12),
        ] {
            assert!(!route(
                &mut state,
                &mut acc,
                VIEWPORT,
                &event(kind, none, 1, 1),
                at(t0, ms)
            ));
        }
        notch_at(
            &mut state,
            &mut acc,
            MouseEventKind::ScrollDown,
            none,
            at(t0, 20),
        );
        assert_eq!(
            scroll::scroll_top(&state),
            56,
            "1 row, then 5 rows after a 20 ms gap"
        );
    }

    /// A notch over the dock BELOW the document rect still scrolls the transcript — pi's "Wheel
    /// over the dock falls back to the primary transcript scroll view"
    /// (`test/tui-alt-screen.test.ts:251`) — by the accelerator's count.
    #[test]
    fn a_notch_below_the_document_rect_scrolls_the_primary_view_by_the_same_count() {
        let t0 = Instant::now();
        let mut state = parked();
        let mut acc = WheelAccelerator::new(fixed(4.0), true);
        // Row 12 is outside the 10-row document rect: it is where an input dock would be painted.
        let ev = event(MouseEventKind::ScrollUp, KeyModifiers::NONE, 5, 12);
        assert!(route(&mut state, &mut acc, VIEWPORT, &ev, t0));
        assert_eq!(scroll::scroll_top(&state), 46);
    }
}
