//! Branch summarization on `/tree` navigation (arch-05 §6.5, R-05-016/017/018). Pure collection +
//! budgeting; the model call goes through the injected `Summarizer`.

use cyrup_core::{CancelToken, EntryId, Message};

use crate::compaction::error::CompactionError;
use crate::compaction::files::{FileOps, format_file_operations};
use crate::compaction::serialize::serialize_conversation;
use crate::compaction::summarize::{SUMMARIZATION_SYSTEM_PROMPT, SummarizationRequest, Summarizer};
use crate::compaction::tokens::{
    estimate_agent_message, estimate_custom_message_content, estimate_summary_text,
};
use crate::context::{branch_summary_message, compaction_summary_message};
use crate::entry::{Entry, KnownEntry};
use cyrup_core::Usage;
use cyrup_core::{ModelRef, ModelThinkingLevel};
use cyrup_provider::Model;

/// A branch summary plus the usage of the call that produced it — Pi `BranchSummaryResult`
/// (`branch-summarization.ts:34-40`), whose `usage` is optional because the "nothing to summarize"
/// short-circuit returns before any model call.
#[derive(Clone, Debug)]
pub struct BranchSummaryOutput {
    pub text: String,
    pub usage: Option<Usage>,
}

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
            vec![branch_summary_message(
                summary,
                crate::context::parse_entry_ts(&base.timestamp),
            )],
            estimate_summary_text(summary),
            true,
        )),
        Entry::Known(KnownEntry::Compaction {
            summary,
            tokens_before,
            base,
            ..
        }) => Some((
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
pub const BRANCH_SUMMARY_PREAMBLE: &str = "The user explored a different conversation branch before returning here.\nSummary of that \
exploration:\n\n";

/// Branch-summary prompt (R-05-016). Byte-1:1 with Pi `BRANCH_SUMMARY_PROMPT`
/// (`branch-summarization.ts:252-279`). Note: Pi's branch prompt has NO `## Critical Context`
/// section (unlike the compaction prompt).
pub const BRANCH_SUMMARY_PROMPT: &str =
    "Create a structured summary of this conversation branch for \
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

/// Pi's fallback context window for a model whose catalog reports none — `model.contextWindow ||
/// 128000` (`branch-summarization.ts:312` @v0.83.0).
pub const DEFAULT_BRANCH_CONTEXT_WINDOW: u32 = 128_000;

/// The branch-summary token budget: `(model.context_window || 128000) − reserve_tokens` (Pi
/// `branch-summarization.ts:312-313` @v0.83.0; `reserveTokens = 16384` default at `:305`).
///
/// The `|| 128000` fallback is load-bearing and its absence fails INVERTED.
/// [`prepare_branch_entries`] reads `budget == 0` as "no limit" (as Pi reads a non-positive `tokenBudget`), so a model with a
/// zero/unknown context window would otherwise get an UNLIMITED budget and serialize the entire
/// abandoned branch into one summarization prompt — instead of the 111616-token cap Pi applies.
///
/// A `reserve_tokens` larger than the window saturates to `0`, which matches Pi: its subtraction
/// goes negative and `tokenBudget > 0` is likewise false, so both treat that case as "no limit".
pub fn branch_token_budget(model: &Model, reserve_tokens: u32) -> u32 {
    let window = u32::try_from(model.context_window).unwrap_or(u32::MAX);
    let window = if window == 0 {
        DEFAULT_BRANCH_CONTEXT_WINDOW
    } else {
        window
    };
    window.saturating_sub(reserve_tokens)
}

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
    let common_ancestor_id = common_len
        .checked_sub(1)
        .and_then(|i| old_path.get(i))
        .map(Entry::id);
    let entries = old_path.get(common_len..).unwrap_or(&[]).to_vec();
    BranchCollection {
        entries,
        common_ancestor_id,
    }
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
    //
    // Pi (LIVE fork, UNCHANGED v0.83.0 → v0.84.1):
    //   `// Only extract from pi-generated summaries (fromHook !== true), not extension-generated ones`
    //   `if (entry.type === "branch_summary" && !entry.fromHook && entry.details) {`
    //   (`v0.84.1 coding-agent/src/core/compaction/branch-summarization.ts:202-204`).
    //
    // The harness fork dropped this guard at v0.84.1 (`44289550a`,
    // `v0.84.1 agent/src/harness/compaction/branch-summarization.ts:137`) only because that rewrite
    // deleted `fromHook` from `BranchSummaryEntry`
    // (`v0.84.1 agent/src/harness/session/types.ts:53-60`) — see the matching note at
    // `prepare.rs`. cyrup keeps the field (`entry.rs:105`), so it keeps the guard.
    // Pinned by `tests/compaction.rs::g21_prepare_branch_entries_ignores_from_hook_details`.
    for e in entries {
        if let Entry::Known(KnownEntry::BranchSummary {
            details: Some(d),
            from_hook,
            ..
        }) = e
            && !from_hook.unwrap_or(false)
        {
            file_ops.absorb_prev_details(d);
        }
    }

    let mut selected: Vec<Message> = Vec::new();
    let mut total: u64 = 0;
    let budget = u64::from(budget);
    for e in entries.iter().rev() {
        let Some((msgs, est, is_summary)) = branch_contribution(e) else {
            continue;
        };
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

    BranchPreparation {
        messages: selected,
        file_ops,
    }
}

/// Prepend `head` before `selected` (keeps the final order oldest→newest as we walk backward).
fn prepend(selected: &mut Vec<Message>, mut head: Vec<Message>) {
    head.append(selected);
    *selected = head;
}

/// Generate a branch summary (preamble + structured summary + machine file blocks). The
/// summarization completion is capped at a fixed 2048 tokens (Pi `branch-summarization.ts:341`).
///
/// Returns the text together with the call's [`Usage`], which the caller persists on the
/// `branch_summary` entry (Pi `BranchSummaryResult.usage` → `BranchSummaryEntry.usage`,
/// `branch-summarization.ts:372`, `session-manager.ts:88-89`). The short-circuit placeholder path
/// makes no call, hence `usage: None`.
pub async fn generate_branch_summary<S: Summarizer>(
    summarizer: &S,
    prep: &BranchPreparation,
    model: &Model,
    cancel: CancelToken,
) -> Result<BranchSummaryOutput, CompactionError> {
    generate_branch_summary_with_instructions(summarizer, prep, model, None, false, cancel).await
}

/// Pi's `customInstructions` / `replaceInstructions` selector for the branch-summary prompt
/// (`branch-summarization.ts:326-334`), verbatim:
///
/// ```text
/// if (replaceInstructions && customInstructions) instructions = customInstructions;
/// else if (customInstructions) instructions = `${BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: ${customInstructions}`;
/// else instructions = BRANCH_SUMMARY_PROMPT;
/// ```
///
/// `replace_instructions` alone (no `custom_instructions`) falls through to the plain prompt —
/// the `&&` is load-bearing.
pub async fn generate_branch_summary_with_instructions<S: Summarizer>(
    summarizer: &S,
    prep: &BranchPreparation,
    model: &Model,
    custom_instructions: Option<&str>,
    replace_instructions: bool,
    cancel: CancelToken,
) -> Result<BranchSummaryOutput, CompactionError> {
    // Pi short-circuits BEFORE the model call when there is nothing to summarize, returning the
    // placeholder string (`branch-summarization.ts:309-311`). The caller decides whether to append.
    if prep.messages.is_empty() {
        return Ok(BranchSummaryOutput {
            text: BRANCH_SUMMARY_EMPTY_PLACEHOLDER.to_string(),
            usage: None,
        });
    }
    // `.filter(|c| !c.is_empty())` reproduces JS falsiness: Pi's guards are bare truthiness tests
    // on `customInstructions`, so an EMPTY string takes neither branch and the plain
    // `BRANCH_SUMMARY_PROMPT` is used (`branch-summarization.ts:328-333`).
    let instructions: String = match custom_instructions.filter(|c| !c.is_empty()) {
        Some(c) if replace_instructions => c.to_string(),
        Some(c) => format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {c}"),
        None => BRANCH_SUMMARY_PROMPT.to_string(),
    };
    let transcript = serialize_conversation(&prep.messages);
    let prompt = format!("<conversation>\n{transcript}\n</conversation>\n\n{instructions}");
    let req = SummarizationRequest {
        system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
        prompt_text: prompt,
        max_tokens: 2048,
        model: ModelRef {
            provider: model.provider.clone(),
            api: Some(model.api.clone()),
            model: model.id.clone(),
        },
        // Pi builds the branch-summary request options INLINE — `{ apiKey, headers, env, signal,
        // maxTokens: 2048 }` (`branch-summarization.ts:348`) — rather than through
        // `createSummarizationOptions`, so `reasoning` is never set for a branch summary even on a
        // reasoning model with thinking enabled. `Off` is that absence, not an oversight.
        thinking: ModelThinkingLevel::Off,
    };
    let resp = summarizer.complete(req, cancel).await?;
    match resp.stop_reason {
        cyrup_core::StopReason::Error => Err(CompactionError::Summarization(
            resp.error_message.unwrap_or_default(),
        )),
        cyrup_core::StopReason::Aborted => Err(CompactionError::Aborted),
        // An unsettled response is NOT a summary — see the same guard, and the `Deferred`
        // rationale, in `summarize.rs`.
        cyrup_core::StopReason::Pending | cyrup_core::StopReason::Deferred => {
            Err(CompactionError::Summarization(
                resp.error_message
                    .unwrap_or_else(|| crate::compaction::summarize::PENDING_SUMMARY.to_string()),
            ))
        }
        cyrup_core::StopReason::Stop
        | cyrup_core::StopReason::Length
        | cyrup_core::StopReason::ToolUse => {
            let body = resp
                .content
                .iter()
                .filter_map(|c| match c {
                    cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            let (read, modified) = prep.file_ops.compute_lists();
            Ok(BranchSummaryOutput {
                text: format!(
                    "{BRANCH_SUMMARY_PREAMBLE}{body}{}",
                    format_file_operations(&read, &modified)
                ),
                usage: Some(resp.usage),
            })
        }
    }
}
