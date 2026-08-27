//! **Semantic-prompt navigation** — the `ctrl+shift+up` / `ctrl+shift+down` jump between prompts,
//! cyrup's port of pi's `TuiAltScreen.scrollToPrompt`
//! (`packages/tui/src/tui-alt-screen.ts:412-424` @v0.84.3; ADR-0005 cites `:366-379`, the @v0.84.1
//! line numbering for the same method). ADR-0005 §Decision B-10.
//!
//! # The sanctioned mechanism difference
//! Upstream has only rendered lines to work with, so it scans them for the OSC 133 prompt-start
//! mark: `OSC133_PROMPT_START` (`tui-alt-screen.ts:63`, the @v0.84.1 `:56` the ADR cites) is tested
//! against each candidate row (`:419`), and the mark itself is stripped again before the row is
//! painted (`:318`, `:1285`) so it is never visible. cyrup retains structured [`Entry`]s instead
//! (ADR-0005 §B-1), so a prompt is [`Entry::User`] — directly identifiable, with no text scan and no
//! escape-sequence round trip. ADR-0005 §B-10 authorises exactly that substitution and forbids the
//! alternative: cyrup emits no OSC 133 anywhere in the crate — ADR-0005 §B-10's acceptance criterion
//! is a grep for the escape's `133` payload over `crates/cyrup-tui/src` returning nothing, which is
//! why not even a doc comment here spells the sequence out — and manufacturing marks purely so this
//! module could re-parse them would be a strictly worse mechanism for the identical result.
//!
//! One consequence of the substitution is worth recording so a later reader does not mistake it for
//! an oversight. Upstream's mark is written by the *message components*, and a tool-call-free
//! assistant message carries it too (`components/user-message.ts:66` and
//! `components/assistant-message.ts:80-84`, both in `packages/coding-agent/src/modes/interactive`),
//! so pi's scan also halts on those. ADR-0005 §B-10 names [`Entry::User`] as cyrup's anchor, and
//! that is what this module walks — a deliberate narrowing pinned by the ADR, not a difference this
//! unit may quietly widen.
//!
//! # The search rule, which is upstream's and is load-bearing
//! `for (let row = scrollView.scrollTop + direction; row >= 0 && row < lines.length; row += direction)`
//! (`tui-alt-screen.ts:418`) says three things, and all three are ported literally by
//! [`scroll_to_prompt`]:
//!
//! 1. The scan starts **strictly past** the current offset — `scrollTop + direction`, never
//!    `scrollTop` — which is what makes repeated presses of the same binding advance instead of
//!    sticking on the prompt already at the top of the viewport.
//! 2. It stops at the first match **in the search direction**: the nearest candidate row below the
//!    offset going forward, the nearest above it going back.
//! 3. It walks only rows that exist (`row >= 0 && row < lines.length`), and with **no match it does
//!    nothing at all** (`:423-424` falls out of the loop) — no wrap to the far end, no clamp to an
//!    edge, no scrollbar activity, not even a re-render request.
//!
//! # This module holds no state
//! Per `altscreen/mod.rs` rule 2, the rendered document is not owned here: the entry list and the
//! entry-index-to-first-row map arrive as arguments from the per-frame document cache (ADR-0005
//! §B-5), which is also where [`super::scroll::row_of_entry`] — the one lookup into that map —
//! lives. The jump itself is nothing but a [`super::scroll::scroll_to_row`], so `follow: end`,
//! the clamp and the scrollbar-activity mark are the scroll model's, exactly as upstream's
//! `scrollView.scrollTo(row)` (`:420`) leaves all three to `ScrollView`
//! (`components/scroll-view.ts:119-138`).
//!
//! # Who calls it
//! ADR-0005 §B-3's input dispatcher, on the two outcomes [`super::keys::route`] reports rather than
//! performs: [`super::keys::KeyOutcome::PreviousPrompt`] runs [`previous`] and
//! [`super::keys::KeyOutcome::NextPrompt`] runs [`next`], which is upstream's
//! `if (!isRelease) this.scrollToPrompt(-1)` / `(1)` (`tui-alt-screen.ts:629-636`). The key stays
//! consumed either way, including when the walk finds nothing — the binding resolves where the §B-9
//! shadowing rule is, and the walk happens where the document is. Upstream additionally calls
//! `requestRender()` on a successful jump (`:421`); cyrup's equivalent is the alternate-screen loop
//! redrawing on the next tick, as it is for every other mutator in this tree.

use super::scroll::{self, ScrollState};
use crate::transcript::Entry;

/// Which way [`scroll_to_prompt`] walks — pi's `direction: -1 | 1` (`tui-alt-screen.ts:412`), which
/// is both the step and the sign of the comparison against the current offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    /// Upstream's `-1`, reached from `tui.altScreen.previousPrompt` (`tui-alt-screen.ts:629-632`):
    /// the nearest prompt row **above** the current offset.
    Previous,
    /// Upstream's `1`, reached from `tui.altScreen.nextPrompt` (`:633-636`): the nearest prompt row
    /// **below** the current offset.
    Next,
}

/// Jump to the previous prompt — pi's `scrollToPrompt(-1)` (`tui-alt-screen.ts:629-632`).
///
/// See [`scroll_to_prompt`] for the arguments and for the "strictly past the current offset, no-op
/// when there is no match" rule both directions share.
pub(super) fn previous(scroll: &mut ScrollState, entries: &[Entry], row_starts: &[usize]) {
    scroll_to_prompt(scroll, entries, row_starts, Direction::Previous);
}

/// Jump to the next prompt — pi's `scrollToPrompt(1)` (`tui-alt-screen.ts:633-636`).
///
/// See [`scroll_to_prompt`] for the arguments and the shared rule.
pub(super) fn next(scroll: &mut ScrollState, entries: &[Entry], row_starts: &[usize]) {
    scroll_to_prompt(scroll, entries, row_starts, Direction::Next);
}

/// The walk itself — pi's `scrollToPrompt(direction)` (`tui-alt-screen.ts:412-424`).
///
/// `entries` is the retained document as of this frame ([`crate::TranscriptView::document`]) and
/// `row_starts` is the entry-index-to-first-row map rebuilt beside it (ADR-0005 §B-5); the two are
/// read together through [`scroll::row_of_entry`], so an entry the map does not cover is simply not
/// a candidate rather than an error — which is also how upstream's `lines[row] ?? ""`
/// (`tui-alt-screen.ts:419`) absorbs a row it cannot resolve.
///
/// Rows at or past the rendered content are skipped, which is upstream's `row < lines.length`
/// guard (`:418`) over the same quantity: [`scroll::content_height`] is the height of the document
/// the last layout measured, so before the first frame it is zero and nothing is a candidate —
/// upstream's `if (!this.currentLayout) return` (`:413`) refuses the same walk for the same reason.
///
/// The landing is [`scroll::scroll_to_row`], upstream's `scrollView.scrollTo(row)` (`:420`), so a
/// prompt inside the last viewport-height of the document lands clamped to the maximum offset and
/// re-arms the tail follow exactly as it does upstream (`components/scroll-view.ts:119-138`).
fn scroll_to_prompt(
    scroll: &mut ScrollState,
    entries: &[Entry],
    row_starts: &[usize],
    direction: Direction,
) {
    let content_height = scroll::content_height(scroll);
    let from = scroll::scroll_top(scroll);
    let prompt_rows = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            // The whole of cyrup's substitution for `OSC133_PROMPT_START.test(...)` (`:419`): a
            // prompt is an entry kind, and its row is the map's, not a parse of painted text.
            Entry::User { .. } => scroll::row_of_entry(row_starts, index),
            _ => None,
        })
        .filter(|row| *row < content_height);
    // `scrollTop + direction` as the first row examined (`:418`): the comparison is strict in both
    // directions, and `max`/`min` pick the first match the loop would have reached without assuming
    // the map is sorted. A `Previous` walk from row 0 finds nothing, which is upstream's loop
    // refusing to start at `row === -1`.
    let target = match direction {
        Direction::Previous => prompt_rows.filter(|row| *row < from).max(),
        Direction::Next => prompt_rows.filter(|row| *row > from).min(),
    };
    // No match ahead is a total no-op — no wrap, no clamp to an edge (`:423-424`).
    let Some(row) = target else {
        return;
    };
    scroll::scroll_to_row(scroll, row);
}
