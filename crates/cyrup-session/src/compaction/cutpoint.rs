//! The deterministic cut-point algorithm (arch-05 §3.3/§6.2, R-05-005/006/007). Pure, no I/O.
//!
//! The cut MUST NOT fall between a tool call and its tool result: `ToolResult` entries are never
//! valid cut points, so the first-kept boundary always snaps to a user/assistant/bash/custom/summary
//! entry, keeping a tool call and its following results on the same side. Mirrors Pi
//! `findValidCutPoints`/`findTurnStartIndex`/`findCutPoint` (`compaction.ts:305-454`).

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

/// Whether an entry may serve as a cut boundary (Pi `findValidCutPoints`, `compaction.ts:305-343`):
/// a `message` entry whose role is NOT `toolResult`, a `branch_summary` entry, or a `custom_message`
/// entry. `model_change`/`thinking_level_change`/`compaction`/`custom`/`label`/`session_info` are
/// explicitly NOT valid.
fn is_valid_cut_point(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::Known(KnownEntry::Message { message, .. }) if !message.is_tool_result()
    ) || matches!(
        entry,
        Entry::Known(KnownEntry::BranchSummary { .. } | KnownEntry::CustomMessage { .. })
    )
}

/// Whether an entry starts a turn (Pi `findTurnStartIndex`, `compaction.ts:350-365`): a
/// `branch_summary`/`custom_message` entry (user-role messages), or a `message` entry with role
/// `user` or `bashExecution`.
fn is_turn_start_entry(entry: &Entry) -> bool {
    match entry {
        Entry::Known(KnownEntry::BranchSummary { .. } | KnownEntry::CustomMessage { .. }) => true,
        Entry::Known(KnownEntry::Message { message, .. }) => message.is_turn_start(),
        _ => false,
    }
}

/// Whether the cut entry is a core `user` message (Pi `compaction.ts:446`: only role `user` makes a
/// non-split cut — a `bashExecution` is treated as a split-turn start).
fn is_core_user_entry(entry: &Entry) -> bool {
    matches!(
        entry,
        Entry::Known(KnownEntry::Message { message, .. })
            if matches!(message, crate::agent_message::AgentMessage::Core(cyrup_core::Message::User { .. }))
    )
}

/// Valid cut boundaries (indices) in `[start, end)` (R-05-005).
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

/// First turn-start entry at or before `idx` (Pi `findTurnStartIndex`), bounded by `start`.
pub fn find_turn_start(entries: &[Entry], idx: usize, start: usize) -> Option<usize> {
    let mut i = idx.min(entries.len());
    loop {
        if let Some(e) = entries.get(i)
            && is_turn_start_entry(e) {
                return Some(i);
            }
        if i <= start {
            return None;
        }
        i -= 1;
    }
}

/// Walk backward from `end` accumulating each entry's RAW-CONTEXT estimate until
/// `keep_recent_tokens` is reached, snap to the nearest valid cut point at or after that entry, then
/// fold leading context-invisible entries into the kept region. Mirrors Pi `findCutPoint`
/// (`coding-agent/src/core/compaction/compaction.ts:403-461`).
///
/// Both the accumulation and the back-scan key off the SAME "is this entry context-visible?"
/// predicate (`sessionEntryToContextMessages(entry).length > 0`), so `custom_message` and non-empty
/// `branch_summary` entries both consume budget and stop the back-scan (SESS-002). The older harness
/// fork (`agent/src/harness/compaction/compaction.ts:412`) special-cased `entry.type === "message"`
/// at both sites instead; that is the behavior this replaces.
pub fn find_cut_point(
    entries: &[Entry],
    cache: &TokenCache,
    start: usize,
    end: usize,
    keep_recent_tokens: u32,
) -> CutPoint {
    let valid = find_valid_cut_points(entries, start, end);
    if valid.is_empty() {
        return CutPoint { first_kept_index: start, turn_start_index: None, is_split_turn: false };
    }

    // Walk backward, summing the RAW-CONTEXT estimate of every entry — Pi:
    //   const messageTokens = sessionEntryToContextMessages(entry)
    //       .reduce((sum, message) => sum + estimateTokens(message), 0);
    //   if (messageTokens === 0) continue;
    //   accumulatedTokens += messageTokens;
    // (`coding-agent/src/core/compaction/compaction.ts:418-427`) — until we cross the keep-recent
    // budget. Entries the context skips (model/thinking change, label, session_info, plain custom,
    // unknown) estimate 0 and are skipped, but a `custom_message`, a non-empty `branch_summary` or a
    // prior `compaction`'s summary DOES count: those are context-visible and can be arbitrarily
    // large. (`boundary_start` is the PREVIOUS compaction's first-kept index, so that compaction
    // entry itself falls inside `[start, end)` — Pi counts it too.)
    let mut acc: u32 = 0;
    // Default: keep from the first valid message (Pi `cutPoints[0]`).
    let mut cut_idx = valid.first().copied().unwrap_or(start);
    let mut i = end;
    while i > start {
        i -= 1;
        let Some(e) = entries.get(i) else { continue };
        let est = cache.estimate_raw_entry(e);
        if est == 0 {
            continue;
        }
        acc = acc.saturating_add(est);
        if acc >= keep_recent_tokens {
            // Snap to the closest valid cut point at or after this entry.
            if let Some(&v) = valid.iter().find(|&&v| v >= i) {
                cut_idx = v;
            }
            break;
        }
    }

    // Back-scan: fold leading entries that do NOT affect context (model/thinking change, label,
    // session_info, plain custom, unknown) into the kept region, stopping at a compaction boundary
    // or at any CONTEXT-VISIBLE entry — Pi:
    //   if (prevEntry.type === "compaction" || sessionEntryToContextMessages(prevEntry).length > 0)
    //       break;
    // (`coding-agent/src/core/compaction/compaction.ts:439-446`). `custom_message` and non-empty
    // `branch_summary` are context-visible, so they stop the scan rather than being folded in — the
    // same predicate the accumulation loop above uses, which is why both sites had to move together
    // (SESS-002): folding a context-visible entry back in would re-inflate the very tail the budget
    // walk just measured.
    while cut_idx > start {
        match entries.get(cut_idx - 1) {
            Some(Entry::Known(KnownEntry::Compaction { .. })) | None => break,
            Some(prev) => {
                if !crate::context::raw_context_messages(prev).is_empty() {
                    break;
                }
                cut_idx -= 1;
            }
        }
    }

    // Split-turn iff the cut entry is not a core `user` message and a prior turn start exists.
    let is_user = entries.get(cut_idx).is_some_and(is_core_user_entry);
    let (is_split_turn, turn_start_index) = if is_user {
        (false, None)
    } else {
        match find_turn_start(entries, cut_idx, start) {
            Some(ts) if ts < cut_idx => (true, Some(ts)),
            _ => (false, None),
        }
    };

    CutPoint { first_kept_index: cut_idx, turn_start_index, is_split_turn }
}
