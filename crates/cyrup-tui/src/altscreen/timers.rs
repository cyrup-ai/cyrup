//! The alternate screen's **timer surface** — the deadlines the run loop must wake on, and the
//! tick that services them. cyrup's replacement for the three per-component schedulers pi arms
//! inside `packages/tui/src/tui-alt-screen.ts` @v0.84.3 and its components. ADR-0005 §Decision
//! B-14.
//!
//! # Why this module exists at all
//! Upstream is free to arm a timer wherever it likes: a flash entry gets its own `setTimeout`
//! (`components/alt-screen-flash.ts:24-33`), the transient scrollbar gets another
//! (`components/scroll-view.ts:98-103`), and a selection drag held past the viewport edge gets a
//! `setInterval` (`tui-alt-screen.ts:949-951`). Each callback ends in `requestRender`. cyrup has
//! no per-component scheduler and exactly one place that may sleep — the `select!` in
//! `app/run.rs` — so every sibling that would have armed a timer instead records the [`Instant`]
//! it wants to be woken at, and something has to fold those into the single deadline that loop can
//! arm. That fold is [`next_deadline`]; the callback bodies behind it are [`tick`].
//!
//! Without this module all three behaviours are silently absent rather than broken: a flash
//! lingers until some unrelated event forces a frame, an `auto` scrollbar never fades, and a drag
//! held motionless against the edge stops extending. None of the three is observable as a crash,
//! which is exactly why the aggregation is worth naming rather than open-coding in the loop.
//!
//! # The contract with the loop
//! Two calls per iteration, and they are not interchangeable:
//!
//! 1. Before sleeping, [`next_deadline`] answers the earliest instant any sibling wants. `None`
//!    means nothing is pending, and the loop's timer arm stays `std::future::pending()` — the
//!    idiom every optional arm in `app/run.rs:216-248` already uses, so an idle alternate screen
//!    costs no wakeups at all.
//! 2. On waking — from that deadline **or** from anything else, since a key or a mouse report may
//!    arrive first — [`tick`] services whatever has come due and answers whether the frame is now
//!    stale. `true` obliges the caller to repaint.
//!
//! That obligation is load-bearing for one of the three. The scrollbar's deadline disarms itself
//! on the clock ([`super::scroll::next_hide`] returns `None` the moment the bar is no longer
//! visible) and the auto-scroll re-arms or stops itself inside
//! [`super::selection::tick_auto_scroll`] — but a flash is pruned by
//! [`super::flash::overlay`], on the paint. A caller that ignores a `true` therefore keeps being
//! handed the same elapsed deadline; the repaint is what retires it. That is upstream's shape, not
//! a shortcut: pi's timer callback also splices the entry out *and* requests a render
//! (`components/alt-screen-flash.ts:26-29`, `:32`), and cyrup splits those halves across the tick
//! and the frame it provokes.
//!
//! # State by parameter, never by `&mut AltScreen`
//! Every function here takes the individual pieces it touches rather than the renderer that owns
//! them — `altscreen/mod.rs` rule 1, and here for a second reason besides: `AltScreen`'s own tick
//! method needs to hand out a `&mut` to two of its fields and a `&` to two others at once, which
//! is a borrow of disjoint fields the compiler accepts and a `&mut self` forwarded into a helper
//! is not. That exact conflict has already cost this module one fix.
//!
//! # What is deliberately *not* a deadline here
//! [`super::selection::DOUBLE_CLICK_INTERVAL`] (pi's `DOUBLE_CLICK_INTERVAL_MS`,
//! `tui-alt-screen.ts:68`) is a comparison made when the *next* press arrives, not something that
//! must fire on its own: a double-click window that expires with no second press changes nothing
//! on screen, so waking for it would be a wakeup with no frame behind it. Upstream arms no timer
//! for it either (`:906`).

use std::time::Instant;

use ratatui::layout::Rect;
use ratatui::text::Line;

use super::{flash, scroll, selection};

/// The earliest instant any sibling wants the loop to wake at, or `None` when nothing is pending.
///
/// The three sources are the three `setTimeout`/`setInterval` sites upstream owns, in the order the
/// module doc names them: [`flash::next_expiry`] (`components/alt-screen-flash.ts:24-33`),
/// [`scroll::next_hide`] (`components/scroll-view.ts:98-103`) and
/// [`selection::next_auto_scroll`] (`tui-alt-screen.ts:949-951`). Each already answers `None` when
/// its own behaviour is idle, so the minimum over the live ones is the whole computation — and a
/// screen with no flash queued, no fading `auto` bar and no drag against an edge returns `None`,
/// which is the loop's `pending()` arm and no wakeup at all.
///
/// Cheap enough to call every iteration: the flash queue is walked (it holds at most a handful of
/// notices), and the other two are field reads behind a mode test.
pub(super) fn next_deadline(
    flashes: &flash::FlashStack,
    bar: &scroll::ScrollbarView,
    scroll: &scroll::ScrollState,
    sel: &selection::SelectionState,
) -> Option<Instant> {
    [
        flash::next_expiry(flashes),
        scroll::next_hide(bar, scroll),
        selection::next_auto_scroll(sel),
    ]
    .into_iter()
    .flatten()
    .min()
}

/// Service whatever has come due, answering whether the screen needs repainting — the three timer
/// callback bodies behind [`next_deadline`], each of which ends in upstream's `requestRender`
/// (`components/alt-screen-flash.ts:32`, `components/scroll-view.ts:101`,
/// `tui-alt-screen.ts:969`).
///
/// `area` is the whole frame, as [`super::AltScreen::handle_mouse`] takes it; the viewport the
/// document was painted into is derived from it with [`scroll::content_width`] so the rows the
/// auto-scroll re-resolves the pointer over are the rows the user can see.
///
/// Safe to call on every wake, not only on a timer wake: with nothing armed it is three
/// comparisons and a `false`, and each of the three checks its own deadline rather than trusting
/// the caller to have waited for it.
///
/// # Order
/// The two passive deadlines are read against a single `now` **before** the auto-scroll runs,
/// because a step that moves the document calls [`scroll::mark_activity`] — reading the scrollbar
/// afterwards would see the freshly re-armed activity and miss a fade that was already due at the
/// instant the loop woke.
///
/// # What each arm does, and does not, mutate
/// * **Flash** — nothing. The queue is pruned by [`flash::overlay`] on the paint this `true`
///   provokes (see the module doc), which is why the stack arrives by `&`.
/// * **Scrollbar** — nothing. [`scroll::is_visible`] reads the clock, so the bar is already gone
///   as far as the next frame is concerned; the deadline exists only to make that frame happen.
/// * **Auto-scroll** — [`selection::tick_auto_scroll`] moves the offset, re-resolves the selection
///   focus under the stored pointer and re-arms or stops itself, exactly as pi's
///   `autoScrollSelection` does (`tui-alt-screen.ts:954-970`).
pub(super) fn tick(
    flashes: &flash::FlashStack,
    bar: &scroll::ScrollbarView,
    sel: &mut selection::SelectionState,
    scroll: &mut scroll::ScrollState,
    doc: &[Line<'_>],
    area: Rect,
) -> bool {
    let now = Instant::now();
    // A notice whose deadline has passed is still in the queue and still painted; the frame this
    // provokes is what drops it (`components/alt-screen-flash.ts:26-29`).
    let flash_due = flash::next_expiry(flashes).is_some_and(|at| at <= now);
    // An `auto` bar past its hide delay is already invisible to `scroll::is_visible`, so this
    // fires once and `scroll::next_hide` answers `None` from here on.
    let bar_due = scroll::next_hide(bar, scroll).is_some_and(|at| at <= now);
    let viewport = Rect {
        width: scroll::content_width(bar, area.width),
        ..area
    };
    let scrolled = selection::tick_auto_scroll(sel, scroll, doc, viewport);
    flash_due || bar_due || scrolled
}
