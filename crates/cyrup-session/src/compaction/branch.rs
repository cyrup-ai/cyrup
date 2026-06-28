//! Branch summarization on `/tree` navigation (arch-05 §6.5, R-05-016/017/018). Pure collection +
//! budgeting; the model call goes through the injected `Summarizer`.

use cyrup_core::{CancelToken, EntryId, Message};

use crate::compaction::error::CompactionError;
use crate::compaction::files::{format_file_operations, FileOps};
use crate::compaction::prepare::messages_for;
use crate::compaction::serialize::serialize_conversation;
use crate::compaction::summarize::{
    SummarizationRequest, Summarizer, SUMMARIZATION_SYSTEM_PROMPT,
};
use crate::compaction::tokens::estimate_tokens;
use crate::entry::{Entry, KnownEntry};
use cyrup_core::{ModelRef, ThinkingLevel};
use cyrup_provider::Model;

/// Preamble prepended to a branch summary so it reads as abandoned-branch context.
pub const BRANCH_SUMMARY_PREAMBLE: &str =
    "The following summarizes work done on a branch the user navigated away from:\n\n";

/// Branch-summary prompt (R-05-016, §6 format).
pub const BRANCH_SUMMARY_PROMPT: &str = "Summarize the work done on this conversation branch using \
EXACTLY these sections:\n\n## Goal\n## Constraints & Preferences\n## Progress\n### Done\n### In \
Progress\n### Blocked\n## Key Decisions\n## Next Steps\n## Critical Context";

/// The entries unique to the abandoned branch (old leaf back to, but excluding, the common ancestor)
/// and the common-ancestor id (R-05-016).
pub struct BranchCollection {
    pub entries: Vec<Entry>,
    pub common_ancestor_id: Option<EntryId>,
}

/// Longest-common-prefix of the two root→leaf paths gives the common ancestor; the suffix of the old
/// path is the abandoned-branch work to summarize.
pub fn collect_entries_for_branch_summary(
    old_path: &[Entry],
    target_path: &[Entry],
) -> BranchCollection {
    let mut common_len = 0;
    while let (Some(a), Some(b)) = (old_path.get(common_len), target_path.get(common_len)) {
        if a.id() == b.id() {
            common_len += 1;
        } else {
            break;
        }
    }
    let common_ancestor_id =
        common_len.checked_sub(1).and_then(|i| old_path.get(i)).map(Entry::id);
    let entries = old_path.get(common_len..).unwrap_or(&[]).to_vec();
    BranchCollection { entries, common_ancestor_id }
}

/// Newest-first selection of branch messages within `budget`, plus cumulative file tracking seeded
/// from any nested branch-summary `details` (R-05-015/016).
pub struct BranchPreparation {
    pub messages: Vec<Message>,
    pub file_ops: FileOps,
}

/// Mirrors Pi `prepareBranchEntries`: seed `FileOps` from all branch-summary `details`, then add
/// messages newest→oldest until the token budget is reached.
pub fn prepare_branch_entries(entries: &[Entry], budget: u32) -> BranchPreparation {
    let mut file_ops = FileOps::default();
    for e in entries {
        if let Entry::Known(KnownEntry::BranchSummary { details: Some(d), .. }) = e {
            file_ops.absorb_prev_details(d);
        }
    }

    let mut selected: Vec<Message> = Vec::new();
    let mut total: u32 = 0;
    for e in entries.iter().rev() {
        let mut msgs = messages_for(std::slice::from_ref(e));
        let cost: u32 =
            msgs.iter().map(estimate_tokens).fold(0u32, |a, b| a.saturating_add(b));
        if total.saturating_add(cost) > budget && !selected.is_empty() {
            break;
        }
        total = total.saturating_add(cost);
        // Prepend so the final order is oldest→newest.
        msgs.append(&mut selected);
        selected = msgs;
    }

    for m in &selected {
        file_ops.absorb_message(m);
    }
    BranchPreparation { messages: selected, file_ops }
}

/// Generate a branch summary (preamble + structured summary + machine file blocks).
pub async fn generate_branch_summary<S: Summarizer>(
    summarizer: &S,
    prep: &BranchPreparation,
    model: &Model,
    budget: u32,
    cancel: CancelToken,
) -> Result<String, CompactionError> {
    let transcript = serialize_conversation(&prep.messages);
    let prompt =
        format!("<conversation>\n{transcript}\n</conversation>\n\n{BRANCH_SUMMARY_PROMPT}");
    let req = SummarizationRequest {
        system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
        prompt_text: prompt,
        max_tokens: budget.max(1),
        model: ModelRef {
            provider: model.provider.clone(),
            api: Some(model.api.clone()),
            model: model.id.clone(),
        },
        thinking: ThinkingLevel::Off,
    };
    let resp = summarizer.complete(req, cancel).await?;
    match resp.stop_reason {
        cyrup_core::StopReason::Error => {
            Err(CompactionError::Summarization(resp.error_message.unwrap_or_default()))
        }
        cyrup_core::StopReason::Aborted => Err(CompactionError::Aborted),
        _ => {
            let body = resp
                .content
                .iter()
                .filter_map(|c| match c {
                    cyrup_core::Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let (read, modified) = prep.file_ops.compute_lists();
            Ok(format!(
                "{BRANCH_SUMMARY_PREAMBLE}{body}{}",
                format_file_operations(&read, &modified)
            ))
        }
    }
}
