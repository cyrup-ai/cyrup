//! **Click-to-toggle** over the retained document and the in-flight turn: which block a click
//! landed on, and keeping the viewport where the user was looking once that block changes height.
//!
//! # Upstream
//! Pi's thinking runs, completed tool blocks, branch and compaction summaries and skill invocations
//! each wrap their content in a `MouseRegion` that flips the component's own expansion on a left
//! `click` (`assistant-message.ts:164`, `tool-execution.ts:175-181`, `branch-summary-message.ts`,
//! `compaction-summary-message.ts`, `skill-invocation-message.ts` @v1.0.0). A `click` is the
//! alternate screen's own synthesis: a release on the cell of the press, with no drag in between
//! (`tui-alt-screen.ts:1324-1328`); a link under the pointer is activated first
//! (`:1329-1341`), and a click a component handled clears the text selection instead of copying it
//! (`:1342-1353`). The condition is `button === "left"` — neither the click count nor the modifiers
//! are read.
//!
//! cyrup has no component tree to hit-test, so the hit test is data: [`super::document`] records,
//! per committed entry, the cells its region covers relative to the entry's first row, and the
//! transcript's live render records the same for the blocks of the in-flight turn (a streaming
//! thinking run, a tool with a result), which sit after the committed rows. [`hit_at`] resolves a
//! document row against both.
//!
//! # Keeping the viewport put — a deliberate deviation from pi
//! **This is not pi's behaviour, and is isolated in [`anchor`] / [`restore`] for the owner to veto.**
//! Pi's `ScrollView.updateLayout` only clamps the offset (`components/scroll-view.ts:187`), so
//! collapsing a block that begins above the viewport top leaves the viewport over whatever now sits
//! at the old offset. Here the row the user clicked is put back where it was on screen — or, when
//! the block no longer has that many rows, its last clickable row, so the pointer stays over the
//! block it toggled. For a click on a block whose first row is on screen the result is pi's. Removing
//! the deviation means deleting the [`restore`] call in `AltScreen::sync_document`.
//!
//! A view following the tail is left to its follow, as pi's is (`:186`); anchoring would release it.

use super::scroll::{self, ScrollState};
use crate::transcript::{LiveBlock, LiveToggle, ToggleRegion, ToggleTarget};

/// A left click that landed on a toggle region: the answer [`hit_at`] gives and
/// [`super::PointerOutcome::Toggle`] carries to the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToggleHit {
    target: ToggleTarget,
    /// The clicked row, relative to the block's first row.
    offset: usize,
    /// The clicked row in the whole rendered document.
    row: usize,
}

impl ToggleHit {
    /// What was clicked, as [`crate::TranscriptView::toggle`] takes it.
    pub fn target(&self) -> ToggleTarget {
        self.target
    }
}

/// The toggle region under document cell (`row`, `col`), if any.
///
/// Rows below `committed_rows` belong to retained entries; a zero-row entry shares the next entry's
/// start, so `partition_point` — the last start at or before `row` — lands on the entry that has the
/// row. Rows from `committed_rows` on are the in-flight turn's, addressed through `live`.
pub(super) fn hit_at(
    row_starts: &[usize],
    toggles: &[Option<ToggleRegion>],
    committed_rows: usize,
    live: &[LiveToggle],
    row: usize,
    col: usize,
) -> Option<ToggleHit> {
    if let Some(rel) = row.checked_sub(committed_rows) {
        return live.iter().find_map(|t| {
            let offset = rel.checked_sub(t.start)?;
            t.region.contains(offset, col).then_some(ToggleHit {
                target: ToggleTarget::Live(t.block),
                offset,
                row,
            })
        });
    }
    let entry = row_starts
        .partition_point(|&start| start <= row)
        .checked_sub(1)?;
    let offset = row.checked_sub(*row_starts.get(entry)?)?;
    let region = toggles.get(entry)?.as_ref()?;
    region.contains(offset, col).then_some(ToggleHit {
        target: ToggleTarget::Entry(entry),
        offset,
        row,
    })
}

/// What an [`Anchor`] is attached to.
#[derive(Clone, Copy, Debug)]
enum AnchorTarget {
    /// A retained entry, by sequence number (`retained_dropped + index`): stable across a front trim.
    Entry(u64),
    /// A block of the in-flight turn.
    Live(LiveBlock),
}

/// Where the toggled block's clicked row stood, so [`restore`] can put it back.
#[derive(Clone, Copy, Debug)]
pub(super) struct Anchor {
    target: AnchorTarget,
    /// The clicked row, relative to the block's first row.
    offset: usize,
    /// The clicked row's distance below the viewport top.
    screen_row: usize,
    /// Whether the view was following the tail when the click landed.
    following: bool,
}

/// Record the anchor for `hit` against the viewport as it is now.
pub(super) fn anchor(hit: ToggleHit, dropped: u64, scroll: &ScrollState) -> Anchor {
    let target = match hit.target {
        ToggleTarget::Entry(index) => {
            AnchorTarget::Entry(dropped.saturating_add(u64::try_from(index).unwrap_or(u64::MAX)))
        }
        ToggleTarget::Live(block) => AnchorTarget::Live(block),
    };
    Anchor {
        target,
        offset: hit.offset,
        screen_row: hit.row.saturating_sub(scroll::scroll_top(scroll)),
        following: scroll::is_following_end(scroll),
    }
}

/// The rows the anchored block occupies in the rebuilt document: its first row, and the last row it
/// still answers a click on.
fn span(
    anchor: &Anchor,
    row_starts: &[usize],
    toggles: &[Option<ToggleRegion>],
    committed_rows: usize,
    dropped: u64,
    live: &[LiveToggle],
) -> Option<(usize, usize)> {
    match anchor.target {
        AnchorTarget::Live(block) => {
            let t = live.iter().find(|t| t.block == block)?;
            Some((committed_rows + t.start, t.region.last_row()?))
        }
        AnchorTarget::Entry(seq) => {
            let index = usize::try_from(seq.checked_sub(dropped)?).ok()?;
            let start = *row_starts.get(index)?;
            let end = row_starts.get(index + 1).copied().unwrap_or(committed_rows);
            let last_of_entry = end.checked_sub(start)?.checked_sub(1)?;
            // The entry's own last row when it has no region any more.
            let last = toggles
                .get(index)
                .and_then(Option::as_ref)
                .and_then(ToggleRegion::last_row)
                .map_or(last_of_entry, |r| r.min(last_of_entry));
            Some((start, last))
        }
    }
}

/// Re-anchor the viewport on the rebuilt document.
///
/// `content_height` is the rebuilt document's full height (live rows included), applied first so the
/// clamp the move runs against is the new one, not the previous frame's. A view that was following
/// the tail keeps following it; a block the front trim evicted, or that committed away, in the
/// meantime has nothing to anchor to.
#[allow(clippy::too_many_arguments)]
pub(super) fn restore(
    anchor: Anchor,
    scroll: &mut ScrollState,
    row_starts: &[usize],
    toggles: &[Option<ToggleRegion>],
    committed_rows: usize,
    dropped: u64,
    live: &[LiveToggle],
    content_height: usize,
) {
    let viewport = scroll::viewport_height(scroll);
    scroll::update_layout(scroll, content_height, viewport);
    if anchor.following {
        return;
    }
    let Some((start, last)) = span(&anchor, row_starts, toggles, committed_rows, dropped, live)
    else {
        return;
    };
    let row = start + anchor.offset.min(last);
    scroll::scroll_to_row(scroll, row.saturating_sub(anchor.screen_row));
}
