//! Manual compaction and branch-summary cancellation.
//!
//! Pi `compact` (agent-session.ts:1654-1806) — the user-invoked `/compact` path, its
//! `before_compact` extension hook, and the cancel-token guard that keeps `is_compacting()` honest
//! when a caller drops the future mid-flight. The threshold/overflow trigger lives in
//! [`super::auto_compaction`].

use std::sync::Mutex;

use cyrup_agent::AgentMessage;
use cyrup_core::{CancelToken, EntryId};
use cyrup_ext::{CompactionReduction, HostEvent};
use cyrup_session::compaction::{
    CompactionOverride, CompactionPreparation, CompactionReason, Compactor,
};

use crate::compact::DynSummarizer;
use crate::error::SessionServiceError;
use crate::event::{AgentSessionEvent, SummarizationRetrySource, raw_message_to_agent};

use super::AgentSession;

// Doc-only: this guard's rationale is written against its twin in `bash.rs`, which nothing here
// names in code. Same `cfg(doc)` treatment as `types.rs`/`accessors.rs`/`mod.rs`.
#[cfg(doc)]
use super::bash::BashCancelGuard;

impl AgentSession {
    /// Trigger a compaction of the current branch (R-11-014 `compact`; Pi `compact`,
    /// agent-session.ts:1647-1788). Aborts any active run first, emits
    /// `compaction_start`/`compaction_end`, offers the extension `session_before_compact` veto hook,
    /// appends a `CompactionEntry`, and notifies `session_compact`.
    ///
    /// Returns the [`crate::state::CompactionResult`] on success. A refusal is an **error**, never a
    /// success-with-`None` — Pi's `compact` is typed `Promise<CompactionResult>` and `throw`s
    /// (agent-session.ts:1801-1808/1823-1825), so an RPC client / SDK embedder gets a distinguishable
    /// reason: [`SessionServiceError::AlreadyCompacted`], [`SessionServiceError::NothingToCompact`]
    /// or [`SessionServiceError::CompactionCancelled`].
    pub async fn compact(
        &self,
        custom_instructions: Option<String>,
    ) -> Result<crate::state::CompactionResult, SessionServiceError> {
        let reason = CompactionReason::Manual;
        // Disconnect/abort dance: stop the active run before compacting AND wait for it to settle
        // — Pi is `this._disconnectFromAgent(); await this.abort();` (agent-session.ts:1784-1785),
        // and its `abort()` ends in `await this.waitForIdle()`. SEAM-024: compaction installs its
        // own cancel token and rewrites the branch immediately below, so starting that while the
        // aborted turn was still writing tool results raced the transcript it is about to compact.
        self.abort_and_settle().await;
        let cancel = self.session_cancel.child_token();
        // The slot is installed BY the guard so the two can never be written apart — see
        // [`CompactionCancelGuard`] for why a hand-written clear at each `return` is not enough in
        // Rust, and why the ordered `clear()` calls below still stand.
        let mut cancel_slot = CompactionCancelGuard::install(&self.compaction_cancel, cancel.clone());
        self.fanout_emit(AgentSessionEvent::CompactionStart { reason }).await;

        // Pi's very first statement inside `compact()`'s `try` is the model check —
        // `if (!this.model) { throw new Error(formatNoModelSelectedMessage()); }`
        // (agent-session.ts:1790-1792) — thrown AFTER `compaction_start` was emitted (`:1787`) and
        // caught by the same handler that emits `compaction_end` with
        // `errorMessage: "Compaction failed: …"` (`:1908-1917`), which is why the exit below mirrors
        // the `NothingToCompact` arm rather than returning bare.
        let current_model = Self::lock(&self.compaction_model).clone();
        let model = match current_model {
            Some(m) => m,
            None => {
                cancel_slot.clear();
                let err = SessionServiceError::NoModelSelected;
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: false,
                    will_retry: false,
                    error_message: Some(format!("Compaction failed: {err}")),
                })
                .await;
                return Err(err);
            }
        };
        // Pi: `this._summarizationRetryCallbacks({ source: "compaction", reason: "manual" })`
        // (agent-session.ts:1859).
        let (retry_observer, retry_rx) = crate::compact::summarization_retry_channel(
            SummarizationRetrySource::Compaction { reason },
        );
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        // Pi threads the session thinking level into every compaction summarization call
        // (`agent-session.ts:1855,2129`); `summarization_reasoning` applies the `model.reasoning`
        // gate before it reaches the request.
        let compactor = Compactor::new(summarizer).with_thinking(self.thinking_level().await);
        let settings = self.compaction_settings.clone();

        // Compute the REAL preparation BEFORE the extension hook (Pi computes `prepareCompaction`
        // then fires `session_before_compact` against it, agent-session.ts:1663-1693; L4 gap #5).
        // `None` ⇒ nothing to compact — this is the ONLY preparation (no double-prep: the same
        // `prep` feeds `run_compaction_prepared` below).
        let (prep, branch_entries) = {
            let guard = self.manager.lock().await;
            match compactor.prepare(&guard, &settings) {
                Some(x) => x,
                None => {
                    // Distinguish WHY, exactly as Pi does (agent-session.ts:1801-1807): a branch that
                    // already ends in a `compaction` entry is "Already compacted"; anything else is
                    // "Nothing to compact (session too small)".
                    let already = matches!(
                        guard.branch_path(None).last(),
                        Some(cyrup_session::entry::Entry::Known(
                            cyrup_session::entry::KnownEntry::Compaction { .. }
                        ))
                    );
                    drop(guard);
                    cancel_slot.clear();
                    let err = if already {
                        SessionServiceError::AlreadyCompacted
                    } else {
                        SessionServiceError::NothingToCompact
                    };
                    // Pi's catch emits `compaction_end` with `errorMessage: "Compaction failed: …"`
                    // for a non-abort throw (agent-session.ts:1908-1917).
                    self.fanout_emit(AgentSessionEvent::CompactionEnd {
                        reason,
                        result: None,
                        aborted: false,
                        will_retry: false,
                        error_message: Some(format!("Compaction failed: {err}")),
                    })
                    .await;
                    return Err(err);
                }
            }
        };

        // session_before_compact ext hook: veto (cancel) OR return a compaction override, both seen
        // against the real preparation (agent-session.ts:1672-1693).
        let external_override = match self
            .emit_before_compact(
                &prep,
                &branch_entries,
                custom_instructions.as_deref(),
                reason,
                false,
                &cancel,
            )
            .await
        {
            BeforeCompactOutcome::Cancel => {
                cancel_slot.clear();
                // Pi throws "Compaction cancelled" (agent-session.ts:1824); its catch classifies that
                // exact message as an ABORT, so `compaction_end` carries `aborted:true` and NO
                // errorMessage (agent-session.ts:1909-1916).
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                return Err(SessionServiceError::CompactionCancelled);
            }
            BeforeCompactOutcome::Proceed(ov) => ov,
        };

        let mut guard = self.manager.lock().await;
        let result = compactor
            .run_compaction_prepared(
                &mut guard,
                &model,
                custom_instructions,
                &prep,
                external_override,
                cancel,
            )
            .await;
        // Estimate the rebuilt context size for the result payload (Pi `estimateMessagesTokens`).
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
        cancel_slot.clear();

        match result {
            Ok(Some(entry)) => {
                // SEAM-112 — the re-seed is on the SUCCESS PATH ONLY. pi orders it
                // `appendCompaction(...)` → `this.agent.state.messages = sessionContext.messages;`
                // (agent-session.ts:1952-1955 manual / :2275-2280 auto), both of them AFTER the
                // `signal.aborted` early-return, so a cancelled / declined / failed compaction
                // leaves `agent.state.messages` exactly as the run found it.
                //
                // cyrup ran it unconditionally, before `match result`. That resurrected work a
                // failed compaction had no right to restore: overflow recovery calls
                // `drop_trailing_assistant` (`:4775-4781`) to remove the overflow response from the
                // agent transcript BEFORE compacting, but the response is already persisted (it was
                // written on `message_end`), so an aborted or erroring compaction re-seeded it
                // straight back out of the session file — the exact state `Agent::continue_run`
                // rejects with `ContinueFromAssistant`.
                //
                // pi `agent-session.ts:1955` (manual `compact`) / `:2280` (`_runAutoCompaction`):
                // re-seed the AGENT's in-memory transcript from the compacted context.
                // `appendCompaction` only writes the JSONL entry — this assignment is what actually
                // shrinks the next request.
                //
                // Without it `/compact` reported success and the TUI re-rendered a compacted
                // transcript from the session, while the very next turn still shipped the ENTIRE
                // pre-compaction history to the provider: zero token reduction, full cost. Overflow
                // recovery was worse than useless — `check_compaction` set
                // `overflow_recovery_attempted`, `continue_run()` resent the unchanged context, it
                // overflowed again, and the one-shot latch reported "Try reducing context or
                // switching to a larger-context model", blaming the model for a compaction that had
                // never taken effect.
                //
                // `navigate_tree` already did exactly this (`:1857-1862`, citing
                // `agent-session.ts:2871`); the two compaction paths were the ones that built the
                // context only to COUNT it.
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
                // session_compact ext notify (agent-session.ts:1740-1747): the full Pi payload —
                // the produced compaction entry, whether an extension drove it, reason, retry flag.
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
                            will_retry: false,
                        },
                        &notify_cancel,
                    )
                    .await;
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: Some(cr.clone()),
                    aborted: false,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                Ok(cr)
            }
            // The compactor produced no entry — the same refusal Pi reports as "Compaction
            // cancelled" (agent-session.ts:1824/1869). Unreachable on this path (the preparation is
            // already known non-empty), kept so a future `None` is reported, not silently dropped.
            Ok(None) => {
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted: true,
                    will_retry: false,
                    error_message: None,
                })
                .await;
                Err(SessionServiceError::CompactionCancelled)
            }
            Err(e) => {
                let aborted = matches!(e, cyrup_session::compaction::CompactionError::Aborted);
                let error_message = if aborted {
                    None
                } else {
                    Some(format!("Compaction failed: {e}"))
                };
                self.fanout_emit(AgentSessionEvent::CompactionEnd {
                    reason,
                    result: None,
                    aborted,
                    will_retry: false,
                    error_message,
                })
                .await;
                if aborted {
                    // An in-flight abort (Esc during `/compact` → `abort_compaction`) is the SAME
                    // refusal Pi raises as the bare `Compaction cancelled`
                    // (agent-session.ts:1869 `if (this._compactionAbortController.signal.aborted)
                    // { throw new Error("Compaction cancelled"); }`), propagated verbatim to an RPC
                    // client by rpc-mode.ts:789-795. Surfacing the wrapped
                    // `SessionServiceError::Compaction` here would emit `compaction: compaction
                    // cancelled` instead, and Pi's own catch classifies an abort by comparing
                    // `message === "Compaction cancelled"` (agent-session.ts:1911), so the exact
                    // string is load-bearing.
                    Err(SessionServiceError::CompactionCancelled)
                } else {
                    Err(e.into())
                }
            }
        }
    }

    /// Fire the `session_before_compact` extension hook against a REAL preparation and reduce the
    /// guest's decision (L4 gap #5). Shared by manual [`Self::compact`] and [`Self::run_auto_compaction`].
    /// Returns [`BeforeCompactOutcome::Cancel`] on a veto, else the (optional) compaction override.
    pub(super) async fn emit_before_compact(
        &self,
        prep: &CompactionPreparation,
        branch_entries: &[cyrup_session::entry::Entry],
        custom_instructions: Option<&str>,
        reason: CompactionReason,
        will_retry: bool,
        cancel: &CancelToken,
    ) -> BeforeCompactOutcome {
        if self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionBeforeCompact)
        {
            return BeforeCompactOutcome::Proceed(None);
        }
        let preparation = compaction_preparation_value(prep);
        let branch = serde_json::to_value(branch_entries).unwrap_or_else(|_| serde_json::json!([]));
        match self
            .services
            .ext_host
            .emit_session_before_compact(
                preparation,
                branch,
                custom_instructions.map(str::to_string),
                compaction_reason_str(reason),
                will_retry,
                cancel,
            )
            .await
        {
            CompactionReduction::Blocked { .. } => BeforeCompactOutcome::Cancel,
            CompactionReduction::Override(v) => {
                BeforeCompactOutcome::Proceed(Some(parse_compaction_override(&v)))
            }
            CompactionReduction::Proceed => BeforeCompactOutcome::Proceed(None),
        }
    }

    /// Cancel an in-flight manual **or auto** compaction — Pi `abortCompaction`
    /// (`agent-session.ts:1930-1933` @v0.83.0), whose whole body is two aborts:
    ///
    /// ```ts
    /// abortCompaction(): void {
    ///     this._compactionAbortController?.abort();
    ///     this._autoCompactionAbortController?.abort();
    /// }
    /// ```
    ///
    /// SESS-041: the second line was missing. `run_auto_compaction` installs its own child token in
    /// `auto_compaction_cancel` (`:4535`) and NEVER writes `compaction_cancel`, so an auto
    /// compaction — the 10-18 s one a user actually wants to escape, since they did not ask for it
    /// — was unreachable from here: `abort_compaction` cancelled a `None` and returned, and the run
    /// went to completion. `is_compacting` already reads both (`:4393`), which is what made the
    /// asymmetry invisible.
    pub fn abort_compaction(&self) {
        if let Some(c) = Self::lock(&self.compaction_cancel).as_ref() {
            c.cancel();
        }
        if let Some(c) = Self::lock(&self.auto_compaction_cancel).as_ref() {
            c.cancel();
        }
    }

    /// Cancel an in-flight branch summarization (Pi `abortBranchSummary`, agent-session.ts:1796).
    pub fn abort_branch_summary(&self) {
        if let Some(c) = Self::lock(&self.branch_summary_cancel).as_ref() {
            c.cancel();
        }
    }
}

/// Clears the compaction cancel-token slot it installed into, whatever happens to the future that
/// installed it — the same role [`BashCancelGuard`] plays for `_bashAbortControllers`, and for the
/// same reason.
///
/// pi's `compact` / `_runAutoCompaction` clear `this._compactionAbortController` /
/// `this._autoCompactionAbortController` on every settling path, and a JS `async fn` ALWAYS settles.
/// A Rust future does not: `AgentSession::compact` is a public API whose body is one 10-20 s
/// provider call surrounded by `.await`s, and any caller that races it — `tokio::time::timeout`
/// around the `cyrup-sdk` handle (`cyrup-sdk/src/handle.rs:285`), or `run_rpc`'s `select!` dropping
/// the driver when the write pump reports a broken pipe (`cyrup-modes/src/rpc.rs:668-676`) — drops
/// it mid-flight. The hand-written clears at each `return` cannot run then, so the slot keeps a
/// token nobody is awaiting and **`is_compacting()` answers true forever**. That is not a leaked
/// allocation: the TUI's Submit arm consults `session.is_compacting()` before anything else
/// (`cyrup-tui/src/app.rs`, the `AppAction::Submit if session.is_compacting()` arm), so every
/// subsequent prompt the user types is diverted into the compaction queue and drained by a
/// `compaction_end` that can never arrive — the session accepts input and silently sends none of
/// it, with no way out short of a restart.
///
/// [`Self::clear`] is what the settling paths call, so the slot is still emptied at pi's exact
/// point in the sequence — **before** `compaction_end` is fanned out, which is load-bearing: the
/// TUI drains its compaction queue in response to that event and would re-queue the whole batch if
/// `is_compacting()` were still true when it did. Clearing only in `Drop` would move the clear
/// after the emit and reintroduce that race, so this guard is a backstop for the dropped-future
/// path, not a replacement for the ordered clear. Once cleared it disarms, so it can never wipe a
/// token a later compaction installed.
pub(super) struct CompactionCancelGuard<'a> {
    slot: &'a Mutex<Option<CancelToken>>,
    armed: bool,
}

impl<'a> CompactionCancelGuard<'a> {
    /// Install `cancel` into `slot` and arm the guard.
    pub(super) fn install(slot: &'a Mutex<Option<CancelToken>>, cancel: CancelToken) -> Self {
        *AgentSession::lock(slot) = Some(cancel);
        Self { slot, armed: true }
    }

    /// Clear the slot now (pi's ordered clear), and disarm.
    pub(super) fn clear(&mut self) {
        if self.armed {
            *AgentSession::lock(self.slot) = None;
            self.armed = false;
        }
    }
}

impl Drop for CompactionCancelGuard<'_> {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The reduced `session_before_compact` decision (L4 gap #5): cancel the compaction, or proceed with
/// an optional extension-supplied compaction override.
pub(super) enum BeforeCompactOutcome {
    /// A handler vetoed the compaction (Pi `{cancel:true}`).
    Cancel,
    /// Proceed — `Some` carries the guest's compaction override (Pi
    /// `SessionBeforeCompactResult.compaction`), `None` runs the default model summarization.
    Proceed(Option<CompactionOverride>),
}

/// Serialize a [`CompactionPreparation`] into the Pi `CompactionPreparation` byte-shape (camelCase)
/// for the `session_before_compact` seam (compaction.ts:690-700): the guest reads the real cut point,
/// the messages to summarize, the file operations, and the compaction settings.
///
/// `messagesToSummarize`/`turnPrefixMessages` are RAW `AgentMessage`s (Pi's own element type), so a
/// guest sees `{"role":"bashExecution","command":…}` / `{"role":"custom","customType":…}` rather
/// than the `convertToLlm`-rendered user messages, and `!!`-excluded bash commands are included.
fn compaction_preparation_value(prep: &CompactionPreparation) -> serde_json::Value {
    serde_json::json!({
        "firstKeptEntryId": prep.first_kept_entry_id,
        "messagesToSummarize": prep.messages_to_summarize,
        "turnPrefixMessages": prep.turn_prefix_messages,
        "isSplitTurn": prep.is_split_turn,
        "tokensBefore": prep.tokens_before,
        "previousSummary": prep.previous_summary,
        "fileOps": prep.file_ops.to_details(),
        "settings": prep.settings,
    })
}

/// Parse a guest compaction override (Pi `SessionBeforeCompactResult.compaction`, a `CompactionResult`)
/// into a [`CompactionOverride`]. A missing `summary` degrades to empty (never a panic).
fn parse_compaction_override(v: &serde_json::Value) -> CompactionOverride {
    CompactionOverride {
        summary: v.get("summary").and_then(|s| s.as_str()).unwrap_or_default().to_string(),
        first_kept_entry_id: v.get("firstKeptEntryId").and_then(|s| s.as_str()).map(EntryId::from),
        tokens_before: v.get("tokensBefore").and_then(serde_json::Value::as_u64),
        details: v.get("details").cloned(),
        // Pi threads `extensionCompaction.usage` straight into `appendCompaction`
        // (`agent-session.ts:1844,1872`); a malformed/absent bag simply records no usage.
        usage: v.get("usage").and_then(|u| serde_json::from_value(u.clone()).ok()),
    }
}

/// The Pi wire string for a compaction `reason` (`"manual"|"threshold"|"overflow"`).
pub(super) fn compaction_reason_str(r: CompactionReason) -> &'static str {
    match r {
        CompactionReason::Manual => "manual",
        CompactionReason::Threshold => "threshold",
        CompactionReason::Overflow => "overflow",
    }
}
