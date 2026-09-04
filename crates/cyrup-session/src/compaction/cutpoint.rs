//! The deterministic cut-point algorithm (arch-05 §3.3/§6.2, R-05-005/006/007). Pure, no I/O.
//!
//! The cut MUST NOT fall between a tool call and its tool result: `ToolResult` entries are never
//! valid cut points, so the first-kept boundary always snaps to a user/assistant/bash/custom/summary
//! entry, keeping a tool call and its following results on the same side. Mirrors Pi's **live**
//! fork, `coding-agent/src/core/compaction/compaction.ts:308-462`
//! (`isCutPointMessage`/`isTurnStartMessage`/`isTurnStartEntry`/`findValidCutPoints`/
//! `findTurnStartIndex`/`findCutPoint`).
//!
//! Every one of the four decisions this module makes — is an entry a valid cut point, does it start
//! a turn, does it consume keep-recent budget, does it stop the back-scan — is a predicate over the
//! SAME projection, `sessionEntryToContextMessages(entry)`. Pi's live fork unified them in commit
//! a6f720e6 (2026-07-09); the older harness fork
//! (`agent/src/harness/compaction/compaction.ts`) that cyrup originally ported instead switched on
//! `entry.type` structurally at each site, which made an entry's classification disagree with its
//! own context visibility (an EMPTY `branch_summary` counted as a cut point and a turn start while
//! contributing nothing to the context, and a `custom`-role message was not a turn start). Here the
//! projection is reached through [`crate::context::context_message_role`] (classification, no
//! clone) and [`crate::compaction::tokens::TokenCache::estimate_raw_entry`] (measurement, memoized).

use crate::agent_message::MessageRole;
use crate::compaction::tokens::TokenCache;
use crate::context::context_message_role;
use crate::entry::{Entry, KnownEntry};

/// The chosen boundary between summarized-history and kept-recent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CutPoint {
    /// Index of the first entry to KEEP verbatim.
    pub first_kept_index: usize,
    /// Index of the entry that starts the turn being split, or `None`.
    pub turn_start_index: Option<usize>,
    /// True iff the cut lands mid-turn — the cut entry does not itself start a turn (Pi
    /// `isTurnStartEntry`) and an earlier in-range entry does.
    pub is_split_turn: bool,
}

/// Whether an entry is context-visible at all — Pi
/// `sessionEntryToContextMessages(entry).length > 0` (`compaction.ts:445`). Used by the back-scan.
fn is_context_visible(entry: &Entry) -> bool {
    context_message_role(entry).is_some()
}

/// Whether an entry may serve as a cut boundary — Pi `findValidCutPoints`
/// (`compaction.ts:351-362`): skip `compaction` entries outright, then keep the entry iff its
/// projection contains a non-`toolResult` message (`isCutPointMessage`). A tool result must stay
/// with the call that produced it, and an entry that projects to NOTHING (an empty
/// `branch_summary`, a `model_change`, a `label`, …) is not a boundary the context can express.
fn is_valid_cut_point(entry: &Entry) -> bool {
    if matches!(entry, Entry::Known(KnownEntry::Compaction { .. })) {
        return false;
    }
    context_message_role(entry).is_some_and(MessageRole::is_cut_point)
}

/// Whether an entry starts a turn — Pi `isTurnStartEntry` (`compaction.ts:338-343`): never a
/// `compaction` entry, otherwise iff its projection contains a `user`, `bashExecution`, `custom`,
/// `branchSummary` or `compactionSummary` message (`isTurnStartMessage`).
fn is_turn_start_entry(entry: &Entry) -> bool {
    if matches!(entry, Entry::Known(KnownEntry::Compaction { .. })) {
        return false;
    }
    context_message_role(entry).is_some_and(MessageRole::is_turn_start)
}

/// Valid cut boundaries (indices) in `[start, end)` (R-05-005; Pi `findValidCutPoints`,
/// `coding-agent/src/core/compaction/compaction.ts:351-362`).
pub fn find_valid_cut_points(entries: &[Entry], start: usize, end: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        if let Some(e) = entries.get(i)
            && is_valid_cut_point(e)
        {
            out.push(i);
        }
        i += 1;
    }
    out
}

/// First turn-start entry at or before `idx`, bounded by `start` (Pi `findTurnStartIndex`,
/// `coding-agent/src/core/compaction/compaction.ts:369-376`; `None` is Pi's `-1`).
pub fn find_turn_start(entries: &[Entry], idx: usize, start: usize) -> Option<usize> {
    let mut i = idx.min(entries.len());
    loop {
        if let Some(e) = entries.get(i)
            && is_turn_start_entry(e)
        {
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
        return CutPoint {
            first_kept_index: start,
            turn_start_index: None,
            is_split_turn: false,
        };
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
                if is_context_visible(prev) {
                    break;
                }
                cut_idx -= 1;
            }
        }
    }

    // Split-turn determination — Pi:
    //   const startsTurn = isTurnStartEntry(cutEntry);
    //   const turnStartIndex = startsTurn ? -1 : findTurnStartIndex(entries, cutIndex, startIndex);
    //   isSplitTurn: !startsTurn && turnStartIndex !== -1
    // (`coding-agent/src/core/compaction/compaction.ts:452-461`). The cut splits a turn iff the
    // entry we keep from does NOT itself begin one and some earlier entry in range does.
    let starts_turn = entries.get(cut_idx).is_some_and(is_turn_start_entry);
    let (is_split_turn, turn_start_index) = if starts_turn {
        (false, None)
    } else {
        // `find_turn_start` scans from `cut_idx` downward; since `!starts_turn` it can never return
        // `cut_idx` itself, so the guard below is an invariant check, not a behavioral filter —
        // keep it so a future predicate change cannot silently produce an EMPTY turn prefix.
        match find_turn_start(entries, cut_idx, start) {
            Some(ts) if ts < cut_idx => (true, Some(ts)),
            _ => (false, None),
        }
    };

    CutPoint {
        first_kept_index: cut_idx,
        turn_start_index,
        is_split_turn,
    }
}
