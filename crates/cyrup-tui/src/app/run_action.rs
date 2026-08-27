use super::*;

use super::run::RunCtx;
use super::run_arms::RunFlow;

impl App<InlineBackend<Stdout>> {
    /// The nested `AppAction` dispatch of the run loop's input arm (§7.2): the twelve-way match
    /// that routes a mapped action to its session effect. Arm bodies moved verbatim from
    /// `App::run`; control-flow exits surface as [`RunFlow`].
    pub(crate) async fn dispatch_run_action(
        &mut self,
        ctx: &mut RunCtx,
        action: AppAction,
    ) -> Result<RunFlow, TuiError> {
        match action {
                AppAction::Quit => return Ok(RunFlow::Break),
                AppAction::Suspend => self.suspend()?,
                AppAction::OpenExternalEditor => {
                    let editor_cmd = resolve_external_editor(&ctx.session);
                    self.open_external_editor(&editor_cmd)?;
                }
                AppAction::OpenExternalEditorForSelector => {
                    let editor_cmd = resolve_external_editor(&ctx.session);
                    self.open_external_editor_for_selector(&editor_cmd)?;
                }
                // ADR-0005 §B-8 tail + §B-11 — the async half of a completed fullscreen
                // selection. pi flashes in the alternate screen rather than writing a status line,
                // because there is no status line to write to (`interactive-mode.ts:6107-6112`);
                // `renderer_mut` resolves to whichever renderer is live, so this is correct in both
                // modes without a branch here.
                AppAction::CopySelection(text) => {
                    let ok = crate::clipboard::copy_to_clipboard(&text).await;
                    let message = if ok { "Copied!" } else { "Copy failed" };
                    self.renderer_mut().flash(message, None);
                }
                AppAction::Interrupt => {
                    ctx.session.abort();
                    // Also kill a running bash child (the block was already marked cancelled
                    // in `apply_action`); the reader task's terminal `Done` clears `bash_rx`.
                    ctx.session.abort_bash();
                }
                AppAction::InterruptRestoreQueued => {
                    // Pi `onEscape` while streaming (interactive-mode.ts:2636-2637):
                    // `restoreQueuedMessagesToEditor({abort: true})` — take-all BOTH queues,
                    // put their text back in the editor, and only then abort. Without the
                    // restore, an Esc during a turn silently discards every steering /
                    // follow-up message the user typed while it ran.
                    // TUI-031 — Pi's `clearAllQueues` (`interactive-mode.ts:3959-3971`)
                    // drains the SESSION's two queues AND `compactionQueuedMessages`, in
                    // `[...steering, ...compactionSteering]` /
                    // `[...followUp, ...compactionFollowUp]` order. Without the second
                    // source an Escape mid-compaction left the compaction queue holding
                    // messages the user believed they had just taken back.
                    //
                    // TUI-092 §5b.1 — the take-all is SPAWNED, not awaited here.
                    // `drain_queue` ends in an awaited send into every subscription's
                    // BOUNDED channel, one of which is this loop's own `events` receiver:
                    // awaiting it on this task is a self-deadlock the moment that channel is
                    // full. The interleave, the editor restore and the abort all still
                    // happen, in this exact order, in `apply_queue_drain`.
                    self.dispatch_queue_drain(&ctx.session, QueueDrainReason::Interrupt).await;
                }
                AppAction::AbortCompaction => {
                    ctx.session.abort_compaction();
                }
                AppAction::AbortBranchSummary => {
                    // Pi `:4793` — cancel the summarization only. The spawned navigation
                    // resolves with `{cancelled: true, aborted: true}`, and the `tree_nav_rx`
                    // arm re-shows the tree; the indicator/Escape rebind are torn down there.
                    ctx.session.abort_branch_summary();
                }
                AppAction::RunBash { command, excluded } => {
                    // Replace any prior job (Pi keeps one `bashComponent`; a second `!` while
                    // the first still runs supersedes it).
                    ctx.session.abort_bash();
                    ctx.bash_rx = Some(spawn_session_bash(ctx.session.clone(), command, excluded));
                }
                AppAction::Submit(text) if ctx.session.is_compacting()
                    && !is_extension_command(&ctx.session, &text) =>
                {
                    // TUI-031 — Pi tests `this.session.isCompacting` **before** the
                    // streaming branch (`interactive-mode.ts:2813-2822` @v0.83.0): an
                    // extension command runs immediately, anything else goes to
                    // `queueCompactionMessage(text, "steer")` and returns. cyrup consulted
                    // `is_streaming` only, and the session layer has no compaction guard
                    // either (`AgentSession::prepare` has none, and `is_streaming` reads the
                    // agent snapshot, which compaction does not set — compaction ABORTS the
                    // active run), so a message typed during a 10-20 s compaction was
                    // dispatched as a fresh turn assembled from a context the compaction was
                    // in the middle of rewriting, with no status and no queue.
                    self.queue_compaction_message(text, false);
                }
                AppAction::Submit(text) => {
                    // Spawned, not awaited inline (L4 review §2.1 — the SAME deadlock reason
                    // as `ExtensionShortcut` below): `prompt_accepted`/`steer` run Pi's
                    // pre-send extension-command dispatch + `input`-hook fan-out INLINE, before
                    // the run itself is spawned (`session.rs` `prepare` →
                    // `try_execute_extension_command` / `emit_input_event`), and either can
                    // call a guest's synchronous `ui.*` capability — this is in fact the MOST
                    // common guest-reentrant path (an extension's own `/command` handler, or an
                    // `on_input` hook, prompting for confirmation). This arm never touches
                    // `self.state` — the optimistic transcript echo already happened
                    // synchronously in `dispatch_submission` — so no channel-back is needed.
                    let session = ctx.session.clone();
                    tokio::spawn(async move {
                        let ui = UserInput::text(text, InputSource::Tui);
                        if session.is_streaming().await {
                            let _ = session.steer(ui).await;
                        } else {
                            let _ = session.prompt_accepted(ui).await;
                        }
                    });
                }
                AppAction::FollowUp(text) => {
                    // Pi `handleFollowUp` (interactive-mode.ts:3554-3585): while a turn is
                    // streaming, queue the text as a follow-up (delivered once the agent goes
                    // idle — a SEPARATE queue from `steer`); when idle, Alt+Enter behaves like a
                    // plain Enter submit. The editor is cleared here (Pi's `setText("")` in both
                    // branches) since `apply_action` deferred the mutation until this async
                    // streaming check. Spawned, not awaited, for the same guest-reentrancy reason
                    // as `Submit`.
                    self.state.editor.clear();
                    // TUI-031 — Pi's follow-up path has the identical compaction gate:
                    // `this.queueCompactionMessage(text, "followUp")`
                    // (`interactive-mode.ts:3744`), ahead of the streaming branch.
                    if ctx.session.is_compacting() && !is_extension_command(&ctx.session, &text) {
                        self.queue_compaction_message(text, true);
                    } else {
                        let streaming = ctx.session.is_streaming().await;
                        // TUI-016 / TUI-052 — no optimistic echo in EITHER branch. The idle
                        // branch is Pi's plain submit, which also writes nothing to the chat
                        // container; the bubble arrives with `message_start`.
                        let session = ctx.session.clone();
                        tokio::spawn(async move {
                            let ui = UserInput::text(text, InputSource::Tui);
                            if streaming {
                                let _ = session.follow_up(ui).await;
                            } else {
                                let _ = session.prompt_accepted(ui).await;
                            }
                        });
                    }
                }
                AppAction::Dequeue => {
                    // Pi `handleDequeue` → `restoreQueuedMessagesToEditor`
                    // (interactive-mode.ts:3587-3594,3852-3871): drain BOTH the steering and
                    // follow-up queues (steering first, then follow-up — Pi's
                    // `[...steering, ...followUp]` order), join their text by blank lines, and
                    // prepend it to the current editor buffer. When nothing is queued, show
                    // Pi's exact `No queued messages to restore` status and leave the editor
                    // untouched.
                    // One atomic take-all (Pi's `clearAllQueues()` returns what it drained),
                    // not a read-then-clear pair — the split form loses any message queued
                    // between the two calls.
                    //
                    // TUI-092 §5b.1 — spawned for the same reason as the Escape arm above:
                    // `drain_queue`'s fan-out awaits a send into this loop's own bounded
                    // `events` channel. `apply_queue_drain` does the `clearAllQueues`
                    // interleave, the restore and Pi's status line.
                    self.dispatch_queue_drain(&ctx.session, QueueDrainReason::Dequeue).await;
                }
                AppAction::Command(cmd) => {
                    self.execute_command(cmd, &ctx.session, ctx.runtime.as_ref()).await;
                    if should_honor_extension_shutdown(&ctx.session, false) {
                        return Ok(RunFlow::ReturnOk);
                    }
                }
                AppAction::ExtensionShortcut(key) => {
                    // Route the fired shortcut to the owning live extension (R-08-017; Pi
                    // `registerShortcut` handler) — SPAWNED, not awaited inline (L4 review
                    // §2.1). The shortcut handler may itself call a synchronous
                    // `ui.{confirm,input,select,editor}` capability, which blocks ITS calling
                    // tokio task on `ui_roundtrip`'s one-shot reply until this very `select!`
                    // loop services `ui_rx` and answers it. Awaiting `run_shortcut` inline HERE
                    // would make that blocked task and the loop that must unblock it the SAME
                    // task — a single task's `poll()` can never reach a sibling `select!` arm
                    // while it is synchronously blocked deeper in its own call stack (tokio's
                    // `block_in_place` frees a WORKER THREAD for other tasks, not this task's
                    // own other branches) — a genuine self-deadlock. Spawning it as its own task
                    // keeps the main loop free to poll `ui_rx` concurrently, exactly why
                    // `SessionManager::spawn_run` already spawns agent-turn tool execution
                    // (session.rs `drive_run`) instead of awaiting it inline. A guest fault
                    // (or, now, a spawn-side error) is surfaced as a status block via
                    // `shortcut_status_tx`, never a panic; the run loop keeps going regardless.
                    let ext_host = ctx.session.services().ext_host.clone();
                    let shortcut_cancel = ctx.cancel.clone();
                    let status_tx = ctx.shortcut_status_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = ext_host.run_shortcut(&key, &shortcut_cancel).await {
                            let _ = status_tx.send(format!("shortcut {key}: {e}"));
                        }
                    });
                }
                AppAction::Redraw | AppAction::None => {}

        }
        Ok(RunFlow::Continue)
    }

    /// The run loop's input arm (§7.2): drain every queued key BEFORE drawing, dispatch each
    /// through [`Self::dispatch_run_action`], then draw once and bump the liveness beacon
    /// (TUI-092 F3). Moved verbatim from `App::run`; control-flow exits surface as [`RunFlow`].
    pub(crate) async fn on_input_event(
        &mut self,
        ctx: &mut RunCtx,
        maybe_in: Option<InputEvent>,
        input: &mut EventStream<InputEvent>,
        events: &mut EventStream<AgentSessionEvent>,
    ) -> Result<RunFlow, TuiError> {
                    // SEAM-022, `rpc.rs:836-844`: a replacement may have landed since this arm last
                    // ran (the `session_swapped` arm settles anything caused BY one of the keys this
                    // very call is about to service). Settle it BEFORE the keys are serviced, so a
                    // submitted prompt reaches the session the runtime is actually serving and never
                    // the disposed one.
                    if ctx.gen_rx.as_mut().is_some_and(|rx| rx.has_changed().unwrap_or(false)) {
                        self.on_session_swapped(ctx, events).await?;
                    }
                    let _arm = ArmGuard::enter("input");
                    let Some(first) = maybe_in else { return Ok(RunFlow::Break) };
                    // TUI-092 F3 — drain every queued key BEFORE drawing: key auto-repeat (30–60/s)
                    // against a slow frame is otherwise an unbounded one-frame-per-key backlog the
                    // loop can never catch up on (the backlog half of the phase-4 lockup). The reader
                    // thread's channel stays unbounded by design — it is a `std::thread` that cannot
                    // `.await`, and a bounded channel's `try_send` would drop the user's keys —
                    // drain-on-read bounds the backlog's PROCESSING to one frame per wakeup, which
                    // is the property that matters.
                    let mut pending = std::collections::VecDeque::from([first]);
                    while let Some(Some(ev)) = input.next().now_or_never() {
                        pending.push_back(ev);
                    }
                    let mut serviced = 0u64;
                    while let Some(ev) = pending.pop_front() {
                        let action = self.handle_input(&ev);
                        match self.dispatch_run_action(ctx, action).await? {
                            RunFlow::Continue => {}
                            flow => return Ok(flow),
                        }
                        serviced += 1;
                    }
                    self.draw_synchronized()?;
                    // TUI-092 — the liveness beacon the input reader's wedge detector watches.
                    // Once per serviced event, and deliberately AFTER the single draw, so it still
                    // means "serviced", not "started": a frame the user never sees is not service.
                    // This remains the ONLY place it is bumped — counting loop iterations instead
                    // would call a spinner-starved loop healthy, which is the very state the escape
                    // hatch exists for.
                    for _ in 0..serviced {
                        mark_input_serviced();
                    }
        Ok(RunFlow::Continue)
    }
    pub(crate) async fn on_session_event(
        &mut self,
        ctx: &mut RunCtx,
        ev: AgentSessionEvent,
        events: &mut EventStream<AgentSessionEvent>,
    ) -> Result<RunFlow, TuiError> {
                    // TUI-092 — names this arm for its duration; the input reader's wedge detector
                    // reads it, and an overrun is reported on the next healthy iteration. Nothing is
                    // interrupted: a `tokio::time::timeout` here would be inert against the real
                    // wedge (a `block_in_place`d task is never polled again) AND would silently
                    // destroy the compaction queue `ingest_session_event` takes before it awaits.
                    // ONE guard brackets the WHOLE drain (TUI-092 F3): the wedge detector keeps
                    // seeing a single "events" span, not N.
                    let _arm = ArmGuard::enter("events");
                    // TUI-092 F3 — drain every already-queued event BEFORE drawing: N queued deltas
                    // cost N state folds and ONE frame, not N frames. Bounded in practice by the
                    // channel's CHANNEL_CAPACITY (1024) + awaited sends; backstopped by ARM_BUDGET.
                    // `now_or_never` polls once and drops the `Next` future — cancel-safe on tokio
                    // mpsc, so a pending poll loses nothing.
                    //
                    // A closed stream can no longer enter this handler at all: `select!`'s
                    // `Some(ev) = events.next()` pattern (`run.rs`) is refutable, so a `None` from a
                    // dead subscription (every session swap ends the old one, `subscriber.rs:89-93`)
                    // disables the branch instead of matching it — there is no seed to drain, and no
                    // early-return path needed for one.
                    let mut pending = std::collections::VecDeque::from([ev]);
                    while let Some(Some(ev)) = events.next().now_or_never() {
                        pending.push_back(ev);
                    }
                    while let Some(ev) = pending.pop_front() {
                        // Computed BEFORE the ingest call so F8's by-value swap stays a one-line
                        // change (the owned call moves `ev`).
                        let info_changed = matches!(ev, AgentSessionEvent::SessionInfoChanged { .. });
                        let settled = matches!(ev, AgentSessionEvent::AgentSettled);
                        // EXT-006: fold through the extension-aware path so a registered renderer
                        // actually draws the block (a custom message / a tool row). No renderer for the
                        // event's key ⇒ a sync pre-check short-circuits and this is the old behavior.
                        // `ingest_session_event_owned` adds the footer's context-usage refresh, which
                        // needs the session this arm holds (`footer.ts:108`).
                        // TUI-092 F8 — the OWNED ingest: this drain owns every event it dequeued, so
                        // the payloads (`args` / `partial_result` / `result` / the queue vectors) MOVE
                        // into the transcript instead of being cloned per event, which cost CPU
                        // proportional to payload size on the one path event rate multiplies.
                        self.ingest_session_event_owned(ev, &ctx.session).await;
                        // A rename recomputed the window title inside `ingest_event`; the OSC 0 write is
                        // this loop's (Pi `session_info_changed` → `updateTerminalTitle`, `:2900-2903`).
                        // Gated on the event kind so no other event pays for a title recomputation.
                        if info_changed && let Some(title) = self.state.terminal_title.clone() {
                            write_terminal_title(&title);
                        }
                        // SEAM-005 / EXT-005: a guest's `ctx.shutdown()` is honored at the settle point
                        // (Pi interactive-mode.ts:3137-3138 `case "agent_settled": await
                        // this.checkShutdownRequested()`), and only there — `agent_end` cannot tell us
                        // whether a retry or a queued continuation is still coming. Returning mid-drain
                        // is correct: the process is exiting, so the still-queued residue is moot.
                        if should_honor_extension_shutdown(&ctx.session, settled) {
                            return Ok(RunFlow::ReturnOk);
                        }
                    }
                    self.draw_synchronized()?;
        Ok(RunFlow::Continue)
    }
}
