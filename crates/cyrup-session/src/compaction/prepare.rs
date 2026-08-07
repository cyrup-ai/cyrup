//! Compaction preparation (arch-05 §3.4/§6.2, R-05-007/019). Pure: derives everything a compaction
//! needs from the current branch path. Returns `None` when there is nothing to compact.

use cyrup_core::EntryId;

use crate::agent_message::AgentMessage;
use crate::compaction::cutpoint::find_cut_point;
use crate::compaction::files::FileOps;
use crate::compaction::settings::CompactionSettings;
use crate::compaction::tokens::{estimate_context_tokens_raw, TokenCache};
use crate::context::{build_context_agent_messages, raw_context_messages};
use crate::entry::{Entry, KnownEntry};

/// The prepared compaction (also the before-compact hook input).
///
/// The two message lists carry **raw [`AgentMessage`]s**, roles intact — Pi
/// `CompactionPreparation.messagesToSummarize: AgentMessage[]`
/// (`coding-agent/src/core/compaction/compaction.ts:690-700`). `convertToLlm` is applied later, in
/// [`crate::compaction::summarize::generate_summary`], immediately before `serializeConversation`.
/// Rendering here instead (as the harness fork's port did) would flatten a `bashExecution` into a
/// ``Ran `cmd` `` user message, discard a `custom` message's `customType`, and DROP
/// `excludeFromContext` bash messages outright — invisible to any extension reading this payload,
/// and enough to make a history of only `!!` commands look empty and skip compaction entirely.
#[derive(Clone, Debug)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: EntryId,
    /// `boundary_start .. history_end`.
    pub messages_to_summarize: Vec<AgentMessage>,
    /// `turn_start .. first_kept` (split turn only).
    pub turn_prefix_messages: Vec<AgentMessage>,
    pub is_split_turn: bool,
    pub tokens_before: u32,
    pub previous_summary: Option<String>,
    pub file_ops: FileOps,
    pub settings: CompactionSettings,
}

/// The raw `AgentMessage` an entry contributes to a compaction — Pi
/// `getMessageFromEntryForCompaction` (`compaction.ts:80-85`):
/// `sessionEntryToContextMessages(entry)[0]`, with `compaction` entries excluded (a previous
/// compaction's summary reaches the model through `previousSummary`, not the transcript).
pub(crate) fn message_for_compaction(entry: &Entry) -> Option<AgentMessage> {
    if matches!(entry, Entry::Known(KnownEntry::Compaction { .. })) {
        return None;
    }
    raw_context_messages(entry).into_iter().next()
}

/// Project a slice of entries to their raw `AgentMessage` form (drops entries that contribute none).
pub(crate) fn messages_for(entries: &[Entry]) -> Vec<AgentMessage> {
    entries.iter().filter_map(message_for_compaction).collect()
}

/// Derive a compaction from the current branch `path`. `None` ⇒ nothing to compact (already
/// compacted, short path, or zero messages to summarize). Never panics.
pub fn prepare_compaction(
    path: &[Entry],
    cache: &TokenCache,
    settings: &CompactionSettings,
) -> Option<CompactionPreparation> {
    if path.is_empty() {
        return None;
    }
    // Already compacted: the last entry is a compaction summary.
    if matches!(path.last(), Some(Entry::Known(KnownEntry::Compaction { .. }))) {
        return None;
    }

    // Resume from the previous compaction boundary, if any (cumulative summarization).
    let prev_idx = path
        .iter()
        .rposition(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })));
    let (previous_summary, prev_details, boundary_start) = match prev_idx.and_then(|i| path.get(i)) {
        Some(Entry::Known(KnownEntry::Compaction {
            summary,
            first_kept_entry_id,
            details,
            from_hook,
            ..
        })) => {
            // Pi: `const firstKeptEntryIndex = prevCompaction.firstKeptEntryId ? findIndex(...) :
            // -1; boundaryStart = firstKeptEntryIndex >= 0 ? firstKeptEntryIndex :
            // prevCompactionIndex + 1` (`agent/src/harness/compaction/compaction.ts:661-664`; the
            // live fork's unguarded `findIndex` returns -1 on an absent id, same outcome —
            // `coding-agent/src/core/compaction/compaction.ts:731-732`). So an UNRESOLVABLE
            // `firstKeptEntryId` resumes just after the previous compaction, never at 0.
            let bs = first_kept_entry_id
                .as_ref()
                .and_then(|fk| path.iter().position(|e| &e.id() == fk))
                .unwrap_or_else(|| prev_idx.map(|i| i + 1).unwrap_or(0));
            // Hook-sourced details may use a custom shape; only absorb our default shape.
            let det = if from_hook.unwrap_or(false) { None } else { details.clone() };
            (Some(summary.clone()), det, bs)
        }
        _ => (None, None, 0),
    };

    let boundary_end = path.len();
    let cut = find_cut_point(path, cache, boundary_start, boundary_end, settings.keep_recent_tokens);
    let first_kept_entry_id = path.get(cut.first_kept_index).map(Entry::id)?;

    let history_end = if cut.is_split_turn {
        cut.turn_start_index.unwrap_or(cut.first_kept_index)
    } else {
        cut.first_kept_index
    };

    let messages_to_summarize =
        messages_for(path.get(boundary_start..history_end).unwrap_or(&[]));
    let turn_prefix_messages = if cut.is_split_turn {
        let ts = cut.turn_start_index.unwrap_or(history_end);
        messages_for(path.get(ts..cut.first_kept_index).unwrap_or(&[]))
    } else {
        Vec::new()
    };

    if messages_to_summarize.is_empty() && turn_prefix_messages.is_empty() {
        return None;
    }

    let mut file_ops = FileOps::default();
    if let Some(d) = &prev_details {
        file_ops.absorb_prev_details(d);
    }
    for m in &messages_to_summarize {
        file_ops.absorb_agent_message(m);
    }
    for m in &turn_prefix_messages {
        file_ops.absorb_agent_message(m);
    }

    // tokens_before = estimated size of the ENTIRE pre-compaction context being reduced (Pi
    // `estimateContextTokens(buildSessionContext(pathEntries).messages).tokens`, `compaction.ts:678`),
    // NOT just the summarized slice — this is the number persisted in `CompactionEntry.tokensBefore`.
    // Estimate over the RAW `AgentMessage` context (roles intact) so summary wrappers are not
    // over-counted and `excludeFromContext` bash messages are still counted, matching Pi byte-for-byte.
    let refs: Vec<&Entry> = path.iter().collect();
    let full_context = build_context_agent_messages(&refs);
    let tokens_before = estimate_context_tokens_raw(&full_context).tokens;

    Some(CompactionPreparation {
        first_kept_entry_id,
        messages_to_summarize,
        turn_prefix_messages,
        is_split_turn: cut.is_split_turn,
        tokens_before,
        previous_summary,
        file_ops,
        settings: settings.clone(),
    })
}
