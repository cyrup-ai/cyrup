//! Branch summarization on `/tree` navigation (arch-05 §6.5, R-05-016/017/018). Pure collection +
//! budgeting; the model call goes through the injected `Summarizer`.

use cyrup_core::{CancelToken, EntryId, Message};

use crate::compaction::error::CompactionError;
use crate::compaction::files::{format_file_operations, FileOps};
use crate::compaction::serialize::serialize_conversation;
use crate::compaction::summarize::{
    SummarizationRequest, Summarizer, SUMMARIZATION_SYSTEM_PROMPT,
};
use crate::compaction::tokens::{
    estimate_agent_message, estimate_custom_message_content, estimate_summary_text,
};
use crate::context::{branch_summary_message, compaction_summary_message};
use crate::entry::{Entry, KnownEntry};
use cyrup_core::{ModelRef, ModelThinkingLevel};
use cyrup_provider::Model;

/// The LLM messages an entry contributes to a branch summary, its Pi `estimateTokens` cost, and
/// whether it is a summary entry (eligible for the over-budget force-include). `None` skips the
/// entry. Mirrors Pi branch `getMessageFromEntry` (`branch-summarization.ts:150-174`): drops
/// `toolResult`, INCLUDES `compaction` summaries.
fn branch_contribution(entry: &Entry) -> Option<(Vec<Message>, u32, bool)> {
    match entry {
        Entry::Known(KnownEntry::Message { message, .. }) => {
            if message.is_tool_result() {
                return None;
            }
            let mut msgs = Vec::new();
            message.push_llm(&mut msgs);
            Some((msgs, estimate_agent_message(message), false))
        }
        Entry::Known(KnownEntry::CustomMessage { content, base, .. }) => {
            let ts = crate::context::parse_entry_ts(&base.timestamp);
            Some((
                vec![crate::agent_message::custom_to_message(content, ts)],
                estimate_custom_message_content(content),
                false,
            ))
        }
        Entry::Known(KnownEntry::BranchSummary { summary, base, .. }) => Some((
            vec![branch_summary_message(summary, crate::context::parse_entry_ts(&base.timestamp))],
            estimate_summary_text(summary),
            true,
        )),
        Entry::Known(KnownEntry::Compaction { summary, tokens_before, base, .. }) => Some((
            vec![compaction_summary_message(
                summary,
                *tokens_before,
                crate::context::parse_entry_ts(&base.timestamp),
            )],
            estimate_summary_text(summary),
            true,
        )),
        _ => None,
    }
}

/// Placeholder summary Pi returns when an abandoned branch yields no summarizable messages (every
/// entry filtered out / over budget). Byte-1:1 with Pi `generateBranchSummary`'s
/// `{ summary: "No content to summarize" }` early return (`branch-summarization.ts:309-311`). The
/// agent-session caller still appends it (Pi's `if (summaryText)` is truthy), so an explored-but-empty
/// branch is recorded rather than silently dropped.
pub const BRANCH_SUMMARY_EMPTY_PLACEHOLDER: &str = "No content to summarize";

/// Preamble prepended to a branch summary so it reads as abandoned-branch context. Byte-1:1 with Pi
/// `BRANCH_SUMMARY_PREAMBLE` (`branch-summarization.ts:247-250`).
pub const BRANCH_SUMMARY_PREAMBLE: &str =
    "The user explored a different conversation branch before returning here.\nSummary of that \
exploration:\n\n";

/// Branch-summary prompt (R-05-016). Byte-1:1 with Pi `BRANCH_SUMMARY_PROMPT`
/// (`branch-summarization.ts:252-279`). Note: Pi's branch prompt has NO `## Critical Context`
/// section (unlike the compaction prompt).
pub const BRANCH_SUMMARY_PROMPT: &str = "Create a structured summary of this conversation branch for \
context when returning later.

Use this EXACT format:

## Goal
[What was the user trying to accomplish in this branch?]

## Constraints & Preferences
- [Any constraints, preferences, or requirements mentioned]
- [Or \"(none)\" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks/changes]

### In Progress
- [ ] [Work that was started but not finished]

### Blocked
- [Issues preventing progress, if any]

## Key Decisions
- **[Decision]**: [Brief rationale]

## Next Steps
1. [What should happen next to continue this work]

Keep each section concise. Preserve exact file paths, function names, and error messages.";

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

/// Mirrors Pi `prepareBranchEntries` (`branch-summarization.ts:189-241`): seed `FileOps` from
/// pi-generated branch-summary `details`, then walk newest→oldest adding messages until the token
/// budget is reached. A `compaction`/`branch_summary` message that would overflow is force-included
/// when we are still under `budget * 0.9` (important context). `budget == 0` means no limit.
pub fn prepare_branch_entries(entries: &[Entry], budget: u32) -> BranchPreparation {
    let mut file_ops = FileOps::default();
    // First pass: cumulative file tracking from pi-generated (`!fromHook`) branch summaries only.
    for e in entries {
        if let Entry::Known(KnownEntry::BranchSummary { details: Some(d), from_hook, .. }) = e
            && !from_hook.unwrap_or(false) {
                file_ops.absorb_prev_details(d);
            }
    }

    let mut selected: Vec<Message> = Vec::new();
    let mut total: u64 = 0;
    let budget = u64::from(budget);
    for e in entries.iter().rev() {
        let Some((msgs, est, is_summary)) = branch_contribution(e) else { continue };
        // File ops are extracted for every contributing entry (Pi extracts BEFORE the budget check).
        for m in &msgs {
            file_ops.absorb_message(m);
        }
        let cost = u64::from(est);
        if budget > 0 && total + cost > budget {
            // Over budget: force-include a summary entry while under 90% of budget.
            if is_summary && total * 10 < budget * 9 {
                prepend(&mut selected, msgs);
            }
            break;
        }
        prepend(&mut selected, msgs);
        total += cost;
    }

    BranchPreparation { messages: selected, file_ops }
}

/// Prepend `head` before `selected` (keeps the final order oldest→newest as we walk backward).
fn prepend(selected: &mut Vec<Message>, mut head: Vec<Message>) {
    head.append(selected);
    *selected = head;
}

/// Generate a branch summary (preamble + structured summary + machine file blocks). The
/// summarization completion is capped at a fixed 2048 tokens (Pi `branch-summarization.ts:341`).
pub async fn generate_branch_summary<S: Summarizer>(
    summarizer: &S,
    prep: &BranchPreparation,
    model: &Model,
    cancel: CancelToken,
) -> Result<String, CompactionError> {
    // Pi short-circuits BEFORE the model call when there is nothing to summarize, returning the
    // placeholder string (`branch-summarization.ts:309-311`). The caller decides whether to append.
    if prep.messages.is_empty() {
        return Ok(BRANCH_SUMMARY_EMPTY_PLACEHOLDER.to_string());
    }
    let transcript = serialize_conversation(&prep.messages);
    let prompt =
        format!("<conversation>\n{transcript}\n</conversation>\n\n{BRANCH_SUMMARY_PROMPT}");
    let req = SummarizationRequest {
        system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
        prompt_text: prompt,
        max_tokens: 2048,
        model: ModelRef {
            provider: model.provider.clone(),
            api: Some(model.api.clone()),
            model: model.id.clone(),
        },
        thinking: ModelThinkingLevel::Off,
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
