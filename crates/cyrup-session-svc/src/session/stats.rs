//! Usage, cost and context-window statistics.
//!
//! Pi `agent-session.ts` `getSessionStats`/`getContextUsage`. The aggregated token + cost rollups
//! a front-end's `/stats` and footer render, and the post-compaction-aware context-usage estimate
//! the auto-compaction threshold is measured against.

use cyrup_core::Message;

use super::AgentSession;

impl AgentSession {
    /// Aggregate session stats (Pi `getSessionStats`, agent-session.ts:3112; RPC
    /// `get_session_stats`).
    ///
    /// SEAM-031: computed from `sessionManager.getEntries()` — ALL entries, including history a
    /// compaction replaced — not from the rebuilt LLM context, so token/cost totals reflect what was
    /// actually billed across the session (Pi's own docstring, agent-session.ts:3107-3111).
    pub async fn session_stats(&self) -> crate::state::SessionStats {
        let context_usage = self.stats_context_usage().await;
        let mgr = self.manager.lock().await;
        crate::state::SessionStats::from_entries(
            mgr.entries(),
            self.session_id.to_string(),
            mgr.session_file().map(|p| p.display().to_string()),
            context_usage,
        )
    }

    /// Per-model cost/token breakdown for `/session` (Pi `getUsageCostBreakdown(entries)`, called
    /// from `handleSessionCommand` at `interactive-mode.ts:5665` @v0.83.0). PROV-036.
    ///
    /// Reads the SAME `mgr.entries()` [`Self::session_stats`] reads — every entry, including
    /// history a compaction replaced — so the rows sum to `SessionStats::cost` exactly.
    pub async fn usage_cost_breakdown(&self) -> Vec<crate::state::UsageCostBreakdownEntry> {
        let mgr = self.manager.lock().await;
        crate::state::usage_cost_breakdown(mgr.entries())
    }

    /// Session-wide prompt-cache waste for `/session` (Pi `computeCacheWaste(entries,
    /// this.session.modelRuntime)`, `interactive-mode.ts:5660` @v0.83.0). PROV-035.
    ///
    /// The price source is the session's full model registry, which is what pi's `modelRuntime`
    /// argument resolves `getModel(provider, id)?.cost.cacheRead` against. A model the registry
    /// does not know prices at `0`, exactly as pi's `?? undefined` fallback does — so an unknown
    /// model still contributes its MISSED TOKENS, just no dollar figure.
    pub async fn cache_waste(&self) -> cyrup_provider::cache_stats::CacheWasteTotals {
        let models = self.full_model_registry();
        let mgr = self.manager.lock().await;
        let scan = crate::state::cache_scan_entries(mgr.entries());
        cyrup_provider::cache_stats::compute_cache_waste(&scan, &*models)
    }

    /// The prompt-cache miss charged to the MOST RECENT assistant turn, if it was above the
    /// detector's noise floor — the input to pi's per-turn transcript notice
    /// (`maybeShowCacheMissNotice`, `modes/interactive/interactive-mode.ts:3820-3826` @v0.83.0).
    ///
    /// **This is deliberately not [`cyrup_provider::cache_stats::detect_cache_miss`], and the
    /// difference is an ordering one.** Upstream calls `detectCacheMiss(getEntries(), message, …)`
    /// from its `message_end` handler and notes *"Entries don't contain `message` yet: message_end
    /// fires before persistence"* — the just-finished turn is compared against the one before it.
    /// cyrup inverts that ordering: [`crate::subscriber`] appends the finalized message to the
    /// session tree BEFORE it fans the event out, so by the time any subscriber sees
    /// `MessageEnd` the turn is already an entry. Passing those entries to `detect_cache_miss`
    /// would compare the turn against ITSELF — `missed_tokens` collapses to `input + cache_write`
    /// with `idle_ms == 0` (`cache_stats.rs:176-179`), i.e. a large false positive on every
    /// big-prompt turn.
    ///
    /// Scanning the persisted entries and reading the LAST assistant one's miss is the faithful
    /// equivalent: `scan` reaches that entry with `prev` set to the preceding request, which is
    /// exactly the state `detect_cache_miss` synthesises upstream.
    ///
    /// `None` when there is no assistant turn yet, when the turn is the first after a reset, or
    /// when the miss was at or below the noise floor — the same three silences upstream has.
    pub async fn last_cache_miss(&self) -> Option<cyrup_provider::cache_stats::CacheMiss> {
        use cyrup_provider::cache_stats::CacheScanEntry;
        let models = self.full_model_registry();
        let mgr = self.manager.lock().await;
        let scan = crate::state::cache_scan_entries(mgr.entries());
        let last = scan
            .iter()
            .rposition(|e| matches!(e, CacheScanEntry::Assistant(_)))?;
        cyrup_provider::cache_stats::collect_cache_misses(&scan, &*models)
            .get(&last)
            .copied()
    }

    /// `(previous, current)` counts of `thinking_dropped` input transformations on the current
    /// branch's last two assistant messages — the input to pi's `maybeShowThinkingDropNotice`
    /// (`interactive-mode.ts:4001-4025` @v0.87.1). TUI-117.
    ///
    /// The ordering trap is the one [`Self::last_cache_miss`] documents: pi's `message_end` fires
    /// BEFORE persistence, so its "last assistant entry" is the previous response; cyrup persists
    /// BEFORE fan-out, so the finishing message is already the last entry and the previous response
    /// is the one before it. See [`crate::state::thinking_drop_counts`]. Walks the branch
    /// (`getBranch()`), not every entry, so a `/tree` jump compares along the live path.
    pub async fn thinking_drop_counts(&self) -> (usize, usize) {
        let mgr = self.manager.lock().await;
        crate::state::thinking_drop_counts(&mgr.branch_path(None))
    }

    /// The `contextUsage` sub-object of [`Self::session_stats`], in Pi's `ContextUsage` shape
    /// (`{tokens, contextWindow, percent}`, extensions/types.ts:288-294). `None` when no model /
    /// no known context window — Pi's `getContextUsage` returns `undefined` there
    /// (`agent-session.ts:3859-3863` @v0.87.1).
    ///
    /// Public because it is a 1:1 port of `AgentSession.getContextUsage()`
    /// (`agent-session.ts:3858-3901` @v0.87.1), which upstream's footer calls directly on every
    /// render (`footer.ts:108`) to build its `{pct}%/{window}` segment. The TUI needs exactly this
    /// three-state answer — including the `percent: null` case — which the coarser
    /// [`Self::context_usage`] (always a number) cannot express.
    ///
    /// The number is `estimateProjectedContextTokens(projection, branch)` (`:3893`), NOT the last
    /// assistant's usage: the anchor is the last assistant that neither aborted nor errored and
    /// whose `calculateContextTokens(usage)` is non-zero (`compaction.ts:170-183`), and every
    /// projected message after it is added as a chars/4 estimate. So an aborted turn — whose partial
    /// usage is typically zero — falls through to the previous good reading plus the new prompt,
    /// and the meter holds instead of dropping to `0.0%` (TUI-056).
    pub async fn stats_context_usage(&self) -> Option<crate::state::StatsContextUsage> {
        use cyrup_session::entry::{Entry, KnownEntry};

        // `const model = this.model; if (!model) return undefined;` and
        // `if (contextWindow <= 0) return undefined;` (`:3859-3863`), taken before the branch lock
        // exactly as [`Self::context_usage`] takes them.
        let context_window = {
            Self::lock(&self.compaction_model)
                .as_ref()
                .map_or(0, |m| m.context_window)
        };
        if context_window == 0 {
            return None;
        }

        let guard = self.manager.lock().await;
        let branch = guard.branch_path(None);
        // Pi's post-compaction guard (`:3865-3891`). After a compaction the last assistant `usage`
        // still describes the PRE-compaction context, so Pi only trusts one from an assistant that
        // responded AFTER the latest compaction on this branch — and only if that assistant
        // survives the projection (a `context_edit` can omit it), neither aborted nor errored, and
        // actually consumed context. With none the count is genuinely unknown, and Pi returns
        // `{tokens: null, percent: null}` while still reporting the window.
        if let Some(compaction_index) = branch
            .iter()
            .rposition(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
        {
            let projection = cyrup_session::context::build_context_agent_messages_tagged(&branch);
            let has_post_compaction_usage = branch.iter().skip(compaction_index + 1).any(|entry| {
                let id = entry.id();
                projection
                    .iter()
                    .any(|(source, m)| *source == id && has_valid_usage(m))
            });
            if !has_post_compaction_usage {
                return Some(crate::state::StatsContextUsage {
                    tokens: None,
                    context_window,
                    percent: None,
                });
            }
        }

        let estimate = cyrup_session::compaction::estimate_projected_context_tokens(&branch);
        let tokens = u64::from(estimate.tokens);
        Some(crate::state::StatsContextUsage {
            tokens: Some(tokens),
            context_window,
            percent: Some(tokens as f64 / context_window as f64 * 100.0),
        })
    }

    /// Context-window occupancy from the last assistant turn (Pi `getContextUsage`,
    /// agent-session.ts:3164-3208 @v0.83.0; byte-identical at `:3375-3413` @v0.84.4).
    ///
    /// Answers from a **reverse walk of the active branch's entries** — Pi's own
    /// `sessionManager.getBranch()` shape (`:3174`) — never from a rebuilt message list. The
    /// previous body called [`Self::messages`], i.e. `build_context()` →
    /// `build_context_messages()`, which deep-clones every message on the branch (tool payloads
    /// included) purely so this function could reverse the vector, take the first assistant, and
    /// drop the rest: O(session history) of allocation on **every** `MessageEnd`, awaited on the
    /// TUI run-loop task (TUI-092 F4).
    pub async fn context_usage(&self) -> crate::state::ContextUsage {
        use cyrup_core::StopReason;
        use cyrup_session::AgentMessage;
        use cyrup_session::entry::{Entry, KnownEntry};

        // Pi `getContextUsage`: `const model = this.model; if (!model) return undefined;`
        // (agent-session.ts:3165-3166) and `if (contextWindow <= 0) return undefined;` (:3168-3169).
        // Taken FIRST, exactly as Pi orders it — the model read precedes `getBranch()` at :3174 — so
        // the `compaction_model` leaf lock is released before the async `manager` guard is acquired
        // and no lock-nesting question arises at all. cyrup's return type is non-optional, so the
        // modelless case degrades to a zero window, which `from_last_assistant` already renders as
        // fraction 0.0 — the same "unknown occupancy" the TUI shows for an undefined usage.
        let window = {
            Self::lock(&self.compaction_model)
                .as_ref()
                .map_or(0, |m| m.context_window)
        };

        let guard = self.manager.lock().await;
        // The last assistant ON THE ACTIVE BRANCH, by parent-link walk — the same answer
        // `messages().await.iter().rev().find_map(..)` gave, without building or cloning the
        // branch's whole message list to get it.
        //
        // `StopReason::Deferred` is skipped because the OLD path could not return one:
        // `push_as_message`'s first arm drops a deferred assistant from the built context
        // (`cyrup-session/src/context.rs:62`, `is_deferred_assistant` at `:114-120`). A deferred
        // turn is a durable provider handle with empty content, not a settled context measurement
        // (`cyrup-core/src/message.rs:172-188`), so its `usage` must not drive the footer. cyrup
        // cannot produce one yet, but a Pi-written session carrying one must still read identically
        // (R-00-013). `filter_map(..).find(..)` — not `find_map` — so a deferred tail does not stop
        // the scan.
        let last = guard
            .branch_path(None)
            .into_iter()
            .rev()
            .filter_map(|e| match e {
                Entry::Known(KnownEntry::Message {
                    message: AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) => Some(a),
                _ => None,
            })
            .find(|a| a.stop_reason != StopReason::Deferred);
        crate::state::ContextUsage::from_last_assistant(last, window)
    }

    /// A serializable snapshot of the session for RPC `get_state`.
    ///
    /// cyrup-original in shape: Pi's `RpcSessionState` (`modes/rpc/rpc-types.ts:95-108`, built at
    /// `modes/rpc/rpc-mode.ts:446-461`) carries twelve scalars and NO occupancy or stats, and Pi's
    /// `state` getter is `return this.agent.state` (agent-session.ts:863-865). The extra `stats` /
    /// `context_usage` fields are cyrup's.
    pub async fn state_view(&self) -> crate::state::SessionStateView {
        let stats = self.session_stats().await;
        let messages = self.messages().await;
        // ONE producer for occupancy, as upstream has: Pi's `getSessionStats` does not re-derive it
        // either, it returns `contextUsage: this.getContextUsage()` (agent-session.ts:3160
        // @v0.83.0, `:3371` @v0.84.4). Pinned by `tests/context_usage_branch.rs` (SEAM-115).
        // Deriving it inline here duplicated the pre-F4 windowed-build scan, so it disagreed with
        // `GetContextUsage` whenever a compaction's kept window held no assistant while an earlier
        // pre-compaction assistant existed — including every unresolvable-v1 `first_kept_entry_id`
        // session, whose kept window is empty by construction (`cyrup-session/src/context.rs:166-172`).
        let context_usage = self.context_usage().await;
        let model = Self::lock(&self.model).clone();
        crate::state::SessionStateView {
            session_id: self.session_id.to_string(),
            cwd: self.services.cwd.display().to_string(),
            provider: model.as_ref().map(|m| m.provider.to_string()),
            model: model.as_ref().map(|m| m.model.to_string()),
            session_name: self.session_name().await,
            is_streaming: self.is_streaming().await,
            message_count: messages.len(),
            pending_message_count: self.pending_message_count(),
            stats,
            context_usage,
        }
    }
}

/// The per-message predicate of Pi's `projectedAssistants` set (`agent-session.ts:3873-3885`
/// @v0.87.1) — the same three clauses `getAssistantUsage` applies (`compaction.ts:170-183`): an
/// assistant that neither aborted nor errored, whose `calculateContextTokens(usage)` —
/// `totalTokens` first, else the four-field sum — is non-zero.
fn has_valid_usage(message: &cyrup_session::AgentMessage) -> bool {
    use cyrup_core::StopReason;
    matches!(
        message,
        cyrup_session::AgentMessage::Core(Message::Assistant(a))
            if !matches!(a.stop_reason, StopReason::Aborted | StopReason::Error)
                && cyrup_session::compaction::context_tokens_from_usage(&a.usage) > 0
    )
}
