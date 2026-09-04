//! Making the scrollbar **interactive** — hit test, hover and thumb drag, cyrup's port of pi's
//! `getScrollbarTargetAt` / `setScrollbarHover` / `handleScrollbarMouseEvent`
//! (`packages/tui/src/tui-alt-screen.ts` @v0.84.3: `:718-733`, `:735-748`, `:750-795`). ADR-0005
//! §Decision B-7.
//!
//! # Why any of this is application work
//! [`ratatui::widgets::Scrollbar`] *draws* a thumb and answers no question about it: it has no
//! pointer model, no hit test and no drag. Everything a user does to a scrollbar — hovering it,
//! grabbing it, sliding it, clicking beside it — is code someone has to write, which is why
//! upstream writes it too. This module is that code, and nothing else: it never paints (the thumb
//! is [`super::scroll::draw`]'s), never derives thumb geometry of its own (it asks
//! [`super::scroll::geometry`], the single derivation both the paint and the hit test share) and
//! never moves the offset by hand (it calls [`super::scroll::scroll_to_row`] and
//! [`super::scroll::scroll_by`]).
//!
//! # Named for what it drags
//! `scrollbar_drag`, not `drag`: ADR-0005 §B-8 owns a second, unrelated pointer drag — the text
//! selection one (`tui-alt-screen.ts:943-1039`) — and upstream keeps the two apart by prefix
//! (`scrollbarDrag` at `:192` against `selectionDragPointer` at `:188`). The two are mutually
//! exclusive by construction, and this is the one that wins: upstream offers every mouse report to
//! the scrollbar first and only passes it on when the scrollbar declined it (`:573-575`).
//!
//! # The parameters ADR-0005 §B-3 will eventually bundle
//! The renderer's `AltUi` bag does not exist yet (§B-3 lands after this unit in the file order the
//! module doc records), so the three pieces of state a scrollbar interaction touches arrive as
//! three arguments rather than as one `&mut AltUi`. That is exactly the destructure §B-3 performs
//! at its call site, so the signatures below are stable across its arrival: a
//! [`super::scroll::ScrollState`], the [`super::scroll::ScrollbarView`] beside it, and this
//! module's own [`DragState`]. The viewport [`Rect`] is an argument for the same reason it is one
//! on [`super::scroll::draw`] — the renderer knows the area, this module only asks about it.
//!
//! # The contract the dispatcher owes this module
//! Upstream's mouse arm is four lines (`:571-576`), and two of them are ordering rules that cannot
//! live here:
//!
//! 1. **Wheel first.** `parseWheelEvent` runs before `parseSgrMouseEvent` (`:565-575`), so a wheel
//!    notch never reaches the scrollbar at all. [`route`] therefore declines every scroll kind
//!    rather than treating it as pointer motion.
//! 2. **Scrollbar before selection**, and a consumed report is not offered on: `handled` gates the
//!    call to `handleSelectionMouseEvent` (`:575`). Upstream additionally *clears* every field of
//!    the in-flight selection when a grab starts (`:776-784`); cyrup's equivalent is one line in
//!    §B-3's dispatcher — a `true` from [`route`] means the pointer belongs to the scrollbar, so
//!    §B-8's `selection::cancel` runs and the report is not routed to selection. Cancelling on
//!    every consumed report rather than only on the grab is equivalent: a drag holds the pointer
//!    for its whole life, so no selection can exist to cancel after the first one.

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::scroll::{
    ScrollState, ScrollbarGeom, ScrollbarView, geometry, scroll_by, scroll_to_row, set_hover,
    viewport_height,
};

/// A live thumb drag — pi's `ScrollbarDrag` (`tui-alt-screen.ts:117-120`), held in the renderer's
/// UI bag as upstream holds `scrollbarDrag?: ScrollbarDrag` (`:192`).
///
/// Upstream's `scrollView` field names *which* view is being dragged, because its layout can hold
/// several (`getScrollViewsAt`, `:721`). cyrup's alternate screen has exactly one scroll view — the
/// transcript (`interactive-mode.ts:918-923`) — so the field would name the only candidate and is
/// dropped; what remains is upstream's `grabOffset`, and `None` is upstream's `undefined`.
///
/// The offset is the whole of the fix for the defect this type exists to prevent: without it, a
/// drag maps the pointer row straight onto the thumb *top* and the thumb jumps so its first row
/// lands under the cursor. Recording where inside the thumb the grab happened, and subtracting it
/// on every motion, is what keeps the grabbed row under the pointer for the life of the drag
/// (`:764`).
#[derive(Default)]
pub(super) struct DragState {
    /// Rows between the thumb's first row and the row the press landed on — upstream's
    /// `grabOffset` (`:119`, set at `:788`). `None` when no drag is in flight.
    grab_offset: Option<u16>,
}

/// Whether a thumb drag currently holds the pointer — upstream's `if (this.scrollbarDrag)` test
/// (`:574`, `:751`).
///
/// The renderer reads it for the same reason upstream does: while a drag is live the pointer
/// belongs to the scrollbar, so hover is not re-derived from the pointer position and no other
/// consumer may claim the report.
pub(super) fn is_dragging(drag: &DragState) -> bool {
    drag.grab_offset.is_some()
}

/// Offer one decoded mouse report to the scrollbar, returning whether it was **consumed** — pi's
/// `handleScrollbarMouseEvent` (`tui-alt-screen.ts:750-791`) together with the hover refresh its
/// caller performs immediately afterwards (`:574`).
///
/// `area` is the scroll viewport, the same [`Rect`] [`super::scroll::draw`] paints the thumb into;
/// the geometry both agree on comes from [`super::scroll::geometry`], so the rows this hit-tests
/// are the rows the user can see.
///
/// The four outcomes, in upstream's order:
///
/// 1. **A drag is live and the button came up** — the drag ends and the report is consumed
///    (`:752-755`).
/// 2. **A drag is live and the pointer moved** — the offset follows the pointer through
///    [`offset_for_pointer`], preserving the grab (`:756-770`).
/// 3. **No drag, and a left press landed on the thumb** — the grab offset is recorded, hover is
///    pinned, and the report is consumed (`:773-790`).
/// 4. **Anything else** — declined, so §B-8's selection sees it (`:773`, `:775`). A press outside
///    the bar's own column, a release with no drag, a non-left button and every motion report with
///    no drag all land here, which is upstream's `(event.button & 32) !== 0` motion test and its
///    `(event.button & 3) !== 0` button test expressed as [`MouseEventKind`] arms.
///
/// The one cyrup addition is inside case 3 — see [`page_toward`] for the trough press upstream
/// does not have.
pub(super) fn route(
    drag: &mut DragState,
    bar: &mut ScrollbarView,
    scroll: &mut ScrollState,
    ev: &MouseEvent,
    area: Rect,
) -> bool {
    let consumed = handle(drag, bar, scroll, ev, area);
    // `if (!this.scrollbarDrag) this.updateScrollbarHover(event.x, event.y);` (`:574`) — refreshed
    // AFTER the handler, so a release that just ended a drag re-derives hover from where the
    // pointer actually is, and a press that just started one keeps the hover the grab pinned.
    if !is_dragging(drag) {
        update_hover(bar, scroll, ev.column, ev.row, area);
    }
    consumed
}

/// Re-derive hover from a pointer position — pi's `updateScrollbarHover(x, y)` (`:742-744`) over
/// `getScrollbarTargetAt` (`:718-733`) and `setScrollbarHover` (`:735-740`).
///
/// Exposed because [`route`] is not the only caller upstream has: `routeWheel` refreshes hover
/// after every notch (`:685`), since scrolling moves the thumb out from under a stationary pointer.
/// ADR-0005 §B-6 is that call site.
///
/// Hover is the pointer being **on the thumb**, not merely in the bar's column: upstream's target
/// test is `y >= thumbTop && y < thumbTop + thumbHeight` (`:727-729`). It is also only ever true
/// while the bar is visible at all, because [`super::scroll::geometry`] returns `None` otherwise
/// (`layout.ts:267`) — so a faded `auto` bar is not revealed by a pointer passing over where it
/// would have been, which is upstream's behaviour and not an omission.
///
/// What hover *does* is upstream's `setScrollbarActive` (`components/scroll-view.ts:113-117`): an
/// `auto` bar under the pointer stops fading (`:97`) and stays up for as long as the pointer holds
/// it. The thumb's colour is not part of it — upstream paints every thumb with the one
/// `scrollbarStyle` (`layout.ts:300`, `components/scroll-view.ts:51`) whether it is hovered or not.
pub(super) fn update_hover(
    bar: &mut ScrollbarView,
    scroll: &mut ScrollState,
    column: u16,
    row: u16,
    area: Rect,
) {
    let on_thumb = geometry(bar, scroll, area).is_some_and(|geom| hits_thumb(&geom, column, row));
    set_hover(bar, scroll, on_thumb);
}

/// Drop a live drag and the hover with it — pi's `stopScrollbarDrag` (`:793-795`) and
/// `stopScrollbarHover` (`:746-748`), which upstream always calls as a pair: on `FOCUS_OUT`
/// (`:548-549`), on entering the alternate screen (`:260-261`) and on leaving it (`:301-302`).
///
/// The focus-loss call is what closes the failure this unit would otherwise ship: a pointer that
/// leaves the window mid-drag sends no release, so without an explicit cancel the grab outlives
/// the gesture and the next unrelated motion report slides the document. `?1004h` — the focus
/// reporting ADR-0005 §B-4 asks the terminal for, and the third reason it does not use crossterm's
/// mouse capture (`altscreen/mouse.rs`) — is what delivers the event this arm reads.
///
/// Idempotent, so §B-3's `FocusLost` arm may call it unconditionally alongside §B-8's own cancel:
/// clearing an absent drag is a no-op and [`super::scroll::set_hover`] returns early when the state
/// already matches (`components/scroll-view.ts:114`).
pub(super) fn cancel(drag: &mut DragState, bar: &mut ScrollbarView, scroll: &mut ScrollState) {
    drag.grab_offset = None;
    set_hover(bar, scroll, false);
}

/// The body of [`route`] without its trailing hover refresh — pi's `handleScrollbarMouseEvent`
/// proper (`tui-alt-screen.ts:750-791`).
fn handle(
    drag: &mut DragState,
    bar: &mut ScrollbarView,
    scroll: &mut ScrollState,
    ev: &MouseEvent,
    area: Rect,
) -> bool {
    if let Some(grab) = drag.grab_offset {
        return drive(drag, bar, scroll, ev, area, grab);
    }
    // `if (event.release || (event.button & 32) !== 0 || (event.button & 3) !== 0) return false;`
    // (`:773`): only a left press with no motion bit set can begin a drag. Every other kind — a
    // release, a motion report, a wheel notch, a right or middle press — is declined here and
    // reaches ADR-0005 §B-8 instead.
    if !matches!(ev.kind, MouseEventKind::Down(MouseButton::Left)) {
        return false;
    }
    let Some(geom) = geometry(bar, scroll, area) else {
        return false;
    };
    // `x === geometry.column` (`:726`). A press anywhere else in the viewport is content, and in
    // `auto` — where the bar reserves no column of its own (`components/scroll-view.ts:86-88`) —
    // this single column is the whole of what the scrollbar takes from selection.
    if ev.column != geom.column {
        return false;
    }
    if hits_thumb(&geom, ev.column, ev.row) {
        // `this.setScrollbarHover(target.scrollView)` (`:785`) before the grab is recorded, so an
        // `auto` bar is pinned up for the life of the drag rather than fading out from under the
        // pointer holding it.
        set_hover(bar, scroll, true);
        // `grabOffset: event.y - target.geometry.thumbTop` (`:788`). The hit test above puts the
        // row inside the thumb, so the subtraction cannot go negative.
        drag.grab_offset = Some(ev.row.saturating_sub(geom.thumb_top));
        return true;
    }
    if !hits_track(&geom, ev.row) {
        return false;
    }
    page_toward(scroll, &geom, ev.row);
    true
}

/// Follow the pointer while a drag holds the thumb — pi's `if (this.scrollbarDrag)` branch
/// (`tui-alt-screen.ts:751-771`).
///
/// A release ends the drag (`:752-755`). A wheel notch is declined rather than read as motion,
/// because upstream never routes one here at all (see the module doc's dispatcher contract).
/// Anything else is pointer motion — including the further presses a terminal can emit while a
/// button is held, which upstream's single `event.release` test also treats as motion.
///
/// Geometry is re-derived rather than captured at grab time, exactly as upstream re-derives it from
/// the current layout on every motion (`:756-759`): the document grows underneath a live drag, so a
/// thumb sized once at the press would map the pointer onto a track that no longer exists. When the
/// bar has gone (the content shrank to fit) the report is still consumed — the drag is live and the
/// pointer is not the content's — and the offset simply does not move (`:760`).
fn drive(
    drag: &mut DragState,
    bar: &mut ScrollbarView,
    scroll: &mut ScrollState,
    ev: &MouseEvent,
    area: Rect,
    grab: u16,
) -> bool {
    match ev.kind {
        MouseEventKind::Up(_) => {
            drag.grab_offset = None;
            return true;
        }
        MouseEventKind::ScrollUp
        | MouseEventKind::ScrollDown
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => return false,
        MouseEventKind::Down(_) | MouseEventKind::Drag(_) | MouseEventKind::Moved => {}
    }
    if let Some(geom) = geometry(bar, scroll, area) {
        scroll_to_row(scroll, offset_for_pointer(&geom, ev.row, grab));
    }
    true
}

/// The offset a pointer row maps to under a drag that grabbed `grab` rows into the thumb — pi's
/// `:761-767`.
///
/// ```text
/// maxThumbOffset = trackHeight - thumbHeight
/// thumbOffset    = max(0, min(maxThumbOffset, y - trackTop - grabOffset))
/// scrollTop      = maxThumbOffset === 0 ? 0 : round(thumbOffset / maxThumbOffset * maxScrollTop)
/// ```
///
/// Subtracting `grabOffset` is what stops the thumb snapping its top — or, under a hit test that
/// centred instead, its middle — to the pointer: the row the user pressed on stays under the
/// pointer for the whole gesture.
///
/// The arithmetic runs in `i64` so the intermediate product cannot wrap a `u16` track against a
/// document of any length, and every step is saturating; the two `try_from` fallbacks are
/// unreachable, since `thumb_offset` is bounded by a `u16` track and the quotient is bounded by
/// `max_scroll_top`. `Math.round` on a non-negative quotient is the `+ denominator / 2` before an
/// integer division, which is also how [`super::scroll`] reproduces ratatui's own rounding.
fn offset_for_pointer(geom: &ScrollbarGeom, row: u16, grab: u16) -> usize {
    let max_thumb_offset =
        i64::from(geom.track_height).saturating_sub(i64::from(geom.thumb_height));
    if max_thumb_offset <= 0 {
        // `maxThumbOffset === 0 ? 0` (`:767`) — a thumb that fills its track has one position, and
        // it is the top.
        return 0;
    }
    let thumb_offset = i64::from(row)
        .saturating_sub(i64::from(geom.track_top))
        .saturating_sub(i64::from(grab))
        .max(0)
        .min(max_thumb_offset);
    let max_scroll_top = i64::try_from(geom.max_scroll_top).unwrap_or(i64::MAX);
    let scaled = thumb_offset
        .saturating_mul(max_scroll_top)
        .saturating_add(max_thumb_offset / 2);
    usize::try_from(scaled / max_thumb_offset).unwrap_or(geom.max_scroll_top)
}

/// Page the view toward a press that landed in the **trough** — the part of the bar's column the
/// thumb is not on.
///
/// `[CYRUP-DELTA]`: upstream has no trough behaviour. `getScrollbarTargetAt` matches the thumb rows
/// only (`tui-alt-screen.ts:727-729`), so a press beside the thumb returns no target, is declined
/// by `handleScrollbarMouseEvent` (`:775`) and reaches `handleSelectionMouseEvent` — which, in the
/// `auto` mode both cyrup and pi default to, begins a text selection in the content column the bar
/// is overlaying. ADR-0005 §B-7 asks for the conventional scrollbar instead: a trough press pages
/// toward the click. The cost is one column of selection while the bar is actually on screen; the
/// gain is that the bar behaves the way every other scrollbar the user owns does.
///
/// Paging, not jumping, is the point: a trough press moves by a viewport and stops, so a user who
/// wants the position under their finger drags the thumb there and a user who wants the next
/// screenful clicks once. Absolute positioning on a trough click would make the two gestures
/// indistinguishable and lose the first.
///
/// A page here is the **whole** viewport, deliberately unlike the keyboard page, which is
/// `max(1, viewportHeight - PAGE_SCROLL_OVERLAP)` (`:64`, `:603`) and belongs to ADR-0005 §B-9's
/// file so that constant keeps one definition. The overlap exists to carry context across a blind
/// jump; a trough press is not blind — the pointer marks where the reader is looking — and an
/// overlap would leave the thumb visibly short of it.
///
/// Movement goes through [`super::scroll::scroll_by`], so the clamp, the release of `follow: end`
/// and the activity mark that keeps an `auto` bar up are the ones every other mover uses.
fn page_toward(scroll: &mut ScrollState, geom: &ScrollbarGeom, row: u16) {
    // `Math.max(1, …)` on the page size (`:603`): a viewport of zero or one row still moves.
    let page = i32::try_from(viewport_height(scroll))
        .unwrap_or(i32::MAX)
        .max(1);
    if row < geom.thumb_top {
        scroll_by(scroll, page.saturating_neg());
    } else {
        scroll_by(scroll, page);
    }
}

/// Whether `(column, row)` is on the thumb — pi's target test
/// (`tui-alt-screen.ts:726-729`): `x === geometry.column && y >= geometry.thumbTop &&
/// y < geometry.thumbTop + geometry.thumbHeight`.
fn hits_thumb(geom: &ScrollbarGeom, column: u16, row: u16) -> bool {
    column == geom.column && row >= geom.thumb_top && row < thumb_bottom(geom)
}

/// Whether `row` is inside the track, thumb included — the half of the trough test the column
/// check at the call site does not cover. Upstream needs no equivalent, because it has no trough
/// behaviour to bound; see [`page_toward`].
fn hits_track(geom: &ScrollbarGeom, row: u16) -> bool {
    row >= geom.track_top && row < geom.track_top.saturating_add(geom.track_height)
}

/// The first row **below** the thumb — `geometry.thumbTop + geometry.thumbHeight`
/// (`tui-alt-screen.ts:729`), saturating rather than wrapping at the bottom of a `u16` screen.
fn thumb_bottom(geom: &ScrollbarGeom) -> u16 {
    geom.thumb_top.saturating_add(geom.thumb_height)
}
