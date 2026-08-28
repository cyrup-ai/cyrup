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

use ratatui::crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::scroll::{self, ScrollState};

/// Document rows one wheel notch moves — pi's `wheelScrollLines`, clamped to at least one line and
/// defaulting to one (`tui-alt-screen.ts:220`) from the option at `:147-148`.
///
/// A constant rather than a field because cyrup's composition root, like upstream's, passes no
/// override: the alternate screen is constructed with `searchMatchStyle`, `openUrl`,
/// `onRightClickPaste` and `copySelection` only (`interactive-mode.ts:372-384`).
pub(super) const WHEEL_SCROLL_LINES: i32 = 1;

/// The signed line count a wheel event requests, or `None` when the event is not a vertical
/// notch — pi's `direction` (`tui-alt-screen.ts:653-656`), already multiplied by
/// [`WHEEL_SCROLL_LINES`] as `routeWheel` does on its first line (`:676`).
///
/// Negative is **up**, matching [`crate::ViewportRenderer::scroll_by`] and pi's
/// `direction === 0 ? -1 : 1` for the wheel-up button (`:656`, `:667`).
///
/// [`MouseEventKind::ScrollLeft`] and [`MouseEventKind::ScrollRight`] are `None`, which is
/// upstream's `if (direction !== 0 && direction !== 1) return undefined` (`:654`, `:665`): a
/// horizontal notch is not a scroll here, and returning `None` is what lets [`route`] report the
/// event as unconsumed so the caller can offer it to the handlers that follow.
fn notch(kind: MouseEventKind) -> Option<i32> {
    match kind {
        MouseEventKind::ScrollUp => Some(-WHEEL_SCROLL_LINES),
        MouseEventKind::ScrollDown => Some(WHEEL_SCROLL_LINES),
        MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight
        | MouseEventKind::Down(_)
        | MouseEventKind::Up(_)
        | MouseEventKind::Drag(_)
        | MouseEventKind::Moved => None,
    }
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
pub(super) fn route(scroll: &mut ScrollState, viewport: Rect, ev: &MouseEvent) -> bool {
    let Some(lines) = notch(ev.kind) else {
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
