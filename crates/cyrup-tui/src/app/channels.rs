use super::*;

impl<B: Backend> App<B> {
    /// Install the off-task `/tree` navigation channel and hand back its receiver.
    ///
    /// [`App::run`] calls this once at startup; without it [`Self::begin_tree_navigation`] falls
    /// back to awaiting the navigation inline, which is only ever correct for a NON-summarizing
    /// navigation. `pub` so `tests/*.rs` can exercise the spawned path (and therefore the
    /// Escape→abort routing and the live `IndicatorKind::BranchSummary` indicator) without standing
    /// up a whole run loop.
    pub fn install_tree_nav_channel(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<TreeNavMsg> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<TreeNavMsg>();
        self.tree_nav_tx = Some(tx);
        rx
    }

    /// Install the off-task `/share` gist-upload channel and hand back its receiver.
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_tree_nav_channel`].
    /// Without it `App::share_session` awaits `gh gist create` inline and the run loop is frozen for
    /// the whole upload — no frame with the loader on it, no spinner tick, no key read — see
    /// [`Self::share_tx`]. `pub` so a test can drive the spawned path (and therefore the
    /// Escape→`Share cancelled` routing) without standing up a run loop.
    pub fn install_share_channel(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<ShareMsg> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<ShareMsg>();
        self.share_tx = Some(tx);
        rx
    }

    // ========================================================================
    // `/login` + `/logout` (Pi `interactive-mode.ts:4941-5051`, `:5229-5403`)
    // ========================================================================

    /// Install the off-task `/login` channel and hand back its receiver.
    ///
    /// [`App::run`] calls this once at startup, exactly like
    /// [`Self::install_tree_nav_channel`]. Without it [`Self::begin_provider_login`] refuses to
    /// start a flow — see [`Self::login_tx`] for why there is no inline fallback.
    ///
    /// `pub` so `tests/*.rs` can drive a whole login without standing up a run loop (the crate's
    /// established run-loop-only testing seam, same as [`Self::open_extension_dialog`]).
    pub fn install_login_channel(&mut self) -> tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LoginUiMsg>();
        self.login_tx = Some(tx);
        rx
    }

    /// Install the off-task `/compact` channel and hand back its receiver (TUI-055).
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_tree_nav_channel`].
    /// Without it `C::Compact` awaits the compaction inline and the run loop is frozen for its whole
    /// duration — see [`Self::compact_tx`] for the measurement that made this necessary. `pub` so a
    /// test can drive the spawned path without standing up a run loop.
    pub fn install_compact_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<CompactOutcome> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<CompactOutcome>();
        self.compact_tx = Some(tx);
        rx
    }

    /// Install the off-task queue-drain channel and hand back its receiver (TUI-092 §5b.1).
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_compact_channel`].
    /// Without it `Escape` and `Alt+Up` await `AgentSession::drain_queue` on the run loop's own
    /// task, and that call ends in an awaited send into the BOUNDED channel the loop itself is the
    /// only drain of — a self-deadlock, not a slow path. See [`Self::queue_drain_tx`] for the full
    /// cycle. `pub` so a test can drive the spawned path without standing up a run loop.
    pub fn install_queue_drain_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<QueueDrain> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<QueueDrain>();
        self.queue_drain_tx = Some(tx);
        rx
    }

    /// Finish a settled [`QueueDrain`] on the run loop's task (TUI-092 §5b.1).
    ///
    /// Everything here was inline in the `Escape` / `Alt+Up` arms and in
    /// [`Self::begin_tree_navigation`] before the split, in exactly this order — only the
    /// `drain_queue().await` itself moved off-task. `take_compaction_queue` and
    /// `restore_queued_to_editor` are `&mut self` and could not move anyway; the abort is
    /// deliberately kept AFTER the restore, which is Pi's own order
    /// (`restoreQueuedMessagesToEditor({abort: true})`, interactive-mode.ts:2636-2637 — restore
    /// first, "and only then abort"), and safely so because the queues were already taken
    /// atomically by the drain that produced this message.
    ///
    /// Shared by every reason so the interleave cannot drift between them, mirroring
    /// [`Self::apply_compact_outcome`].
    pub fn apply_queue_drain(&mut self, drained: QueueDrain, session: &Arc<AgentSession>) {
        let QueueDrain {
            steering,
            follow_up,
            reason,
        } = drained;
        // TUI-031 — Pi's `clearAllQueues` (`interactive-mode.ts:3959-3971`) drains the SESSION's two
        // queues AND `compactionQueuedMessages`, in `[...steering, ...compactionSteering]` /
        // `[...followUp, ...compactionFollowUp]` order. Without the second source an Escape
        // mid-compaction left the compaction queue holding messages the user believed they had just
        // taken back. `/tree` keeps Pi's narrower `[...steering, ...followUp]` (`:4781-4785`).
        let queued: Vec<String> = if reason == QueueDrainReason::TreeNav {
            steering.into_iter().chain(follow_up).collect()
        } else {
            let compaction = self.take_compaction_queue();
            steering
                .into_iter()
                .chain(
                    compaction
                        .iter()
                        .filter(|m| !m.follow_up)
                        .map(|m| m.text.clone()),
                )
                .chain(follow_up)
                .chain(
                    compaction
                        .iter()
                        .filter(|m| m.follow_up)
                        .map(|m| m.text.clone()),
                )
                .collect()
        };
        let restored = self.restore_queued_to_editor(&queued);
        match reason {
            QueueDrainReason::Interrupt => {
                session.abort();
                // Also kill a running bash child (the block was already marked cancelled in
                // `apply_action`); the reader task's terminal `Done` clears `bash_rx`.
                session.abort_bash();
            }
            QueueDrainReason::Dequeue => match restored {
                0 => self
                    .state
                    .transcript
                    .push_status("No queued messages to restore"),
                n => self.state.transcript.push_status(format!(
                    "Restored {n} queued message{} to editor",
                    if n > 1 { "s" } else { "" }
                )),
            },
            // The abort already happened in the spawning task, ahead of `navigate_tree`.
            QueueDrainReason::TreeNav => {}
        }
    }

    /// Install the off-task session-lifecycle channel and hand back its receiver (TUI-092 §5b.2).
    ///
    /// [`App::run`] calls this once at startup, exactly like [`Self::install_compact_channel`].
    /// Without it `/new`, `/reload`, `/import`, `/resume` and `/fork` await their runtime op on the
    /// run loop's own task, where a guest session-lifecycle hook that opens a `ui.*` dialog
    /// deadlocks the loop against itself. See [`Self::lifecycle_tx`]. `pub` so a test can drive the
    /// spawned path without standing up a run loop.
    pub fn install_lifecycle_channel(
        &mut self,
    ) -> tokio::sync::mpsc::UnboundedReceiver<LifecycleOutcome> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LifecycleOutcome>();
        self.lifecycle_tx = Some(tx);
        rx
    }

    /// Apply a settled session-lifecycle op on the run loop's task (TUI-092 §5b.2).
    ///
    /// On failure or cancellation the OPTIMISTIC `pending_swap_status` is cleared before the status
    /// line is shown: no generation bump follows a failed op, so nothing would ever consume it and
    /// it would otherwise surface against the NEXT swap, attributing this command's message to an
    /// unrelated one.
    ///
    /// Shared by the spawned and inline paths so the two cannot drift, mirroring
    /// [`Self::apply_compact_outcome`].
    pub fn apply_lifecycle_outcome(&mut self, outcome: LifecycleOutcome) {
        let effects = match outcome.0 {
            Ok(effects) => effects,
            Err(status) => {
                self.state.pending_swap_status = None;
                self.state.transcript.push_status(status);
                return;
            }
        };
        if let Some(text) = effects.selected_text {
            self.state.editor.set_text(&text);
        }
        if let Some(agent_dir) = effects.reload_keybindings_in {
            // TUI-051 — Pi's ordering: session reload first, THEN `this.keybindings.reload()`
            // (`interactive-mode.ts:5386`). A malformed document must not wipe the live keymap
            // silently, so the error is surfaced; the maps have already been reset to defaults by
            // then, which is also what pi's replace-semantics `rebuild()` leaves behind.
            // CFG-038 — a rejected ENTRY is reported by id and the rest of the document still
            // applies; only an unusable DOCUMENT keeps the old whole-file wording.
            match self.reload_keybindings_from(&agent_dir) {
                Err(e) => self
                    .state
                    .transcript
                    .push_status(format!("keybindings error: {e}")),
                Ok(issues) => {
                    for issue in issues {
                        self.state
                            .transcript
                            .push_status(format!("keybindings: ignoring {issue}"));
                    }
                }
            }
        }
    }

    /// Run a session-lifecycle op off the run loop's task whenever one is servicing
    /// [`Self::lifecycle_tx`], and finish it through [`Self::apply_lifecycle_outcome`]
    /// (TUI-092 §5b.2).
    ///
    /// The caller sets `pending_swap_status` OPTIMISTICALLY before calling this, because the
    /// runtime's generation bump and this channel are two independent paths: once the op is spawned,
    /// the `session_swapped` arm can fire BEFORE the outcome message arrives, and it reads
    /// `pending_swap_status` to caption the swap. Setting it after the fact would leave that arm
    /// painting an unattributed swap. [`Self::apply_lifecycle_outcome`] clears it if the op turns
    /// out to have failed or been cancelled.
    ///
    /// `None` — an embedder or a test with no run loop — awaits inline, exactly as `/compact` does.
    pub(crate) async fn dispatch_lifecycle(
        &mut self,
        op: impl std::future::Future<Output = LifecycleOutcome> + Send + 'static,
    ) {
        match self.lifecycle_tx.clone() {
            Some(tx) => {
                tokio::spawn(async move {
                    let _ = tx.send(op.await);
                });
            }
            None => {
                let outcome = op.await;
                self.apply_lifecycle_outcome(outcome);
            }
        }
    }

    /// Take-all both session queues and finish through [`Self::apply_queue_drain`], off the run
    /// loop's task whenever one is servicing [`Self::queue_drain_tx`] (TUI-092 §5b.1).
    ///
    /// `None` — an embedder or a test driving the action directly — awaits inline, exactly as
    /// `/compact` and `/tree` do. That is correct there and only there: with no run loop there is no
    /// `events` subscription for `drain_queue`'s fan-out to block against.
    pub(crate) async fn dispatch_queue_drain(
        &mut self,
        session: &Arc<AgentSession>,
        reason: QueueDrainReason,
    ) {
        match self.queue_drain_tx.clone() {
            Some(tx) => {
                let session = session.clone();
                tokio::spawn(async move {
                    let (steering, follow_up) = session.drain_queue().await;
                    let _ = tx.send(QueueDrain {
                        steering,
                        follow_up,
                        reason,
                    });
                });
            }
            None => {
                let (steering, follow_up) = session.drain_queue().await;
                self.apply_queue_drain(
                    QueueDrain {
                        steering,
                        follow_up,
                        reason,
                    },
                    session,
                );
            }
        }
    }

    /// Render a settled `/compact` — the summary message on success, Pi's reason string on refusal.
    ///
    /// Shared by the inline and spawned paths so the two cannot drift; the run loop calls it from
    /// the `compact_rx` arm.
    pub fn apply_compact_outcome(&mut self, outcome: CompactOutcome) {
        match outcome {
            // Render the compaction-summary message (`compaction-summary-message.ts`): the
            // `[compaction]` label + `**Compacted from N tokens**` markdown body produced by the
            // op (Pi appends a `CompactionSummaryMessage` after a manual `/compact`).
            Ok(result) => {
                let usage = result.usage.clone();
                self.state
                    .transcript
                    .push_compaction_summary(result.tokens_before, result.summary);
                // The manual half of pi's `compaction_end` cost notice
                // (`interactive-mode.ts:3431-3437`). It rides HERE and not only on the event
                // because of the `[CYRUP-DELTA]` recorded on the `CompactionEnd` arm
                // (`app/events_fold.rs`): that arm renders the AUTOMATIC reasons only, so a
                // `/compact` would otherwise print its summary with no cost line while an
                // auto-compaction printed both. `CompactionResult::usage` (SEAM-034) is the same
                // value the entry carries, which is the same value pi reads off `event.result`.
                if self.state.show_cache_miss_notices
                    && let Some(u) = usage.as_ref()
                {
                    self.state.transcript.push_compaction_cost_notice(
                        crate::transcript::CompactionCostKind::Compaction,
                        u,
                    );
                }
            }
            // SESS-040 — the failure half of the MANUAL compaction surface, which this path owns
            // in full (see the `[CYRUP-DELTA]` on the `CompactionEnd` arm: the event renders the
            // automatic reasons only, because upstream's `/compact` handler renders nothing at all
            // and cyrup's returns an outcome here instead).
            //
            // Pi's manual `compaction_end` branches are BOTH `showError`
            // (`interactive-mode.ts:3099-3100` aborted, `:3116-3117` `errorMessage`), never the dim
            // `showStatus` (`:3200-3213`); and pi classifies the abort by comparing the thrown
            // message to the bare `"Compaction cancelled"` (`agent-session.ts:1911`) — the same
            // test on the same string, because `SessionServiceError::CompactionCancelled`'s
            // `Display` is that message verbatim (`cyrup-session-svc/src/error.rs:92`).
            //
            // Before this: pressing the Escape the band advertises produced the dim status line
            // `compact error: Compaction cancelled` — a cyrup-invented prefix that reports the
            // user's own deliberate cancel as an error, in the wrong channel. A genuine failure
            // took the same dim line, where pi shows `Compaction failed: …` in error styling (the
            // wrapper its catch applies at `agent-session.ts:1908-1917`, which cyrup already emits
            // verbatim on the `compaction_end` event — this path was the one that disagreed).
            Err(e) if e == "Compaction cancelled" => self.state.transcript.push_error(e),
            Err(e) => self
                .state
                .transcript
                .push_error(format!("Compaction failed: {e}")),
        }
    }
}
