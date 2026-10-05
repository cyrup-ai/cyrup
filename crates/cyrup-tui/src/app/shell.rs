use super::*;

/// How many times a colour re-query was asked for in this process — the seam a test reads to see
/// that a colour-scheme report made the app query the terminal (no real terminal answers there).
#[cfg(test)]
static REQUERIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// [`REQUERIES`], for the tests.
#[cfg(test)]
pub(crate) fn requery_count_for_test() -> usize {
    REQUERIES.load(std::sync::atomic::Ordering::SeqCst)
}

impl<B: Backend> App<B> {
    /// Build an app over `backend` using a **content-sized inline viewport** (R-ARCH-TUI-003,
    /// ADR-0001 #1): the live region holds only the active turn + status band + editor/selector +
    /// footer, so finished history flushes to native scrollback (`insert_before`) instead of the
    /// inline region swallowing the whole screen. No alternate screen is entered.
    pub fn new(backend: B, theme: UiTheme) -> Result<Self, TuiError> {
        let size = backend
            .size()
            .map_err(|e| TuiError::Backend(e.to_string()))?;
        let mut state = AppState::new(theme);
        // Seeded here as well as in `draw`, because a `--resume`/`--continue` boot replays its whole
        // conversation BEFORE the first frame, and the markdown-transform pass that replay ends with
        // reads this for `availableWidth` (see [`AppState::term_cols`]). Without the seed a resumed
        // session would transform its history at the 80-column default and a live one at the real
        // width.
        state.term_cols = size.width.max(1);
        let height = live_region_height(&mut state, size.width, size.height.max(1));
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(height.max(1)),
            },
        )
        .map_err(|e| TuiError::Backend(e.to_string()))?;
        // Seed `0` so the first `draw` always rebuilds the viewport bottom-anchored (the constructed
        // `Terminal` is top-anchored at the backend's initial cursor; the rebuild fixes the anchor).
        Ok(App {
            terminal,
            state,
            frames: crate::app::frames::FrameScheduler::default(),
            // ADR-0005 §B-14 — `None` IS regular mode. `App::new` is the inline renderer's
            // constructor and stays exactly that: the alternate screen is only ever installed by
            // `App::switch_tui_mode` (`app/mode_switch.rs`), so a session that never switches
            // behaves as it did before ADR-0005 in the strongest available sense.
            altscreen: None,
            alt_keymap: AltScreenKeymap::default(),
            // CFG-078 — pi's two documented defaults (`settings-manager.ts:143`, `:145` @v0.84.4),
            // so an `App` the composition root never configures behaves as upstream's does with an
            // empty `settings.json`.
            fullscreen_exit_output: crate::altscreen::FullscreenExitOutput::default(),
            fullscreen_copy_on_select: true,
            // `options.fullscreenWheelScrollLines ?? "auto"` (`tui-renderer.ts:38`).
            fullscreen_wheel_scroll_lines: cyrup_config::settings::WheelScrollLines::Auto,
            viewport_height: 0,
            live_floor: 0,
            tree_nav_tx: None,
            share_tx: None,
            package_update_rx: None,
            terminal_colors_rx: None,
            terminal_colors_tx: None,
            login_tx: None,
            login_refresh_tx: None,
            model_refresh_tx: None,
            session_list_tx: None,
            login_providers: None,
            radius_gateway: None,
            compact_tx: None,
            queue_drain_tx: None,
            lifecycle_tx: None,
            render_debug: render_debug::RenderDebug::default(),
            last_frame_size: render_debug::NO_FRAME,
            debug_previous_lines: Vec::new(),
        })
    }

    /// Restore the terminal: pop keyboard flags, disable bracketed paste, leave raw mode, show
    /// cursor. Total and idempotent so an error path always leaves a usable terminal.
    ///
    /// The escape sequence itself lives in [`crate::panic_hook::restore_terminal_best_effort`] and
    /// this method is a thin delegation to it, deliberately: the panic hook runs the *same*
    /// teardown, and two hand-maintained copies would silently drift the first time
    /// [`App::into_stdout`] learned to enable a fourth mode — a drift only ever discovered by a
    /// user whose terminal was already broken. Note the release profile sets `panic = "abort"`, so
    /// no `Drop` guard can stand in for the hook (`Cargo.toml:215`).
    ///
    /// Generic over the backend rather than confined to the crossterm one it is *used* from: nothing
    /// in it is crossterm-specific (the escapes go straight to stdout; `show_cursor` is a `Backend`
    /// method), and a `CrosstermBackend<Stdout>` cannot be constructed in a test without a
    /// controlling terminal — which would leave the pairing below with no way to assert itself.
    pub fn restore(&mut self) -> Result<(), TuiError> {
        crate::panic_hook::restore_terminal_best_effort();
        // Not a second `Show`-for-its-own-sake: ratatui's `Terminal` tracks `hidden_cursor` itself
        // and its `Drop` re-emits `Show` when that flag is still set, so the flag is synced through
        // the API rather than left stale by the raw-stdout write above.
        let _ = self.terminal.show_cursor();
        Ok(())
    }

    /// The **exit** teardown: drain stdin, then [`Self::restore`] — Pi's `shutdown()`, which runs
    /// `await this.ui.terminal.drainInput(1000)` immediately before `this.stop()`
    /// (`interactive-mode.ts:3578`/`:3589` then `:3591`, both the signal and the interactive-quit
    /// branch). `crates/cyrup/src/main.rs` calls it at the single exit from the interactive loop.
    ///
    /// This is a distinct method rather than a change to [`Self::restore`] because the drain is only
    /// correct on the way out. `restore` also runs on [`App::suspend`] (Ctrl+Z) and around the
    /// external editor, where the terminal is handed to someone else and taken back — anything the
    /// user types there is theirs to keep, and discarding it would be a new bug. Pi draws the line in
    /// exactly the same place: `handleCtrlZ` calls a bare `ui.stop()` (`:3722`) and never `drainInput`.
    ///
    /// See [`crate::drain`] for what the drain protects against (buffered Kitty key-release reports
    /// and the quit keystroke itself leaking to the parent shell once raw mode is off).
    pub fn drain_and_restore(&mut self) -> Result<(), TuiError> {
        // Pi's `stop()` clears the OSC 9;4 indicator first (`interactive-mode.ts:6041-6043`), before
        // `ui.stop()` tears the terminal down. Doing it here as well as inside
        // [`crate::panic_hook::restore_terminal_best_effort`] is Pi's own two-level structure: the
        // interactive mode clears its indicator, and `ProcessTerminal.stop()` clears whatever is
        // still armed. Both are idempotent; this one additionally drops the session's own armed bit
        // so the keepalive cannot re-arm on the way out.
        self.clear_terminal_progress_on_exit();
        let _ = crate::drain::drain_stdin_before_exit();
        self.restore()
    }

    /// Write the parked OSC 9;4 transition, if any — the second half of Pi's
    /// `ui.terminal.setProgress` (`tui/src/terminal.ts:509-523`).
    pub fn flush_terminal_progress(&mut self) {
        if let Some(active) = self.state.terminal_progress.take_pending() {
            crate::write_terminal_progress(active);
        }
    }

    /// Re-send the active sequence — Pi's `setInterval(..., TERMINAL_PROGRESS_KEEPALIVE_MS)`
    /// (`terminal.ts:514-516`). Driven from the run loop's 1 s ticker, gated on
    /// [`crate::TerminalProgress::keepalive`] so an idle session never writes.
    ///
    /// Also the resume path: a Ctrl+Z suspend runs [`Self::restore`], which clears the terminal's
    /// indicator, and the next tick after `fg` puts it back for a turn that is still running.
    pub fn tick_terminal_progress_keepalive(&mut self) {
        if self.state.terminal_progress.keepalive() {
            crate::write_terminal_progress(true);
        }
    }

    /// The exit clear — Pi `stop()` (`interactive-mode.ts:6041-6043`) and `ProcessTerminal.stop()`
    /// (`terminal.ts:407-409`). Answers from the TERMINAL's armed bit, so an indicator this process
    /// lit is always taken back down even if the setting was turned off in between.
    pub fn clear_terminal_progress_on_exit(&mut self) {
        if self.state.terminal_progress.shutdown() {
            crate::write_terminal_progress(false);
        }
    }

    /// Immutable state access.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Mutable state access (drive the transcript/editor/status directly).
    pub fn state_mut(&mut self) -> &mut AppState {
        &mut self.state
    }

    /// Install the extension-registered keyboard shortcuts (R-08-017; delegates to
    /// [`AppState::set_extension_shortcuts`]) VERBATIM, with no conflict gate.
    ///
    /// Production installs go through [`Self::install_extension_shortcuts`], which is the gated
    /// form; this one is the raw seam a test (or an embedder with no extension host) drives
    /// directly, the way pi's own `getShortcuts` tests build a runner and read its map back
    /// (`test/extensions-runner.test.ts:176-358` @v0.84.4).
    pub fn set_extension_shortcuts(
        &mut self,
        specs: impl IntoIterator<Item = impl Into<ShortcutSpec>>,
    ) {
        self.state.set_extension_shortcuts(specs);
    }

    /// Every live binding table as upstream's `KeybindingsConfig` — `action id -> key specs`, pi
    /// `KeybindingsManager.getEffectiveConfig()` (`core/keybindings.ts` @v0.84.4).
    ///
    /// pi keeps ONE `KeybindingsManager` over every registered definition, so its config is
    /// naturally whole; cyrup splits the same ids across a table per focus context, so the whole is
    /// the concatenation. Order does not matter to the only consumer — EXT-039's gate inverts it
    /// key-first and lets the RESERVED id win any tie (`extensions/runner.ts:104-106`).
    ///
    /// [`crate::keymap::AutocompleteKeymap`] is deliberately absent: the popup has no ids of its
    /// own (it reuses `tui.select.*` and `tui.input.tab` — see
    /// [`crate::keymap::AutocompleteAction::from_id`]), so every id it could contribute is
    /// already contributed by the select and editor maps, and adding it would only duplicate rows.
    pub fn effective_keybindings(&self) -> Vec<(String, Vec<String>)> {
        let mut out = self.state.keymap.effective_config();
        out.extend(self.state.select_keymap.effective_config());
        out.extend(self.state.tree_keymap.effective_config());
        out.extend(self.state.session_keymap.effective_config());
        out.extend(self.state.thinking_keymap.effective_config());
        out.extend(self.state.models_keymap.effective_config());
        out.extend(self.state.editor.keymap_ref().effective_config());
        out.extend(self.alt_keymap.effective_config());
        out
    }

    /// Resolve an extension host's registered shortcuts against the live keybindings and install
    /// the survivors — EXT-039, pi `setupExtensionShortcuts`
    /// (`modes/interactive/interactive-mode.ts:2078-2131` @v0.84.4), whose first statement is
    /// `const shortcuts = extensionRunner.getShortcuts(this.keybindings.getEffectiveConfig());`.
    ///
    /// This is the gate cyrup had built and never called: `resolve_shortcut_specs` REFUSES a key
    /// bound to a reserved built-in (Ctrl+C, Enter, …), lets a non-reserved override through with a
    /// warning, and records both — so a refused key is neither dispatched nor listed by `/hotkeys`,
    /// instead of being advertised and permanently dead. The warnings are read back with
    /// `ExtensionHost::shortcut_diagnostics()` and belong in the `[Extension issues]` startup panel
    /// (`interactive-mode.ts:1884-1886`).
    ///
    /// Call it wherever either input changes: at boot after `keybindings.json` is loaded, and after
    /// a session swap brings a new host.
    pub fn install_extension_shortcuts(&mut self, host: &cyrup_ext::ExtensionHost) {
        let effective = self.effective_keybindings();
        self.state
            .set_extension_shortcuts(host.resolve_shortcut_specs(&effective));
    }

    /// Plumb the `autocompleteMaxVisible` setting (Pi, item #6) into the editor's autocomplete popup
    /// (clamped 3–20). The binary calls this from `settings.autocompleteMaxVisible` at boot.
    pub fn set_autocomplete_max_visible(&mut self, n: u16) {
        self.state.editor.set_autocomplete_max_visible(n);
    }

    /// CFG-063 — install the render-debug instruments (`CYRUP_TUI_DEBUG`,
    /// `CYRUP_TUI_DEBUG_REDRAW`). The composition root resolves them with
    /// [`RenderDebug::from_env`] against the agent directory, pi's `logDirectory: getAgentDir()`
    /// (`interactive-mode.ts:581` @v0.87.1).
    pub fn set_render_debug(&mut self, debug: RenderDebug) {
        self.render_debug = debug;
    }

    /// Whether the idle 2-row status band is reserved (kept present) to avoid an editor/footer reflow
    /// when a spinner appears (item #9). Plumbed from Pi's `terminal.clearOnShrink` setting
    /// (interactive-mode.ts:1638-1642: an idle status container is cleared only when clearOnShrink is
    /// off — so `reserve_status_rows == clearOnShrink`). Default `false` matches Pi's default.
    pub fn set_reserve_status_rows(&mut self, reserve: bool) {
        self.state.reserve_status_rows = reserve;
    }

    /// Load a user `keybindings.json` document and merge it into every live keymap (R-10-018; Pi
    /// `KeybindingsManager.create`, keybindings.ts:348-352). Each map's `merge_json` applies only the
    /// ids in its own namespace (`app.*` / `tui.editor.*` / `tui.select.*` / `app.tree.*`) and ignores the
    /// rest, so one document configures the global, editor, selector and tree maps in a single pass.
    /// A malformed DOCUMENT (unparseable JSON, or a non-object top level) is surfaced as a typed
    /// error and nothing is applied — Pi's `loadRawConfig` returning `undefined`
    /// (`core/keybindings.ts:328-336` @v0.83.0). An individual bad ENTRY is not an error: it comes
    /// back in the returned [`KeybindingIssue`] list and every other entry still applies, so the
    /// binary can name the offending ids instead of claiming it ignored a file it half-applied
    /// (CFG-038). Never a panic.
    ///
    /// The issue lists of all six maps are concatenated rather than short-circuited, for the same
    /// reason: `?` between the maps used to leave the global keymap applied and the editor keymap
    /// untouched whenever a later map rejected something.
    pub fn load_keybindings_json(&mut self, json: &str) -> Result<Vec<KeybindingIssue>, TuiError> {
        let mut issues = self.state.keymap.merge_json(json)?;
        // X9 — every `… to expand` hint resolves its key label through the LIVE keymap upstream
        // (`keyText("app.tools.expand")`, `keybinding-hints.ts:34-36`). The transcript holds no
        // keymap, so the resolved label is pushed to it whenever bindings change.
        let expand = self.state.keymap.keys_label(Action::ToolsExpand);
        self.state.transcript.set_expand_hint(expand);
        issues.extend(self.state.select_keymap.merge_json(json)?);
        issues.extend(self.state.tree_keymap.merge_json(json)?);
        issues.extend(self.state.session_keymap.merge_json(json)?);
        issues.extend(self.state.thinking_keymap.merge_json(json)?);
        issues.extend(self.state.models_keymap.merge_json(json)?);
        issues.extend(self.state.editor.merge_keybindings_json(json)?);
        // TUI-109 — the `tui.altScreen.*` table was never merged, so `keybindings.json` could not
        // rebind any of the eight ids (the gap `alt_keymap`'s doc recorded). The "Jump to latest
        // message" label prints `tui.altScreen.bottom`, so a rebind has to reach both the key
        // routing and the live label.
        issues.extend(self.alt_keymap.merge_json(json)?);
        self.push_scroll_to_end_key();
        Ok(issues)
    }

    /// Push the current `tui.altScreen.bottom` label into the live alternate screen — pi's
    /// `keyDisplayText("tui.altScreen.bottom")` (`tui-renderer.ts:30`), which upstream re-reads on
    /// every frame. [`AltScreen`] owns no keymap, so this is the same push `set_expand_hint` is for
    /// the transcript. A no-op in inline mode; `adopt_fullscreen_renderer` pushes again when a
    /// renderer is built.
    pub(crate) fn push_scroll_to_end_key(&mut self) {
        let key = self.scroll_to_end_key();
        if let Some(alt) = self.altscreen.as_mut() {
            alt.set_scroll_to_end_key(key);
        }
    }

    /// `keyDisplayText("tui.altScreen.bottom")`: every bound key, title-cased and joined with `/`,
    /// or `None` when the action is unbound.
    pub(crate) fn scroll_to_end_key(&self) -> Option<String> {
        self.alt_keymap
            .keys_label(crate::keymap::AltScreenAction::Bottom)
            .map(|label| crate::chrome::format_key_text(&label, true))
    }

    /// TUI-051 — re-read `<agent_dir>/keybindings.json` and re-apply it to every live map.
    ///
    /// Pi calls `this.keybindings.reload()` inside `handleReloadCommand`, immediately after
    /// `await this.session.reload(...)` (`interactive-mode.ts:5386` @v0.83.0) →
    /// `core/keybindings.ts:354-357` `setUserBindings(KeybindingsManager.loadFromFile(configPath))`
    /// → `loadFromFile` (`:363-367`) re-reads the file, re-runs `migrateKeybindingsConfig` and hands
    /// the result to `packages/tui/src/keybindings.ts:167-192` `rebuild()`.
    ///
    /// cyrup's `/reload` never touched the file — while both the command's help string
    /// (`commands.rs`) and the handler's own comment claimed it did — so the single documented way
    /// to apply an edited `keybindings.json` was a process restart, which nothing told the user.
    ///
    /// **Reset-then-merge, not merge**: `rebuild()` REPLACES (`keybindings.ts:187-191`), so an entry
    /// the user deleted must go back to its default. A missing file is not an error — it means "no
    /// user bindings", i.e. every default (Pi's `loadFromFile` returns `{}` for one).
    ///
    /// Returns the entries the reloaded document could not use (CFG-038), so `/reload` can name
    /// them the same way startup does.
    pub fn reload_keybindings_from(
        &mut self,
        agent_dir: &std::path::Path,
    ) -> Result<Vec<KeybindingIssue>, TuiError> {
        let path = agent_dir.join("keybindings.json");
        let json = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            // No file ⇒ defaults only, which the reset below already produces.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::from("{}"),
            Err(e) => return Err(TuiError::Backend(e.to_string())),
        };
        self.state.keymap = Keymap::default();
        self.state.select_keymap = crate::keymap::SelectKeymap::default();
        self.state.tree_keymap = crate::keymap::TreeKeymap::default();
        self.state.session_keymap = crate::keymap::SessionKeymap::default();
        self.state.thinking_keymap = crate::keymap::ThinkingKeymap::default();
        self.state.models_keymap = crate::keymap::ModelsKeymap::default();
        self.state.editor.reset_keybindings_to_defaults();
        self.alt_keymap = AltScreenKeymap::default();
        self.load_keybindings_json(&json)
    }

    /// The transcript view.
    pub fn transcript_mut(&mut self) -> &mut TranscriptView {
        &mut self.state.transcript
    }

    /// The input editor.
    pub fn editor_mut(&mut self) -> &mut InputEditor {
        &mut self.state.editor
    }

    /// The status line.
    pub fn status_mut(&mut self) -> &mut StatusLine {
        &mut self.state.status
    }

    /// Point the footer's git-branch source at `cwd` and publish the branch it finds — Pi's
    /// `new FooterDataProvider(cwd)` followed by the footer's `getGitBranch()`
    /// (`footer-data-provider.ts`, consumed at `footer.ts:116-120`).
    ///
    /// This is the ONLY producer of [`StatusLine::branch`] in the binary: without it the `(branch)`
    /// segment of the location line can never appear, because nothing else resolves a git HEAD.
    /// Called once from the bin's footer seeding, before the first frame.
    pub fn set_footer_git_cwd(&mut self, cwd: &std::path::Path) {
        self.state.git_branch = crate::footer_data::FooterGitBranch::discover(cwd);
        let branch = self.state.git_branch.branch().map(str::to_string);
        self.state.status.set_branch(branch);
    }

    /// Install the channel the detached startup package-update check answers on — Pi fires that
    /// check from `run()` and shows the notification whenever it settles
    /// (`interactive-mode.ts:850-861`, `:3920-3936`).
    ///
    /// Must be called before [`App::run`]; the binary passes the receiver
    /// `cyrup::update_check::spawn_package_update_check` returns, which is `None` when the
    /// [`NetworkPolicy`](cyrup_config::policy::NetworkPolicy) declined — and then no arm exists.
    pub fn set_package_update_channel(
        &mut self,
        rx: Option<tokio::sync::mpsc::UnboundedReceiver<Vec<String>>>,
    ) {
        self.package_update_rx = rx;
    }

    /// Install the channel the terminal's colours arrive on once the boot query has given up on
    /// them — the receiving half for [`App::run`], the sending half for the callback handed to
    /// [`ThemeController::request_terminal_colors`] and for a later re-query.
    ///
    /// Must be called before [`App::run`]. Without it a colour reply that misses the boot deadline
    /// is swallowed by the input reader and the system theme stays as the timeout left it.
    pub fn set_terminal_colors_channel(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<crate::TerminalColors>,
        rx: tokio::sync::mpsc::UnboundedReceiver<crate::TerminalColors>,
    ) {
        self.terminal_colors_tx = Some(tx);
        self.terminal_colors_rx = Some(rx);
    }

    /// Ask the terminal for its colours again without waiting — the `queryTerminalColors()` pi's
    /// `applyFromSettings` ends with (`theme-controller.ts:108`). The input reader thread owns
    /// stdin by now, so the answer is routed to the pending query by the reader and delivered on
    /// the channel [`Self::set_terminal_colors_channel`] installed. A no-op off a real terminal
    /// and when no channel was installed.
    #[cfg(unix)]
    pub(crate) fn requery_terminal_colors(&self) {
        #[cfg(test)]
        REQUERIES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(tx) = self.terminal_colors_tx.clone() {
            crate::terminal_query::request_terminal_colors_async(
                crate::write_log::tui_stdout(),
                std::sync::Arc::new(move |colors| {
                    let _ = tx.send(colors);
                }),
            );
        }
    }

    /// Windows has no byte reader to route a reply through, so there is nothing to ask.
    #[cfg(not(unix))]
    pub(crate) fn requery_terminal_colors(&self) {}

    /// Re-check the git refs and republish the branch when it moved — Pi's watch-driven
    /// `refreshGitBranchAsync` → `notifyBranchChange` (`footer-data-provider.ts`), driven here by
    /// [`App::run`]'s poll tick. Returns `true` when the footer needs a repaint.
    pub fn poll_footer_git_branch(&mut self) -> bool {
        if !self.state.git_branch.poll() {
            return false;
        }
        let branch = self.state.git_branch.branch().map(str::to_string);
        self.state.status.set_branch(branch);
        true
    }

    /// The terminal (test access to the rendered buffer via `terminal.backend()`).
    pub fn terminal(&self) -> &Terminal<B> {
        &self.terminal
    }

    /// The committed scrollback lines already emitted via `insert_before` (test/inspection access).
    #[cfg(any(test, feature = "scrollback-accumulator"))]
    pub fn scrollback_lines(&self) -> &[Line<'static>] {
        &self.state.scrollback
    }

    /// The current inline-viewport (live-region) height in rows — the bottom band of the screen the
    /// app repaints each frame (ADR-0001 #1). Committed history scrolls *above* this band into native
    /// scrollback; tests use it to read only the live region (the bottom `viewport_height` rows).
    pub fn viewport_height(&self) -> u16 {
        self.viewport_height
    }

    /// The committed scrollback content as text, one entry per line (test/inspection access). This is
    /// the exact payload `Terminal::insert_before` received, so tests can assert finalized turns
    /// reached native scrollback without driving a real terminal.
    #[cfg(any(test, feature = "scrollback-accumulator"))]
    pub fn scrollback_text(&self) -> String {
        self.state
            .scrollback
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Attach a decoded image to the next prompt (rendered inline above the editor, spec/tui/06 §6;
    /// `components/image.ts`). The `@`-mention of an image file and clipboard-image paste both land here.
    pub fn attach_image(&mut self, image: ImageBlock) {
        self.state.pending_images.push(image);
    }

    /// Attach an image file by path (the `@`-mention image source); a no-op (returns `false`) when the
    /// path is not a decodable image, so a stray mention never disrupts the prompt.
    pub fn attach_image_path(&mut self, path: &std::path::Path) -> bool {
        match ImageBlock::from_path(path) {
            Some(block) => {
                self.state.pending_images.push(block);
                true
            }
            None => false,
        }
    }

    /// Insert the temp-file PATH of a pasted clipboard image at the editor cursor as ordinary text —
    /// Pi's literal mechanism (`this.editor.insertTextAtCursor(filePath)`,
    /// interactive-mode.ts:2552). The bare path becomes editable text and, on submit, rides the
    /// outgoing user message AS TEXT (no image content block): the agent loads the raster on demand
    /// via a file-read tool, so a potentially huge image never floods context — Pi's deliberate
    /// context-economy choice, which the former `pending_images` embed here violated. Kept separate
    /// from the clipboard read so the path→editor step is unit-testable without a live system
    /// clipboard (`try_paste_clipboard_image_path` supplies the path in the binary).
    pub(crate) fn insert_clipboard_image_path(&mut self, path: &std::path::Path) {
        self.state.editor.insert_str(&path.to_string_lossy());
    }

    /// Pi `handleClipboardPaste` (`interactive-mode.ts:2870-2892` @v0.84.2): read an **image**
    /// first and, only when there is none, read **text** — both inserted at the editor cursor with
    /// `insertTextAtCursor`. Returns whether anything was pasted.
    ///
    /// The two clipboard reads are passed as closures rather than performed here so the ORDER is a
    /// unit-testable fact without a live system clipboard: pi's text read is lazy — it never runs
    /// when an image was found (`:2882` returns before `:2884`) — and a version that read both up
    /// front would pass an equality assertion while diverging on a clipboard holding both.
    pub(crate) fn paste_from_clipboard(
        &mut self,
        image: impl FnOnce() -> Option<std::path::PathBuf>,
        text: impl FnOnce() -> Option<String>,
    ) -> bool {
        // `const image = await readClipboardImage(); if (image) { … return; }` (`:2872-2882`).
        if let Some(path) = image() {
            self.insert_clipboard_image_path(&path);
            return true;
        }
        // `const text = await readClipboardText(); if (text) { this.editor.insertTextAtCursor(text) }`
        // (`:2884-2888`). DRIFT-045: this branch did not exist, so a Ctrl+V over a clipboard
        // holding text inserted nothing at all — against a help table that advertises
        // "Paste image or text from clipboard" (`:2101`).
        if let Some(text) = text().filter(|t| !t.is_empty()) {
            self.state.editor.insert_str(&text);
            return true;
        }
        false
    }

    /// Read a system-clipboard image, materialize it to a `cyrup-clipboard-<uuid>.png` temp file, and
    /// insert its PATH as text at the editor cursor; failing that, insert the clipboard's TEXT
    /// (Pi `handleClipboardPaste`, interactive-mode.ts:2870-2892). Returns `true` when something
    /// was pasted; `false` when the clipboard holds neither an image nor text, or on any
    /// clipboard/encode/IO error — so the caller still lets Ctrl+V fall through to the editor.
    pub(crate) fn try_paste_clipboard_image_path(&mut self) -> bool {
        self.paste_from_clipboard(
            read_clipboard_image_to_temp,
            crate::clipboard::read_clipboard_text,
        )
    }

    /// Clear all attached images (after the prompt is sent, or on `Esc`).
    pub fn clear_images(&mut self) {
        self.state.pending_images.clear();
    }

    /// The images attached to the next prompt (test/inspection access).
    pub fn pending_images(&self) -> &[ImageBlock] {
        &self.state.pending_images
    }

    /// Env-sniff the controlling terminal's capabilities (feature #7; Pi `detectCapabilities`) and
    /// upgrade the portable half-block default to the negotiated image protocol (Kitty/iTerm2), while
    /// caching the resolved [`TerminalCapabilities`] so the OSC-8 hyperlink gate (feature #8) can read
    /// them. Called by the binary at startup; tests keep the half-block default (the inline path still
    /// renders to `TestBackend`).
    pub fn detect_image_support(&mut self) {
        // CFG-090 — Pi's `getCapabilities()` detects UNDER the settings overrides
        // (`terminal-image.ts:160-169` @v0.87.1), which the launcher set first (`main.ts:853`).
        let caps = crate::image::detect_capabilities_with_settings_overrides();
        // Seed the process-wide OSC-8 answer the markdown renderer reads (Pi's cached
        // `getCapabilities()`, terminal-image.ts:138-143) so the link gate at `markdown.ts:692`
        // sees the same detection this call already paid for.
        // TUI-N12 — seed the WHOLE record, not just `hyperlinks`: the cache now carries `images`
        // and `true_color` too, and this call site already holds all three.
        crate::image::seed_capabilities(caps);
        // …and, when the terminal HAS an image protocol, measure its font cell instead of guessing
        // it (Pi `queryCellSize`, `tui.ts:647`/`:679-686`, gated on `getCapabilities().images` at
        // `:681`). Without this every inline image is laid out against `ratatui-image`'s `10x20`
        // placeholder cell, so a Kitty/iTerm2 image that is not width-clamped reserves the wrong
        // number of rows and is drawn at the wrong scale.
        //
        // Called by the binary from the SAME pre-reader-thread window as the theme probe (see
        // `crate::terminal_query`'s module docs for the timeout / input-safety contract); off a real
        // terminal `stdin_is_queryable` short-circuits it to `None` in microseconds, which is what
        // keeps this callable from tests.
        let cell_size = if caps.images.is_some() {
            use crate::terminal_query::TerminalProbe as _;
            crate::terminal_query::StdinTerminalProbe
                .query_cell_size(crate::terminal_query::CELL_SIZE_TIMEOUT)
        } else {
            None
        };
        self.publish_capabilities(caps, cell_size);
    }

    /// Re-apply `terminal.{images,trueColor,hyperlinks}` from the (re)loaded settings — Pi
    /// `applyRuntimeSettings()`'s `setCapabilityOverrides(this.settingsManager
    /// .getTerminalCapabilityOverrides())` (`interactive-mode.ts:1992-1993` @v0.87.1), run on every
    /// session rebind and `/reload`. An unchanged set is a no-op; a changed one drops the cache
    /// (pi `:175-186`), and because cyrup derives the image renderer and the transcript's image and
    /// OSC-8 gates from the capabilities once rather than per paint, they are re-derived here. The
    /// font cell is not re-queried — pi measures it once at TUI start (`tui.ts:679-686`) — so a
    /// cell measured earlier is kept. CFG-090.
    pub fn apply_terminal_capability_overrides(
        &mut self,
        overrides: cyrup_config::settings::TerminalCapabilityOverrides,
    ) {
        if !crate::image::set_capability_overrides(
            crate::image::CapabilityOverrides::from_settings(overrides),
        ) {
            return;
        }
        let caps = crate::image::detect_capabilities_with_settings_overrides();
        crate::image::set_capabilities(caps);
        let cell_size = self
            .state
            .image_renderer
            .is_graphical()
            .then(|| self.state.image_renderer.cell_pixels());
        self.publish_capabilities(caps, cell_size);
        // pi's next `createTheme` reads `getCapabilities().trueColor` (`theme.ts:529` @v0.87.1);
        // the controller carries the mode every later theme application projects through.
        let color_mode = ColorMode::from_true_color(caps.true_color);
        self.state.color_mode = color_mode;
        if let Some(controller) = self.state.theme_controller.as_mut() {
            controller.set_color_mode(color_mode);
        }
    }

    /// Publish resolved capabilities to every consumer that derives from them once: the app's
    /// record, the image renderer, and the transcript's image and OSC-8 gates.
    fn publish_capabilities(&mut self, caps: TerminalCapabilities, cell_size: Option<(u16, u16)>) {
        self.state.capabilities = caps;
        self.state.image_renderer =
            ImageRenderer::from_capabilities_with_cell_size(caps, cell_size);
        // TUI-N01 / TUI-036 — publish the capability where the two consumers can reach it: the
        // transcript's tool-result image gate (Pi `tool-execution.ts:331`) and the `/settings` grid
        // builder, which must not offer image rows on a terminal with no protocol
        // (`settings-selector.ts:654-671`). `AppState::image_renderer` is not reachable from either.
        self.state
            .transcript
            .set_graphical_images(self.state.image_renderer.is_graphical());
        // Feature #8 / TUI-020 — the same publish for the OSC-8 gate `tool_path_span` reads, so a
        // `read`/`write`/`edit`/`ls` header path becomes a clickable `file://` target on a terminal
        // that forwards hyperlinks.
        self.state.transcript.set_hyperlinks(caps.hyperlinks);
    }

    /// Apply a new theme, bumping its generation so caches invalidate (R-10-026). The theme is
    /// re-projected through the app's live [`ColorMode`] (feature #3/#4) so a `/theme` switch or hot
    /// reload on a 256-color terminal keeps indexed colors (`with_color_mode` is idempotent for an
    /// already-projected theme).
    pub fn set_theme(&mut self, theme: UiTheme) {
        let mut theme = theme.with_color_mode(self.state.color_mode);
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// Boot the render theme from a [`ThemeController`] (feature #4): adopt the controller's resolved
    /// color mode and set the projected theme. This is the seam the binary uses to honor
    /// `settings.theme` + the terminal background at startup instead of the hardwired dark boot.
    pub fn apply_theme_controller(&mut self, controller: &ThemeController) {
        self.state.color_mode = controller.color_mode();
        let mut theme = controller.theme();
        theme.generation = self.state.theme.generation.saturating_add(1);
        self.state.theme = theme;
    }

    /// The theme a picker preview or confirm paints for `setting` — pi's `themeController.preview`
    /// (`theme-controller.ts:134-141`): `resolveThemeSetting(setting, getTerminalTheme()) ??
    /// activeThemeName`, so a `light/dark` pair previews the half the terminal's appearance picks.
    /// The name then loads as the controller loads it — `system` generated from the colours the
    /// terminal reported, which only the controller holds; a discovered theme from its document;
    /// else a compiled-in one. Without a controller (a harness app) there are no reported colours,
    /// and the compiled-in answer stands.
    pub(crate) fn theme_for_picker(&self, setting: &str) -> UiTheme {
        let controller = self.state.theme_controller.as_ref();
        let terminal = controller.map_or(crate::TerminalTheme::Dark, |c| c.terminal_theme());
        let name =
            crate::theme::resolve_theme_setting(Some(setting), terminal).unwrap_or_else(|| {
                controller.map_or_else(
                    || self.state.theme.name.clone(),
                    |c| c.active_name().to_string(),
                )
            });
        if name != crate::system_theme::SYSTEM_THEME_NAME
            && let Some(data) = self
                .state
                .theme_access
                .as_ref()
                .and_then(|access| access.document(&name))
        {
            return UiTheme::from_theme_data(&data, 0).with_terminal_appearance(terminal.into());
        }
        match controller {
            Some(controller) => controller.theme_named(&name),
            None => UiTheme::builtin(&name),
        }
    }

    /// Pi `getThemeSelection() || SYSTEM_THEME_NAME` (`interactive-mode.ts:4847`): the theme setting
    /// in force, which the Theme row shows and the theme submenu opens on.
    pub(crate) fn theme_selection(&self) -> String {
        match self.state.theme_controller.as_ref() {
            Some(controller) => controller.theme_selection().to_string(),
            None => self.state.theme.name.clone(),
        }
    }

    /// Pi `getAvailableThemes()` (`interactive-mode.ts:4849`): the names the theme picker offers.
    pub(crate) fn available_theme_names(&self) -> Vec<String> {
        match self.state.theme_access.as_ref() {
            Some(access) => access.available_names(),
            None => vec![
                crate::system_theme::SYSTEM_THEME_NAME.to_string(),
                "dark".to_string(),
                "light".to_string(),
            ],
        }
    }

    /// TUI-004 — hand the app the boot [`ThemeController`] so a session swap can re-run Pi's
    /// `applyFromSettings` (`modes/interactive/theme/theme-controller.ts:57-81` @v0.84.4).
    ///
    /// Upstream's controller is a FIELD of the interactive mode (`interactive-mode.ts:960`), which
    /// is why its `setRebindSession` hook can reach it (`:576-579`); cyrup's lived in the
    /// composition root's stack frame and was consulted exactly once, at boot, so `/reload` re-read
    /// five other settings rows and left the theme alone. The controller is CLONED in, not borrowed:
    /// the launcher still needs `active_name()` to bind the theme file watcher, and the app owns
    /// every mutation from here on.
    pub fn set_theme_controller(&mut self, controller: ThemeController) {
        self.state.theme_controller = Some(controller);
    }

    /// TUI-004 — re-resolve and re-apply the render theme from a swapped-in session's freshly
    /// re-read `settings.theme`, loading the named theme out of THAT session's freshly discovered
    /// resources. Pi's `setRegisteredThemes(resourceLoader.getThemes().themes)` +
    /// `await this.themeController.applyFromSettings()` pair (`interactive-mode.ts:1977`/`:5985` and
    /// `:578`/`:5987` @v0.84.4).
    ///
    /// Unconditional, exactly as upstream's is: the name may be unchanged while the theme FILE it
    /// names has been rewritten — the case `/reload` exists for — so the load is redone every time
    /// rather than gated on the name differing (see [`ThemeController::apply_from_settings`]).
    ///
    /// `resources` is the session's whole discovered set, so a file-backed custom theme resolves
    /// here exactly as it does for an extension's `getTheme` ([`crate::theme_access::TuiThemeAccess`]
    /// answers from the same [`cyrup_resources::ResourceSet`]).
    ///
    /// TUI-096 — a name that resolves to NOTHING is pi's `setTheme` failure, and upstream does two
    /// things with it (`applyThemeName`, `theme-controller.ts:178-186` @v1.0.0), not one:
    ///
    /// 1. `this.activeThemeName = result.success ? themeName : SYSTEM_THEME_NAME` — the ACTIVE name
    ///    becomes the theme that is actually painted ([`ThemeController::fall_back_to_system`]);
    ///    and
    /// 2. under `showError` — which is `true` whenever the setting names a theme (`:107`), i.e. on
    ///    every path that reaches here — it surfaces
    ///    ``Failed to load theme "<name>": <error>\nFell back to the system theme.``
    ///
    /// `<error>` is pi's `result.error`: for a name nothing resolves, `loadThemeJson`'s
    /// `Theme not found: <name>` (`theme/theme.ts:567`, caught by `setTheme` at `:783-792`). The
    /// sentence goes to
    /// [`crate::transcript::TranscriptView::push_error`], which is `showError`'s
    /// `Spacer(1)` + `Text(theme.fg("error", …), outputPad, 0)`
    /// (`interactive-mode.ts:4258-4262`) — with the `Error: ` prefix supplied here, since that
    /// entry renders verbatim (see `transcript/render.rs`'s `Entry::Error` arm).
    ///
    /// cyrup has ONE `<error>` string where pi can have two, and the reason is where the parse
    /// happens: a malformed theme file is rejected by `cyrup-resources` during discovery
    /// (`discovery/scan.rs:551-558` turns the `Theme::load` error into a `ResourceWarning`, which
    /// the startup diagnostics panel already renders) and therefore never reaches the registry, so
    /// by the time this seam looks the file is indistinguishable from a deleted one and reads
    /// `Theme not found`. pi parses lazily inside `setTheme` and would print
    /// `Failed to parse theme <label>: …` (`theme/theme.ts:600-604`) for that case. The user is
    /// told either way, once here and once in the panel.
    ///
    /// A no-op when no controller was handed over — an app the composition root did not boot has no
    /// `settings.theme` to answer from and keeps the theme it was constructed with.
    pub fn reapply_theme_from_settings(
        &mut self,
        setting: Option<&str>,
        resources: &cyrup_resources::ResourceRegistry,
    ) -> ThemeApply {
        self.apply_theme_setting(setting, resources, true)
    }

    /// The boot-time `applyFromSettings` (`interactive-mode.ts:990`): [`Self::reapply_theme_from_settings`]
    /// without the colour re-query, because the composition root has just asked the terminal for
    /// them synchronously (`ThemeController::request_terminal_colors`) and asking again would
    /// only repeat the question.
    pub fn settle_boot_theme(
        &mut self,
        setting: Option<&str>,
        resources: &cyrup_resources::ResourceRegistry,
    ) -> ThemeApply {
        self.apply_theme_setting(setting, resources, false)
    }

    fn apply_theme_setting(
        &mut self,
        setting: Option<&str>,
        resources: &cyrup_resources::ResourceRegistry,
        requery_colors: bool,
    ) -> ThemeApply {
        let Some(controller) = self.state.theme_controller.as_mut() else {
            return ThemeApply::NoController;
        };
        let name = controller.apply_from_settings(setting);
        let loaded = Self::load_theme_named(controller, resources, &name);
        let (projected, outcome) = match loaded {
            Some(theme) => (theme, ThemeApply::Loaded(name)),
            None => (
                controller.fall_back_to_system(),
                ThemeApply::FellBackToSystem {
                    error: format!("Theme not found: {name}"),
                    name,
                },
            ),
        };
        // `set_theme`, not a bare assignment: it re-projects through the app's live `ColorMode` and
        // bumps the generation, which is what invalidates the render caches (`notifyChanged` →
        // `ui.invalidate()`, `theme-controller.ts:249-252`).
        self.set_theme(projected);
        if let ThemeApply::FellBackToSystem { name, error } = &outcome {
            self.state.transcript.push_error(format!(
                "Error: Failed to load theme \"{name}\": {error}\n{}",
                crate::theme::THEME_FALLBACK_SENTENCE
            ));
        }
        // `applyFromSettings` opens with `setAutoSync(…)` (`theme-controller.ts:104-106`).
        self.sync_color_scheme_notifications();
        // …and ends by asking the terminal for its colours again
        // (`theme-controller.ts:108`), so a terminal theme the user switched since boot reaches the
        // system theme and the `""` tokens.
        if requery_colors {
            self.requery_terminal_colors();
        }
        outcome
    }

    /// pi's `loadTheme(name)` (`theme.ts:631-640`) for the controller's owner: `system` first — it
    /// "is reserved: it takes precedence over custom themes of the same name" and is generated from
    /// the colours the terminal reported, which only the controller holds — then the session's
    /// discovered themes, then the compiled-in built-ins. `None` from all three is pi's `setTheme`
    /// throw, the failure the caller owes the user a sentence for.
    ///
    /// [CYRUP-DELTA] vs `loadThemeJson`, which checks `if (name in builtinThemes)` FIRST
    /// (`theme/theme.ts:552-554` @v1.0.0) and so shadows a user theme that reuses a built-in name.
    /// Discovery already seeds the registry with the built-ins, so the two orders differ only for
    /// that shadowing case, and cyrup resolves it the other way everywhere else it resolves a theme
    /// by name — `TuiThemeAccess::get`/`set` (`theme_access.rs`) and the boot theme-file watcher
    /// (`crates/cyrup/src/interactive.rs`'s `build_theme_watcher`) both go through
    /// `ResourceSet::get_name`. Re-ordering HERE alone would repaint one theme while the watcher
    /// watched another's file. `builtin_named` is therefore the fallback, which is what a registry
    /// that discovered nothing (a harness `ResourceRegistry::default()`) needs to keep `dark`/`light`
    /// loadable. Pinned by
    /// `tests::theme_reapply_on_reload::a_discovered_theme_that_shadows_a_builtin_name_beats_the_builtin`,
    /// which is red under the upstream order and is the ONLY test that can tell the two apart.
    fn load_theme_named(
        controller: &ThemeController,
        resources: &cyrup_resources::ResourceRegistry,
        name: &str,
    ) -> Option<UiTheme> {
        if name == crate::system_theme::SYSTEM_THEME_NAME {
            return Some(controller.system_theme());
        }
        resources
            .themes
            .get_name(name)
            .map(|theme| UiTheme::from_theme_data(&theme.data, 0))
            .or_else(|| UiTheme::builtin_named(name))
            .map(|theme| theme.with_terminal_appearance(controller.terminal_theme().into()))
    }

    /// The terminal reported its colours after the boot query gave up — Pi's `onLateReply`
    /// (`theme-controller.ts:34`, `applyTerminalColors` `:197-211`): record them, regenerate the
    /// system theme (or switch the theme of an automatic pair) and repaint. A repeat of what the
    /// controller already holds changes nothing.
    pub fn apply_terminal_colors(
        &mut self,
        colors: crate::terminal_query::TerminalColors,
        resources: &cyrup_resources::ResourceRegistry,
    ) {
        let Some(controller) = self.state.theme_controller.as_mut() else {
            return;
        };
        let Some(name) = controller.apply_terminal_colors(colors) else {
            return;
        };
        let loaded = Self::load_theme_named(controller, resources, &name);
        // A name that no longer loads is pi's silent `applyThemeName(themeName)` failure
        // (`showError` is false on this path): the system theme is painted and seated.
        let theme = loaded.unwrap_or_else(|| controller.fall_back_to_system());
        self.set_theme(theme);
    }

    /// Paint the theme an extension's `setTheme` resolved to and say which name to persist.
    ///
    /// The generated `system` theme is not in the resources: it is built from the colours the
    /// controller holds (`setThemeName("system")`, `theme-controller.ts:124-131`), which also seats
    /// it as the active name. A discovered theme is projected from its document.
    pub(crate) fn apply_theme_switch(
        &mut self,
        switch: crate::theme_access::ThemeSwitch,
    ) -> String {
        let (name, projected) = match switch {
            crate::theme_access::ThemeSwitch::System => {
                let name = crate::system_theme::SYSTEM_THEME_NAME.to_string();
                let projected = match self.state.theme_controller.as_mut() {
                    Some(controller) => controller.set_theme_name(name.clone()),
                    None => UiTheme::builtin(&name),
                };
                (name, projected)
            }
            crate::theme_access::ThemeSwitch::Resource(theme) => {
                // `from_theme_data`, not `UiTheme::builtin`: the listing this name came from is
                // the session's whole discovered set, so a file-backed custom theme is
                // switchable exactly as upstream's is, and would otherwise silently render as
                // `dark` (`UiTheme::builtin`'s unknown-name fallback).
                let projected = UiTheme::from_theme_data(&theme.data, 0);
                (theme.key.as_str().to_string(), projected)
            }
        };
        self.set_theme(projected);
        name
    }

    /// Pi `setAutoSync` (`theme-controller.ts:225-229`): ask the terminal for appearance-change
    /// notifications (mode `2031`) exactly while the theme follows the terminal — an automatic
    /// `light/dark` pair or the system theme. A no-op without a controller (a harness `App`).
    pub(crate) fn sync_color_scheme_notifications(&self) {
        if let Some(controller) = self.state.theme_controller.as_ref() {
            crate::color_scheme::set_notifications(controller.auto_sync());
        }
    }

    /// The terminal reported a light/dark switch (`CSI ? 997 ; N n`) — pi's
    /// `applyTerminalColorSchemeChange` (`theme-controller.ts:240-248`): record the scheme, re-theme
    /// when it moved the terminal's appearance, then query the terminal's colours again; they
    /// decide the appearance for a terminal that reports its background, and regenerate the system
    /// theme from the new palette when they arrive.
    pub fn apply_color_scheme_report(
        &mut self,
        scheme: crate::TerminalTheme,
        resources: &cyrup_resources::ResourceRegistry,
    ) {
        let Some(controller) = self.state.theme_controller.as_mut() else {
            return;
        };
        if !controller.auto_sync() {
            return;
        }
        if let Some(name) = controller.apply_color_scheme(scheme) {
            let loaded = Self::load_theme_named(controller, resources, &name);
            let theme = loaded.unwrap_or_else(|| controller.fall_back_to_system());
            self.set_theme(theme);
        }
        self.requery_terminal_colors();
    }

    /// The boot [`ThemeController`] the composition root handed over, if any (test/inspection).
    /// `None` for a harness app, which has no `settings.theme` to answer from.
    pub fn theme_controller(&self) -> Option<&ThemeController> {
        self.state.theme_controller.as_ref()
    }

    /// The app's active color mode (test/inspection).
    pub fn color_mode(&self) -> ColorMode {
        self.state.color_mode
    }

    /// Point the automatic terminal title at the live session's working directory — Pi's
    /// `sessionManager.getCwd()` (`interactive-mode.ts:819`). Does NOT write anything on its own;
    /// [`Self::update_terminal_title`] is what recomputes the title.
    pub fn set_title_cwd(&mut self, cwd: PathBuf) {
        // X7 — the same value Pi hands the tool renderers as `ToolRenderContext.cwd`
        // (`tool-execution.ts:126`), which `read`'s compact classification resolves against.
        self.state.transcript.set_cwd(Some(cwd.clone()));
        self.state.title_cwd = cwd;
    }

    /// Recompute the automatic window title from the session name + cwd — Pi `updateTerminalTitle`
    /// (`interactive-mode.ts:818-826`) — and store it on [`AppState::terminal_title`].
    ///
    /// Returns the new title **only when it changed**, so a caller writes the OSC 0 sequence no more
    /// often than Pi calls `setTitle`. Pi's four call sites are startup (`:860`), a session
    /// (re-)bind (`:1761`), unbinding the extension set (`:1995`) and `session_info_changed`
    /// (`:2901`); [`App::run`] drives the first, second and fourth — the third has no cyrup
    /// counterpart, since extension chrome here is not torn down per session. Never per stream
    /// event. The write itself is the crossterm run loop's job
    /// ([`write_terminal_title`]), for the same reason the extension `SetTitle` effect is written
    /// there: a `TestBackend` app must not emit escape sequences onto the real stdout.
    ///
    /// The session name is read from the footer's [`StatusLine::session_name`], which is where the
    /// live value already lands (Pi reads the same value the footer does, `footer.ts:116-130`).
    pub fn update_terminal_title(&mut self) -> Option<String> {
        let title = session_terminal_title(
            self.state.status.session_name.as_deref(),
            &self.state.title_cwd,
        );
        if self.state.terminal_title.as_deref() == Some(title.as_str()) {
            return None;
        }
        self.state.terminal_title = Some(title.clone());
        Some(title)
    }
}

/// The inline renderer's half of the ADR-0005 §B-2 renderer seam
/// ([`crate::ViewportRenderer`], pi `tui.ts:322-330`).
///
/// Four of the five operations are no-ops, and that is upstream's shape rather than a shortcut:
/// `TuiMainScreen` implements `TUI` and NOT `ViewportTUI` (`tui-main-screen.ts:123`), so it has no
/// `setLayoutRoot`; it does not scroll, because the terminal's native scrollback holds the history
/// (R-ARCH-TUI-003 — the same reason [`App::draw`] flushes committed entries with
/// `Terminal::insert_before` instead of retaining them); and it has a status line, so it does not
/// flash. **Nothing about the inline renderer's behaviour changes by implementing this** — the
/// trait is purely additive, and [`App::new`] above stays the one construction path.
impl<B: Backend> crate::altscreen::ViewportRenderer for App<B> {
    /// Always [`crate::TuiRenderMode::Regular`] — pi's `TuiMainScreen` fixes
    /// `readonly mode = "regular"` on the class too (`tui-main-screen.ts:124`). `App` is the inline
    /// renderer; entering fullscreen swaps in the alternate-screen renderer rather than mutating
    /// this one (ADR-0005 §B-14), so there is no state for this to read.
    fn mode(&self) -> crate::altscreen::TuiRenderMode {
        crate::altscreen::TuiRenderMode::Regular
    }

    /// No-op: the inline layout is fixed by [`crate::render`], which composes the active region, the
    /// status band, the editor/selector slot and the footer out of [`AppState`]. There is no
    /// upstream counterpart to install — `TuiMainScreen` has no `setLayoutRoot`
    /// (`tui-main-screen.ts:123`).
    fn set_layout_root(&mut self, _root: Option<Box<dyn Component>>) {}

    /// No-op: native scrollback scrolls, not the viewport (R-ARCH-TUI-003).
    ///
    /// This is deliberately NOT routed to the active region's `PageUp`/`PageDown`. That surface is
    /// [`crate::TranscriptView`]'s own scroll offset over the LIVE region only, reached through the
    /// editor keymap; moving it from here would scroll a different thing than the one pi's
    /// `tui.altScreen.*` bindings name (`keybindings.ts:159-183`, ADR-0005 §B-9).
    fn scroll_by(&mut self, _lines: i32) {}

    /// No-op — see [`crate::ViewportRenderer::scroll_by`].
    fn scroll_to_top(&mut self) {}

    /// No-op — see [`crate::ViewportRenderer::scroll_by`].
    fn scroll_to_bottom(&mut self) {}

    /// No-op: inline, transient notices already go to the status line, which is why pi's `/copy`
    /// keeps its `showStatus` branch for the main screen and flashes only when
    /// `this.ui instanceof TuiAltScreen` (`interactive-mode.ts:6107-6112`, ADR-0005 §B-11).
    fn flash(&mut self, _message: &str, _duration: Option<Duration>) {}
}
