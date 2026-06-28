//! Compaction & branch-summary **generation** (arch-05). The cut-point/serialization/file-tracking
//! logic is a set of pure functions ([`cutpoint`], [`serialize`], [`files`], [`tokens`],
//! [`prepare`]); the model call and hook dispatch are injected seams ([`Summarizer`],
//! [`CompactionHooks`]). [`Compactor`] orchestrates them against a [`SessionManager`].
//!
//! The on-disk record is never reduced (DI-9): compaction/branch-summary only *append* a
//! `CompactionEntry`/`BranchSummaryEntry`; context reduction is the read-time job of
//! [`SessionManager::build_context`].

pub mod branch;
pub mod cutpoint;
pub mod error;
pub mod files;
pub mod hooks;
pub mod prepare;
pub mod serialize;
pub mod settings;
pub mod summarize;
pub mod tokens;

use cyrup_core::{CancelToken, EntryId};
use cyrup_provider::Model;

use crate::entry::{Entry, KnownEntry};
use crate::manager::SessionManager;

pub use branch::{
    collect_entries_for_branch_summary, generate_branch_summary, prepare_branch_entries,
    BranchCollection, BranchPreparation, BRANCH_SUMMARY_PREAMBLE, BRANCH_SUMMARY_PROMPT,
};
pub use cutpoint::{find_cut_point, find_turn_start, find_valid_cut_points, CutPoint};
pub use error::CompactionError;
pub use files::{format_file_operations, CompactionDetails, FileOps};
pub use hooks::{
    BeforeCompactDecision, BeforeCompactEvent, BeforeTreeDecision, BeforeTreeEvent,
    BranchSummaryEntry, CompactionEntry, CompactionHooks, CompactionReason, NoHooks,
    PostCompactEvent, PostTreeEvent,
};
pub use prepare::{prepare_compaction, CompactionPreparation};
pub use serialize::serialize_conversation;
pub use settings::{BranchSummarySettings, CompactionSettings};
pub use summarize::{
    compact_default, generate_summary, generate_turn_prefix_summary, ProviderSummarizer,
    SummarizationRequest, Summarizer, SUMMARIZATION_PROMPT, SUMMARIZATION_SYSTEM_PROMPT,
    TURN_PREFIX_SUMMARIZATION_PROMPT, UPDATE_SUMMARIZATION_PROMPT,
};
pub use tokens::{
    context_tokens_from_usage, estimate_context_tokens, estimate_tokens, ContextUsageEstimate,
    TokenCache,
};

/// Orchestrates compaction + branch summarization against a [`SessionManager`], wiring the pure
/// algorithm to the injected [`Summarizer`] (model call) and [`CompactionHooks`] (extensions).
pub struct Compactor<S: Summarizer, H: CompactionHooks> {
    summarizer: S,
    hooks: H,
    cache: TokenCache,
}

impl<S: Summarizer, H: CompactionHooks> Compactor<S, H> {
    /// Build a compactor over an injected summarizer + hook dispatcher.
    pub fn new(summarizer: S, hooks: H) -> Self {
        Self { summarizer, hooks, cache: TokenCache::default() }
    }

    /// The per-session token-estimate cache (R-05-024).
    pub fn cache(&self) -> &TokenCache {
        &self.cache
    }

    /// The injected hook dispatcher (inspection / notification observation).
    pub fn hooks(&self) -> &H {
        &self.hooks
    }

    /// The injected summarizer seam.
    pub fn summarizer(&self) -> &S {
        &self.summarizer
    }

    /// Cheap trigger check for the agent loop (R-05-001/024): true iff enabled and the estimated
    /// live context exceeds `window − reserve_tokens`.
    pub fn should_compact(&self, path: &[Entry], window: u32, s: &CompactionSettings) -> bool {
        if !s.enabled {
            return false;
        }
        let msgs = prepare::messages_for(path);
        let est = estimate_context_tokens(&msgs);
        est.tokens > window.saturating_sub(s.reserve_tokens)
    }

    /// Run a full compaction: prepare → `before_compact` hook → (default | custom) summarize →
    /// append `CompactionEntry` → `post_compact`. Returns the appended entry, or `None` when there
    /// is nothing to compact or the hook cancelled (R-05-002/008/009/019/020/021).
    #[allow(clippy::too_many_arguments)]
    pub async fn run_compaction(
        &self,
        session: &mut SessionManager,
        model: &Model,
        settings: &CompactionSettings,
        reason: CompactionReason,
        custom_instructions: Option<String>,
        will_retry: bool,
        cancel: CancelToken,
    ) -> Result<Option<CompactionEntry>, CompactionError> {
        let path: Vec<Entry> = session.branch_path(None).into_iter().cloned().collect();
        let prep = match prepare_compaction(&path, &self.cache, settings) {
            Some(p) => p,
            None => return Ok(None),
        };

        let event = BeforeCompactEvent {
            messages_to_summarize: prep.messages_to_summarize.clone(),
            turn_prefix_messages: prep.turn_prefix_messages.clone(),
            previous_summary: prep.previous_summary.clone(),
            file_ops: prep.file_ops.to_details(),
            tokens_before: u64::from(prep.tokens_before),
            first_kept_entry_id: prep.first_kept_entry_id.clone(),
            settings: settings.clone(),
            branch_entries: path.clone(),
            custom_instructions: custom_instructions.clone(),
            reason,
            will_retry,
        };

        // before-compact hook: cancel / supply custom summary / proceed (R-05-019/020).
        let (summary, first_kept, tokens_before, details, from_hook) =
            match self.hooks.before_compact(&event, cancel.child_token()).await? {
                BeforeCompactDecision::Cancel => return Ok(None),
                BeforeCompactDecision::Custom {
                    summary,
                    first_kept_entry_id,
                    tokens_before,
                    details,
                } => (
                    summary,
                    first_kept_entry_id,
                    tokens_before,
                    details.unwrap_or_else(|| serde_json::json!({})),
                    true,
                ),
                BeforeCompactDecision::Proceed => {
                    let summary = compact_default(
                        &self.summarizer,
                        &prep,
                        model,
                        custom_instructions.as_deref(),
                        cancel.clone(),
                    )
                    .await?;
                    let details = serde_json::to_value(prep.file_ops.to_details())
                        .unwrap_or_else(|_| serde_json::json!({}));
                    (
                        summary,
                        prep.first_kept_entry_id.clone(),
                        u64::from(prep.tokens_before),
                        details,
                        false,
                    )
                }
            };

        let id = session.append_compaction(
            summary,
            first_kept,
            tokens_before,
            Some(details),
            from_hook,
        )?;
        let entry = compaction_entry_of(session, &id).ok_or(CompactionError::MissingEntryId)?;

        self.hooks
            .post_compact(&PostCompactEvent {
                entry: entry.clone(),
                from_extension: from_hook,
                reason,
                will_retry,
            })
            .await;
        Ok(Some(entry))
    }

    /// Branch summarization on `/tree` navigation (R-05-016/017/018/022). Fires `before_tree`,
    /// optionally generates + appends a `BranchSummaryEntry` at the navigation point, then navigates
    /// the leaf to `target_id` (the abandoned branch is never deleted) and fires `post_tree`.
    /// Returns the appended branch-summary entry, if any. `Ok(None)` means navigation was cancelled
    /// or no summary was produced.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_branch_summary(
        &self,
        session: &mut SessionManager,
        model: &Model,
        target_id: EntryId,
        old_leaf_id: Option<EntryId>,
        user_wants_summary: bool,
        settings: &BranchSummarySettings,
        cancel: CancelToken,
    ) -> Result<Option<BranchSummaryEntry>, CompactionError> {
        let old_leaf = old_leaf_id.or_else(|| session.leaf_id().cloned());
        let old_path: Vec<Entry> =
            session.branch_path(old_leaf.as_ref()).into_iter().cloned().collect();
        let target_path: Vec<Entry> =
            session.branch_path(Some(&target_id)).into_iter().cloned().collect();
        let collection = collect_entries_for_branch_summary(&old_path, &target_path);

        let event = BeforeTreeEvent {
            target_id: target_id.clone(),
            old_leaf_id: old_leaf.clone(),
            common_ancestor_id: collection.common_ancestor_id.clone(),
            entries_to_summarize: collection.entries.clone(),
            user_wants_summary,
        };

        // before-tree hook: cancel navigation / supply custom summary / proceed (R-05-022).
        let decision = self.hooks.before_tree(&event, cancel.child_token()).await?;
        let (summary_and_details, from_hook): (Option<(String, serde_json::Value)>, bool) =
            match decision {
                BeforeTreeDecision::Cancel => return Ok(None),
                BeforeTreeDecision::CustomSummary { summary, details } if user_wants_summary => {
                    (Some((summary, details.unwrap_or_else(|| serde_json::json!({})))), true)
                }
                // Proceed, or a custom summary the user did not ask for → default path.
                BeforeTreeDecision::Proceed | BeforeTreeDecision::CustomSummary { .. } => {
                    if !user_wants_summary && settings.skip_prompt {
                        (None, false) // R-05-018: skip generation
                    } else {
                        let prep = prepare_branch_entries(&collection.entries, settings.reserve_tokens);
                        if prep.messages.is_empty() {
                            (None, false)
                        } else {
                            let summary = generate_branch_summary(
                                &self.summarizer,
                                &prep,
                                model,
                                settings.reserve_tokens,
                                cancel.clone(),
                            )
                            .await?;
                            let details = serde_json::to_value(prep.file_ops.to_details())
                                .unwrap_or_else(|_| serde_json::json!({}));
                            (Some((summary, details)), false)
                        }
                    }
                }
            };

        // Navigate the leaf to the target (R-05-017: abandoned branch untouched).
        session.branch(&target_id)?;

        let entry = match summary_and_details {
            Some((summary, details)) => {
                let from_id = old_leaf.clone().unwrap_or_else(|| EntryId::from("root"));
                let id =
                    session.append_branch_summary(from_id, summary, Some(details), from_hook)?;
                branch_summary_entry_of(session, &id)
            }
            None => None,
        };

        self.hooks
            .post_tree(&PostTreeEvent {
                entry: entry.clone(),
                target_id: target_id.clone(),
                from_extension: from_hook,
            })
            .await;
        Ok(entry)
    }
}

/// Build the hook-event `CompactionEntry` payload from a freshly appended session entry.
fn compaction_entry_of(session: &SessionManager, id: &EntryId) -> Option<CompactionEntry> {
    match session.entry(id) {
        Some(Entry::Known(KnownEntry::Compaction {
            base,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            from_hook,
        })) => Some(CompactionEntry {
            id: base.id.clone(),
            parent_id: base.parent_id.clone(),
            summary: summary.clone(),
            first_kept_entry_id: first_kept_entry_id.clone(),
            tokens_before: *tokens_before,
            from_hook: from_hook.unwrap_or(false),
            details: details.clone(),
        }),
        _ => None,
    }
}

/// Build the hook-event `BranchSummaryEntry` payload from a freshly appended session entry.
fn branch_summary_entry_of(session: &SessionManager, id: &EntryId) -> Option<BranchSummaryEntry> {
    match session.entry(id) {
        Some(Entry::Known(KnownEntry::BranchSummary {
            base,
            from_id,
            summary,
            details,
            from_hook,
        })) => Some(BranchSummaryEntry {
            id: base.id.clone(),
            parent_id: base.parent_id.clone(),
            summary: summary.clone(),
            from_id: from_id.clone(),
            from_hook: from_hook.unwrap_or(false),
            details: details.clone(),
        }),
        _ => None,
    }
}
