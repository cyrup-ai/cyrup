//! Fork, branch and tree navigation.
//!
//! Pi `agent-session-runtime.ts:259-284` + `agent-session.ts` `branch`/`navigateTree`. Anchoring a
//! new session file at an entry, the optional branch summary generated on the way, and the
//! `/tree` navigation that re-roots the live manager.

use std::path::Path;

use cyrup_agent::AgentMessage;
use cyrup_core::{CancelToken, Content, EntryId, ModelRef, ModelThinkingLevel, SessionId, Usage};
use cyrup_ext::{HostEvent, TreeReduction};
use cyrup_provider::Model;
use cyrup_session::compaction::{BranchSummaryOutput, Compactor, NoHooks};
use cyrup_session::manager::SessionManager;

use crate::compact::DynSummarizer;
use crate::error::SessionServiceError;
use crate::event::{SummarizationRetrySource, raw_message_to_agent};

use super::AgentSession;
use super::transcript::{custom_message_text, user_message_text};
use super::types::{
    ForkAnchor, ForkOutcome, ForkPosition, NavigateTreeOptions, NavigateTreeOutcome,
};

impl AgentSession {
    /// Navigate the session leaf to `entry` (no file mutation; R-04-023).
    pub async fn branch(&self, entry: EntryId) -> Result<(), SessionServiceError> {
        self.manager.lock().await.branch(&entry)?;
        Ok(())
    }

    /// Navigate to `entry`, recording a branch-summary of the abandoned branch (R-04-024/R-05-016).
    /// Returns the summary text, if one was produced.
    pub async fn branch_with_summary(
        &self,
        entry: EntryId,
        user_wants_summary: bool,
    ) -> Result<Option<String>, SessionServiceError> {
        // Pi `navigateTree`: `if (options.summarize && !this.model) { throw new Error("No model
        // available for summarization"); }` (agent-session.ts:2910-2912) — a DIFFERENT string from
        // `formatNoModelSelectedMessage`, and gated on the caller actually asking for a summary, so
        // a modelless session can still navigate the tree without one.
        let current_model = crate::sync::lock(&self.compaction_model).clone();
        let model = match current_model {
            Some(m) => m,
            // pi's gate is on the SUMMARY, not the navigation, so a modelless session still moves
            // the leaf; only the summarizing variant is refused.
            None if !user_wants_summary => {
                // `Compactor::run_branch_summary` takes `&Model` unconditionally even though it
                // only reads it to build the summary, so the no-summary navigation is done
                // directly here (`session.branch(&target_id)`, compaction/mod.rs:384). Residual:
                // the `session_before_tree`/`session_tree` hooks that `run_branch_summary` also
                // fires are skipped on this narrow path — `run_branch_summary` should take
                // `Option<&Model>` so they are not.
                self.manager.lock().await.branch(&entry)?;
                return Ok(None);
            }
            None => return Err(SessionServiceError::NoModelForSummarization),
        };
        // Pi: `this._summarizationRetryCallbacks({ source: "branchSummary" })`
        // (agent-session.ts:2998).
        let (retry_observer, retry_rx) =
            crate::compact::summarization_retry_channel(SummarizationRetrySource::BranchSummary);
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        let compactor = Compactor::new(summarizer, NoHooks);
        let cancel = self.session_cancel.child_token();
        *crate::sync::lock(&self.branch_summary_cancel) = Some(cancel.clone());

        let mut guard = self.manager.lock().await;
        let old_leaf = guard.leaf_id().cloned();
        let entry_opt = compactor
            .run_branch_summary(
                &mut guard,
                &model,
                entry,
                old_leaf,
                user_wants_summary,
                &self.branch_summary_settings,
                cancel,
            )
            .await;
        drop(guard);
        // Close + flush the retry queue with the manager guard already released (see
        // `spawn_event_pump`).
        drop(compactor);
        let _ = retry_pump.await;
        *crate::sync::lock(&self.branch_summary_cancel) = None;
        Ok(entry_opt?.map(|e| e.summary))
    }

    /// The unified `/tree` navigation op (Pi `navigateTree(targetId, options)`,
    /// agent-session.ts:2704-2895). Navigates the leaf to `target`, optionally summarizing the
    /// abandoned branch, and returns `{editor_text, cancelled, aborted, summary_entry}`:
    ///
    /// - No-op (`{cancelled:false}`) when `target` is already the leaf (agent-session.ts:2712).
    /// - The `session_before_tree` extension hook may veto the navigation (`{cancelled:true}`,
    ///   agent-session.ts:2757).
    /// - When summarizing, an aborted summarization returns `{cancelled:true, aborted:true}`
    ///   (agent-session.ts:2796).
    /// - A `user`/`custom_message` target re-roots the leaf at the target's PARENT and returns the
    ///   target's text as `editor_text` (so a UI can re-edit it); any other target navigates to the
    ///   target itself (agent-session.ts:2823-2841).
    /// - The summary is attached at the navigation target position via `branch_with_summary`
    ///   (agent-session.ts:2847); the `label` lands on the summary entry, or — with no summary — on
    ///   the target (agent-session.ts:2858/2867). Finally the agent transcript is rebuilt from the
    ///   navigated context and `session_tree` is emitted (agent-session.ts:2871-2884).
    pub async fn navigate_tree(
        &self,
        target: EntryId,
        options: NavigateTreeOptions,
    ) -> Result<NavigateTreeOutcome, SessionServiceError> {
        use cyrup_session::compaction::{
            branch_token_budget, collect_entries_for_branch_summary, prepare_branch_entries,
        };
        use cyrup_session::entry::{Entry, KnownEntry};

        // Phase 1 (guard held): read the session to compute the navigation target + the branch
        // collection, then build the real `TreePreparation` for the extension hook. The guard is
        // RELEASED before the hook so a guest may read the session during `session_before_tree`
        // without a re-entrant manager-lock deadlock (agent-session.ts:2704-2751; L4 gap #5).
        let (old_leaf, new_leaf, editor_text, collection, common_ancestor_id) = {
            let guard = self.manager.lock().await;
            let old_leaf = guard.leaf_id().cloned();

            // No-op if already at target, BEFORE the hook (agent-session.ts:2712).
            if old_leaf.as_ref() == Some(&target) {
                return Ok(NavigateTreeOutcome::default());
            }

            // "Model required for summarization" (agent-session.ts:2910-2912) — after the
            // already-at-target no-op and BEFORE the target lookup, exactly as pi orders it.
            if options.summarize && crate::sync::lock(&self.model).is_none() {
                return Err(SessionServiceError::NoModelForSummarization);
            }

            // Target must exist (agent-session.ts:2721).
            let target_entry = guard
                .entry(&target)
                .cloned()
                .ok_or_else(|| SessionServiceError::InvalidForkEntry(target.to_string()))?;

            // Determine the new leaf position + re-editable text by target type
            // (agent-session.ts:2823-2841).
            let (new_leaf, editor_text): (Option<EntryId>, Option<String>) = match &target_entry {
                Entry::Known(KnownEntry::Message { .. })
                    if user_message_text(&target_entry).is_some() =>
                {
                    (target_entry.parent_id(), user_message_text(&target_entry))
                }
                Entry::Known(KnownEntry::CustomMessage { content, .. }) => {
                    (target_entry.parent_id(), Some(custom_message_text(content)))
                }
                _ => (Some(target.clone()), None),
            };

            let old_path: Vec<Entry> =
                guard.branch_path(old_leaf.as_ref()).into_iter().cloned().collect();
            let target_path: Vec<Entry> =
                guard.branch_path(Some(&target)).into_iter().cloned().collect();
            let collection = collect_entries_for_branch_summary(&old_path, &target_path);
            let common_ancestor_id = collection.common_ancestor_id.clone();
            (old_leaf, new_leaf, editor_text, collection, common_ancestor_id)
        };

        // Phase 2 (no guard): session_before_tree ext hook — veto OR a summary/customInstructions/
        // label override, against the real `TreePreparation` (agent-session.ts:2752-2783).
        let mut eff_custom_instructions = options.custom_instructions.clone();
        let mut eff_replace_instructions = options.replace_instructions;
        let mut eff_label = options.label.clone();
        let mut override_summary: Option<(String, serde_json::Value)> = None;
        if !self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionBeforeTree)
        {
            let preparation = serde_json::json!({
                "targetId": target,
                "oldLeafId": old_leaf,
                "commonAncestorId": common_ancestor_id,
                "entriesToSummarize": collection.entries,
                "userWantsSummary": options.summarize,
                "customInstructions": options.custom_instructions,
                "replaceInstructions": options.replace_instructions,
                "label": options.label,
            });
            let cancel = self.session_cancel.child_token();
            match self.services.ext_host.emit_session_before_tree(preparation, &cancel).await {
                TreeReduction::Blocked { .. } => {
                    return Ok(NavigateTreeOutcome { cancelled: true, ..Default::default() });
                }
                TreeReduction::Override(v) => {
                    if let Some(ci) = v.get("customInstructions").and_then(|s| s.as_str()) {
                        eff_custom_instructions = Some(ci.to_string());
                    }
                    if let Some(ri) = v.get("replaceInstructions").and_then(serde_json::Value::as_bool)
                    {
                        eff_replace_instructions = ri;
                    }
                    if let Some(lbl) = v.get("label").and_then(|s| s.as_str()) {
                        eff_label = Some(lbl.to_string());
                    }
                    // A summary override (Pi `SessionBeforeTreeResult.summary = {summary, details?}`)
                    // is used directly as the branch summary (fromExtension), skipping the model.
                    if let Some(s) = v.get("summary") {
                        let text = s
                            .get("summary")
                            .and_then(|t| t.as_str())
                            .or_else(|| s.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let details =
                            s.get("details").cloned().unwrap_or_else(|| serde_json::json!({}));
                        override_summary = Some((text, details));
                    }
                }
                TreeReduction::Proceed => {}
            }
        }

        // Phase 3 (STILL no guard): summarize the abandoned branch (unless the extension supplied
        // the summary), threading the (possibly overridden) instructions.
        //
        // The manager mutex is deliberately NOT held across this leg. Branch summarization is a full
        // provider round-trip plus its retry backoff — holding `self.manager` across it would stall
        // every other session-manager consumer (the tree/DAG readers a TUI polls, an extension's
        // `getEntries`, a concurrent `compact`) for the whole call. `AgentSession::compact` already
        // scopes its guard for exactly this reason; this path used to take the guard first and hold
        // it through the `.await`, which was invisible only because no front end could reach the
        // summarize branch (SESS-023). Pi has no equivalent hazard: its session manager is not
        // mutex-guarded.
        let mut from_extension_summary = false;
        // (text, details, usage) — `usage` is the summarization call's token spend, persisted on the
        // appended `branch_summary` entry (Pi `BranchSummaryEntry.usage`). An extension-supplied
        // summary reports none.
        let mut summary_payload: Option<(String, serde_json::Value, Option<Usage>)> = None;
        if let Some((text, details)) = override_summary {
            // The extension supplied the branch summary directly (agent-session.ts:2762-2775).
            if options.summarize {
                summary_payload = Some((text, details, None));
                from_extension_summary = true;
            }
        } else if options.summarize && !collection.entries.is_empty() {
            // Summarize the abandoned branch (agent-session.ts:2787). Pi still appends the non-empty
            // "No content to summarize" placeholder, so we gate only on the collected entry count.
            // Non-`None` here: the `options.summarize && !this.model` gate above already returned
            // `NoModelForSummarization` (agent-session.ts:2910-2912), and this arm is inside
            // `options.summarize`.
            let model = crate::sync::lock(&self.compaction_model)
                .clone()
                .ok_or(SessionServiceError::NoModelForSummarization)?;
            // `(contextWindow || 128000) − reserve` (Pi `branch-summarization.ts:315-317`). The
            // fallback matters: without it a model reporting a zero context window would get budget
            // `0`, which `prepare_branch_entries` reads as "no limit".
            let budget =
                branch_token_budget(&model, self.branch_summary_settings.reserve_tokens);
            let prep = prepare_branch_entries(&collection.entries, budget);
            let cancel = self.session_cancel.child_token();
            *crate::sync::lock(&self.branch_summary_cancel) = Some(cancel.clone());
            let result = self
                .generate_branch_summary_with_instructions(
                    &prep,
                    &model,
                    eff_custom_instructions.as_deref(),
                    eff_replace_instructions,
                    cancel,
                )
                .await;
            *crate::sync::lock(&self.branch_summary_cancel) = None;
            match result {
                Ok(produced) => {
                    let details = serde_json::to_value(prep.file_ops.to_details())
                        .unwrap_or_else(|_| serde_json::json!({}));
                    summary_payload = Some((produced.text, details, produced.usage));
                }
                Err(cyrup_session::compaction::CompactionError::Aborted) => {
                    return Ok(NavigateTreeOutcome {
                        cancelled: true,
                        aborted: true,
                        ..Default::default()
                    });
                }
                Err(e) => return Err(e.into()),
            }
        }

        // Phase 4 (guard re-held): apply the navigation + summary/label (agent-session.ts:2845-2868).
        let mut guard = self.manager.lock().await;
        let summary_entry = match &summary_payload {
            Some((text, details, usage)) => {
                let id = guard.branch_with_summary(
                    new_leaf.as_ref(),
                    text.clone(),
                    Some(details.clone()),
                    usage.clone(),
                    from_extension_summary,
                )?;
                let entry = branch_summary_entry_of(&guard, &id);
                if let Some(label) = eff_label.as_deref() {
                    guard.append_label(&id, Some(label))?;
                }
                entry
            }
            None => {
                match new_leaf.as_ref() {
                    None => guard.reset_leaf(),
                    Some(id) => guard.branch(id)?,
                }
                // No summary entry to label → label the navigation target itself.
                if let Some(label) = eff_label.as_deref() {
                    guard.append_label(&target, Some(label))?;
                }
                None
            }
        };

        // Rebuild the agent transcript from the navigated context (agent-session.ts:2871).
        // SESS-043 — the RAW projection, matching pi's `this.agent.state.messages =
        // sessionContext.messages` (`agent-session.ts:3067-3068` @v0.83.0); see the two compaction
        // re-seeds for why the flattened twin gave the wrong list.
        let raw = guard.build_context_raw();
        let new_leaf_id = guard.leaf_id().cloned();
        drop(guard);
        let msgs: Vec<AgentMessage> = raw.iter().map(raw_message_to_agent).collect();
        self.agent.set_messages(msgs).await;

        // session_tree notify (agent-session.ts:2877). cyrup collapses the Pi payload into one
        // `tree` JSON value (the SDK forwards it to the guest as `tree_json`).
        if !self
            .services
            .ext_host
            .dispatcher()
            .no_subscribers(cyrup_ext::EventKind::SessionTree)
        {
            let tree = serde_json::json!({
                "newLeafId": new_leaf_id,
                "oldLeafId": old_leaf,
                "summaryEntry": summary_entry,
                "fromExtension": summary_payload.as_ref().map(|_| from_extension_summary),
            });
            let notify_cancel = self.session_cancel.child_token();
            self.services
                .ext_host
                .dispatcher()
                .dispatch_notify(&HostEvent::SessionTree { tree }, &notify_cancel)
                .await;
        }

        Ok(NavigateTreeOutcome { editor_text, cancelled: false, aborted: false, summary_entry })
    }

    /// Generate a branch summary with optional custom/replace instructions (Pi `generateBranchSummary`
    /// with `customInstructions`/`replaceInstructions`, branch-summarization.ts:318-336). cyrup-session's
    /// `generate_branch_summary` takes no instruction knobs, so the `/tree` op threads them here over
    /// the same public branch-summary primitives.
    async fn generate_branch_summary_with_instructions(
        &self,
        prep: &cyrup_session::compaction::BranchPreparation,
        model: &Model,
        custom_instructions: Option<&str>,
        replace_instructions: bool,
        cancel: CancelToken,
    ) -> Result<BranchSummaryOutput, cyrup_session::compaction::CompactionError> {
        use cyrup_session::compaction::{
            format_file_operations, serialize_conversation, SummarizationRequest, Summarizer,
            BRANCH_SUMMARY_EMPTY_PLACEHOLDER, BRANCH_SUMMARY_PREAMBLE, BRANCH_SUMMARY_PROMPT,
            SUMMARIZATION_SYSTEM_PROMPT,
        };
        // Pi short-circuits BEFORE the model call when there is nothing to summarize
        // (branch-summarization.ts:309-311).
        if prep.messages.is_empty() {
            return Ok(BranchSummaryOutput {
                text: BRANCH_SUMMARY_EMPTY_PLACEHOLDER.to_string(),
                usage: None,
            });
        }
        let transcript = serialize_conversation(&prep.messages);
        // Instruction selection (branch-summarization.ts:319-326): `replace` swaps the default
        // prompt; a bare custom instruction is appended as "Additional focus".
        let instructions = match custom_instructions {
            Some(ci) if !ci.is_empty() && replace_instructions => ci.to_string(),
            Some(ci) if !ci.is_empty() => {
                format!("{BRANCH_SUMMARY_PROMPT}\n\nAdditional focus: {ci}")
            }
            _ => BRANCH_SUMMARY_PROMPT.to_string(),
        };
        let prompt = format!("<conversation>\n{transcript}\n</conversation>\n\n{instructions}");
        // Pi: `this._summarizationRetryCallbacks({ source: "branchSummary" })`
        // (agent-session.ts:2998).
        let (retry_observer, retry_rx) =
            crate::compact::summarization_retry_channel(SummarizationRetrySource::BranchSummary);
        let retry_pump = self.spawn_event_pump(retry_rx);
        let summarizer =
            DynSummarizer::new(self.provider.current(), model.clone(), self.summarization_retry())
                .with_observer(retry_observer);
        let req = SummarizationRequest {
            system_prompt: SUMMARIZATION_SYSTEM_PROMPT,
            prompt_text: prompt,
            max_tokens: 2048,
            model: ModelRef {
                provider: model.provider.clone(),
                api: Some(model.api.clone()),
                model: model.id.clone(),
            },
            // Pi builds the branch-summary options inline (`{ apiKey, headers, env, signal,
            // maxTokens: 2048 }`, branch-summarization.ts:348) rather than through
            // `createSummarizationOptions`, so `reasoning` is never set for a branch summary.
            thinking: ModelThinkingLevel::Off,
        };
        let resp = summarizer.complete(req, cancel).await;
        // Close + flush the retry queue BEFORE the `?` early-returns on a failed summarization, so
        // an exhausted retry still reports its `summarization_retry_finished`.
        drop(summarizer);
        let _ = retry_pump.await;
        let resp = resp?;
        match resp.stop_reason {
            cyrup_core::StopReason::Error => Err(
                cyrup_session::compaction::CompactionError::Summarization(
                    resp.error_message.unwrap_or_default(),
                ),
            ),
            cyrup_core::StopReason::Aborted => {
                Err(cyrup_session::compaction::CompactionError::Aborted)
            }
            // An unsettled response is NOT a summary — same guard, and the same `Deferred`
            // rationale, as `cyrup_session::compaction::{summarize,branch}`.
            cyrup_core::StopReason::Pending | cyrup_core::StopReason::Deferred => {
                Err(cyrup_session::compaction::CompactionError::Summarization(
                    resp.error_message.unwrap_or_else(|| {
                        cyrup_session::compaction::PENDING_SUMMARY.to_string()
                    }),
                ))
            }
            cyrup_core::StopReason::Stop
            | cyrup_core::StopReason::Length
            | cyrup_core::StopReason::ToolUse => {
                let body = resp
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.clone()),
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
                    // The branch-summary call's token spend is persisted on the entry (Pi
                    // `BranchSummaryResult.usage`, `branch-summarization.ts:372`).
                    usage: Some(resp.usage),
                })
            }
        }
    }

    /// Republish `CYRUP_SESSION_ID` / `CYRUP_SESSION_FILE` from the LIVE manager for the next `bash`
    /// child.
    ///
    /// Pi never needs this: `resolveSpawnContext` calls `ctx.sessionManager.getSessionId()` /
    /// `getSessionFile()` at spawn time (bash.ts:172-174), so a `createBranchedSession` that mutates
    /// the manager in place is picked up automatically. cyrup's `Tool::execute` has no session
    /// context, so the branching paths — `fork`, `clone_at`, `fork_at_entry` — push instead.
    /// Without this a `bash` child run after `/fork` would report the PRE-fork session id and a
    /// session file that is no longer the one being appended to.
    fn republish_session_identity(&self, guard: &SessionManager) {
        let mut info = self.bash_session_env.get();
        info.session_id = Some(guard.session_id().to_string());
        info.session_file = guard.session_file().map(Path::to_path_buf);
        self.bash_session_env.set(info);
    }

    /// Fork the current persisted session into a new file under the same cwd (R-04-020/021).
    pub async fn fork(&self) -> Result<SessionId, SessionServiceError> {
        // A fork clones the active path through the current leaf into a new file.
        let mut guard = self.manager.lock().await;
        let layout = branch_layout(&guard);
        // Pi forks at an explicit leaf and mutates the manager in place
        // (`createBranchedSession(leafId)`, session-manager.ts:1292-1392). Fork-at-current-position
        // passes the current leaf; an empty session has nothing to fork.
        let leaf = guard.leaf_id().cloned().ok_or_else(|| {
            cyrup_session::SessionError::EmptyFork(
                guard.session_file().map(Path::to_path_buf).unwrap_or_default(),
            )
        })?;
        guard.create_branched_session(&leaf, &layout)?;
        self.republish_session_identity(&guard);
        let id = guard.session_id().clone();
        Ok(id)
    }

    /// Clone the session at an explicit entry (or the current leaf when `None`) into a new file,
    /// WITHOUT switching the active session to it (arch-11 `clone_at`; distinct from `fork`, which
    /// switches). Returns the new branched session id. Unlike `fork_at_entry`'s `before` anchoring,
    /// `clone_at` anchors the branch leaf at the selected entry itself (the full path up to and
    /// including it is cloned).
    pub async fn clone_at(&self, entry: Option<EntryId>) -> Result<SessionId, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let leaf = match entry {
            Some(e) => {
                guard
                    .entry(&e)
                    .ok_or_else(|| SessionServiceError::InvalidForkEntry(e.to_string()))?;
                e
            }
            None => guard.leaf_id().cloned().ok_or_else(|| {
                cyrup_session::SessionError::EmptyFork(
                    guard.session_file().map(Path::to_path_buf).unwrap_or_default(),
                )
            })?,
        };
        let layout = branch_layout(&guard);
        guard.create_branched_session(&leaf, &layout)?;
        self.republish_session_identity(&guard);
        Ok(guard.session_id().clone())
    }

    /// Entry-anchored fork (Pi `fork(entryId, {position})`, agent-session-runtime.ts:259-344). For
    /// `position:"before"` the anchor must be a *user* message; the new branch leaf is that message's
    /// parent and its text is returned as `selected_text` (so a UI can re-edit it). For
    /// `position:"at"` the new branch leaf is the selected entry itself. A persisted session forks
    /// into a new file via `createBranchedSession(leafId)`; an anchor with no parent (forking before
    /// the very first message) yields a fresh empty session.
    pub async fn fork_at_entry(
        &self,
        entry: &EntryId,
        position: ForkPosition,
    ) -> Result<ForkOutcome, SessionServiceError> {
        let mut guard = self.manager.lock().await;
        let (target_leaf, selected_text) = fork_anchor(&guard, entry, position)?;

        match target_leaf {
            Some(leaf) => {
                let layout = branch_layout(&guard);
                guard.create_branched_session(&leaf, &layout)?;
                self.republish_session_identity(&guard);
                let id = guard.session_id().clone();
                Ok(ForkOutcome { session_id: Some(id), selected_text })
            }
            // Forking before the first user message: nothing to branch from.
            None => Ok(ForkOutcome { session_id: None, selected_text }),
        }
    }

    /// Enumerate the user-message fork anchors on the current branch (Pi `getUserMessagesForForking`,
    /// agent-session.ts:2901) — each `{entry_id, text}` is a candidate the `/fork`/`/tree` UI offers.
    pub async fn user_messages_for_forking(&self) -> Vec<ForkAnchor> {
        let guard = self.manager.lock().await;
        let leaf = guard.leaf_id().cloned();
        guard
            .branch_path(leaf.as_ref())
            .into_iter()
            .filter_map(|e| user_message_text(e).map(|text| ForkAnchor { entry_id: e.id(), text }))
            .collect()
    }

    /// Resolve a fork anchor against the **live** session manager (SEAM-009).
    ///
    /// Pi resolves the anchor BEFORE it splits on persistence: `getEntry(entryId)` +
    /// `throw new Error("Invalid entry ID for forking")` at agent-session-runtime.ts:275-276 and
    /// :282-283, i.e. strictly above the `isPersisted()` branch at :290. cyrup used to resolve it
    /// against a throwaway manager reopened from the session FILE, which meant an unsaved session
    /// had no validation at all (a bogus entry id "succeeded") and no anchor to branch at.
    ///
    /// Reading the live manager is also strictly more correct for the persisted case: a branched
    /// session defers its first file write until an assistant message exists
    /// (`create_branched_session`), so the on-disk copy can legitimately lag the in-memory entries.
    pub(crate) async fn fork_anchor_live(
        &self,
        entry: &EntryId,
        position: ForkPosition,
    ) -> Result<(Option<EntryId>, Option<String>), SessionServiceError> {
        let mgr = self.manager.lock().await;
        fork_anchor(&mgr, entry, position)
    }

    /// Branch the **live, non-persisted** session manager at `target_leaf`, IN PLACE (SEAM-009).
    ///
    /// Pi's in-memory fork branch mutates the very object the outgoing session still holds:
    /// `const sessionManager = this.session.sessionManager; …
    /// sessionManager.createBranchedSession(targetLeafId); await this.teardownCurrent("fork", …)`
    /// (agent-session-runtime.ts:333-341). Branching first and tearing down second is not
    /// incidental: the outgoing run is still writing, and everything it appends while it settles
    /// lands in the *already-branched* manager — which is the manager the fork is built from. That
    /// is how Pi honours its own teardown contract, "the aborted turn (including tool results) is
    /// persisted to the outgoing session before it is replaced" (:167-169), on the fork path.
    ///
    /// So this method deliberately does NOT hand the manager over; [`Self::take_manager`] does, and
    /// the caller must settle the outgoing run in between. Merging the two (branch + move in one
    /// step, as this used to) re-opens the data loss from the other side: every append made between
    /// the move and the teardown goes to the throwaway placeholder and is dropped with it.
    ///
    /// Before any of this, the in-memory arm built a `SessionTarget::New` session and the whole
    /// transcript was silently discarded — unrecoverable, since a non-persisted session has no file
    /// to recover it from.
    pub(crate) async fn branch_live_manager(
        &self,
        target_leaf: &EntryId,
    ) -> Result<(), SessionServiceError> {
        let mut guard = self.manager.lock().await;
        // `create_branched_session` returns early for a non-persisted manager (adopting the branch
        // in memory and returning `None`), so the layout is unused here — pass the manager's own,
        // which is what the persisted arm would use too.
        let layout = branch_layout(&guard);
        guard.create_branched_session(target_leaf, &layout)?;
        Ok(())
    }
}

/// Build a [`cyrup_session::compaction::BranchSummaryEntry`] payload from a freshly appended summary
/// entry (mirrors cyrup-session's private `branch_summary_entry_of`, compaction/mod.rs:309) so the
/// `/tree` op can surface the entry without re-running the summarizer.
fn branch_summary_entry_of(
    mgr: &SessionManager,
    id: &EntryId,
) -> Option<cyrup_session::compaction::BranchSummaryEntry> {
    use cyrup_session::compaction::BranchSummaryEntry;
    use cyrup_session::entry::{Entry, KnownEntry};
    match mgr.entry(id) {
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

/// For a `position:"before"` fork: require a user-message anchor and return `(parent_id, text)`.
fn user_message_anchor(e: &cyrup_session::Entry) -> Option<(Option<EntryId>, String)> {
    user_message_text(e).map(|text| (e.parent_id(), text))
}

/// The [`SessionLayout`] a fork/clone writes its new file into. Mirrors Pi
/// `createBranchedSession`'s reuse of `this.getSessionDir()` (session-manager.ts:918-920,1343): the
/// directory fixed once at manager construction, never re-derived or re-encoded on branch. cyrup's
/// equivalent of `this.sessionDir` is the currently-open session file's own parent directory, which
/// is ALREADY fully resolved (`<root>/--<encoded-cwd>--` for a default session, or a literal
/// `--session-dir`), so it must be used LITERALLY. Feeding it back through the *encoded*
/// [`SessionLayout::new`] would append `--<encoded-cwd>--` a second time and land the branch one
/// directory too deep — orphaning it from every listing/resume path (gap-analysis 05, Finding 1). An
/// in-memory session (no file) never persists a branch, so the default-root fallback is inert.
pub(crate) fn branch_layout(mgr: &SessionManager) -> cyrup_session::SessionLayout {
    match mgr.session_file().and_then(Path::parent) {
        Some(dir) => cyrup_session::SessionLayout::literal(dir.to_path_buf(), mgr.cwd().to_path_buf()),
        None => cyrup_session::SessionLayout::for_cwd(mgr.cwd().to_path_buf()),
    }
}

/// Resolve the branch leaf + optional selected-text for an entry-anchored fork (Pi
/// agent-session-runtime.ts:268-284). Shared by [`AgentSession::fork_at_entry`] and the runtime's
/// throwaway-manager fork path so the anchor semantics stay identical.
pub(crate) fn fork_anchor(
    mgr: &SessionManager,
    entry: &EntryId,
    position: ForkPosition,
) -> Result<(Option<EntryId>, Option<String>), SessionServiceError> {
    let selected = mgr
        .entry(entry)
        .ok_or_else(|| SessionServiceError::InvalidForkEntry(entry.to_string()))?;
    match position {
        ForkPosition::At => Ok((Some(selected.id()), None)),
        ForkPosition::Before => {
            let (parent, text) = user_message_anchor(selected)
                .ok_or_else(|| SessionServiceError::InvalidForkEntry(entry.to_string()))?;
            Ok((parent, Some(text)))
        }
    }
}
