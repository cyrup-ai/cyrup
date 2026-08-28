use super::*;

use super::run::RunCtx;

/// How an arm handler leaves the run loop: continue polling, break the loop, or return `Ok(())`: continue polling, break the loop, or return `Ok(())`
/// from [`App::run`] outright (the two control-flow exits the inline arm bodies used to take).
pub(crate) enum RunFlow {
    Continue,
    Break,
    ReturnOk,
}

impl App<InlineBackend<Stdout>> {
    /// The boot-time UI seed (§7.2): the `/` menu registry, every persisted setting's
    /// first-frame application, the window title, the auth/context snapshots and the
    /// project-trust banner — moved verbatim from `App::run`'s setup. The session-swap
    /// arm re-reads the same settings inline (it is the loop's control flow and stays
    /// in `run.rs`).
    pub(crate) async fn seed_session_ui(
        &mut self,
        session: &Arc<AgentSession>,
        runtime: Option<&Arc<AgentSessionRuntime>>,
    ) {
        // The `/` menu's dynamic half (pi `interactive-mode.ts:1240-1300`). `slash_command_catalog()`
        // already merges registered extension commands, prompt templates and skills — it was just
        // never consumed outside RPC mode, so the interactive `/` list showed builtins only while an
        // RPC client saw everything from the SAME session. Re-installed on session swap below, for
        // the same reason the sinks are: a replacement session brings different extensions.
        // …gated by `enableSkillCommands`, which Pi applies at exactly this seam
        // (`interactive-mode.ts:613`) and nowhere else.
        let gate = session.services().settings.effective().enable_skill_commands();
        self.rebuild_command_registry(session, gate);
        // ...and the `/model` / `/login` argument completers, from the same seed point, so the very
        // first `/model ` of the session already completes.
        self.refresh_argument_sources(session);
        // `editorPaddingX` + `showHardwareCursor` — Pi seeds both while CONSTRUCTING the editor and
        // the TUI (`interactive-mode.ts:459` `new TUI(terminal, getShowHardwareCursor(), …)` and
        // `:470-474` `new CustomEditor(…, { paddingX: getEditorPaddingX(), … })`), so the very first
        // frame must already honour them. Re-applied on `/settings` cycle and on session swap below.
        {
            let eff = session.services().settings.effective();
            self.state.editor.set_padding_x(eff.editor_padding_x());
            self.state
                .editor
                .set_show_hardware_cursor(
                    eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::from_process()),
                );
        }
        // Honor the persisted `outputPad` at boot (Pi seeds `this.outputPad = getOutputPad()`,
        // interactive-mode.ts:440): the transcript defaults to Pi's `1`, but a configured `0` must take
        // effect on the first frame. Re-read after each session swap below (a swap resets the transcript).
        self.state
            .transcript
            .set_output_pad(session.services().settings.effective().output_pad().max(0) as usize);
        // Same for `hideThinkingBlock` (Pi seeds `this.hideThinkingBlock = getHideThinkingBlock()`
        // before constructing any `AssistantMessageComponent`): the very first reasoning block must
        // already honour the persisted setting.
        self.state
            .transcript
            .set_hide_thinking_block(session.services().settings.effective().hide_thinking_block());
        // `terminal.showImages` / `terminal.imageWidthCells` govern how a tool result's `image`
        // content blocks render (TUI-007) — seed both before the first frame.
        let eff = session.services().settings.effective();
        self.state.show_images = eff.show_images();
        self.state.transcript.set_show_images(self.state.show_images);
        self.state
            .transcript
            .set_image_width_cells(eff.image_width_cells().clamp(1, u16::MAX as i64) as u16);
        // `markdown.mermaid` — Pi seeds the transformer's `getMode()` closure when it registers the
        // built-in transformer at construction (`interactive-mode.ts:484-486`), so the very first
        // frame already honours the persisted value.
        self.state.transcript.set_mermaid_mode(eff.mermaid_rendering_mode());
        // TUI-009 — `doubleEscapeAction` had no consumer at all; the Escape chain reads it out of
        // `AppState` because `apply_action` has no session in hand.
        self.state.double_escape_action = eff.double_escape_action();
        // TUI-032 — the `Warnings` submenu is built from this cache.
        self.state.warn_anthropic_extra_usage =
            eff.warnings().anthropic_extra_usage.unwrap_or(true);
        // `terminal.showTerminalProgress` — the gate on the OSC 9;4 taskbar indicator. Pi re-reads
        // it at each of its five call sites (`interactive-mode.ts:2865`/`:3057`/`:3076`/`:3090`/
        // `:6041`); cyrup caches it here and re-seeds it on a `/settings` flip and on a session swap,
        // which is the same liveness. Seeding only — never arms, since Pi arms only from an
        // `agent_start`/`compaction_start`.
        self.state.terminal_progress = crate::TerminalProgress::with_enabled(
            eff.show_terminal_progress(),
        );
        // The automatic window title (Pi `updateTerminalTitle`, interactive-mode.ts:818-826, called
        // at `:860` right after `init()`): `cyrup - <session name> - <cwd basename>`. Both inputs are
        // read from the LIVE session here — the name Pi reads via `sessionManager.getSessionName()`
        // and the cwd via `getCwd()` (the runtime's, which a `/resume` of a session recorded
        // elsewhere moves; the process cwd is the fallback seeded in `AppState::new`). Refreshed on
        // `session_info_changed` (`ingest_event`) and on every session swap (the `session_swapped`
        // arm below), which is exactly Pi's `:2901` / `:1761` call sites.
        // X7(b) — through [`Self::set_title_cwd`], NOT a bare `state.title_cwd = …`. That funnel is
        // what also lands the value on the transcript as Pi's `ToolRenderContext.cwd`
        // (`tool-execution.ts:126`), which `read`'s compact classification resolves its path against
        // (`read.ts:336`, `resolveToCwd(rawPath, cwd)`). Assigning the field directly left
        // `transcript.cwd()` at `None`, so the classification silently fell back to the PROCESS cwd
        // — which is exactly what the paragraph above says can differ after a `/resume`.
        if let Some(rt) = runtime.as_ref() {
            self.set_title_cwd(rt.cwd().to_path_buf());
        }
        self.state.status.set_session_name(session.session_name().await);
        if let Some(title) = self.update_terminal_title() {
            write_terminal_title(&title);
        }
        // The footer's ` (sub)` marker (`footer.ts:138-145`). pi answers it per repaint from
        // `modelRuntime.snapshot.auth`, which the runtime has already loaded by the time the first
        // frame draws; cyrup reads `auth.json` once, here, so the very FIRST frame of a session
        // started with a stored Pro/Max credential already shows the marker. Refreshed again on
        // every credential change (`finish_login`, the `/logout` arm) and on session swap below.
        self.refresh_auth_snapshot(session).await;
        // …and, for the same reason, the context segment: upstream's `render()` reads
        // `getContextUsage()` per frame (`footer.ts:108`), so a `/resume`d session shows its
        // occupancy on the very first frame rather than only after the next assistant message.
        self.refresh_context_usage(session).await;
        // TUI-N04 — Pi's `renderInitialMessages()` runs `renderProjectTrustWarningIfNeeded()`
        // straight after the replay (`interactive-mode.ts:3479-3485`). cyrup's replay is the
        // caller's (`crates/cyrup/src/main.rs` for a `--resume`/`--continue` boot, the
        // `session_swapped` arm below for a `/resume`/`/fork`/`/import`), so the check lands HERE
        // rather than inside `replay_session_*`: pi's call is UNconditional, while cyrup's replay is
        // skipped entirely when `raw_context_messages()` is empty — and a fresh session in an
        // untrusted project is precisely the case that most needs the banner.
        self.render_project_trust_warning_if_needed(session);
    }

    /// Bind the UI to the runtime's currently-installed session — pi's awaited `rebindSession`
    /// (`agent-session-runtime.ts:187-193`, registered at `interactive-mode.ts:536-538`). Called
    /// from the `session_swapped` arm AND from the input arm's pre-dispatch reconcile
    /// (`rpc.rs:836-844`'s own rationale), so the loop can never act through a disposed session.
    ///
    /// **[CYRUP-DELTA]** vs `agent-session-runtime.ts:187-193`: pi's `finishSessionReplacement`
    /// AWAITS a host callback the interactive mode registers
    /// (`interactive-mode.ts:536-538`) — a replacement is not COMPLETE until the host has rebound.
    /// A Rust `Arc<dyn Fn()>` cannot capture `&mut App`, so cyrup RECONCILES instead of awaiting: this
    /// helper is invoked both when the generation watch fires and, defensively, right before the
    /// next key is serviced, so a stale rebind can never be acted on for longer than one `select!`
    /// wakeup.
    pub(crate) async fn on_session_swapped(
        &mut self,
        ctx: &mut RunCtx,
        events: &mut EventStream<AgentSessionEvent>,
    ) -> Result<(), TuiError> {
        let _arm = ArmGuard::enter("session_swapped");
        let Some(rt) = ctx.runtime.clone() else { return Ok(()) };
        // Whichever path got here (the generation-watch arm firing directly, or the input arm's
        // reconcile noticing `has_changed()`), this rebind answers for every bump observed so far
        // — there is nothing left for a redundant firing of the OTHER caller to do.
        if let Some(rx) = ctx.gen_rx.as_mut() {
            rx.mark_unchanged();
        }
        // Captured before the awaits below: pi's re-entrancy guard
        // (`if (this.session !== session) return;`, `interactive-mode.ts:1977-1979`) compares the
        // generation this rebind answers for against whatever the runtime holds once the awaits
        // settle — if a NEWER session landed while we were awaiting, abandon this rebind rather than
        // paint a superseded one; the `session_swapped` arm fires again for the newer generation.
        let bound_gen = rt.generation().await;
        let new_session = rt.session().await;
        *events = new_session.subscribe();
        ctx.session = new_session;
        self.rebind_session();
        // TUI-030 — `rebind_session` ran `reset_extension_ui`, whose
        // `reset_extension_working_state` drops the outgoing extension's `setWorkingIndicator`
        // options (pi's `this.setWorkingIndicator()` with no argument, `interactive-mode.ts:2212`
        // @v0.84.2). Upstream that call re-arms `Loader`'s own `setInterval` back to
        // `DEFAULT_INTERVAL_MS` (`loader.ts:67-69` → `:77-80`); cyrup has no per-indicator timer —
        // the run loop's single tick IS the animation clock — so the period has to be re-read here,
        // exactly as the `SetWorkingIndicator` effect arm re-reads it. Without this a guest that
        // asked for `intervalMs: 1000` would leave the NEXT session's built-in Braille spinner
        // sampled once a second.
        ctx.spinner = tokio::time::interval(self.state.indicator.spinner_period());
        ctx.spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Pi re-titles the window from the newly bound session (`bindSession` →
        // `updateTerminalTitle`, interactive-mode.ts:1761): a `/new`, `/resume` or `/fork` almost
        // always changes the name, and a swap must never leave the previous session's name in the
        // tab. The cwd is the runtime's factory base and does not move with the swap, so only the
        // name is re-read.
        self.state.status.set_session_name(ctx.session.session_name().await);
        if let Some(title) = self.update_terminal_title() {
            write_terminal_title(&title);
        }
        // The replacement session brings its own `AuthStore` (a `/resume` of a session recorded
        // under a different agent dir reads a different `auth.json`), so the cached snapshot the
        // ` (sub)` marker answers from is re-read here for the same reason the ui sinks are
        // re-installed.
        self.refresh_auth_snapshot(&ctx.session).await;
        // …and the context segment, which is a property of the NEW branch's entries and its model's
        // window (`footer.ts:108-111`).
        self.refresh_context_usage(&ctx.session).await;
        // The swapped-in session owns a fresh `LiveHostServices`; re-install the ui sink so a
        // post-swap guest dialog still reaches this loop (L4 review §2.1, same re-install this run
        // loop's `AppAction::Command` rebind mirrors from `crates/cyrup-modes/src/rpc.rs`'s
        // `run_rpc`).
        Self::install_ui_sinks(
            &ctx.session.services().host_services,
            ctx.ui_tx.clone(),
            ctx.ui_effect_tx.clone(),
        );
        Self::install_overlay_sink(&ctx.session.services().host_services, ctx.overlay_tx.clone());
        // ...and the two read-back seams (SEAM-T01/T02). The theme half additionally has to be
        // REBUILT rather than merely re-attached: it answers `getAllThemes`/`getTheme` out of the
        // session's resource snapshot, and a swap (`/reload` above all) is exactly when a newly
        // discovered theme appears — pi re-runs `setRegisteredThemes(resourceLoader.getThemes())`
        // on the same events (`interactive-mode.ts:1910`, `:5787`).
        self.install_extension_readbacks(
            &ctx.session.services().host_services,
            Arc::clone(&ctx.session.services().resources),
            ctx.theme_switch_tx.clone(),
        );
        // ...and the fault listener, whose `ExtensionHost` is likewise brand new on the swapped-in
        // session (Pi re-binds `onError` from `rebindSession`, and `crates/cyrup-modes/src/rpc.rs`'s
        // `rebind_session` does the same).
        Self::install_error_listener(&ctx.session.services().ext_host, ctx.ext_error_tx.clone());
        // ...and HA-1's command listener, for the same reason: the old host's subscribers do not
        // reach the new one, so a late command on the replacement session would never rebuild.
        Self::install_commands_listener(
            &ctx.session.services().ext_host,
            ctx.commands_changed_tx.clone(),
        );
        // ...and the same for the `/` menu: a replacement session can load a DIFFERENT extension set
        // (`/reload` exists precisely to change it), so a registry built from the previous session's
        // catalog would be stale.
        let gate = ctx.session.services().settings.effective().enable_skill_commands();
        let swapped = Arc::clone(&ctx.session);
        self.rebuild_command_registry(&swapped, gate);
        // ...and the argument completers, for the same reason: a replacement session brings its own
        // model catalog and scoped set.
        self.refresh_argument_sources(&swapped);
        // `rebind_session` reset the transcript to Pi's default pad; re-read the swapped-in
        // session's `outputPad` so a configured value survives the swap.
        self.state
            .transcript
            .set_output_pad(ctx.session.services().settings.effective().output_pad().max(0) as usize);
        self.state.transcript.set_hide_thinking_block(
            ctx.session.services().settings.effective().hide_thinking_block(),
        );
        let eff = ctx.session.services().settings.effective();
        self.state.show_images = eff.show_images();
        self.state.transcript.set_show_images(self.state.show_images);
        // Re-read the progress gate for the swapped-in session's settings, for the same reason as
        // the image rows beside it. Any indicator the OUTGOING session lit is dropped with its
        // state; the swap arrives between turns.
        self.state.terminal_progress =
            crate::TerminalProgress::with_enabled(eff.show_terminal_progress());
        self.state
            .transcript
            .set_image_width_cells(eff.image_width_cells().clamp(1, u16::MAX as i64) as u16);
        // `rebind_session` reset the transcript to the derived default (`streaming`); re-read the
        // swapped-in session's `markdown.mermaid` so a configured value survives the swap.
        self.state.transcript.set_mermaid_mode(eff.mermaid_rendering_mode());
        // `editorPaddingX` / `showHardwareCursor` are per-settings-layer, and a swap can move the
        // project scope (`/resume` of a session recorded elsewhere), so re-apply both — Pi does
        // exactly this from `rebindSession` (`interactive-mode.ts:1721-1732`:
        // `ui.setShowHardwareCursor(...)` then `defaultEditor.setPaddingX(getEditorPaddingX())`).
        self.state.editor.set_padding_x(eff.editor_padding_x());
        self.state.editor.set_show_hardware_cursor(
            eff.show_hardware_cursor(&cyrup_session_svc::EnvVars::from_process()),
        );
        // TUI-009 — same liveness as the rows above: a swap can move the settings scope, so re-read
        // `doubleEscapeAction` for the swapped-in session.
        self.state.double_escape_action = eff.double_escape_action();
        // TUI-032 — the `Warnings` submenu is built from this cache.
        self.state.warn_anthropic_extra_usage = eff.warnings().anthropic_extra_usage.unwrap_or(true);
        // TUI-003: seed the view from the swapped-in session's conversation (Pi re-runs
        // `renderInitialMessages()` after a tree/fork navigation, interactive-mode.ts:1737-1742).
        // Without this a `/resume`, `/fork` or `/import` leaves the user staring at an empty
        // transcript while the session file holds the whole history. `raw_context_messages` (NOT
        // `messages()`) is Pi's `buildContextEntries()` projection: roles intact, so a
        // compaction/branch summary, a `custom` message and a `!` run each reach their own component
        // instead of replaying as user prose.
        let restored = ctx.session.raw_context_messages().await;
        if !restored.is_empty() {
            // X11 — with extensions: the swapped-in session brings its own host, and Pi resolves
            // `getMessageRenderer` on the replay walk too (`interactive-mode.ts:3471`).
            let ext_host = ctx.session.services().ext_host.clone();
            self.replay_session_with_extensions(&restored, &ext_host).await;
        }
        // TUI-N04 — the same statement `renderInitialMessages()` runs after its replay
        // (`interactive-mode.ts:3485`), and it must run here too: a `/resume` of a session recorded
        // in a DIFFERENT project swaps the cwd and the trust decision with it, so the banner's
        // answer changes on the swap.
        self.render_project_trust_warning_if_needed(&ctx.session);
        // The swapped-in session owns a fresh extension host; re-source its registered shortcuts
        // (R-08-017) so a post-swap press still routes. EXT-040: `shortcut_specs()` carries the
        // description `/hotkeys` renders; `shortcut_keys()` drops it.
        let shortcuts = ctx.session.services().ext_host.shortcut_specs();
        self.state.set_extension_shortcuts(shortcuts);

        // pi's re-entrancy guard, `interactive-mode.ts:1977-1979`
        // (`if (this.session !== session) return;`): a newer session landed while we awaited above,
        // so abandon this rebind without painting it — the `session_swapped` arm will fire again for
        // the newer generation and repaint from ITS state.
        if rt.generation().await != bound_gen {
            return Ok(());
        }
        self.draw_synchronized()?;
        Ok(())
    }

    /// The loop-head budget diagnostic (TUI-092): surface an arm that blew [`ARM_BUDGET`]
    /// on the previous iteration, drained on the first healthy iteration after it.
    pub(crate) fn drain_over_budget_arm(&mut self) {
            // TUI-092 — surface an arm that blew [`ARM_BUDGET`] on the previous iteration. Recorded
            // by [`ArmGuard`]'s `Drop` (which cannot draw: it runs inside the arm, on a raw-mode
            // terminal the frame owns) and drained HERE, on the first healthy iteration after it,
            // so the diagnostic reaches the user as an ordinary transcript line. `push_warning`
            // queues into `TranscriptView::pending`; every arm below ends in `draw_synchronized`,
            // which paints it.
            if let Ok(mut over) = OVER_BUDGET_ARM.lock()
                && let Some(arm) = over.take()
            {
                self.state.transcript.push_warning(format!(
                    "Warning: run-loop arm `{arm}` exceeded its {ARM_BUDGET:?} budget"
                ));
            }
    }

    pub(crate) fn on_spinner_tick(&mut self) -> Result<(), TuiError> {
                    // The live `!` block's glyph and a running bash tool's `Elapsed` footer are
                    // re-derived from `Instant::now()` inside `lines()` — invalidate so this tick
                    // re-materialises them (once, not 3×). Quiet turns hit the cache and stay free.
                    if self.state.transcript.bash_running()
                        || self.state.transcript.has_running_elapsed_tool()
                    {
                        self.state.transcript.bump_render_tick();
                    }
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_dialog_countdown_tick(&mut self) -> Result<(), TuiError> {
                    self.tick_extension_dialog_countdown();
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_elapsed_tick(&mut self) -> Result<(), TuiError> {
                    // Pi's `context.invalidate()` → `ui.requestRender()`: the `Elapsed` figure is
                    // computed from `started_at` inside `lines()`, so the render cache must be
                    // invalidated for the repaint to show a fresh value.
                    self.state.transcript.bump_render_tick();
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_git_branch_poll(&mut self) -> Result<(), TuiError> {
                    // Pi repaints only when the branch actually CHANGED (`notifyBranchChange` fires
                    // inside `if (this.cachedBranch !== nextBranch)`); an unchanged `stat` draws
                    // nothing.
                    if self.poll_footer_git_branch() {
                        self.draw_synchronized()?;
                    }
        Ok(())
    }

    pub(crate) fn on_bash_msg(&mut self, ctx: &mut RunCtx, msg: BashMsg) -> Result<(), TuiError> {
                    // TUI-092 F3 — drain every queued BashMsg BEFORE drawing: a chatty `!` run
                    // otherwise pays one full frame per output chunk. `try_recv`, not
                    // `now_or_never`: `bash_rx` is the concrete `UnboundedReceiver` in scope, so
                    // the drain is synchronous and constructs no future.
                    let mut pending = std::collections::VecDeque::from([msg]);
                    if let Some(rx) = ctx.bash_rx.as_mut() {
                        while let Ok(msg) = rx.try_recv() {
                            pending.push_back(msg);
                        }
                    }
                    while let Some(msg) = pending.pop_front() {
                        match msg {
                            BashMsg::Chunk(chunk) => self.state.transcript.bash_append(&chunk),
                            BashMsg::Done { exit_code, cancelled, truncated, full_output_path } => {
                                // X13 — Pi's completion arm verbatim (`interactive-mode.ts:6347-6353`):
                                //   this.bashComponent.setComplete(
                                //       result.exitCode, result.cancelled,
                                //       result.truncated ? {truncated:true, content:result.output} : undefined,
                                //       result.fullOutputPath);
                                // All FOUR fields, so `Output truncated. Full output: …`
                                // (`bash-execution.ts:195-199`) is reachable in a LIVE session and not
                                // only on replay. Recording into the session is NOT done here: it is
                                // `executeBash`'s own `recordBashResult` (agent-session.ts:2628-2643),
                                // which `AgentSession::execute_bash` already performs — with the
                                // `truncated`/`fullOutputPath` fields intact, which is what puts the
                                // warning back on the block after a `/resume`.
                                self.state.transcript.bash_complete(
                                    exit_code,
                                    cancelled,
                                    truncated,
                                    full_output_path,
                                );
                                self.state.transcript.commit_bash();
                                ctx.bash_rx = None;
                                // `Done` is terminal — the producer sends it last and drops its
                                // sender, so nothing legitimately follows it in the queue.
                                break;
                            }
                        }
                    }
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_overlay_ticked(&mut self) -> Result<(), TuiError> {
                    // Pi's `setInterval(() => { this.invalidate(); this.tui.requestRender(); })`
                    // (`fleet.ts:516-520`): let the component re-collect, and repaint only when it
                    // says the frame actually changed — the same "no-op edge costs no draw" rule the
                    // git-branch poll arm above follows.
                    let mut changed = false;
                    for overlay in self.state.overlays.iter_mut() {
                        changed |= overlay.tick();
                    }
                    if changed {
                        self.draw_synchronized()?;
                    }
        Ok(())
    }

    pub(crate) fn on_overlay_request(&mut self, ctx: &mut RunCtx, req: cyrup_session_svc::OverlayRequest) -> Result<(), TuiError> {
                    // An extension handed over an interactive modal (Pi `ctx.ui.custom(factory,
                    // { overlay: true, … })`). Its calling task is BLOCKED on the one-shot inside
                    // `req.done` until the `ExtensionOverlay` we build here is dropped, which
                    // happens on `Close` (`handle_overlay_key` pops it), on a session swap
                    // (`rebind_session` clears the stack) or on quit.
                    let cyrup_session_svc::OverlayRequest { overlay, done } = req;
                    let adapter = ExtensionOverlay::new(overlay, done);
                    // Arm the shared tick at THIS overlay's cadence before pushing, so the very
                    // first refresh lands one interval from now rather than immediately.
                    let refresh_ms = Overlay::refresh_ms(&adapter);
                    ctx.overlay_tick = (refresh_ms > 0).then(|| {
                        let period = std::time::Duration::from_millis(refresh_ms);
                        // `interval_at`, not `interval`: the latter's first tick resolves
                        // IMMEDIATELY, which would re-collect and repaint the frame we are about to
                        // draw below for no reason.
                        let mut interval =
                            tokio::time::interval_at(tokio::time::Instant::now() + period, period);
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                        interval
                    });
                    self.state.overlays.push(Box::new(adapter));
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_ui_request(&mut self, req: UiRequest) -> Result<(), TuiError> {
                    // A loaded guest opened a `ui.*` dialog (L4 review §2.1). EVERY kind, including
                    // `editor` (L4 review §3 — an INLINE dialog is now the default, matching Pi's
                    // `ExtensionEditorComponent`; `$VISUAL`/`$EDITOR` is reachable only via the
                    // dialog's own `Ctrl+G`, `AppAction::OpenExternalEditorForSelector`, above), opens
                    // the matching input-slot selector via `open_extension_dialog` and waits for a
                    // future key event to confirm/cancel it (`AppState::pending_ui_reply`).
                    self.open_extension_dialog(req);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_ui_effect(&mut self, ctx: &mut RunCtx, effect: UiEffect) -> Result<(), TuiError> {
                    // The fire-and-forget counterpart of the `ui_rx` arm above: a loaded guest pushed
                    // a `ui.*` mutator and did NOT block on a reply, so there is nothing to answer —
                    // just apply it and redraw (Pi's mutators end in `this.ui.requestRender()`).
                    if let UiEffect::SetTitle { title } = &effect {
                        // Pi `setTitle` reaches the terminal, not a component
                        // (`interactive-mode.ts:2238` → `terminal.ts:504-507`), so it is written here
                        // on the crossterm path rather than inside the backend-generic
                        // `apply_ui_effect`.
                        write_terminal_title(title);
                    }
                    let reframe = matches!(effect, UiEffect::SetWorkingIndicator { .. });
                    self.apply_ui_effect(effect);
                    if reframe {
                        // TUI-030 — pi's `Loader.setIndicator` re-arms its `setInterval` with the
                        // extension's `intervalMs` (`loader.ts:69` → `:77-80` @v0.84.2). cyrup has
                        // no timer per indicator: the run loop's single tick IS the animation
                        // clock, so the
                        // new period has to replace it here, next to the `SetTitle` write above and
                        // for the same reason — it is run-loop state `apply_ui_effect` cannot reach.
                        // Without this a `frames`-heavy indicator with a 40 ms `intervalMs` would
                        // still only be sampled every 80 ms.
                        ctx.spinner = tokio::time::interval(self.state.indicator.spinner_period());
                        ctx.spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                    }
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_ext_error(&mut self, err: cyrup_ext::ExtensionError) -> Result<(), TuiError> {
                    // A guest handler faulted and the dispatcher CONTAINED it (R-08-036). Pi shows
                    // it: `onError: (error) => this.showExtensionError(...)`
                    // (`interactive-mode.ts:1700-1701`). Without this arm the fault reached only
                    // `tracing`, so a broken extension silently ate its hook — or silently denied a
                    // tool — with nothing on screen (TUI-S02).
                    self.show_extension_error(&err);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_shortcut_status(&mut self, msg: String) -> Result<(), TuiError> {
                    self.state.transcript.push_status(msg);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_compact_outcome(&mut self, outcome: CompactOutcome) -> Result<(), TuiError> {
                    // A spawned `/compact` settled (TUI-055). The band was cleared by the
                    // `compaction_end` event that preceded this message on the `events` stream.
                    self.apply_compact_outcome(outcome);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_lifecycle_outcome(&mut self, outcome: LifecycleOutcome) -> Result<(), TuiError> {
                    // A spawned `/new`, `/reload`, `/import`, `/resume` or `/fork` settled
                    // (TUI-092 §5b.2). On success the generation-watch arm has usually already
                    // re-bound the UI off the runtime's bump, captioned by the optimistic
                    // `pending_swap_status`; this arm carries the residue that needs `&mut self`
                    // (the `/fork` editor re-seed, the `/reload` keybinding rebuild) and clears that
                    // caption if the op turned out to have failed.
                    self.apply_lifecycle_outcome(outcome);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_queue_drain(&mut self, ctx: &mut RunCtx, drained: QueueDrain) -> Result<(), TuiError> {
                    // A spawned take-all settled (TUI-092 §5b.1). Everything that used to follow
                    // `drain_queue().await` inline happens here, in the same order: the
                    // `clearAllQueues` interleave, the editor restore, then the abort.
                    self.apply_queue_drain(drained, &ctx.session);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_package_updates(&mut self, ctx: &mut RunCtx, maybe_updates: Option<Vec<String>>) -> Result<(), TuiError> {
                    // Pi `:851-855` — `if (updates.length > 0) this.showPackageUpdateNotification(updates)`.
                    // The producer only ever sends a non-empty list and then drops its sender, so the
                    // receiver is retired here and the arm goes permanently pending: exactly one
                    // notification per session, as upstream's single `.then()` gives.
                    ctx.package_update_rx = None;
                    if let Some(packages) = maybe_updates {
                        self.state.transcript.push_package_updates(&packages);
                        self.draw_synchronized()?;
                    }
        Ok(())
    }

    pub(crate) fn on_tmux_warning(&mut self, warning: &'static str) -> Result<(), TuiError> {
                    // Pi `:866-868` — `showWarning`, whose copy is `Warning: {message}`
                    // (`interactive-mode.ts:3885-3889`), the same framing the extension `notify`
                    // path uses in `apply_ui_effect`.
                    self.state.transcript.push_warning(format!("Warning: {warning}"));
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) async fn on_theme_switch(&mut self, ctx: &mut RunCtx, theme: cyrup_resources::Theme) -> Result<(), TuiError> {
                    let _arm = ArmGuard::enter("theme_switch");
                    // SEAM-T01 — a guest called `ctx.ui().set_theme(name)` and the name RESOLVED
                    // (`TuiThemeAccess::set` rejected it otherwise, which is where pi's
                    // `{success: false, error}` comes from). Pi's handler does two things
                    // (`interactive-mode.ts:2406-2417` @v0.84.2): `themeController.setThemeName`,
                    // which repaints, and — guarded on the value actually differing —
                    // `settingsManager.setTheme(name)`, which persists. Both are done here, and both
                    // are the SAME pair the `/settings → theme` confirm arm runs
                    // (`SelectorKind::Theme` in `apply_selection`), so an extension switch and a
                    // human switch cannot drift apart.
                    let name = theme.key.as_str().to_string();
                    // `from_theme_data`, not `UiTheme::builtin`: the listing this name came from is
                    // the session's whole discovered set, so a file-backed custom theme is
                    // switchable exactly as upstream's is, and would otherwise silently render as
                    // `dark` (`UiTheme::builtin`'s unknown-name fallback).
                    let projected = UiTheme::from_theme_data(&theme.data, 0);
                    self.set_theme(projected);
                    // [CYRUP-DELTA] vs `interactive-mode.ts:2412`: upstream guards the persist with
                    // `if (this.settingsManager.getTheme() !== themeOrName)`. That guard is a pure
                    // write-avoidance — writing the same value yields the same file — and cyrup
                    // cannot evaluate it correctly here: the session's `SettingsManager` is a boot
                    // snapshot that `ApplySetting` does not refresh (its own arm says the effective
                    // view is re-read on `/reload`), so a stale read would SKIP a write that is
                    // genuinely needed after an earlier switch. Persisting unconditionally is what
                    // the human `/settings → theme` confirm arm already does, for the same reason.
                    self.execute_command(
                        AppCommand::ApplySetting { id: "theme".to_string(), value: name },
                        &ctx.session,
                        ctx.runtime.as_ref(),
                    )
                    .await;
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_login_msg(&mut self, msg: crate::login_dialog::LoginUiMsg) -> Result<(), TuiError> {
                    // The spawned `/login` flow wants something: a prompt rendered, progress shown,
                    // or the whole login settled (Pi's `prompt`/`notify` callbacks +
                    // the `try`/`catch` around `loginProvider`, `interactive-mode.ts:5367-5374`,
                    // `:5285-5296`). Answers travel back over the one-shot the message carried.
                    self.apply_login_msg(msg);
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) async fn on_tree_nav_msg(&mut self, ctx: &mut RunCtx, msg: TreeNavMsg) -> Result<(), TuiError> {
                    let _arm = ArmGuard::enter("tree_nav");
                    // A spawned `/tree` navigation settled (Pi `interactive-mode.ts:4805-4820`). An
                    // ABORTED summarization asks for the tree to be re-shown at the same entry, which
                    // needs the session (`session_dag`), so it comes back as a follow-up command.
                    if let Some(cmd) = self.apply_tree_nav_outcome(msg) {
                        self.execute_command(cmd, &ctx.session, ctx.runtime.as_ref()).await;
                    }
                    self.draw_synchronized()?;
        Ok(())
    }

    pub(crate) fn on_theme_changed(&mut self, ok: bool, theme_rx: &Option<tokio::sync::watch::Receiver<Arc<ThemeData>>>) -> Result<(), TuiError> {
                    if ok && let Some(rx) = theme_rx.as_ref() {
                        let data = rx.borrow().clone();
                        self.set_theme(UiTheme::from_theme_data(&data, 0));
                        self.draw_synchronized()?;
                    }
        Ok(())
    }
    /// The tmux keyboard-setup diagnostic (Pi `checkTmuxKeyboardSetup`, interactive-mode.ts:940-988,
    /// wired at `:865-869`). Spawned, never awaited: Pi starts it alongside the version/package
    /// checks and shows the warning whenever it settles, so a wedged `tmux show` (bounded at 2 s)
    /// delays no frame. The sender is kept alive by the run loop (in [`RunCtx`]) for the same reason
    /// `shortcut_status_tx` is: a closed channel would make its `select!` arm's `Some(..)` pattern
    /// fail on every iteration.
    pub(crate) fn spawn_tmux_keyboard_check()
        -> (
            tokio::sync::mpsc::UnboundedSender<&'static str>,
            tokio::sync::mpsc::UnboundedReceiver<&'static str>,
        )
    {
        let (tmux_warning_tx, tmux_warning_rx) =
            tokio::sync::mpsc::unbounded_channel::<&'static str>();
        let tx = tmux_warning_tx.clone();
        tokio::spawn(async move {
            if let Some(warning) = crate::tmux::check_keyboard_setup().await {
                let _ = tx.send(warning);
            }
        });
        (tmux_warning_tx, tmux_warning_rx)
    }

}
