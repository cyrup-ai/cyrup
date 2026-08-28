use super::*;

/// Project one flattened [`SessionDagNode`] into the `/tree` selector's [`TreeNode`].
///
/// `pub` so the projection can be driven directly from a test with a hand-built `SessionDagNode`:
/// it is the production converter (`App::run`'s `/tree` arm maps the whole `session_dag()` through
/// it, `:2338`), and the alternative — standing a real multi-branch `AgentSession` up inside a TUI
/// test — would exercise the session layer rather than this mapping.
pub fn tree_node_from_dag(n: &SessionDagNode) -> TreeNode {
    let kind = match n.kind {
        SessionDagKind::Message | SessionDagKind::Other => TreeKind::Message,
        SessionDagKind::Tool => TreeKind::ToolGroup,
        SessionDagKind::ModelChange => TreeKind::ModelChange,
        SessionDagKind::ThinkingChange => TreeKind::ThinkingChange,
        SessionDagKind::Compaction => TreeKind::Compaction,
    };
    TreeNode {
        id: n.entry_id.to_string(),
        depth: n.depth,
        label: truncate_summary(&n.label),
        kind,
        foldable: n.foldable,
        folded: false,
        has_label: n.has_label,
        // Pi's column here is `labelTimestamp` — WHEN the entry's label was set — and the row it
        // decorates is a labeled one (`tree-selector.ts:741-743`). It was previously fed the literal
        // string `"current"` on the branch tip, which is neither: the `t` toggle
        // (`app.tree.toggleLabelTimestamp`) rendered the word "current" where Pi renders a clock
        // time, and did so on an unlabeled row, in a column Pi leaves off by default.
        //
        // Pi does mark the active path, just not here: `pathMarker` is a `•` prefix ahead of the
        // entry text, driven by an `activePathIds` SET covering the whole root→tip path
        // (`tree-selector.ts:736-738`). `SessionDagNode` carries only `is_leaf`, so that marker is
        // not portable from here either; it is not a substitute this column can hold.
        //
        // Set to `None` until the value exists to put here. It is dropped one and two layers down,
        // not in this crate: `cyrup_session::TreeNode` (manager.rs:29-34) has no timestamp field
        // even though `SessionManager::labels` already holds `(label, label-change timestamp)`
        // (manager.rs:43-44), so `SessionDagNode` (cyrup-session-svc session.rs:136-155) has nothing
        // to carry — its `timestamp` is the ENTRY's, a different quantity. Threading the label
        // timestamp through those two crates is the remaining half of this fix; the render side
        // (Pi's gate, Pi's default, Pi's `[+label time]` marker) is done and will display it the
        // moment a producer sets it.
        time_label: None,
    }
}

impl<B: Backend> App<B> {
    /// Open Pi's three-option "Summarize branch?" prompt (`interactive-mode.ts:4755-4760`). Pi uses
    /// its generic `showExtensionSelector`; cyrup renders the same three options through a
    /// first-party [`ListSelector`] so the answer arrives as an ordinary
    /// [`AppCommand::ConfirmSelection`] rather than occupying the single extension-dialog reply slot.
    pub fn open_branch_summary_prompt(&mut self) {
        let rows = vec![
            (BRANCH_SUMMARY_NONE.to_string(), "No summary".to_string(), None),
            (BRANCH_SUMMARY_YES.to_string(), "Summarize".to_string(), None),
            (
                BRANCH_SUMMARY_CUSTOM.to_string(),
                "Summarize with custom prompt".to_string(),
                None,
            ),
        ];
        let title = SelectorKind::BranchSummary.title().to_string();
        self.open_boxed_selector(
            SelectorKind::BranchSummary,
            Box::new(ListSelector::prompt(title, rows, 0).with_upstream_chrome(
                SelectorKind::BranchSummary,
                &self.state.select_keymap,
            )),
        );
    }

    /// Open the custom-instructions editor (Pi `showExtensionEditor("Custom summarization
    /// instructions")`, `interactive-mode.ts:4769`) — the same INLINE editor component Pi's default
    /// `ExtensionEditorComponent` provides, never a teardown to `$EDITOR`.
    pub(crate) fn open_branch_summary_instructions(&mut self) {
        let title = SelectorKind::BranchSummaryInstructions.title().to_string();
        self.open_boxed_selector(
            SelectorKind::BranchSummaryInstructions,
            Box::new(
                ExtensionEditorSelector::new(title, "")
                    .with_keymaps(&self.state.select_keymap, &self.state.keymap),
            ),
        );
    }

    /// Dispatch the `/tree` navigation the user committed to (Pi `interactive-mode.ts:4781-4820`).
    ///
    /// Pi aborts an in-flight response FIRST — "the user committed to navigating: stop the active
    /// response" (`:4781-4785`), restoring the queued messages to the editor on the way — then runs
    /// `navigateTree`. cyrup did neither before SESS-023.
    ///
    /// The navigation itself is SPAWNED whenever a run loop is present, never awaited on the loop
    /// task. A summarizing navigation is a provider round-trip plus retry backoff; awaited inline in
    /// `App::run`'s `select!` it would freeze the loop for the whole call, so no keystroke could
    /// reach `abort_branch_summary` and no `IndicatorKind::BranchSummary` frame could ever render —
    /// exactly the residual `execute_command`'s own doc comment flags. The outcome comes back over
    /// [`Self::tree_nav_tx`] and is applied by [`Self::apply_tree_nav_outcome`].
    pub(crate) async fn begin_tree_navigation(
        &mut self,
        session: &Arc<AgentSession>,
        target: String,
        summarize: bool,
        custom_instructions: Option<String>,
    ) {
        let opts = NavigateTreeOptions {
            summarize,
            custom_instructions,
            ..NavigateTreeOptions::default()
        };
        let entry = cyrup_core::EntryId::from(target.as_str());
        let Some(tx) = self.tree_nav_tx.clone() else {
            // No run loop (unit/embedder driving `execute_command` directly): await inline. Safe
            // for the non-summarizing path, which makes no model call — and safe for the drain,
            // because with no run loop there is no `events` subscription for its fan-out to block
            // against (see [`Self::queue_drain_tx`]).
            // Pi `:4781-4785` — `restoreQueuedMessagesToEditor()` then `session.abort()`.
            if session.is_streaming().await {
                self.dispatch_queue_drain(session, QueueDrainReason::TreeNav).await;
                session.abort();
            }
            let outcome = session.navigate_tree(entry, opts).await.map_err(|e| e.to_string());
            self.apply_tree_nav_outcome(TreeNavMsg { target, outcome });
            return;
        };
        if summarize {
            // Pi shows the `BranchSummaryStatusIndicator` and rebinds Escape for the duration
            // (`:4796-4799`, `:4792-4795`); both are torn down in `apply_tree_nav_outcome`.
            self.state.branch_summary_in_flight = true;
            self.state
                .indicator
                .set(IndicatorKind::BranchSummary, Some("Summarizing branch...".to_string()));
        }
        let session = session.clone();
        // TUI-092 §5b.1 — Pi's pre-step (`:4781-4785`, "the user committed to navigating: stop the
        // active response") moves INTO this task rather than staying on the loop. Both of its awaits
        // belong off-task: `is_streaming` is cheap but `drain_queue` ends in an awaited send into
        // this loop's own BOUNDED `events` channel, so awaiting it on the loop is the §5b.1
        // self-deadlock. Sequencing is preserved exactly — the drain and the abort still complete
        // BEFORE `navigate_tree` starts, because they are statements in this same task. Only the
        // editor restore travels back to the loop, over `queue_drain_tx`.
        let drain_tx = self.queue_drain_tx.clone();
        tokio::spawn(async move {
            if session.is_streaming().await {
                let (steering, follow_up) = session.drain_queue().await;
                if let Some(drain_tx) = drain_tx {
                    let _ = drain_tx.send(QueueDrain {
                        steering,
                        follow_up,
                        reason: QueueDrainReason::TreeNav,
                    });
                }
                session.abort();
            }
            let outcome = session.navigate_tree(entry, opts).await.map_err(|e| e.to_string());
            let _ = tx.send(TreeNavMsg { target, outcome });
        });
    }

    /// Apply a settled `/tree` navigation (Pi `interactive-mode.ts:4805-4820`).
    ///
    /// The arm ORDER is load-bearing and was wrong before SESS-023: cyrup returns
    /// `{cancelled: true, aborted: true}` on an aborted summarization (matching Pi
    /// `agent-session.ts:3000-3001`), and the old code tested `cancelled` first — so aborting a
    /// summarization printed "tree navigation cancelled" and silently swallowed the tree. Pi tests
    /// `result.aborted` first (`:4805`) and re-shows the tree at the same entry, then `cancelled`
    /// (`:4809`).
    ///
    /// `pub` so `tests/*.rs` can drive the settle half without a live run loop, the same reason
    /// [`Self::open_extension_dialog`] is public.
    pub fn apply_tree_nav_outcome(&mut self, msg: TreeNavMsg) -> Option<AppCommand> {
        let TreeNavMsg { target, outcome } = msg;
        // Pi's `finally` (`:4830-4833`): clear the indicator and restore the Escape binding
        // regardless of how the navigation ended.
        if self.state.branch_summary_in_flight {
            self.state.branch_summary_in_flight = false;
            if self.state.indicator.kind() == IndicatorKind::BranchSummary
                || self.state.indicator.kind() == IndicatorKind::Retry
            {
                if self.state.status.streaming {
                    self.state.indicator.working();
                } else {
                    self.state.indicator.idle();
                }
            }
        }
        match outcome {
            Ok(o) if o.aborted => {
                // Pi `:4805-4808` — status, then re-show the tree at the same entry.
                self.state.transcript.push_status("Branch summarization cancelled");
                self.state.pending_tree_nav = Some(PendingTreeNav { target });
                return Some(AppCommand::OpenSelector(SelectorKind::Tree));
            }
            Ok(o) if o.cancelled => {
                self.state.transcript.push_status("Navigation cancelled");
            }
            Ok(o) => {
                if let Some(text) = o.editor_text {
                    self.state.editor.set_text(&text);
                }
                // A summarized branch navigation records a branch-summary message
                // (`branch-summary-message.ts`) into the transcript.
                if let Some(entry) = o.summary_entry {
                    // Taken before `entry.summary` moves below.
                    let usage = entry.usage.clone();
                    self.state.transcript.push_branch_summary(entry.summary);
                    // pi synthesises the `compaction_cost` notice for a `branch_summary` entry
                    // that carries `usage` (`interactive-mode.ts:3788-3794`); on the live
                    // navigation path the entry is in hand right here, so no re-derivation from
                    // the session is needed.
                    if self.state.show_cache_miss_notices
                        && let Some(u) = usage.as_ref()
                    {
                        self.state.transcript.push_compaction_cost_notice(
                            crate::transcript::CompactionCostKind::BranchSummary,
                            u,
                        );
                    }
                }
                self.state.transcript.push_status("navigated session tree");
            }
            Err(e) => self.state.transcript.push_status(format!("tree error: {e}")),
        }
        None
    }
}
