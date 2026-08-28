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
        cyrup_provider::cache_stats::compute_cache_waste(&scan, &models)
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
        let last = scan.iter().rposition(|e| matches!(e, CacheScanEntry::Assistant(_)))?;
        cyrup_provider::cache_stats::collect_cache_misses(&scan, &models).get(&last).copied()
    }

    /// The `contextUsage` sub-object of [`Self::session_stats`], in Pi's `ContextUsage` shape
    /// (`{tokens, contextWindow, percent}`, extensions/types.ts:288-294). `None` when no model /
    /// no known context window — Pi's `getContextUsage` returns `undefined` there
    /// (agent-session.ts:3165-3170).
    ///
    /// Public because it is a 1:1 port of `AgentSession.getContextUsage()`, which upstream's footer
    /// calls directly on every render (`footer.ts:108`) to build its `{pct}%/{window}` segment. The
    /// TUI needs exactly this three-state answer — including the `percent: null` case — which the
    /// coarser [`Self::context_usage`] (always a number) cannot express.
    pub async fn stats_context_usage(&self) -> Option<crate::state::StatsContextUsage> {
        let usage = self.context_usage().await;
        if usage.context_window == 0 {
            return None;
        }
        // Pi's post-compaction guard (agent-session.ts:3175-3197). After a compaction the last
        // assistant `usage` still describes the PRE-compaction context, so reporting it would show a
        // stale — and much larger — occupancy as if it were current. Pi only trusts a usage from an
        // assistant that responded AFTER the latest compaction on this branch, and where that
        // assistant neither aborted nor errored and actually consumed context. With no such
        // assistant the count is genuinely unknown, and Pi returns `{tokens: null, percent: null}`
        // while still reporting the window.
        //
        // Without this branch `tokens`/`percent` were unconditionally `Some`, so the `null` case the
        // struct's own doc comment describes was unreachable.
        if !self.has_post_compaction_usage().await {
            return Some(crate::state::StatsContextUsage {
                tokens: None,
                context_window: usage.context_window,
                percent: None,
            });
        }
        Some(crate::state::StatsContextUsage {
            tokens: Some(usage.used_tokens),
            context_window: usage.context_window,
            percent: Some(usage.fraction * 100.0),
        })
    }

    /// `true` when this branch's occupied-token count can be trusted — i.e. there is no compaction
    /// on the branch, or an assistant has responded since the latest one (Pi
    /// `getContextUsage`'s `hasPostCompactionUsage` scan, agent-session.ts:3181-3193).
    ///
    /// Scans backwards from the branch tail to the compaction boundary, matching Pi's loop
    /// direction, and accepts the first assistant that neither aborted nor errored and whose usage
    /// accounts for a non-zero context.
    async fn has_post_compaction_usage(&self) -> bool {
        use cyrup_core::StopReason;
        use cyrup_session::entry::{Entry, KnownEntry};
        use cyrup_session::AgentMessage;

        let guard = self.manager.lock().await;
        // Pi scans `sessionManager.getBranch()` (agent-session.ts:3174, indexed at :3181-3193) —
        // the ACTIVE BRANCH — not `getEntries()`. `entries()` is the flat append-only store
        // (`manager.rs:818`): after a `/fork` or a `/tree` navigation it also holds the abandoned
        // branch, so `rposition` could latch an OFF-BRANCH compaction as the boundary and `skip`
        // could then count an off-branch assistant as post-compaction usage — printing a stale
        // pre-compaction occupancy as current, the exact failure this guard exists to prevent.
        // `branch_path(None)` is cyrup's `getBranch()`, and is O(branch-depth) rather than O(all
        // entries) besides (TUI-092 F4 C2).
        let path = guard.branch_path(None);
        let Some(compaction_idx) = path
            .iter()
            .rposition(|e| matches!(e, Entry::Known(KnownEntry::Compaction { .. })))
        else {
            // No compaction on this branch: the last assistant usage is current by construction.
            return true;
        };
        path.iter()
            .copied()
            .skip(compaction_idx + 1)
            .rev()
            .filter_map(|e| match e {
                Entry::Known(KnownEntry::Message {
                    message: AgentMessage::Core(Message::Assistant(a)),
                    ..
                }) => Some(a),
                _ => None,
            })
            .any(|a| {
                // Same four-field sum `ContextUsage::from_last_assistant` uses, so "consumed
                // context" means the same thing in both places (Pi `calculateContextTokens`).
                let context_tokens =
                    a.usage.input + a.usage.cache_read + a.usage.cache_write + a.usage.output;
                !matches!(a.stop_reason, StopReason::Aborted | StopReason::Error)
                    && context_tokens > 0
            })
    }

    /// Context-window occupancy from the last assistant turn (Pi `getContextUsage`,
    /// agent-session.ts:3164-3208 @v0.83.0).
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
        use cyrup_session::entry::{Entry, KnownEntry};
        use cyrup_session::AgentMessage;

        // Pi `getContextUsage`: `const model = this.model; if (!model) return undefined;`
        // (agent-session.ts:3165-3166) and `if (contextWindow <= 0) return undefined;` (:3168-3169).
        // Taken FIRST, exactly as Pi orders it — the model read precedes `getBranch()` at :3174 — so
        // the `compaction_model` leaf lock is released before the async `manager` guard is acquired
        // and no lock-nesting question arises at all. cyrup's return type is non-optional, so the
        // modelless case degrades to a zero window, which `from_last_assistant` already renders as
        // fraction 0.0 — the same "unknown occupancy" the TUI shows for an undefined usage.
        let window = { Self::lock(&self.compaction_model).as_ref().map_or(0, |m| m.context_window) };

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
        // the scan; same shape as the neighbour `has_post_compaction_usage`.
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
        // either, it returns `contextUsage: this.getContextUsage()` (agent-session.ts:3170).
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
