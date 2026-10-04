//! Making the scrollbar **interactive** — hit test, hover and thumb drag, cyrup's port of pi's
//! `getScrollbarTargetAt` / `setScrollbarHover` / `handleScrollbarMouseEvent`
//! (`packages/tui/src/tui-alt-screen.ts` @v1.0.0: `:1036-1051`, `:1053-1066`, `:1080-1127`).
//! ADR-0005 §Decision B-7.
//!
//! # Why any of this is application work
//! [`ratatui::widgets::Scrollbar`] *draws* a thumb and answers no question about it: it has no
//! pointer model, no hit test and no drag. Everything a user does to a scrollbar — hovering it,
//! grabbing it, sliding it, clicking beside it — is code someone has to write, which is why
//! upstream writes it too. This module is that code, and nothing else: it never paints (the thumb
//! is [`super::scroll::draw`]'s), never derives thumb geometry of its own (it asks
//! [`super::scroll::geometry`], the single derivation both the paint and the hit test share) and
//! never moves the offset by hand (it calls [`super::scroll::scroll_to_row`]).
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
    ScrollState, ScrollbarGeom, ScrollbarView, geometry, geometry_with, scroll_to_row, set_hover,
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
/// 3. **No drag, and a left press landed on the track** — hover is pinned and a drag starts
///    (`:1102-1123`). On the thumb the grab offset is the row pressed within it; OFF the thumb the
///    view first jumps so the thumb is centred on the pointer — `grabOffset = floor(thumbHeight /
///    2)` through [`offset_for_pointer`] (`:1115-1117`) — and the drag continues from there.
/// 4. **Anything else** — declined, so §B-8's selection sees it (`:1102-1104`). A press outside
///    the bar's own column or rows, a press while the bar is faded, a release with no drag, a
///    non-left button and every motion report with no drag all land here, which is upstream's
///    `(event.button & 32) !== 0` motion test and its `(event.button & 3) !== 0` button test
///    expressed as [`MouseEventKind`] arms.
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

/// Re-derive hover from a pointer position — pi's `updateScrollbarHover(x, y)` (`:1060-1062`) over
/// `getScrollbarTargetAt(x, y, true)` (`:1036-1051`) and `setScrollbarHover` (`:1053-1058`).
///
/// Upstream also refreshes hover after every wheel notch (`routeWheel`, `:995`), because scrolling
/// moved the thumb out from under a stationary pointer. Hover here is the TRACK, whose position
/// does not depend on the offset, and [`route`] already refreshes it for the wheel report itself,
/// so that second call site has nothing left to do.
///
/// Hover is the pointer being **anywhere on the track** — the bar's column over its full height,
/// thumb or not — and the lookup passes `includeHiddenAuto`, so a pointer arriving over the column
/// of a faded `auto` bar whose content overflows reveals it (and, being hovered, it then stays up
/// and paints its thumb as `█`). A faded bar is therefore found by pointing at where it would be;
/// it is only a *press* that needs the bar already showing ([`handle`]).
///
/// What hover *does* is upstream's `setScrollbarActive` (`components/scroll-view.ts:113-117`): an
/// `auto` bar under the pointer stops fading (`:97`) and stays up for as long as the pointer holds
/// it, and [`super::scroll::draw`] paints the thumb as `█` rather than `┃` while it holds.
pub(super) fn update_hover(
    bar: &mut ScrollbarView,
    scroll: &mut ScrollState,
    column: u16,
    row: u16,
    area: Rect,
) {
    let on_track =
        geometry_with(bar, scroll, area, true).is_some_and(|geom| hits_track(&geom, column, row));
    set_hover(bar, scroll, on_track);
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
    // `getScrollbarTargetAt(event.x, event.y)` (`:1102`) — `includeHiddenAuto` defaults to false,
    // so a faded bar has no geometry and cannot be pressed.
    let Some(geom) = geometry(bar, scroll, area) else {
        return false;
    };
    // `x === geometry.column && y >= trackTop && y < trackTop + trackHeight` (`:1042-1046`). A
    // press anywhere else in the viewport is content, and in `auto` — where the bar reserves no
    // column of its own (`components/scroll-view.ts:86-88`) — this single column is the whole of
    // what the scrollbar takes from selection.
    if !hits_track(&geom, ev.column, ev.row) {
        return false;
    }
    // `this.setScrollbarHover(target.scrollView)` (`:1113`) before the grab is recorded, so an
    // `auto` bar is pinned up for the life of the drag rather than fading out from under the
    // pointer holding it.
    set_hover(bar, scroll, true);
    // `onThumb ? event.y - thumbTop : Math.floor(thumbHeight / 2)` (`:1115-1116`). Off the thumb the
    // view first scrolls so the thumb's middle is under the pointer, and the drag that follows
    // holds that middle row (`:1117`).
    let grab = if hits_thumb(&geom, ev.row) {
        ev.row.saturating_sub(geom.thumb_top)
    } else {
        let grab = geom.thumb_height / 2;
        scroll_to_row(scroll, offset_for_pointer(&geom, ev.row, grab));
        grab
    };
    drag.grab_offset = Some(grab);
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
/// `scrollScrollbarToPointer` (`tui-alt-screen.ts:1068-1078`).
///
/// ```text
/// maxThumbOffset = trackHeight - thumbHeight
/// thumbOffset    = max(0, min(maxThumbOffset, y - trackTop - grabOffset))
/// scrollTop      = maxThumbOffset === 0 ? 0 : round(thumbOffset / maxThumbOffset * maxScrollTop)
/// ```
///
/// Subtracting `grabOffset` is what stops the thumb snapping its top — or, under a hit test that
/// centred instead, its middle — to the pointer: the row the user pressed on stays under the
/// pointer for the whole gesture. A press OFF the thumb passes `thumbHeight / 2` so it is the
/// thumb's middle that lands there.
///
/// Computed in `f64` in pi's operation order so a quotient on a half rounds as `Math.round` does.
/// The pointer arithmetic runs in `i64` so a `u16` row against any track cannot wrap.
fn offset_for_pointer(geom: &ScrollbarGeom, row: u16, grab: u16) -> usize {
    let max_thumb_offset =
        i64::from(geom.track_height).saturating_sub(i64::from(geom.thumb_height));
    if max_thumb_offset <= 0 {
        // `maxThumbOffset === 0 ? 0` — a thumb that fills its track has one position, the top.
        return 0;
    }
    let thumb_offset = i64::from(row)
        .saturating_sub(i64::from(geom.track_top))
        .saturating_sub(i64::from(grab))
        .max(0)
        .min(max_thumb_offset);
    let fraction = thumb_offset as f64 / max_thumb_offset as f64;
    // `as usize` saturates; the product is bounded by `max_scroll_top`.
    ((fraction * geom.max_scroll_top as f64).round() as usize).min(geom.max_scroll_top)
}

/// Whether `row` is on the thumb — `y >= geometry.thumbTop && y < geometry.thumbTop +
/// geometry.thumbHeight` (`tui-alt-screen.ts:1109-1110`). The caller has already matched the
/// column.
fn hits_thumb(geom: &ScrollbarGeom, row: u16) -> bool {
    row >= geom.thumb_top && row < thumb_bottom(geom)
}

/// Whether `(column, row)` is on the track, thumb included — `getScrollbarTargetAt`'s test
/// (`tui-alt-screen.ts:1042-1046`): `x === geometry.column && y >= geometry.trackTop && y <
/// geometry.trackTop + geometry.trackHeight`.
fn hits_track(geom: &ScrollbarGeom, column: u16, row: u16) -> bool {
    column == geom.column
        && row >= geom.track_top
        && row < geom.track_top.saturating_add(geom.track_height)
}

/// The first row **below** the thumb — `geometry.thumbTop + geometry.thumbHeight`
/// (`tui-alt-screen.ts:1110`), saturating rather than wrapping at the bottom of a `u16` screen.
fn thumb_bottom(geom: &ScrollbarGeom) -> u16 {
    geom.thumb_top.saturating_add(geom.thumb_height)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use ratatui::crossterm::event::KeyModifiers;

    use super::super::scroll::{
        ScrollbarMode, fade, is_following_end, is_hovered, is_visible, scroll_to_row, scroll_top,
        set_mode, update_layout,
    };
    use super::*;

    /// The document rect: offset from the origin (a header above it) and SHORTER than the terminal
    /// it sits in (an input dock below). Every coordinate below is relative to it, and nothing in
    /// this module may consult a terminal size. The bar's column is `4 + 20 - 1 = 23`.
    const AREA: Rect = Rect::new(4, 2, 20, 10);
    const COLUMN: u16 = 23;

    struct Fixture {
        drag: DragState,
        bar: ScrollbarView,
        scroll: ScrollState,
    }

    /// `content` rows in a [`AREA`]-high viewport under `mode`.
    fn fixture(mode: ScrollbarMode, content: usize) -> Fixture {
        let mut f = Fixture {
            drag: DragState::default(),
            bar: ScrollbarView::default(),
            scroll: ScrollState::default(),
        };
        set_mode(&mut f.bar, &mut f.scroll, mode);
        update_layout(&mut f.scroll, content, usize::from(AREA.height));
        // Park at the top with the movement mark cleared: an `auto` bar starts out faded.
        scroll_to_row(&mut f.scroll, 0);
        fade(&mut f.scroll);
        f
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn send(f: &mut Fixture, kind: MouseEventKind, column: u16, row: u16) -> bool {
        route(
            &mut f.drag,
            &mut f.bar,
            &mut f.scroll,
            &mouse(kind, column, row),
            AREA,
        )
    }

    fn press() -> MouseEventKind {
        MouseEventKind::Down(MouseButton::Left)
    }

    /// `jumps to a scrollbar track position and continues dragging from there`
    /// (`test/tui-alt-screen.test.ts:481-506` @v1.0.0): 50 rows in a 10-row viewport with a 2-row
    /// thumb. A press on track row 5 — off the thumb — scrolls to 20 (the thumb centred on the
    /// pointer, `grabOffset = floor(2 / 2)`), and a drag to the last row carries on from there to 40.
    ///
    /// Paging by a whole viewport would land on 10 and start no drag.
    #[test]
    fn a_press_off_the_thumb_jumps_to_the_pointer_and_then_drags() {
        let mut f = fixture(ScrollbarMode::Always, 50);
        assert_eq!(scroll_top(&f.scroll), 0);

        assert!(send(&mut f, press(), COLUMN, AREA.y + 5));
        assert_eq!(scroll_top(&f.scroll), 20, "jumped, not paged");
        assert!(is_dragging(&f.drag), "and the press began a drag");

        assert!(send(
            &mut f,
            MouseEventKind::Drag(MouseButton::Left),
            COLUMN,
            AREA.y + 9
        ));
        assert_eq!(
            scroll_top(&f.scroll),
            40,
            "the drag continues from the jump"
        );
        assert!(is_following_end(&f.scroll));

        assert!(send(
            &mut f,
            MouseEventKind::Up(MouseButton::Left),
            COLUMN,
            AREA.y + 9
        ));
        assert!(!is_dragging(&f.drag));
    }

    /// A press ON the thumb grabs it where it was pressed and does not move the document; the
    /// drag keeps that row under the pointer (`scrollbarDrag.grabOffset`, `:1115-1123`).
    #[test]
    fn a_press_on_the_thumb_keeps_its_grab_offset() {
        let mut f = fixture(ScrollbarMode::Always, 50);
        // The thumb is track rows 0-1; press its second row.
        assert!(send(&mut f, press(), COLUMN, AREA.y + 1));
        assert_eq!(scroll_top(&f.scroll), 0, "a thumb grab moves nothing");
        assert!(is_dragging(&f.drag));
        // Pointer at track row 3, grab 1 -> thumb offset 2 of 8 -> round(2 / 8 * 40) = 10.
        send(
            &mut f,
            MouseEventKind::Drag(MouseButton::Left),
            COLUMN,
            AREA.y + 3,
        );
        assert_eq!(scroll_top(&f.scroll), 10);
    }

    /// A press beside the bar, above it or below it is content: declined, so selection sees it.
    #[test]
    fn a_press_outside_the_track_is_declined() {
        let mut f = fixture(ScrollbarMode::Always, 50);
        assert!(
            !send(&mut f, press(), COLUMN - 1, AREA.y + 5),
            "one column left"
        );
        assert!(
            !send(&mut f, press(), COLUMN, AREA.y - 1),
            "the row above the rect"
        );
        assert!(
            !send(&mut f, press(), COLUMN, AREA.y + AREA.height),
            "the row below the rect"
        );
        assert!(!is_dragging(&f.drag));
        assert_eq!(scroll_top(&f.scroll), 0);
    }

    /// `reveals an auto scrollbar when the pointer enters its hidden track`
    /// (`test/tui-alt-screen.test.ts:456-479`): a faded `auto` bar over overflowing content shows,
    /// hovered, as soon as the pointer reaches its column — anywhere on the track, thumb or not —
    /// and a pointer one column over, or past the rect, does not.
    #[test]
    fn hover_reveals_a_hidden_auto_bar_from_anywhere_on_the_track() {
        let mut f = fixture(ScrollbarMode::Auto, 50);
        assert!(!is_visible(&f.bar, &f.scroll), "fixture: the bar is faded");

        update_hover(&mut f.bar, &mut f.scroll, COLUMN - 1, AREA.y + 5, AREA);
        assert!(
            !is_hovered(&f.bar) && !is_visible(&f.bar, &f.scroll),
            "one column left of the track"
        );
        update_hover(
            &mut f.bar,
            &mut f.scroll,
            COLUMN,
            AREA.y + AREA.height,
            AREA,
        );
        assert!(!is_hovered(&f.bar), "below the rect");

        // Row 7 is nowhere near the thumb (rows 0-1), which is what the old thumb-only hover needed.
        update_hover(&mut f.bar, &mut f.scroll, COLUMN, AREA.y + 7, AREA);
        assert!(is_hovered(&f.bar), "the track is hovered");
        assert!(
            is_visible(&f.bar, &f.scroll),
            "and a hovered auto bar is revealed"
        );

        update_hover(&mut f.bar, &mut f.scroll, COLUMN - 1, AREA.y + 7, AREA);
        assert!(!is_hovered(&f.bar), "leaving the column un-hovers it");
    }

    /// Nothing to scroll, nothing to reveal: `includeHiddenAuto` needs `contentHeight >
    /// trackHeight`, and a hidden-mode bar is never revealed.
    #[test]
    fn hover_does_not_reveal_a_bar_with_nothing_to_scroll() {
        let mut fits = fixture(ScrollbarMode::Auto, 10);
        update_hover(&mut fits.bar, &mut fits.scroll, COLUMN, AREA.y + 3, AREA);
        assert!(!is_hovered(&fits.bar) && !is_visible(&fits.bar, &fits.scroll));

        let mut hidden = fixture(ScrollbarMode::Hidden, 50);
        update_hover(
            &mut hidden.bar,
            &mut hidden.scroll,
            COLUMN,
            AREA.y + 3,
            AREA,
        );
        assert!(!is_hovered(&hidden.bar) && !is_visible(&hidden.bar, &hidden.scroll));
    }

    /// pi checks `getScrollbarTargetAt(x, y)` WITHOUT `includeHiddenAuto` for a press, so the very
    /// first press on a faded bar's column is the content's (declined) — and the hover refresh that
    /// follows it reveals the bar, so the next press is the bar's.
    #[test]
    fn a_press_on_a_faded_bar_is_declined_but_reveals_it() {
        let mut f = fixture(ScrollbarMode::Auto, 50);
        assert!(
            !send(&mut f, press(), COLUMN, AREA.y + 5),
            "faded: not the bar's"
        );
        assert!(!is_dragging(&f.drag));
        assert!(
            is_hovered(&f.bar) && is_visible(&f.bar, &f.scroll),
            "but the press revealed it"
        );
        assert!(!send(
            &mut f,
            MouseEventKind::Up(MouseButton::Left),
            COLUMN,
            AREA.y + 5
        ));
        assert!(
            send(&mut f, press(), COLUMN, AREA.y + 5),
            "now it is the bar's"
        );
        assert!(is_dragging(&f.drag));
    }

    /// A press that starts a drag pins hover for its life, even with the pointer dragged off the
    /// column, and the release re-derives it from where the pointer actually is.
    #[test]
    fn a_drag_pins_hover_until_the_release() {
        let mut f = fixture(ScrollbarMode::Auto, 50);
        send(&mut f, MouseEventKind::Moved, COLUMN, AREA.y + 5);
        assert!(is_hovered(&f.bar));
        assert!(send(&mut f, press(), COLUMN, AREA.y + 5));
        send(
            &mut f,
            MouseEventKind::Drag(MouseButton::Left),
            0,
            AREA.y + 6,
        );
        assert!(is_hovered(&f.bar), "dragged off the column: still held");
        assert!(is_dragging(&f.drag));
        send(&mut f, MouseEventKind::Up(MouseButton::Left), 0, AREA.y + 6);
        assert!(!is_dragging(&f.drag));
        assert!(
            !is_hovered(&f.bar),
            "released over content: no longer hovered"
        );
    }
}
