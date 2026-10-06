//! The **scroll model** over the retained document and the **scrollbar** that reports it — cyrup's
//! port of pi's `ScrollView` (`packages/tui/src/components/scroll-view.ts` @v0.84.3) together with
//! the thumb geometry and painter the layout pass derives from one (`layout.ts:278-330` @v1.0.0).
//! ADR-0005 §Decision B-5.
//!
//! # Why the offset and the bar share a file
//! Upstream they share an *object*: `ScrollView` holds `currentScrollTop`, `contentHeight` and
//! `currentViewportHeight` next to `currentScrollbar`, `transientScrollbarVisible` and
//! `scrollbarActive` (`scroll-view.ts:27-37`), because every scrollbar answer is a question about
//! the offset — whether the thumb is visible at all is `contentHeight > viewportHeight`
//! (`:71-76`), and every movement is what re-arms the transient timer (`:136`, `:151`, `:164`,
//! `:176`). cyrup splits the *state* in two ([`ScrollState`] and [`ScrollbarView`]) so the two
//! consumers of a scrollbar — the painter here and ADR-0005 §B-7's thumb drag — can borrow only
//! what each needs from the renderer's UI bag, but keeps the *policy* in one module so there is
//! exactly one definition of "is the thumb showing" and exactly one of "where is it".
//!
//! # Deadlines, not timers
//! Upstream hides an `auto` bar with a `setTimeout` that flips `transientScrollbarVisible` back to
//! false and calls `requestRender` (`scroll-view.ts:98-103`). cyrup has no per-component scheduler,
//! so [`mark_activity`] records the [`Instant`] of the last movement and [`is_visible`] asks
//! whether it is younger than [`SCROLLBAR_HIDE_DELAY`] — the same shape [`super::flash`] uses for
//! its notices. [`next_hide`] is what the alternate-screen loop schedules its next wake on, so the
//! bar still fades with no further input, which is the acceptance criterion the timer exists for.
//!
//! Two of upstream's explicit hide calls fall out of the deadline model rather than being ported:
//! `hideTransientScrollbar` on a mode change away from `auto` (`:81`) and on content shrinking back
//! inside the viewport (`:192`) are both already false branches of [`is_visible`], which re-derives
//! visibility from the live mode and the live heights on every read.
//!
//! # What the alternate screen builds
//! One primary scroll view over the whole retained document, `follow: "end"`,
//! `overscroll: "chain"`, its scrollbar taken from the `fullscreenScrollbar` setting and its track
//! and thumb styled with `theme.fg("scrollbarTrack", …)` / `theme.fg("scrollbarThumb", …)`
//! (`interactive-mode.ts:962-967` @v1.0.0). Because there is
//! exactly one and it is always built that way, `followEnd` and `overscroll` are constants here
//! rather than fields: this module always follows the tail (`scroll-view.ts:46-47`) and always
//! reports its unconsumed remainder for a caller to chain (`tui-alt-screen.ts:675-686`).
//!
//! `followSuppressedAtEnd` (`scroll-view.ts:33`) is likewise absent: its only upstream writer is
//! `scrollTo(target, { disableFollow: true })` from the alternate-screen search reveal
//! (`tui-alt-screen.ts:529`), and search is not among ADR-0005 §Decision B's units. With no writer
//! the flag is permanently false, and every expression guarded by it (`:123-124`, `:188-191`)
//! collapses to the form written below.
//!
//! # Unmet prerequisite (ADR-0005 §Decision A-3)
//! `fullscreenScrollbar` does not exist in `cyrup-config`. Upstream's accessor degrades anything
//! that is not `always` or `hidden` to `auto` (`settings-manager.ts:1221-1224`), so
//! [`ScrollbarMode`]'s `Default` is `Auto` and [`set_mode`] is the single seam A-3 will drive when
//! it lands. This module adds no configuration surface of its own.
//!
//! # No application state
//! Nothing here holds a transcript, a theme or a keymap (`altscreen/mod.rs`, rule 2): the heights
//! arrive through [`update_layout`], the track and thumb colours arrive as a `&UiTheme` argument at
//! paint time, and the document itself is never touched — which is what lets
//! [`crate::ViewportRenderer::scroll_by`] and its siblings work from `&mut self` alone.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect};
use ratatui::style::Color;

use crate::text_width::str_width;
use crate::theme::UiTheme;

/// How long an `auto` bar stays up after the last movement — pi's `scrollbarHideDelayMs`, whose
/// default is 1000 ms (`components/scroll-view.ts:52`). Upstream exposes it as a constructor
/// option; the alternate screen never passes one (`interactive-mode.ts:918-923`), so cyrup keeps
/// the default as the constant it always is.
pub(super) const SCROLLBAR_HIDE_DELAY: Duration = Duration::from_millis(1000);

/// The three scrollbar policies — pi's
/// `type ScrollViewScrollbar = "hidden" | "auto" | "always"` (`components/scroll-view.ts:4`).
///
/// `Default` is [`ScrollbarMode::Auto`] rather than upstream's constructor default of `"hidden"`
/// (`:50`), because cyrup's only scroll view is the transcript one, which upstream constructs from
/// `getFullscreenScrollbar()` — and that accessor's own default is `auto`
/// (`settings-manager.ts:1221-1224`). See the module doc's A-3 note.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollbarMode {
    /// Never drawn and never reserves a column — pi's `"hidden"`.
    Hidden,
    /// Drawn only while the content overflows the viewport *and* the bar is recently active — pi's
    /// `"auto"` (`components/scroll-view.ts:73-75`). Reserves no column: an `auto` bar overlays the
    /// content's last column, because `getContentWidth` narrows for `"always"` only (`:86-88`).
    #[default]
    Auto,
    /// Always drawn while the viewport has height, and permanently reserves the rightmost column —
    /// pi's `"always"` (`:72`, `:87`).
    Always,
}

/// The scroll offset over the retained document — pi's `ScrollView` offset half
/// (`components/scroll-view.ts:29-33`).
///
/// Lives in the alternate-screen renderer's UI bag (ADR-0005 §B-3). Heights are rows of the
/// *rendered* document, not entries of the transcript, so every field is a plain `usize` and this
/// type borrows nothing.
pub(super) struct ScrollState {
    /// First document row painted at the top of the viewport — `currentScrollTop` (`:29`).
    scroll_top: usize,
    /// Rows the viewport can show — `currentViewportHeight` (`:31`).
    viewport_height: usize,
    /// Rows the rendered document occupies — `contentHeight` (`:30`).
    content_height: usize,
    /// Whether the view is stuck to the tail — `followingEnd` (`:32`). New output keeps itself
    /// visible while this holds, and [`scroll_by_remaining`] clears it the moment the user moves
    /// away from the bottom (`:149`).
    following_end: bool,
    /// When the view last moved, standing in for upstream's transient-hide timer — see the module
    /// doc. `None` until the first movement, which is upstream's initial
    /// `transientScrollbarVisible = false` (`:35`).
    last_activity: Option<Instant>,
    /// The value of [`crate::TranscriptView::retained_dropped`] this offset was last reconciled
    /// against — the front-trim counter ADR-0005 §B-1 keeps so a renderer can tell that every row
    /// index it holds has moved. Compared, and advanced, by [`rebuild_rows`].
    seen_dropped: u64,
}

impl Default for ScrollState {
    /// `followingEnd` starts *true*, mirroring `this.followingEnd = this.followEnd`
    /// (`components/scroll-view.ts:47`) for the alternate screen's `follow: "end"` view
    /// (`interactive-mode.ts:919`): a session that has printed nothing is already at its tail, and
    /// the first output must not need a keystroke to become visible.
    fn default() -> Self {
        Self {
            scroll_top: 0,
            viewport_height: 0,
            content_height: 0,
            following_end: true,
            last_activity: None,
            seen_dropped: 0,
        }
    }
}

/// The scrollbar policy and the pointer state that shapes how it paints — pi's `ScrollView`
/// scrollbar half (`components/scroll-view.ts:27-37`).
///
/// Held beside a [`ScrollState`] in the renderer's UI bag. Every mutator below takes both, because
/// upstream's answers are joint: whether the bar shows depends on the heights, and every movement
/// re-arms the bar.
#[derive(Default)]
pub(super) struct ScrollbarView {
    /// Which policy is in force — `currentScrollbar` (`:27`).
    mode: ScrollbarMode,
    /// Whether the pointer is over the bar's track column, or a drag holds it — `scrollbarActive`
    /// (`:36`), written by `setScrollbarActive` (`:113-117`) from the alternate screen's hover
    /// tracking (`tui-alt-screen.ts:1053-1062`). While it holds, an `auto` bar does not fade:
    /// upstream arms no hide timer at all in that case (`components/scroll-view.ts:97`), and the
    /// thumb paints as `█` instead of `┃`. ADR-0005 §B-7 is its only writer, through
    /// [`set_hover`].
    hover: bool,
}

/// Where the thumb is — pi's `ScrollbarGeometry` (`layout.ts:38-45`), in absolute frame
/// coordinates.
///
/// This is ADR-0005 §B-7's only window into the bar: the hit test is
/// `x == column && track_top <= y < track_top + track_height` (`tui-alt-screen.ts:1036-1051`), the
/// thumb is `thumb_top <= y < thumb_top + thumb_height`, and a pointer row converts into an offset
/// with `round(thumb_offset / (track_height - thumb_height) * max_scroll_top)` (`:1068-1078`).
/// Deriving it anywhere else would let the hit test and the paint disagree.
pub(super) struct ScrollbarGeom {
    /// The column the thumb occupies — `column` (`layout.ts:39`), upstream's
    /// `box.rect.x + box.rect.width - 1` (`:280`).
    pub(super) column: u16,
    /// First row of the track — `trackTop` (`:40`).
    pub(super) track_top: u16,
    /// Rows in the track — `trackHeight` (`:41`).
    pub(super) track_height: u16,
    /// First row of the thumb — `thumbTop` (`:42`).
    pub(super) thumb_top: u16,
    /// Rows in the thumb — `thumbHeight` (`:43`).
    pub(super) thumb_height: u16,
    /// The offset the bottom of the track corresponds to — `maxScrollTop` (`:44`).
    pub(super) max_scroll_top: usize,
}

/// The largest legal [`scroll_top`] — upstream's
/// `Math.max(0, this.contentHeight - this.currentViewportHeight)`, recomputed at each of its five
/// use sites (`components/scroll-view.ts:121`, `:143`, `:170`, `:185`).
fn max_scroll_top_of(scroll: &ScrollState) -> usize {
    scroll.content_height.saturating_sub(scroll.viewport_height)
}

/// First document row painted — pi's `get scrollTop()` (`components/scroll-view.ts:55-57`).
pub(super) fn scroll_top(scroll: &ScrollState) -> usize {
    scroll.scroll_top
}

/// Rows the viewport can show — pi's `get viewportHeight()` (`components/scroll-view.ts:63-65`).
/// ADR-0005 §B-9 sizes a page from this (`tui-alt-screen.ts:603`, `:614`).
pub(super) fn viewport_height(scroll: &ScrollState) -> usize {
    scroll.viewport_height
}

/// Rows the rendered document occupies — upstream's private `contentHeight`
/// (`components/scroll-view.ts:30`), exposed here because the scrollbar policy and ADR-0005 §B-8's
/// edge auto-scroll both read it.
pub(super) fn content_height(scroll: &ScrollState) -> usize {
    scroll.content_height
}

/// Whether the view is stuck to the tail — pi's `get isFollowingEnd()`
/// (`components/scroll-view.ts:59-61`). Read by the scroll-to-end indicator (TUI-109,
/// `tui-alt-screen.ts:1626`) and, through `TuiAltScreen.isFollowingOutput`, by upstream's tests.
pub(super) fn is_following_end(scroll: &ScrollState) -> bool {
    scroll.following_end
}

/// The largest legal offset — see [`max_scroll_top_of`].
/// `#[cfg(test)]`: pi's `get maxScrollTop()` (`components/scroll-view.ts:34-36`). Every in-module
/// caller uses `max_scroll_top_of` directly; this is the out-of-module form, and its only consumer
/// is an assertion.
#[cfg(test)]
pub(super) fn max_scroll_top(scroll: &ScrollState) -> usize {
    max_scroll_top_of(scroll)
}

/// Record a movement, restarting the transient bar's fade — pi's `markScrollbarActivity`
/// (`components/scroll-view.ts:90-104`), called from every mutator that actually moved
/// (`:136`, `:151`, `:164`, `:176`).
///
/// Upstream refuses the mark outright when the mode is not `auto` or the content fits
/// (`:91`); cyrup records unconditionally and lets [`is_visible`] apply both tests on the way out,
/// which is what keeps this free of any knowledge of the bar. The two differ only for a mark taken
/// while the content fits that is then read, within [`SCROLLBAR_HIDE_DELAY`], after the content has
/// grown past the viewport — where cyrup shows the bar the growth is about to justify and upstream
/// waits for the next movement.
pub(super) fn mark_activity(scroll: &mut ScrollState) {
    scroll.last_activity = Some(Instant::now());
}

/// Re-clamp after the document or the viewport changed size — pi's
/// `updateLayout(contentHeight, viewportHeight, requestRender)`
/// (`components/scroll-view.ts:181-193`), minus the render callback cyrup's loop does not need.
///
/// This is where `follow: end` earns its keep: while `is_following_end` holds, the offset is
/// pulled to the new bottom, so output appended since the last frame is on screen without the user
/// asking (`:186`). Otherwise the offset only ever shrinks to fit (`:187`), so a user reading
/// history is not yanked anywhere. Reaching the bottom by shrinkage re-arms the follow
/// (`:189-191`).
pub(super) fn update_layout(
    scroll: &mut ScrollState,
    content_height: usize,
    viewport_height: usize,
) {
    scroll.content_height = content_height;
    scroll.viewport_height = viewport_height;
    let max = max_scroll_top_of(scroll);
    scroll.scroll_top = if scroll.following_end {
        max
    } else {
        scroll.scroll_top.min(max)
    };
    if scroll.scroll_top == max {
        scroll.following_end = true;
    }
}

/// Reconcile the offset with ADR-0005 §B-1's front trim — the compensation R6 exists for.
///
/// `retained_dropped` is [`crate::TranscriptView::retained_dropped`] as of this frame and
/// `rows_dropped` is how many *rendered rows* those evicted entries occupied, which only the
/// caller that rebuilt the document can know. When the counter has not moved this is a no-op, so
/// calling it every frame is free.
///
/// A following view needs no shift — [`update_layout`] re-derives its offset from the new bottom —
/// but a user parked in history does: without this, dropping the front of the document slides every
/// row up underneath a fixed `scroll_top` and the reader silently jumps forward. `retained_dropped`
/// is the only signal that happened (`transcript/mod.rs:178`), which is why it is monotonic and
/// why [`crate::TranscriptView::clear_document`] bumps it too.
pub(super) fn rebuild_rows(scroll: &mut ScrollState, retained_dropped: u64, rows_dropped: usize) {
    if retained_dropped == scroll.seen_dropped {
        return;
    }
    scroll.seen_dropped = retained_dropped;
    if scroll.following_end {
        return;
    }
    scroll.scroll_top = scroll.scroll_top.saturating_sub(rows_dropped);
}

/// The first rendered row of the entry at `entry`, or `None` when it is not in the document.
///
/// `row_starts` is the entry-index-to-first-row map the caller rebuilt this frame. ADR-0005 §B-10
/// walks the document for a user prompt and lands on it through here and [`scroll_to_row`], which
/// is pi's `scrollToPrompt` shape without its OSC 133 row scan (`tui-alt-screen.ts:412-425`).
pub(super) fn row_of_entry(row_starts: &[usize], entry: usize) -> Option<usize> {
    row_starts.get(entry).copied()
}

/// Move to an absolute row — pi's `scrollTo(scrollTop)`
/// (`components/scroll-view.ts:119-138`), without the `disableFollow` option whose only upstream
/// caller is the unported search reveal (see the module doc).
///
/// Landing on the bottom re-arms the tail follow, exactly as upstream's
/// `nextFollowingEnd = this.followEnd && next === maxScrollTop` (`:124`) does with
/// `followSuppressedAtEnd` false.
pub(super) fn scroll_to_row(scroll: &mut ScrollState, row: usize) {
    let max = max_scroll_top_of(scroll);
    let next = row.min(max);
    let next_following = next == max;
    // `if (next === … && nextFollowingEnd === …) return;` (`:125-131`) — an idempotent call marks
    // no activity, so it neither re-arms nor extends a fading `auto` bar.
    if next == scroll.scroll_top && next_following == scroll.following_end {
        return;
    }
    let moved = next != scroll.scroll_top;
    scroll.scroll_top = next;
    scroll.following_end = next_following;
    if moved {
        mark_activity(scroll);
    }
}

/// Move by `lines`, **negative for up**, returning the part of the request the clamp refused —
/// pi's `scrollBy(lines: number): number` (`components/scroll-view.ts:140-154`).
///
/// The remainder is what makes overscroll *chaining* possible: the alternate screen offers a wheel
/// notch to each scroll view under the pointer in turn and passes on whatever the last one could
/// not consume, stopping at the first view that either consumed it all or is
/// `overscroll: "contain"` (`tui-alt-screen.ts:675-686`). cyrup's single view is `"chain"`
/// (`interactive-mode.ts:921`), so ADR-0005 §B-6 is the consumer of this return value.
///
/// A following view scrolls from the bottom rather than from its stale offset (`:144`), so an
/// upward notch taken while output is streaming moves relative to what is actually on screen.
pub(super) fn scroll_by_remaining(scroll: &mut ScrollState, lines: i32) -> i32 {
    if lines == 0 {
        return 0;
    }
    let max = max_scroll_top_of(scroll);
    let start = if scroll.following_end {
        max
    } else {
        scroll.scroll_top
    };
    // The arithmetic runs in `i64` so a large offset plus a large request cannot wrap: both ends
    // are clamped back into `0..=max` before the result returns to `usize`.
    let start_wide = i64::try_from(start).unwrap_or(i64::MAX);
    let max_wide = i64::try_from(max).unwrap_or(i64::MAX);
    let next_wide = start_wide
        .saturating_add(i64::from(lines))
        .clamp(0, max_wide.max(0));
    let moved_wide = next_wide.saturating_sub(start_wide);
    let next = usize::try_from(next_wide).unwrap_or(0);
    scroll.scroll_top = next;
    // `this.followingEnd = this.followEnd && next === maxScrollTop` (`:149`): scrolling away from
    // the bottom is what releases the follow, and scrolling back to it is what re-arms it.
    scroll.following_end = next == max;
    if moved_wide != 0 {
        mark_activity(scroll);
    }
    // `return requested - moved` (`:153`). `moved` never exceeds `lines` in magnitude nor differs
    // from it in sign, so the difference is bounded by `lines` and the fallbacks are unreachable.
    let moved = i32::try_from(moved_wide).unwrap_or(lines);
    lines.saturating_sub(moved)
}

/// Move by `lines`, discarding the unconsumed remainder — pi's `TuiAltScreen.scrollBy`
/// (`tui-alt-screen.ts:397-400`), which calls the view and ignores its return.
///
/// This is what [`crate::ViewportRenderer::scroll_by`] delegates to. A caller that needs to chain
/// the overflow into another view wants [`scroll_by_remaining`] instead.
pub(super) fn scroll_by(scroll: &mut ScrollState, lines: i32) {
    let _ = scroll_by_remaining(scroll, lines);
}

/// Jump to the first row — pi's `scrollToStart` (`components/scroll-view.ts:156-167`), reached by
/// `TuiAltScreen.scrollToTop` (`tui-alt-screen.ts:402-405`).
///
/// The follow is *not* re-armed unless the document already fits in the viewport, which is
/// upstream's `this.followEnd && this.contentHeight <= this.currentViewportHeight` (`:161`): with
/// nothing to scroll, the top is also the bottom.
pub(super) fn scroll_to_top(scroll: &mut ScrollState) {
    let fits = scroll.content_height <= scroll.viewport_height;
    let changed = scroll.scroll_top != 0 || scroll.following_end != fits;
    scroll.scroll_top = 0;
    scroll.following_end = fits;
    if changed {
        mark_activity(scroll);
    }
}

/// Jump to the last row and re-arm the tail follow — pi's `scrollToEnd`
/// (`components/scroll-view.ts:169-179`), reached by `TuiAltScreen.scrollToBottom`
/// (`tui-alt-screen.ts:407-410`).
pub(super) fn scroll_to_bottom(scroll: &mut ScrollState) {
    let next = max_scroll_top_of(scroll);
    let changed = scroll.scroll_top != next || !scroll.following_end;
    scroll.scroll_top = next;
    scroll.following_end = true;
    if changed {
        mark_activity(scroll);
    }
}

/// Which policy is in force — pi's `get scrollbar()` (`components/scroll-view.ts:67-69`).
/// `#[cfg(test)]`: pi's `get scrollbar()` (`components/scroll-view.ts:67-69`), which upstream needs
/// because `currentScrollbar` is private — this module's own consumers read `bar.mode` directly.
/// Upstream's caller is the settings round-trip (`interactive-mode.ts:1983`), which cyrup routes
/// through `set_scrollbar_mode` instead, so the read-back has no production consumer here.
#[cfg(test)]
pub(super) fn mode(bar: &ScrollbarView) -> ScrollbarMode {
    bar.mode
}

/// Change the policy — pi's `setScrollbar` (`components/scroll-view.ts:78-84`), upstream's single
/// application point for the `fullscreenScrollbar` setting, both at construction
/// (`interactive-mode.ts:922`) and on a live settings change (`:1983`).
///
/// Upstream's two side effects are its timer bookkeeping: hiding the transient bar when the new
/// mode is not `auto` (`:81`), which [`is_visible`] now decides on read instead, and re-arming it
/// when the new mode is `auto` while the pointer holds the thumb (`:82`), which is kept here
/// because it is what makes the bar appear under a stationary pointer.
pub(super) fn set_mode(bar: &mut ScrollbarView, scroll: &mut ScrollState, mode: ScrollbarMode) {
    if bar.mode == mode {
        return;
    }
    bar.mode = mode;
    if mode == ScrollbarMode::Auto && bar.hover {
        mark_activity(scroll);
    }
}

/// Report the pointer entering or leaving the bar's track column — pi's `setScrollbarActive`
/// (`components/scroll-view.ts:113-117`), driven by the alternate screen's hover tracking
/// (`tui-alt-screen.ts:1053-1062`). ADR-0005 §B-7 is its only caller.
///
/// Both edges mark activity, as upstream does: entering pins an `auto` bar up for as long as the
/// pointer stays (`:97`), and leaving starts the fade from that moment rather than from the last
/// movement.
pub(super) fn set_hover(bar: &mut ScrollbarView, scroll: &mut ScrollState, hover: bool) {
    if bar.hover == hover {
        return;
    }
    bar.hover = hover;
    mark_activity(scroll);
}

/// Whether the thumb is painted this frame — pi's `get isScrollbarVisible()`
/// (`components/scroll-view.ts:71-76`).
///
/// `always` shows whenever the viewport has height (`:72`); `auto` shows only while the content
/// overflows *and* the bar is transiently up (`:73-75`) — which here means the pointer holds it or
/// the last movement is younger than [`SCROLLBAR_HIDE_DELAY`]; `hidden` never.
pub(super) fn is_visible(bar: &ScrollbarView, scroll: &ScrollState) -> bool {
    match bar.mode {
        ScrollbarMode::Hidden => false,
        ScrollbarMode::Always => scroll.viewport_height > 0,
        ScrollbarMode::Auto => {
            scroll.content_height > scroll.viewport_height && (bar.hover || recently_active(scroll))
        }
    }
}

/// Whether the last movement is younger than [`SCROLLBAR_HIDE_DELAY`] — the deadline standing in
/// for upstream's `transientScrollbarVisible` flag (`components/scroll-view.ts:35`, `:92`, `:100`).
fn recently_active(scroll: &ScrollState) -> bool {
    scroll
        .last_activity
        .is_some_and(|at| Instant::now().saturating_duration_since(at) < SCROLLBAR_HIDE_DELAY)
}

/// When an `auto` bar next needs repainting because it will have faded, or `None` when nothing is
/// pending — cyrup's replacement for the `setTimeout` upstream arms (`:98-103`).
///
/// The alternate-screen loop schedules its next wake on this, exactly as it does on
/// [`super::flash::next_expiry`]. Without it the thumb would linger until some unrelated event
/// forced a frame, and "disappears 1000 ms after the last activity" would not be observable. A
/// held thumb is pinned, so it returns `None` while [`set_hover`] holds.
pub(super) fn next_hide(bar: &ScrollbarView, scroll: &ScrollState) -> Option<Instant> {
    if bar.mode != ScrollbarMode::Auto || bar.hover || !is_visible(bar, scroll) {
        return None;
    }
    scroll
        .last_activity
        .and_then(|at| at.checked_add(SCROLLBAR_HIDE_DELAY))
}

/// Whether the bar permanently narrows the content — pi's `getContentWidth`, which subtracts a
/// column for `"always"` and for nothing else (`components/scroll-view.ts:86-88`).
///
/// `auto` deliberately reserves nothing: a bar that appears and fades must not reflow the document
/// underneath it, so a transient thumb overlays the content's last column instead.
pub(super) fn reserves_column(bar: &ScrollbarView, width: u16) -> bool {
    bar.mode == ScrollbarMode::Always && width > 1
}

/// The width the document is rendered at inside `width` — pi's `getContentWidth`
/// (`components/scroll-view.ts:86-88`).
pub(super) fn content_width(bar: &ScrollbarView, width: u16) -> u16 {
    if reserves_column(bar, width) {
        width.saturating_sub(1)
    } else {
        width
    }
}

/// Where the thumb sits inside `area`, or `None` when none is painted — pi's
/// `getScrollbarGeometry(box)` (`layout.ts:278-306` @v1.0.0) with `includeHiddenAuto` false: the
/// geometry the PAINTER and a PRESS see, which exists only while the bar is visible.
///
/// `area` is the scroll document's rectangle and the only thing consulted for placement — the track
/// is its rightmost column over its full height — so a document laid out above an input dock is
/// measured by its own rect, never by the terminal's.
pub(super) fn geometry(
    bar: &ScrollbarView,
    scroll: &ScrollState,
    area: Rect,
) -> Option<ScrollbarGeom> {
    geometry_with(bar, scroll, area, false)
}

/// [`geometry`] with pi's `includeHiddenAuto` switch (`layout.ts:278`): when set, an `auto` bar that
/// is currently faded still answers with its geometry while the content overflows
/// (`canRevealHiddenAuto`, `:285`). Hover uses it (`updateScrollbarHover`,
/// `tui-alt-screen.ts:1060-1062`), because that is how a pointer arriving over the faded bar's
/// column reveals it; a press and the painter do not (`getScrollbarTargetAt(x, y)` defaults it to
/// false), so a bar nobody can see cannot be grabbed.
///
/// The arithmetic is pi's own, not ratatui's `Scrollbar` widget's: [`draw`] is a per-row painter
/// over this geometry, so the hit test and the paint share one derivation by construction and the
/// two-row minimum thumb (`Math.min(2, trackHeight)`, `:293`) lives in exactly one place.
pub(super) fn geometry_with(
    bar: &ScrollbarView,
    scroll: &ScrollState,
    area: Rect,
    include_hidden_auto: bool,
) -> Option<ScrollbarGeom> {
    // `if (!box.scrollView || box.rect.width <= 0 || box.rect.height <= 0) return undefined;`
    if area.width == 0 || area.height == 0 {
        return None;
    }
    // `canRevealHiddenAuto = includeHiddenAuto && scrollbar === "auto" && contentHeight >
    // trackHeight; if (!isScrollbarVisible && !canRevealHiddenAuto) return undefined;`
    let can_reveal_hidden_auto = include_hidden_auto
        && bar.mode == ScrollbarMode::Auto
        && scroll.content_height > usize::from(area.height);
    if !is_visible(bar, scroll) && !can_reveal_hidden_auto {
        return None;
    }
    let (thumb_offset, thumb_height) =
        thumb_span(area.height, scroll.content_height, scroll.scroll_top);
    Some(ScrollbarGeom {
        // `box.rect.x + box.rect.width - 1` (`layout.ts:303`) — the rightmost column.
        column: area.x.saturating_add(area.width.saturating_sub(1)),
        track_top: area.y,
        track_height: area.height,
        thumb_top: area.y.saturating_add(thumb_offset),
        thumb_height,
        // `Math.max(0, contentHeight - trackHeight)` (`:297`).
        max_scroll_top: scroll
            .content_height
            .saturating_sub(usize::from(area.height)),
    })
}

/// The thumb's `(offset from the track top, height)` in rows — the body of pi's
/// `getScrollbarGeometry` (`layout.ts:292-300` @v1.0.0):
///
/// ```text
/// minThumbHeight = min(2, trackHeight)
/// thumbHeight    = max(minThumbHeight, min(trackHeight, round(trackHeight² / contentHeight)))
/// maxScrollTop   = max(0, contentHeight - trackHeight)
/// thumbOffset    = maxScrollTop === 0 ? 0 : round(scrollTop / maxScrollTop * (trackHeight - thumbHeight))
/// ```
///
/// Computed in `f64` in pi's own operation order, so a quotient that lands on a half rounds the way
/// `Math.round` does (up) rather than the way an integer rounding division happens to. An empty
/// document divides by zero in JavaScript to `Infinity`, which `Math.min(trackHeight, …)` turns
/// into a full-track thumb — the `content == 0` arm.
fn thumb_span(track_height: u16, content_length: usize, scroll_top: usize) -> (u16, u16) {
    let track = f64::from(track_height);
    let min_thumb_height = track_height.min(2);
    let sized = if content_length == 0 {
        track_height
    } else {
        // `as u16` saturates, and the `min` below bounds it by the track either way.
        ((track * track) / content_length as f64).round() as u16
    };
    let thumb_height = sized.min(track_height).max(min_thumb_height);
    let max_scroll_top = content_length.saturating_sub(usize::from(track_height));
    let max_thumb_top = track_height - thumb_height;
    let thumb_offset = if max_scroll_top == 0 {
        0
    } else {
        let fraction = scroll_top as f64 / max_scroll_top as f64;
        ((fraction * f64::from(max_thumb_top)).round() as u16).min(max_thumb_top)
    };
    (thumb_offset, thumb_height)
}

/// `#[cfg(test)]`: pi's `get isScrollbarActive()` (`components/scroll-view.ts:63`), whose only
/// production reader is [`draw`] inside this module.
#[cfg(test)]
pub(super) fn is_hovered(bar: &ScrollbarView) -> bool {
    bar.hover
}

/// `#[cfg(test)]`: let the `auto` bar's fade deadline pass — a test of a faded bar cannot wait out
/// [`SCROLLBAR_HIDE_DELAY`], and the instant it compares against is private to this module.
#[cfg(test)]
pub(super) fn fade(scroll: &mut ScrollState) {
    scroll.last_activity = None;
}

/// `#[cfg(test)]`: hold an `auto` bar up for the rest of the case — the mirror of [`fade`], and
/// there for the same reason: the instant it compares against is private to this module.
///
/// [`recently_active`] measures the last movement against [`Instant::now`], so a case that scrolls
/// and *then* paints is racing [`SCROLLBAR_HIDE_DELAY`]: on a loaded machine the 1000 ms can expire
/// between those two statements, the painter correctly draws nothing, and the assertion reads as a
/// painter bug. `auto_keeps_the_cell_background_and_always_does_not` and
/// `a_double_width_glyph_under_the_bar_is_blanked` both failed exactly that way in the full `--lib`
/// binary (2038 cases over 4 threads) while passing alone and passing with their own module, and an
/// injected 1.1 s sleep reproduces the identical diff. Dating the movement in the FUTURE makes
/// `saturating_duration_since` answer zero for the rest of the case, so what these cases assert is
/// the painter and not the scheduler.
///
/// This does NOT weaken the fade itself: that it fades at all is still asserted by
/// `a_hidden_or_faded_bar_paints_nothing`, which calls [`fade`] and holds this seam at arm's length.
#[cfg(test)]
pub(super) fn hold_visible(scroll: &mut ScrollState) {
    let now = Instant::now();
    // An hour is past any case's runtime and cannot overflow an `Instant`. The `unwrap_or` is the
    // safe direction on a platform that disagreed: visible NOW, rather than the `None` that means
    // faded — which is the state this exists to rule out.
    scroll.last_activity = Some(now.checked_add(Duration::from_secs(3600)).unwrap_or(now));
}

/// The track glyph — `scrollbarTrackStyle("│")` (`layout.ts:314`).
const TRACK_GLYPH: &str = "│";
/// The resting thumb glyph — `scrollbarThumbStyle("┃")` (`:313`).
const THUMB_GLYPH: &str = "┃";
/// The thumb glyph while the pointer hovers or drags the bar — `isScrollbarActive ? "█" : "┃"`
/// (`:313`).
const THUMB_ACTIVE_GLYPH: &str = "█";

/// Paint the bar over the rightmost column of `area`, one row at a time — pi's `paintScrollbar`
/// (`layout.ts:309-330` @v1.0.0) over [`geometry`].
///
/// Every row of the track is written: `│` in the theme's `scrollbarTrack` foreground, and on the
/// thumb's rows `┃` — `█` while the pointer hovers or drags it ([`set_hover`]) — in
/// `scrollbarThumb` (both [`UiTheme`] foregrounds, falling back to `muted` / `text`). The cell's own
/// background is kept for every mode except `always` (`replaceScrollbarCell(…, scrollbar !==
/// "always")`, `:327`), where the column is reserved and the bar stands on the terminal default.
/// Everything else about the cell is replaced: pi opens the replacement with a full SGR reset, so
/// bold, underline and the old foreground do not leak onto the glyph.
///
/// `[CYRUP-DELTA]` (mechanism only): pi splices the glyph into a rendered ANSI line and pads the
/// grapheme it displaced (`replaceScrollbarCell`, `:255-277`); cyrup writes the [`Buffer`] cell and,
/// where the target is the trailing half of a double-width glyph, blanks the leading half so the
/// terminal does not draw the wide character over the bar.
pub(super) fn draw(
    bar: &ScrollbarView,
    scroll: &ScrollState,
    theme: &UiTheme,
    frame: &mut Frame,
    area: Rect,
) {
    let Some(geom) = geometry(bar, scroll, area) else {
        return;
    };
    // `theme.fg(token, …)` on a token that resolves to the terminal default emits the default-
    // foreground code, i.e. `Color::Reset`.
    let track_fg = theme.scrollbar_track().unwrap_or(Color::Reset);
    let thumb_fg = theme.scrollbar_thumb().unwrap_or(Color::Reset);
    let keep_background = bar.mode != ScrollbarMode::Always;
    let thumb_rows = geom.thumb_top..geom.thumb_top.saturating_add(geom.thumb_height);
    let buffer = frame.buffer_mut();
    for offset in 0..geom.track_height {
        let row = geom.track_top.saturating_add(offset);
        let (glyph, fg) = if thumb_rows.contains(&row) {
            let glyph = if bar.hover {
                THUMB_ACTIVE_GLYPH
            } else {
                THUMB_GLYPH
            };
            (glyph, thumb_fg)
        } else {
            (TRACK_GLYPH, track_fg)
        };
        replace_cell(
            buffer,
            Position::new(geom.column, row),
            glyph,
            fg,
            keep_background,
        );
    }
}

/// Overwrite one cell with `glyph` in foreground `fg` — pi's `replaceScrollbarCell`
/// (`layout.ts:255-277`). A position outside the buffer is skipped, which is the clip check in
/// `paintScrollbar` (`row < 0 || row >= screen.length`).
fn replace_cell(buffer: &mut Buffer, at: Position, glyph: &str, fg: Color, keep_background: bool) {
    // The cell to the left may be the leading half of a double-width glyph whose trailing half is
    // `at`: ratatui would skip our cell when it emits the wide one, so blank that half first.
    if let Some(left_x) = at.x.checked_sub(1)
        && let Some(left) = buffer.cell_mut(Position::new(left_x, at.y))
        && str_width(left.symbol()) > 1
    {
        restyle(left, " ", Color::Reset, keep_background);
    }
    if let Some(cell) = buffer.cell_mut(at) {
        restyle(cell, glyph, fg, keep_background);
    }
}

/// A full style reset, then (when preserved) the old background, then `fg` and `glyph`.
fn restyle(cell: &mut Cell, glyph: &str, fg: Color, keep_background: bool) {
    let background = cell.bg;
    cell.reset();
    cell.set_symbol(glyph).set_fg(fg);
    if keep_background {
        cell.set_bg(background);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::style::Modifier;
    use ratatui::text::Text;
    use ratatui::widgets::Paragraph;

    use super::*;

    /// Terminal width. The document rect is narrower than the terminal and SHORTER than it: the
    /// rows below it are where an input dock would be painted, and nothing here may touch them.
    const TERM_W: u16 = 30;
    const TERM_H: u16 = 20;

    fn view(mode: ScrollbarMode, content: usize, viewport: usize) -> (ScrollbarView, ScrollState) {
        let mut bar = ScrollbarView::default();
        let mut state = ScrollState::default();
        set_mode(&mut bar, &mut state, mode);
        update_layout(&mut state, content, viewport);
        // A scroll view that follows its tail starts at the bottom; pi's fixtures start at the top
        // with no movement yet, so an `auto` bar is faded until a test scrolls.
        scroll_to_row(&mut state, 0);
        fade(&mut state);
        (bar, state)
    }

    /// A theme whose track and thumb are distinguishable from each other and from `muted`/`text`.
    fn themed() -> UiTheme {
        let mut theme = UiTheme::dark();
        theme
            .roles
            .insert("scrollbarTrack".to_owned(), Color::Rgb(1, 2, 3));
        theme
            .roles
            .insert("scrollbarThumb".to_owned(), Color::Rgb(4, 5, 6));
        theme
    }

    /// Paint `texts` (one per row, from `(rect.x, rect.y)`) over a `fill` style, then the bar, and
    /// return the buffer.
    fn paint(
        bar: &ScrollbarView,
        state: &ScrollState,
        theme: &UiTheme,
        rect: Rect,
        texts: &[&str],
        fill: ratatui::style::Style,
    ) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(TERM_W, TERM_H)).unwrap();
        terminal
            .draw(|frame| {
                let lines: Vec<ratatui::text::Line<'_>> = texts
                    .iter()
                    .map(|t| ratatui::text::Line::styled((*t).to_owned(), fill))
                    .collect();
                frame.render_widget(Paragraph::new(Text::from(lines)).style(fill), rect);
                draw(bar, state, theme, frame, rect);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn symbol(buffer: &Buffer, x: u16, y: u16) -> String {
        buffer.cell((x, y)).unwrap().symbol().to_owned()
    }

    /// The glyphs down column `x` over rows `top..top + rows`.
    fn column(buffer: &Buffer, x: u16, top: u16, rows: u16) -> Vec<String> {
        (top..top + rows).map(|y| symbol(buffer, x, y)).collect()
    }

    /// `renders a proportional glyph scrollbar with an expanded active thumb`
    /// (`test/layout.test.ts:193-241` @v1.0.0): 8 rows in a 4-row viewport, scrolled by 2 — track
    /// `│` on the first and last row, `┃` on the two thumb rows, `█` while the pointer holds it —
    /// in the track and thumb FOREGROUND colours, over a document rect shorter than the terminal.
    #[test]
    fn the_bar_is_a_per_row_glyph_painter() {
        let rect = Rect::new(3, 2, 6, 4);
        let (mut bar, mut state) = view(ScrollbarMode::Auto, 8, 4);
        scroll_by(&mut state, 2);
        hold_visible(&mut state);
        let theme = themed();
        let rows = ["abcd界", "abcde2", "abcde3", "abcde4"];
        let buffer = paint(
            &bar,
            &state,
            &theme,
            rect,
            &rows,
            ratatui::style::Style::default(),
        );

        assert_eq!(column(&buffer, 8, 2, 4), ["│", "┃", "┃", "│"]);
        assert_eq!(
            buffer.cell((8, 2)).unwrap().fg,
            Color::Rgb(1, 2, 3),
            "track fg"
        );
        assert_eq!(
            buffer.cell((8, 3)).unwrap().fg,
            Color::Rgb(4, 5, 6),
            "thumb fg"
        );
        assert_eq!(buffer.cell((8, 4)).unwrap().fg, Color::Rgb(4, 5, 6));
        assert_eq!(buffer.cell((8, 5)).unwrap().fg, Color::Rgb(1, 2, 3));
        // The document text beside the bar is untouched, and nothing below the rect is.
        assert_eq!(symbol(&buffer, 3, 3), "a");
        assert!(
            column(&buffer, 8, 6, 8).iter().all(|s| s == " "),
            "the dock rows stay empty"
        );

        set_hover(&mut bar, &mut state, true);
        let buffer = paint(
            &bar,
            &state,
            &theme,
            rect,
            &rows,
            ratatui::style::Style::default(),
        );
        assert_eq!(
            column(&buffer, 8, 2, 4),
            ["│", "█", "█", "│"],
            "active thumb"
        );
    }

    /// The track is painted on EVERY row of the document rect, not just beside the thumb.
    #[test]
    fn every_row_of_the_last_column_is_track_or_thumb() {
        let rect = Rect::new(0, 3, 12, 10);
        let (bar, state) = view(ScrollbarMode::Always, 100, 10);
        let buffer = paint(
            &bar,
            &state,
            &themed(),
            rect,
            &[],
            ratatui::style::Style::default(),
        );
        let glyphs = column(&buffer, 11, 3, 10);
        assert!(
            glyphs.iter().all(|g| g == "│" || g == "┃"),
            "no gap in the track: {glyphs:?}"
        );
        assert_eq!(
            glyphs.iter().filter(|g| *g == "┃").count(),
            2,
            "min thumb is two rows"
        );
        assert_eq!(
            symbol(&buffer, 11, 2),
            " ",
            "the row above the rect is not the bar's"
        );
        assert_eq!(symbol(&buffer, 11, 13), " ", "nor the row below it");
    }

    /// pi's thumb sizing, `round(track² / content)` clamped to `[min(2, track), track]` — the
    /// four sizes in `layout.test.ts:225-237` (21 -> 19, 40 -> 10, 100 -> 4, 400 -> 2) on a 20-row
    /// track, and the one-row-track corner `min(2, 1)`.
    #[test]
    fn thumb_height_is_proportional_with_a_two_row_minimum() {
        let area = Rect::new(2, 4, 10, 20);
        for (content, expected) in [(21, 19), (40, 10), (100, 4), (400, 2), (100_000, 2)] {
            let (bar, state) = view(ScrollbarMode::Always, content, 20);
            let geom = geometry(&bar, &state, area).unwrap();
            assert_eq!(geom.thumb_height, expected, "content {content}");
            assert_eq!(geom.track_height, 20);
            assert_eq!(geom.track_top, 4, "placement comes from the rect");
            assert_eq!(geom.column, 11);
        }
        let one_row = Rect::new(0, 0, 4, 1);
        let (bar, state) = view(ScrollbarMode::Always, 500, 1);
        assert_eq!(geometry(&bar, &state, one_row).unwrap().thumb_height, 1);
        let three_rows = Rect::new(0, 0, 4, 3);
        let (bar, state) = view(ScrollbarMode::Always, 9_000, 3);
        assert_eq!(geometry(&bar, &state, three_rows).unwrap().thumb_height, 2);
    }

    /// The thumb offset is `round(scrollTop / maxScrollTop * (track - thumb))` (`layout.ts:299`):
    /// top at the start, bottom at the end, and exactly half way at a half-way scrollTop.
    #[test]
    fn thumb_offset_follows_the_scroll_position() {
        let area = Rect::new(0, 1, 8, 10);
        let (bar, mut state) = view(ScrollbarMode::Always, 50, 10);
        assert_eq!(geometry(&bar, &state, area).unwrap().thumb_top, 1);
        scroll_to_row(&mut state, 20);
        // thumb 2 rows, 8 free: round(20/40 * 8) = 4 -> top at 1 + 4.
        assert_eq!(geometry(&bar, &state, area).unwrap().thumb_top, 5);
        scroll_to_bottom(&mut state);
        let geom = geometry(&bar, &state, area).unwrap();
        assert_eq!(geom.thumb_top + geom.thumb_height, area.y + area.height);
        assert_eq!(geom.max_scroll_top, 40);
    }

    /// `always` with content that FITS: pi still shows the bar and the thumb fills the track
    /// (`layout.test.ts:268-272`), and the column is reserved.
    #[test]
    fn an_always_bar_over_fitting_content_is_one_full_thumb() {
        let rect = Rect::new(0, 0, 6, 4);
        let (bar, state) = view(ScrollbarMode::Always, 2, 4);
        let buffer = paint(
            &bar,
            &state,
            &themed(),
            rect,
            &[],
            ratatui::style::Style::default(),
        );
        assert_eq!(column(&buffer, 5, 0, 4), ["┃", "┃", "┃", "┃"]);
    }

    /// `preserves only the underlying background beneath overlay scrollbar glyphs`
    /// (`layout.test.ts:306-332`): in `auto` the cell keeps its background and loses its other
    /// styling — the old foreground and bold do not leak onto the glyph — while `always` stands on
    /// the terminal default.
    #[test]
    fn auto_keeps_the_cell_background_and_always_does_not() {
        let rect = Rect::new(0, 0, 6, 4);
        let fill = ratatui::style::Style::default()
            .bg(Color::Green)
            .fg(Color::Red)
            .add_modifier(Modifier::BOLD);
        let rows = ["xxxxxx", "xxxxxx", "xxxxxx", "xxxxxx"];

        let (mut bar, mut state) = view(ScrollbarMode::Auto, 8, 4);
        scroll_by(&mut state, 1);
        hold_visible(&mut state);
        let buffer = paint(&bar, &state, &themed(), rect, &rows, fill);
        for y in 0..4 {
            let cell = buffer.cell((5, y)).unwrap();
            assert!(matches!(cell.symbol(), "│" | "┃"), "row {y}");
            assert_eq!(
                cell.bg,
                Color::Green,
                "row {y}: the document's background is kept"
            );
            assert_ne!(
                cell.fg,
                Color::Red,
                "row {y}: the old foreground is replaced"
            );
            assert!(
                !cell.modifier.contains(Modifier::BOLD),
                "row {y}: bold does not leak"
            );
            assert_eq!(
                buffer.cell((4, y)).unwrap().bg,
                Color::Green,
                "neighbours untouched"
            );
        }

        set_mode(&mut bar, &mut state, ScrollbarMode::Always);
        let buffer = paint(&bar, &state, &themed(), rect, &rows, fill);
        for y in 0..4 {
            assert_eq!(
                buffer.cell((5, y)).unwrap().bg,
                Color::Reset,
                "row {y}: `always` resets"
            );
        }
    }

    /// `renders a proportional glyph scrollbar …` at the start (`layout.test.ts:244-246`): the
    /// bar column is the trailing half of a double-width glyph, so the glyph is replaced by a
    /// space and the bar takes the cell — `"abcd ┃"`.
    #[test]
    fn a_double_width_glyph_under_the_bar_is_blanked() {
        let rect = Rect::new(0, 0, 6, 4);
        let (bar, mut state) = view(ScrollbarMode::Auto, 8, 4);
        scroll_to_row(&mut state, 2);
        scroll_to_top(&mut state);
        hold_visible(&mut state);
        let rows = ["abcd界", "abcde2", "abcde3", "abcde4"];
        let buffer = paint(
            &bar,
            &state,
            &themed(),
            rect,
            &rows,
            ratatui::style::Style::default(),
        );
        let first_row: String = (0..6).map(|x| symbol(&buffer, x, 0)).collect();
        assert_eq!(first_row, "abcd ┃");
    }

    /// Nothing is painted when the bar is not showing: `hidden`, and a faded `auto` bar — whose
    /// geometry exists only for a hover lookup, never for the painter.
    #[test]
    fn a_hidden_or_faded_bar_paints_nothing() {
        let rect = Rect::new(0, 0, 6, 4);
        let rows = ["abcde1", "abcde2", "abcde3", "abcde4"];
        let (bar, mut state) = view(ScrollbarMode::Hidden, 100, 4);
        scroll_by(&mut state, 3);
        let buffer = paint(
            &bar,
            &state,
            &themed(),
            rect,
            &rows,
            ratatui::style::Style::default(),
        );
        assert_eq!(column(&buffer, 5, 0, 4), ["1", "2", "3", "4"]);

        // `auto` that has never moved is faded.
        let (bar, state) = view(ScrollbarMode::Auto, 100, 4);
        assert!(!is_visible(&bar, &state));
        assert!(geometry(&bar, &state, rect).is_none());
        let buffer = paint(
            &bar,
            &state,
            &themed(),
            rect,
            &rows,
            ratatui::style::Style::default(),
        );
        assert_eq!(column(&buffer, 5, 0, 4), ["1", "2", "3", "4"]);
        // …but a hover lookup finds it while the content overflows (`includeHiddenAuto`).
        assert!(geometry_with(&bar, &state, rect, true).is_some());
        let (bar_fit, state_fit) = view(ScrollbarMode::Auto, 4, 4);
        assert!(
            geometry_with(&bar_fit, &state_fit, rect, true).is_none(),
            "nothing to scroll"
        );
    }

    /// With no `scrollbarTrack`/`scrollbarThumb` in the theme the painter uses the theme's own
    /// `muted` and `text` (`theme.ts:173-174`) — never a background, never a hardcoded colour.
    #[test]
    fn omitted_theme_tokens_paint_muted_and_text() {
        let rect = Rect::new(0, 0, 6, 4);
        // pi's own `dark.json` configures both tokens; the fallback is for a theme that omits them.
        let mut theme = UiTheme::dark();
        theme.roles.remove("scrollbarTrack");
        theme.roles.remove("scrollbarThumb");
        let (bar, mut state) = view(ScrollbarMode::Auto, 8, 4);
        scroll_by(&mut state, 2);
        hold_visible(&mut state);
        let buffer = paint(
            &bar,
            &state,
            &theme,
            rect,
            &[],
            ratatui::style::Style::default(),
        );
        assert_eq!(Some(buffer.cell((5, 0)).unwrap().fg), theme.muted);
        assert_eq!(Some(buffer.cell((5, 1)).unwrap().fg), theme.foreground);
        assert_eq!(
            buffer.cell((5, 1)).unwrap().bg,
            Color::Reset,
            "the thumb is not a background fill"
        );
    }
}
