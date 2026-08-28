//! The **scroll model** over the retained document and the **scrollbar** that reports it — cyrup's
//! port of pi's `ScrollView` (`packages/tui/src/components/scroll-view.ts` @v0.84.3) together with
//! the thumb geometry the layout pass derives from one (`layout.ts:266-290`). ADR-0005 §Decision
//! B-5.
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
//! `overscroll: "chain"`, its scrollbar taken from the `fullscreenScrollbar` setting and its thumb
//! styled with `theme.bg("scrollbarThumb", …)` (`interactive-mode.ts:918-923`). Because there is
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
//! arrive through [`update_layout`], the thumb colour arrives as a `&UiTheme` argument at paint
//! time, and the document itself is never touched — which is what lets
//! [`crate::ViewportRenderer::scroll_by`] and its siblings work from `&mut self` alone.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Frame;

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

/// The scrollbar policy and the ratatui widget state that paints it — pi's `ScrollView` scrollbar
/// half (`components/scroll-view.ts:27-37`).
///
/// Held beside a [`ScrollState`] in the renderer's UI bag. Every mutator below takes both, because
/// upstream's answers are joint: whether the bar shows depends on the heights, and every movement
/// re-arms the bar.
#[derive(Default)]
pub(super) struct ScrollbarView {
    /// Which policy is in force — `currentScrollbar` (`:27`).
    mode: ScrollbarMode,
    /// Whether the pointer is over the thumb, or a drag holds it — `scrollbarActive` (`:36`),
    /// written by `setScrollbarActive` (`:113-117`) from the alternate screen's hover tracking
    /// (`tui-alt-screen.ts:735-743`). While it holds, an `auto` bar does not fade: upstream arms no
    /// hide timer at all in that case (`components/scroll-view.ts:97`). ADR-0005 §B-7 is its only
    /// writer, through [`set_hover`].
    hover: bool,
    /// The ratatui widget's own view of content length, position and viewport length, refreshed
    /// from [`ScrollState`] on every [`draw`]. Carried across frames only because
    /// [`ratatui::widgets::Scrollbar`] renders statefully; it is never the source of truth.
    state: ScrollbarState,
}

/// Where the thumb is — pi's `ScrollbarGeometry` (`layout.ts:38-45`), in absolute frame
/// coordinates.
///
/// This is ADR-0005 §B-7's only window into the thumb: the drag hit-tests
/// `x == column && thumb_top <= y < thumb_top + thumb_height` (`tui-alt-screen.ts:725-730`) and
/// converts a pointer row into an offset with
/// `round(thumb_offset / (track_height - thumb_height) * max_scroll_top)` (`:760-768`). Deriving it
/// anywhere else would let the hit test and the paint disagree.
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
/// (`components/scroll-view.ts:59-61`).
/// `#[cfg(test)]`: pi's `get isFollowingEnd()` (`components/scroll-view.ts:59-61`), reached through
/// `TuiAltScreen.isFollowingOutput`, whose every upstream reference is in
/// `test/tui-alt-screen.test.ts`. The flag itself is maintained and read inside this module.
#[cfg(test)]
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
/// This is where `follow: end` earns its keep: while [`is_following_end`] holds, the offset is
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

/// Report the pointer entering or leaving the thumb — pi's `setScrollbarActive`
/// (`components/scroll-view.ts:113-117`), driven by the alternate screen's hover tracking
/// (`tui-alt-screen.ts:735-743`). ADR-0005 §B-7 is its only caller.
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
/// `getScrollbarGeometry(box)` (`layout.ts:266-290`).
///
/// The thumb *arithmetic* is ratatui's, not upstream's: [`draw`] paints with
/// [`ratatui::widgets::Scrollbar`], so the hit test must agree with that widget's own idea of the
/// thumb rather than with pi's `round(trackHeight² / contentHeight)` (`layout.ts:273-276`). The two
/// place the thumb within a row of each other and differ only in rounding; a hit test that
/// disagreed with the paint by even one row would be a bug the user feels. The reproduction below
/// mirrors `Scrollbar::part_lengths` and is fed the identical `ScrollbarState` inputs [`draw`]
/// uses, so the agreement holds by construction.
pub(super) fn geometry(
    bar: &ScrollbarView,
    scroll: &ScrollState,
    area: Rect,
) -> Option<ScrollbarGeom> {
    // `if (!box.scrollView?.isScrollbarVisible || …) return undefined;` (`layout.ts:267`).
    if !is_visible(bar, scroll) || area.width == 0 || area.height == 0 {
        return None;
    }
    let (thumb_offset, thumb_height) = thumb_span(
        area.height,
        scroll.content_height,
        scroll.viewport_height,
        scroll.scroll_top,
    )?;
    Some(ScrollbarGeom {
        // `box.rect.x + box.rect.width - 1` (`layout.ts:280`) — the rightmost column, which is also
        // the one `Scrollbar` takes for `ScrollbarOrientation::VerticalRight`.
        column: area.x.saturating_add(area.width.saturating_sub(1)),
        track_top: area.y,
        track_height: area.height,
        thumb_top: area.y.saturating_add(thumb_offset),
        thumb_height,
        max_scroll_top: max_scroll_top_of(scroll),
    })
}

/// The thumb's `(offset from the track top, height)` in rows, reproducing
/// `ratatui_widgets::scrollbar::Scrollbar::part_lengths` for the arrow-less vertical bar [`draw`]
/// configures. See [`geometry`] for why this is ratatui's arithmetic rather than pi's.
fn thumb_span(
    track_height: u16,
    content_length: usize,
    viewport_length: usize,
    position: usize,
) -> Option<(u16, u16)> {
    // `Scrollbar::render` draws nothing at all in either of these cases.
    if track_height == 0 || content_length == 0 {
        return None;
    }
    let track = usize::from(track_height);
    // `Scrollbar::viewport_length` falls back to the track when the state carries no viewport
    // length, which is the shape of a frame drawn before the first `update_layout`. Mirrored here
    // so the two never disagree about the thumb in that window.
    let viewport = if viewport_length == 0 {
        track
    } else {
        viewport_length
    };
    let max_position = content_length.saturating_sub(1);
    let start_position = position.min(max_position);
    let max_viewport_position = max_position.saturating_add(viewport);
    if max_viewport_position == 0 {
        // ratatui's own division-by-zero guard, which yields a full-track thumb.
        return Some((0, track_height));
    }
    let height = rounding_divide(viewport.saturating_mul(track), max_viewport_position)
        .max(1)
        .min(track);
    let offset = rounding_divide(start_position.saturating_mul(track), max_viewport_position)
        .min(track.saturating_sub(height));
    // Both values are already clamped into `0..=track`, so the conversions cannot narrow.
    let height = u16::try_from(height)
        .unwrap_or(track_height)
        .min(track_height);
    let offset = u16::try_from(offset)
        .unwrap_or(0)
        .min(track_height.saturating_sub(height));
    Some((offset, height))
}

/// Integer division rounding to nearest instead of down, as ratatui's private `rounding_divide`
/// does, with the zero denominator its caller rules out made explicit.
fn rounding_divide(numerator: usize, denominator: usize) -> usize {
    if denominator == 0 {
        return 0;
    }
    numerator.saturating_add(denominator / 2) / denominator
}

/// Paint the thumb over the rightmost column of `area` — pi's `paintScrollbar`
/// (`layout.ts:292-302`), which styles only the thumb rows and leaves the track alone.
///
/// The colour is [`UiTheme`]'s already-resolved `scrollbarThumb`, which upstream applies as
/// `scrollbarStyle: (text) => theme.bg("scrollbarThumb", text)` (`interactive-mode.ts:923`). It is
/// a *background* token that falls back to the theme's own `selectedBg` in the loader
/// (`theme.rs:507`, `theme.rs:1056-1060`), so no new theme key is introduced and a theme that omits
/// it still paints. A theme that resolves it to nothing paints nothing, which is upstream's
/// behaviour when `theme.bg` has no colour to emit.
///
/// The track, the arrow heads and the pi-style glyph restyle are all suppressed: `begin_symbol`,
/// `end_symbol` and `track_symbol` are `None` so only the thumb rows are written, and the thumb is
/// a space carrying the background colour. That covers the cell underneath, where upstream restyles
/// the character in place (`layout.ts:300`) — visible only for `auto`, which by design does not
/// reserve a column of its own.
pub(super) fn draw(
    bar: &mut ScrollbarView,
    scroll: &ScrollState,
    theme: &UiTheme,
    frame: &mut Frame,
    area: Rect,
) {
    if !is_visible(bar, scroll) || area.width == 0 || area.height == 0 {
        return;
    }
    let Some(thumb) = theme.backgrounds().scrollbar_thumb else {
        return;
    };
    // The same three inputs `thumb_span` reads, so the painted thumb and `geometry` agree.
    bar.state = ScrollbarState::new(scroll.content_height)
        .viewport_content_length(scroll.viewport_height)
        .position(scroll.scroll_top);
    let widget = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(None)
        .thumb_symbol(" ")
        .thumb_style(Style::default().bg(thumb));
    frame.render_stateful_widget(widget, area, &mut bar.state);
}
