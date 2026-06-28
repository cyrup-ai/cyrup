//! The deterministic cut-point algorithm (arch-05 §3.3/§6.2, R-05-005/006/007). Pure, no I/O.
//!
//! The cut MUST NOT fall between a tool call and its tool result: `ToolResult` entries are never
//! valid cut points, so the first-kept boundary always snaps to a user/assistant/custom/summary
//! entry, keeping a tool call and its following results on the same side.

use cyrup_core::Message;

use crate::compaction::tokens::TokenCache;
use crate::entry::{Entry, KnownEntry};

/// The chosen boundary between summarized-history and kept-recent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CutPoint {
    /// Index of the first entry to KEEP verbatim.
    pub first_kept_index: usize,
    /// User message index that starts the turn being split, or `None`.
    pub turn_start_index: Option<usize>,
    /// True iff the cut lands mid-turn (the cut entry is not a user message).
    pub is_split_turn: bool,
}

/// Whether an entry may serve as a cut boundary. Everything except a `ToolResult` message qualifies
/// (a tool result must stay with the assistant call that produced it).
fn is_valid_cut_point(entry: &Entry) -> bool {
    !matches!(
        entry,
        Entry::Known(KnownEntry::Message { message: Message::ToolResult { .. }, .. })
    )
}

fn is_user_entry(entry: &Entry) -> bool {
    matches!(entry, Entry::Known(KnownEntry::Message { message: Message::User { .. }, .. }))
}

/// Valid cut boundaries (indices) in `[start, end)`; never a `ToolResult` (R-05-005).
pub fn find_valid_cut_points(entries: &[Entry], start: usize, end: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        if let Some(e) = entries.get(i)
            && is_valid_cut_point(e) {
                out.push(i);
            }
        i += 1;
    }
    out
}

/// First user entry at or before `idx` (turn start), bounded by `start`.
pub fn find_turn_start(entries: &[Entry], idx: usize, start: usize) -> Option<usize> {
    let mut i = idx.min(entries.len());
    loop {
        if let Some(e) = entries.get(i)
            && is_user_entry(e) {
                return Some(i);
            }
        if i <= start {
            return None;
        }
        i -= 1;
    }
}

/// Walk backward from `end` accumulating `estimate_tokens` until `keep_recent_tokens` is reached,
/// then snap to the nearest valid cut point at or after that entry. Mirrors Pi `findCutPoint`.
pub fn find_cut_point(
    entries: &[Entry],
    cache: &TokenCache,
    start: usize,
    end: usize,
    keep_recent_tokens: u32,
) -> CutPoint {
    let valid = find_valid_cut_points(entries, start, end);

    // Walk backward, summing per-entry estimates until we cross the keep-recent budget.
    let mut acc: u32 = 0;
    let mut cut_idx = start;
    let mut crossed = false;
    let mut i = end;
    while i > start {
        i -= 1;
        if let Some(e) = entries.get(i) {
            acc = acc.saturating_add(cache.estimate_entry(e));
        }
        if acc >= keep_recent_tokens {
            cut_idx = i;
            crossed = true;
            break;
        }
    }
    if !crossed {
        cut_idx = start;
    }

    // Snap to the nearest valid cut point at or after `cut_idx`; absent one, keep nothing extra.
    let first_kept = valid.iter().copied().find(|&v| v >= cut_idx).unwrap_or(end);

    // Split-turn iff the cut entry is not a user message and a turn start exists before it.
    let is_user = entries.get(first_kept).is_some_and(is_user_entry);
    let (is_split_turn, turn_start_index) = if is_user {
        (false, None)
    } else {
        match find_turn_start(entries, first_kept, start) {
            Some(ts) if ts < first_kept => (true, Some(ts)),
            _ => (false, None),
        }
    };

    CutPoint { first_kept_index: first_kept, turn_start_index, is_split_turn }
}
