//! Compaction & branch-summary **generation** (arch-05). The cut-point/serialization/file-tracking
//! logic is a set of pure functions ([`cutpoint`], [`serialize`], [`files`], [`tokens`],
//! [`prepare`]); the model call is an injected seam ([`Summarizer`]). [`Compactor`] orchestrates
//! them against a [`SessionManager`].
//!
//! Extension dispatch (`session_before_compact` / `session_tree`) is NOT run from here: the
//! session-service producer fires it against a real [`CompactionPreparation`] via `cyrup-ext` and
//! feeds the guest's decision back in as a [`CompactionOverride`].
//!
//! The on-disk record is never reduced (DI-9): compaction/branch-summary only *append* a
//! `CompactionEntry`/`BranchSummaryEntry`; context reduction is the read-time job of
//! [`SessionManager::build_context`].

pub mod branch;
pub mod cutpoint;
pub mod error;
pub mod events;
pub mod files;
pub mod prepare;
pub mod serialize;
pub mod settings;
pub mod summarize;
pub mod tokens;

use cyrup_core::{CancelToken, EntryId, ModelThinkingLevel, Usage};
use cyrup_provider::Model;

use crate::entry::{Entry, KnownEntry};
use crate::manager::SessionManager;

pub use branch::{
    branch_token_budget, collect_entries_for_branch_summary, generate_branch_summary,
    generate_branch_summary_with_instructions, prepare_branch_entries, BranchCollection,
    BranchPreparation, BranchSummaryOutput, BRANCH_SUMMARY_EMPTY_PLACEHOLDER,
    BRANCH_SUMMARY_PREAMBLE, BRANCH_SUMMARY_PROMPT, DEFAULT_BRANCH_CONTEXT_WINDOW,
};
pub use cutpoint::{find_cut_point, find_turn_start, find_valid_cut_points, CutPoint};
pub use error::CompactionError;
pub use events::{BranchSummaryEntry, CompactionEntry, CompactionOverride, CompactionReason};
pub use files::{format_file_operations, CompactionDetails, FileOps};
pub use prepare::{prepare_compaction, CompactionPreparation};
pub use serialize::serialize_conversation;
pub use settings::{BranchSummarySettings, CompactionSettings};
pub use summarize::{
    combine_usage, compact_default, complete_summarization, generate_summary,
    generate_turn_prefix_summary, summarization_reasoning, DefaultCompaction, ProviderSummarizer,
    SummarizationRequest, SummaryOutput, Summarizer, PENDING_SUMMARY, SUMMARIZATION_PROMPT,
    SUMMARIZATION_SYSTEM_PROMPT, TURN_PREFIX_SUMMARIZATION_PROMPT, UPDATE_SUMMARIZATION_PROMPT,
};
pub use tokens::{
    context_tokens_from_usage, estimate_context_tokens, estimate_context_tokens_raw,
    estimate_tokens, ContextUsageEstimate, TokenCache,
};

/// Orchestrates compaction + branch summarization against a [`SessionManager`], wiring the pure
/// algorithm to the injected [`Summarizer`] (model call).
pub struct Compactor<S: Summarizer> {
    summarizer: S,
    cache: TokenCache,
    thinking: ModelThinkingLevel,
}

impl<S: Summarizer> Compactor<S> {
    /// Build a compactor over an injected summarizer.
    ///
    /// The thinking level defaults to `Off`; production callers bind the live session level with
    /// [`Self::with_thinking`].
    pub fn new(summarizer: S) -> Self {
        Self { summarizer, cache: TokenCache::default(), thinking: ModelThinkingLevel::Off }
    }

    /// Bind the session's thinking level for the summarization calls this compactor makes.
    ///
    /// Pi passes `this.thinkingLevel` at every `compact(...)` call site
    /// (`agent-session.ts:1855,2129`); cyrup builds one `Compactor` per compaction operation from
    /// the same live session state that supplies the model, so the level is bound here alongside
    /// the summarizer. It reaches the request only through
    /// [`summarize::summarization_reasoning`], which reproduces Pi's
    /// `model.reasoning && level !== "off"` gate. Branch summaries deliberately ignore it — see
    /// [`branch::generate_branch_summary`].
    #[must_use]
    pub fn with_thinking(mut self, thinking: ModelThinkingLevel) -> Self {
        self.thinking = thinking;
        self
    }

    /// The bound session thinking level.
    pub fn thinking(&self) -> ModelThinkingLevel {
        self.thinking
    }

    /// The per-session token-estimate cache (R-05-024).
    pub fn cache(&self) -> &TokenCache {
        &self.cache
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
        // Estimate over the RAW `AgentMessage` context — Pi passes
        // `estimateContextTokens(buildSessionContext(pathEntries).messages)`
        // (`compaction.ts:192-228,678`; `session-manager.ts:389-403`), whose `messages` keep the
        // `bashExecution`/`branchSummary`/`compactionSummary`/`custom` roles intact. Estimating over
        // the `convertToLlm`-rendered context instead would over-count summary wrappers and DROP
        // `excludeFromContext` bash messages that Pi's raw context still counts.
        let refs: Vec<&Entry> = path.iter().collect();
        let msgs = crate::context::build_context_agent_messages(&refs);
        let est = estimate_context_tokens_raw(&msgs);
        est.tokens > window.saturating_sub(s.reserve_tokens)
    }

    /// Prepare a compaction over the current branch WITHOUT running it (R-05-007; Pi
    /// `prepareCompaction`, compaction.ts:652). Exposes the computed [`CompactionPreparation`] + the
    /// branch path so a caller (the session service) can fire the external `session_before_compact`
    /// extension hook against the REAL preparation, then feed the SAME prep back to
    /// [`Self::run_compaction_prepared`] — no double-preparation. `None` ⇒ nothing to compact.
    pub fn prepare(
        &self,
        session: &SessionManager,
        settings: &CompactionSettings,
    ) -> Option<(CompactionPreparation, Vec<Entry>)> {
        let path: Vec<Entry> = session.branch_path(None).into_iter().cloned().collect();
        let prep = prepare_compaction(&path, &self.cache, settings)?;
        Some((prep, path))
    }

    /// Run a full compaction: prepare → summarize → append `CompactionEntry`. Returns the appended
    /// entry, or `None` when there is nothing to compact (R-05-002/008/009).
    pub async fn run_compaction(
        &self,
        session: &mut SessionManager,
        model: &Model,
        settings: &CompactionSettings,
        custom_instructions: Option<String>,
        cancel: CancelToken,
    ) -> Result<Option<CompactionEntry>, CompactionError> {
        let (prep, _path) = match self.prepare(session, settings) {
            Some(x) => x,
            None => return Ok(None),
        };
        self.finish_compaction(session, model, custom_instructions, &prep, None, cancel).await
    }

    /// Run a compaction from an ALREADY-computed preparation (L4 gap #5). The session-service producer
    /// calls [`Self::prepare`], fires the external `session_before_compact` extension hook against the
    /// real prep, and then calls this — passing the guest's compaction override, if any (Pi
    /// `SessionBeforeCompactResult.compaction`). The preparation is NOT recomputed (no double-prep);
    /// `external_override` (when `Some`) replaces the default model summarization and the appended
    /// entry is marked `fromExtension`.
    pub async fn run_compaction_prepared(
        &self,
        session: &mut SessionManager,
        model: &Model,
        custom_instructions: Option<String>,
        prep: &CompactionPreparation,
        external_override: Option<CompactionOverride>,
        cancel: CancelToken,
    ) -> Result<Option<CompactionEntry>, CompactionError> {
        self.finish_compaction(session, model, custom_instructions, prep, external_override, cancel)
            .await
    }

    /// Shared compaction tail: resolve the summary (external override > default model
    /// summarization), then append the `CompactionEntry`.
    async fn finish_compaction(
        &self,
        session: &mut SessionManager,
        model: &Model,
        custom_instructions: Option<String>,
        prep: &CompactionPreparation,
        external_override: Option<CompactionOverride>,
        cancel: CancelToken,
    ) -> Result<Option<CompactionEntry>, CompactionError> {
        let (summary, first_kept, tokens_before, details, usage, from_hook) = match external_override
        {
            // An external extension override (Pi `SessionBeforeCompactResult.compaction`): its
            // summary/details land in the entry, which is marked `fromExtension`.
            Some(ov) => (
                ov.summary,
                ov.first_kept_entry_id.unwrap_or_else(|| prep.first_kept_entry_id.clone()),
                ov.tokens_before.unwrap_or(u64::from(prep.tokens_before)),
                ov.details.unwrap_or_else(|| serde_json::json!({})),
                ov.usage,
                true,
            ),
            None => {
                let produced = compact_default(
                    &self.summarizer,
                    prep,
                    model,
                    custom_instructions.as_deref(),
                    self.thinking,
                    cancel.clone(),
                )
                .await?;
                let details = serde_json::to_value(prep.file_ops.to_details())
                    .unwrap_or_else(|_| serde_json::json!({}));
                (
                    produced.summary,
                    prep.first_kept_entry_id.clone(),
                    u64::from(prep.tokens_before),
                    details,
                    produced.usage,
                    false,
                )
            }
        };

        // Pi re-tests the abort signal IMMEDIATELY before the append, unconditionally, covering both
        // summary sources (extension override, default summarizer) — the manual path throws
        // `new Error("Compaction cancelled")` (`agent-session.ts:1868-1870`), the auto path emits
        // `compaction_end { result: undefined, aborted: true }` and returns `false` (`:2142-2151`).
        // Without it, a cancel landing while a `session_before_compact` guest is producing a
        // summary — or in the window after the summarization stream settles but before the write —
        // still mutates the session file and reports success.
        if cancel.is_cancelled() {
            return Err(CompactionError::Aborted);
        }

        let id = session.append_compaction(
            summary,
            first_kept,
            tokens_before,
            Some(details),
            usage,
            from_hook,
        )?;
        let entry = compaction_entry_of(session, &id).ok_or(CompactionError::MissingEntryId)?;
        Ok(Some(entry))
    }

    /// Branch summarization on `/tree` navigation (R-05-016/017/018). Optionally generates +
    /// appends a `BranchSummaryEntry` at the navigation point, then navigates the leaf to
    /// `target_id` (the abandoned branch is never deleted). Returns the appended branch-summary
    /// entry, if any. `Ok(None)` means no summary was produced.
    ///
    /// The `/tree` extension seam (`session_before_tree`/`session_tree`, with its
    /// instruction/label overrides) lives in the session service, which drives
    /// [`collect_entries_for_branch_summary`] + [`generate_branch_summary_with_instructions`]
    /// directly rather than through this convenience wrapper.
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

        // Pi's gate is the USER's choice alone: `if (options.summarize &&
        // entriesToSummarize.length > 0 && !extensionSummary)` (`agent-session.ts:2983`).
        // `skipPrompt` is a front-end-only setting upstream — it never appears in
        // `agent-session.ts`; its sole consumer repo-wide is `interactive-mode.ts:4672`, which uses
        // it to decide whether to ASK, not whether to summarize. Consulting it here made an
        // embedder's `summarize: false` still pay for a summarization call.
        let summary_and_details: Option<(String, serde_json::Value, Option<Usage>)> =
            if !user_wants_summary || collection.entries.is_empty() {
                // Pi also gates the default summarizer on `entriesToSummarize.length > 0`
                // (`agent-session.ts:2983`): with NO abandoned entries, produce nothing.
                None
            } else {
                // Budget = (context window || 128000) − reserve (Pi
                // `branch-summarization.ts:312-313`), NOT a flat reserve_tokens — this keeps far
                // more branch history than a bare reserve would.
                let budget = branch_token_budget(model, settings.reserve_tokens);
                let prep = prepare_branch_entries(&collection.entries, budget);
                // `generate_branch_summary` returns the "No content to summarize" placeholder when
                // the abandoned branch filtered to no messages (all `toolResult` / over budget).
                // Pi's caller still appends it — `if (summaryText)` is truthy on the non-empty
                // placeholder (`agent-session.ts:3038`) — so we append it too rather than silently
                // dropping an explored branch.
                let produced =
                    generate_branch_summary(&self.summarizer, &prep, model, cancel).await?;
                let details = serde_json::to_value(prep.file_ops.to_details())
                    .unwrap_or_else(|_| serde_json::json!({}));
                Some((produced.text, details, produced.usage))
            };

        let entry = match summary_and_details {
            Some((summary, details, usage)) => {
                // Pi `branchWithSummary(newLeafId, …)` (`agent-session.ts:3040-3046`) with the
                // comment "Summary is attached at the navigation target position (newLeafId), not
                // the old branch" (`:3036`): ONE value — the navigation target — becomes `leafId`,
                // `parentId` AND `fromId` (`session-manager.ts:1391-1397`). Recording the ABANDONED
                // leaf as `fromId` gave the SDK path a different provenance graph than the live
                // `/tree` path, which already routes through `branch_with_summary`.
                let id = session.branch_with_summary(
                    Some(&target_id),
                    summary,
                    Some(details),
                    usage,
                    false,
                )?;
                branch_summary_entry_of(session, &id)
            }
            None => {
                // Navigate the leaf to the target (R-05-017: abandoned branch untouched).
                session.branch(&target_id)?;
                None
            }
        };
        Ok(entry)
    }
}

/// Build the [`CompactionEntry`] payload from a freshly appended session entry.
fn compaction_entry_of(session: &SessionManager, id: &EntryId) -> Option<CompactionEntry> {
    match session.entry(id) {
        Some(Entry::Known(KnownEntry::Compaction {
            base,
            summary,
            first_kept_entry_id,
            tokens_before,
            details,
            usage,
            from_hook,
        })) => Some(CompactionEntry {
            id: base.id.clone(),
            parent_id: base.parent_id.clone(),
            summary: summary.clone(),
            // `None` here means the on-disk entry carried an unresolvable v1 `firstKeptEntryIndex`
            // (see `entry.rs`). These payloads are only ever built from an entry cyrup JUST
            // appended, which always carries the id, so this fails closed rather than inventing one
            // — the caller maps `None` to `CompactionError::MissingEntryId`.
            first_kept_entry_id: first_kept_entry_id.clone()?,
            tokens_before: *tokens_before,
            from_hook: from_hook.unwrap_or(false),
            details: details.clone(),
            usage: usage.clone(),
        }),
        _ => None,
    }
}

/// Build the [`BranchSummaryEntry`] payload from a freshly appended session entry.
fn branch_summary_entry_of(session: &SessionManager, id: &EntryId) -> Option<BranchSummaryEntry> {
    match session.entry(id) {
        Some(Entry::Known(KnownEntry::BranchSummary {
            base,
            from_id,
            summary,
            details,
            usage,
            from_hook,
        })) => Some(BranchSummaryEntry {
            id: base.id.clone(),
            parent_id: base.parent_id.clone(),
            summary: summary.clone(),
            from_id: from_id.clone(),
            from_hook: from_hook.unwrap_or(false),
            details: details.clone(),
            usage: usage.clone(),
        }),
        _ => None,
    }
}
