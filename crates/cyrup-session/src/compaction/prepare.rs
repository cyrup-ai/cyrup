//! Compaction preparation (arch-05 §3.4/§6.2, R-05-007/019). Pure: derives everything a compaction
//! needs from the current branch path. Returns `None` when there is nothing to compact.

use cyrup_core::{EntryId, Message};

use crate::compaction::cutpoint::find_cut_point;
use crate::compaction::files::FileOps;
use crate::compaction::settings::CompactionSettings;
use crate::compaction::tokens::{estimate_context_tokens, TokenCache};
use crate::context::push_as_message;
use crate::entry::{Entry, KnownEntry};

/// The prepared compaction (also the before-compact hook input).
#[derive(Clone, Debug)]
pub struct CompactionPreparation {
    pub first_kept_entry_id: EntryId,
    /// `boundary_start .. history_end`.
    pub messages_to_summarize: Vec<Message>,
    /// `turn_start .. first_kept` (split turn only).
    pub turn_prefix_messages: Vec<Message>,
    pub is_split_turn: bool,
    pub tokens_before: u32,
    pub previous_summary: Option<String>,
    pub file_ops: FileOps,
    pub settings: CompactionSettings,
}

/// Convert a slice of entries to their LLM-message form (drops non-message entries).
pub(crate) fn messages_for(entries: &[Entry]) -> Vec<Message> {
    let mut out = Vec::new();
    for e in entries {
        push_as_message(&mut out, e);
    }
    out
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
            let bs = path
                .iter()
                .position(|e| &e.id() == first_kept_entry_id)
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
        file_ops.absorb_message(m);
    }
    for m in &turn_prefix_messages {
        file_ops.absorb_message(m);
    }

    // tokens_before = estimated size of the history portion being summarized.
    let mut summarized = messages_to_summarize.clone();
    summarized.extend(turn_prefix_messages.iter().cloned());
    let tokens_before = estimate_context_tokens(&summarized).tokens;

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
