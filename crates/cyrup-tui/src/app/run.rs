use super::*;

use super::run_arms::RunFlow;

/// The run loop's shared mutable context (§7.2): every binding an arm handler must read or write
/// while the `select!` borrows the receivers and timers it polls. The seven sink senders are
/// re-installed on every session swap, so they must outlive the `select!`.
pub(crate) struct RunCtx {
    pub(crate) session: Arc<AgentSession>,
    pub(crate) runtime: Option<Arc<AgentSessionRuntime>>,
    pub(crate) cancel: CancelToken,
    pub(crate) gen_rx: Option<tokio::sync::watch::Receiver<u64>>,
    /// The 80 ms animation clock — re-armed by the `SetWorkingIndicator` effect arm and the
    /// session-swap arm (TUI-030), so it lives here rather than with the pure timers.
    pub(crate) spinner: tokio::time::Interval,
    pub(crate) overlay_tick: Option<tokio::time::Interval>,
    pub(crate) bash_rx: Option<tokio::sync::mpsc::UnboundedReceiver<BashMsg>>,
    pub(crate) package_update_rx: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>>,
    pub(crate) ui_tx: tokio::sync::mpsc::UnboundedSender<UiRequest>,
    pub(crate) ui_effect_tx: tokio::sync::mpsc::UnboundedSender<UiEffect>,
    pub(crate) ext_error_tx: tokio::sync::mpsc::UnboundedSender<cyrup_ext::ExtensionError>,
    pub(crate) overlay_tx: tokio::sync::mpsc::UnboundedSender<cyrup_session_svc::OverlayRequest>,
    pub(crate) theme_switch_tx: tokio::sync::mpsc::UnboundedSender<cyrup_resources::Theme>,
    pub(crate) shortcut_status_tx: tokio::sync::mpsc::UnboundedSender<String>,
}

impl App<CrosstermBackend<Stdout>> {
    /// The interactive event loop: `select!` over terminal input, the agent event stream, theme
    /// hot-reload, and cancellation (arch-10 §5). Renders with synchronized output. Submissions are
    /// routed to `session` (steer while streaming, else a fresh prompt; R-10-030).
    pub async fn run(
        &mut self,
        mut input: EventStream<InputEvent>,
        mut events: EventStream<AgentSessionEvent>,
        session: Arc<AgentSession>,
        runtime: Option<Arc<AgentSessionRuntime>>,
        mut theme_rx: Option<tokio::sync::watch::Receiver<Arc<ThemeData>>>,
        cancel: CancelToken,
    ) -> Result<(), TuiError> {
        // The active session + its event subscription are re-bound on every runtime replacement
        // (arch-11 §3.4): a session-swap command (or a runtime-side `SessionReplaced`) bumps the
        // runtime's generation `watch`, the loop drops the stale subscription, subscribes the new
        // session, and re-binds the UI ([`App::rebind_session`]). Without a runtime they are fixed.
        let session = session;
        let gen_rx = runtime.as_ref().map(|r| r.watch_generation());
        // The synchronous extension-dialog sink (L4 review §2.1): a loaded guest's `ui.{confirm,input,
        // select,editor}` capability blocks its OWN tokio task on a one-shot
        // (`LiveHostServices::ui_roundtrip`) while this loop's `ui_rx` arm renders the matching dialog
        // and replies once the user answers — the interactive-TUI mirror of `crates/cyrup-modes/src/
        // rpc.rs`'s `run_rpc`, which wires the SAME `UiSink` mechanism for RPC mode. Installed here
        // (only when a TUI is present — `App::run` is never invoked headless) and re-installed on every
        // session swap below, since a replacement session brings a fresh `LiveHostServices`.
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel::<UiRequest>();
        // The FIRE-AND-FORGET sibling of `ui_tx` (TUI-S01). `LiveHostServices::emit_ui_effect` drops
        // every `ui.{notify,set-status,set-widget,set-header,set-footer,set-title,set-editor-text,
        // paste-editor-text,set-tools-expanded}` call when this sink is unset, which is exactly Pi's
        // headless `noOpUIContext` policy (`extensions/runner.ts:230-265`) — but interactive is NOT
        // headless in Pi: it passes a real `uiContext` (`interactive-mode.ts:2223-2268`). Cyrup's RPC
        // mode already installs this (`crates/cyrup-modes/src/rpc.rs`'s `run_rpc`); without the same
        // install here every fire-and-forget extension UI call vanished in the DEFAULT mode. Also
        // re-installed on session swap below, for the same reason `ui_tx` is.
        let (ui_effect_tx, mut ui_effect_rx) = tokio::sync::mpsc::unbounded_channel::<UiEffect>();
        // The THIRD extension seam (TUI-S02): the contained-fault listener Pi's interactive mode
        // passes as `bindExtensions({ … onError })` (`interactive-mode.ts:1700-1701`). Every guest
        // handler fault the dispatcher contains + skips — or contains and turns into a BLOCK — is
        // reported here and drawn into the transcript by the `ext_error_rx` arm below
        // (`show_extension_error`). RPC mode has had this since `run_rpc` was written; interactive
        // had nothing, so `Dispatcher::report` degraded to a `tracing::warn!` and the fault was
        // invisible in the DEFAULT mode. Re-installed on session swap below, for the same reason
        // `ui_tx` is: a replacement session brings a fresh `ExtensionHost`.
        let (ext_error_tx, mut ext_error_rx) =
            tokio::sync::mpsc::unbounded_channel::<cyrup_ext::ExtensionError>();
        // The FOURTH extension seam: an interactive modal an extension owns the state of and this
        // loop owns the terminal for (Pi `ctx.ui.custom(factory, { overlay: true, … })`,
        // `interactive-mode.ts:2719`). `LiveHostServices::open_overlay` blocks the extension's OWN
        // (always spawned) task on a one-shot while this loop pushes the component onto the
        // `state.overlays` z-stack, routes every keystroke to it through the existing
        // `handle_overlay_key` chain, and ticks it at its own cadence. Dropping the overlay — a
        // `Close` outcome, a session swap, a quit — fires the one-shot and releases that task.
        // Re-installed on session swap below, for the same reason `ui_tx` is.
        let (overlay_tx, mut overlay_rx) =
            tokio::sync::mpsc::unbounded_channel::<cyrup_session_svc::OverlayRequest>();
        // The FIFTH extension seam (SEAM-T01): a guest's `setTheme`. Unlike its `set_*` siblings it
        // does NOT ride `ui_effect_tx` — RPC mode installs that sink, and pi's RPC `setTheme` is a
        // hard-coded failure (`modes/rpc/rpc-mode.ts:298-300`), so routing it there would make the
        // switch succeed in a mode upstream refuses it in. `TuiThemeAccess::set` validates the name
        // against the session's discovered themes first (pi's `loadTheme` throw,
        // `theme/theme.ts:622`) and only a RESOLVED theme reaches this channel. Re-installed on
        // session swap below, for the same reason `ui_tx` is.
        let (theme_switch_tx, mut theme_switch_rx) =
            tokio::sync::mpsc::unbounded_channel::<cyrup_resources::Theme>();
        Self::install_ui_sinks(
            &session.services().host_services,
            ui_tx.clone(),
            ui_effect_tx.clone(),
        );
        Self::install_overlay_sink(&session.services().host_services, overlay_tx.clone());
        self.install_extension_readbacks(
            &session.services().host_services,
            Arc::clone(&session.services().resources),
            theme_switch_tx.clone(),
        );
        // The open overlay's self-refresh timer, armed from its own `refresh_ms` when it arrives and
        // dropped when the stack empties. `None` means "no ticking overlay is open", which the
        // `select!` arm below expresses as a `pending()` future rather than a spinning interval.
        let overlay_tick: Option<tokio::time::Interval> = None;
        Self::install_error_listener(&session.services().ext_host, ext_error_tx.clone());
        self.seed_session_ui(&session, runtime.as_ref()).await;
        self.draw_synchronized()?;
        // The spinner tick (spec/tui/01 §6.2 / §10): an 80 ms redraw used **only while** a status
        // indicator is active, so the Braille frame advances without a timer thread and an idle
        // session never busy-loops (the branch is `if`-gated on `indicator.is_active()`).
        let mut spinner = tokio::time::interval(SPINNER_INTERVAL);
        spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The extension-UI dialog countdown tick (Pi's `CountdownTimer`, `countdown-timer.ts:21-30`):
        // a 1s redraw used **only while** an open `ui.{confirm,select,input}` dialog has a
        // guest-set `opts.timeout_ms` armed, so an idle session (or a dialog with no timeout) never
        // pays for it — mirrors the spinner's own `if`-gated pattern immediately above.
        let mut dialog_countdown = tokio::time::interval(Duration::from_secs(1));
        dialog_countdown.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The OSC 9;4 keepalive (Pi's `setInterval(..., TERMINAL_PROGRESS_KEEPALIVE_MS)`,
        // `tui/src/terminal.ts:514-516`): re-send the active sequence once a second for as long as a
        // turn or a compaction is running, because several terminals expire an indeterminate
        // progress state that is not refreshed. Same `if`-gated shape as the spinner above, so an
        // idle session — or any session with the setting off — never writes.
        let mut progress_keepalive = tokio::time::interval(crate::TERMINAL_PROGRESS_KEEPALIVE);
        progress_keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The footer's git-branch refresh (Pi watches `.git/HEAD` with `fs.watch` + a 500 ms debounce,
        // `footer-data-provider.ts`). cyrup polls the same 500 ms instead of holding an inotify
        // watch, and the branch is `if`-gated on actually being inside a repo — outside one this arm
        // never runs at all, and inside one a tick costs a `stat` and repaints only on a real change.
        let mut git_branch_poll = tokio::time::interval(crate::footer_data::POLL_INTERVAL);
        git_branch_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The running-`bash` elapsed tick — Pi's `setInterval(() => context.invalidate(), 1000)`,
        // armed by bash's own `renderResult` while its result is still partial and cleared on the
        // final one (bash.ts:471-479). Without it the `Elapsed …` figure would only advance when
        // some OTHER event happened to redraw. Same `if`-gated shape as the spinner above: an idle
        // session, and any turn not running a bash call, never ticks.
        let mut elapsed_tick = tokio::time::interval(ELAPSED_TICK_INTERVAL);
        elapsed_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // A live `!`/`!!` bash run: the receiver its deltas + terminal result arrive on. Kept as a
        // run-loop local (not on `self`) so the `select!` borrow does not collide with the
        // input-arm `&mut self`. X13 — cancellation is NOT a local token any more: the run goes
        // through `session.execute_bash*`, which owns the child's token (`_bashAbortController`,
        // agent-session.ts:2660), so `Esc` is `session.abort_bash()` — Pi's `abortBash()`.
        let bash_rx: Option<tokio::sync::mpsc::UnboundedReceiver<BashMsg>> = None;
        // A fired extension shortcut is spawned onto its own tokio task (see the
        // `AppAction::ExtensionShortcut` arm below for why); this channel carries its status/error
        // line back to the transcript once it settles, mirroring the `bash_rx` pattern above.
        let (shortcut_status_tx, mut shortcut_status_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (_tmux_warning_tx, mut tmux_warning_rx) = Self::spawn_tmux_keyboard_check();
        // A `/tree` navigation runs on its OWN task (see `App::begin_tree_navigation`) and posts its
        // outcome back here, so a branch summarization's provider round-trip never blocks this loop
        // — the same channel-back shape as `bash_rx` / `shortcut_status_rx`. Installing the sender is
        // what makes the spawned path (and therefore Escape→abort and the live
        // `IndicatorKind::BranchSummary` spinner) reachable at all.
        let mut tree_nav_rx = self.install_tree_nav_channel();
        // The `/login` channel (`login_dialog::LoginUiMsg`): the spawned flow's prompts, progress
        // events and final outcome. Installed for the same reason `tree_nav_rx` is — the flow must
        // not run on this task, or no keystroke could ever answer its prompts.
        let mut login_rx = self.install_login_channel();
        // The `/compact` outcome channel (TUI-055). Installed for exactly the same reason as
        // `tree_nav_rx`: a 10–20 s provider call awaited on THIS task freezes every other arm, so
        // the compaction status band Pi shows for the whole operation never reaches a frame.
        let mut compact_rx = self.install_compact_channel();
        // The queue take-all channel (TUI-092 §5b.1). Installed for the same reason as `compact_rx`
        // and, before it, `tree_nav_rx`: `AgentSession::drain_queue` awaits a send into every live
        // subscription's BOUNDED channel — one of which is the `events` receiver THIS task is the
        // sole drain of — so awaiting it here is a self-deadlock, reachable from an ordinary
        // `Escape` or `Alt+Up` on a busy session.
        let mut queue_drain_rx = self.install_queue_drain_channel();
        // The session-lifecycle channel (TUI-092 §5b.2). Installed for the same reason as the two
        // above: `/new`, `/reload`, `/import`, `/resume` and `/fork` each dispatch a
        // `HostEvent::Session*` hook to every live extension, and a guest that answers one by
        // opening a `ui.*` dialog parks its task until THIS loop services `ui_rx` — which it cannot
        // do while awaiting the op that is waiting for it.
        let mut lifecycle_rx = self.install_lifecycle_channel();
        // The startup package-update check's answer channel, moved out of `self` so the `select!`
        // arm's borrow does not collide with the `&mut self` the other arms take — the same
        // run-loop-local shape as `bash_rx` / `tree_nav_rx`. `None` when the binary wired no channel
        // (offline / `--offline` / `CYRUP_SKIP_VERSION_CHECK`), in which case the arm never resolves.
        let package_update_rx = self.package_update_rx.take();
        let mut ctx = RunCtx {
            session,
            runtime,
            cancel: cancel.clone(),
            gen_rx,
            spinner,
            overlay_tick,
            bash_rx,
            package_update_rx,
            ui_tx,
            ui_effect_tx,
            ext_error_tx,
            overlay_tx,
            theme_switch_tx,
            shortcut_status_tx,
        };
        'run: loop {
            self.drain_over_budget_arm();
            let theme_changed = async {
                match theme_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            // The open overlay's own cadence (Pi arms it inside the component's constructor —
            // `pi-subagents/src/tui/fleet.ts:516-521` `setInterval(… , options.refreshMs ?? 750)`).
            // No ticking overlay ⇒ a future that never resolves, so the arm costs nothing when the
            // z-stack is empty or the open modal is static.
            let overlay_ticked = async {
                match ctx.overlay_tick.as_mut() {
                    Some(t) => {
                        t.tick().await;
                    }
                    None => std::future::pending().await,
                }
            };
            let bash_next = async {
                match ctx.bash_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            // Resolve to `true` when the runtime swaps the active session (generation bump). When no
            // runtime is threaded in, never resolves (single fixed session).
            let package_updates = async {
                match ctx.package_update_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            };
            let session_swapped = async {
                match ctx.gen_rx.as_mut() {
                    Some(rx) => rx.changed().await.is_ok(),
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                // REQUIRED, not a micro-optimisation — the same statement `cyrup-tools/src/lock.rs:
                // 178` makes for its own cancel race, and the shape every `select!` in
                // `cyrup-ext/src/host/live.rs` already uses. Without it tokio picks a READY arm at
                // random, so a loop iteration in which teardown was requested AND a keystroke,
                // agent event or ticker is simultaneously ready could service the work arm instead:
                // one more consumed key, one more drawn frame, one more applied event after the
                // token fired. It terminates quickly in expectation, but nothing in the code bounds
                // how much runs after cancellation — and shutdown ordering is exactly what the
                // token is for. `biased;` makes the cancel arm win every such tie, deterministically.
                //
                // Nothing below depends on being polled ahead of the cancel arm: the five ticker
                // arms are all `if`-guarded and idempotent (a skipped tick is re-armed by
                // `MissedTickBehavior::Skip`), and every channel arm keeps its message queued for
                // the next poll. `src/tests/run_loop_cancel_bias.rs` pins this.
                //
                // THE ORDERING RULE IS NOW STRONGER THAN "cancel first" — it is **cancel, then
                // input, then everything else**, and the second half is as load-bearing as the
                // first (TUI-092 §2.5). `biased;` takes the FIRST ready arm, so any arm above
                // `input.next()` that is ready on every poll starves it *permanently*. The spinner
                // ticker is exactly that: armed for the whole of a streaming turn and re-ready
                // every 80 ms (`SPINNER_INTERVAL`, `status_indicator.rs:48`), so as soon as one
                // `draw_synchronized` costs more than a tick — which is what growing transcripts
                // do — the input arm is never reached again and the keyboard dies while the screen
                // keeps animating. Do NOT "tidy" the input arm back down among the tickers.
                biased;
                _ = cancel.cancelled() => break,
                // Input outranks every ticker (TUI-092 §2.5/§5c). `biased;` takes the FIRST
                // ready arm, and the spinner re-arms every `SPINNER_INTERVAL` (80 ms,
                // `status_indicator.rs:48`) for the whole of a streaming turn — so the moment one
                // `draw_synchronized` costs more than a tick, a spinner arm placed ABOVE this one
                // is always ready when the loop comes round and this arm is never polled again.
                // The loop keeps drawing; the keyboard is dead, progressively, exactly as reported.
                // No `.await` has to hang for that to happen.
                //
                // Nothing is lost by demoting the tickers: they are `if`-guarded and idempotent,
                // `MissedTickBehavior::Skip` re-arms a skipped tick, and this arm ends in
                // `draw_synchronized()` anyway — so servicing a key repaints the frame the spinner
                // would have drawn.
                maybe_in = input.next() => match self.on_input_event(&mut ctx, maybe_in, &mut input).await? {
                    RunFlow::Break => break 'run,
                    RunFlow::ReturnOk => return Ok(()),
                    RunFlow::Continue => {}
                },
                _ = ctx.spinner.tick(),
                    if self.state.indicator.is_active()
                        || self.state.transcript.bash_running() =>
                {
                    self.on_spinner_tick()?
                }
                _ = dialog_countdown.tick(),
                    if self.state.pending_ui_reply.as_ref().is_some_and(|p| p.deadline.is_some()) =>
                {
                    self.on_dialog_countdown_tick()?
                }
                _ = progress_keepalive.tick(), if self.state.terminal_progress.keepalive() => {
                    // Pure terminal output, no UI state — Pi's interval writes the escape and
                    // nothing else, so this arm deliberately does NOT redraw.
                    self.tick_terminal_progress_keepalive();
                }
                _ = elapsed_tick.tick(), if self.state.transcript.has_running_elapsed_tool() => {
                    self.on_elapsed_tick()?
                }
                _ = git_branch_poll.tick(), if self.state.git_branch.in_repo() => {
                    self.on_git_branch_poll()?
                }
                Some(msg) = bash_next => self.on_bash_msg(&mut ctx, msg)?,
                () = overlay_ticked, if !self.state.overlays.is_empty() => self.on_overlay_ticked()?,
                Some(req) = overlay_rx.recv() => self.on_overlay_request(&mut ctx, req)?,
                Some(req) = ui_rx.recv() => self.on_ui_request(req)?,
                Some(effect) = ui_effect_rx.recv() => self.on_ui_effect(&mut ctx, effect)?,
                Some(err) = ext_error_rx.recv() => self.on_ext_error(err)?,
                Some(msg) = shortcut_status_rx.recv() => self.on_shortcut_status(msg)?,
                Some(outcome) = compact_rx.recv() => self.on_compact_outcome(outcome)?,
                Some(outcome) = lifecycle_rx.recv() => self.on_lifecycle_outcome(outcome)?,
                Some(drained) = queue_drain_rx.recv() => self.on_queue_drain(&mut ctx, drained)?,
                maybe_updates = package_updates => self.on_package_updates(&mut ctx, maybe_updates)?,
                Some(warning) = tmux_warning_rx.recv() => self.on_tmux_warning(warning)?,
                Some(theme) = theme_switch_rx.recv() => self.on_theme_switch(&mut ctx, theme).await?,
                Some(msg) = login_rx.recv() => self.on_login_msg(msg)?,
                Some(msg) = tree_nav_rx.recv() => self.on_tree_nav_msg(&mut ctx, msg).await?,
                maybe_ev = events.next() => match self.on_session_event(&mut ctx, maybe_ev, &mut events).await? {
                    RunFlow::Break => break 'run,
                    RunFlow::ReturnOk => return Ok(()),
                    RunFlow::Continue => {}
                },
                ok = theme_changed => self.on_theme_changed(ok, &theme_rx)?,
                swapped = session_swapped => {
                    let _arm = ArmGuard::enter("session_swapped");
                    // A runtime replacement (a `/new`/`/resume`/`/fork`/`/reload`/`/import` op, or a
                    // runtime-side `SessionReplaced`, R-11-021) installed a new active session: drop
                    // the stale subscription, subscribe the NEW session's event stream, and re-bind
                    // the UI. Honors a runtime-driven swap identically to a UI-driven one.
                    if swapped && let Some(rt) = ctx.runtime.as_ref() {
                        let new_session = rt.session().await;
                        events = new_session.subscribe();
                        ctx.session = new_session;
                        self.rebind_session();
                        // TUI-030 — `rebind_session` ran `reset_extension_ui`, whose
                        // `reset_extension_working_state` drops the outgoing extension's
                        // `setWorkingIndicator` options (pi's `this.setWorkingIndicator()` with no
                        // argument, `interactive-mode.ts:2212` @v0.84.2). Upstream that call
                        // re-arms `Loader`'s own `setInterval` back to `DEFAULT_INTERVAL_MS`
                        // (`loader.ts:67-69` → `:77-80`); cyrup has no per-indicator timer — the run
                        // loop's single tick IS the animation clock — so the period has to be
                        // re-read here, exactly as the `SetWorkingIndicator` effect arm above
                        // re-reads it. Without this a guest that asked for `intervalMs: 1000` would
                        // leave the NEXT session's built-in Braille spinner sampled once a second.
                        ctx.spinner = tokio::time::interval(self.state.indicator.spinner_period());
                        ctx.spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        // Pi re-titles the window from the newly bound session (`bindSession` →
                        // `updateTerminalTitle`, interactive-mode.ts:1761): a `/new`, `/resume` or
                        // `/fork` almost always changes the name, and a swap must never leave the
                        // previous session's name in the tab. The cwd is the runtime's factory base
                        // and does not move with the swap, so only the name is re-read.
                        self.state.status.set_session_name(ctx.session.session_name().await);
                        if let Some(title) = self.update_terminal_title() {
                            write_terminal_title(&title);
                        }
                        // The replacement session brings its own `AuthStore` (a `/resume` of a
                        // session recorded under a different agent dir reads a different
                        // `auth.json`), so the cached snapshot the ` (sub)` marker answers from is
                        // re-read here for the same reason the ui sinks are re-installed.
                        self.refresh_auth_snapshot(&ctx.session).await;
                        // …and the context segment, which is a property of the NEW branch's entries
                        // and its model's window (`footer.ts:108-111`).
                        self.refresh_context_usage(&ctx.session).await;
                        // The swapped-in session owns a fresh `LiveHostServices`; re-install the ui
                        // sink so a post-swap guest dialog still reaches this loop (L4 review §2.1,
                        // same re-install this run loop's `AppAction::Command` rebind mirrors from
                        // `crates/cyrup-modes/src/rpc.rs`'s `run_rpc`).
                        Self::install_ui_sinks(
                            &ctx.session.services().host_services,
                            ctx.ui_tx.clone(),
                            ctx.ui_effect_tx.clone(),
                        );
                        Self::install_overlay_sink(
                            &ctx.session.services().host_services,
                            ctx.overlay_tx.clone(),
                        );
                        // ...and the two read-back seams (SEAM-T01/T02). The theme half additionally
                        // has to be REBUILT rather than merely re-attached: it answers
                        // `getAllThemes`/`getTheme` out of the session's resource snapshot, and a
                        // swap (`/reload` above all) is exactly when a newly discovered theme
                        // appears — pi re-runs `setRegisteredThemes(resourceLoader.getThemes())` on
                        // the same events (`interactive-mode.ts:1910`, `:5787`).
                        self.install_extension_readbacks(
                            &ctx.session.services().host_services,
                            Arc::clone(&ctx.session.services().resources),
                            ctx.theme_switch_tx.clone(),
                        );
                        // ...and the fault listener, whose `ExtensionHost` is likewise brand new on
                        // the swapped-in session (Pi re-binds `onError` from `rebindSession`, and
                        // `crates/cyrup-modes/src/rpc.rs`'s `rebind_session` does the same).
                        Self::install_error_listener(
                            &ctx.session.services().ext_host,
                            ctx.ext_error_tx.clone(),
                        );
                        // ...and the same for the `/` menu: a replacement session can load a
                        // DIFFERENT extension set (`/reload` exists precisely to change it), so a
                        // registry built from the previous session's catalog would be stale.
                        self.state.editor.set_registry(
                            crate::commands::CommandRegistry::with_dynamic(
                                crate::commands::dynamic_commands_from_catalog_gated(
                                    &ctx.session.slash_command_catalog(),
                                    ctx.session
                                        .services()
                                        .settings
                                        .effective()
                                        .enable_skill_commands(),
                                ),
                            ),
                        );
                        // `rebind_session` reset the transcript to Pi's default pad; re-read the
                        // swapped-in session's `outputPad` so a configured value survives the swap.
                        self.state.transcript.set_output_pad(
                            ctx.session.services().settings.effective().output_pad().max(0) as usize,
                        );
                        self.state.transcript.set_hide_thinking_block(
                            ctx.session.services().settings.effective().hide_thinking_block(),
                        );
                        let eff = ctx.session.services().settings.effective();
                        self.state.show_images = eff.show_images();
                        self.state.transcript.set_show_images(self.state.show_images);
                        // Re-read the progress gate for the swapped-in session's settings, for the
                        // same reason as the image rows beside it. Any indicator the OUTGOING
                        // session lit is dropped with its state; the swap arrives between turns.
                        self.state.terminal_progress =
                            crate::TerminalProgress::with_enabled(eff.show_terminal_progress());
                        self.state.transcript.set_image_width_cells(
                            eff.image_width_cells().clamp(1, u16::MAX as i64) as u16,
                        );
                        // `editorPaddingX` / `showHardwareCursor` are per-settings-layer, and a swap
                        // can move the project scope (`/resume` of a session recorded elsewhere), so
                        // re-apply both — Pi does exactly this from `rebindSession`
                        // (`interactive-mode.ts:1721-1732`: `ui.setShowHardwareCursor(...)` then
                        // `defaultEditor.setPaddingX(getEditorPaddingX())`).
                        self.state.editor.set_padding_x(eff.editor_padding_x());
                        self.state.editor.set_show_hardware_cursor(
                            eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::from_process()),
                        );
                        // TUI-009 — same liveness as the rows above: a swap can move the settings
                        // scope, so re-read `doubleEscapeAction` for the swapped-in session.
                        self.state.double_escape_action = eff.double_escape_action();
        // TUI-032 — the `Warnings` submenu is built from this cache.
        self.state.warn_anthropic_extra_usage =
            eff.warnings().anthropic_extra_usage.unwrap_or(true);
                                        // TUI-003: seed the view from the swapped-in session's conversation (Pi
                        // re-runs `renderInitialMessages()` after a tree/fork navigation,
                        // interactive-mode.ts:1737-1742). Without this a `/resume`, `/fork` or
                        // `/import` leaves the user staring at an empty transcript while the
                        // session file holds the whole history. `raw_context_messages` (NOT
                        // `messages()`) is Pi's `buildContextEntries()` projection: roles intact,
                        // so a compaction/branch summary, a `custom` message and a `!` run each
                        // reach their own component instead of replaying as user prose.
                        let restored = ctx.session.raw_context_messages().await;
                        if !restored.is_empty() {
                            // X11 — with extensions: the swapped-in session brings its own host,
                            // and Pi resolves `getMessageRenderer` on the replay walk too
                            // (`interactive-mode.ts:3471`).
                            let ext_host = ctx.session.services().ext_host.clone();
                            self.replay_session_with_extensions(&restored, &ext_host).await;
                        }
                        // TUI-N04 — the same statement `renderInitialMessages()` runs after its
                        // replay (`interactive-mode.ts:3485`), and it must run here too: a
                        // `/resume` of a session recorded in a DIFFERENT project swaps the cwd and
                        // the trust decision with it, so the banner's answer changes on the swap.
                        self.render_project_trust_warning_if_needed(&ctx.session);
                        // The swapped-in session owns a fresh extension host; re-source its
                        // registered shortcuts (R-08-017) so a post-swap press still routes.
                        // EXT-040: `shortcut_specs()` carries the description `/hotkeys` renders;
                        // `shortcut_keys()` drops it.
                        let shortcuts = ctx.session.services().ext_host.shortcut_specs();
                        self.state.set_extension_shortcuts(shortcuts);
                        self.draw_synchronized()?;
                    }
                }
            }
            if self.state.should_quit {
                break;
            }
        }
        // pi `interactive-mode.ts:3589-3591`: drain, THEN stop. The drain MUST happen here, before
        // this function's own restore, not at the caller — `run` disables raw mode on the way out,
        // so a drain after it returns is a guaranteed no-op on the exact path it exists for, and
        // whatever is still queued (a late Kitty key-release report, or the Ctrl+D that asked for
        // this quit) has already been handed to the parent shell.
        self.drain_and_restore()
    }
}
