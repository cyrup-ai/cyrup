//! The automatic compaction trigger.
//!
//! Pi `agent-session.ts:831,1811-1905,2078-2086`. The pre-send + post-run check that keeps a long
//! session inside its context window, the enable toggle, and the shared `is_compacting` view.
//! Manual `/compact` lives in [`super::compaction`].

use cyrup_agent::AgentMessage;
use cyrup_core::{AssistantMessage, Message};
use cyrup_ext::HostEvent;
use cyrup_provider::is_context_overflow;
use cyrup_session::compaction::{
    CompactionReason, CompactionSettings, Compactor, NoHooks, context_tokens_from_usage,
};

use crate::compact::DynSummarizer;
use crate::error::SessionServiceError;
use crate::event::{AgentSessionEvent, SummarizationRetrySource, raw_message_to_agent};

use super::AgentSession;
use super::compaction::{BeforeCompactOutcome, CompactionCancelGuard, compaction_reason_str};

impl AgentSession {
    /// Whether any compaction (manual / auto / branch-summary) is running (Pi `isCompacting`,
    /// agent-session.ts:831).
    pub fn is_compacting(&self) -> bool {
        Self::lock(&self.compaction_cancel).is_some()
            || Self::lock(&self.auto_compaction_cancel).is_some()
            || Self::lock(&self.branch_summary_cancel).is_some()
    }

    /// Whether auto-compaction is enabled (runtime override, else the settings default; Pi
    /// `autoCompactionEnabled`, agent-session.ts:2086).
    pub fn auto_compaction_enabled(&self) -> bool {
        Self::lock(&self.auto_compaction_override).unwrap_or(self.auto_compaction_enabled_default)
    }

    /// Toggle auto-compaction (Pi `setAutoCompactionEnabled`, agent-session.ts:2078).
    pub fn set_auto_compaction_enabled(&self, enabled: bool) {
        *Self::lock(&self.auto_compaction_override) = Some(enabled);
    }

    /// Check whether the given assistant turn requires compaction and run it (Pi `_checkCompaction`,
    /// agent-session.ts:1808-1898). Returns `true` when a compaction ran. `skip_aborted` skips a
    /// user-cancelled turn (post-run); the pre-send check passes `false` to catch aborted responses.
    pub async fn check_compaction(
        &self,
        assistant: &AssistantMessage,
        skip_aborted: bool,
    ) -> Result<bool, SessionServiceError> {
        if !self.auto_compaction_enabled() {
            return Ok(false);
        }
        if skip_aborted && assistant.stop_reason == cyrup_core::StopReason::Aborted {
            return Ok(false);
        }
        // Pi `_checkCompaction` reads `const contextWindow = this.model?.contextWindow ?? 0;`
        // (agent-session.ts:1960) — a modelless session has window 0, which `shouldCompact` and
        // `isContextOverflow` both treat as "unknown", so nothing triggers.
        let model = { Self::lock(&self.compaction_model).clone() };
        let window = model.as_ref().map_or(0, |m| m.context_window);
        let same_model = {
            let cur = Self::lock(&self.model);
            cur.as_ref().is_some_and(|c| {
                assistant.provider == c.provider && assistant.model.as_str() == c.model.as_str()
            })
        };

        // Stale-compaction-boundary guard (Pi agent-session.ts:1859-1864): skip all checks if this
        // assistant turn predates the latest compaction boundary, so a stale pre-compaction
        // usage/error does not retrigger compaction on the first prompt after a compaction.
        let compaction_ts = self.latest_compaction_ts().await;
        if let Some(boundary_ts) = compaction_ts
            && assistant.timestamp <= boundary_ts
        {
            return Ok(false);
        }

        // Case 1: overflow — a context-overflow error/usage on the SAME model compacts (no retry
        // for a completed answer; the overflow-recovery flag guards an infinite loop).
        if same_model && is_context_overflow(assistant, Some(window)) {
            let will_retry = assistant.stop_reason != cyrup_core::StopReason::Stop;
            if !will_retry {
                return self
                    .run_auto_compaction(CompactionReason::Overflow, false)
                    .await;
            }
            if *Self::lock(&self.overflow_recovery_attempted) {
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason: CompactionReason::Overflow,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(
                        "Context overflow recovery failed after one compact-and-retry attempt. \
                         Try reducing context or switching to a larger-context model."
                            .to_string(),
                    ),
                })
                .await;
                return Ok(false);
            }
            *Self::lock(&self.overflow_recovery_attempted) = true;
            self.drop_trailing_assistant().await;
            return self
                .run_auto_compaction(CompactionReason::Overflow, will_retry)
                .await;
        }

        // Case 2: threshold — the context is getting large (Pi agent-session.ts:1900-1927). Prefer the
        // assistant turn's OWN reported usage; only for an error / all-zero-usage message fall back to
        // estimating from the live context, with a post-compaction-usage verification so a kept
        // pre-compaction usage (stale, reflecting the old larger context) cannot falsely trigger.
        let settings = self.effective_compaction_settings();
        let direct_context_tokens = context_tokens_from_usage(&assistant.usage);
        let context_tokens: u32 = if assistant.stop_reason == cyrup_core::StopReason::Error
            || direct_context_tokens == 0
        {
            // SESS-028 — the estimate basis is the RAW `AgentMessage` transcript, not the
            // `convertToLlm`-flattened projection. pi reads `this.agent.state.messages`
            // (`agent-session.ts:2020-2021` @v0.83.0), an `AgentMessage[]` that keeps the
            // `bashExecution`/`branchSummary`/`compactionSummary`/`custom` roles intact.
            //
            // `self.messages()` is the flattened view: it renders summary wrappers into text
            // (over-counting them) and DROPS `excludeFromContext` bash messages that pi's raw
            // context still counts. Both errors are in the estimate that decides whether to
            // compact, so this fallback fired at a different context size than upstream — and
            // `Compactor::should_compact` next door already estimated over the raw basis
            // correctly, so the two disagreed inside one crate.
            // `raw_context_messages()` is `build_context_raw()` — `buildSessionContext(pathEntries)
            // .messages` (`session-manager.ts:389-403`), the same basis `Compactor::should_compact`
            // uses. It stands in for pi's `this.agent.state.messages`, and since SESS-043 that is
            // an equality rather than an approximation on every path that re-seeds the transcript:
            // the three re-seed sites now assign this exact list. It stays the basis (rather than
            // reading the agent back) because a live cyrup session ALSO appends its own
            // `!`-execution results straight onto the transcript as `AgentMessage::Custom`
            // (`record_bash_result`), which the session file — and therefore this projection —
            // holds as the authoritative copy.
            let messages = self.raw_context_messages().await;
            let estimate = cyrup_session::compaction::estimate_context_tokens_raw(&messages);
            let Some(last_usage_index) = estimate.last_usage_index else {
                return Ok(false); // No usage data at all.
            };
            // If the usage source predates the compaction boundary, its tokens are stale.
            if let (
                Some(boundary_ts),
                Some(cyrup_session::agent_message::AgentMessage::Core(Message::Assistant(
                    usage_msg,
                ))),
            ) = (compaction_ts, messages.get(last_usage_index))
                && usage_msg.timestamp <= boundary_ts
            {
                return Ok(false);
            }
            estimate.tokens
        } else {
            direct_context_tokens
        };
        // Pi `shouldCompact`: contextTokens > contextWindow − reserveTokens (compaction.ts).
        let threshold = window.saturating_sub(u64::from(settings.reserve_tokens));
        if u64::from(context_tokens) > threshold {
            return self
                .run_auto_compaction(CompactionReason::Threshold, false)
                .await;
        }
        Ok(false)
    }

    /// The unix-ms timestamp of the latest `compaction` entry on the current branch, or `None`
    /// (Pi `getLatestCompactionEntry(this.sessionManager.getBranch())`, agent-session.ts:1859).
    async fn latest_compaction_ts(&self) -> Option<i64> {
        let guard = self.manager.lock().await;
        guard
            .branch_path(None)
            .into_iter()
            .rev()
            .find_map(|e| match e {
                cyrup_session::Entry::Known(cyrup_session::KnownEntry::Compaction {
                    base, ..
                }) => Some(cyrup_session::context::parse_entry_ts(&base.timestamp)),
                _ => None,
            })
    }

    /// Run an auto-compaction with its own abort controller + events (Pi `_runAutoCompaction`,
    /// agent-session.ts:1905-2076). Mirrors [`Self::compact`]'s dance but tagged with the auto
    /// `reason` and tracked under `auto_compaction_cancel` so `is_compacting`/`abort_compaction`
    /// observe it.
    async fn run_auto_compaction(
        &self,
        reason: CompactionReason,
        will_retry: bool,
    ) -> Result<bool, SessionServiceError> {
        // Pi's FIRST statement inside the try is `if (!this.model) { return false; }`
        // (agent-session.ts:2052-2054) — before `_emit({type:"compaction_start"})` (`:2072`) and
        // before `started = true`, so a modelless session emits NEITHER `compaction_start` nor
        // `compaction_end`; it just declines. The check therefore sits ahead of the emit here too.
        // (Unreachable in practice: `check_compaction`'s window is 0 with no model, so neither the
        // overflow nor the threshold arm fires — but pi guards it and so do we.)
        let Some(model) = ({ Self::lock(&self.compaction_model).clone() }) else {
            return Ok(false);
        };
        let cancel = self.session_cancel.child_token();
        // Same guard as the manual path — an auto compaction runs on the spawned `drive_run` task,
        // but `check_compaction` is also awaited from `prepare` on the CALLER's future
        // (`:1199`), which a racing caller can drop.
        let mut cancel_slot =
            CompactionCancelGuard::install(&self.auto_compaction_cancel, cancel.clone());
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason })
            .await;

        // Pi: `this._summarizationRetryCallbacks({ source: "compaction", reason })` — the LIVE
        // threshold/overflow reason, not a literal (agent-session.ts:2133).
        let (retry_observer, retry_rx) =
            crate::compact::summarization_retry_channel(SummarizationRetrySource::Compaction {
                reason,
            });
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer = DynSummarizer::new(
            self.provider.current(),
            model.clone(),
            self.summarization_retry(),
        )
        .with_observer(retry_observer);
        // Pi threads the session thinking level into every compaction summarization call
        // (`agent-session.ts:1855,2129`); `summarization_reasoning` applies the `model.reasoning`
        // gate before it reaches the request.
        let compactor =
            Compactor::new(summarizer, NoHooks).with_thinking(self.thinking_level().await);
        let settings = self.effective_compaction_settings();

        // Compute the REAL preparation BEFORE the extension hook (L4 gap #5) — the ONLY preparation.
        let (prep, branch_entries) = {
            let guard = self.manager.lock().await;
            match compactor.prepare(&guard, &settings) {
                Some(x) => x,
                None => {
                    drop(guard);
                    cancel_slot.clear();
                    self.fanout_emit(AgentSessionEvent::CompactionEnd {
                        reason,
                        result: None,
                        aborted: false,
                        will_retry: false,
                        error_message: None,
                    })
                    .await;
                    return Ok(false);
                }
            }
        };

        // session_before_compact ext hook: veto OR compaction override, against the real preparation
        // (agent-session.ts:1980-1990).
        let external_override = match self
            .emit_before_compact(&prep, &branch_entries, None, reason, will_retry, &cancel)
            .await
        {
            BeforeCompactOutcome::Cancel => {
                cancel_slot.clear();
                // Pi agent-session.ts:1984-1990: a cancelling handler emits aborted:true, willRetry:false.
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                return Ok(false);
            }
            BeforeCompactOutcome::Proceed(ov) => ov,
        };

        let mut guard = self.manager.lock().await;
        let result = compactor
            .run_compaction_prepared(
                &mut guard,
                &model,
                &settings,
                reason,
                None,
                will_retry,
                &prep,
                branch_entries,
                external_override,
                cancel,
            )
            .await;
        // Pi agent-session.ts:2045: estimate the rebuilt context for the result payload. Hoisted
        // out of the `Ok(Some(_))` arm (as `compact` already does) so the manager guard is released
        // on ONE path, before the retry queue is flushed.
        // SESS-043 — the RAW projection, which is both the estimate basis below AND the transcript
        // pi re-seeds from (`this.agent.state.messages = sessionContext.messages`). It used to be
        // `build_context()`, the `convertToLlm`-flattened twin.
        let compacted_raw = guard.build_context_raw();
        // Pi measures `estimatedTokensAfter` over the RAW `AgentMessage` context, NOT the
        // `convertToLlm`-flattened one: `estimateMessagesTokens(sessionContext.messages)`
        // (agent-session.ts:1876 manual / :2157 auto) sums `estimateTokens` (compaction.ts:266-300)
        // over `buildSessionContext().messages`, and that list is
        // `buildContextEntries().flatMap(sessionEntryToContextMessages)` with every role intact
        // (session-manager.ts:461-469 composed with :383-408). So a retained `compactionSummary`
        // costs `summary.length/4` with NO wrapper prose, and an `excludeFromContext` (`!!`) bash
        // execution still costs `(command.length + output.length)/4`.
        //
        // Measuring `compacted_ctx` instead billed the ~107-char COMPACTION_SUMMARY wrapper that
        // `push_as_message` adds (cyrup-session/src/context.rs:16-18) — ~27 tokens, and a compacted
        // context ALWAYS leads with one — while silently dropping every `excludeFromContext` bash
        // entry, which `AgentMessage::push_llm` removes at the LLM boundary. It also disagreed with
        // `tokens_before` on the SAME `compaction_end` event, which `prepare_compaction` already
        // computes over the raw projection (cyrup-session/src/compaction/prepare.rs).
        let estimated_tokens_after: u64 = u64::from(
            compacted_raw
                .iter()
                .map(cyrup_session::compaction::tokens::estimate_agent_message)
                .fold(0u32, u32::saturating_add),
        );
        drop(guard);
        // Close the retry queue (the compactor owns the emitter) and flush it — with the manager
        // guard already released — so every `summarization_retry_*` lands BEFORE `compaction_end`.
        drop(compactor);
        let _ = retry_pump.await;
        match result {
            Ok(Some(entry)) => {
                cancel_slot.clear();
                // SEAM-112 — the re-seed is on the SUCCESS PATH ONLY. pi runs
                // `appendCompaction(...)` → `this.agent.state.messages = sessionContext.messages;`
                // at `agent-session.ts:2275-2280`, AFTER the `signal.aborted` early-return at
                // `:2260-2275`, so a cancelled / declined / failed auto-compaction leaves
                // `agent.state.messages` exactly as the run found it.
                //
                // cyrup ran it unconditionally, before `match result`, which mattered most on the
                // path that reaches here: `check_compaction` (`session/auto_compaction.rs:78`) calls
                // `drop_trailing_assistant` (`session/retry.rs:140`) to strip the overflow response
                // from the agent transcript BEFORE compacting, but that response was already
                // persisted on `message_end`, so an aborted or erroring compaction re-seeded it
                // straight back out of the session file — the exact state `Agent::continue_run`
                // rejects with `ContinueFromAssistant`.
                //
                // pi `agent-session.ts:2280`: re-seed the AGENT's in-memory transcript from the
                // compacted context. `appendCompaction` only writes the JSONL entry — this
                // assignment is what actually shrinks the next request.
                //
                // Without it auto-compaction reported success while the very next turn still
                // shipped the ENTIRE pre-compaction history to the provider: zero token reduction,
                // full cost. Overflow recovery was worse than useless — `check_compaction` set
                // `overflow_recovery_attempted`, `continue_run()` resent the unchanged context, it
                // overflowed again, and the one-shot latch reported "Try reducing context or
                // switching to a larger-context model", blaming the model for a compaction that had
                // never taken effect.
                //
                // SESS-043 — seeded from the RAW projection. Folding `build_context().messages`
                // through `core_message_to_agent` produced a transcript of a different LENGTH and
                // different roles from pi's: `convertToLlm` DROPS every `excludeFromContext` (`!!`)
                // bash message and rewrites each summary into wrapper prose, so pi's
                // `messages.slice(0, -1)` arithmetic (`agent-session.ts:2008`, `:2188`, `:2703`)
                // and every agent-state token estimate ran over a different list.
                let compacted_messages: Vec<AgentMessage> =
                    compacted_raw.iter().map(raw_message_to_agent).collect();
                self.agent.set_messages(compacted_messages).await;
                let cr = crate::state::CompactionResult {
                    summary: entry.summary.clone(),
                    first_kept_entry_id: entry.first_kept_entry_id.to_string(),
                    tokens_before: entry.tokens_before,
                    estimated_tokens_after: Some(estimated_tokens_after),
                    // SEAM-034: pi's `usage?` (compaction.ts:93) — the token spend of the
                    // summarization call(s), already recorded on the compaction entry.
                    usage: entry.usage.clone(),
                    details: entry.details.clone(),
                };
                let notify_cancel = self.session_cancel.child_token();
                self.services
                    .ext_host
                    .dispatcher()
                    .dispatch_notify(
                        &HostEvent::SessionCompact {
                            compaction_entry: serde_json::to_value(&entry)
                                .unwrap_or(serde_json::Value::Null),
                            from_extension: entry.from_hook,
                            reason: compaction_reason_str(reason).to_string(),
                            will_retry,
                        },
                        &notify_cancel,
                    )
                    .await;
                // Pi agent-session.ts:2069: result present, aborted:false, carries the run's willRetry.
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: Some(cr),
                    aborted: false,
                    will_retry,
                    error_message: None,
                })
                .await;
                // SEAM-112 — pi `agent-session.ts:2307-2317`, positioned exactly here (after the
                // `compaction_end` emit, before the `return true`), with upstream's own reasoning:
                // "The overflow response was persisted on message_end before _checkCompaction()
                // removed it from agent state. Rebuilding state from the new compaction can restore
                // that kept entry, leaving an assistant as the final message. agent.continue()
                // rejects that state, so remove the retriable error or truncated-length response
                // again before continuing the interrupted turn."
                //
                // Concretely on cyrup's side: `check_compaction` (`session/auto_compaction.rs:78`)
                // calls `drop_trailing_assistant` (`session/retry.rs:140`), then this run's re-seed
                // above pulls the SAME response back out of the session file (it was written on
                // `message_end`). Without this re-drop `handle_post_agent_run`
                // (`session/run.rs:235-237`) returns `true`, `Agent::continue_run`
                // (`cyrup-agent/src/agent/lifecycle.rs:209`, `cyrup-agent/src/loop_fn.rs:200`
                // and `:280`) sees a trailing assistant with both queues empty and returns
                // `ContinueFromAssistant`, and `drive_run` (`session/run.rs`) logs it and stops —
                // overflow recovery compacts and never retries.
                //
                // The predicate is pi's exact one — `stopReason === "error" || === "length"` — and
                // is deliberately NARROWER than [`Self::drop_trailing_assistant`]'s "any trailing
                // assistant", which would also swallow a legitimately-completed `Stop`/`ToolUse`
                // turn that the compaction happened to leave last. The predicate IS the
                // narrowness; both go through the same locked `pop_trailing_assistant_if`.
                if will_retry {
                    let _ = self.agent.pop_trailing_assistant_if(|a| {
                        matches!(
                            a.stop_reason,
                            cyrup_core::StopReason::Error | cyrup_core::StopReason::Length
                        )
                    });
                }
                Ok(true)
            }
            Ok(None) => {
                cancel_slot.clear();
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                Ok(false)
            }
            Err(e) => {
                cancel_slot.clear();
                let aborted = matches!(e, cyrup_session::compaction::CompactionError::Aborted);
                // Pi agent-session.ts:2083-2097: on a non-abort failure, emit the reason-tagged
                // recovery/auto-compaction error message; an abort emits no errorMessage.
                let error_message = if aborted {
                    None
                } else if reason == CompactionReason::Overflow {
                    Some(format!("Context overflow recovery failed: {e}"))
                } else {
                    Some(format!("Auto-compaction failed: {e}"))
                };
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message,
                })
                .await;
                if aborted { Ok(false) } else { Err(e.into()) }
            }
        }
    }

    /// The effective compaction settings with the live `enabled` toggle applied.
    fn effective_compaction_settings(&self) -> CompactionSettings {
        CompactionSettings {
            enabled: self.auto_compaction_enabled(),
            reserve_tokens: self.compaction_settings.reserve_tokens,
            keep_recent_tokens: self.compaction_settings.keep_recent_tokens,
        }
    }
}
